async page => {
  const base = new URL(page.url()).origin;
  const run = async (page, widths, touch) => {
    page.setDefaultTimeout(10000);
    page.removeAllListeners('dialog');
    const project = `sh${Date.now().toString(36)}`;
    const names = [`${project}:plan`, `${project}:${'metadata'.repeat(6)}`];
    const results = [];
    const assert = (ok, message) => { if (!ok) throw Error(message); };
    const post = async (path, data) => {
      const response = await page.request.post(base + path, {data:{by:'agent', ...data}});
      assert(response.ok(), `${path}: ${await response.text()}`);
      return response.json();
    };
    const entry = name => page.locator(`.entry[data-name="${name}"]`);
    const button = (name, act) => entry(name).locator(`header [data-act="${act}"]`);
    const activate = async locator => touch ? locator.tap() : locator.click();
    const savedPin = async (name, pinned) => {
      await page.waitForFunction(async ({name,pinned}) => (await (await fetch(`/api/jellies/${name}`)).json()).pinned === pinned, {name,pinned});
      await entry(name).waitFor({state:pinned ? 'detached' : 'visible'});
    };
    const comment = async (name, on) => {
      await page.waitForFunction(({name,on}) => document.querySelector(`.entry[data-name="${name}"] [data-act="comment"]`)?.classList.contains('on') === on, {name,on});
    };
    const exitComment = async name => {
      const target = entry(name).frameLocator('iframe').locator('#target');
      await page.locator('.top b').hover();
      await target.hover();
      await page.waitForFunction(name => document.querySelector(`.entry[data-name="${name}"] iframe`).contentDocument.querySelector('#target').classList.contains('jf-hover'), name);
      await target.focus();
      await page.keyboard.press('Escape');
      await comment(name, false);
    };
    const geometry = async (name, label, ordinary, expanded) => {
      await entry(name).scrollIntoViewIfNeeded();
      if (await page.evaluate(() => innerWidth <= 900)) await page.waitForFunction(name => getComputedStyle(document.querySelector(`.entry[data-name="${name}"] .acts`)).opacity === '1', name);
      await page.waitForFunction(name => document.querySelector(`.entry[data-name="${name}"]`).getAnimations({subtree:true}).every(a => a.playState !== 'running'), name);
      const measured = await entry(name).locator('header').evaluate((header, {label,ordinary,expanded}) => {
        const rect = el => el.getBoundingClientRect();
        const h = rect(header), title = header.querySelector('h2'), meta = header.querySelector('.who');
        const t = rect(title), m = rect(meta), acts = header.querySelector('.acts'), a = rect(acts);
        const controls = [...acts.children], boxes = controls.map(rect);
        const labels = [expanded ? 'Click a part · Esc' : 'Comment','Open','Pin','Remove'];
        const fail = message => { throw Error(`${label}: ${message}`); };
        const overlap = (a,b) => a.left < b.right - 1 && a.right > b.left + 1 && a.top < b.bottom - 1 && a.bottom > b.top + 1;
        const inside = r => r.left >= h.left - 1 && r.right <= h.right + 1;
        if (ordinary && innerWidth === 390 && (t.width < 299 || m.width < 299 || m.height > 26 || t.height > parseFloat(getComputedStyle(title).lineHeight) + 1)) fail(`ordinary title squeezed: ${t.width}px, ${t.height}px high`);
        if (!inside(t) || !inside(m) || header.scrollWidth > header.clientWidth + 1) fail('header/identification overflow');
        for (const el of [title,meta]) {
          const range = document.createRange(); range.selectNodeContents(el);
          const box = rect(el);
          for (const r of range.getClientRects()) if (r.left < box.left - 1 || r.right > box.right + 1) fail(`text outside identification column ${el.tagName}: ${r.left}..${r.right} vs ${box.left}..${box.right}`);
        }
        for (const [i,r] of boxes.entries()) {
          if (controls[i].textContent !== labels[i] || controls[i].scrollWidth > controls[i].clientWidth + 1) fail('action label clipped or changed');
          if (!inside(r) || r.top < h.top - 1 || r.bottom > h.bottom + 1 || r.left < 0 || r.right > innerWidth || overlap(r,t) || overlap(r,m)) fail('action outside header or overlaps identification');
          if (boxes.slice(i+1).some(b => overlap(r,b))) fail('actions overlap');
        }
        if (innerWidth <= 900) {
          if (a.top < Math.max(t.bottom,m.bottom) - 1 || getComputedStyle(acts).opacity !== '1') fail('responsive actions must be visible below identification');
          if (innerWidth === 320 && expanded && boxes.at(-1).top <= boxes[0].top) fail('expanded actions must wrap');
        } else {
          if (a.left < t.right - 1 || a.top > m.top + 1) fail('desktop action column lost');
          if (ordinary && innerWidth === 1280 && h.height > 60) fail('normal desktop header no longer compact');
        }
        return {label,width:innerWidth,titleWidth:t.width,metadataWidth:m.width,titleHeight:t.height,headerHeight:h.height,columns:getComputedStyle(header).gridTemplateColumns};
      }, {label,ordinary,expanded});
      for (const control of await entry(name).locator('header button').all()) {
        assert(await control.evaluate(el => { el.scrollIntoView({block:'center'}); const r = el.getBoundingClientRect(); return el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)); }), `${label}: action obscured`);
      }
      results.push(measured);
    };
    try {
      for (const [i,name] of names.entries()) {
        await post('/api/jellies', {name, about:i ? 'A long weekly project plan '.repeat(8) + 'title'.repeat(35) : 'Weekly project plan', by:i ? 'author'.repeat(20) : 'agent', view:'<!doctype html><button id="target">Weekly plan preview</button>', verbs:'const verbs = { activity(s) { s.count++; } };', state:{count:0}});
      }
      await page.goto(`${base}/p/${project}`);
      await entry(names[0]).waitFor();
      await page.waitForFunction(() => [...document.querySelectorAll('.entry iframe')].every(f => f.contentWindow.jelly));
      for (const width of widths) {
        await page.setViewportSize({width,height:900});
        assert(await page.evaluate(width => innerWidth === width && Math.abs(visualViewport.scale-1) < .01, width), 'device-width viewport at full scale');
        for (const [i,name] of names.entries()) {
          await geometry(name, `${width} ${i ? 'long' : 'ordinary'}`, !i, false);
          if (!i && [390,1280].includes(width)) await page.screenshot({path:`stream-${width}.png`});
          await activate(button(name,'comment'));
          await comment(name,true);
          await geometry(name, `${width} ${i ? 'long' : 'ordinary'} expanded`, !i, true);
          if (!i && width === 390) await page.screenshot({path:'stream-390-expanded.png'});
          await exitComment(name);
        }
      }
      const long = names[1];
      await post(`/api/jellies/${long}/working`, {what:'Preparing the plan',by:'worker'.repeat(20)});
      await post(`/api/jellies/${long}/comment`, {anchor:{selector:'#target'},text:'Review this section'});
      await post(`/api/jellies/${long}/attention`, {kind:'look',id:'review',title:'Review the plan'});
      await post(`/api/jellies/${long}/call`, {verb:'activity',args:{},by:'activity'.repeat(20)});
      await page.waitForFunction(name => {
        const meta = document.querySelector(`.entry[data-name="${name}"] .who`);
        return meta?.querySelector('.work') && meta.textContent.includes('open comment') && meta.textContent.includes('for you') && meta.querySelector('.last')?.textContent.includes('activity');
      }, long);
      for (const width of widths) {
        await page.setViewportSize({width,height:900});
        await geometry(long,`${width} status and activity`,false,false);
        await activate(button(long,'comment'));
        await comment(long,true);
        await geometry(long,`${width} status and activity expanded`,false,true);
        await exitComment(long);
      }
      for (const width of widths.filter(w => [390,1280].includes(w))) {
        await page.setViewportSize({width,height:900});
        const name = names[0];
        for (const path of ['/', `/p/${project}`]) {
          await page.waitForFunction(() => pendingFocusWrites === 0);
          await page.goto(base + path);
          await entry(name).waitFor();
          await page.waitForFunction(() => [...document.querySelectorAll('.entry iframe')].every(f => f.contentWindow.jelly));
          assert(await entry(name).locator('.who code').textContent() === (path === '/' ? name : 'plan'), 'project/All identification');
          await geometry(long,`${width} ${path} long metadata`,false,false);
          await geometry(name,`${width} ${path} ordinary metadata`,true,false);
        }
        await entry(name).locator('h2').click();
        await page.keyboard.press('c');
        await comment(name,true);
        await exitComment(name);
        await entry(name).locator('h2').click();
        await page.keyboard.press('k');
        assert(await entry(long).evaluate(el => el.classList.contains('sel')), 'k selects previous entry');
        await page.keyboard.press('j');
        assert(await entry(name).evaluate(el => el.classList.contains('sel')), 'j selects next entry');
        await page.keyboard.press('Enter');
        await page.locator('#full').waitFor({state:'visible'});
        assert(await page.locator('#full iframe').getAttribute('title') === name, 'Enter opens selected jelly');
        await page.locator('#back').focus();
        await page.keyboard.press('Escape');
        await page.locator('#full').waitFor({state:'hidden'});
        await activate(button(name,'open'));
        await page.locator('#full').waitFor({state:'visible'});
        assert(await page.locator('#ftitle').textContent() === 'Weekly project plan', 'Open shows selected title');
        await activate(page.locator('#back'));
        await page.locator('#full').waitFor({state:'hidden'});
        await page.waitForFunction(() => pendingFocusWrites === 0);
        await entry(long).locator('h2').click();
        await entry(name).scrollIntoViewIfNeeded();
        await page.mouse.move(1,45);
        await page.waitForFunction(({name,width}) => getComputedStyle(document.querySelector(`.entry[data-name="${name}"] .acts`)).opacity === (width > 900 ? '0' : '1'), {name,width});
        if (width > 900) {
          await button(name,'open').hover();
          await page.waitForFunction(name => getComputedStyle(document.querySelector(`.entry[data-name="${name}"] .acts`)).opacity === '1', name);
          await page.mouse.move(1,45);
        }
        await button(name,'open').focus();
        await page.keyboard.press('Shift+Tab');
        for (const act of ['comment','open','pin','rm']) {
          assert(await button(name,act).evaluate(el => document.activeElement === el && el.matches(':focus-visible') && getComputedStyle(el).outlineStyle !== 'none'), `visible keyboard focus: ${act}`);
          await page.waitForFunction(name => getComputedStyle(document.querySelector(`.entry[data-name="${name}"] .acts`)).opacity === '1', name);
          if (act !== 'rm') await page.keyboard.press('Tab');
        }
        for (const act of ['pin','open','comment']) {
          await page.keyboard.press('Shift+Tab');
          assert(await button(name,act).evaluate(el => document.activeElement === el), `reverse tab order: ${act}`);
        }
        await page.keyboard.press('Space');
        await comment(name,true);
        await exitComment(name);
        await activate(button(name,'pin'));
        await savedPin(name,true);
        await post(`/api/jellies/${name}/pin`, {pinned:false});
        await savedPin(name,false);
        page.once('dialog', dialog => dialog.dismiss());
        await activate(button(name,'rm'));
        assert((await page.request.get(`${base}/api/jellies/${name}`)).ok(), 'cancel Remove preserves fixture');
        page.once('dialog', dialog => dialog.accept());
        await activate(button(name,'rm'));
        await entry(name).waitFor({state:'detached'});
        assert((await page.request.get(`${base}/api/jellies/${name}`)).status() === 404, 'Remove deletes fixture');
        assert((await page.request.get(`${base}/api/jellies/${long}`)).ok(), 'Remove preserves other fixture');
      }
      return results;
    } catch (error) {
      throw Error(`After ${results.at(-1)?.label || 'setup'}: ${error.message}`);
    } finally {
      await page.goto('about:blank');
      for (const name of names) await page.request.delete(`${base}/api/jellies/${name}?by=test`);
    }
  };
  const mobile = await page.context().browser().newContext({viewport:{width:390,height:844},isMobile:true,hasTouch:true,deviceScaleFactor:3});
  try {
    const phone = await mobile.newPage();
    const phoneResults = await run(phone,[390],true);
    const desktopResults = await run(page,[1280,320,900,901],false);
    return {passed:true,phone:phoneResults,desktop:desktopResults};
  } finally {
    await mobile.close();
  }
}
