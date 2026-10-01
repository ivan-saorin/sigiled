/* Project journeys. Durable records remain authoritative; browser markers are personal reading aids. */
'use strict';
window.SigilJourneys = (() => {
  const drafts = new Map();
  const safeLink = value => {
    try {
      if (typeof value !== 'string' || /[\\\x00-\x20]/.test(value)) return null;
      const u = new URL(value, location.origin);
      if (u.username || u.password) return null;
      return u.protocol === 'https:' || (value.startsWith('/ui/') && u.origin === location.origin) ? u.href : null;
    } catch { return null; }
  };
  const actorKey = c => c.actor || 'unknown';
  const markerKey = (p,c) => `sigil:reviewed:${encodeURIComponent(actorKey(c))}:${encodeURIComponent(p.name)}`;
  function overview(p,c) {
    const {el,link,button,date,projectUrl} = c;
    let since = null;
    try { const v=Number(localStorage.getItem(markerKey(p,c))); if(Number.isFinite(v)&&v>0&&v<=p.observed_at)since=v; } catch {}
    const activity = p.activity || {items:[],total:0};
    // Include the boundary second: a later event can share the snapshot's timestamp.
    const events = (activity.items || []).filter(a => !since || a.at_epoch>=since);
    const node=el('section',{class:'journey-home'},
      el('div',{class:'journey-intro'},el('span',{class:'eyebrow'},'PROJECT BRIEF'),el('h2',{},since?'Since you last caught up':'Pick up where the project stands'),
        el('p',{class:'muted'},since?`You marked this project reviewed on ${date(since)} in this browser.`:'Review recent work, the questions waiting for you and the latest results.')),
      el('div',{class:'journey-grid'},
        el('article',{class:'journey-card'},el('h3',{},'Needs you'),el('p',{},`${p.attention?.total ?? 0} system attention item${p.attention?.total===1?'':'s'}`),el('p',{'data-question-count':'',class:'metadata'},c.requestsEnabled?'Checking agent questions…':'Agent questions need service setup.'),link('Review agent questions',projectUrl(p.name,'requests')),link('Review system attention',projectUrl(p.name,'pending'))),
        el('article',{class:'journey-card'},el('h3',{},'Work in progress'),el('p',{},`${p.sessions?.total ?? p.session_count ?? 0} open workspace session${(p.sessions?.total ?? p.session_count)===1?'':'s'}`),el('p',{class:'metadata'},'An open session does not prove an external agent is running.'),link('Inspect workspaces',projectUrl(p.name,'workspace'))),
        el('article',{class:'journey-card'},el('h3',{},'Results and knowledge'),link('Research results',projectUrl(p.name,'research')),link('Project memory',projectUrl(p.name,'memory')),link('Deployment status',projectUrl(p.name,'app')))),
      el('h3',{},since?'Changes since your review':'Recent recorded changes'));
    node.append(events.length?el('ol',{class:'journey-timeline'},events.slice(0,8).map(a=>el('li',{},el('time',{class:'metadata'},date(a.at_epoch)),el('strong',{},c.label(a.event?.kind)),el('span',{},a.event?.job || a.event?.session_id || 'Project')))):el('p',{},since?'No newer events in this activity page.':'No project activity has been recorded.'));
    if(activity.next_offset!=null || events.length>8 || (activity.total||0)>(activity.items||[]).length)node.append(el('p',{class:'metadata'},'This is a recent activity preview, not a complete change count. Open the activity history for older pages.'));
    const status=el('p',{class:'metadata',role:'status'});
    const mark=button('Mark caught up',()=>{
      try {
        if(!Number.isFinite(p.observed_at)||!p.observed_at)throw new Error();
        localStorage.setItem(markerKey(p,c),String(p.observed_at));
        status.textContent='Review point saved in this browser. Project work and other people’s review points are unchanged.';
        mark.disabled=true;
      } catch {status.textContent='This browser could not save your review point. Project records are unchanged.';}
    });
    node.append(el('div',{class:'actions'},link('Open activity history',projectUrl(p.name,'activity')),mark),status);
    node.load=async()=>{
      if(!c.requestsEnabled)return;
      const count=node.querySelector('[data-question-count]');
      try {const data=await c.request(`/browser/api/projects/${encodeURIComponent(p.name)}/requests?limit=1`);if(node.isConnected)count.textContent=`${data.waiting} agent question${data.waiting===1?'':'s'} waiting for you`;}
      catch {if(node.isConnected)count.textContent='Agent questions could not be checked. Open Needs you to retry.';}
    };
    return node;
  }
  function deployment(p,c) {
    const {el,link,button,date,facts,badge,projectUrl}=c,app=p.app||{},build=app.latest_build,runtime=app.runtime||{};
    const failed=build?.ok===false,building=app.state==='building';
    const title=building?'A build is in progress':failed?'The latest build failed':app.deployed_revision?'A deployed version is recorded':'No deployed version is recorded';
    const node=el('section',{class:'deployment-journey'},el('span',{class:'eyebrow'},'SOURCE TO RUNNING SERVICE'),el('h2',{},title));
    if(failed&&!building)node.append(el('p',{class:'notice error'},app.deployed_revision?'The failed build and the recorded deployment are separate. The deployment record does not establish current service health.':'The build failed and no deployed version is recorded.'));
    node.append(el('div',{class:'journey-grid deployment-stages'},
      el('article',{class:'journey-card'},el('h3',{},'1 · Source'),facts([['Repository revision',p.repository_revision||'Not observed'],['Last observed',date(p.setup?.observed_at)]])),
      el('article',{class:'journey-card'},el('h3',{},'2 · Latest completed build'),facts([['Result',build?badge(build.ok===true?'succeeded':build.ok===false?'failed':'unknown'):'No build record'],['Built revision',build?.revision||'Not recorded'],['Finished',date(build?.finished_at)]])),
      el('article',{class:'journey-card'},el('h3',{},'3 · Deployment'),facts([['Deployed revision',app.deployed_revision||'Not deployed'],['Runtime health',badge(runtime.state||'unknown')],['Health observed',date(runtime.observed_at)]]))));
    if(p.setup?.stale)node.append(el('p',{class:'notice'},'Source information is stale. Refresh before choosing a revision.'));
    if(app.revision_drift===true)node.append(el('p',{},'The repository revision differs from the recorded deployment. A successful build alone does not publish a version.'));
    node.append(el('h3',{},'Next step'),el('p',{},building?'Wait for the current build result, then refresh this view.':failed?'Give your agent the failed revision and build time below. Ask it to inspect the build evidence, correct the cause on a branch and report its validation. Deployment still requires the normal operator action.':'Review the source, build and deployment records independently before making a change.'),el('p',{class:'metadata'},'This view does not probe the running service or expose build logs. Health remains unknown until an authorized runtime check confirms it.'));
    const brief=`Project: ${p.name}\nSource revision: ${p.repository_revision||'unknown'}\nSource observed: ${date(p.setup?.observed_at)}${p.setup?.stale?' (stale)':''}\nLatest completed build: ${build?.ok===false?'failed':build?.ok===true?'succeeded':'not recorded'}\nBuild revision: ${build?.revision||'unknown'}\nBuild finished: ${date(build?.finished_at)}\nRecorded deployed revision: ${app.deployed_revision||'none'}\nRuntime health: ${runtime.state||'unknown'}; observed: ${date(runtime.observed_at)}\nInspect the authoritative build evidence and report the cause and verified fix. Preserve the deployed version. Do not deploy or change access as part of this diagnosis.`;
    const details=el('details',{},el('summary',{},'Agent handoff'),el('pre',{},brief)),status=el('span',{role:'status',class:'metadata'});
    node.append(el('div',{class:'actions'},button('Copy diagnostic handoff',async()=>{try{await navigator.clipboard.writeText(brief);status.textContent='Handoff copied. Open it in your external agent.';}catch{details.open=true;status.textContent='Copy the handoff text below.';}}),link('View project activity',projectUrl(p.name,'activity'))),status,details);
    return node;
  }
  function requests(p,c) {
    const {el,button,link,request,date,projectUrl}=c;
    const project=p.name,base=`/browser/api/projects/${encodeURIComponent(project)}/requests`;
    const root=el('section',{class:'requests-journey','data-actor':actorKey(c)}),list=el('div',{class:'request-list'}),detail=el('section',{class:'request-detail','aria-label':'Selected question'}),status=el('p',{role:'status',class:'metadata'});
    root.append(el('span',{class:'eyebrow'},'COLLABORATE WITH YOUR AGENTS'),el('h2',{},'Needs you'),el('p',{},'Questions recorded by external agents. Your answer is saved for the agent to read; it does not start a conversation, execute work or grant access.'),status,el('div',{class:'request-layout'},list,detail));
    let selected=c.query.get('request'),offset=0,epoch=0,detailEpoch=0,shown=null,listShown=null;
    const getDraft=i=>{
      const key=`${actorKey(c)}:${project}:${i.id}`;
      if(!drafts.has(key))drafts.set(key,{text:'',pending:false,submitted:null,error:null});
      return drafts.get(key);
    };
    const draw=i=>{
      if(!root.isConnected)return;
      const d=getDraft(i);
      // An older read may finish after an answer receipt, including across navigation.
      if(d.record?.revision>i.revision)i=d.record;else d.record=i;
      const q=i.request;
      if(!q)return;
      const fingerprint=JSON.stringify([i.id,i.revision,q.answer,d.pending,d.error]);
      if(shown===fingerprint)return;
      shown=fingerprint;
      detail.replaceChildren(el('h3',{},i.title),el('p',{class:'metadata'},`Requested by ${i.creator} · ${date(i.created_at)}`),el('p',{class:'request-question'},q.question),q.context?el('p',{class:'request-context'},q.context):document.createTextNode(''));
      const source=safeLink(i.source_link);if(source)detail.append(link('Open request context',source,{target:'_blank',rel:'noopener noreferrer'}));
      if(q.answer){
        d.pending=false;d.submitted=null;
        detail.append(el('div',{class:'notice good'},el('strong',{},'Answer recorded'),el('p',{},q.answer.text),el('p',{class:'metadata'},`${q.answer.actor} · ${date(q.answer.at)}`)),el('p',{},'The answer is available to the external agent. Its continuation has not been observed.'));
        return;
      }
      const field=el('textarea',{rows:5,maxlength:8000,disabled:d.pending||d.submitted?true:null});field.value=d.text;
      field.addEventListener('input',()=>{d.text=field.value;});
      const feedback=el('p',{class:'form-status',role:'status'},d.error||'');
      const save=button(d.pending?'Saving answer…':d.submitted?'Retry this answer':'Record answer',async()=>{
        if(d.pending)return;
        if(!d.submitted && (!d.text.trim()||new TextEncoder().encode(d.text).length>8000)){feedback.textContent='Write an answer of at most 8,000 bytes.';return;}
        const submitted=d.submitted||{expected_revision:i.revision,text:d.text};d.submitted=submitted;d.pending=true;d.error=null;shown=null;draw(i);
        try {
          // Browser identity/CSRF remains authoritative; never replay after sign-in.
          await c.checkIdentity();
          const saved=await request(`${base}/${encodeURIComponent(i.id)}/answer`,{method:'POST',body:JSON.stringify(submitted)});
          d.pending=false;d.submitted=null;d.error=null;d.record=saved;
          if(selected===i.id&&root.isConnected){shown=null;draw(saved);root.load();}
        } catch(e) {
          d.pending=false;
          if([400,401,403,404,409,413,422].includes(e.status))d.submitted=null;
          d.error=e.message==='identity_changed'?'The signed-in person changed. Refresh this project before answering.':e.status===409?'This request changed or was answered elsewhere. Reload the question before answering.':d.submitted?'The save is not confirmed. Check the recorded answer or retry this exact answer.':c.errorText(e);
          if(selected===i.id&&root.isConnected){shown=null;draw(i);}
        }
      },'primary');save.disabled=d.pending;
      detail.append(el('label',{},'Your answer',field),el('p',{class:'form-note'},'An answer applies only to this question. SIGIL’s permission and deployment approvals remain separate.'),el('div',{class:'actions'},save,button('Check recorded answer',()=>choose(i.id))),feedback);
    };
    async function choose(id) {
      if(selected!==id){shown=null;detail.replaceChildren(el('p',{},'Loading question…'));}
      selected=id;c.updateQuery({request:id});const tick=++detailEpoch;
      for(const row of list.querySelectorAll('[data-request-id]')){const active=row.dataset.requestId===id;row.classList.toggle('request-selected',active);row.setAttribute('aria-pressed',String(active));}
      try{const i=await request(`${base}/${encodeURIComponent(id)}`);if(tick===detailEpoch&&selected===id&&root.isConnected){shown=null;draw(i);}}
      catch(e){if(tick===detailEpoch&&root.isConnected)status.textContent=c.errorText(e);}
    }
    root.load=async()=>{
      const tick=++epoch;
      try{
        const data=await request(`${base}?offset=${offset}&limit=20`);
        if(tick!==epoch||!root.isConnected)return;
        status.textContent=`${data.waiting ?? 0} waiting · ${data.total} recorded question${data.total===1?'':'s'}. Checked ${new Date().toLocaleTimeString()}.`;
        if(!selected&&data.items.length){selected=data.items[0].id;c.updateQuery({request:selected});}
        const listFingerprint=JSON.stringify([offset,data.total,data.next_offset,data.items.map(i=>[i.id,i.title,i.revision])]);
        if(listShown!==listFingerprint){
          const focus=list.contains(document.activeElement)?document.activeElement.dataset.requestId:null;
          listShown=listFingerprint;
          list.replaceChildren(...data.items.map(i=>{const row=button(`${i.request?.answer?'Answered':'Waiting'} · ${i.title}`,()=>choose(i.id),selected===i.id?'request-selected':'');row.dataset.requestId=i.id;row.setAttribute('aria-pressed',String(selected===i.id));return row;}));
          if(!data.items.length)list.append(el('p',{},'No agent questions recorded for this project.'));
          if(offset>0||data.next_offset!=null)list.append(c.pager(offset,data.total,data.next_offset,n=>{offset=n;root.load();}));
          if(focus)Array.from(list.querySelectorAll('[data-request-id]')).find(row=>row.dataset.requestId===focus)?.focus();
        }
        const i=data.items.find(i=>i.id===selected);
        if(i)draw(i);else if(selected)await choose(selected);
        else detail.replaceChildren(el('p',{},'Your agent can record a question through the project requests API. It remains here until answered.'));
      }catch(e){if(tick===epoch&&root.isConnected)status.textContent='Questions could not refresh. Any earlier records are stale. '+c.errorText(e);}
    };
    root.append(link('Review system attention and tracked work',projectUrl(project,'pending')));
    return root;
  }
  return {overview,deployment,requests};
})();
