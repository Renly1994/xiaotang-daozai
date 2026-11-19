import bridge from './notify.cjs';
import channel from './channel.cjs';

export const name = 'daozai-agent-notices';
export function apply(ctx) {
  const last = new WeakMap(), calls = new WeakMap(), active = new Set();
  const ask = async (kind, payload, req) => {
    const controller = new AbortController(), abort = () => controller.abort();
    active.add(controller); req.signal?.addEventListener('abort',abort,{once:true});
    if (req.signal?.aborted) abort();
    try { return await channel.exchange('dsh',kind,payload,controller.signal); }
    finally { active.delete(controller); req.signal?.removeEventListener('abort',abort); }
  };
  ctx.on('session/event', (session, event) => {
    const data=event?.data ?? {};
    if (event.type === 'turn/start') { last.delete(session); calls.delete(session); }
    if (event.type === 'assistant/message' && channel.wantsText('dsh')) {
      const content=data.message?.content ?? data.content;
      if (Array.isArray(content)) last.set(session,content.filter(b=>b.type==='text').map(b=>b.text).join('\n').slice(0,32000));
    }
    const callId=data.callId??data.call?.id??data.id, tool=data.name??data.call?.name;
    if (event.type === 'tool/call' && callId && tool && channel.enabled('dsh','interactive')) {
      const map=calls.get(session) ?? new Map(); calls.set(session,map);
      const value=data.arguments??data.call?.arguments;
      const args=typeof value==='string' ? value : JSON.stringify(value);
      map.delete(callId);
      if (map.size<64 && args && args.length<=24000) map.set(callId,{name:tool,args});
    }
    if (event.type === 'tool/result') calls.get(session)?.delete(data.message?.toolCallId??data.toolCallId??callId);
    if (event.type === 'turn/end') {
      const text=last.get(session) ?? ''; last.delete(session); calls.delete(session);
      void channel.exchange('dsh','finished',mode=>withSession({text:mode===2?text:mode===1?text.slice(0,1200):''},session))
        .then(result=>{if (!result?.done) bridge.notify('dsh','finished');});
    }
  }, { global: true });
  ctx.on('approval/request', async (req,next) => {
    const session=req.agent?.session, call=calls.get(session)?.get(req.callId);
    const reply=await ask('approval',()=>withSession({tool:String(req.toolName||''),reason:String(req.reason||''),
      cwd:session?.header?.cwd||'',operation:call?.name===req.toolName?call.args:''},session),req);
    if (req.signal?.aborted) return 'cancelled';
    if (reply?.decision==='allow' && call && call.name===req.toolName && call.args) return 'allowed-once';
    if (reply?.decision==='deny') return 'rejected';
    if (!reply) bridge.notify('dsh','approval');
    return next();
  }, {prepend:true});
  ctx.on('user-questions/request', async (req, next) => {
    const questions=req.questions, session=req.agent?.session;
    // An intent can grant session-wide or permanent authority, or approve a plan.
    // DSH must present those controls with their native semantics.
    if (Array.isArray(questions) && questions.some(q=>q?.intent)) {
      const q=questions[0];
      if (questions.length!==1 || q.intent?.kind!=='approval' || !q.intent.reject ||
          !q.options?.some(o=>o.label===q.intent.reject)) return next();
      const reply=await ask('approval',()=>withSession({tool:q.question,reason:q.detail || '',nativeScope:true},session),req);
      if (req.signal?.aborted) throw req.signal.reason ?? Error('Question cancelled');
      if (reply?.decision==='deny') return {answers:[{id:q.id,selected:[q.intent.reject]}]};
      if (!reply) bridge.notify('dsh','approval');
      return next();
    }
    const reply=await ask('question',()=>withSession(questionPayload(questions),session),req);
    if (req.signal?.aborted) throw req.signal.reason ?? Error('Question cancelled');
    if (reply?.decision==='answer' && validAnswers(questions,reply.answers)) return {answers:reply.answers};
    if (!reply) bridge.notify('dsh', 'question');
    return next();
  }, { prepend: true });
  const stop=watchContinued(ctx);
  const unmount=channel.mounted();
  const dispose=()=>{unmount();stop();for(const job of active)job.abort();};
  if (typeof ctx.effect==='function') ctx.effect(()=>dispose,'daozai: live requests');
  else ctx.on('dispose',dispose);
}

// A timed-out native question is no longer a tool return. Address the live
// projection by agent and call, and only use the host's continued-answer API.
export function watchContinued(ctx){
  const jobs=new Map();
  const clear=()=>{for(const job of jobs.values())job.abort.abort();jobs.clear();};
  const scan=()=>{
    if (!channel.enabled('dsh','deferred')){clear();return;}
    const agents=ctx.get?.('agents'), states=ctx.get?.('sessionProjections'), service=ctx.get?.('userQuestions');
    const roots=()=>typeof agents?.list==='function' ? agents.list().filter(a=>a.session?.header?.origin!=='subagent') : agents?.roots?.()??[];
    if (!service?.answer || (!service.continued && !states?.stateOf)){clear();return;}
    const pending=(a)=>typeof service.continued==='function' ? service.continued(a) : states.stateOf(a.session,'userQuestions')?.questions?.active??[];
    const live=new Set();
    for(const agent of roots()) for(const q of pending(agent)){
      if(q.state!=='continued'||!q.callId||!agent.session?.id)continue;
      const key=JSON.stringify([agent.session.id,q.callId]);live.add(key);
      const prior=jobs.get(key);
      if (prior?.agent===agent && (!prior.retry || Date.now()<prior.retry))continue;
      prior?.abort.abort();
      const payload=questionPayload(q.questions);if(!payload)continue;
      const signature=JSON.stringify(payload),job={agent,abort:new AbortController()};jobs.set(key,job);
      void channel.exchange('dsh','question',()=>({...payload,deferred:true}),job.abort.signal).then(reply=>{
        if(jobs.get(key)!==job||job.abort.signal.aborted)return;
        if(!reply){job.retry=Date.now()+4000;return;}
        if(reply.decision!=='answer'||!channel.enabled('dsh','deferred')||!roots().includes(agent))return;
        const current=pending(agent).find(x=>x.state==='continued'&&x.callId===q.callId);
        if(current && JSON.stringify(questionPayload(current.questions))===signature && validAnswers(current.questions,reply.answers)){
          service.answer(agent,q.callId,{answers:reply.answers});
        }
      }).catch(()=>{ /* Hosts may reject a decision that lost a race to the native UI. */ });
    }
    for(const [key,job] of jobs)if(!live.has(key)){job.abort.abort();jobs.delete(key);}
  };
  const poll=setInterval(()=>{try{scan();}catch{clear();}},1500);poll.unref();
  const stop=()=>{clearInterval(poll);clear();};stop.scan=scan;return stop;
}
// The island jumps back to the originating conversation, so every request carries
// the session identity. `null` payloads stay null: nothing is sent without a body.
const withSession=(payload,session)=>payload&&{...payload,sessionId:String(session?.id ?? '')};
function questionPayload(qs) {
  if (!Array.isArray(qs)||!qs.length||qs.length>20) return null;
  const ids=new Set();
  for (const q of qs) {
    if (typeof q?.id!=='string'||!q.id||ids.has(q.id)||typeof q.question!=='string'||!q.question) return null;
    ids.add(q.id);
    const options=q.options??[];
    if (!Array.isArray(options)||options.length>50||options.some(o=>typeof o?.label!=='string'||!o.label)||
        new Set(options.map(o=>o.label)).size!==options.length) return null;
  }
  return {questions:qs.map(q=>({id:q.id,question:q.question,detail:q.detail||'',multi:!!(q.multiSelect??q.multi_select),
    options:(q.options??[]).map(o=>({label:o.label,description:o.description||''}))}))};
}
function validAnswers(qs,answers) {
  return !!questionPayload(qs) && Array.isArray(answers) && answers.length===qs.length && qs.every((q,i)=>{
    const a=answers[i];
    return a?.id===q.id && Array.isArray(a.selected) && (!a.custom||typeof a.custom==='string') &&
      new Set(a.selected).size===a.selected.length && a.selected.every(v=>q.options?.some(o=>o.label===v)) &&
      ((q.multiSelect??q.multi_select)||a.selected.length+Number(!!a.custom)<=1);
  });
}
