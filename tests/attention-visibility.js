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
      return item && document.getElementById(item.anchor.selector.slice(1))?.contains(document.activeElement);
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
  assert(await frame.locator(`#jf-q-${id} .exposure`).textContent() === 'About The tucked-away part · The question is shown here so you can answer it.', 'fallback context');
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
  await frame.evaluate(() => { document.getElementById('details').hidden = true; void document.getElementById('details').offsetHeight; });
  await rescued(frame, id, true);
  assert(await frame.evaluate(() => savedHost.shadowRoot.activeElement?.value === 'keep this draft' && savedHost.shadowRoot.activeElement.selectionStart === 2 && savedHost.shadowRoot.activeElement.selectionEnd === 7), 'hide keeps focus and selection');
  await frame.evaluate(() => { details.hidden = false; });
  await rescued(frame, id, false);
  await frame.evaluate(() => {
    savedHost.shadowRoot.querySelector('button.o').focus();
    details.hidden = true; void details.offsetHeight;
  });
  await rescued(frame, id, true);
  assert(await frame.evaluate(() => savedHost.shadowRoot.activeElement?.matches('button.o')), 'hide preserves the latest control selected inside the shadow root');
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
  for (const selector of ['#attribute','#closed','#invisible','#transparent','#inert','#clipped','#short','#contents','#below','#missing','#floating','#svg']) {
    const item = await ask(selector);
    frame = await dock(name, item.args.id);
    await focused(frame, item.args.id);
    const rescue = ['#attribute','#closed','#invisible','#transparent','#inert','#clipped','#short'].includes(selector);
    await rescued(frame, item.args.id, rescue);
    assert(!!item.anchor_exposed === rescue, `exposure diagnostic ${selector}`);
    if (selector === '#below') assert(item.anchor === 'below', 'offscreen anchors are scroll destinations');
    if (selector === '#attribute') {
      await frame.evaluate(() => { window.attributeHeight = document.body.getBoundingClientRect().height; document.querySelector('#attribute').parentElement.dataset.visible = 'true'; });
      await rescued(frame, item.args.id, false);
      await frame.evaluate(() => document.querySelector('#attribute').parentElement.dataset.visible = 'false');
      await rescued(frame, item.args.id, true);
      assert(await frame.evaluate(() => attributeHeight === document.body.getBoundingClientRect().height), 'attribute placement without a body resize');
    }
    if (selector === '#short') assert(!(await frame.locator(`#jf-q-${item.args.id} .exposure`).textContent()).includes('hidden'), 'visible subject is not described as hidden');
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
  // Required refresh failures remain visible after the dock has closed;
  // a successfully refreshed, absent/withdrawn item has a separate message.
  for (const outcome of ['http','network','absent','withdrawn']) {
    const old = await (await page.request.get(`${base}/api/jellies/${name}`)).json();
    let seenStale;
    const staleReady = new Promise(resolve => { seenStale = resolve; });
    let openingRefresh = false, missingId;
    const failRefresh = async route => {
      if (route.request().frame() !== frame) return route.continue();
      if (!openingRefresh) { await route.fulfill({json:old}); seenStale(); return; }
      if (outcome === 'http') return route.fulfill({status:503,body:'temporarily unavailable'});
      if (outcome === 'network') return route.abort('failed');
      if (outcome === 'withdrawn') {
        await post(`/api/jellies/${name}/attention/${missingId}/withdraw`, {});
        const result = await (await page.request.get(showUrl)).json();
        return route.fulfill({json:result});
      }
      return route.fulfill({json:old});
    };
    await page.route(showUrl, failRefresh);
    const missing = await ask('#tucked', 'look');
    missingId = missing.args.id;
    await staleReady;
    await page.locator(`#dock-list [data-goto="${name}"][data-item="${missingId}"]`).waitFor({state:'attached'});
    openingRefresh = true;
    await frame.evaluate(() => { window.previousOpeningMessage = document.querySelector('[role="alert"]'); });
    if (!(await page.locator('#dock-panel').isVisible())) await page.locator('#dock-btn').click();
    await page.locator(`#dock-list [data-goto="${name}"][data-item="${missingId}"]`).click();
    const message = frame.locator('[role="alert"]');
    await frame.waitForFunction(text => {
      const el = document.querySelector('[role="alert"]');
      return el && el !== window.previousOpeningMessage && el === document.activeElement && el.textContent.includes(text);
    }, ['http','network'].includes(outcome) ? 'Could not load' : 'no longer waiting');
    await message.waitFor({state:'visible'});
    assert((await message.textContent()).includes(['http','network'].includes(outcome) ? 'Could not load' : 'no longer waiting'), `visible refresh outcome ${outcome}`);
    assert(await message.evaluate(el => el === document.activeElement), `refresh outcome focused ${outcome}`);
    await page.unroute(showUrl, failRefresh);
  }
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
  await post(`/api/jellies/${groupedName}/ask`, {spec:{title:'Grouped questions',questions:[{id:'design:mobile',question:'One?',options:[{id:'y',label:'Yes'}]},{id:'design.mobile[wide]',question:'Two?',options:[{id:'y',label:'Yes'}]}]}});
  await page.locator('#dock-list [data-goto="'+groupedName+'"]').waitFor({state:'attached'});
  let groupedId = await page.locator('#dock-list [data-goto="'+groupedName+'"]').getAttribute('data-item');
  assert(!!groupedId, 'grouped row carries an item id');
  frame = await dock(groupedName, groupedId);
  await frame.waitForFunction(id => {
    const a = jelly.items.find(a => a.id === id), target = document.getElementById(a.anchor.selector.slice(1));
    return target?.contains(document.activeElement) && document.activeElement.matches('[data-pick]');
  }, groupedId);
  const inlineItems = await frame.evaluate(() => jelly.items.map(a => ({id:a.id,selector:a.anchor.selector})));
  for (const item of inlineItems) {
    await frame.evaluate(selector => { location.hash = selector; document.activeElement.blur(); }, item.selector);
    frame = await dock(groupedName, groupedId);
    await frame.waitForFunction(id => {
      const item = jelly.items.find(a => a.id === id);
      return document.getElementById(item.anchor.selector.slice(1))?.contains(document.activeElement);
    }, groupedId);
  }
  await frame.locator(`[data-pick="${groupedId}|y"]`).click();
  const remaining = inlineItems.find(item => item.id !== groupedId);
  await page.locator(`#dock-list [data-goto="${groupedName}"][data-item="${remaining.id}"]`).waitFor({state:'attached'});
  groupedId = remaining.id;
  frame = await dock(groupedName, groupedId);
  // Same-document navigation also retains view callbacks for ordinary cards.
  frame = await dock(name, twin.args.id);
  await frame.evaluate(() => { location.hash = '#tucked'; document.activeElement.blur(); });
  frame = await dock(name, twin.args.id);
  await focused(frame, twin.args.id);
  assert(await frame.locator('.jf-ask').count() > 0, 'ordinary cards survive hash navigation');
  frame = await dock(groupedName, groupedId);
  assert(await frame.locator('.jf-ask').count() === 0, 'inline view has no duplicate cards');
  // Hold an earlier local write before committing it; wait for both response
  // echoes before checking the final frame and focused question.
  const focusUrl = `${base}/api/jellies/surface/call`;
  let releaseFirst, firstStarted;
  const firstReady = new Promise(resolve => { firstStarted = resolve; });
  const firstGate = new Promise(resolve => { releaseFirst = resolve; });
  const writes = [];
  let held = false;
  const delayFocus = async route => {
    if (route.request().postDataJSON().verb !== 'focus') return route.continue();
    if (!held) { held = true; firstStarted(); await firstGate; }
    const response = await route.fetch();
    writes.push(await response.json());
    await route.fulfill({response});
  };
  await page.route(focusUrl, delayFocus);
  await page.evaluate(({name,id,groupedName,groupedId}) => {
    document.querySelector(`#dock-list [data-goto="${name}"][data-item="${id}"]`).click();
    document.querySelector(`#dock-list [data-goto="${groupedName}"][data-item="${groupedId}"]`).click();
  }, {name,id:twin.args.id,groupedName,groupedId});
  await firstReady;
  frame = await (await page.locator('#full iframe').elementHandle()).contentFrame();
  await frame.waitForURL(`${base}/j/${encodeURIComponent(groupedName)}/`, {waitUntil:'load'});
  await frame.waitForFunction(id => window.jelly?.items.find(a => a.id === id) && document.activeElement.matches('[data-pick]'), groupedId);
  releaseFirst();
  await page.waitForFunction(name => pendingFocusWrites === 0 && jelly.state.focus === name, groupedName);
  assert(writes.length === 2, 'both delayed local focus writes completed');
  assert(await page.locator('#full iframe').getAttribute('title') === groupedName, 'final selection after earlier writes and echoes');
  await frame.waitForFunction(() => document.activeElement.matches('[data-pick]'));
  await page.unroute(focusUrl, delayFocus);
  // An external focus committed after a local open or close must survive a
  // delayed response to that local write, including its queue-drain handling.
  for (const operation of ['open','close']) {
    let release, started;
    const ready = new Promise(resolve => { started = resolve; });
    const gate = new Promise(resolve => { release = resolve; });
    const holdResponse = async route => {
      if (route.request().postDataJSON().verb !== 'focus') return route.continue();
      const response = await route.fetch();
      started(); await gate;
      await route.fulfill({response});
    };
    await page.route(focusUrl, holdResponse);
    if (operation === 'open') {
      await page.evaluate(({name,id}) => document.querySelector(`#dock-list [data-goto="${name}"][data-item="${id}"]`).click(), {name,id:twin.args.id});
    } else await page.locator('#back').click();
    await ready;
    await post('/api/jellies/surface/call', {verb:'focus',args:{name:groupedName}});
    await page.waitForFunction(name => jelly.state.focus === name, groupedName);
    release();
    await page.waitForFunction(name => pendingFocusWrites === 0 && jelly.state.focus === name && full === name, groupedName);
    assert(await page.locator('#full').isVisible(), `external focus survives local ${operation}`);
    assert(await page.locator('#full iframe').getAttribute('title') === groupedName, `external focus selects its frame after local ${operation}`);
    await page.unroute(focusUrl, holdResponse);
  }
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
  return {measurement,cases,callbacks:'once, immediate/delayed reveal, unsubscribe and failure',draft:'preserved and persisted',grouped:'focused inline',rapid:'newest frame and focused control after delayed writes',externalFocus:'local open and close responses delayed',refresh:'HTTP, network, absent and withdrawn messages visible and focused',hash:'inline and runtime cards',snapshot:'inert subscription'};
}
