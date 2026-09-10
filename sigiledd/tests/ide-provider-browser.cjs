const fs=require('node:fs'),https=require('node:https'),http=require('node:http'),net=require('node:net'),cp=require('node:child_process'),path=require('node:path'),assert=require('node:assert/strict');
const root=process.env.GATEWAY_SMOKE_ROOT,upstream=new URL(process.env.GATEWAY_SMOKE_UPSTREAM),url=new URL(process.env.GATEWAY_SMOKE_URL);
const {chromium}=require('/workspace/target/gateway-browser/node_modules/playwright');
(async()=>{
 cp.execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',root+'/edge.key','-out',root+'/edge.crt','-days','1','-subj','/CN=*.ide.example.test'],{stdio:'ignore'});
 const tunnels=new Set();
 const edge=https.createServer({key:fs.readFileSync(root+'/edge.key'),cert:fs.readFileSync(root+'/edge.crt')},(req,res)=>{const p=http.request({host:'127.0.0.1',port:upstream.port,path:req.url,method:req.method,headers:req.headers},r=>{res.writeHead(r.statusCode,r.headers);r.pipe(res)});p.on('error',()=>{res.writeHead(502);res.end()});req.pipe(p);});
 edge.on('upgrade',(req,socket,head)=>{tunnels.add(socket);socket.on('close',()=>tunnels.delete(socket));const p=net.connect(Number(upstream.port),'127.0.0.1',()=>{p.write(`${req.method} ${req.url} HTTP/1.1\r\n`+Object.entries(req.headers).map(([k,v])=>`${k}: ${v}\r\n`).join('')+'\r\n');if(head.length)p.write(head);p.pipe(socket);socket.pipe(p)});p.on('error',()=>socket.destroy());socket.on('error',()=>p.destroy());socket.on('close',()=>p.destroy());});
 await new Promise(r=>edge.listen(0,'127.0.0.1',r));let browser;
 try {
  browser=await chromium.launch({headless:true,args:[`--host-resolver-rules=MAP *.example.test 127.0.0.1:${edge.address().port}`,'--no-proxy-server'],env:{...process.env,XDG_CONFIG_HOME:root+'/browser-config',XDG_CACHE_HOME:root+'/browser-cache'}});
  const context=await browser.newContext({ignoreHTTPSErrors:true,viewport:{width:1440,height:1000}});
  const page=await context.newPage();global.smokePage=page;let workers=0,resources=0,wsFrames=0,activities=0;const failures=[],sockets=[];
  page.on('request',r=>{if(r.url().endsWith('/_sigil/activity'))activities++;});
  page.on('worker',()=>workers++);page.on('websocket',ws=>{sockets.push(ws.url());ws.on('framereceived',()=>wsFrames++);});
  page.on('response',r=>{if(r.url().includes('/static/')&&r.ok())resources++;if(r.status()>=400){failures.push({url:r.url().split('?')[0],status:r.status()});fs.appendFileSync(root+'/network.log',JSON.stringify({url:r.url().split('?')[0],status:r.status()})+'\n');}});
  page.on('console',m=>{if(m.type()==='error')fs.appendFileSync(root+'/browser-console.log',m.text().slice(0,1000)+'\n');});
  await page.goto(url.toString());await page.waitForURL('**/_sigil/workbench',{timeout:15000});
  const frame=page.frameLocator('#editor');await frame.locator('.monaco-workbench').waitFor({timeout:45000});
  await frame.getByRole('tab',{name:/navigation.txt/}).first().waitFor({timeout:30000});
  await frame.getByText('Ln 2, Col 3',{exact:true}).waitFor({timeout:15000});
  assert.equal(await page.evaluate(()=>window.isSecureContext),true);
  assert(workers>0,'actual provider workers must start');assert(resources>3,'actual provider static resources must load');assert(wsFrames>0,'actual provider WebSocket messages must arrive');
  assert(sockets.every(s=>s.startsWith('wss://'+url.host+'/')),'no loopback or insecure WebSocket authority');
  assert.equal(activities,0,'background provider traffic cannot claim human login activity');
  await frame.locator('.monaco-editor textarea').first().focus();await page.keyboard.press('ArrowRight');
  await page.waitForResponse(r=>r.url().endsWith('/_sigil/activity')&&r.status()===204,{timeout:3000}).catch(()=>{assert(activities>0,'trusted editor event must renew login idle');});
  await page.screenshot({path:root+'/provider-workbench.png'});
  const resource='/vscode-remote-resource?path='+encodeURIComponent(root+'/project.html');
  const result=await page.evaluate(async resource=>{const r=await fetch(resource);return {status:r.status,disposition:r.headers.get('content-disposition'),csp:r.headers.get('content-security-policy')};},resource);
  fs.writeFileSync(root+'/resource-probe.json',JSON.stringify(result));assert.equal(result.status,200);assert.equal(result.disposition,'attachment');assert(result.csp.includes('sandbox'));
  const attack=await context.newPage();await attack.goto(url.origin+resource);assert.equal(await attack.evaluate(()=>window.compromised),undefined);assert.match(await attack.locator('body').innerText(),/project_resource_navigation_denied/);
  const proxy=await page.evaluate(async()=>{const r=await fetch('/proxy/3000/');return r.status;});assert.equal(proxy,409);
  await page.evaluate(()=>{window.open=()=>null;});await page.getByRole('button',{name:'Open preview',exact:true}).click();
  const link=page.getByRole('link',{name:'Continue to preview'});await link.waitFor();const previewUrl=await link.getAttribute('href');assert(new URL(previewUrl).host.endsWith('.preview.example.test'));
  const previewPage=await context.newPage();await previewPage.goto(previewUrl);await previewPage.getByRole('heading',{name:'Isolated project preview'}).waitFor();
  const boundary=await previewPage.evaluate(async ide=>{
    const own=await fetch('/_sigil/session');const activity=await fetch('/_sigil/activity',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});
    let cross;try{await fetch(ide+'/_sigil/session',{credentials:'include'});cross='allowed';}catch{cross='blocked';}
    const ideWs=await new Promise(resolve=>{const socket=new WebSocket(ide.replace('https:','wss:')+'/');socket.onopen=()=>{resolve('allowed');socket.close();};socket.onerror=()=>resolve('blocked');});
    return {own:own.status,activity:activity.status,cross,ideWs,cookie:document.cookie};
  },url.origin);assert.deepEqual(boundary,{own:403,activity:403,cross:'blocked',ideWs:'blocked',cookie:''});
  await previewPage.screenshot({path:root+'/isolated-preview.png'});
  fs.writeFileSync(root+'/browser-evidence.json' ,JSON.stringify({browser:browser.version(),boundary,activities,workers,resources,wsFrames,sockets:sockets.map(u=>u.split('?')[0]),failures,resource:result,secureContext:true,file:'navigation.txt',line:2,column:3},null,2));
  console.log('PASS pinned provider frame, resources, secure WSS messages, file line/column, resource sandbox and proxy denial');
 }catch(e){if(global.smokePage){await global.smokePage.screenshot({path:root+'/provider-failure.png'}).catch(()=>{});fs.writeFileSync(root+'/frames.json',JSON.stringify(await Promise.all(global.smokePage.frames().map(async f=>({url:f.url().split('?')[0],body:(await f.locator('body').innerText().catch(()=>'' )).slice(0,4000)}))),null,2));}throw e;}finally{if(browser)await browser.close();for(const s of tunnels)s.destroy();edge.closeAllConnections();await new Promise(r=>edge.close(r));}
})().catch(e=>{console.error(e.stack);process.exitCode=1});
