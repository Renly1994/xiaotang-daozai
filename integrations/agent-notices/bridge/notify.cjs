'use strict';
const { spawn } = require('node:child_process');
const { isAbsolute } = require('node:path');

function notify(provider, status) {
  if (!['codex', 'dsh'].includes(provider) || !['finished', 'approval', 'question'].includes(status)) return;
  try {
    const exe = require('./target.json');
    if (typeof exe !== 'string' || !isAbsolute(exe)) return;
    const child = spawn(exe, ['--agent-notice', provider, status], { stdio: 'ignore', windowsHide: true });
    child.on('error', () => {});
    child.unref();
  } catch { /* Optional notification: never interrupt the host. */ }
}

module.exports = { notify };
// Codex names the conversation differently per hook family; an unknown id is empty,
// never a fabricated string, so the island only offers a jump it can actually do.
function sessionOf(event) {
  for (const key of ['session_id','thread_id','sessionId']) {
    const value = event ? event[key] : undefined;
    if (typeof value === 'string' && value) return value.slice(0,500);
  }
  return '';
}
async function run(status) {
  if (status==='question') return notify('codex',status); // PreToolUse cannot answer a native Codex question.
  let raw='';
  process.stdin.setEncoding('utf8');
  for await (const chunk of process.stdin) {
    raw+=chunk;
    if (raw.length>1024*1024) return;
  }
  let event;
  try { event=JSON.parse(raw); } catch { return; }
  const channel=require('./channel.cjs');
  if (status==='finished' && event.hook_event_name==='Stop') {
    const done=await channel.exchange('codex',status,mode=>({text:typeof event.last_assistant_message==='string' ?
      event.last_assistant_message.slice(0,mode===2?32000:mode===1?1200:0):'',sessionId:sessionOf(event)}));
    if (!done?.done) notify('codex',status);
  } else if (status==='approval' && event.hook_event_name==='PermissionRequest') {
    const operation=event.tool_input && typeof event.tool_input==='object' ? JSON.stringify(event.tool_input,null,2) : '';
    const reply=await channel.exchange('codex',status,()=>({tool:event.tool_name||'',cwd:event.cwd||'',
      operation:operation.length<=24000?operation:'',reason:'',sessionId:sessionOf(event)}));
    if (!reply) return notify('codex',status);
    if ((reply.decision==='allow' && operation && operation.length<=24000) || reply.decision==='deny') {
      const decision={behavior:reply.decision};
      if (reply.decision==='deny') decision.message='用户在岛仔中拒绝了此次请求';
      process.stdout.write(JSON.stringify({hookSpecificOutput:{hookEventName:'PermissionRequest',decision}})+'\n');
    }
  }
}
module.exports.run=run;
module.exports.sessionOf=sessionOf;
if (require.main === module) run(process.argv[2]).catch(()=>{});
