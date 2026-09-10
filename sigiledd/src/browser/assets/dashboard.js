/* Sigil browser shell. Data enters the DOM only through textContent/text nodes. */
'use strict';
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
    headers['X-Sigil-CSRF'] = session?.csrf_token || '';
  }
  if(options.method === 'POST' && !options.body)headers['X-Sigil-CSRF'] = session?.csrf_token || '';
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
      notice('Signed in. Review your draft and choose Save when ready.','good');
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
    view:'overview',query:u.searchParams
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
function renderProject(p) {
  heading(p.display_name || p.name,p.description || p.name,el('div', {
  }
  ,el('button', {
    type:'button',disabled:true
  }
  ,'Open IDE'),el('div', {
    class:'metadata'
  }
  ,'IDE integration pending')));
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
    ,'Closing this browser does not close or merge a workspace. IDE launch, checkpoint and finish controls await the reviewed IDE adapter.'));
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
    ,`Generation ${s.generation??'not observed'}`)),el('div', {
    }
    ,badge(s.ready === true?'ready':'pending'),el('p', {
    }
    ,label(s.error || s.readiness?.reason || 'readiness_not_observed')),diagnostics(s))])):empty('No workspace sessions recorded for this project.'));
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
    ,'Memory curation is pending its browser adapter.'),facts([['Sharing',label(p.memory?.sharing || 'private')]]));
    break;
    case'research':panel.append(el('h2', {
    }
    ,'Research'),empty('Research is pending its reviewed orchestration adapter. No workflow has been started from this dashboard.'));
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
  el,request,navigate,showAuth,checkSession
}
;
renderRoute();
checkSession().then(() => refresh()).catch(e => {
  if(e.status !== 401)notice(errorText(e),'error');
}
);
