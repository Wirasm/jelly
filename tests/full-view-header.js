async page => {
  const run = async (page, widths) => {
    const base = new URL(page.url()).origin;
    const project = `header-${Date.now()}`;
    const names = [];
    const results = [];
    const assert = (ok, message) => { if (!ok) throw Error(message); };
    const post = async (path, data) => {
      const response = await page.request.post(base + path, {data: {...data, by:'test'}});
      if (!response.ok()) throw Error(await response.text());
      return response.json();
    };
    const read = async name => (await page.request.get(`${base}/api/jellies/${name}`)).json();
    const waitSaved = async (name, key, value) => {
      await page.waitForFunction(async ({name, key, value}) => {
        const saved = await (await fetch(`/api/jellies/${name}`)).json();
        return (key === 'count' ? saved.state.count : saved[key]) === value;
      }, {name, key, value});
    };
    const open = async name => {
      await post('/api/jellies/surface/call', {verb:'focus', args:{name}});
      await page.locator('#full').waitFor({state:'visible'});
      await page.waitForFunction(name => document.querySelector('#full iframe')?.contentWindow.jelly?.name === name, name);
    };
    const geometry = async (label, long) => {
      const measured = await page.evaluate(({label, long}) => {
        const rect = el => el.getBoundingClientRect();
        const title = document.querySelector('#ftitle'), meta = document.querySelector('#fmeta');
        const t = rect(title), m = rect(meta), bar = rect(document.querySelector('.fbar'));
        const frame = rect(document.querySelector('#full iframe'));
        const controls = [...document.querySelectorAll('.fbar button')];
        const fail = message => { throw Error(`${label}: ${message}`); };
        if (t.width < (innerWidth === 390 ? 200 : 100) || !t.height || !m.height) fail('title/metadata collapsed');
        if (t.left < 0 || t.right > innerWidth || m.right > innerWidth || meta.scrollWidth > meta.clientWidth + 1) fail('identification overflow');
        const range = document.createRange(); range.selectNodeContents(meta);
        for (const r of range.getClientRects()) if (r.left < m.left - 1 || r.right > m.right + 1) fail('metadata text outside its column');
        for (const control of controls) {
          const r = rect(control);
          if (r.left < 0 || r.right > innerWidth || r.top < bar.top || r.bottom > bar.bottom) fail('action outside header');
          for (const text of [t,m]) if (r.left < text.right && r.right > text.left && r.top < text.bottom && r.bottom > text.top) fail('action overlaps identification');
          if (!control.contains(document.elementFromPoint(r.x + r.width/2, r.y + r.height/2))) fail('action obscured');
        }
        if (long && !(title.scrollWidth > title.clientWidth && getComputedStyle(title).textOverflow === 'ellipsis' && getComputedStyle(title).whiteSpace === 'nowrap')) fail('long title does not truncate');
        if (frame.top < bar.bottom - 1 || frame.height < 300) fail('frame overlaps header or lacks useful height');
        if (innerWidth === 1280 && bar.height > 80) fail('desktop normal header no longer compact');
        if (innerWidth === 320 && document.querySelector('#fcomment').classList.contains('on') && rect(controls.at(-1)).top <= rect(controls[1]).top) fail('expanded actions do not wrap');
        return {label, width:innerWidth, titleWidth:t.width, headerHeight:bar.height, frameHeight:frame.height, removeRight:rect(controls.at(-1)).right};
      }, {label,long});
      results.push(measured);
    };
    try {
      for (const suffix of ['normal', 'metadata-' + 'x'.repeat(50)]) {
        const name = `${project}:${suffix}`;
        await post('/api/jellies', {name, about:'Weekly project plan', view:'<!doctype html><button id="target">Comment target</button>', verbs:'const verbs = { increment(s) { s.count++; } };', state:{count:0}});
        names.push(name);
      }
      await page.goto(`${base}/p/${project}`);
      await page.waitForFunction(() => window.jelly && document.querySelector('.entry'));
      for (const width of widths) {
        await page.setViewportSize({width,height:width === 1280 ? 900 : 844});
        assert(await page.evaluate(width => innerWidth === width && Math.abs(visualViewport.scale - 1) < 0.01, width), `${width}: layout viewport must match device width at full scale`);
        for (const name of names) {
          await open(name);
          await geometry(`${width} normal ${name === names[0] ? 'metadata' : 'long metadata'}`, false);
          await post(`/api/jellies/${name}/reshape`, {about:'A long weekly project plan title '.repeat(12)});
          await page.waitForFunction(() => document.querySelector('#ftitle').textContent.startsWith('A long'));
          await page.locator('#fpin').click();
          await page.waitForFunction(() => document.querySelector('#fpin').textContent === 'Unpin');
          await waitSaved(name,'pinned',true);
          await page.locator('#fcomment').click();
          await page.waitForFunction(() => document.querySelector('#fcomment').textContent === 'Click a part · Esc');
          await geometry(`${width} long expanded Unpin`, true);
          if (name === names[0] && [390,1280].includes(width)) {
            await page.screenshot({path:`header-${width}.png`});
            const frame = page.frameLocator('#full iframe');
            await page.locator('#back').hover();
            await frame.locator('#target').hover();
            await page.waitForFunction(() => document.querySelector('#full iframe').contentDocument.querySelector('#target').classList.contains('jf-hover'));
            await page.keyboard.press('Escape');
            await page.waitForFunction(() => document.querySelector('#fcomment').textContent === 'Comment' && !document.querySelector('#full').hidden);
            await page.locator('#back').focus();
            await page.keyboard.press('c');
            await page.waitForFunction(() => document.querySelector('#fcomment').classList.contains('on'));
            await page.locator('#back').hover();
            await frame.locator('#target').hover();
            await page.waitForFunction(() => document.querySelector('#full iframe').contentDocument.querySelector('#target').classList.contains('jf-hover'));
            await page.keyboard.press('Escape');
            await page.waitForFunction(() => !document.querySelector('#fcomment').classList.contains('on'));
            await post(`/api/jellies/${name}/call`, {verb:'increment',args:{}});
            await waitSaved(name,'count',1);
            await page.locator('#fundo').click();
            await waitSaved(name,'count',0);
            await page.locator('#back').focus();
            for (const id of ['fcomment','fundo','fpin','frm']) {
              await page.keyboard.press('Tab');
              assert(await page.evaluate(id => document.activeElement.id === id,id), `tab order ${id}`);
            }
          } else {
            await page.locator('#fcomment').click();
          }
          await page.locator('#fpin').click();
          await page.waitForFunction(() => document.querySelector('#fpin').textContent === 'Pin');
          await waitSaved(name,'pinned',false);
          await page.locator('#back').click();
          await page.locator('#full').waitFor({state:'hidden'});
          await page.locator('.entry.sel h2').click();
          await page.keyboard.press('Enter');
          await page.locator('#full').waitFor({state:'visible'});
          await page.locator('#back').focus();
          await page.keyboard.press('Escape');
          await page.locator('#full').waitFor({state:'hidden'});
          await post(`/api/jellies/${name}/reshape`, {about:'Weekly project plan'});
        }
      }
      page.removeAllListeners("dialog");
      for (const width of widths.filter(width => [390,1280].includes(width))) {
        await page.setViewportSize({width,height:900});
        const name = names[width === 390 ? 0 : 1];
        await open(name);
        page.once('dialog', dialog => dialog.accept());
        await page.locator('#frm').click();
        await page.locator('#full').waitFor({state:'hidden'});
        assert((await page.request.get(`${base}/api/jellies/${name}`)).status() === 404, 'Remove deletes fixture');
        if (width === 390) assert((await read(names[1])).name === names[1], 'Remove preserves other fixture');
      }
      console.log(JSON.stringify({passed:true, measurements:results}));
      return results;
    } finally {
      await page.goto('about:blank');
      for (const name of names) await page.request.delete(`${base}/api/jellies/${name}?by=test`);
    }
  };
  const base = new URL(page.url()).origin;
  const mobile = await page.context().browser().newContext({
    viewport:{width:390,height:844}, isMobile:true, hasTouch:true, deviceScaleFactor:3
  });
  try {
    const phone = await mobile.newPage();
    await phone.goto(base);
    const phoneResults = await run(phone, [390]);
    const desktopResults = await run(page, [1280,320,900,901]);
    return {phone:phoneResults, desktop:desktopResults};
  } finally {
    await mobile.close();
  }
}
