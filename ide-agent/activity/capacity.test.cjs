const assert = require('node:assert/strict');
const test = require('node:test');
const vm = require('node:vm');
const fs = require('node:fs');
function fixture(initial = []) {
  const callbacks = {}, maps = [], sets = [];
  const on = name => callback => {callbacks[name]=callback;return {dispose(){}};};
  const window={terminals:initial,onDidOpenTerminal:on('open'),onDidCloseTerminal:on('close'),onDidChangeTerminalShellIntegration:on('integration'),onDidChangeTextEditorSelection:on('selection'),onDidStartTerminalShellExecution:on('start'),onDidEndTerminalShellExecution:on('end')};
  const state={window,callbacks,maps,sets,last:null,maxBytes:0,tick:null};
  class TrackedMap extends Map {constructor(...args){super(...args);maps.push(this);}}
  class TrackedSet extends Set {constructor(...args){super(...args);sets.push(this);}}
  const vscode={window,workspace:{onDidChangeTextDocument:on('edit'),onWillSaveTextDocument:on('save')},TextEditorSelectionChangeKind:{},TextDocumentSaveReason:{}};
  const context={exports:{},process:{env:{SIGIL_IDE_ACTIVITY_TOKEN:'fixture',SIGIL_IDE_GENERATION:'1'}},Buffer,Date,Map:TrackedMap,Set:TrackedSet,setInterval(f){state.tick=f;return 1;},clearInterval(){},require(name){if(name==='vscode')return vscode;if(name==='crypto')return {randomUUID:()=> 'test-instance'};if(name==='http')return {request(){return {on(){},end(body){state.maxBytes=Math.max(state.maxBytes,Buffer.byteLength(body));state.last=JSON.parse(body);}};}};throw Error(name);}};
  vm.runInNewContext(fs.readFileSync(require.resolve('./extension.js'),'utf8'),context);
  context.exports.activate({subscriptions:[]});
  return state;
}
function overflow(f) {
  assert.equal(f.last.observation.terminals.length,1);
  assert.equal(f.last.observation.terminals[0].id,'observation-overflow');
  assert.equal(f.last.observation.terminals[0].integrated,false,'legacy observer sees unsupported, never idle');
  assert.equal(f.last.observation.executions.length,0);
  assert.equal(f.last.observation.event,undefined,'overflow observation is not human activity');
  assert(Buffer.byteLength(JSON.stringify(f.last))<512,'overflow wire stays bounded');
}
test('exact terminal capacity, repeated overflow and conservative complete recovery',()=>{
  const f=fixture();f.window.terminals=Array.from({length:256},()=>({shellIntegration:{}}));f.callbacks.open();
  assert.equal(f.last.observation.terminals.length,256);
  assert(f.last.observation.terminals.every(t=>t.integrated));
  f.window.terminals.push({shellIntegration:{}});f.callbacks.open();overflow(f);
  for(let n=0;n<500;n++){f.window.terminals[256]={shellIntegration:{}};f.callbacks.start({terminal:f.window.terminals[256],execution:{}});f.tick();overflow(f);}
  assert(f.maps.every(m=>m.size<=4096));assert(f.sets.every(s=>s.size<=256));
  f.window.terminals.pop();f.callbacks.close({});
  assert.equal(f.last.observation.terminals.length,256);
  assert(f.last.observation.terminals.every(t=>!t.integrated),'capacity return cannot invent idle for unseen executions');
  for(const t of [...f.window.terminals]){f.window.terminals=f.window.terminals.filter(x=>x!==t);f.callbacks.close(t);}
  assert.equal(f.last.observation.terminals.length,0);assert.equal(f.last.observation.executions.length,0);
});
test('exact execution capacity, lost ends and repeated overflow bound strong retention',()=>{
  const f=fixture(),t={shellIntegration:{}};f.window.terminals=[t];f.callbacks.open();
  const known=[];for(let n=0;n<4096;n++){const execution={};known.push(execution);f.callbacks.start({terminal:t,execution});}
  assert.equal(f.last.observation.executions.length,4096);
  for(let n=0;n<500;n++){f.callbacks.start({terminal:t,execution:{}});f.tick();overflow(f);assert(f.maps.every(m=>m.size<=4096));assert(f.sets.every(s=>s.size<=256));}
  for(const execution of known)f.callbacks.end({terminal:t,execution});
  overflow(f);f.callbacks.end({terminal:t,execution:{}});overflow(f);
  f.window.terminals=[];f.callbacks.close(t);assert.equal(f.last.observation.terminals.length,0);assert.equal(f.last.observation.executions.length,0);
  assert(f.maxBytes<400000,'complete snapshot wire has a finite bound');
  console.log(JSON.stringify({executionLimit:4096,retained:f.maps.map(m=>m.size),maxWireBytes:f.maxBytes}));
});
test('startup overflow does not strongly retain all existing terminals',()=>{
  const f=fixture(Array.from({length:10000},()=>({shellIntegration:{}})));overflow(f);
  assert(f.sets.every(s=>s.size<=256));assert(f.maps.every(m=>m.size<=4096));
  f.window.terminals=[];f.tick();assert.equal(f.last.observation.terminals.length,0);
});

test('overflow recovery identities stay bounded under missing close events',()=>{
  const f=fixture(),t={shellIntegration:{}};f.window.terminals=[t];
  for(let n=0;n<4096;n++)f.callbacks.start({terminal:t,execution:{}});
  for(let n=0;n<1000;n++){const replacement={shellIntegration:{}};f.window.terminals=[replacement];f.callbacks.start({terminal:replacement,execution:{}});overflow(f);assert(f.maps.every(m=>m.size<=4096));assert(f.sets.every(s=>s.size<=256));}
  for(const s of f.sets)for(const t of [...s])f.callbacks.close(t);
  f.window.terminals=[];f.tick();overflow(f);
  console.log('PASS exhausted recovery identities require custody recovery, never silent idle');
});

test('Bash plus virtual-terminal closure preserves uncertainty until execution evidence',()=>{
  const f=fixture(),bash={shellIntegration:{}};f.window.terminals=[bash];f.callbacks.open();
  const virtual=[];for(let n=0;n<257;n++){const t={};virtual.push(t);f.window.terminals.push(t);f.callbacks.open(t);}overflow(f);
  for(const t of virtual){f.window.terminals=f.window.terminals.filter(x=>x!==t);f.callbacks.close(t);}
  assert.equal(f.last.observation.terminals.length,1);assert.equal(f.last.observation.terminals[0].integrated,false);
  const execution={};f.callbacks.start({terminal:bash,execution});assert.equal(f.last.observation.executions.length,1);
  f.callbacks.end({terminal:bash,execution});assert.equal(f.last.observation.terminals[0].integrated,true);assert.equal(f.last.observation.executions.length,0);
  console.log('PASS controlled close-event transcript: incomplete -> unsupported -> observed execution -> ready');
});

test('exact recovery-set capacity does not latch on repeated known overflow',()=>{
  const f=fixture(),t={shellIntegration:{}};f.window.terminals=[t];for(let n=0;n<4096;n++)f.callbacks.start({terminal:t,execution:{}});
  const affected=Array.from({length:256},()=>({shellIntegration:{}}));f.window.terminals=affected;
  for(const terminal of affected)f.callbacks.start({terminal,execution:{}});
  for(let n=0;n<500;n++){f.callbacks.start({terminal:affected[0],execution:{}});overflow(f);}
  for(const terminal of affected)f.callbacks.close(terminal);
  f.callbacks.close(t);f.window.terminals=[];f.tick();assert.equal(f.last.observation.terminals.length,0);assert.equal(f.last.observation.executions.length,0);
});
if(process.env.SIGIL_OVERFLOW_EXPORT){const f=fixture();f.window.terminals=Array.from({length:257},()=>({}));f.callbacks.open();overflow(f);fs.writeFileSync(process.env.SIGIL_OVERFLOW_EXPORT,JSON.stringify(f.last));}
