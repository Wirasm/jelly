// python3 -m http.server 7889 --bind 127.0.0.1
// playwright-cli -s=jelly-placement open
// playwright-cli -s=jelly-placement run-code --filename=tests/attention-placement.js --raw
// playwright-cli -s=jelly-placement close
async (page) => {
  const results=[];
  const settle=()=>page.waitForTimeout(650);
  for(const width of [1280,390]) {
    const errors=[];
    const onError=e=>errors.push(String(e));
    page.on('pageerror',onError);
    await page.setViewportSize({width,height:900});
    await page.goto('http://127.0.0.1:7889/tests/attention-placement.html?agent=1');
    await settle();
    await page.evaluate(()=>{
      window.assert=(ok,msg)=>{if(!ok) throw Error(msg)};
      window.rect=el=>el.getBoundingClientRect();
      window.host=id=>document.getElementById('jf-q-'+id);
      window.checkRow=(id,group,anchor)=>{
        const g=document.getElementById(group), a=document.getElementById(anchor), ar=rect(a), r=rect(host(id));
        const peers=[...g.children].filter(el=>!el.classList.contains('jf-ask'));
        const same=peers.filter(el=>{ const p=rect(el); return p.top<ar.bottom-1 && p.bottom>ar.top+1; });
        const later=peers.filter(el=>rect(el).top>=ar.bottom-1 && !same.includes(el));
        assert(same.every(el=>rect(el).bottom<=r.top+1),id+' after all row peers');
        assert(later.every(el=>rect(el).top>=r.bottom-1),id+' before later rows');
        const gr=rect(g),cs=getComputedStyle(g);
        assert(Math.abs(r.left-gr.left-parseFloat(cs.paddingLeft)-parseFloat(cs.borderLeftWidth))<=1,id+' left edge');
        assert(Math.abs(r.right-gr.right+parseFloat(cs.paddingRight)+parseFloat(cs.borderRightWidth))<=1,id+' right edge');
      };
      checkRow('grid','grid','grid-7'); checkRow('flex','flex','flex-7');
      checkRow('list','list','li2');
      assert(rect(host('table')).top>=rect(document.getElementById('tr1')).bottom-1,'table after row');
      assert(rect(host('table')).bottom<=rect(document.getElementById('tr2')).top+1,'table before next row');
      assert(host('table').parentElement.colSpan===3,'logical table colspan');
      assert(host('table').parentElement.parentElement.parentElement.tagName==='TBODY','valid table wrapper');
      assert(host('list').parentElement.tagName==='LI','valid list wrapper');
      assert(getComputedStyle(host('list').parentElement).counterIncrement==='list-item 0','slot does not increment list');
      assert(document.getElementById('list').start===4,'list start unchanged');
      window.originalHost=host('grid');
      window.originalTextarea=host('grid').shadowRoot.querySelector('textarea');
      originalTextarea.value='Keep this draft'; originalTextarea.focus();
      document.getElementById('grid-6').style.height='100px';
      items.push({...items[0],id:'grid2',anchor:{selector:'#grid-6'}});
      refresh();
    });
    await settle();
    await page.evaluate(()=>{
      checkRow('grid','grid','grid-7'); checkRow('grid2','grid','grid-6');
      assert(host('grid')===originalHost,'host survives redraw');
      assert(host('grid').shadowRoot.querySelector('textarea')===originalTextarea,'textarea survives redraw');
      assert(originalTextarea.value==='Keep this draft','draft survives redraw');
      assert(document.activeElement===host('grid'),'focus survives redraw');
      assert(host('grid').parentElement.nextElementSibling===host('grid2').parentElement,'stable same-row order');
      originalTextarea.setSelectionRange(5,9);
      document.getElementById('grid').style.gridTemplateColumns='repeat(4,minmax(0,1fr))';
    });
    await settle();
    await page.setViewportSize({width:width===1280?390:1280,height:900});
    await settle();
    await page.evaluate(()=>{
      checkRow('grid','grid','grid-7');
      assert(document.activeElement===host('grid') && host('grid').shadowRoot.activeElement===originalTextarea,'responsive relocation preserves focus');
      assert(originalTextarea.selectionStart===5 && originalTextarea.selectionEnd===9,'responsive relocation preserves selection');
    });
    await page.keyboard.insertText('this');
    await page.evaluate(()=>{
      assert(originalTextarea.value==='Keep this draft','typing and selection survive relocation');
    });
    await page.setViewportSize({width,height:900});
    await settle();
    await page.evaluate(()=>{
      checkRow('grid','grid','grid-7');
      assert(host('grid')===originalHost && originalTextarea.value==='Keep this draft','draft survives new row');
      window.addCase=(id,html,selector)=>{
        document.body.insertAdjacentHTML('beforeend',html);
        items.push({id,kind:'ask',title:id,by:'test',anchor:selector?{selector}:undefined,options:[{id:'yes',label:'Yes'}]});
      };
      for (const kind of ['grid','flex','list','table']) {
        const id='struct-'+kind;
        const children=Array.from({length:5},(_,i)=>kind==='table'?`<tr id="${id}-${i}"><td>${i}</td></tr>`:`<li id="${id}-${i}" style="min-height:40px">${i}</li>`).join('');
        const style=kind==='grid'?'display:grid;grid-template-columns:1fr 1fr':kind==='flex'?'display:flex;flex-wrap:wrap':'';
        const tag=kind==='table'?'tbody':'ul';
        addCase(id,`<style>#${id}>li {flex:0 0 50%;box-sizing:border-box}</style>${kind==='table'?'<table>':''}<${tag} id="${id}" style="${style}">${children}</${tag}>${kind==='table'?'</table>':''}`,`#${id} > :nth-last-child(2)`);
      }
      for (const tag of ['button','a','label','p','li','td','th']) {
        const id='promote-'+tag, outer=document.createElement(tag);
        outer.id=id;
        outer.innerHTML=`<div id="${id}-grid" style="display:grid;grid-template-columns:1fr 1fr"><div id="${id}-target">One</div><div>Two</div><div id="${id}-later">Three</div></div>`;
        if (tag==='li') { const list=document.createElement('ul'); list.append(outer); document.body.append(list); }
        else if (['td','th'].includes(tag)) { const table=document.createElement('table'); table.insertRow().append(outer); document.body.append(table); }
        else document.body.append(outer);
        items.push({id,kind:'ask',title:id,by:'test',anchor:{selector:'#'+id+'-target'}});
      }
      addCase('nested','<div id="outer" style="display:grid;grid-template-columns:1fr"><div id="nested" style="display:grid;grid-template-columns:1fr 1fr;gap:8px"><button id="n1"><b id="deep">One</b></button><button>Two</button><button id="n3">Three</button></div></div>','#deep');
      addCase('last','<div id="last" style="display:grid;grid-template-columns:1fr 1fr"><button>One</button><button id="last-tile">Two</button></div>','#last-tile');
      addCase('nowrap','<div id="nowrap" style="display:flex"><button id="nw">One</button><button>Two</button></div>','#nw');
      addCase('order','<div id="order" style="display:grid;grid-template-columns:1fr 1fr"><button id="ord" style="order:1">One</button><button>Two</button></div>','#ord');
      addCase('span','<div id="span" style="display:grid;grid-template-columns:1fr 1fr"><button id="sp" style="grid-row:span 2">One</button><button>Two</button><button>Three</button></div>','#sp');
      addCase('dense','<div id="dense" style="display:grid;grid-auto-flow:row dense"><button id="de">One</button><button>Two</button></div>','#de');
      for (const [id,style,childStyle] of [
        ['column-grid','display:grid;grid-auto-flow:column',''],
        ['explicit-row','display:grid;grid-template-columns:1fr 1fr','grid-row:2'],
        ['flex-order','display:flex;flex-wrap:wrap','order:1'],
        ['wrap-reverse','display:flex;flex-wrap:wrap-reverse',''],
        ['column-flex','display:flex;flex-direction:column;flex-wrap:wrap',''],
        ['vertical','display:grid;writing-mode:vertical-rl',''],
        ['transform','display:grid;grid-template-columns:1fr 1fr','transform:translateY(10px)'],
        ['overlap','display:flex;flex-wrap:wrap','margin-right:-10px']
      ]) addCase(id,`<div id="${id}" style="${style}"><button id="${id}-child" style="${childStyle}">One</button><button>Two</button></div>`,'#'+id+'-child');
      addCase('reject','<style>#reject > .jf-slot { width:50% !important }</style><div id="reject" style="display:grid;grid-template-columns:1fr 1fr"><button id="reject-child">One</button><button>Two</button><button>Three</button></div>','#reject-child');
      items.push({...items.find(a=>a.id==='reject'),id:'reject-group',anchor:{selector:'#reject'}});
      addCase('custom-list','<style>#custom-list > li::before {content:counter(custom);counter-increment:custom}</style><ol id="custom-list"><li id="custom-item">One</li><li>Two</li></ol>','#custom-item');
      addCase('styled-list','<ol id="styled-list" start="7" style="display:grid;grid-template-columns:1fr 1fr"><li id="styled-item">One</li><li>Two</li><li>Three</li></ol>','#styled-item');
      addCase('whole','<div id="whole" style="display:grid;grid-template-columns:1fr 1fr"><div>One</div><div>Two</div></div>','#whole');
      addCase('cross','<table id="cross"><tbody><tr><td id="cross-cell" rowspan="2">A</td><td>B</td></tr><tr><td>C</td></tr></tbody></table>','#cross-cell');
      addCase('zero-span','<table id="zero-span"><tbody><tr><td rowspan="0">A</td><td>B</td></tr><tr id="zero-last"><td>C</td></tr></tbody></table>','#zero-last');
      addCase('separate-table','<table id="separate-table" style="border-collapse:separate;border-spacing:4px"><tbody><tr id="separate-first"><td id="separate-cell">A</td><td>B</td></tr><tr id="separate-next"><td>C</td><td>D</td></tr></tbody></table>','#separate-cell');
      addCase('svg','<svg id="drawing" width="200" height="100"><circle id="circle" cx="50" cy="50" r="30"/></svg>','#circle');
      for(const pos of ['fixed','sticky','absolute']) addCase(pos,`<div style="position:${pos}"><span id="${pos}">Floating</span></div>`,'#'+pos);
      addCase('hidden','<div hidden><button id="hidden">Hidden</button></div>','#hidden');
      addCase('missing','', '#missing'); addCase('invalid','', '['); addCase('top','');
      addCase('ul','<ul id="ul"><li id="ul1">One</li><li>Two</li></ul>','#ul1');
      addCase('reverse','<ol id="reverse" reversed><li id="rev1">Three</li><li>Two</li><li>One</li></ol>','#rev1');
      addCase('reverse-start','<ol id="reverse-start" reversed start="10"><li id="revstart">Ten</li><li>Nine</li></ol>','#revstart');
      refresh();
    });
    await settle();
    await page.evaluate(()=>{
      window.gridWidth=document.getElementById('grid').getBoundingClientRect().width;
      document.styleSheets[0].insertRule('#grid { width:70% }',document.styleSheets[0].cssRules.length);
    });
    await settle();
    await page.evaluate(()=>{
      checkRow('grid','grid','grid-7');
      assert(rect(document.getElementById('grid')).width<gridWidth,'observed group resize');
      assert(host('grid')===originalHost && originalTextarea.value==='Keep this draft','group resize keeps draft');
    });
    for (let pass=0;pass<3;pass++) {
      await page.evaluate(()=>refresh());
      await settle();
      await page.evaluate(()=>{
        for (const kind of ['grid','flex','list','table']) {
          const id='struct-'+kind, slot=host(id).parentElement.closest('.jf-slot');
          assert(slot.previousElementSibling.id===id+'-3',id+' resolves original structural anchor');
          assert(rect(host(id)).bottom<=rect(document.getElementById(id+'-4')).top+1,id+' remains before later row');
          assert(!screens.at(-1).floating.some(m=>m.startsWith(id+' ')),id+' is not floating');
        }
        for (const tag of ['button','a','label','p','li','td','th']) {
          const id='promote-'+tag;
          if (['li','td','th'].includes(tag)) checkRow(id,id+'-grid',id+'-target');
          else {
            const f=screens.at(-1).fallback.find(f=>f.id===id);
            assert(f?.destination.kind==='after',id+' reports restricted-content fallback');
            assert(document.querySelector(f.destination.selector).nextElementSibling===host(id).parentElement,id+' reports actual boundary');
          }
        }
      });
    }
    await page.evaluate(()=>{
      for (const kind of ['grid','flex','list','table']) {
        for (const i of [3,4]) document.getElementById('struct-'+kind+'-'+i).remove();
      }
      refresh();
    });
    await settle();
    await page.evaluate(()=>{
      for (const kind of ['grid','flex','list','table']) {
        const id='struct-'+kind;
        assert(host(id).closest('.jf-slot').previousElementSibling.id===id+'-1',id+' re-resolves changed view structure');
        assert(rect(host(id)).bottom<=rect(document.getElementById(id+'-2')).top+1,id+' follows the new anchor row');
      }
    });
    const cdp=await page.context().newCDPSession(page);
    const {nodes}=await cdp.send('Accessibility.getFullAXTree');
    const ax=new Map(nodes.map(n=>[n.nodeId,n])), numbers=new Map();
    for(const n of nodes.filter(n=>n.role?.value==='ListMarker')) {
      const parent=ax.get(n.parentId);
      const {node}=await cdp.send('DOM.describeNode',{backendNodeId:parent.backendDOMNodeId});
      const attrs=node.attributes||[], idIndex=attrs.indexOf('id');
      if(idIndex>=0) numbers.set(attrs[idIndex+1],n.name?.value.trim());
    }
    await cdp.detach();
    for(const [id,number] of [['li1','4.'],['li2','5.'],['li3','12.'],['rev1','3.'],['revstart','10.'],['styled-item','7.']]) {
      if(numbers.get(id)!==number) throw Error(id+' numbering: '+numbers.get(id));
    }
    await page.evaluate(()=>{
      checkRow('nested','nested','n1'); checkRow('last','last','last-tile'); checkRow('ul','ul','ul1'); checkRow('reverse-start','reverse-start','revstart'); checkRow('styled-list','styled-list','styled-item');
      assert(document.getElementById('whole').nextElementSibling===host('whole').parentElement,'whole group anchor stays after group');
      const screen=screens.at(-1);
      assert(screen.errors.length===0,'runtime errors: '+screen.errors);
      for(const [id,selector] of [['nowrap','#nowrap'],['order','#order'],['span','#span'],['dense','#dense'],['cross','#cross'],['zero-span','#zero-span'],['svg','#drawing'],['reverse','#reverse'],...['column-grid','explicit-row','flex-order','wrap-reverse','column-flex','vertical','transform','overlap','reject','custom-list'].map(id=>[id,'#'+id])]) {
        const f=screen.fallback.find(f=>f.id===id);
        assert(f?.destination.kind==='after' && f.destination.selector===selector,id+' reports actual fallback');
        const boundary=document.querySelector(selector);
        assert(boundary.nextElementSibling===host(id).parentElement,id+' renders at reported fallback');
        assert(rect(host(id)).top>=rect(boundary).bottom-1,id+' below fallback group');
      }
      assert(host('separate-table').parentElement.parentElement.previousElementSibling.id==='separate-first','separated table row inserted after anchor');
      assert(rect(host('separate-table')).bottom<=rect(document.getElementById('separate-next')).top+1,'separated table before later row');
      assert(!screen.fallback.some(f=>f.id==='separate-table'),'separated table is supported');
      assert(host('reject').parentElement.nextElementSibling===host('reject-group').parentElement,'fallback and direct cards retain attention order');
      assert(screen.hidden.includes('hidden #hidden'),'hidden outcome preserved');
      for(const pos of ['fixed','sticky','absolute']) assert(screen.floating.some(m=>m.startsWith(pos+' ')),pos+' floating');
      for(const id of ['missing','invalid']) assert(screen.missing.some(m=>m.startsWith(id+' ')),id+' missing');
      assert(host('top').closest('.jf-ask-top'),'no anchor at top');
      assert(document.querySelectorAll('#reverse > li').length===3,'reversed auto numbering preserved');
      assert(document.getElementById('sp').style.gridRow==='span 2','original styles preserved');
      assert(document.querySelectorAll('#grid > .tile').length===17,'original items preserved');
      assert(document.documentElement.scrollWidth<=innerWidth,'no horizontal overflow');
      host('grid').shadowRoot.querySelector('.o').click();
      assert(requests.some(r=>r.url.endsWith('/grid/answer') && r.body.option==='yes' && r.body.text==='Keep this draft'),'answer still works after relocation');
      items.find(a=>a.id==='grid2').withdrawn=true;
      items.find(a=>a.id==='list').answer={option:'yes',text:'',at_ms:Date.now()};
      refresh();
    });
    await settle();
    await page.evaluate(()=>{
      assert(!host('grid2'),'withdrawn host removed');
      assert(!document.querySelector('#grid > .jf-slot + .jf-slot'),'withdrawn slot removed');
      assert(host('list').shadowRoot.querySelector('.answered'),'answered card shrinks');
      host('list').shadowRoot.querySelector('.chg').click();
      assert(host('list').shadowRoot.querySelector('textarea'),'answered card reopens');
      items.find(a=>a.id==='list').answer.at_ms=Date.now()-2*86400000;
      refresh();
    });
    await settle();
    await page.evaluate(()=>{
      assert(!host('list'),'aged-out host removed');
      assert(!document.querySelector('#list > .jf-slot'),'aged-out wrapper removed');
      window.stablePositions=[...document.querySelectorAll('.jf-slot')].map(el=>[el,rect(el).top]);
      window.movementCount=0;
      window.moveObserver=new MutationObserver(ms=>{movementCount+=ms.filter(m=>m.type==='childList').length});
      moveObserver.observe(document.body,{childList:true,subtree:true});
    });
    await settle();
    await page.evaluate(()=>{
      assert(stablePositions.every(([el,top])=>rect(el).top===top),'settled geometry stable');
      assert(movementCount===0,'no redraw loop'); moveObserver.disconnect();
      document.head.insertAdjacentHTML('beforeend','<meta name="jelly-attention" content="inline">');
      refresh();
    });
    await settle();
    await page.evaluate(()=>{
      assert(!document.querySelector('.jf-slot'),'inline attention opts out');
      const screen=screens.at(-1);
      assert(screen.missing.some(m=>m.startsWith('missing ')),'inline missing diagnostic');
      assert(screen.floating.some(m=>m.startsWith('fixed ')),'inline floating diagnostic');
      assert(screen.hidden.some(m=>m.startsWith('hidden ')),'inline hidden diagnostic');
      assert(!screen.below.length && !screen.fallback.length,'inline visible placements remain unchecked');
      document.querySelector('meta[name="jelly-attention"]').remove();
      document.body.innerHTML='<button id="body-item" style="order:1">Body layout</button>';
      document.body.style.cssText='display:grid;grid-template-columns:1fr';
      items=[{id:'body',kind:'ask',title:'Body layout',by:'test',anchor:{selector:'#body-item'}}];
      refresh();
    });
    await settle();
    await page.evaluate(()=>{
      const f=screens.at(-1).fallback.find(f=>f.id==='body');
      assert(f?.destination.kind==='top','no normal boundary reports top');
      assert(host('body').closest('.jf-ask-top'),'no normal boundary renders at top');
    });
    await page.evaluate(()=>{
      window.fetchBeforeDelay=window.fetch;
      window.fetch=(url,opts)=>opts ? fetchBeforeDelay(url,opts) : new Promise(resolve=>window.finishRefresh=()=>resolve({ok:true,json:async()=>({attention:items})}));
      socket.onmessage({data:JSON.stringify({jelly:'test:placement',kind:'attention',version:2,state:{},view_rev:1})});
      document.getElementById('body-item').textContent='Changed while refreshing';
    });
    await settle();
    await page.evaluate(()=>{
      const screen=screens.at(-1);
      assert(screen.version===2,'new version snapshot posted during refresh');
      assert(!screen.below.length && !screen.fallback.length,'pending refresh does not claim old placement');
      finishRefresh(); window.fetch=fetchBeforeDelay;
    });
    await settle();
    await page.evaluate(()=>assert(screens.at(-1).fallback.some(f=>f.id==='body'),'settled refresh measures placement'));
    for (const [newestFirst,eventKind] of ['attention','answer','withdraw','comment','resolve','reply'].map((kind,i)=>[i%2===1,kind])) {
      await page.evaluate((eventKind)=>{
        window.pending=[];
        window.fetch=(url,opts)=>opts ? fetchBeforeDelay(url,opts) : new Promise(resolve=>pending.push(resolve));
        for (let i=0;i<2;i++) {
          window.eventVersion=(window.eventVersion||2)+1;
          socket.onmessage({data:JSON.stringify({jelly:'test:placement',kind:eventKind,version:eventVersion,state:{},view_rev:1})});
        }
        window.oldAttention=[{id:'body',kind:'ask',title:'Old',by:'test',anchor:{selector:'#body-item'}}];
        window.newAttention=[{id:'body',kind:'ask',title:'New',by:'test',anchor:{selector:'#absent'}}];
      },eventKind);
      await page.evaluate((newestFirst)=>{
        const i=newestFirst?1:0;
        pending[i]({ok:true,json:async()=>({attention:i?newAttention:oldAttention})});
        document.getElementById('body-item').textContent='Overlapping refresh';
      },newestFirst);
      await settle();
      await page.evaluate((newestFirst)=>{
        const screen=screens.at(-1);
        assert(screen.version===eventVersion,'overlapping refresh snapshot has current version');
        assert(!screen.below.length && !screen.fallback.length,'older refresh cannot certify current placement');
        assert(newestFirst ? screen.missing.includes('body #absent') : !screen.missing.length,'only latest completed refresh publishes diagnostics');
        const i=newestFirst?0:1;
        pending[i]({ok:true,json:async()=>({attention:i?newAttention:oldAttention})});
        document.getElementById('body-item').textContent='Both refreshes finished';
      },newestFirst);
      await settle();
      await page.evaluate(()=>{
        assert(screens.at(-1).missing.includes('body #absent'),'late older response cannot overwrite current attention');
        assert(host('body').shadowRoot.querySelector('h3').textContent==='New','latest card retained');
        window.fetch=fetchBeforeDelay;
      });
    }
    for (const kind of ['grid','flex','list','table']) {
      await page.goto('http://127.0.0.1:7889/tests/attention-placement.html?agent=1');
      await settle();
      await page.evaluate((kind)=>{
        const group=document.getElementById(kind);
        const selector=kind==='table'?'#table:focus-within tbody > tr:first-child':`#${kind}:focus-within > :first-child`;
        const anchor=group.querySelector(kind==='table'?'tr':':scope > :first-child');
        document.head.insertAdjacentHTML('beforeend','<style>#grid:focus-within {grid-template-columns:1fr} #flex:focus-within > button {flex-basis:100%}</style>');
        anchor.tabIndex=0; anchor.focus();
        window.items=[{id:'focus',kind:'ask',title:'Focus',by:'test',anchor:{selector}}];
        refresh();
      },kind);
      await settle();
      await page.locator('#jf-q-focus textarea').fill('Still typing');
      await page.locator('#jf-q-focus textarea').focus();
      await page.evaluate(()=>refresh());
      await settle();
      await page.evaluate((kind)=>{
        const host=document.getElementById('jf-q-focus'), slot=host.closest('.jf-slot');
        const anchor=document.getElementById(kind).querySelector(kind==='table'?'tr':':scope > :first-child');
        const later=[...anchor.parentElement.children].filter(el=>!el.classList.contains('jf-ask')).slice(1);
        const r=host.getBoundingClientRect();
        if(slot.previousElementSibling!==anchor) throw Error(kind+' focus-dependent boundary');
        if(!later.every(el=>el.getBoundingClientRect().top>=r.bottom-1)) throw Error(kind+' focused card precedes later rows');
        if(!screens.at(-1).below.some(m=>m.startsWith('focus '))) throw Error(kind+' positive focused measurement');
        if(host.shadowRoot.activeElement?.value!=='Still typing') throw Error(kind+' focus draft retained');
      },kind);
      // Combine focus state with a structural selector that counts from the end.
      await page.evaluate((kind)=>{
        items[0].anchor.selector=kind==='table'?'#table:focus-within tbody > tr:nth-last-child(2)':`#${kind}:focus-within > :nth-last-child(2)`;
        refresh();
      },kind);
      await settle();
      await page.evaluate((kind)=>{
        const host=document.getElementById('jf-q-focus');
        const group=document.getElementById(kind), parent=kind==='table'?group.tBodies[0]:group;
        const originals=[...parent.children].filter(el=>!el.classList.contains('jf-ask'));
        if(host.closest('.jf-slot').previousElementSibling!==originals.at(-2)) throw Error(kind+' focus and structural selector identity');
      },kind);
      await page.evaluate((kind)=>{
        const group=document.getElementById(kind), parent=kind==='table'?group.tBodies[0]:group;
        for (const el of parent.children) if (!el.classList.contains('jf-ask')) el.dataset.focusProbe=':focus-within';
        items[0].anchor.selector=items[0].anchor.selector.replace(':focus-within', ':is(:f\\6f cus-within)/*:focus*/')+'[data-focus-probe=":focus-within'+String.fromCharCode(92,10)+'"]';
        refresh();
      },kind);
      await settle();
      await page.evaluate((kind)=>{
        const host=document.getElementById('jf-q-focus'), group=document.getElementById(kind);
        const parent=kind==='table'?group.tBodies[0]:group;
        const originals=[...parent.children].filter(el=>!el.classList.contains('jf-ask'));
        if(host.closest('.jf-slot').previousElementSibling!==originals.at(-2)) throw Error(kind+' escaped focus pseudo and literal attribute');
      },kind);
      for (const whitespace of ['', ' ', '\t', '\n', '\r', '\f', '\r\n']) {
        await page.evaluate(({kind,whitespace})=>{
          items[0].anchor.selector=kind==='table'
            ? '#table:fo\\63'+whitespace+'us-within tbody > tr:first-child'
            : '#'+kind+':fo\\63'+whitespace+'us-within > :first-child';
          if (!document.querySelector(items[0].anchor.selector)) throw Error(kind+' native escaped focus selector');
          refresh();
        },{kind,whitespace});
        await settle();
        await page.evaluate(({kind,whitespace})=>{
          const group=document.getElementById(kind), parent=kind==='table'?group.tBodies[0]:group;
          const anchor=parent.firstElementChild, host=document.getElementById('jf-q-focus');
          if(host.closest('.jf-slot').previousElementSibling!==anchor) throw Error(kind+' escaped focus whitespace '+JSON.stringify(whitespace));
          if(!screens.at(-1).below.includes('focus '+items[0].anchor.selector)) throw Error(kind+' escaped focus measurement '+JSON.stringify(whitespace));
        },{kind,whitespace});
      }
      // Escapes past U+10FFFF become U+FFFD; the forgiving list keeps its valid branch.
      for (const hex of ['0','10ffff','110000','ffffff']) for (const focus of ['',':focus-within']) {
        await page.evaluate(({kind,hex,focus})=>{
          items[0].anchor.selector=':is(:\\'+hex+' , #'+kind+')'+focus+(kind==='table'?' tbody > tr:first-child':' > :first-child');
          if (!document.querySelector(items[0].anchor.selector)) throw Error(kind+' native out-of-range escape '+hex);
          refresh();
        },{kind,hex,focus});
        await settle();
        await page.evaluate(({kind,hex,focus})=>{
          const group=document.getElementById(kind), parent=kind==='table'?group.tBodies[0]:group;
          const host=document.getElementById('jf-q-focus');
          if(host.closest('.jf-slot').previousElementSibling!==parent.firstElementChild) throw Error(kind+' escaped code point '+hex+focus);
          if(!screens.at(-1).below.includes('focus '+items[0].anchor.selector)) throw Error(kind+' escaped code point measurement '+hex+focus);
        },{kind,hex,focus});
      }
      for (const pseudo of ['focus-within','focus','focus-visible']) {
        await page.evaluate(({kind,pseudo})=>{
          const group=document.getElementById(kind), parent=kind==='table'?group.tBodies[0]:group;
          const anchor=parent.firstElementChild;
          const decoy=group.cloneNode(true);
          decoy.querySelectorAll('.jf-ask').forEach(el=>el.remove());
          decoy.removeAttribute('id');
          decoy.querySelectorAll('[id]').forEach(el=>el.removeAttribute('id'));
          group.before(decoy);
          window.focusDecoy=decoy;
          const target=pseudo==='focus-within'?group:anchor;
          const falseTarget=pseudo==='focus-within'?decoy:decoy.querySelector(kind==='table'?'tr':':scope > :first-child');
          target.classList.add('focus-target'); falseTarget.classList.add('focus-target');
          for(const name of ['focus','focus-visible','focus-within']) falseTarget.setAttribute('data-jf-'+name,'view-owned');
          anchor.tabIndex=0; anchor.focus();
          const suffix=pseudo==='focus-within'?(kind==='table'?' tbody > tr:first-child':' > :first-child'):'';
          items[0].anchor.selector='.focus-target:'+pseudo+suffix;
          if(document.querySelector(items[0].anchor.selector)!==anchor) throw Error(kind+' native focus identity '+pseudo);
          refresh();
        },{kind,pseudo});
        await settle();
        await page.evaluate(({kind,pseudo})=>{
          const group=document.getElementById(kind), parent=kind==='table'?group.tBodies[0]:group;
          const host=document.getElementById('jf-q-focus');
          if(host.closest('.jf-slot').previousElementSibling!==parent.firstElementChild) throw Error(kind+' view marker collision '+pseudo);
          const suffix=pseudo==='focus-within'?(kind==='table'?' tbody > tr:first-child':' > :first-child'):'';
          items[0].anchor.selector='.focus-target:not([data-jf-'+pseudo+']):'+pseudo+suffix;
          if(document.querySelector(items[0].anchor.selector)!==parent.firstElementChild) throw Error(kind+' native attribute selector '+pseudo);
          refresh();
        },{kind,pseudo});
        await settle();
        await page.evaluate(({kind,pseudo})=>{
          if(!screens.at(-1).below.includes('focus '+items[0].anchor.selector)) throw Error(kind+' view attribute semantics '+pseudo);
          const group=document.getElementById(kind), parent=kind==='table'?group.tBodies[0]:group;
          const target=pseudo==='focus-within'?group:parent.firstElementChild;
          for(const name of ['focus','focus-visible','focus-within']) {
            if(target.hasAttribute('data-jf-'+name)) throw Error(kind+' temporary attribute leaked');
            if(focusDecoy.querySelector('.focus-target')?.getAttribute('data-jf-'+name)!=='view-owned' && focusDecoy.getAttribute('data-jf-'+name)!=='view-owned') throw Error(kind+' view attribute changed');
          }
          target.classList.remove('focus-target');
          focusDecoy.remove();
        },{kind,pseudo});
      }
    }
    if(errors.length) throw Error(errors.join('\n'));
    page.off('pageerror',onError);
    results.push({width,passed:true});
  }
  return results;
}
