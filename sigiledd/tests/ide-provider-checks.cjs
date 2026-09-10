const assert=require('node:assert/strict');
async function trustedActivity(page,action){
 const response=page.waitForResponse(r=>r.url().endsWith('/_sigil/activity')&&r.request().method()==='POST',{timeout:3000});
 // Observe before input; a request count is never evidence of successful renewal.
 const [,observed]=await Promise.all([action(),response]);assert.equal(observed.status(),204,'trusted login activity must succeed');return observed.status();
}
class Diagnostics {
 constructor(){this.http=[];this.console=[];this.network=[];this.expectedHttp=new Map();this.expectedConsole=new Set();this.expectedNetwork=new Map();}
 expectHttp(url,status,method='GET'){this.expectedHttp.set(method+' '+url,status);}
 expectConsole(text,url=null,phase=null){assert(url||/https?:\/\/|wss?:\/\//.test(text),"console exception must identify its exact URL");this.expectedConsole.add(JSON.stringify([text,url,phase]));}
 expectNetwork(url,error,{method='GET',status=null,resourceType=null,phase=null}={}){this.expectedNetwork.set(method+' '+url+' '+error,{status,resourceType,phase});}
 check(){
  const badHttp=this.http.filter(r=>this.expectedHttp.get(r.method+' '+r.url)!==r.status);
  const phrases={403:'Forbidden',404:'Not Found',409:'Conflict'};
  const badConsole=this.console.filter(r=>!this.expectedConsole.has(JSON.stringify([r.text,r.url,r.phase||null]))&&!this.expectedConsole.has(JSON.stringify([r.text,r.url,null]))&&!this.expectedConsole.has(JSON.stringify([r.text,null,null]))&&!Object.entries(phrases).some(([status,phrase])=>this.http.some(h=>h.url===r.url&&h.status===Number(status)&&this.expectedHttp.get(h.method+' '+h.url)===h.status)&&r.text===`Failed to load resource: the server responded with a status of ${status} (${phrase})`));
  const badNetwork=this.network.filter(r=>{const expected=this.expectedNetwork.get(r.method+' '+r.url+' '+r.error);return !expected || (expected.phase!==null&&expected.phase!==r.phase) || (expected.resourceType!==null&&expected.resourceType!==r.resourceType) || (expected.status!==null&&!this.http.some(h=>h.url===r.url&&h.method===r.method&&h.status===expected.status));});
  assert.deepEqual({http:badHttp,console:badConsole,network:badNetwork},{http:[],console:[],network:[]},'unexpected provider/browser failures');
 }
}
module.exports={trustedActivity,Diagnostics};
