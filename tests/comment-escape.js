// Escape that ends comment mode leaves the map's trace step alone; see tests/README.md.
async page => {
  const base = new URL(page.url()).origin, name = 'escape:map';
  const assert = (ok, message) => { if (!ok) throw Error(message); };
  const results = [];
  const until = async (fn, message) => {
    for (let i = 0; i < 40; i++) { if (await fn()) return; await page.waitForTimeout(50); }
    throw Error(message);
  };

  // Walk to step 3, end commenting with Esc, cancel a comment editor with Esc,
  // then Esc again does the map's ordinary dismissal.
  const check = async (mode, doc, comment, commenting) => {
    const step = () => doc.locator('[data-goto].now').getAttribute('data-goto', {timeout: 1000}).catch(() => null);
    await doc.locator('#t-click').click();
    await doc.locator('[data-step="1"]').click();
    await doc.locator('[data-step="1"]').click();
    assert(await step() === '2', `${mode}: trace reaches step 3`);

    const enterCommenting = async () => {
      await comment.click();
      await until(commenting, `${mode}: commenting starts`);
      // The jelly itself is in comment mode once hovering highlights a part.
      await doc.locator('.say').hover();
      await until(async () => /jf-hover/.test(await doc.locator('.say').getAttribute('class')), `${mode}: jelly enters comment mode`);
    };
    await enterCommenting();
    await page.keyboard.press('Escape');
    await until(async () => !await commenting(), `${mode}: Esc ends commenting`);
    assert(await step() === '2', `${mode}: Esc that ends commenting keeps the trace step`);

    await enterCommenting();
    await doc.locator('.say').click();
    await doc.locator('.jf-pop textarea').waitFor();
    await page.keyboard.press('Escape');
    await doc.locator('.jf-pop').waitFor({state: 'detached'});
    assert(await step() === '2', `${mode}: Esc in the comment editor keeps the trace step`);

    await page.keyboard.press('Escape');
    assert(await step() === null, `${mode}: Esc without commenting closes the trace`);
    results.push(mode);
  };

  await page.goto(`${base}/j/${name}/`);
  await page.waitForFunction(() => window.jelly && document.querySelector('#t-click'));
  const btn = page.locator('.jf-btn');
  await check('standalone', page, btn, async () => (await btn.textContent()).includes('Esc'));

  await page.goto(`${base}/p/escape`);
  await page.waitForFunction(() => window.jelly && document.querySelector('.entry'));
  await page.waitForFunction(() => pendingFocusWrites === 0);
  const r = await page.request.post(`${base}/api/jellies/page/do`, {data: {verb: 'focus', args: {name}, by: 'test'}});
  if (!r.ok()) throw Error(await r.text());
  await page.waitForFunction(() => document.querySelector('#full iframe')?.contentDocument?.querySelector('#t-click') && !document.querySelector('#full').hidden);
  const frame = page.frameLocator('#full iframe');
  const full = () => page.evaluate(() => !document.querySelector('#full').hidden);
  await check('embedded', frame, page.locator('#fcomment'), async () => {
    assert(await full(), 'embedded: full view stays open while commenting ends');
    return page.evaluate(() => document.querySelector('#fcomment').classList.contains('on'));
  });
  // Esc without commenting still reaches the jelly page, which closes the full view.
  await page.locator('#full').waitFor({state: 'hidden'});
  return {passed: results};
}
