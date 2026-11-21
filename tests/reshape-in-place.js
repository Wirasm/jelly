// A shape updates open pages in place; see tests/README.md.
async page => {
  const base = new URL(page.url()).origin, project = `shape-${Date.now()}`, name = `${project}:fixture`;
  const assert = (ok, message) => { if (!ok) throw Error(message); };
  const post = async (path, data) => {
    const r = await page.request.post(base + path, {data: {...data, by: 'test'}});
    if (!r.ok()) throw Error(await r.text());
    return r.json();
  };
  // A tall page with a scrolled box and a text field, all with ids.
  const view = (label) => `<!doctype html><style>body{margin:0;font:16px system-ui} .row{height:60px} #box{height:100px;overflow:auto}</style>
    <h1 id="title">${label}</h1><div id="box">${'<p>line</p>'.repeat(12)}</div>
    <div>${'<div class="row">row</div>'.repeat(25)}</div><input id="note"><div>${'<div class="row">row</div>'.repeat(25)}</div>
    <p id="n"></p><script>jelly.onState((s) => { document.getElementById('n').textContent = 'n=' + s.n; });</script>`;
  let rev = 0;
  const shape = () => post(`/api/jellies/${name}/shape`, {view: view(`v${++rev}`)});
  await post('/api/jellies', {name, view: view('v0'), verbs: 'const verbs = { inc(s) { s.n++; } };', state: {n: 0}});

  // Scroll, focus the field and type, shape, and check what the new version kept.
  const results = [];
  const check = async (mode, frame, scroll, extra = async () => {}) => {
    await (await frame()).evaluate(() => { document.getElementById('box').scrollTop = 40; });
    await scroll();
    await (await frame()).locator('#note').fill('draft');
    await (await frame()).evaluate(() => document.getElementById('note').setSelectionRange(1, 3));
    const before = await scrollOf(mode);
    await page.evaluate(() => { window.watching = true; });
    await shape();
    await extra();
    const label = `v${rev}`;
    await shows(frame, label);
    await page.evaluate(() => { window.watching = false; });
    const kept = await (await frame()).evaluate(() => {
      const note = document.getElementById('note');
      return {focus: document.hasFocus() && document.activeElement.id, value: note.value, sel: [note.selectionStart, note.selectionEnd],
        box: document.getElementById('box').scrollTop};
    });
    assert(kept.focus === 'note', `${mode}: focus kept (${kept.focus})`);
    assert(kept.value === 'draft' && kept.sel.join() === '1,3', `${mode}: typed text and selection kept (${kept.value} ${kept.sel})`);
    assert(kept.box === 40, `${mode}: scrolled box kept (${kept.box})`);
    const after = await scrollOf(mode);
    assert(Math.abs(after - before) < 2 && before > 300, `${mode}: scroll kept (${before} -> ${after})`);
    results.push({mode, scroll: after, ...kept});
  };
  let scrollOf;
  // Waits until the visible copy shows `label`, re-finding it as frames are swapped.
  const shows = async (frame, label) => {
    for (let i = 0; i < 100; i++) {
      const text = await (async () => (await frame()).locator('#title').textContent({timeout: 100}))().catch(() => null);
      const settled = await page.evaluate(() => document.readyState === 'complete' && !document.querySelector('iframe.next')).catch(() => false);
      if (text === label && settled) return;
      await page.waitForTimeout(50);
    }
    throw Error(`never showed ${label}`);
  };

  // Standalone: the page reloads and carries scroll and focus through it.
  await page.goto(`${base}/j/${encodeURIComponent(name)}/`);
  scrollOf = () => page.evaluate(() => scrollY);
  await check('standalone', () => page, () => page.evaluate(() => scrollTo(0, 1200)));

  // The jelly page: a sampler checks, on every frame, that a visible copy of the
  // jelly never changes its document (a reload in place goes blank while the new
  // one loads) and only ever shows a loaded one.
  await page.goto(`${base}/p/${project}`);
  await page.locator('article .frame iframe').contentFrame().locator('#title').waitFor();
  await page.evaluate(() => {
    window.blanks = [];
    window.watching = false;
    const docs = new WeakMap();
    (function tick() {
      for (const f of document.querySelectorAll('iframe:not(.next)')) {
        const d = f.contentDocument, where = f.closest('.entry, .tile, #full')?.className;
        if (watching && !f.closest('[hidden]')) {
          if (docs.has(f) && docs.get(f) !== d) blanks.push(`${where}: reloaded in place`);
          if (!(d?.readyState === 'complete' && d.getElementById('title'))) blanks.push(`${where}: not loaded`);
        }
        docs.set(f, d);
      }
      requestAnimationFrame(tick);
    })();
  });
  const inFrame = (selector) => async () => (await page.locator(selector).elementHandle()).contentFrame();
  const entry = inFrame('article .frame iframe:not(.next)');
  scrollOf = () => page.evaluate(() => document.getElementById('stream').scrollTop);
  await check('stream', entry, () => page.evaluate(() => { document.getElementById('stream').scrollTop = 400; }));

  // Verb calls during a swap must not restart it, and two quick shapes end on the last.
  await check('stream, busy', entry, () => page.evaluate(() => { document.getElementById('stream').scrollTop = 400; }), async () => {
    for (let i = 0; i < 6; i++) { await post(`/api/jellies/${name}/do`, {verb: 'inc', args: {}}); await page.waitForTimeout(30); }
    await shape();
  });
  assert(await (await entry()).locator('#n').textContent() === 'n=6', 'stream: state after the busy swap');

  await page.locator('article [data-act="open"]').first().click();
  const fullFrame = inFrame('#full iframe:not(.next)');
  await (await fullFrame()).locator('#title').waitFor();
  scrollOf = async () => (await fullFrame()).evaluate(() => scrollY);
  await check('full size', fullFrame, async () => (await fullFrame()).evaluate(() => scrollTo(0, 1200)));
  await page.locator('#back').click();

  // A pinned tile swaps too.
  await post(`/api/jellies/${name}/keep`, {pinned: true});
  const tile = page.locator('.tile iframe:not(.next)');
  await tile.contentFrame().locator('#title').waitFor();
  await page.evaluate(() => { window.watching = true; });
  await shape();
  await shows(async () => (await tile.elementHandle()).contentFrame(), `v${rev}`);
  await page.evaluate(() => { window.watching = false; });
  assert(await page.locator('.tile iframe').count() === 1, 'tile: one frame after the swap');

  const blanks = await page.evaluate(() => blanks);
  assert(!blanks.length, `jelly page frames were not swapped in place (${blanks.length}): ${[...new Set(blanks)]}`);
  await page.request.delete(`${base}/api/jellies/${encodeURIComponent(name)}?by=test`);
  return results;
}
