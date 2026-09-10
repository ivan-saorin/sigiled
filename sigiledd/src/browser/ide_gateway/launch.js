'use strict';
(async()=>{
  const ticket=location.hash.slice(1);
  history.replaceState(null,'','/_sigil/launch');
  const status=document.getElementById('status');
  try {
    if(!/^[A-Za-z0-9_-]{43}$/.test(ticket))throw new Error('This launch link is missing or has already been used. Return to Sigil and choose Open IDE again.');
    const r=await fetch('/_sigil/consume',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({ticket}),credentials:'same-origin',cache:'no-store',referrerPolicy:'no-referrer'});
    if(!r.ok)throw new Error('This launch link expired or your access changed. Return to Sigil, check your sign-in, and choose Open IDE again.');
    const data=await r.json();
    if(data.destination!=='/_sigil/workbench' && data.destination!=='/')throw new Error('The workspace returned an invalid launch destination.');
    location.replace(data.destination);
  }catch(e){status.textContent=e.message;}
})();
