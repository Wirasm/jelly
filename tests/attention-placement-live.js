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
  `}});
  if(!response.ok()) throw Error('create: '+await response.text());
  try {
    await page.goto(base+'/j/'+name+'/?agent=1');
    // A live socket must receive the attention before the handler waits for its snapshot.
    await page.waitForFunction(()=>window.jelly);
    await page.waitForTimeout(100);
    const results=[];
    for(const kind of ['ask','look']) for(const layout of ['grid','flex','list','table']) {
      const selector='#'+layout+'0';
      const response=await page.request.post(api+'/attention',{data:{kind,title:'Check '+layout,by:'test',anchor:{selector},options:kind==='ask'?[{id:'yes',label:'Yes'}]:[]}});
      if(!response.ok()) throw Error(await response.text());
      const reply=await response.json(), id=reply.args.id;
      if(reply.anchor!=='below') throw Error(kind+' '+layout+' reply: '+JSON.stringify(reply));
      const snapshot=screens.find(screen=>screen.version>=reply.version && screen.below?.includes(id+' '+selector));
      if(!snapshot) throw Error('reply lacks browser-produced positive measurement for '+id);
      await page.evaluate(({id,layout})=>{
        const card=document.getElementById('jf-q-'+id).getBoundingClientRect();
        const anchor=document.getElementById(layout+'0').getBoundingClientRect();
        const later=document.getElementById(layout+'Later').getBoundingClientRect();
        if(card.top<anchor.bottom-1 || card.bottom>later.top+1) throw Error(layout+' card is not between anchor and later row');
      },{id,layout});
      results.push({kind,layout,anchor:reply.anchor,measurement:id+' '+selector});
    }
    return results;
  } finally {
    page.off('request',capture);
    await page.goto('about:blank');
    await page.request.delete(api);
  }
}
