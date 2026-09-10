/* Sigil browser shell. Data enters the DOM only through textContent/text nodes. */
'use strict';
const researchDrafts = new Map();
let researchSelection = 0;
const $ = id => document.getElementById(id);
const el = (tag,attrs = {
}
,...children) => {
  const n = document.createElement(tag);
  for(const[k,v]of Object.entries(attrs)) {
    if(k === 'class')n.className = v;
    else if(k.startsWith('on'))n.addEventListener(k.slice(2),v);
    else if(v !== undefined && v !== null)n.setAttribute(k,String(v));
  }
  for(const child of children.flat()) {
    if(child !== undefined && child !== null)n.append(child instanceof Node?child:document.createTextNode(String(child)));
  }
  return n;
}
;
const label = s => String(s ?? 'Unknown').replaceAll('_',' ').replace(/^./,c => c.toUpperCase());
const date = t => t?new Date(t*1000).toLocaleString():'Not observed';
const link = (text,href,attrs = {
}
) => el('a', {
  href,...attrs
}
,text);
const button = (text,fn,cls = '') => el('button', {
  type:'button',class:cls,onclick:fn
}
,text);
const empty = text => el('div', {
  class:'empty'
}
,text);
const badge = value => el('span', {
  class:'status '+(['failed','failure','blocked','error'].includes(value)?'failure':['pending','warning','partial','not_deployed','stale'].includes(value)?'warning':['ready','done','succeeded'].includes(value)?'good':'')
}
,label(value));
const heading = (name,subtitle,action) => {
  $('heading').replaceChildren(el('div', {
  }
  ,el('h1', {
  }
  ,name),subtitle?el('p', {
    class:'muted'
  }
  ,subtitle):null),...(action?[action]:[]));
  document.title = `${name} · Sigil`;
}
;
let session = null,route = null,generation = 0,controller = null,current = null,lastObserved = null;
const drafts = new Map();
const itemFields = ['title','description','state','owner','source_link'];
let itemSelection = 0, jobSelection = 0, editorIdentity = 0, activeDraft = null, pendingItem = null;
let projects = [],attention = [];
class ApiError extends Error {
  constructor(status,body) {
    super(body?.error || 'request_failed');
    this.status = status;
    this.body = body;
  }
}
async function request(path,options = {
}
) {
  const signal = options.signal;
  const headers = {
    'Accept':'application/json',...options.headers
  }
  ;
  if(options.body) {
    headers['Content-Type'] = 'application/json';
    headers['X-Sigil-CSRF'] = headers['X-Sigil-CSRF'] || session?.csrf_token || '';
  }
  if(options.method === 'POST' && !options.body)headers['X-Sigil-CSRF'] = headers['X-Sigil-CSRF'] || session?.csrf_token || '';
  let r;
  try {
    r = await fetch(path, {
      credentials:'same-origin',...options,headers,signal
    }
    );
  }
  catch(e) {
    if(e.name === 'AbortError')throw e;
    throw new ApiError(0, {
      error:'connection_unavailable'
    }
    );
  }
  const body = r.status === 204?null:await r.json().catch(() => ({
    error:'invalid_response'
  }
  ));
  if(!r.ok) {
    if(r.status === 401)showAuth();
    throw new ApiError(r.status,body);
  }
  return body;
}
function showAuth() {
  window.SigilMemory?.authExpired();
  session = null;
  $('auth').hidden = false;
  $('auth').replaceChildren(el('strong', {
  }
  ,'Sign in to continue. '),document.createTextNode('Your unsaved work stays in this page. '),link('Sign in in another tab','/browser/login?return_to=/ui/overview', {
    target:'_blank',rel:'noopener'
  }
  ),button('Check sign-in',async() => {
    try {
      await checkSession();
      notice((route?.view==="research"||route?.tab==="research")?"Signed in. Review your research before starting or retrying.":route?.view==="models"?"Signed in. Model observations can be refreshed.":"Signed in. Review your draft and choose Save when ready.","good");
      await refresh();
    }
    catch {
      showAuth();
    }
  }
  ));
  $('operator').textContent = 'Sign-in required';
  $('signout').hidden = true;
}
async function checkSession() {
  session = await request('/browser/session');
  $('auth').hidden = true;
  $('operator').textContent = session.identity.display_name || 'Signed-in operator';
  $('signout').hidden = false;
  for(const project of workspaceLaunches.keys())syncWorkspaceLaunch(project);
  return session;
}
function notice(text,type = '') {
  const n = $('feedback');
  n.replaceChildren(text?el('div', {
    class:`notice ${type}`
  }
  ,text):document.createTextNode(''));
}
function parseRoute() {
  const u = new URL(location.href),p = u.pathname.split('/').filter(Boolean);
  if(p.length === 0)return {
    view:document.body.dataset.entry==='memory'?'memory':'overview',query:u.searchParams
  }
  ;
  return {
    view:p[1] || 'overview',project:p[2] && p[2] !== 'new'?p[2]:null,newProject:p[1] === 'projects' && p[2] === 'new',tab:p[3] || 'overview',query:u.searchParams
  }
  ;
}
function navigate(path) {
  if(location.pathname+location.search === path)return;
  history.pushState({
  }
  ,'',path);
  renderRoute();
}
function updateQuery(values) {
  const u = new URL(location.href);
  for(const[k,v]of Object.entries(values))v?u.searchParams.set(k,v):u.searchParams.delete(k);
  history.replaceState({
  }
  ,'',u.pathname+u.search);
  route.query = u.searchParams;
}
document.addEventListener('click',e => {
  const a = e.target.closest('a');
  if(a && !e.defaultPrevented && e.button === 0 && !e.ctrlKey && !e.metaKey && !e.shiftKey && !a.target) {
    const u = new URL(a.href);
    // Let native fragment navigation move keyboard focus to the skip target.
    if(u.hash && u.origin === location.origin && u.pathname === location.pathname && u.search === location.search)return;
    if(u.origin === location.origin && (u.pathname === '/' || u.pathname.startsWith('/ui/'))) {
      e.preventDefault();
      navigate(u.pathname+u.search);
    }
  }
}
);
window.addEventListener('popstate',renderRoute);
function revealSelectedTab() {
  const nav = document.querySelector('.tabs'),active = nav?.querySelector('[aria-current]');
  if(!active)return;
  const n = nav.getBoundingClientRect(),a = active.getBoundingClientRect();
  if(a.left<n.left)nav.scrollLeft -= n.left-a.left;
  else if(a.right>n.right)nav.scrollLeft += a.right-n.right;
}
window.addEventListener('resize',revealSelectedTab);
$('signout').onclick = async() => {
  try {
    await request('/browser/logout', {
      method:'POST'
    }
    );
    showAuth();
    notice('Signed out. Unsaved drafts remain in this tab.');
  }
  catch(e) {
    notice(errorText(e),'error');
  }
}
;
$('refresh').onclick = () => refresh();
function renderRoute() {
  generation++;
  itemSelection++;
  jobSelection++;
  editorIdentity++;
  activeDraft = null;
  pendingItem = null;
  controller?.abort();
  current = null;
  lastObserved = null;
  route = parseRoute();
  notice('');
  $('content').replaceChildren();
  for(const a of document.querySelectorAll('.rail nav a')) {
    a.removeAttribute('aria-current');
    if(a.pathname === `/ui/${route.view}`)a.setAttribute('aria-current','page');
  }
  if(route.newProject) {
    renderNewProject();
    return;
  }
  if(route.project) {
    renderProjectShell();
  }
  else if(['overview','projects'].includes(route.view)) {
    renderOverviewShell();
  }
  else if(route.view === "memory") { heading("Memory","Browse records, preserve provenance and curate corrections"); $("content").append(window.SigilMemory.shell()); return; }
  else if(route.view === "research") { heading("Research","Inspect runs and their actual stage outcomes"); $("content").append(researchShell(null)); return; }
  else if(route.view === "models") { heading("Models","Models and recent observed requests");const root=el("section",{id:"models-root"},el("p",{role:"status"}),el("div",{class:"models-data"}));root.intent=0;$("content").append(root);modelsView(root);return; }
  else {
    heading(label(route.view),'Shared workspace');
    $('content').append(empty(route.view === 'memory'?'Memory curation is pending its reviewed browser adapter. Existing project memory declarations appear on each project.':route.view === 'research'?'Research orchestration is pending its reviewed service adapter. No research has been started from this dashboard.':'Model inventory and inference are pending the reviewed Models adapter. No model calls have been made.'));
    $('connection').textContent = 'Integration pending';
    return;
  }
  if(session)refresh();
  else $('connection').textContent = 'Sign in to load project observations';
}
function renderOverviewShell() {
  if(route.view==="overview")$("content").append(el("section",{},el("h2",{},"Research attention"),el("div",{id:"research-attention"},"Loading research observations…")));
  heading(route.view === 'overview'?'Overview':'Projects','Your projects and ongoing work',link('New project','/ui/projects/new', {
    class:'button primary'
  }
  ));
  if(route.view === 'overview')$('content').append(el('section', {
    class:'section'
  }
  ,el('h2', {
  }
  ,'Needs attention'),el('div', {
    id:'attention'
  }
  ,empty('Waiting for project observations…'))),el('section', {
    class:'section'
  }
  ,el('h2', {
  }
  ,'Active work'),el('div', {
    id:'active'
  }
  ,empty('Waiting for project observations…'))));
  const search = el('input', {
    id:'search',type:'search',placeholder:'Find a project',value:route.query.get('q') || '',oninput:e => {
      updateQuery({
        q:e.target.value,page:null
      }
      );
      renderProjects();
    }
  }
  );
  const filter = el('select', {
    id:'filter','aria-label':'Show',onchange:e => {
      updateQuery({
        filter:e.target.value,page:null
      }
      );
      renderProjects();
    }
  }
  ,...['all','working','attention'].map(v => el('option', {
    value:v
  }
  ,v === 'all'?'All projects':v === 'working'?'Working':'Needs attention')));
  filter.value = route.query.get('filter') || 'all';
  const sort = el('select', {
    id:'sort','aria-label':'Sort by',onchange:e => {
      updateQuery({
        sort:e.target.value,page:null
      }
      );
      renderProjects();
    }
  }
  ,el('option', {
    value:'name'
  }
  ,'Name'),el('option', {
    value:'activity'
  }
  ,'Latest activity'));
  sort.value = route.query.get('sort') || 'name';
  $('content').append(el('section', {
    class:'section'
  }
  ,el('h2', {
  }
  ,'Projects'),el('div', {
    class:'toolbar'
  }
  ,el('label', {
    class:'grow'
  }
  ,'Search projects',search),el('label', {
  }
  ,'Show',filter),el('label', {
  }
  ,'Sort by',sort)),el('div', {
    id:'projects'
  }
  ,empty('Loading registered projects…'))));
}
// Polling may refresh a list while its link/button has keyboard focus.
function withFocus(root,render) {
  const active=root?.contains(document.activeElement)?document.activeElement:null;
  const key=active?.dataset.focusKey;
  const id=active?.id;
  const href=active?.getAttribute('href');
  const text=active?.textContent;
  render();
  if (!active) return;
  const replacement=active.isConnected?active:Array.from(root.querySelectorAll('button,a,input,select,textarea')).find(n=>
    key?n.dataset.focusKey===key:id?n.id===id:href?n.getAttribute('href')===href:n.tagName===active.tagName&&n.textContent===text);
  replacement?.focus({preventScroll:true});
}
function replaceLive(target,...children) {withFocus(target,()=>target.replaceChildren(...children));}
function renderProjects() {withFocus($('projects'),renderProjectsContent);}
function renderProjectsContent() {
  if(!$('projects'))return;
  const q = (route.query.get('q') || '').toLowerCase(),filter = route.query.get('filter') || 'all';
  let rows = projects.filter(p => `${p.name} ${p.display_name||''} ${p.description||''}`.toLowerCase().includes(q) && (filter === 'all' || filter === 'working' && p.session_count>0 || filter === 'attention' && (p.setup?.state !== 'ready' || p.setup?.stale || attention.some(a => a.project === p.name || a.source?.startsWith(`project:${p.name}/`)))));
  rows.sort((a,b) => route.query.get('sort') === 'activity'?(b.latest_activity_at || 0)-(a.latest_activity_at || 0):a.name.localeCompare(b.name));
  const page = Math.max(0,Number(route.query.get('page')) || 0),start = page*20;
  const target = $('projects');
  target.replaceChildren();
  if(!rows.length) {
    target.append(empty(projects.length?'No projects match these filters.':'No registered projects yet. Create a project to start.'));
    return;
  }
  const table = makeTable(['Project','Current work','Project information','App','Last activity'],rows.slice(start,start+20).map(p => [el('div', {
  }
  ,link(p.display_name || p.name,projectUrl(p.name)),el('p', {
  }
  ,p.description || p.name)),p.session_count?`${p.session_count} workspace${p.session_count===1?'':'s'}`:'No workspaces',el('div', {
  }
  ,badge(p.setup?.stale?'last_known':p.setup?.state === 'ready'?'current':p.setup?.state || 'unknown'),el('div', {
    class:'metadata'
  }
  ,p.setup?.error?.message || date(p.setup?.observed_at))),badge(p.app?.state || 'unknown'),el('span', {
    class:'metadata'
  }
  ,date(p.latest_activity_at))]));
  target.append(table,pager(start,rows.length,start+20<rows.length?start+20:null,n => {
    updateQuery({
      page:String(n/20)
    }
    );
    renderProjects();
  }
  ,20));
}
function projectUrl(name,tab = 'overview') {
  return`/ui/projects/${encodeURIComponent(name)}/${tab}`;
}
function makeTable(head,rows) {
  return el('div', {
    class:'table-wrap'
  }
  ,el('table', {
  }
  ,el('thead', {
  }
  ,el('tr', {
  }
  ,head.map(h => el('th', {
    scope:'col'
  }
  ,h)))),el('tbody', {
  }
  ,rows.map(r => el('tr', {
  }
  ,r.map(c => el('td', {
  }
  ,c)))))));
}
function pager(offset,total,next,onPage,limit = 20) {
  return el('div', {
    class:'pager'
  }
  ,el('span', {
    class:'metadata'
  }
  ,`${total?offset+1:0}–${Math.min(offset+limit,total)} of ${total}`),el('button', {
    type:'button',disabled:offset === 0?true:null,onclick:() => onPage(Math.max(0,offset-limit))
  }
  ,'Previous'),el('button', {
    type:'button',disabled:next==null?true:null,onclick:() => onPage(next)
  }
  ,'Next'));
}
function attentionList(items) {
  return items.length?el('ul', {
    class:'attention'
  }
  ,items.map(a => {
    const p = a.project || a.source?.match(/^project:([^/]+)/)?.[1];
    return el('li', {
    }
    ,el('span', {
      class:`dot ${a.severity||''}`
    }
    ),el('div', {
    }
    ,el('strong', {
    }
    ,p || 'System'),el('div', {
      class:'metadata'
    }
    ,label(a.reason || a.code || a.kind))),p?link('Review',projectUrl(p,'pending')):null);
  }
  )):empty('No system attention in the latest observation.');
}
function renderProjectShell() {
  heading(route.project,'Loading project observations…');
  const tabs = ['overview','workspace','research','jobs','app','memory','activity','pending'];
  $('content').append(el('nav', {
    class:'tabs','aria-label':'Project'
  }
  ,tabs.map(t => link(label(t),projectUrl(route.project,t), {
    'aria-current':route.tab === t?'page':null
  }
  ))),el('div', {
    id:'project-data'
  }
  ,empty('Loading project…')));
  if(route.tab === 'pending')$('content').append(el('div', {
    class:'columns'
  }
  ,el('section', {
    class:'panel'
  }
  ,el('h2', {
  }
  ,'Work items'),el('p', {
    class:'muted'
  }
  ,'Explicit work you track. Completing an item does not dismiss system attention.'),el('div', {
    id:'work-items'
  }
  ,empty('Loading work items…'))),el('section', {
    id:'editor',class:'panel'
  }
  )));
  if(route.tab === 'pending')renderItemEditor();
  requestAnimationFrame(revealSelectedTab);
}
function facts(entries) {
  return el('dl', {
    class:'facts'
  }
  ,entries.flatMap(([k,v]) => [el('dt', {
  }
  ,k),el('dd', {
  }
  ,v ?? 'Unknown')]));
}
function diagnostics(data) {
  return el('details', {
  }
  ,el('summary', {
  }
  ,'Technical details'),el('pre', {
  }
  ,JSON.stringify(data,null,2)));
}
function capability(name,p) {
  const c = p.capabilities?.[name];
  return el('div', {
  }
  ,badge(c?.state || 'pending'),el('p', {
  }
  ,label(c?.reason || `${name}_adapter_not_configured`)));
}
const workspaceLaunches = new Map();
function launchDraft(project) {
  if(!workspaceLaunches.has(project)) {
    const storageKey=`sigil-ide-operation:${project}`;
    let key;try{key=sessionStorage.getItem(storageKey);}catch{}
    if(!key){key=crypto.randomUUID();try{sessionStorage.setItem(storageKey,key);}catch{}}
    workspaceLaunches.set(project,{key,storageKey,pending:false});
  }
  return workspaceLaunches.get(project);
}
function workspaceLaunch(project) {
  const node=el('div',{'data-workspace-launch':project});
  fillWorkspaceLaunch(node,project);return node;
}
function fillWorkspaceLaunch(node,project) {
  const draft=launchDraft(project),enabled=session?.features?.workspace_actions===true;
  const open=button(draft.pending?'Opening workspace…':'Open IDE',()=>openIDE(project));open.disabled=!enabled||draft.pending;
  node.replaceChildren(el('div',{class:'actions'},open));
  if(draft.result?.launch_url)node.append(el('a',{href:draft.result.launch_url,target:'_blank',rel:'noopener noreferrer'},'Continue to IDE'),el('p',{class:'metadata'},'Your workspace is ready. This page and its unsaved drafts stay open.'));
  else node.append(el('p',{class:'metadata',role:'status'},draft.error || (enabled?'Open your own workspace. Saved files can be checkpointed and pushed.':'IDE access needs deployment setup.')));
  if(draft.absent)node.append(button('Start a new workspace',()=>{draft.key=crypto.randomUUID();try{sessionStorage.setItem(draft.storageKey,draft.key);}catch{}draft.absent=false;return openIDE(project);}));
}
function syncWorkspaceLaunch(project) {for(const node of document.querySelectorAll('[data-workspace-launch]'))if(node.dataset.workspaceLaunch===project)fillWorkspaceLaunch(node,project);}
async function openIDE(project,target) {
  const draft=launchDraft(project);if(draft.pending)return;
  const submittedTarget=target?structuredClone(target):undefined;
  let popup;try{popup=window.open('about:blank','_blank');if(popup){popup.opener=null;popup.document.title='Opening workspace';popup.document.body.textContent='Sigil is preparing your workspace. Your dashboard stays open.';}}catch{}
  draft.pending=true;draft.error=null;draft.result=null;syncWorkspaceLaunch(project);
  try {
    await checkSession();
    const result=await request(`/browser/api/projects/${encodeURIComponent(project)}/ide`,{method:'POST',body:JSON.stringify({idempotency_key:draft.key,...(submittedTarget?{target:submittedTarget}:{})})});
    draft.result=result;
    if(popup&&!popup.closed)popup.location.replace(result.launch_url);
    return result;
  }catch(e){if(popup&&!popup.closed)popup.close();if(e.status===401)showAuth();draft.error=errorText(e);draft.absent=e.body?.error==='allocation_absent_or_closed';return {state:'failed',error:draft.error};}
  finally{draft.pending=false;syncWorkspaceLaunch(project);}
}
function workspaceControls(record) {
  const node=el('div',{class:'actions'});
  if(!session?.features?.workspace_actions || session.actor?.driver!==record.actor?.driver)return node;
  const status=el('p',{class:'metadata',role:'status'});let busy=false;
  for(const action of ['checkpoint','finish']) {
    const control=button(action==='checkpoint'?'Checkpoint & push':'Finish workspace',async()=>{
      if(busy)return;if(action==='finish'&&!confirm('Save your editor buffers first. Finish stops the editor, pushes saved files and attempts to merge. Failures preserve the workspace. Continue?'))return;
      busy=true;status.textContent='Checking workspace…';
      try{await checkSession();const observed=await request(`/browser/api/sessions/${encodeURIComponent(record.session_id)}/ide`);if(typeof observed.generation!=='string')throw new Error('Workspace generation unavailable');const result=await request(`/browser/api/sessions/${encodeURIComponent(record.session_id)}/ide`,{method:'POST',body:JSON.stringify({action,generation:observed.generation})});status.textContent=action==='finish'?`Workspace finished. Merge: ${result.merge||'recorded'}.`:'Saved files checkpointed and pushed. Unsaved buffers remain in the editor.';if(action==='finish'){const draft=launchDraft(record.project||route.project);try{sessionStorage.removeItem(draft.storageKey);}catch{}workspaceLaunches.delete(record.project||route.project);}}
      catch(e){if(e.status===401)showAuth();status.textContent=errorText(e)+' Your workspace is preserved. Check status and deliberately retry.';}finally{busy=false;}
    });node.append(control);
  }
  node.append(status);return node;
}
function renderProject(p) {
  heading(p.display_name || p.name,p.description || p.name,workspaceLaunch(p.name));
  const target = $('project-data');
  if(!target)return;
  const panel = el('section', {
    class:'panel section'
  }
  );
  switch(route.tab) {
    case'overview':panel.append(el('h2', {
    }
    ,'Project overview'),facts([['Project information',badge(p.setup?.state === 'ready'?'current':p.setup?.state || 'unknown')],['Observed',date(p.setup?.observed_at)],['Repository revision',el('span', {
      class:'mono'
    }
    ,p.repository_revision || 'Not observed')],['Deployed revision',el('span', {
      class:'mono'
    }
    ,p.app?.deployed_revision || 'Not deployed')],['Workspaces',String(p.sessions?.total ?? p.session_count ?? 'Unknown')],['App',badge(p.app?.state || 'unknown')]]),p.setup?.stale?el('p', {
      class:'notice'
    }
    ,'Registry information is stale. Check the last observation and refresh status.'):null,diagnostics({
      setup:p.setup,capabilities:p.capabilities
    }
    ));
    break;
    case'workspace':panel.append(el('h2', {
    }
    ,'Workspaces'),capability('workspace',p),el('p', {
      class:'muted'
    }
    ,'Closing this browser does not close or merge a workspace. Save editor buffers before checkpointing or finishing.'));
    const sessions = p.sessions?.items || [];
    panel.append(sessions.length?makeTable(['Workspace','Operator','Branch','Lifecycle','Readiness'],sessions.map(s => [s.session_id || s.id,el('div', {
    }
    ,s.actor?.display_name || 'Authenticated operator',diagnostics(s.actor || {
    }
    )),el('span', {
      class:'mono'
    }
    ,s.branch),el('div', {
    }
    ,badge(s.state || s.lifecycle),el('div', {
      class:'metadata'
    }
    ,'Saved files and merge outcomes are checked separately.')),el('div', {
    }
    ,badge(s.ready === true?'ready':'pending'),el('p', {
    }
    ,label(s.error || s.readiness?.reason || 'readiness_not_observed')),workspaceControls(s),diagnostics(s))])):empty('No workspace sessions recorded for this project.'));
    break;
    case'app':panel.append(el('h2', {
    }
    ,'App'),facts([['Deployment',badge(p.app?.state || 'unknown')],['Runtime',badge(p.app?.runtime?.state || 'unknown')],['Runtime observed',date(p.app?.runtime?.observed_at)],['Repository revision',el('span', {
      class:'mono'
    }
    ,p.repository_revision || 'Not observed')],['Deployed revision',el('span', {
      class:'mono'
    }
    ,p.app?.deployed_revision || 'Not deployed')],['Latest build',p.app?.latest_build?badge(p.app.latest_build.ok?'succeeded':'failed'):'No build record']]),p.app?.revision_drift === true?el('p', {
      class:'notice'
    }
    ,'Source has changed since the deployed revision.'):null,el('p', {
      class:'muted'
    }
    ,'Deployment and runtime are separate observations.'),diagnostics(p.app));
    break;
    case'jobs':panel.append(el('h2', {
    }
    ,'Jobs'));
    const jobs = p.jobs?.items || [];
    panel.append(jobs.length?makeTable(['Job','Schedule','Latest result','History'],jobs.map(j => [j.name,j.cron,badge(j.state),button('View history',() => loadJob(j.name))])):empty('No job definitions observed on this project.'));
    panel.append(el('div', {
      id:'job-history'
    }
    ));
    break;
    case'activity':panel.append(el('h2', {
    }
    ,'Activity'));
    const activity = p.activity || {
      items:[],total:0
    }
    ;
    panel.append(activity.items.length?makeTable(['Time','Event','Source'],activity.items.map(a => [date(a.at_epoch),label(a.event?.kind),a.event?.job || a.event?.session_id || 'Project'])):empty('No recorded project activity.'),pager(activity.offset || 0,activity.total,activity.next_offset,n => {
      updateQuery({
        offset:n
      }
      );
      refresh();
    }
    ));
    break;
    case'pending':panel.append(el('h2', {
    }
    ,'System attention'),attentionList(p.attention?.items || []));
    break;
    case'memory':panel.append(el('h2', {
    }
    ,'Memory'),capability('memory',p),el('p', {
    }
    ,({indexed:'Memory was indexed at the last confirmation',awaiting_authorization:'Sign in to retry Memory setup',disabled:'Memory enrollment is disabled',ownership_or_revision_conflict:'Memory setup needs a separate namespace',update_required:'Memory needs a service update',unavailable:'Memory is unavailable; retry setup',projection_pending:'Documents accepted; indexing is pending'}[p.memory_enrollment?.state]||'Memory setup pending')),link('Browse Memory',p.memory_enrollment?.confirmed?.verified?'/ui/memory?index='+encodeURIComponent(p.memory_enrollment.confirmed.index):'/ui/memory'),facts([['Sharing',p.memory?.sharing==='mem0'?'Shared with general Memory':'Project only'],['Indexed accepted commit',p.memory_enrollment?.confirmed?.commit||'Not confirmed']]),button('Retry Memory setup',async()=>{await checkSession();await request('/browser/api/projects/'+encodeURIComponent(route.project)+'/memory-enrollment',{method:'POST',body:'{}'});refresh();}));
    if(p.memory_enrollment?.state==='ownership_or_revision_conflict'&&!p.memory_enrollment?.confirmed)panel.append(el('p',{},'An existing namespace belongs to another owner. Preserve it and create a separate project namespace.'),button('Use separate namespace',async()=>{await checkSession();await request('/browser/api/projects/'+encodeURIComponent(route.project)+'/memory-enrollment/namespace',{method:'POST',body:'{}'});refresh();}));
    break;
    case'research':panel.append(target.querySelector("#research-root") || researchShell(route.project));
    break;
    default:panel.append(empty('This project tab is not available.'));
  }
  // Keep a selected history surface across summary polls.
  const oldHistory=target.querySelector('#job-history');
  if(oldHistory&&panel.querySelector('#job-history'))panel.querySelector('#job-history').replaceWith(oldHistory);
  replaceLive(target,panel);
}
async function loadJob(name,offset = 0) {
  const g = generation,selection = ++jobSelection,target = $('job-history');
  try {
    const data = await request(`/browser/api/projects/${encodeURIComponent(route.project)}/jobs/${encodeURIComponent(name)}/runs?offset=${offset}&limit=20`);
    if(g !== generation || selection !== jobSelection || !target.isConnected)return;
    target.replaceChildren(el('h3', {
    }
    ,`${name} history`),data.items.length?makeTable(['Started','Finished','State','Branch'],data.items.map(r => [date(r.started_at),date(r.finished_at),badge(r.state),r.branch])):empty('No recorded runs.'),pager(offset,data.total,data.next_offset,n => loadJob(name,n)));
  }
  catch(e) {
    if(g === generation && selection === jobSelection && target.isConnected)target.replaceChildren(empty(errorText(e)));
  }
}
async function refresh() {
  if(session && !document.hidden && route.view==="memory") {await $("memory-root")?.load();return;}
  if(session && !document.hidden && route.view==="research") {$("research-root")?.load();return;}
  if(session && !document.hidden && route.view==="models") {const root=$("models-root");if(root)modelsView(root);return;}
  if(!session || document.hidden || route.newProject || !['overview','projects'].includes(route.view))return;
  controller?.abort();
  controller = new AbortController();
  const g = generation,key = location.pathname+location.search;
  const signal = controller.signal;
  $('connection').textContent = lastObserved?`Refreshing; previous observation ${date(lastObserved)}`:'Loading observations…';
  try {
    if(route.project) {
      const p = await request(`/browser/api/projects/${encodeURIComponent(route.project)}?offset=${Number(route.query.get('offset'))||0}&limit=20`, {
        signal
      }
      );
      if(g !== generation || key !== location.pathname+location.search)return;
      current = p;
      renderProject(p);
      if(route.tab === 'pending')await loadItems(signal,g);
      if(route.tab==="research")await $("research-root")?.load();
    }
    else {
      let offset = 0,rows = [],data,inventoryRevision = null,inventoryTotal = null;
      do {
        data = await request(`/browser/api/overview?limit=100&offset=${offset}`, {
          signal
        }
        );
        if(inventoryRevision === null) {
          inventoryRevision = data.inventory_revision;
          inventoryTotal = data.projects.total;
        }
        if(typeof inventoryRevision !== 'string' || data.inventory_revision !== inventoryRevision || data.projects.total !== inventoryTotal)throw new ApiError(0, {
          error:'inventory_changed'
        }
        );
        rows.push(...data.projects.items);
        offset = data.projects.next_offset;
        if(rows.length>=2000 && offset!=null)throw new ApiError(0, {
          error:'project_limit_reached'
        }
        );
      }
      while(offset!=null);
      if(rows.length !== inventoryTotal || new Set(rows.map(p => p.name)).size !== rows.length)throw new ApiError(0, {
        error:'inventory_changed'
      }
      );
      if(g !== generation || key !== location.pathname+location.search)return;
      projects = rows;
      attention = data.attention.items;
      renderProjects();
      if($('attention')) {
        replaceLive($('attention'),attentionList(attention));
        researchAttention();
        if(data.attention.total>attention.length)$('attention').append(el('p', {
          class:'metadata'
        }
        ,`Showing ${attention.length} of ${data.attention.total} attention items. Review project Pending tabs for details.`));
      }
      if($('active')) {
        const working = rows.filter(p => p.session_count>0);
        replaceLive($('active'),working.length?el('ul', {
          class:'attention'
        }
        ,working.map(p => el('li', {
        }
        ,link(p.display_name || p.name,projectUrl(p.name,'workspace')),el('span', {
          class:'muted'
        }
        ,`${p.session_count} workspace${p.session_count===1?'':'s'}`)))):empty('No workspace sessions in the current observation.'));
      }
      current = data;
    }
    if(g !== generation)return;
    lastObserved = current.observed_at;
    $('connection').textContent = `Project and workspace records; checked ${date(lastObserved)}.`;
  }
  catch(e) {
    if(e.name === 'AbortError' || g !== generation)return;
    $('connection').textContent = lastObserved?`Could not refresh. Showing stale data observed ${date(lastObserved)}.`:'Observations unavailable. Use Refresh to try again.';
    if(e.status !== 401)notice(errorText(e)+(lastObserved?' Earlier observations remain visible.':''),'error');
  }
}
function errorText(e) {
  const known = {
    project_creation_approval_required:'Project creation requires a current approval for your signed-in identity.',project_creation_not_configured:'Project creation is not configured on this server.',invalid_project_name:'Use 2–39 lowercase letters, digits or dashes, starting with a letter.',work_item_revision_conflict:'Someone changed this item. Your draft is preserved. Load the latest revision before saving again.',work_item_save_uncertain:'The save could not be confirmed. Your draft remains here. Check the latest saved item before retrying.',invalid_work_item_fields:'Check the title, field lengths and source link. Use an HTTPS URL or /ui/ app path.',work_item_storage_not_configured:'Persistent work-item storage is not configured.',login_required:'Sign in again, then choose Save when ready.',connection_unavailable:'Could not connect to Sigil. Your draft remains here.',provisioning_incomplete:'Provisioning is incomplete. A repository or deploy key may already exist. Retry the same project name to resume; no rollback was performed.',inventory_changed:'The project inventory changed during refresh. The previous list is retained. Refresh to load a consistent inventory.',project_limit_reached:'This view exceeds its bounded project limit. No partial list was substituted.'
  }
  ;
  return known[e.message] || `The request could not be completed (${label(e.message)}). Your draft remains here.`;
}
function inputField(form,name,title,value,options = {
}
) {
  const field = el(options.multiline?'textarea':'input', {
    name,id:`field-${name}`,maxlength:options.max || 200,...(!options.multiline? {
      type:'text'
    }
    : {
    }
    ),...options.attrs
  }
  );
  field.value = value || '';
  form.append(el('label', {
  }
  ,title,field));
  return field;
}
function renderNewProject() {
  heading('New project','Register a project through Sigil’s authenticated creation policy');
  const key = 'new-project',draft = drafts.get(key) || {
    name:''
  }
  ;
  drafts.set(key,draft);
  const form = el('form', {
    class:'panel form'
  }
  );
  inputField(form,'name','Project name',draft.name, {
    max:39,attrs: {
      required:true,pattern:'[a-z][a-z0-9\\-]{1,38}',autocomplete:'off'
    }
  }
  );
  form.append(el('p', {
    class:'form-note'
  }
  ,'Use 2–39 lowercase letters, digits or dashes, starting with a letter. Existing repositories can be adopted. Creation may require approval for your signed-in identity.'),el('p', {
    class:'form-note'
  }
  ,'If a response is lost, retry the same name. Existing keys and registration are reused.'),el('button', {
    type:'submit',class:'primary'
  }
  ,'Create project'),el('div', {
    class:'form-status',role:'status'
  }
  ));
  form.oninput = () => draft.name = form.elements.name.value;
  form.onsubmit = async e => {
    e.preventDefault();
    if(!form.reportValidity())return;
    const submit = form.querySelector('button'),status = form.querySelector('.form-status');
    submit.disabled = true;
    status.textContent = 'Creating or resuming project…';
    try {
      if(!session)throw new ApiError(401, {
        error:'login_required'
      }
      );
      await checkSession();
      const result = await request('/browser/api/projects', {
        method:'POST',body:JSON.stringify({
          name:draft.name
        }
        )
      }
      );
      status.className = 'form-status';
      status.replaceChildren(document.createTextNode(result.existing?'Existing project registration recovered. ':'Project registered. '),link('Open project',projectUrl(result.name)));
    }
    catch(error) {
      if(error.status === 401)showAuth();
      status.className = 'form-status error';
      status.textContent = errorText(error);
    }
    finally {
      submit.disabled = false;
    }
  }
  ;
  $('content').append(form);
  $('connection').textContent = 'Project creation uses your signed-in identity.';
}
async function loadItems(signal,g) {
  const project = route.project,offset = Number(route.query.get('items_offset')) || 0;
  try {
    const data = await request(`/browser/api/projects/${encodeURIComponent(project)}/work-items?offset=${offset}&limit=20`, {
      signal
    }
    );
    if(g !== generation)return;
    const target = $('work-items');
    replaceLive(target,data.items.length?el('ul', {
      class:'work-list'
    }
    ,data.items.map(i => el('li', {
    }
    ,el('button',{type:'button','data-focus-key':i.id,onclick:()=>selectItem(i.id)},i.title),badge(i.state),el('div', {
      class:'metadata'
    }
    ,`${i.owner||'Unassigned'} · Updated ${date(i.updated_at)}`)))):empty('No work items yet. Add an explicit next step.'),pager(offset,data.total,data.next_offset,n => {
      updateQuery({
        items_offset:n
      }
      );
      refresh();
    }
    ));
    const id = route.query.get('item');
    if(id && !drafts.has(itemKey(project,id)) && !(pendingItem?.id === id && pendingItem.selection === itemSelection))await selectItem(id,false);
  }
  catch(e) {
    if(e.name === 'AbortError' || g !== generation)return;
    if(e.status !== 401)$('work-items').replaceChildren(empty(errorText(e)));
  }
}
function itemKey(project,id) {
  return`item:${project}:${id||'new'}`;
}
function chooseNewItem() {
  itemSelection++;
  pendingItem = null;
  const key = itemKey(route.project,null), previous = drafts.get(key);
  // A new intent during an in-flight create owns a different draft. The old
  // response still has its object reference and will be stored under its ID.
  if(previous?._saving || previous?.revision)drafts.delete(key);
  updateQuery({item:null});
  renderItemEditor();
}
async function selectItem(id,changeUrl = true) {
  const g = generation,project = route.project,selection = ++itemSelection;
  pendingItem = {id,selection};
  if(changeUrl)updateQuery({item:id});
  renderItemEditor();
  const key = itemKey(project,id);
  try {
    if(!drafts.has(key)) {
      const item = await request(`/browser/api/projects/${encodeURIComponent(project)}/work-items/${encodeURIComponent(id)}`);
      // Cache this result under its own identity, never over an edited draft.
      if(!drafts.has(key))drafts.set(key,{...item});
    }
    if(g !== generation || selection !== itemSelection || route.query.get('item') !== id)return;
    pendingItem = null;
    renderItemEditor();
  } catch(e) {
    if(g === generation && selection === itemSelection){pendingItem = null;notice(errorText(e),'error');}
  }
}
function saveMessage(draft) {
  return draft._dirty?`Saved revision ${draft.revision}. Newer edits are unsaved.`:`Saved revision ${draft.revision}.`;
}
function syncEditor(draft) {
  if(activeDraft !== draft || !$('editor'))return;
  const form = $('editor').querySelector('form');
  if(!form)return;
  form.querySelector('[type=submit]').disabled = Boolean(draft._saving);
  const summary = form.querySelector('[data-revision-summary]');
  if(summary)summary.textContent = `Revision ${draft.revision}. Updated ${date(draft.updated_at)}`;
  const status = form.querySelector('.form-status');
  status.className = draft._error?'form-status error':'form-status';
  status.textContent = draft._saving?'Saving submitted version…':draft._error || draft._message || (draft._dirty?'Unsaved changes.':'');
}
function renderItemEditor() {
  const target = $('editor');
  if(!target)return;
  const project = route.project;
  let id = route.query.get('item'),key = itemKey(project,id),draft = drafts.get(key);
  const identity = ++editorIdentity;
  if(!draft) {
    if(id) {
      activeDraft = null;
      target.replaceChildren(empty('Loading work item…'),button('New item',chooseNewItem));
      return;
    }
    draft = {id:crypto.randomUUID(),title:'',description:'',state:'open',owner:'',source_link:''};
    drafts.set(key,draft);
  }
  // A create may finish while this page is away. Its original draft alias
  // resolves to the saved ID on return without replaying or losing newer edits.
  if(!id && draft.revision) {
    id = draft.id;
    if(drafts.get(key) === draft)drafts.delete(key);
    key = itemKey(project,id);
    drafts.set(key,draft);
    updateQuery({item:id});
  }
  activeDraft = draft;
  const g = generation,selection = itemSelection;
  const form = el('form',{class:'form'});
  const ownsEditor = () => g === generation && selection === itemSelection && identity === editorIdentity && activeDraft === draft && form.isConnected;
  form.append(el('h2',{},id?'Edit work item':'New work item'));
  inputField(form,'title','Title',draft.title,{attrs:{required:true}});
  inputField(form,'description','Description',draft.description,{multiline:true,max:8000});
  const state = el('select',{name:'state',id:'field-state','aria-label':'State'},['open','blocked','done'].map(v=>el('option',{value:v},label(v))));
  state.value = draft.state;
  form.append(el('label',{},'State',state));
  inputField(form,'owner','Owner',draft.owner);
  inputField(form,'source_link','Source link (optional)',draft.source_link,{max:2048});
  form.append(el('p',{class:'form-note'},'Use an HTTPS URL or a /ui/ app path. Updates are recorded with your signed-in identity.'),
    el('div',{class:'actions'},el('button',{type:'submit',class:'primary'},'Save work item'),button('New item',chooseNewItem)),
    el('div',{class:'form-status',role:'status'}));
  if(id) {
    let latestRequest = 0;
    form.append(button('Load latest revision',async()=> {
      const attempt = ++latestRequest;
      try {
        const latest = await request(`/browser/api/projects/${encodeURIComponent(project)}/work-items/${encodeURIComponent(id)}`);
        if(!ownsEditor() || attempt !== latestRequest)return;
        const status = form.querySelector('.form-status');
        status.replaceChildren(el('p',{},`Saved revision ${latest.revision}. Your draft is unchanged. Review the saved version below, then explicitly use its revision to save your draft.`),
          el('details',{open:true},el('summary',{},'Latest saved fields'),el('pre',{},JSON.stringify(Object.fromEntries(itemFields.map(name=>[name,latest[name]])),null,2))),
          button('Use latest revision for this draft',()=> {
            if(!ownsEditor() || attempt !== latestRequest)return;
            draft.revision = latest.revision;
            draft._error = null;
            draft._message = `Draft now based on revision ${latest.revision}. Choose Save to apply it.`;
            syncEditor(draft);
          }));
      } catch(e) {
        if(ownsEditor() && attempt === latestRequest)form.querySelector('.form-status').textContent = errorText(e);
      }
    }),el('p',{class:'metadata','data-revision-summary':true},`Revision ${draft.revision}. Updated ${date(draft.updated_at)}`),
    diagnostics({creator:draft.creator,editor:draft.editor,audit:draft.audit}));
  }
  form.oninput = () => {
    for(const name of itemFields)draft[name] = form.elements[name].value;
    draft._editVersion = (draft._editVersion || 0)+1;
    draft._dirty = true;
    draft._message = draft.revision?`Revision ${draft.revision}. Unsaved changes.`:'Unsaved changes.';
    if(!draft._saving)syncEditor(draft);
  };
  form.onsubmit = async e => {
    e.preventDefault();
    if(draft._saving || !form.reportValidity())return;
    // Snapshot before session revalidation. Edits during either await belong to
    // this same draft and must not be replaced by the submitted version.
    const submittedVersion = draft._editVersion || 0;
    const fields = Object.fromEntries(itemFields.map(name=>[name,draft[name]]));
    const expectedRevision = draft.revision;
    // This editor may have been reopened before an earlier create completed.
    // Mutation identity belongs to the shared draft, not the rendered URL.
    const mutationId = expectedRevision ? draft.id : null;
    draft._saving = true;
    draft._error = null;
    syncEditor(draft);
    try {
      if(!session)throw new ApiError(401,{error:'login_required'});
      await checkSession();
      const saved = await request(`/browser/api/projects/${encodeURIComponent(project)}/work-items${mutationId?'/'+encodeURIComponent(mutationId):''}`,{
        method:mutationId?'PATCH':'POST',body:JSON.stringify(mutationId?{expected_revision:expectedRevision,fields}:{id:draft.id,fields})
      });
      const changed = (draft._editVersion || 0) !== submittedVersion;
      for(const [name,value] of Object.entries(saved))if(!itemFields.includes(name) || !changed)draft[name] = value;
      draft._dirty = changed;
      draft._saving = false;
      draft._message = saveMessage(draft);
      drafts.set(itemKey(project,saved.id),draft);
      // A later item/New-item/page selection has sole authority over navigation.
      // Otherwise keep the original new alias so return navigation finds it.
      if(!id && ownsEditor()) {
        if(drafts.get(key) === draft)drafts.delete(key);
        updateQuery({item:saved.id});
        withFocus(target,renderItemEditor);
      }
      syncEditor(draft);
      // Publication is complete. Do not let a later refresh hold this save open.
      void refresh();
    } catch(error) {
      if(error.status === 401)showAuth();
      draft._error = errorText(error);
    } finally {
      draft._saving = false;
      syncEditor(draft);
    }
  };
  target.replaceChildren(form);
  syncEditor(draft);
}

document.addEventListener('visibilitychange',() => {
  if(document.hidden)controller?.abort();
  else refresh();
}
);
setInterval(() => refresh(),30000);
// Shared narrow helpers for later reviewed browser adapters. Credentials stay in the server.
window.Sigil = {
  el,request,navigate,showAuth,checkSession,openIDE,session:()=>session,rerender:renderRoute
}
;
renderRoute();
checkSession().then(() => refresh()).catch(e => {
  if(e.status !== 401)notice(errorText(e),'error');
}
);

// Research operation drafts survive auth recovery and rendered instances.
function inspectText(title,value) {return el('details',{},el('summary',{},title),el('pre',{class:'research-text'},typeof value==='string'?value:JSON.stringify(value,null,2)));}
function researchFailure(e) {return ({handoff_bundle_too_large_reduce_content:"The dossier is too large to transfer. Reduce its content, then preview and retry. No handoff was started.",workspace_changes_checkpoint_and_retry:"The workspace has uncommitted changes. Save and checkpoint them, then retry this handoff.",handoff_quarantined_recheck_operation:"The handoff result is not confirmed. The workspace is protected; recheck this same handoff.",editor_ownership_unknown:"The editor could not be safely paused. Workspace recovery is required.",workspace_changed:"The workspace changed during handoff. Your edits are preserved.",research_service_update_or_storage_repair_required:'Research service update or storage repair required. Existing runs remain readable.',research_operation_storage_required:'Durable research operation storage must be configured.',service_authorization_required:'Sign in again, then deliberately retry. Completed stages are preserved.',service_state_conflict:'The run changed or is no longer eligible. Refresh before retrying.',research_revision_conflict:'The run changed. Refresh and review its current stage.',research_project_association_required:'This run has no verified project association in Sigil.',research_operation_store_repair_required:'Operation storage needs repair. Keep the existing operation; do not start a replacement.'})[e.message]||errorText(e);}
function researchShell(project) {
 const root=el('section',{id:'research-root'}),message=el('p',{role:'status'}),list=el('div',{class:'research-list'}),detail=el('div',{class:'research-detail'});
 root.append(message);
 if(project)researchForm(root,project);
 else root.append(el('p',{},'Start research from a project’s Research tab. Older or externally created runs remain unassociated.'));
 root.append(el('div',{class:'research-layout'},list,detail));
 root.load=async(cursor=null)=>{
  const intent=++root.loadIntent;message.textContent='Loading research observations…';
  try {
   const q=new URLSearchParams();if(project)q.set('project',project);if(cursor)q.set('cursor',cursor);
   const data=await request('/browser/api/research?'+q);
   $("connection").textContent=data.state==="observed"?"Research observed "+date(data.observed_at):"Research observations unavailable";root.ready=data.readiness==="ready";if(root.startButton)root.startButton.disabled=!root.accepted && (!root.ready || root.pending);
   if(!root.isConnected || intent!==root.loadIntent)return;
   message.textContent=data.state==='observed'?`Observed ${date(data.observed_at)}. ${data.readiness==='ready'?'Research service ready.':'Research service update or storage repair required before starting or resuming.'}`:'Research service unavailable. Previous observations may be stale. Retry to check again.';
   if(data.state!=='observed')return;
   const rows=data.runs.map(r=>el('article',{},button(r.problem,()=>researchDetail(root,detail,r.run_id)),el('p',{},badge(r.status),' ',r.association.project||'Unassociated'),el('small',{},r.updated_at||r.created_at)));
   replaceLive(list,el('div',{},rows.length?rows:empty('No research runs in this page.'),data.next_cursor?button('Next page',()=>root.load(data.next_cursor)):null));
   const selected=new URLSearchParams(location.search).get('run');if(selected && !detail.childNodes.length)researchDetail(root,detail,selected);
  }catch(e){if(root.isConnected && intent===root.loadIntent)message.textContent=researchFailure(e)+' Previous observations may be stale.';}
 };
 root.loadIntent=0;root.load();return root;
}
function researchForm(root,project) {
 let d=researchDrafts.get(project);if(!d){d={id:crypto.randomUUID(),problem:'',context:'',pending:false,sent:null,accepted:null,papers:4,aperture:1};researchDrafts.set(project,d);}
 const form=el('form',{class:'research-form'}),problem=el('textarea',{id:'research-problem',required:true,maxlength:32768,rows:3}),context=el('textarea',{id:'research-context',maxlength:32768,rows:3}),count=el('input',{type:'number',id:'research-papers',min:1,max:10,value:4}),aperture=el('input',{type:'number',id:'research-aperture',min:1,max:3,value:1});
 problem.value=d.problem;context.value=d.context;count.value=d.papers||4;aperture.value=d.aperture||1;count.oninput=()=>{d.papers=Number(count.value);};aperture.oninput=()=>{d.aperture=Number(aperture.value);};problem.oninput=()=>{d.problem=problem.value;};context.oninput=()=>{d.context=context.value;};
 const feedback=el('p',{role:'status'}),start=el('button',{type:'submit',class:'primary'},d.accepted?'View run':d.sent?'Retry start':'Start research');root.startButton=start;root.accepted=!!d.accepted;start.disabled=!d.accepted;
 form.append(el('h2',{},'New research'),el('label',{for:'research-problem'},'Problem'),problem,el('label',{for:'research-context'},'Context'),context,el('div',{class:'research-options'},el('label',{for:'research-papers'},'Papers per category',count),el('label',{for:'research-aperture'},'Search plan variations',aperture)),el('p',{},'Starting research can use paid model providers. The service completes all research stages automatically.'),start,feedback);
 form.onsubmit=async(e)=>{
  e.preventDefault();if(d.accepted){researchDetail(root,root.querySelector(".research-detail"),d.accepted);return;}if(d.pending)return;d.pending=true;root.pending=true;start.disabled=true;
  try {
   if(!d.sent){
    const snapshot={operation_id:d.id,problem:d.problem,context:d.context,options:{papers_per_category:Number(d.papers),aperture:Number(d.aperture)}};
    const bytes=new TextEncoder();
    if(!snapshot.problem.trim() || bytes.encode(snapshot.problem).length>32768 || bytes.encode(snapshot.context).length>32768 || !Number.isInteger(snapshot.options.papers_per_category) || snapshot.options.papers_per_category<1 || snapshot.options.papers_per_category>10 || !Number.isInteger(snapshot.options.aperture) || snapshot.options.aperture<1 || snapshot.options.aperture>3)throw new Error('invalid_research_options');
    d.sent=snapshot;
   }
   await checkSession();
   const result=await request(`/browser/api/projects/${encodeURIComponent(project)}/research`,{method:"POST",body:JSON.stringify(d.sent)});
   d.accepted=result.run_id;root.accepted=true;feedback.replaceChildren("Research accepted. "+(d.context!==d.sent.context||d.problem!==d.sent.problem?"Newer edits remain unsent. Choose New research to use them. ":""),button("New research",()=>{researchDrafts.set(project,{...d,id:crypto.randomUUID(),sent:null,accepted:null,pending:false});const next=researchShell(project);root.replaceWith(next);}));
   start.textContent='View run';root.load();
  }catch(e){
   if(e.message==='invalid_research_options'){
    // This code is emitted only before the server reserves an operation. A
    // deliberate corrected submission receives a new identity; ambiguous
    // service/auth failures retain the original frozen key and payload.
    d.sent=null;d.id=crypto.randomUUID();
    feedback.textContent='Research was not started. Enter a nonblank problem, keep each text field within 32,768 UTF-8 bytes, and check the options. Edit the draft, then choose Start research.';start.textContent='Start research';
   }else{feedback.textContent=researchFailure(e)+' The start result may be unknown. Retry start reuses the original submitted problem and options; newer edits stay unsent.';start.textContent='Retry start';}
  }
  finally{d.pending=false;root.pending=false;start.disabled=!d.accepted && !root.ready;}
 };
 root.append(form);
 const recoveryList=el('div',{});root.append(recoveryList);
 request(`/browser/api/projects/${encodeURIComponent(project)}/research/operations`).then(data=>{
  if(!root.isConnected)return;
  for(const operation of data.operations.filter(o=>o.handoff && o.handoff.phase!=="complete")){recoveryList.append(button("Recover project handoff: "+operation.problem,async()=>{try{const p=await request(`/browser/api/research/${encodeURIComponent(operation.run_id)}/handoff`);if(root.isConnected)recoveryList.append(handoffPanel(p,operation.run_id));}catch(e){feedback.textContent=researchFailure(e);}}));}
  for(const operation of data.operations.filter(o=>o.state==='acceptance_unknown')){
   let busy=false;const b=button('Recover '+operation.problem,async()=>{if(busy)return;busy=true;b.disabled=true;try{await checkSession();const r=await request(`/browser/api/projects/${encodeURIComponent(project)}/research/operations/${encodeURIComponent(operation.operation_id)}/recover`,{method:'POST'});researchDetail(root,root.querySelector('.research-detail'),r.run_id);b.remove();root.load();}catch(e){feedback.textContent=researchFailure(e);}finally{busy=false;b.disabled=false;}});recoveryList.append(b);
  }
 }).catch(e=>{if(root.isConnected)feedback.textContent=researchFailure(e);});
}
async function researchDetail(root,target,id) {
 const intent=++researchSelection;target.replaceChildren(empty('Loading run…'));
 try {
  const r=await request('/browser/api/research/'+encodeURIComponent(id));
  if(!root.isConnected || intent!==researchSelection)return;
  const url=new URL(location.href);url.searchParams.set('run',id);history.replaceState({},'',url);
  const feedback=el('p',{role:'status'});let pending=false;
  const perform=async(action,output)=>{
   if(pending)return;pending=true;
   try{await checkSession();await request('/browser/api/research/'+encodeURIComponent(id),{method:'POST',body:JSON.stringify({action,expected_revision:r.revision,...(output===undefined?{}:{output})})});if(root.isConnected && intent===researchSelection){await researchDetail(root,target,id);root.load();}}
   catch(e){feedback.textContent=researchFailure(e)+' No automatic retry was made. Refresh the run before another action.';}
   finally{pending=false;}
  };
  target.replaceChildren(el('h2',{},r.problem),el('p',{},badge(r.status),' ',r.association.project||'Unassociated'),feedback);
  if(r.detail)target.append(el('p',{class:'warning'},r.detail));
  target.append(el('ol',{class:'stage-timeline'},r.stages.map(s=>el('li',{},el('strong',{},label(s.stage)),' ',badge(s.status),el('p',{},s.computed_by||'No computation provenance recorded'),s.detail?el('p',{class:'warning'},s.detail):null,s.catalog_attempt?el('small',{},'Catalog attempt '+s.catalog_attempt):null))));
  if(r.status==='failed')target.append(el('p',{},'Completed stages stay intact. An explicit resume may repeat unfinished work that was not checkpointed.'),button('Resume unfinished stages',()=>perform('resume')));
  if(r.awaiting)target.append(el('p',{class:'warning'},'Waiting for caller until '+r.awaiting.deadline),inspectText('Exact caller stage payload',r.awaiting));
  if(r.artifacts.arxiv_only)target.append(el('p',{class:'warning'},'Evidence is arXiv only; wider web evidence did not contribute.'));
  if(r.status==='done' && !r.artifacts.outcome)target.append(el('p',{class:'warning'},'Stages finished without a decision. Supply a valid convergence result to regenerate the dossier and enable handoff.'));
  if(r.awaiting || (r.status==='done'&&!r.artifacts.outcome)){
   const input=el('textarea',{'aria-label':'Stage result JSON',rows:7}),stage=r.awaiting?.stage||'converge';
   target.append(el('details',{},el('summary',{},'Submit an existing '+stage+' result'),el('p',{},'Paste the exact SDE stage output JSON. The service validates its schema and citations.'),input,button('Submit stage result',()=>{try{perform(stage,JSON.parse(input.value));}catch{feedback.textContent='Enter valid JSON before submitting.';}})));
  }
  target.append(inspectText('Corpus',r.artifacts.corpus),inspectText('Decision',r.artifacts.outcome),inspectText('Dossier',r.artifacts.dossier||'Not available'),inspectText('Catalog provenance',r.catalog_attempts));
  if(r.artifacts.adhd_run_id)target.append(button('Inspect ADHD run '+r.artifacts.adhd_run_id,async()=>{try{const a=await request(`/browser/api/research/${encodeURIComponent(id)}/adhd`);if(root.isConnected && intent===researchSelection)target.append(inspectText('Recorded ADHD detail',a));}catch(e){feedback.textContent=researchFailure(e);}}));
  if(r.handoff_available && r.association.project)target.append(button('Preview project handoff',async()=>{try{const p=await request(`/browser/api/research/${encodeURIComponent(id)}/handoff`);if(root.isConnected && intent===researchSelection)target.append(handoffPanel(p,id));}catch(e){feedback.textContent=researchFailure(e);}}));
 }catch(e){if(root.isConnected && intent===researchSelection)target.replaceChildren(empty(researchFailure(e)),button('Retry run',()=>researchDetail(root,target,id)));}
}
async function modelsView(root) {
 const intent=++root.intent;const feedback=root.querySelector('[role=status]');feedback.textContent='Loading model observations…';
 try {
  const d=await request('/browser/api/models');if(!root.isConnected||intent!==root.intent)return;
  feedback.textContent='Observed '+date(d.observed_at);$("connection").textContent="Observed "+date(d.observed_at)+([d.status,d.info,d.usage].some(s=>s.state!=="observed")?"; some model observations unavailable":"");const body=root.querySelector('.models-data');body.replaceChildren();
  body.append(el('p',{},'Recent observation window. Records may be missing, and requests are not attributed to projects. This is not complete billing. Unknown token counts remain unknown.'));
  if(d.status.state==='observed'){const s=d.status.data;body.append(el('h2',{},'Bound model slots'),makeTable(['Slot','Provider kind','Model','Download','Loaded'],s.slots.map(s=>[s.provider+'/'+s.size,s.kind,s.model,s.download_error?'Download error':s.download_progress?JSON.stringify(s.download_progress):s.downloaded?'Downloaded':'Not downloaded',s.loaded?'Loaded':'Not loaded'])),inspectText('Local model lock holder',s.lock));}else body.append(empty('Model status '+label(d.status.state)));
  if(d.usage.state==='observed')body.append(el('h2',{},'Recent requests'),makeTable(['Model','Time','Input tokens','Output tokens','Status'],d.usage.data.entries.map(u=>[u.provider+'/'+u.size+' '+u.model,new Date(u.ts_ms).toLocaleString(),u.input_tokens??'Unknown',u.output_tokens??'Unknown',u.status])));else body.append(empty('Recent usage '+label(d.usage.state)));
  if(d.info.state==='observed')body.append(inspectText('Available models and downloaded artifacts',d.info.data));else body.append(empty('Model catalog '+label(d.info.state)));
 }catch(e){if(root.isConnected&&intent===root.intent)feedback.textContent=researchFailure(e)+' Previous model observations may be stale.';}
}

function handoffPanel(p,id) {
 const panel=el('section',{class:'handoff-preview'},el('h3',{},'Handoff to '+p.project));
 panel.append(el('p',{},'Save your editor buffers before continuing. This action pauses the editor and commits only these dossier files to the project’s session branch. It does not merge to master or index memory.'));
 for(const file of p.bundle.files)panel.append(inspectText(file.path,file.content));
 const feedback=el('p',{role:'status'});let busy=false,completed=false;
 const commit=button(p.recovery?'Recover this handoff':'Pause editor and commit dossier',async()=>{
  if(busy)return;busy=true;commit.disabled=true;
  try{
   await checkSession();
   const workspace=p.recovery||await request(`/browser/api/projects/${encodeURIComponent(p.project)}/ide`,{method:'POST',body:JSON.stringify({idempotency_key:'research-handoff-'+id})});
   const receipt=await request(`/browser/api/research/${encodeURIComponent(id)}/handoff`,{method:'POST',body:JSON.stringify({session_id:workspace.session_id,generation:workspace.generation,bundle_digest:p.bundle.digest})});
   completed=true;commit.textContent="Dossier committed";
   feedback.replaceChildren('Committed and pushed on the session branch: '+receipt.commit+'. '+(receipt.dirty?'Concurrent changes remain in the workspace. ':'')+'Master acceptance and memory indexing are still pending. ',button('Open IDE',()=>openIDE(p.project)));
  }catch(e){feedback.textContent=researchFailure(e)+' Work is preserved. Retry this handoff after resolving the reported condition.';}
  finally{busy=false;commit.disabled=completed;}
 });panel.append(commit,feedback);return panel;
}
async function researchAttention() {
 const target=document.getElementById('research-attention');if(!target)return;const g=generation;
 try {const data=await request('/browser/api/research');if(g!==generation||!target.isConnected)return;
  if(data.state!=='observed'){target.textContent='Research attention unavailable; existing observations may be stale.';return;}
  const runs=data.runs.filter(r=>['failed','awaiting_caller'].includes(r.status));
  replaceLive(target,el('div',{},runs.length?runs.map(r=>el('p',{},badge(r.status),' ',link(r.problem,'/ui/research?run='+encodeURIComponent(r.run_id)))):empty('No research failures or caller waits in the latest page.'),data.next_cursor?link('Review further research pages','/ui/research'):null));
 }catch{if(g===generation&&target.isConnected)target.textContent='Research attention unavailable.';}
}
