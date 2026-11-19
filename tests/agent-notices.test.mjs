import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { createRequire } from 'node:module';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { apply, watchContinued } from '../integrations/agent-notices/bridge/index.mjs';
import bridge from '../integrations/agent-notices/bridge/notify.cjs';
import channel from '../integrations/agent-notices/bridge/channel.cjs';

function fixture(t) {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'daozai-agent-'));
  t.after(()=>fs.rmSync(root,{recursive:true,force:true}));
  const kit=path.join(root,'kit with spaces'), dir=path.join(root,'profile');
  fs.cpSync(fileURLToPath(new URL('../integrations/agent-notices/',import.meta.url)),kit,{recursive:true});
  fs.mkdirSync(dir);
  fs.writeFileSync(path.join(kit,'bridge','target.json'),JSON.stringify(process.execPath));
  const run=(p,a)=>spawnSync(process.execPath,[path.join(kit,'connect.mjs'),p,a,dir],{encoding:'utf8'});
  return {kit,dir,run};
}
const desktopHost=process.platform==='win32' && process.env.LOCALAPPDATA &&
  path.join(process.env.LOCALAPPDATA,'Programs','DeepSeek Harness','DeepSeek Harness.exe');
test('installed DSH host: profile, mounted channel, audited approvals, foreground and continued answers',
  {skip:!desktopHost || !fs.existsSync(desktopHost)},t=>{
    const {dir,run}=fixture(t);
    fs.writeFileSync(path.join(dir,'package.json'),'{}');
    assert.equal(run('dsh','install').status,0);
    const result=spawnSync(desktopHost,[fileURLToPath(new URL('./fixtures/dsh-host.cjs',import.meta.url)),dir,process.execPath],
      {env:{...process.env,ELECTRON_RUN_AS_NODE:'1',DSH_HOME:dir},cwd:dir,windowsHide:true,encoding:'utf8',timeout:25000});
    assert.equal(result.status,0,[result.error,result.stdout,result.stderr].filter(Boolean).join('\n'));
  });
test('Codex merges only owned groups; repeated install unchanged; remove preserves other hooks', t=>{
  const {dir,run}=fixture(t), file=path.join(dir,'hooks.json');
  const old={hooks:{Stop:[{hooks:[{type:'command',command:'another-hook'}]}]},other:true};
  fs.writeFileSync(file,JSON.stringify(old));
  assert.equal(run('codex','install').status,0);
  const config=JSON.parse(fs.readFileSync(file));assert.equal(config.hooks.Stop.length,2);
  assert.equal(config.hooks.PreToolUse[0].matcher,'^request_user_input$');
  assert.equal(config.hooks.PermissionRequest[0].hooks[0].timeout,21605);
  assert.equal(run('codex','install').status,0);
  assert.equal(fs.readdirSync(dir).filter(x=>x.endsWith('.bak')).length,1);
  assert.equal(run('codex','remove').status,0);
  assert.deepEqual(JSON.parse(fs.readFileSync(file)),old);
});
test('DSH bundle resolves through the profile, removes its junction but not the actual kit',t=>{
  const {dir,kit,run}=fixture(t), file=path.join(dir,'package.json');
  const old={dependencies:{example:'1'},dsh:{profile:{bundles:['example'],other:true}},extra:{}};
  fs.writeFileSync(file,JSON.stringify(old));
  const name='@daozai/agent-notices', link=path.join(dir,'node_modules',name);
  assert.equal(run('dsh','install').status,0);
  assert.deepEqual(JSON.parse(fs.readFileSync(file)).dsh.profile.bundles,['example',name]);
  assert.equal(fs.realpathSync(link),fs.realpathSync(path.join(kit,'bridge')));
  assert.equal(run('dsh','install').status,0);
  assert.equal(fs.readdirSync(dir).filter(x=>x.endsWith('.bak')).length,1);
  assert.equal(run('dsh','remove').status,0);assert.ok(!fs.existsSync(link));
  assert.ok(fs.existsSync(path.join(kit,'bridge','index.mjs')));
  assert.deepEqual(JSON.parse(fs.readFileSync(file)),old);
});
test('invalid configuration and a foreign package are preserved',t=>{
  const {dir,run}=fixture(t), hooks=path.join(dir,'hooks.json');
  fs.writeFileSync(hooks,'{"hooks":{"Stop":false}}');
  assert.notEqual(run('codex','install').status,0);
  assert.equal(fs.readFileSync(hooks,'utf8'),'{"hooks":{"Stop":false}}');
  const pkg=path.join(dir,'package.json');fs.writeFileSync(pkg,'{}');
  fs.mkdirSync(path.join(dir,'node_modules','@daozai','agent-notices'),{recursive:true});
  assert.notEqual(run('dsh','install').status,0);assert.equal(fs.readFileSync(pkg,'utf8'),'{}');
});
test('DSH with no live island hands back once and emits only a fixed status', async t=>{
  const seen=[], handlers={};
  t.mock.method(bridge,'notify',(...args)=>seen.push(args));
  apply({on:(name,fn)=>{handlers[name]=fn;}});
  t.mock.method(channel,'exchange',async()=>null);
  await handlers['approval/request']({},()=>{});
  handlers['session/event']({},{type:'turn/end',data:{}});
  const answer={answers:['private answer']};let calls=0;
  assert.equal(await handlers['user-questions/request']({questions:['private question']},()=>{calls++;return answer;}),answer);
  assert.equal(calls,1);
  assert.deepEqual(seen,[['dsh','approval'],['dsh','finished'],['dsh','question']]);
});

test('DSH one-time decisions use matching full operation; validates answers and never grants on fallback',async t=>{
  const handlers={},session={id:'fixture',header:{cwd:'example'}},request={callId:'c1',toolName:'Read',agent:{session}};
  apply({on:(k,h)=>{handlers[k]=h;}});
  let decision={decision:'allow'}, seen;
  t.mock.method(channel,'enabled',()=>true);
  t.mock.method(channel,'exchange',async(_p,_k,build)=>{seen=build(0);return decision;});
  t.mock.method(bridge,'notify',()=>{});
  handlers['session/event'](session,{type:'tool/call',data:{name:'Read',callId:'c1',arguments:{path:'test.txt'}}});
  let native=0; const next=()=>{native++;return 'native';};
  assert.equal(await handlers['approval/request'](request,next),'allowed-once');assert.match(seen.operation,/test\.txt/);
  assert.equal(seen.sessionId,'fixture');assert.equal(seen.cwd,'example');
  assert.equal(await handlers['approval/request']({...request,callId:'other'},next),'native');
  decision={decision:'deny'};assert.equal(await handlers['approval/request'](request,next),'rejected');
  decision={decision:'return'};assert.equal(await handlers['approval/request'](request,next),'native');
  const questions=[{id:'1',question:'Choose',multiSelect:true,options:[{label:'A'},{label:'B'}]}];
  const answers=[{id:'1',selected:['A','B'],custom:''}];decision={decision:'answer',answers};
  assert.deepEqual(await handlers['user-questions/request']({questions,agent:{session}},next),{answers});
  assert.equal(seen.sessionId,'fixture');assert.equal(seen.questions.length,1);
  decision.answers=[{id:'1',selected:['invented']}];
  assert.equal(await handlers['user-questions/request']({questions,agent:{session}},next),'native');
  assert.equal(native,3);
  // Native-scope authority and the completion event carry the same identity.
  const intent=[{id:'i',question:'Allow network?',detail:'because',intent:{kind:'approval',reject:'No'},
    options:[{label:'Yes'},{label:'No'}]}];decision={decision:'deny'};
  assert.deepEqual(await handlers['user-questions/request']({questions:intent,agent:{session}},next),
    {answers:[{id:'i',selected:['No']}]});
  assert.equal(seen.sessionId,'fixture');assert.equal(seen.nativeScope,true);assert.equal(seen.tool,'Allow network?');
  handlers['session/event'](session,{type:'turn/end',data:{}});
  assert.deepEqual(seen,{text:'',sessionId:'fixture'});assert.equal(native,3);
});

test('DSH continued reply reaches only the still-live native projection, never the old tool call',async t=>{
  t.mock.method(channel,'enabled',()=>true);
  const roots=[{session:{id:'fixture'}}], q={state:'continued',callId:'q1',questions:[{id:'a',question:'Choose'}]};
  let active=[q],finish,applied=[];
  t.mock.method(channel,'exchange',()=>new Promise(done=>{finish=done;}));
  const ctx={get:key=>({agents:{roots:()=>roots},sessionProjections:{stateOf:()=>({questions:{active}})},
    userQuestions:{answer:(_agent,id,a)=>applied.push([id,a])}})[key]};
  const stop=watchContinued(ctx);t.after(stop);stop.scan();
  const answer={decision:'answer',answers:[{id:'a',selected:[],custom:'ok'}]};
  active=[];finish(answer);await delay(0);assert.deepEqual(applied,[]);
  stop.scan();active=[q];stop.scan();finish(answer);await delay(0);stop.scan();
  assert.deepEqual(applied,[['q1',{answers:answer.answers}]]);
});

test('live broker isolates concurrent requests, cancellation, completion privacy and Codex decisions', {timeout:12000},async t=>{
  const {kit}=fixture(t), dir=path.join(kit,'bridge'),script=path.join(dir,'channel.cjs');
  const srv=spawn(process.execPath,[script],{stdio:['pipe','pipe','pipe']});
  const closed=once(srv,'close');
  try {
    const queue=createInterface({input:srv.stdout})[Symbol.asyncIterator]();
    let n=0;
    const event=async()=>{
      let timer;const step=++n;
      try {return JSON.parse((await Promise.race([queue.next(),new Promise((_,fail)=>{
        timer=setTimeout(()=>fail(Error(`No broker event at step ${step}`)),2200);
      })])).value);} finally {clearTimeout(timer);}
    };
    const send=msg=>srv.stdin.write(JSON.stringify(msg)+'\n');
    send({policy:{dsh:true,codex:true,interactive:true,completion:true,content:0}});
    const client=createRequire(import.meta.url)(script);
    for(let i=0;i<100 && !fs.existsSync(path.join(dir,'live.json'));i++)await delay(15);
    assert.ok(fs.existsSync(path.join(dir,'live.json')));
    const controller=new AbortController();
    const a=client.exchange('dsh','approval',()=>({tool:'read'}),controller.signal);
    const first=await event();
    const b=client.exchange('codex','approval',()=>({tool:'write'}));
    const second=await event();assert.notEqual(first.id,second.id);
    send({id:second.id,reply:{decision:'deny'}});assert.deepEqual(await b,{decision:'deny'});
    controller.abort();assert.equal(await a,null);
    const cancels=[await event(),await event()].map(m=>m.cancel);assert.ok(cancels.includes(first.id));
    const finish=client.exchange('dsh','finished',mode=>({text:mode?'sensitive':''}));
    assert.deepEqual((await event()).data,{text:''});assert.deepEqual(await finish,{done:true});
    for (const choice of ['return','allow','deny']) {
      const hook=spawn(process.execPath,[path.join(dir,'notify.cjs'),'approval'],{stdio:['pipe','pipe','pipe']});
      let out='';hook.stdout.on('data',s=>out+=s);const exit=once(hook,'close');
      hook.stdin.end(JSON.stringify({hook_event_name:'PermissionRequest',session_id:'codex-approval',
        tool_name:'Read',tool_input:{file:'fixture'}}));
      const req=await event();assert.equal(req.data.sessionId,'codex-approval');
      send({id:req.id,reply:{decision:choice}});assert.equal((await exit)[0],0);
      if (choice==='return') assert.equal(out,'');else assert.equal(JSON.parse(out).hookSpecificOutput.decision.behavior,choice);
      assert.equal((await event()).cancel,req.id);
    }
    // Codex also reports the thread under thread_id, and the Stop hook keeps its text private.
    const stop=spawn(process.execPath,[path.join(dir,'notify.cjs'),'finished'],{stdio:['pipe','pipe','pipe']});
    const stopped=once(stop,'close');
    stop.stdin.end(JSON.stringify({hook_event_name:'Stop',thread_id:'codex-thread',last_assistant_message:'private'}));
    assert.deepEqual((await event()).data,{text:'',sessionId:'codex-thread'});assert.equal((await stopped)[0],0);
    const pending=client.exchange('dsh','question',()=>({questions:[]}));await event();srv.stdin.end();
    assert.equal(await pending,null);
  } finally {srv.kill();await closed;}
});
test('Codex session identity prefers session_id, falls back to thread_id and sessionId, capped at 500',()=>{
  assert.equal(bridge.sessionOf({session_id:'a',thread_id:'b',sessionId:'c'}),'a');
  assert.equal(bridge.sessionOf({session_id:'',thread_id:'b',sessionId:'c'}),'b');
  assert.equal(bridge.sessionOf({thread_id:null,sessionId:'c'}),'c');
  assert.equal(bridge.sessionOf({session_id:42,thread_id:'b'}),'b');
  assert.equal(bridge.sessionOf({sessionId:'x'.repeat(600)}).length,500);
  assert.equal(bridge.sessionOf({}),'');assert.equal(bridge.sessionOf(null),'');assert.equal(bridge.sessionOf('raw'),'');
});
test('Codex hook fails open without a target and never outputs a decision', t=>{
  const {kit}=fixture(t), script=path.join(kit,'bridge','notify.cjs');
  fs.unlinkSync(path.join(kit,'bridge','target.json'));
  const result=spawnSync(process.execPath,[script,'approval'],{input:'{"prompt":"private"}',encoding:'utf8',timeout:2000});
  assert.equal(result.status,0);assert.equal(result.stdout,'');assert.equal(result.stderr,'');
});

test('automatic Codex setup migrates its old entry and the command runs with spaces and no node on PATH',
  {skip:process.platform!=='win32'}, t=>{
  const {kit,dir,run}=fixture(t),file=path.join(dir,'hooks.json'),script=path.join(kit,'bridge','notify.cjs');
  const legacy=`node "${script.replaceAll('\\','/')}" finished`;
  fs.writeFileSync(file,JSON.stringify({hooks:{Stop:[{hooks:[{type:'command',command:legacy,statusMessage:'Daozai'}]}]}}));
  assert.equal(run('codex','install').status,0);
  const groups=JSON.parse(fs.readFileSync(file)).hooks.Stop;assert.equal(groups.length,1);
  const marker=path.join(dir,'signal');
  fs.writeFileSync(script,"require('fs').writeFileSync(process.env.DAOZAI_MARKER,process.argv[2])");
  const shell=path.join(process.env.SystemRoot,'System32','cmd.exe');
  const env={...process.env,PATH:path.dirname(shell),DAOZAI_MARKER:marker};
  const execute=spawnSync(groups[0].hooks[0].command,{shell,env,encoding:'utf8'});
  assert.equal(execute.status,0,execute.stderr);
  assert.equal(fs.readFileSync(marker,'utf8'),'finished');
});
