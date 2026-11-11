async page => {
  const base = new URL(page.url()).origin;
  const pageErrors = [];
  const onError = error => pageErrors.push(error.message);
  page.on('pageerror', onError);
  const fixture = await (await page.request.get(`${base}/api/jellies/visibility:fixture`)).json();
  const assert = (ok, message) => { if (!ok) throw new Error(message); };
  const post = async (path, data) => {
    const response = await page.request.post(`${base}${path}`, {data: {...data, by:'test'}});
    if (!response.ok()) throw new Error(await response.text());
    return response.json();
  };
  const name = `visibility:run-${Date.now()}`;
  await post('/api/jellies', { name, view: fixture.view, verbs: 'const verbs = {};', state: {} });
  const ask = async (selector, kind = 'ask') => post(`/api/jellies/${name}/attention`, {kind,title:`Question about ${selector}`,anchor:{selector,quote:selector === '#tucked' ? 'The tucked-away part' : selector},options:kind === 'ask' ? [{id:'yes',label:'Yes'},{id:'no',label:'No'}] : []});
  const ev = await ask('#tucked');
  const id = ev.args.id;
  const dock = async (jellyName, itemId) => {
    const previous = await page.locator('#full').evaluate((full, name) => {
      const w = full.querySelector('iframe')?.contentWindow;
      return w?.jelly?.name === name ? w.openings?.length || 0 : 0;
    }, jellyName);
    if (!(await page.locator('#dock-panel').isVisible())) await page.locator('#dock-btn').click();
    await page.locator(`#dock-list [data-goto="${jellyName}"][data-item="${itemId}"]`).click();
    await page.locator('#full iframe').waitFor();
    const frame = await (await page.locator('#full iframe').elementHandle()).contentFrame();
    await frame.waitForLoadState('load');
    await frame.waitForFunction(({id, previous}) => {
      if (window.openings && openings.length <= previous) return false;
      const host = document.getElementById(`jf-q-${id}`);
      if (host) return !!host.shadowRoot.activeElement;
      const item = window.jelly?.items.find(a => a.id === id);
      return item && document.querySelector(item.anchor.selector)?.contains(document.activeElement);
    }, {id:itemId, previous});
    return frame;
  };
  const focused = async (frame, itemId) => frame.waitForFunction(id => document.getElementById(`jf-q-${id}`)?.shadowRoot.activeElement, itemId);
  const rescued = async (frame, itemId, value) => frame.waitForFunction(({id, value}) => {
    const h = document.getElementById(`jf-q-${id}`);
    return h && h.hasAttribute('data-exposed') === value && h.checkVisibility({checkOpacity:true,checkVisibilityCSS:true});
  }, {id:itemId, value});
  const standalone = await page.context().newPage();
  await standalone.goto(`${base}/j/${encodeURIComponent(name)}/`);
  await rescued(standalone, id, true);
  await standalone.close();
  await page.goto(`${base}/p/visibility`);
  let frame = await dock(name, id);
  await frame.locator(`#jf-q-${id}`).waitFor({state:'visible',timeout:5000});
  await focused(frame, id);
  await rescued(frame, id, true);
  assert(await frame.locator(`#jf-q-${id} .exposure`).textContent() === 'About The tucked-away part · This part is hidden; the question is shown here.', 'fallback context');
  assert(ev.anchor_exposed === true && ev.anchor === 'hidden', 'measured hidden diagnostic');
  const measurement = await frame.locator(`#jf-q-${id}`).evaluate(h => {
    const r = h.getBoundingClientRect();
    return {width:r.width,height:r.height,parent:h.parentElement.className,parentHidden:h.parentElement.hidden,focusedInCard:!!h.shadowRoot.activeElement,questionVisible:h.checkVisibility()};
  });
  // Move the same focused draft out of, and back into, an attribute-only reveal.
  await frame.locator(`#jf-q-${id} textarea`).fill('keep this draft');
  await frame.evaluate(id => {
    window.savedHost = document.getElementById(`jf-q-${id}`);
    const ta = savedHost.shadowRoot.querySelector('textarea');
    ta.focus(); ta.setSelectionRange(2,7);
    window.originalHeight = document.body.getBoundingClientRect().height;
    document.getElementById('details').hidden = false;
  }, id);
  await rescued(frame, id, false);
  assert(await frame.evaluate(() => savedHost === document.querySelector('#tucked').nextElementSibling && savedHost.shadowRoot.activeElement?.value === 'keep this draft' && savedHost.shadowRoot.activeElement.selectionStart === 2 && savedHost.shadowRoot.activeElement.selectionEnd === 7 && document.body.getBoundingClientRect().height === originalHeight), 'reveal preserves host, draft, selection and focus');
  await frame.evaluate(() => document.getElementById('details').hidden = true);
  await rescued(frame, id, true);
  assert(await frame.evaluate(() => savedHost.shadowRoot.activeElement?.value === 'keep this draft'), 'hide keeps focus');
  // Existing frames receive one callback; subscribers can reveal synchronously.
  await frame.evaluate(() => { reveal = 'immediate'; });
  frame = await dock(name, id);
  await rescued(frame, id, false);
  await focused(frame, id);
  assert(await frame.evaluate(id => openings.length === 2 && openings.every(x => x.id === id && x.selector === '#tucked'), id), 'one callback per dock opening');
  // An async reveal never delays fallback, and only an active card keeps focus.
  await frame.evaluate(() => { details.hidden = true; reveal = 'delayed'; });
  await rescued(frame, id, true);
  frame = await dock(name, id);
  await focused(frame, id);
  await frame.evaluate(() => finishReveal());
  await rescued(frame, id, false);
  await focused(frame, id);
  await frame.evaluate(() => { details.hidden = true; });
  await rescued(frame, id, true);
  await dock(name, id);
  await frame.locator('#show').focus();
  await frame.evaluate(() => finishReveal());
  await rescued(frame, id, false);
  assert(await frame.evaluate(() => document.activeElement.id === 'show'), 'inactive card must not steal focus');
  await frame.evaluate(() => {
    window.unsubscribedCalled = 0;
    const stop = jelly.onAttend(() => { unsubscribedCalled++; }); stop();
    window.stopThrow = jelly.onAttend(() => { throw new Error('expected subscriber failure'); });
    details.hidden = true; reveal = 'none';
  });
  await dock(name, id);
  await focused(frame, id);
  await rescued(frame, id, true);
  assert(await frame.evaluate(() => { stopThrow(); return openings.length === 5 && unsubscribedCalled === 0; }), 'throwing subscriber does not block or duplicate opening');
  // Answer through the real card and observe the daemon, then reopen its summary.
  await frame.locator(`#jf-q-${id} button.o`).first().click();
  await frame.waitForFunction(id => jelly.items.find(a => a.id === id)?.answer?.option === 'yes', id);
  const saved = await (await page.request.get(`${base}/api/jellies/${name}`)).json();
  assert(saved.attention.find(a => a.id === id).answer.text === 'keep this draft', 'answer persisted with draft');
  await frame.evaluate(id => window.postMessage({type:'jelly:attend',id},location.origin), id);
  await frame.waitForFunction(id => document.getElementById(`jf-q-${id}`)?.shadowRoot.activeElement?.className === 'chg', id);
  // Visibility primitives and adjacent-card clipping, with a frame already loaded.
  const cases = [];
  for (const selector of ['#closed','#invisible','#transparent','#inert','#clipped','#short','#contents','#below','#missing','#floating','#svg']) {
    const item = await ask(selector);
    frame = await dock(name, item.args.id);
    await focused(frame, item.args.id);
    const rescue = ['#closed','#invisible','#transparent','#inert','#clipped','#short'].includes(selector);
    await rescued(frame, item.args.id, rescue);
    assert(!!item.anchor_exposed === rescue, `exposure diagnostic ${selector}`);
    if (selector === '#below') assert(item.anchor === 'below', 'offscreen anchors are scroll destinations');
    if (selector === '#invisible') {
      await frame.evaluate(() => document.querySelector('#invisible').parentElement.classList.remove('concealed'));
      await rescued(frame, item.args.id, false);
      await frame.evaluate(() => document.querySelector('#invisible').parentElement.classList.add('concealed'));
      await rescued(frame, item.args.id, true);
    }
    const inViewport = await frame.locator(`#jf-q-${item.args.id}`).evaluate(h => {
      const r = h.shadowRoot.activeElement.getBoundingClientRect();
      return r.left >= 0 && r.right <= innerWidth && r.top >= 0 && r.bottom <= innerHeight;
    });
    assert(inViewport, `answer control in viewport ${selector}`);
    cases.push(selector);
  }
  // A reused frame can miss an update; opening an unknown id refreshes it once.
  const beforeStale = await (await page.request.get(`${base}/api/jellies/${name}`)).json();
  const staleFrame = frame;
  let deliveredStale;
  const staleResponse = new Promise(resolve => { deliveredStale = resolve; });
  const showUrl = `${base}/api/jellies/${encodeURIComponent(name)}`;
  let skipped = false;
  const staleRoute = async route => {
    if (!skipped && route.request().frame() === staleFrame) {
      skipped = true;
      await route.fulfill({json:beforeStale}); deliveredStale();
    } else await route.continue();
  };
  await page.route(showUrl, staleRoute);
  const late = await ask('#tucked', 'look');
  await staleResponse;
  frame = await dock(name, late.args.id);
  await focused(frame, late.args.id);
  await rescued(frame, late.args.id, true);
  await page.unroute(showUrl, staleRoute);
  assert(late.anchor_exposed === true, 'Look reports measured exposure');
  // Two questions sharing a destination must stop moving after placement settles.
  const twin = await ask('#tucked');
  frame = await dock(name, twin.args.id);
  await focused(frame, twin.args.id);
  await frame.evaluate(() => { details.hidden = false; });
  await rescued(frame, twin.args.id, false);
  const stable = await frame.evaluate(async ids => {
    let moves = 0;
    const o = new MutationObserver(ms => { moves += ms.filter(m => m.type === 'childList' && [...m.addedNodes,...m.removedNodes].some(n => ids.includes(n.id))).length; });
    o.observe(document.body, {childList:true,subtree:true});
    dispatchEvent(new Event("resize"));
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    o.disconnect(); return moves;
  }, [id,late.args.id,twin.args.id].map(id => `jf-q-${id}`));
  assert(stable === 0, 'stable destination order must not cause observer redraws');
  // Built-in grouped rounds use their existing inline question and focus a pick.
  const groupedName = `visibility:group-${Date.now()}`;
  await post(`/api/jellies/${groupedName}/ask`, {spec:{title:'Grouped questions',questions:[{id:'one',question:'One?',options:[{id:'y',label:'Yes'}]},{id:'two',question:'Two?',options:[{id:'y',label:'Yes'}]}]}});
  await page.locator('#dock-list [data-goto="'+groupedName+'"]').waitFor({state:'attached'});
  const groupedId = await page.locator('#dock-list [data-goto="'+groupedName+'"]').getAttribute('data-item');
  assert(!!groupedId, 'grouped row carries an item id');
  frame = await dock(groupedName, groupedId);
  await frame.waitForFunction(id => {
    const a = jelly.items.find(a => a.id === id), target = document.querySelector(a.anchor.selector);
    return target?.contains(document.activeElement) && document.activeElement.matches('[data-pick]');
  }, groupedId);
  assert(await frame.locator('.jf-ask').count() === 0, 'inline view has no duplicate cards');
  // Rapid navigation leaves the newest request in the intended frame.
  await page.evaluate(({name,id,groupedName,groupedId}) => {
    document.querySelector(`#dock-list [data-goto="${name}"][data-item="${id}"]`).click();
    document.querySelector(`#dock-list [data-goto="${groupedName}"][data-item="${groupedId}"]`).click();
  }, {name,id:twin.args.id,groupedName,groupedId});
  frame = await (await page.locator('#full iframe').elementHandle()).contentFrame();
  await frame.waitForFunction(id => window.jelly?.items.find(a => a.id === id) && document.activeElement.matches('[data-pick]'), groupedId);
  // Shared snapshots allow the same view to register the hook inertly.
  const shared = await page.request.get(`${base}/api/jellies/${name}/share`);
  const snapshot = await page.context().newPage();
  const snapshotErrors = [];
  snapshot.on('pageerror', e => snapshotErrors.push(e.message));
  await snapshot.setContent(await shared.text());
  assert(await snapshot.evaluate(() => typeof jelly.onAttend(() => {}) === 'function'), 'snapshot subscription stub');
  assert(snapshotErrors.length === 0, `snapshot errors: ${snapshotErrors}`);
  await snapshot.close();
  page.off('pageerror', onError);
  assert(pageErrors.length === 0, `browser errors: ${pageErrors}`);
  return {measurement,cases,callbacks:'once, immediate/delayed reveal, unsubscribe and failure',draft:'preserved and persisted',grouped:'focused inline',rapid:'newest frame only',snapshot:'inert subscription'};
}
