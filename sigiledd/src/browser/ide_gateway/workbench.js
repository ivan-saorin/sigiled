'use strict';
(async()=>{
  const status=document.getElementById('status'),frame=document.getElementById('editor');
  let session,lastActivity=0,busy=false;
  const message=e=>{status.textContent=e.status===401?'Your sign-in expired. Return to Sigil and check your session; your workspace is preserved.':e.message;};
  async function request(path,body){const r=await fetch(path,{method:body?'POST':'GET',credentials:'same-origin',cache:'no-store',headers:body?{'Content-Type':'application/json','X-Sigil-CSRF':session.csrf_token}:{},body:body?JSON.stringify(body):undefined});let data;if(r.status===204){await r.text();data={};}else{data=await r.json();}if(!r.ok){const e=new Error((data.error||'Workspace unavailable').replaceAll('_',' ')+'. Your work is preserved.');e.status=r.status;throw e;}return data;}
  async function activity(e){if(!e.isTrusted || document.hidden || Date.now()-lastActivity<15000)return;lastActivity=Date.now();try{await request('/_sigil/activity',{});}catch(e){message(e);}}
  async function inspect(){try{const s=await request('/_sigil/status');const p=s.provider?.observed||s.provider;status.textContent=p?.durability?.state==='pushed'?'Saved files pushed. Unsaved editor buffers remain in the editor.':(p?.error||p?.state||'Workspace open').replaceAll('_',' ');}catch(e){message(e);}}
  for(const [id,action] of [['checkpoint','checkpoint'],['finish','finish']])document.getElementById(id).onclick=async()=>{
    if(busy || !session)return;
    if(action==='finish' && !confirm('Finish this workspace? Save your editor buffers first. Sigil will stop the editor, push saved files, and attempt to merge. Failures preserve the workspace for recovery.'))return;
    busy=true;status.textContent=action==='finish'?'Finishing workspace…':'Checkpointing saved files…';
    try {const result=await request('/_sigil/operation',{action,generation:session.generation});status.textContent=action==='finish'?`Workspace finished. Merge outcome: ${result.merge||'recorded'}.`:'Saved files checkpointed and pushed.';if(action==='finish'){frame.remove();document.getElementById('finish').disabled=true;document.getElementById('checkpoint').disabled=true;}}
    catch(e){message(e);}finally{busy=false;}
  };
  document.getElementById('preview').onclick=async()=>{
    if(busy || !session)return;
    const popup=window.open('about:blank','_blank');if(popup)popup.opener=null;
    const link=document.getElementById('preview-link');busy=true;
    try{const result=await request('/_sigil/preview',{generation:session.generation,port:Number(document.getElementById('preview-port').value)});link.href=result.launch_url;link.hidden=false;if(popup)popup.location.replace(result.launch_url);status.textContent='Preview ready. Use Continue to preview if a new tab did not open.';}
    catch(e){if(popup)popup.close();message(e);}finally{busy=false;}
  };
  try {
    session=await request('/_sigil/session');
    if(typeof session.generation!=='string' || !/^\d+$/.test(session.generation) || !session.editor_url.startsWith('/?folder='))throw new Error('Invalid workspace navigation.');
    const ports=document.getElementById('preview-port');for(const port of session.preview_ports||[]){const option=document.createElement('option');option.value=String(port);option.textContent=String(port);ports.append(option);}ports.disabled=!ports.options.length;document.getElementById('preview').disabled=ports.disabled;if(ports.disabled)document.getElementById('preview').title='No isolated preview ports are configured for this project.';
    frame.addEventListener('load' ,()=>{try{const d=frame.contentDocument;for(const event of ['keydown','pointerdown','input'])d.addEventListener(event,activity,true);}catch(e){status.textContent='Editor activity could not be observed. Return to Sigil to renew your sign-in.';}});
    frame.src=session.editor_url;
    await inspect();setInterval(()=>{if(!document.hidden && !busy)void inspect();},30000);
  }catch(e){message(e);}
})();
