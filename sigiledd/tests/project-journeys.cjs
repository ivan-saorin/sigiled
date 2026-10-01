// Actual browser assets against controlled records; backend custody is tested in browser/tests.rs.
const http=require('node:http'),fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict');
const {createRequire}=require('node:module');
const {chromium}=process.env.SIGIL_PLAYWRIGHT_PACKAGE?createRequire(process.env.SIGIL_PLAYWRIGHT_PACKAGE)('playwright'):require('playwright');
const root=process.env.SIGIL_ASSETS_DIR||path.resolve(__dirname,'../src/browser/assets');
const output=process.env.SIGIL_SCREENSHOTS||path.resolve('target/journey-screenshots');fs.mkdirSync(output,{recursive:true});
const now=Math.floor(Date.now()/1000),id='f0000000-0000-4000-8000-000000000001';
const item={id,project:'atlas',title:'Choose the evidence to publish',creator:'sigiled-codex',revision:1,created_at:now-600,updated_at:now-600,source_link:'https://example.test/review',request:{question:'Use the reviewed report or wait for the new study?',context:'The reviewed report is ready. The new study has not been checked.',answer:null}};
const project={name:'atlas',display_name:'Atlas',description:'A shared research project, with external agents and a published service.',observed_at:now,repository_revision:'a'.repeat(40),setup:{state:'ready',observed_at:now-20,stale:false},capabilities:{workspace:{state:'ready'}},sessions:{items:[],total:1},attention:{items:[{project:'atlas',reason:'latest_build_failed',severity:'failure'}],total:1},activity:{items:[{at_epoch:now,event:{kind:'session_closed',session_id:'reviewed-report'}},{at_epoch:now-300,event:{kind:'app_build_failed'}}],total:2,next_offset:null},app:{state:'deployed',deployed_revision:'b'.repeat(40),latest_build:{ok:false,revision:'a'.repeat(40),finished_at:now-300},runtime:{state:'unknown',observed_at:null,reason:'not_probed'},revision_drift:true}};
const fixture={auth:true,actor:'human:alex',mutations:0,fail:false,loseAnswer:false,delayAnswer:0,delayList:0,holdDetail:null,items:[item]};
const server=http.createServer(async(req,res)=>{try{
 const u=new URL(req.url,'http://fixture');let body='';for await(const chunk of req)body+=chunk;
 const reply=(v,status=200)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(v));};
 if(u.pathname==='/browser/session')return reply(fixture.auth?{actor:{driver:fixture.actor},identity:{display_name:'Alex'},csrf_token:'fixture',features:{agent_requests:true}}:{error:'login_required'},fixture.auth?200:401);
 if(u.pathname.startsWith('/browser/api/')){
  if(!fixture.auth)return reply({error:'login_required'},401);
  if(fixture.fail)return reply({error:'service_unavailable'},503);
  if(u.pathname==='/browser/api/projects/atlas')return reply(project);
  if(u.pathname==='/browser/api/projects/atlas/requests'){
   const snap=structuredClone(fixture.items);if(fixture.delayList)await new Promise(r=>setTimeout(r,fixture.delayList));
   return reply({items:snap,total:snap.length,waiting:snap.filter(i=>!i.request.answer).length,next_offset:null});
  }
  if(u.pathname.startsWith('/browser/api/projects/atlas/requests/')){
   const found=fixture.items.find(i=>i.id===u.pathname.split('/')[6]);if(!found)return reply({error:'request_not_found'},404);
   if(req.method==='GET'){
    const snap=structuredClone(found),held=fixture.holdDetail;fixture.holdDetail=null;
    if(held){held.started();await held.wait;}
    return reply(snap);
   }
   fixture.mutations++;assert.equal(req.headers['x-sigil-csrf'],'fixture');const b=JSON.parse(body);
   if(found.request.answer)return reply(found.request.answer.text===b.text?found:{error:'request_already_answered'},found.request.answer.text===b.text?200:409);
   if(found.revision!==b.expected_revision)return reply({error:'request_revision_conflict'},409);
   if(fixture.delayAnswer)await new Promise(r=>setTimeout(r,fixture.delayAnswer));
   found.revision++;found.request.answer={text:b.text,actor:fixture.actor,at:now};
   if(fixture.loseAnswer){fixture.loseAnswer=false;return reply({error:'work_item_save_uncertain'},503);}
   return reply(found);
  }
  return reply({items:[],total:0,next_offset:null});
 }
 const asset=u.pathname.startsWith('/browser/assets/')?u.pathname.split('/').pop():'index.html';
 res.writeHead(200,{'content-type':asset.endsWith('.js')?'text/javascript':asset.endsWith('.css')?'text/css':'text/html'});res.end(fs.readFileSync(path.join(root,asset)));
}catch(e){res.writeHead(500);res.end(String(e));}});
async function run(){
 await new Promise(r=>server.listen(0,'127.0.0.1',r));const base=`http://127.0.0.1:${server.address().port}`;
 if(process.env.SIGIL_FIXTURE_ONLY){console.log(base+'/ui/projects/atlas/overview');return;}
 const browser=await chromium.launch({headless:true,...(process.env.SIGIL_BROWSER_EXECUTABLE?{executablePath:process.env.SIGIL_BROWSER_EXECUTABLE}:{})});
 try{
  const page=await browser.newPage({viewport:{width:1440,height:1050}});page.setDefaultTimeout(8000);const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(base+'/ui/projects/atlas/overview');await page.getByRole('heading',{name:'Pick up where the project stands'}).waitFor();
  await page.getByRole('button',{name:'Mark caught up'}).click();await page.getByText('Review point saved in this browser.',{exact:false}).waitFor();
  await page.reload();await page.getByRole('heading',{name:'Since you last caught up'}).waitFor();await page.getByText('reviewed-report',{exact:true}).waitFor();
  assert.equal(fixture.mutations,0,'catch-up marker must not mutate project records');
  await page.screenshot({path:path.join(output,'project-return.png'),fullPage:true});
  await page.getByRole('link',{name:'Review agent questions'}).click();await page.getByLabel('Your answer',{exact:true}).fill('Use the reviewed report.');
  await page.getByRole('button',{name:'Refresh',exact:true}).click();await page.waitForResponse(r=>r.url().includes('/requests?'));assert.equal(await page.getByLabel('Your answer',{exact:true}).inputValue(),'Use the reviewed report.');
  await page.getByRole('link',{name:'Overview',exact:true}).last().click();await page.goBack();await page.getByLabel('Your answer',{exact:true}).waitFor();assert.equal(await page.getByLabel('Your answer',{exact:true}).inputValue(),'Use the reviewed report.');
  await page.screenshot({path:path.join(output,'agent-question.png'),fullPage:true});
  fixture.loseAnswer=true;await page.getByRole('button',{name:'Record answer',exact:true}).click();await page.getByText('The save is not confirmed.',{exact:false}).waitFor();assert.equal(await page.getByLabel('Your answer',{exact:true}).isDisabled(),true);
  await page.getByRole('button',{name:'Check recorded answer',exact:true}).click();await page.getByText('Answer recorded',{exact:true}).waitFor();assert.equal(fixture.mutations,1);assert.equal(item.request.answer.text,'Use the reviewed report.');
  await page.getByText('Its continuation has not been observed.',{exact:false}).waitFor();
  // A delayed pre-answer read must not erase a newer durable receipt.
  const race={...structuredClone(item),id:'f0000000-0000-4000-8000-000000000003',title:'Delayed read',revision:1,request:{question:'Publish the reviewed notes?',context:'',answer:null}};fixture.items=[race];
  await page.goto(base+'/ui/projects/atlas/requests');await page.getByLabel('Your answer',{exact:true}).fill('Yes, the reviewed notes.');
  let releaseDetail,detailStarted;const detailReady=new Promise(r=>detailStarted=r);fixture.holdDetail={started:detailStarted,wait:new Promise(r=>releaseDetail=r)};
  await page.getByRole('button',{name:'Check recorded answer',exact:true}).click();await detailReady;
  await page.getByRole('button',{name:'Record answer',exact:true}).click();await page.getByText('Answer recorded',{exact:true}).waitFor();
  const delayedResponse=page.waitForResponse(r=>r.url().endsWith('/requests/'+race.id));releaseDetail();await delayedResponse;await page.waitForTimeout(50);
  assert.equal(await page.getByText('Answer recorded',{exact:true}).count(),1);assert.equal(await page.getByRole('button',{name:'Record answer',exact:true}).count(),0);
  const row=page.getByRole('button',{name:'Answered · Delayed read',exact:true});await row.waitFor();await row.focus();
  await page.evaluate(()=>document.querySelector('#project-data .requests-journey').load());assert.equal(await row.evaluate(e=>e===document.activeElement),true,'polling retains question-list keyboard focus');
  await page.getByRole('link',{name:'Deployment',exact:true}).click();await page.getByRole('heading',{name:'The latest build failed'}).waitFor();
  await page.getByText('a'.repeat(40),{exact:true}).first().waitFor();await page.getByText('b'.repeat(40),{exact:true}).first().waitFor();
  await page.getByText('Unknown',{exact:true}).waitFor();assert.equal(await page.getByRole('button',{name:/deploy|restart|roll.?back/i}).count(),0,'observation must not invent deployment controls');
  await page.getByText('Agent handoff',{exact:true}).click();await page.getByText('Inspect the authoritative build evidence',{exact:false}).waitFor();
  await page.screenshot({path:path.join(output,'deployment-failure.png'),fullPage:true});
  fixture.fail=true;await page.getByRole('button',{name:'Refresh',exact:true}).click();await page.getByText(/Showing stale data/).waitFor();await page.getByRole('heading',{name:'The latest build failed'}).waitFor();fixture.fail=false;
  await page.setViewportSize({width:390,height:844});await page.screenshot({path:path.join(output,'deployment-mobile.png'),fullPage:true});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
  // A second question proves sign-in never replays and identity changes cannot borrow an old draft.
  const next={...structuredClone(item),id:'f0000000-0000-4000-8000-000000000002',title:'Second question',revision:1,request:{question:'<img src=x onerror=alert(1)>',context:'Test hostile content as text.',answer:null}};fixture.items=[next];
  await page.goto(base+'/ui/projects/atlas/requests');await page.getByLabel('Your answer',{exact:true}).fill('Retained answer');assert.equal(await page.locator('.request-detail img').count(),0);
  fixture.auth=false;await page.getByRole('button',{name:'Record answer',exact:true}).click();await page.getByRole('link',{name:'Sign in in another tab'}).waitFor();const before=fixture.mutations;fixture.auth=true;
  await page.getByRole('button',{name:'Check sign-in'}).click();await page.getByLabel('Your answer',{exact:true}).waitFor();assert.equal(fixture.mutations,before);assert.equal(await page.getByLabel('Your answer',{exact:true}).inputValue(),'Retained answer');
  fixture.actor='human:other';await page.getByRole('button',{name:'Record answer',exact:true}).click();await page.getByText('The signed-in person changed.',{exact:false}).waitFor();assert.equal(fixture.mutations,before);
  await page.getByRole('button',{name:'Refresh',exact:true}).click();await page.waitForResponse(r=>r.url().includes('/requests?'));await page.waitForTimeout(50);assert.equal(await page.getByLabel('Your answer',{exact:true}).inputValue(),'','drafts are actor scoped');
  assert.deepEqual(errors,[]);console.log('PASS: three journeys, timestamp boundary, review persistence, answer draft retention, uncertain-save recovery, stale-read ordering, keyboard focus, no auto-resume/grants/deploy, stale evidence, hostile text, mobile overflow, auth expiry and actor isolation');
 }finally{await browser.close();server.close();}
}
run().catch(e=>{console.error(e);process.exitCode=1;server.close();});
