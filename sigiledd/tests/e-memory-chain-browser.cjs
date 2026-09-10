// Exact actual producer -> typed adapter exports; synthetic session/HTTP transport only.
const fs=require('node:fs'),http=require('node:http'),path=require('node:path'),assert=require('node:assert/strict');
const {chromium}=require('/workspace/target/gateway-browser/node_modules/playwright');
const chain=JSON.parse(fs.readFileSync('target/e-memory-adapter-chain.json','utf8')),out=chain.outputs,ops=chain.operations;
const root=path.resolve(__dirname,'../src/browser/assets'),output='target/e-memory-chain-browser';fs.mkdirSync(output,{recursive:true});
let writes=[],annotated=false;
const server=http.createServer(async(req,res)=>{try{
 const u=new URL(req.url,'http://fixture');let raw='';for await(const x of req)raw+=x;const b=raw?JSON.parse(raw):null;
 const reply=(v,status=200)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(v));};
 if(u.pathname==='/browser/session')return reply({identity:{display_name:'Synthetic accepted research fixture'},actor:{driver:'human:fixture'},csrf_token:'csrf',features:{memory_adapter:true}});
 if(u.pathname==='/browser/api/memory')return reply({indexes:[{name:'demo',rows:out.browse.items.length}],readiness:'ready',observed_at:1789041600});
 if(u.pathname.startsWith('/browser/api/memory/indexes/demo/')){
  const part=u.pathname.split('/demo/')[1];
  if(req.method==='GET'){
   if(part==='chunks')return reply(out.browse);
   if(part.startsWith('chunks/'))return reply(annotated?{...out.chunk,curation:out.curate.curation}:out.chunk);
   if(part.startsWith('manual/'))return reply(out.manual);
  }
  const action=part==='manual'?'create':part==='curation'?'curate':part==='forget/preview'?'preview':part==='forget'?'forget':null;
  if(action){const op=ops.find(o=>o.action===action);assert.deepEqual(b,op.request);writes.push(action);if(action==='curate')annotated=true;return reply(out[action],op.status);}
 }
 if(u.pathname.startsWith('/browser/api/'))return reply({error:'unexpected_fixture_api'},404);
 const name=u.pathname.startsWith('/browser/assets/')?u.pathname.split('/').pop():'index.html';res.writeHead(200,{'content-type':name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html'});res.end(fs.readFileSync(path.join(root,name)));
 }catch(e){fs.appendFileSync(output+'/errors.log',String(e)+'\n');res.writeHead(500);res.end('fixture mismatch');}});
(async()=>{await new Promise(r=>server.listen(0,'127.0.0.1',r));const base='http://127.0.0.1:'+server.address().port;const browser=await chromium.launch({headless:true});try{
 const page=await browser.newPage({viewport:{width:1440,height:1000}});page.setDefaultTimeout(10000);let errors=[];page.on('pageerror',e=>errors.push(String(e)));await page.addInitScript(()=>{crypto.randomUUID=()=> '00000000-0000-4000-8000-000000000098';});
 await page.goto(base+'/ui/memory?index=demo');await page.getByRole('heading',{name:'Memory',exact:true,level:1}).waitFor();
 await page.getByRole('button',{name:'New manual memory',exact:true}).click();await page.getByLabel('Memory text',{exact:true}).fill('Stage E manual memory');await page.getByLabel('Memory tags',{exact:true}).fill('integration');await page.getByRole('button',{name:'Save memory',exact:true}).click();await page.getByRole('button',{name:'Saved',exact:true}).waitFor();
 await page.goto(base+'/ui/memory?index=demo&memory='+out.chunk.id);await page.getByRole('heading',{name:'Source document',exact:true}).waitFor();assert.equal(await page.getByRole('button',{name:'Edit source',exact:true}).isEnabled(),true);assert.equal(await page.getByLabel('Memory text',{exact:true}).count(),0);
 await page.getByLabel('Annotation',{exact:true}).fill('Stage E retained annotation');await page.getByRole('button',{name:'Add annotation',exact:true}).click();await page.getByText('Curation saved. This does not change search-indexing status.',{exact:true}).waitFor();
 await page.getByText('Forget from Memory',{exact:true}).click();await page.getByRole('button',{name:'Preview forget',exact:true}).click();await page.getByText('Affected records: '+out.preview.affected,{exact:true}).waitFor();await page.getByRole('button',{name:'Confirm forget',exact:true}).click();await page.getByText(/Forgotten from Memory. Suppression is saved/).waitFor();
 assert.deepEqual(writes,['create','curate','preview','forget']);assert.deepEqual(errors,[]);await page.screenshot({path:output+'/desktop.png',fullPage:true});await page.setViewportSize({width:390,height:844});await page.screenshot({path:output+'/mobile.png',fullPage:true});fs.writeFileSync(output+'/result.json',JSON.stringify({writes,errors,actualReturnedSourceAssociation:true,originalSourceReadonly:true,exactPreviewConfirmation:true}));console.log('PASS actual Memory producer/adapter bodies in authored assets');
 }finally{await browser.close();await new Promise(r=>server.close(r));}})().catch(e=>{console.error(e);process.exitCode=1;});
