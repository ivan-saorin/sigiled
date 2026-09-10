const vscode = require('vscode');
const http = require('http');
exports.activate = context => {
  const token = process.env.SIGIL_IDE_ACTIVITY_TOKEN;
  const generation = process.env.SIGIL_IDE_GENERATION;
  if (!token || !/^\d+$/.test(generation || '')) return;
  let last = 0;
  let interaction = 0;
  const report = event => {
    if (!event.startsWith('command_') && Date.now() - last < 3000) return;
    last = Date.now();
    const body = JSON.stringify({event,generation});
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
  if(vscode.window.onDidStartTerminalShellExecution)context.subscriptions.push(vscode.window.onDidStartTerminalShellExecution(()=>report('command_start')),vscode.window.onDidEndTerminalShellExecution(()=>report('command_end')));
};
