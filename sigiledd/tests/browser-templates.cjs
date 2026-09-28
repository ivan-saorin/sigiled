// Actual dashboard assets against a synthetic loopback provider. No live writes.
const fs=require('node:fs'),path=require('node:path'),http=require('node:http'),assert=require('node:assert/strict');
const {createRequire}=require('node:module');
const {chromium}=process.env.SIGIL_PLAYWRIGHT_PACKAGE?createRequire(process.env.SIGIL_PLAYWRIGHT_PACKAGE)('playwright'):require('playwright');
const root=process.env.SIGIL_ASSETS_DIR||path.resolve(__dirname,'../src/browser/assets');
let mode='uncertain',writes=[],designations=[],release,started;
const templates=[{name:'film-rust',repository:'fixture/film-rust'},{name:'film-python',repository:'fixture/film-python'}];
const sha='a'.repeat(40);
const server=http.createServer(async(req,res)=>{try{
 let raw='';for await(const chunk of req)raw+=chunk;
 const url=new URL(req.url,'http://fixture');
 const send=(body,status=200)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(body));};
 if(url.pathname==='/browser/session')return send({identity:{display_name:'Template fixture'},csrf_token:'fixture',features:{project_creation:true}});
 if(url.pathname==='/browser/api/overview')return send({projects:{items:[],total:0},attention:{items:[]}});
 if(url.pathname==='/browser/api/templates')return send({default:'vm-tmpl',templates});
 if(url.pathname.startsWith('/browser/api/templates/')){assert.equal(req.headers['x-sigil-csrf'],'fixture');designations.push([url.pathname,JSON.parse(raw)]);return send({enabled:JSON.parse(raw).enabled});}
 if(url.pathname==='/browser/api/projects'){
  assert.equal(req.headers['x-sigil-csrf'],'fixture');const body=JSON.parse(raw);writes.push(body);
  if(mode==='uncertain')return send({error:'destination_key_installation_uncertain'},502);
  if(mode==='conflict')return send({error:'destination_exists_or_changed'},409);
  if(mode==='delayed'){started();await new Promise(r=>release=r);}
  return send({name:body.name,provenance:{source_commit:sha}},writes.length===1?201:200);
 }
 const name=url.pathname.startsWith('/browser/assets/')?url.pathname.split('/').pop():'index.html';
 res.writeHead(200,{'content-type':name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html'});res.end(fs.readFileSync(path.join(root,name)));
 }catch(e){res.writeHead(500);res.end(String(e));}});
(async()=>{await new Promise(r=>server.listen(0,'127.0.0.1',r));const base='http://127.0.0.1:'+server.address().port;console.log('Template fixture listening');
 const browser=await chromium.launch({headless:true,...(process.env.SIGIL_BROWSER_EXECUTABLE?{executablePath:process.env.SIGIL_BROWSER_EXECUTABLE}:{})});
 try{const page=await browser.newPage();page.setDefaultTimeout(5000);page.on('pageerror',e=>{throw e;});
 await page.goto(base+'/ui/projects/new');
 await page.getByRole('option',{name:'fixture/film-rust',exact:true}).waitFor({state:'attached'});
 await page.getByLabel('Project name',{exact:true}).fill('new-film');await page.getByLabel('Template',{exact:true}).selectOption('film-rust');
 await page.getByLabel('Template revision (optional)').fill('release/v1');
 await page.getByRole('button',{name:'Create project',exact:true}).click();await page.getByText(/Retry keeps the same name/).waitFor();
 assert.equal(await page.getByLabel('Project name',{exact:true}).isDisabled(),true);
 assert.equal(await page.getByLabel('Template',{exact:true}).isDisabled(),true);
 mode='success';await page.getByRole('button',{name:'Create project',exact:true}).click();await page.getByText('Template commit: '+sha).waitFor();
 assert.deepEqual(writes,[{name:'new-film',template:'film-rust',template_ref:'release/v1'},{name:'new-film',template:'film-rust',template_ref:'release/v1'}]);
 mode='conflict';await page.getByLabel('Project name',{exact:true}).fill('incumbent');await page.getByRole('button',{name:'Create project',exact:true}).click();await page.getByText(/Choose a different project name/).waitFor();assert.equal(await page.getByLabel('Project name',{exact:true}).isDisabled(),false);
 await page.getByLabel('Project name',{exact:true}).fill('default-film');await page.getByLabel('Template',{exact:true}).selectOption('');await page.getByLabel('Template revision (optional)').fill('');
 mode='delayed';const seen=new Promise(r=>started=r);await page.getByRole('button',{name:'Create project',exact:true}).click();await Promise.race([seen,new Promise((_,reject)=>setTimeout(()=>reject(new Error('creation did not start')),5000))]);
 await page.getByRole('link',{name:'Projects',exact:true}).click();await page.goBack();await page.getByLabel('Project name',{exact:true}).waitFor();assert.equal(await page.getByRole('button',{name:'Create project',exact:true}).isDisabled(),true);
 release();await page.getByText('Template commit: '+sha).waitFor();assert.equal(await page.getByLabel('Project name',{exact:true}).isDisabled(),false);assert.deepEqual(writes.at(-1),{name:'default-film'});
 await page.getByText('Manage reusable templates',{exact:true}).click();await page.getByLabel('Template repository name').fill('reusable');
 await page.getByRole('button',{name:'Designate as template',exact:true}).click();await page.getByText('Template is available for selection.').waitFor();
 await page.getByRole('button',{name:'Remove template designation',exact:true}).click();await page.getByText('Template designation removed.').waitFor();
 assert.deepEqual(designations,[['/browser/api/templates/reusable',{enabled:true}],['/browser/api/templates/reusable',{enabled:false}]]);
 console.log('PASS template picker, pinned ref, default payload, uncertain retry, definitive conflict, navigation during creation, provenance, eligibility management and CSRF');
 }finally{await browser.close();server.close();}
})().catch(e=>{console.error(e);process.exitCode=1;release?.();server.close();});
