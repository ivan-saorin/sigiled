const assert = require('node:assert/strict');
const Module = require('node:module');
const callbacks = {};
const sent = [];
let now = 10000;
Date.now = () => now;
process.env.SIGIL_IDE_ACTIVITY_TOKEN = 'fixture-activity-credential';
process.env.SIGIL_IDE_GENERATION = '18446744073709551615';
const on = name => callback => { callbacks[name] = callback; return {dispose(){}}; };
const vscode = {
 TextEditorSelectionChangeKind:{Keyboard:1,Mouse:2,Command:3}, TextDocumentSaveReason:{Manual:1,AfterDelay:2,FocusOut:3},
 window:{terminals:[],onDidOpenTerminal:on('open'),onDidCloseTerminal:on('close'),onDidChangeTerminalShellIntegration:on('integrated'),onDidChangeTextEditorSelection:on('selection'),onDidStartTerminalShellExecution:on('start'),onDidEndTerminalShellExecution:on('end')},
 workspace:{onDidChangeTextDocument:on('edit'),onWillSaveTextDocument:on('save')}
};
const load = Module._load;
Module._load = function(name,...args) {
 if(name==='vscode') return vscode;
 if(name==='http') return {request(options){assert.equal(options.hostname,'127.0.0.1');return {on(){},end(body){sent.push(JSON.parse(body));}};}};
 return load.call(this,name,...args);
};
const subscriptions=[];require('./extension.js').activate({subscriptions});
const initial=sent.pop();assert.equal(initial.event,'observation');assert.equal(initial.observation.terminals.length,0);
callbacks.edit({contentChanges:[{}]}); callbacks.save({reason:2}); callbacks.save({reason:3}); callbacks.selection({kind:3});
assert.equal(sent.length,0,'background editing, automatic saves and command selection cannot extend lease');
callbacks.selection({kind:1}); assert.equal(sent.at(-1).event,'selection');
now += 6000; callbacks.edit({contentChanges:[{}]}); assert.equal(sent.length,1,'background edits cannot renew the interaction window');
callbacks.save({reason:1}); assert.equal(sent.at(-1).event,'save');
const executionA = {}, executionB = {}, missingStart = {}, executingTerminal={shellIntegration:{}};vscode.window.terminals=[executingTerminal];
callbacks.start({execution:executionA,terminal:executingTerminal}); callbacks.start({execution:executionB,terminal:executingTerminal});
callbacks.end({execution:missingStart}); callbacks.end({execution:executionA}); callbacks.end({execution:executionA}); callbacks.end({execution:executionB});

const events=sent.slice(-6).map(e=>e.observation);
assert.equal(events[0].executions.length,1);assert.equal(events[1].executions.length,2);assert.equal(events[2].executions.length,2);assert.equal(events[3].executions.length,1);assert.equal(events[4].executions.length,1);assert.equal(events[5].executions.length,0);
const terminal={};vscode.window.terminals=[terminal];callbacks.open(terminal);const id=sent.at(-1).observation.terminals[0].id;assert.equal(sent.at(-1).observation.terminals[0].integrated,false);
terminal.shellIntegration={};callbacks.integrated({terminal});assert.equal(sent.at(-1).observation.terminals[0].id,id);assert.equal(sent.at(-1).observation.terminals[0].integrated,true);
vscode.window.terminals=[];callbacks.close(terminal);callbacks.integrated({terminal});assert.equal(sent.at(-1).observation.terminals.length,0,'late event cannot reopen closed terminal');callbacks.start({execution:{},terminal});assert.equal(sent.at(-1).observation.executions.length,0,'late command cannot resurrect closed terminal');
assert(sent.every(e=>e.generation==='18446744073709551615'));
for(const s of subscriptions)s.dispose();
const preexisting={shellIntegration:{}};vscode.window.terminals=[preexisting];const lateSubscriptions=[];require('./extension.js').activate({subscriptions:lateSubscriptions});
assert.equal(sent.at(-1).observation.terminals[0].integrated,false,'late activation cannot infer idle for existing integrated terminal');callbacks.integrated({terminal:preexisting});assert.equal(sent.at(-1).observation.terminals[0].integrated,false,'passive integration telemetry does not resolve preexisting work');callbacks.end({execution:{},terminal:preexisting});assert.equal(sent.at(-1).observation.terminals[0].integrated,true,'actual command end establishes current supported idle');for(const s of lateSubscriptions)s.dispose();
console.log('PASS bounded terminal snapshots, stable identities, closure/late events, execution snapshots and passive activity isolation');
