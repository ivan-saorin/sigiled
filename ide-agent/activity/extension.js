const vscode = require('vscode');
const http = require('http');
const {randomUUID} = require('crypto');
exports.activate = context => {
  const token = process.env.SIGIL_IDE_ACTIVITY_TOKEN;
  const generation = process.env.SIGIL_IDE_GENERATION;
  if (!token || !/^\d+$/.test(generation || '')) return;
  let last = 0;
  let interaction = 0;
  const instance = randomUUID();
  const executions = new WeakMap();
  const terminals = new WeakMap();
  const active = new Map();
  // Existing terminals may already be executing before this host subscribed.
  const unobserved = new Set(vscode.window.terminals || []);
  let terminalSequence = 0, observationSequence = 0;
  const terminalId = terminal => {if(!terminals.has(terminal))terminals.set(terminal,instance+':terminal:'+(++terminalSequence));return terminals.get(terminal);};
  let sequence = 0;
  const executionId = execution => {
    if (!execution || typeof execution !== 'object') return undefined;
    if (!executions.has(execution)) executions.set(execution, instance + ':' + (++sequence));
    return executions.get(execution);
  };
  const report = (event, execution_id) => {
    if (!event.startsWith('command_') && Date.now() - last < 3000) return;
    last = Date.now();
    const body = JSON.stringify({event,generation,execution_id});
    const req = http.request({hostname:'127.0.0.1',port:8090,path:'/activity',method:'POST',headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/json','Content-Length':Buffer.byteLength(body)},timeout:2000},res=>res.resume());
    req.on('error',()=>{}); req.on('timeout',()=>req.destroy()); req.end(body);
  };
  context.subscriptions.push(
    vscode.window.onDidChangeTextEditorSelection(e => {
      if (e.kind === vscode.TextEditorSelectionChangeKind.Keyboard || e.kind === vscode.TextEditorSelectionChangeKind.Mouse) {
        interaction = Date.now(); report('selection');
      }
    }),
    vscode.workspace.onDidChangeTextDocument(e => {
      // VS Code does not expose a universal "human edit" event. Restrict
      // document changes to a short window after observed keyboard/mouse use;
      // extension-only changes cannot continually renew this window.
      if (e.contentChanges.length && Date.now() - interaction < 5000) report('edit');
    }),
    vscode.workspace.onWillSaveTextDocument(e => {
      if (e.reason === vscode.TextDocumentSaveReason.Manual) report('save');
    })
  );

  const observe = event => {
    const supported = vscode.window.onDidStartTerminalShellExecution && vscode.window.onDidEndTerminalShellExecution && vscode.window.onDidChangeTerminalShellIntegration;
    const current = vscode.window.terminals || [];
    const observation = {observer:instance,sequence:String(++observationSequence),terminals: supported ? current.map(t=>({id:terminalId(t),integrated:!!t.shellIntegration&&!unobserved.has(t)})) : [{id:'api-unavailable',integrated:false}],executions:[...active.values()].map(v=>v.id),event};
    const body=JSON.stringify({event:'observation',generation,observation});
    const req=http.request({hostname:'127.0.0.1',port:8090,path:'/activity',method:'POST',headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/json','Content-Length':Buffer.byteLength(body)},timeout:2000},res=>res.resume());
    req.on('error',()=>{});req.on('timeout',()=>req.destroy());req.end(body);
  };
  if(vscode.window.onDidStartTerminalShellExecution)context.subscriptions.push(
    vscode.window.onDidStartTerminalShellExecution(e=>{if(!(vscode.window.terminals||[]).includes(e.terminal)||active.has(e.execution)){observe();return;}unobserved.delete(e.terminal);active.set(e.execution,{id:executionId(e.execution),terminal:e.terminal});observe('command_start');}),
    vscode.window.onDidEndTerminalShellExecution(e=>{const ended=active.delete(e.execution);const resolved=unobserved.delete(e.terminal);observe(ended||resolved?'command_end':undefined);})
  );
  if(vscode.window.onDidOpenTerminal)context.subscriptions.push(vscode.window.onDidOpenTerminal(()=>observe()));
  if(vscode.window.onDidCloseTerminal)context.subscriptions.push(vscode.window.onDidCloseTerminal(t=>{unobserved.delete(t);for(const [key,v] of active)if(v.terminal===t)active.delete(key);observe();}));
  if(vscode.window.onDidChangeTerminalShellIntegration)context.subscriptions.push(vscode.window.onDidChangeTerminalShellIntegration(()=>observe()));
  observe();
  const timer=setInterval(()=>observe(),2000);
  context.subscriptions.push({dispose(){clearInterval(timer);}});
};
