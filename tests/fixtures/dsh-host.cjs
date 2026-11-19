'use strict';
// Run by the installed DSH Electron binary in Node mode, against a temporary profile.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const { spawn } = require('node:child_process');
const { once } = require('node:events');
const { createInterface } = require('node:readline');
const { setTimeout: delay } = require('node:timers/promises');

async function main() {
  const [profile, node] = process.argv.slice(2);
  const anchor = path.join(path.dirname(process.execPath), 'resources/app.asar/dsh');
  const host = name => require(path.join(anchor, 'node_modules/@deepseek-ai', name));
  const load = createRequire(path.join(profile, 'package.json'));
  const layer = host('dsh-app-boot').loadProfileDirectory('dsh', profile, anchor);
  assert.deepEqual(layer.skippedBundles, []);
  const [{name}] = layer.layers[0].patches[0].insert;
  const channel = path.join(path.dirname(load.resolve(name)), 'channel.cjs');
  const broker = spawn(node, [channel], {stdio:['pipe','pipe','inherit'], windowsHide:true});
  const closed = once(broker,'close');
  const queue = createInterface({input:broker.stdout})[Symbol.asyncIterator]();
  const next = async label => {
    let timer;
    try {
      const line = await Promise.race([queue.next(),new Promise((_,reject)=>{
        timer=setTimeout(()=>reject(Error(`No broker event: ${label}`)),5500);
      })]);
      assert.ok(!line.done,label);
      return JSON.parse(line.value);
    } finally {clearTimeout(timer);}
  };
  const send = msg => broker.stdin.write(JSON.stringify(msg)+'\n');
  const reply = (id,decision,extra) => send({id,reply:{decision,...extra}});
  const disposed = [];
  try {
    send({policy:{dsh:true,interactive:true,completion:true,deferred:true,content:0}});
    const descriptor = path.join(path.dirname(channel),'live.json');
    for(let i=0;i<100&&!fs.existsSync(descriptor);i++) await delay(20);
    const ctx = new (host('cordis').Context)();
    const mount = async plugin => {
      const fiber = ctx.plugin(plugin);
      disposed.unshift(fiber);
      await fiber.await();
    };
    for(const n of ['dsh-session','dsh-session-projection','dsh-agent','dsh-user-approval','dsh-user-questions'])
      await mount(host(n).default);
    const session = ctx.get('sessions').create('fixture',{meta:{cwd:profile}});
    const queued = [];
    const agent = {id:session.id,session,inbox:{nextTurn:[],nextStep:[]},steer:m=>queued.push(m)};
    ctx.get('agents').enter(agent);
    const approval = ctx.get('approval'), service = ctx.get('userQuestions');
    let handbacks = 0;
    ctx.on('approval/request',()=>{handbacks++;return 'rejected';});
    await mount(load(name));
    assert.deepEqual(await next('mounted'),{dshReady:true});
    session.append('turn/start',{turn:1});
    const request = {agent,callId:'fixture-call',toolName:'fixture_only'};
    session.append('tool/call',{callId:request.callId,name:request.toolName,arguments:'{"fixture":true}'});
    for(const [action,expected] of [['deny','rejected'],['return','rejected'],['allow','allowed-once']]) {
      const decision = approval.request(request);
      const card = await next('approval');
      assert.equal(card.kind,'approval');
      assert.equal(card.data.operation,'{"fixture":true}');
      if(action==='deny') await delay(4700);
      reply(card.id,action);
      assert.equal(await decision,expected);
      assert.equal(session.snapshotEvents().at(-1).data.outcome,expected);
      assert.deepEqual(await next('approval closed'),{cancel:card.id});
    }
    assert.equal(handbacks,1);
    const controller = new AbortController();
    const cancelled = approval.request({...request,signal:controller.signal});
    const abandoned = await next('cancellable approval');
    controller.abort();
    assert.equal(await cancelled,'cancelled');
    assert.deepEqual(await next('withdrawal'),{cancel:abandoned.id});

    const questions = [{id:'q',question:'Fixture?',options:[{label:'A'}]}];
    const answers = [{id:'q',selected:['A']}];
    const question = service.ask({agent,questions});
    const card = await next('question');
    assert.equal(card.kind,'question'); assert.equal(card.data.questions[0].id,'q');
    reply(card.id,'answer',{answers});
    assert.deepEqual(await question,{answers});
    assert.deepEqual(await next('question closed'),{cancel:card.id});
    const scopeConsent={id:'scope',question:'Grant fixture access?',detail:'For this session',options:[
      {label:'Allow session'},{label:'Deny'},{label:'Always allow'}],
      intent:{kind:'approval',approve:'Allow session',reject:'Deny',always:'Always allow'}};
    const consent=service.ask({agent,questions:[scopeConsent]});
    const scopeCard=await next('scope consent');
    assert.equal(scopeCard.kind,'approval');assert.equal(scopeCard.data.nativeScope,true);
    assert.ok(!scopeCard.data.operation);
    reply(scopeCard.id,'deny');
    assert.deepEqual(await consent,{answers:[{id:'scope',selected:['Deny']}]});
    assert.deepEqual(await next('scope consent closed'),{cancel:scopeCard.id});
    session.append('request/header',{header:{tools:[{name:'ask_user_question',parameters:{properties:{timeout:{}}}}]}});
    session.append('tool/call',{name:'ask_user_question',callId:'late',arguments:JSON.stringify({questions})});
    const timed = service.askTimed({agent,questions},'late',250);
    const initial = await next('timed question');
    const pending = await timed;
    assert.deepEqual(pending,{pending:true,callId:'late'});
    assert.deepEqual(await next('timed withdrawal'),{cancel:initial.id});
    session.append('tool/result',{message:{toolCallId:'late',content:[{type:'text',text:JSON.stringify(pending)}]}},{surfaceOp:'append'});
    const late = await next('continued question');
    assert.equal(late.data.deferred,true);
    reply(late.id,'answer',{answers});
    assert.deepEqual(await next('continued closed'),{cancel:late.id});
    for(let i=0;i<100&&!queued.length;i++) await delay(20);
    assert.equal(queued.length,1);
    assert.equal(queued[0].source.callId,'late');
    assert.deepEqual(JSON.parse(queued[0].content[0].text).answers,answers);
    await disposed.shift().dispose();
    assert.deepEqual(await next('unmounted'),{dshReady:false});
  } finally {
    broker.stdin.end();
    await closed;
    for(const f of disposed) await f.dispose();
  }
}
if (require.main===module) main().catch(error=>{console.error(error);process.exitCode=1;});
