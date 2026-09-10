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
 window:{onDidChangeTextEditorSelection:on('selection'),onDidStartTerminalShellExecution:on('start'),onDidEndTerminalShellExecution:on('end')},
 workspace:{onDidChangeTextDocument:on('edit'),onWillSaveTextDocument:on('save')}
};
const load = Module._load;
Module._load = function(name,...args) {
 if(name==='vscode') return vscode;
 if(name==='http') return {request(options){assert.equal(options.hostname,'127.0.0.1');return {on(){},end(body){sent.push(JSON.parse(body));}};}};
 return load.call(this,name,...args);
};
require('./extension.js').activate({subscriptions:[]});
callbacks.edit({contentChanges:[{}]}); callbacks.save({reason:2}); callbacks.save({reason:3}); callbacks.selection({kind:3});
assert.equal(sent.length,0,'background editing, automatic saves and command selection cannot extend lease');
callbacks.selection({kind:1}); assert.equal(sent.at(-1).event,'selection');
now += 6000; callbacks.edit({contentChanges:[{}]}); assert.equal(sent.length,1,'background edits cannot renew the interaction window');
callbacks.save({reason:1}); assert.equal(sent.at(-1).event,'save');
callbacks.start({});callbacks.end({});assert.deepEqual(sent.slice(-2).map(e=>e.event),['command_start','command_end']);
assert(sent.every(e=>e.generation==='18446744073709551615'));
console.log('activity extension: background isolation and full generation passed');
