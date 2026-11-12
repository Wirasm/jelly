// cargo build --locked
// JELLY_CHROME=/usr/bin/false target/debug/jelly serve --port 7879 --home <throwaway-directory>
// playwright-cli -s=jelly-placement open
// playwright-cli -s=jelly-placement run-code --filename=tests/attention-placement-live.js --raw
// playwright-cli -s=jelly-placement close
async (page) => {
  const base='http://127.0.0.1:7879';
  const name='placement-live-'+Date.now(), api=base+'/api/jellies/'+name;
  const screens=[];
  const capture=request=>{
    if(request.url()===api+'/screen') screens.push(request.postDataJSON());
  };
  page.on('request',capture);
  const response=await page.request.post(base+'/api/jellies',{data:{name,by:'test',verbs:'const verbs = {};',state:{},view:`
    <style>
      body {margin:20px;font:16px system-ui}
      #grid {display:grid;grid-template-columns:1fr 1fr;gap:12px}
      #flex {display:flex;flex-wrap:wrap;gap:12px}
      #flex > button {flex:0 0 40%;height:40px}
      table {width:100%}
    </style>
    <div id="grid"><button id="grid0">One</button><button>Two</button><button id="gridLater">Three</button></div>
    <div id="flex"><button id="flex0">One</button><button>Two</button><button id="flexLater">Three</button></div>
    <ul><li id="list0">One</li><li id="listLater">Two</li></ul>
    <table><tbody><tr id="table0"><td>One</td></tr><tr id="tableLater"><td>Two</td></tr></tbody></table>
    <button id="hidden" hidden>Hidden</button>
    <button id="floating" style="position:fixed;top:0">Floating</button>
    <div id="nowrap" style="display:flex"><button id="fallback">Cannot wrap</button></div>
    <button id="body-item" style="order:1">Body layout</button>
  `}});
  if(!response.ok()) throw Error('create: '+await response.text());
  try {
    await page.goto(base+'/j/'+name+'/?agent=1');
    // A live socket must receive the attention before the handler waits for its snapshot.
    await page.waitForFunction(()=>window.jelly);
    await page.waitForTimeout(100);
    const results=[];
    const cases=[
      ...['grid','flex','list','table'].map(layout=>({layout,selector:'#'+layout+'0',status:'below'})),
      {selector:'[',status:'invalid'},
      {selector:'',status:'invalid'},
      {selector:'#q-absent[',status:'invalid'},
      {selector:'#absent',status:'missing'},
      {selector:'#floating',status:'floating'},
      {selector:'#hidden',status:'hidden'},
      {selector:'#fallback',status:'fallback',destination:{kind:'after',selector:'#nowrap'}},
      {selector:'#body-item',status:'fallback',destination:{kind:'top'},bodyGrid:true},
    ];
    for(const inline of [false,true]) {
      await page.evaluate(inline=>{
        if(inline) {
          document.head.insertAdjacentHTML('beforeend','<meta name="jelly-attention" content="inline">');
          document.body.insertAdjacentHTML('beforeend','<section id="own-question" style="margin-top:1000px">The view places attention here</section>');
        }
      },inline);
      for(const kind of ['ask','look']) for(const test of cases.filter(test=>!inline || ['invalid','missing','floating','hidden'].includes(test.status))) {
        const {layout,selector,status,destination}=test;
        await page.evaluate(bodyGrid=>{
          document.body.style.display=bodyGrid?'grid':'';
          document.body.style.gridTemplateColumns=bodyGrid?'1fr':'';
        },!!test.bodyGrid);
        const response=await page.request.post(api+'/attention',{data:{kind,title:'Check '+selector,by:'test',anchor:{selector},options:kind==='ask'?[{id:'yes',label:'Yes'}]:[]}});
        if(!response.ok()) throw Error(await response.text());
        const reply=await response.json(), id=reply.args.id;
        if(!!reply.anchor_inline!==inline) throw Error('reply lost placement ownership: '+JSON.stringify(reply));
        if(reply.anchor!==status) throw Error(kind+' '+selector+' reply: '+JSON.stringify(reply));
        const snapshot=screens.find(screen=>screen.version>=reply.version && (status==='fallback'
          ? screen.fallback?.some(f=>f.id===id)
          : screen[status]?.includes(id+' '+selector)));
        if(!snapshot) throw Error('reply lacks browser-produced '+status+' measurement for '+id);
        if (['invalid','missing','floating'].includes(status)) {
          await page.evaluate(({id,inline})=>{
            const host=document.getElementById('jf-q-'+id);
            if(inline) {
              if(host || !document.getElementById('own-question')) throw Error('runtime replaced view-owned attention');
            } else if(!host?.closest('.jf-ask-top')) throw Error('selector fallback is not at top');
          },{id,inline});
          const seen=await page.request.get(api+'/see');
          const text=await seen.text();
          const fallback=inline?'placement not checked: the view places its own attention':'it shows at the top instead';
          const diagnosis={invalid:'has invalid CSS selector syntax',missing:'matches nothing on the page',floating:'is in a fixed or sticky part of the page'}[status];
          if(!seen.ok() || !text.includes(id+' asks about '+selector+', which '+diagnosis+'; '+fallback)) {
            throw Error('see lost selector diagnosis: '+text);
          }
          if(status!=='floating' && snapshot[status==='invalid'?'missing':'invalid']?.includes(id+' '+selector)) throw Error('selector reported in both diagnostic categories');
        }
        if(destination) {
          const measurement=snapshot.fallback.find(f=>f.id===id);
          if(measurement.destination.kind!==destination.kind || measurement.destination.selector!==destination.selector ||
             reply.anchor_destination?.kind!==destination.kind || reply.anchor_destination?.selector!==destination.selector ||
             !measurement.reason || reply.anchor_reason!==measurement.reason) {
            throw Error('fallback changed across snapshot boundary: '+JSON.stringify({measurement,reply,destination}));
          }
          await page.evaluate(({id,destination})=>{
            const host=document.getElementById('jf-q-'+id);
            let previous=host.parentElement.previousElementSibling;
            while(previous?.matches('.jf-slot')) previous=previous.previousElementSibling;
            if(destination.kind==='top' ? !host.closest('.jf-ask-top') : document.querySelector(destination.selector)!==previous) {
              throw Error('fallback card is not at reported destination');
            }
          },{id,destination});
        }
        if(layout) await page.evaluate(({id,layout})=>{
          const card=document.getElementById('jf-q-'+id).getBoundingClientRect();
          const anchor=document.getElementById(layout+'0').getBoundingClientRect();
          const later=document.getElementById(layout+'Later').getBoundingClientRect();
          if(card.top<anchor.bottom-1 || card.bottom>later.top+1) throw Error(layout+' card is not between anchor and later row');
        },{id,layout});
        results.push({inline,kind,selector,anchor:reply.anchor,destination:reply.anchor_destination,measurement:id+' '+selector});
      }
    }
    return results;
  } finally {
    page.off('request',capture);
    await page.goto('about:blank');
    await page.request.delete(api);
  }
}
