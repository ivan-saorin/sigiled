const fs=require('node:fs'),https=require('node:https'),http=require('node:http'),net=require('node:net'),cp=require('node:child_process'),path=require('node:path'),assert=require('node:assert/strict');
const root=process.env.GATEWAY_SMOKE_ROOT,repo=process.env.GATEWAY_SMOKE_REPO,remote=process.env.GATEWAY_SMOKE_REMOTE,branch=process.env.GATEWAY_SMOKE_BRANCH,upstream=new URL(process.env.GATEWAY_SMOKE_UPSTREAM),url=new URL(process.env.GATEWAY_SMOKE_URL);
const {trustedActivity,Diagnostics}=require('./ide-provider-checks.cjs');
const {chromium}=require('/workspace/target/gateway-browser/node_modules/playwright');
(async()=>{
 cp.execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',root+'/edge.key','-out',root+'/edge.crt','-days','1','-subj','/CN=*.ide.example.test'],{stdio:'ignore'});
 const pubkey=cp.execFileSync('openssl',['x509','-in',root+'/edge.crt','-pubkey','-noout']);const der=cp.execFileSync('openssl',['pkey','-pubin','-outform','DER'],{input:pubkey});const spki=require('node:crypto').createHash('sha256').update(der).digest('base64');
 const tunnels=new Set();
 const edge=https.createServer({key:fs.readFileSync(root+'/edge.key'),cert:fs.readFileSync(root+'/edge.crt')},(req,res)=>{const p=http.request({host:'127.0.0.1',port:upstream.port,path:req.url,method:req.method,headers:req.headers},r=>{res.writeHead(r.statusCode,r.headers);r.pipe(res)});p.on('error',()=>{res.writeHead(502);res.end()});req.pipe(p);});
 edge.on('upgrade',(req,socket,head)=>{tunnels.add(socket);socket.on('close',()=>tunnels.delete(socket));const p=net.connect(Number(upstream.port),'127.0.0.1',()=>{p.write(`${req.method} ${req.url} HTTP/1.1\r\n`+Object.entries(req.headers).map(([k,v])=>`${k}: ${v}\r\n`).join('')+'\r\n');if(head.length)p.write(head);p.pipe(socket);socket.pipe(p)});p.on('error',()=>socket.destroy());socket.on('error',()=>p.destroy());socket.on('close',()=>p.destroy());});
 await new Promise(r=>edge.listen(0,'127.0.0.1',r));let browser,diagnostics,closing=false,finishing=false;
 try {
  browser=await chromium.launch({headless:true,args:[`--host-resolver-rules=MAP *.example.test 127.0.0.1:${edge.address().port}`,'--no-proxy-server',`--ignore-certificate-errors-spki-list=${spki}`],env:{...process.env,XDG_CONFIG_HOME:root+'/browser-config',XDG_CACHE_HOME:root+'/browser-cache'}});
  const context=await browser.newContext({ignoreHTTPSErrors:true,viewport:{width:1440,height:1000}});
  diagnostics=new Diagnostics();
  context.on('page',p=>{p.on('console',m=>{if(m.type()==='error'){const row={text:m.text(),url:m.location().url,phase:finishing?'finish':'active'};diagnostics.console.push(row);fs.appendFileSync(root+'/browser-console.log',JSON.stringify(row)+'\n');}});p.on('pageerror',e=>diagnostics.console.push({text:e.message,url:p.url().split('#')[0]}));});
  context.on('response',r=>{if(r.status()>=400){const row={url:r.url(),method:r.request().method(),status:r.status()};diagnostics.http.push(row);fs.appendFileSync(root+'/network.log',JSON.stringify(row)+'\n');}});
  context.on('response',async r=>{if(r.url().endsWith('/_sigil/operation'))fs.appendFileSync(root+'/operation-responses.jsonl',JSON.stringify({status:r.status(),body:await r.json()})+'\n');});
  context.on('response',r=>{if(r.url().endsWith('/_sigil/activity'))fs.appendFileSync(root+'/activity-network.log',JSON.stringify({event:'response',url:r.url(),status:r.status(),method:r.request().method(),type:r.request().resourceType()})+'\n');});
  context.on('requestfinished',r=>{if(r.url().endsWith('/_sigil/activity'))fs.appendFileSync(root+'/activity-network.log',JSON.stringify({event:'finished',url:r.url(),method:r.method(),type:r.resourceType()})+'\n');});
  context.on('requestfailed' ,r=>{const row={url:r.url(),error:r.failure()?.errorText||'unknown',method:r.method(),resourceType:r.resourceType(),navigation:r.isNavigationRequest(),phase:finishing?'finish':closing?'shutdown':'active'};diagnostics.network.push(row);});
  let page=await context.newPage();global.smokePage=page;let workers=0,resources=0,wsFrames=0,activities=0;const sockets=[];
  page.on('request',r=>{if(r.url().endsWith('/_sigil/activity'))activities++;});
  page.on('worker',()=>workers++);page.on('websocket',ws=>{sockets.push(ws.url());ws.on('framereceived',()=>wsFrames++);});
  page.on('response',r=>{if(r.url().includes('/static/')&&r.ok())resources++;});
  await page.goto(url.toString());await page.waitForURL('**/_sigil/workbench',{timeout:15000});
  let frame=page.frameLocator('#editor');await frame.locator('.monaco-workbench').waitFor({timeout:45000});
  await frame.getByRole('tab',{name:/navigation.txt/}).first().waitFor({timeout:30000});
  await frame.getByText('Ln 2, Col 3',{exact:true}).waitFor({timeout:15000});
  assert.equal(await page.evaluate(()=>window.isSecureContext),true);
  await page.waitForFunction(async()=>{const registrations=await navigator.serviceWorker.getRegistrations();return registrations.some(r=>r.active?.scriptURL===location.origin+'/_static/out/browser/serviceWorker.js'&&r.scope===location.origin+'/');},{},{timeout:10000});
  assert(workers>0,'actual provider workers must start');assert(resources>3,'actual provider static resources must load');assert(wsFrames>0,'actual provider WebSocket messages must arrive');
  assert(sockets.every(s=>s.startsWith('wss://'+url.host+'/')),'no loopback or insecure WebSocket authority');
  assert.equal(activities,0,'background provider traffic cannot claim human login activity');
  await frame.locator('.monaco-editor textarea').first().focus();
  const activityStatus=await trustedActivity(page,()=>page.keyboard.press('ArrowRight'));
  await page.screenshot({path:root+'/provider-workbench.png'});
  async function helperStatus(){return new Promise((resolve,reject)=>{http.get('http://127.0.0.1:8090/status',{headers:{Authorization:'Bearer fixture-helper-control-token-00000'}},r=>{let b='';r.on('data',d=>b+=d);r.on('end',()=>resolve(JSON.parse(b)));}).on('error',reject);});}
  async function until(check,message){const end=Date.now()+15000;while(Date.now()<end){if(await check())return;await new Promise(r=>setTimeout(r,100));}throw Error(message);}
  const prefix=new URL(sockets[0]).pathname;
  for(const file of ['vsda.js','vsda_bg.wasm'])diagnostics.expectHttp(url.origin+prefix+'/static/node_modules/vsda/rust/web/'+file,404);
  diagnostics.expectNetwork(url.origin+prefix+'/static/node_modules/vsda/rust/web/vsda.js','net::ERR_ABORTED',{status:404,resourceType:'script'});
  diagnostics.expectConsole(`Refused to execute script from '${url.origin+prefix}/static/node_modules/vsda/rust/web/vsda.js' because its MIME type ('application/json') is not executable, and strict MIME type checking is enabled.`);

  await until(async()=>(await helperStatus()).activity_observation==='ready','extension startup must establish observation');
  const before=await helperStatus();assert.equal(before.busy,false);
  await frame.locator('.monaco-editor .view-lines').first().click({position:{x:40,y:10}});
  await page.keyboard.press('Control+a');await page.keyboard.type('Saved through actual browser editor\n');await page.keyboard.press('Control+s');
  await until(()=>fs.readFileSync(repo+'/navigation.txt','utf8')==='Saved through actual browser editor\n','actual browser save');
  await page.keyboard.press('Control+`');
  const terminal=frame.locator('.xterm-helper-textarea').first();await terminal.waitFor({state:'attached',timeout:15000});await terminal.focus();
  await page.keyboard.type("printf 'terminal fixture\\n' > e-terminal.txt; git add e-terminal.txt; sleep 3",{delay:8});await page.keyboard.press('Enter');
  await until(async()=>(await helperStatus()).busy===true,'installed activity extension must observe terminal command_start');
  await until(async()=>(await helperStatus()).busy===false&&fs.existsSync(repo+'/e-terminal.txt'),'terminal ends without permanent busy');
  assert.equal(fs.readFileSync(repo+'/e-terminal.txt','utf8'),'terminal fixture\n');
  assert(cp.execFileSync('git',['-C',repo,'diff','--cached','--name-only'],{encoding:'utf8'}).includes('e-terminal.txt'),'real terminal Git staging');
  const hook=remote+'/hooks/pre-receive';fs.writeFileSync(hook,'#!/bin/sh\nexit 1\n',{mode:0o700});
  diagnostics.expectHttp(url.origin+'/_sigil/operation',409,'POST');
  await page.getByRole('button',{name:'Checkpoint & push',exact:true}).click();await page.getByRole('status').filter({hasText:'Your work is preserved'}).waitFor({timeout:40000});
  assert.equal(fs.readFileSync(repo+'/navigation.txt','utf8'),'Saved through actual browser editor\n');
  fs.unlinkSync(hook);await page.getByRole('button',{name:'Checkpoint & push',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Saved files checkpointed and pushed.'}).waitFor({timeout:40000});
  const pushed=cp.execFileSync('git',['-C',remote,'rev-parse',branch],{encoding:'utf8'}).trim();
  assert.equal(cp.execFileSync('git',['-C',remote,'show',pushed+':navigation.txt'],{encoding:'utf8'}),'Saved through actual browser editor\n');
  assert.equal(cp.execFileSync('git',['-C',remote,'show',pushed+':e-terminal.txt'],{encoding:'utf8'}),'terminal fixture\n');
  const idleBefore=await helperStatus();await new Promise(r=>setTimeout(r,2200));await page.evaluate(()=>fetch('/_sigil/status').then(r=>r.json()));const idleAfter=await helperStatus();assert(idleAfter.idle_secs>=idleBefore.idle_secs+1,'passive status/provider traffic must age activity');
  fs.writeFileSync(root+'/integration-evidence.json',JSON.stringify({pushed,savedBlob:true,terminalGit:true,installedActivityExtension:true,busyEnded:true,idleBefore:idleBefore.idle_secs,idleAfter:idleAfter.idle_secs,failedPushPreserved:true},null,2));

  if(process.env.SIGIL_TEST_DISCONNECT_ONLY==='1') {
  await page.close();await new Promise(r=>setTimeout(r,500));
  assert.equal(fs.readFileSync(repo+'/navigation.txt','utf8'),'Saved through actual browser editor\n');
  assert.equal(cp.execFileSync('git',['-C',remote,'rev-parse',branch],{encoding:'utf8'}).trim(),pushed);
  assert.equal((await helperStatus()).state,'ready');
  page=await context.newPage();global.smokePage=page;await page.goto(url.origin+'/_sigil/workbench');frame=page.frameLocator('#editor');await frame.locator('.monaco-workbench').waitFor({timeout:30000});
  await until(async()=>(await helperStatus()).activity_observation==='stale','replacement observer cannot release earlier custody');
  page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'Finish workspace',exact:true}).click();await page.getByRole('status').filter({hasText:'terminal observation unavailable'}).waitFor({timeout:15000});
  assert.equal(fs.readFileSync(repo+'/navigation.txt','utf8'),'Saved through actual browser editor\n');assert.equal((await helperStatus()).state,'ready');
  fs.writeFileSync(root+'/disconnect-preservation.json',JSON.stringify({savedBytesPreserved:true,remoteUnchanged:true,providerPreserved:true,reopenedEditing:true,observation:'stale',finishBlocked:true,operatorRecoveryRequired:true}));
  console.log('PASS disconnected workspace preserved with explicit uncertain observation');return;
  }
  const resource='/vscode-remote-resource?path='+encodeURIComponent(repo+'/project.html');
  diagnostics.expectHttp(url.origin+resource.replace('remote','%72emote'),404);
  const result=await page.evaluate(async resource=>{const r=await fetch(resource);await r.text();return {status:r.status,disposition:r.headers.get('content-disposition'),csp:r.headers.get('content-security-policy')};},resource);
  fs.writeFileSync(root+'/resource-probe.json',JSON.stringify(result));assert.equal(result.status,200);assert.equal(result.disposition,'attachment');assert(result.csp.includes('sandbox'));

  const variants=await page.evaluate(async({prefix,resource})=>{const results=[];for(const path of [prefix+resource,resource.replace('remote','%72emote')]){const r=await fetch(path);await r.text();results.push({status:r.status,disposition:r.headers.get('content-disposition'),csp:r.headers.get('content-security-policy')});}return results;},{prefix,resource});assert.equal(variants[0].status,200);assert.equal(variants[0].disposition,'attachment');assert(variants[0].csp.includes('sandbox'));assert.equal(variants[1].status,404);
  diagnostics.expectHttp(url.origin+resource,403);
  const attack=await context.newPage();await attack.goto(url.origin+resource);assert.equal(await attack.evaluate(()=>window.compromised),undefined);assert.match(await attack.locator('body').innerText(),/project_resource_navigation_denied/);
  diagnostics.expectHttp(url.origin+'/proxy/3000/',409);
  const proxy=await page.evaluate(async()=>{const r=await fetch('/proxy/3000/');await r.text();return r.status;});assert.equal(proxy,409);
  await page.evaluate(()=>{window.open=()=>null;});await page.getByRole('button',{name:'Open preview',exact:true}).click();
  const link=page.getByRole('link',{name:'Continue to preview'});await link.waitFor();const previewUrl=await link.getAttribute('href');assert(new URL(previewUrl).host.endsWith('.preview.example.test'));
  const previewPage=await context.newPage();await previewPage.goto(previewUrl);await previewPage.getByRole('heading',{name:'Isolated project preview'}).waitFor();
  const previewOrigin=new URL(previewUrl).origin,controlUrl=url.origin+'/_sigil/session';
  diagnostics.expectHttp(previewOrigin+'/_sigil/session',403);diagnostics.expectHttp(previewOrigin+'/_sigil/activity',403,'POST');diagnostics.expectHttp(controlUrl,403);diagnostics.expectNetwork(controlUrl,'net::ERR_FAILED',{resourceType:'fetch'});
  diagnostics.expectConsole(`Access to fetch at '${controlUrl}' from origin '${previewOrigin}' has been blocked by CORS policy: No 'Access-Control-Allow-Origin' header is present on the requested resource.`);
  diagnostics.expectConsole('Failed to load resource: net::ERR_FAILED',controlUrl);
  diagnostics.expectConsole(`WebSocket connection to '${url.origin.replace('https:','wss:')}/' failed: Error during WebSocket handshake: Unexpected response code: 403`);
  const boundary=await previewPage.evaluate(async ide=>{
    const own=await fetch('/_sigil/session');await own.text();const activity=await fetch('/_sigil/activity',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});await activity.text();
    let cross;try{await fetch(ide+'/_sigil/session',{credentials:'include'});cross='allowed';}catch{cross='blocked';}
    const ideWs=await new Promise(resolve=>{const socket=new WebSocket(ide.replace('https:','wss:')+'/');socket.onopen=()=>{resolve('allowed');socket.close();};socket.onerror=()=>resolve('blocked');});
    return {own:own.status,activity:activity.status,cross,ideWs,cookie:document.cookie};
  },url.origin);assert.deepEqual(boundary,{own:403,activity:403,cross:'blocked',ideWs:'blocked',cookie:''});
  await previewPage.screenshot({path:root+'/isolated-preview.png'});

  async function commandPalette(name){await frame.locator('.monaco-workbench').click({position:{x:300,y:15}});await page.keyboard.press('Control+Shift+p');const input=frame.locator('.quick-input-widget input').first();await input.fill('>'+name);await page.keyboard.press('Enter');}
  await commandPalette('Sigil Fixture: Open unsupported terminal');
  await until(async()=>(await helperStatus()).activity_observation==='unsupported','unsupported shell is explicit uncertainty');
  page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'Finish workspace',exact:true}).click();
  await page.getByRole('status').filter({hasText:'terminal observation unavailable'}).waitFor({timeout:15000});
  assert.equal(fs.readFileSync(repo+'/navigation.txt','utf8'),'Saved through actual browser editor\n');
  await commandPalette('Sigil Fixture: Close unsupported terminal');
  await until(async()=>(await helperStatus()).activity_observation==='ready','confirmed unsupported terminal closure recovers observation');
  // Exact observed pinned-provider shutdown diagnostics, only after this
  // deliberate finish; all are retained in diagnostics.json. Other errors fail.
  const script=url.origin+prefix+'/static/out/vs/code/browser/workbench/workbench.js';
  diagnostics.expectConsole('%c  ERR color: #f33 CloseEvent',script,'finish');
  diagnostics.expectConsole('%c  ERR color: #f33 CodeExpectedError: WebSocket close with status code 1006\n    at WebSocket.<anonymous> ('+script+':5596:30044)',script,'finish');
  for(const id of ['join.disconnectRemote','join.chatEditingSession'])diagnostics.expectConsole('%c  ERR color: #f33 [lifecycle] Long running operations during shutdown are unsupported in the web (id: '+id+')',script,'finish');
  for(const socket of sockets){const u=new URL(socket),token=u.searchParams.get('reconnectionToken');if(!token)continue;u.searchParams.set('reconnection','true');diagnostics.expectConsole(`WebSocket connection to '${u.toString()}' failed: Error during WebSocket handshake: Unexpected response code: 503`,script,'finish');for(const kind of ['Management   ','ExtensionHost'])diagnostics.expectConsole(`%c  ERR color: #f33 [remote-connection][${kind}][${token.slice(0,5)}\u2026][reconnect][WebSocket(${url.host}:443)] socketFactory.connect() failed or timed out. Error:`,script,'finish');}
  diagnostics.expectNetwork(url.origin+prefix+'/static/node_modules/vsda/rust/web/vsda_bg.wasm','net::ERR_ABORTED',{status:404,resourceType:'fetch',phase:'finish'});
  finishing=true;
  page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'Finish workspace',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Workspace finished.'}).waitFor({timeout:45000});
  assert.equal(await page.locator('#editor').count(),0);
  diagnostics.check();
  fs.writeFileSync(root+'/browser-evidence.json' ,JSON.stringify({browser:browser.version(),serviceWorker:true,variants,boundary,activityStatus,activities,workers,resources,wsFrames,sockets:sockets.map(u=>u.split('?')[0]),failures:diagnostics.http,consoleExceptions:diagnostics.console,resource:result,secureContext:true,file:'navigation.txt',line:2,column:3},null,2));
  console.log('PASS pinned provider frame, resources, secure WSS messages, file line/column, resource sandbox and proxy denial');
 }catch(e){fs.writeFileSync(root+'/primary-failure.log',e.stack);if(global.smokePage){await global.smokePage.screenshot({path:root+'/provider-failure.png'}).catch(()=>{});fs.writeFileSync(root+'/frames.json',JSON.stringify(await Promise.all(global.smokePage.frames().map(async f=>({url:f.url().split('?')[0],body:(await f.locator('body').innerText().catch(()=>'' )).slice(0,4000)}))),null,2));}throw e;}finally{closing=true;if(browser)await browser.close();for(const s of tunnels)s.destroy();edge.closeAllConnections();await new Promise(r=>edge.close(r));const cleanup={browserClosed:!browser||!browser.isConnected(),edgeClosed:!edge.listening,ownedTunnelsDestroyed:[...tunnels].every(s=>s.destroyed)};fs.writeFileSync(root+'/browser-cleanup.json',JSON.stringify(cleanup));assert(Object.values(cleanup).every(Boolean),'owned browser/edge cleanup');if(diagnostics){fs.writeFileSync(root+'/diagnostics.json',JSON.stringify({http:diagnostics.http,console:diagnostics.console,network:diagnostics.network},null,2));diagnostics.check();}}
})().catch(e=>{console.error(e.stack);process.exitCode=1});
