'use strict';
// 由已安装的 DeepSeek Harness 以 Node 模式（ELECTRON_RUN_AS_NODE=1）运行：
// 加载一个临时 profile，挂真实 dsh-session/agent/user-approval/user-questions 插件，
// 再挂上级 agent-notices bundle（index.mjs → channel.cjs 客户端），让岛仔（daozai.exe）
// 作为桥接服务端接收审批/提问/完成事件。屏幕侧的点击由 scripts/agent-e2e.ps1 负责。
//
// 用法（agent-e2e.ps1 会这样调用）：
//   "DeepSeek Harness.exe" agent-drive.cjs --profile <临时 profile> --scenario approval \
//       --wait 90000 --json-in <场景参数.json>
// 带引号/换行的 operation、多行正文、选项数组一律走 --json-in 文件，避开 Windows 命令行二次解析；
// 命令行参数（--tool/--operation/--reason/--text/--options）仍然可用且优先。
//
// 场景：approval | question-single | question-multi | question-multi-turn | done
// 输出：一行 JSON（--pretty 时格式化），字段含 sent（发给岛仔的内容）、bridge（桥接侧收到的
// 原始回复）、decision（宿主最终拿到的决定）、handback（是否交回客户端）。
// 退出码：0 成功；2 初始化失败（profile/插件/live.json 未就绪）；3 等待超时；
//         4 请求被交回客户端（岛仔没有接管，屏幕侧不可能点得到）。

const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const { setTimeout: delay } = require('node:timers/promises');

function parseArgs(argv) {
  const out = { wait: 60000, scenario: 'approval' };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) continue;
    const key = arg.slice(2);
    if (key === 'pretty') { out.pretty = true; continue; }
    out[key] = argv[++i];
  }
  out.wait = Number(out.wait) || 60000;
  return out;
}

function emit(obj, pretty) {
  process.stdout.write(JSON.stringify(obj, null, pretty ? 2 : 0) + '\n');
}

// Promise.race 用超时：必须能取消计时器，否则进程要等满 --wait 才退出。
function timer(ms, value) {
  let handle;
  const promise = new Promise(resolve => { handle = setTimeout(() => resolve(value), ms); });
  promise.cancel = () => clearTimeout(handle);
  return promise;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  // --json-in <file>：场景参数（含带引号/换行的 operation、多行正文、选项数组）走文件，
  // 避开 Windows 命令行对引号的二次解析。命令行参数优先。
  if (args['json-in']) {
    const extra = JSON.parse(fs.readFileSync(args['json-in'], 'utf8'));
    for (const [key, value] of Object.entries(extra)) {
      if (args[key] === undefined) args[key] = value;
    }
  }
  const labels = (value, fallback) => {
    if (Array.isArray(value)) return value.map(String);
    if (typeof value === 'string' && value) return value.split('|');
    return fallback;
  };
  const finish = (result, code, error) => {
    const payload = { ...result, ok: code === 0 };
    if (error) payload.error = error;
    emit(payload, args.pretty);
    process.exitCode = code;
    return code;
  };
  const profile = args.profile;
  if (!profile) return finish({ scenario: args.scenario }, 2, '缺少 --profile');

  // 与 tests/fixtures/dsh-host.cjs 相同的定位方式：Electron 可执行文件旁边的 app.asar。
  const anchor = path.join(path.dirname(process.execPath), 'resources/app.asar/dsh');
  const host = name => require(path.join(anchor, 'node_modules/@deepseek-ai', name));
  const load = createRequire(path.join(profile, 'package.json'));

  const result = {
    scenario: args.scenario,
    profile,
    kit: null,
    descriptor: null,
    policy: null,
    sent: null,
    bridge: [],
    decision: null,
    handback: false,
  };

  let layer;
  try {
    layer = host('dsh-app-boot').loadProfileDirectory('dsh', profile, anchor);
  } catch (error) {
    return finish(result, 2, `加载 profile 失败: ${error.message}`);
  }
  if (Array.isArray(layer.skippedBundles) && layer.skippedBundles.length) {
    return finish({ ...result, skippedBundles: layer.skippedBundles }, 2,
      `profile 里的 bundle 没有加载成功：${layer.skippedBundles.join(', ')}（检查 junction 是否指向岛仔导出的 kit）`);
  }
  const inserted = layer.layers?.[0]?.patches?.[0]?.insert ?? [];
  const name = inserted[0]?.name;
  if (!name) return finish({ ...result, skippedBundles: layer.skippedBundles }, 2,
    'profile 里没有安装岛仔 bundle（先在岛仔设置里打开「DeepSeek Harness」来源，让它导出并接入）');

  // 桥接客户端所在目录就是 bundle 的真实路径（junction 已被 Node 解析成岛仔导出的 kit）。
  const kitBridge = path.dirname(load.resolve(name));
  const channelPath = path.join(kitBridge, 'channel.cjs');
  const descriptor = path.join(kitBridge, 'live.json');
  result.kit = kitBridge;
  result.descriptor = descriptor;
  if (!fs.existsSync(channelPath)) return finish(result, 2, `找不到 channel.cjs: ${channelPath}`);

  const channel = load(channelPath);
  if (!channel || typeof channel.exchange !== 'function') return finish(result, 2, 'channel.cjs 模块接口不符');

  // 可选调试日志：排查 bundle 实际使用的 channel 模块实例（ECHO 到文件，不污染 stdout JSON）。
  const debugFile = process.env.DAOZAI_E2E_DEBUG;
  const debug = (...parts) => {
    if (!debugFile) return;
    try { fs.appendFileSync(debugFile, JSON.stringify(parts) + '\n'); } catch { /* 忽略 */ }
  };
  // 记录桥接请求与回复：这是「岛仔真的收到并作答」的证据，不依赖截图。
  // 先入列再等待，超时也能看到「已经发出去、但没等到决定」；sent/mode 记录真正发给岛仔的内容。
  const original = channel.exchange;
  channel.exchange = async (provider, kind, makePayload, signal) => {
    const entry = { provider, kind, mode: null, sent: null, reply: null };
    result.bridge.push(entry);
    let wrapped = makePayload;
    if (typeof makePayload === 'function') {
      wrapped = mode => {
        const payload = makePayload(mode);
        entry.mode = mode;
        entry.sent = payload;
        return payload;
      };
    }
    debug('exchange-called', { provider, kind });
    try {
      entry.reply = await original(provider, kind, wrapped, signal);
      return entry.reply;
    } finally {
      debug('exchange-settled', { provider, kind, reply: entry.reply });
    }
  };
  debug('patched-channel', { channelPath, exports: Object.keys(channel), same: channel.exchange !== original });
  const lastReply = kind => {
    for (let i = result.bridge.length - 1; i >= 0; i--) if (result.bridge[i].kind === kind) return result.bridge[i].reply;
    return null;
  };

  const ctx = new (host('cordis').Context)();
  const fibers = [];
  const plug = async plugin => {
    const fiber = ctx.plugin(plugin);
    fibers.unshift(fiber);
    await fiber.await();
  };
  const handbacks = { approval: 0, question: 0 };

  try {
    for (const n of ['dsh-session', 'dsh-session-projection', 'dsh-agent', 'dsh-user-approval', 'dsh-user-questions']) {
      await plug(host(n).default);
    }

    // 岛仔导出 kit 并启动桥接服务端需要一点时间；先等 live.json 出现且 dsh 来源打开。
    const startDeadline = Date.now() + Math.min(args.wait, 30000);
    let descriptorSeen = null;
    while (Date.now() < startDeadline) {
      if (fs.existsSync(descriptor)) {
        try {
          descriptorSeen = JSON.parse(fs.readFileSync(descriptor, 'utf8'));
          if (descriptorSeen?.policy?.dsh) break;
        } catch { /* 岛仔正在原子替换，重试 */ }
      }
      await delay(100);
    }
    result.policy = descriptorSeen?.policy ?? null;
    if (!descriptorSeen?.policy?.dsh) {
      return finish(result, 2, 'live.json 未出现或岛仔没有打开 DSH 来源（检查 dsh_notifications/notifications 设置与岛仔是否在运行）');
    }

    const session = ctx.get('sessions').create('e2e-' + args.scenario, { meta: { cwd: profile } });
    const agent = { id: session.id, session, inbox: { nextTurn: [], nextStep: [] }, steer: () => {} };
    ctx.get('agents').enter(agent);
    session.append('turn/start', { turn: 1 });

    // 交回客户端（岛仔不接管 / 用户点「回客户端」/ 决定无效）时的兜底，避免宿主一直挂着。
    ctx.on('approval/request', () => { handbacks.approval++; return 'rejected'; });
    ctx.on('user-questions/request', req => {
      handbacks.question++;
      return { answers: (req?.questions ?? []).map(q => ({ id: q.id, selected: [] })) };
    });

    await plug(load(name));
    result.mounted = true;
    for (let i = 0; i < 100 && !channel.enabled('dsh', 'interactive'); i++) await delay(50);
    try { result.policy = JSON.parse(fs.readFileSync(descriptor, 'utf8')).policy; } catch { /* 保留旧值 */ }

    if (args.scenario === 'approval') {
      const tool = args.tool || 'Bash';
      const operation = args.operation || JSON.stringify({ command: 'echo daozai-e2e' });
      const reason = args.reason || '岛仔端到端验收：请在岛上点击「仅允许这一次」';
      const callId = 'e2e-approval-' + Date.now();
      session.append('tool/call', { callId, name: tool, arguments: operation });
      result.sent = { tool, operation, reason, callId, sessionId: session.id };
      const timeout = timer(args.wait, { timeout: true });
      let decision;
      try {
        decision = await Promise.race([
          ctx.get('approval').request({ agent, callId, toolName: tool, reason }),
          timeout,
        ]);
      } finally { timeout.cancel(); }
      const reply = lastReply('approval');
      result.decision = decision?.timeout ? 'timeout' : decision;
      result.handback = handbacks.approval > 0;
      const answered = reply !== null && (reply.decision === 'allow' || reply.decision === 'deny');
      if (!answered || result.handback) {
        const message = result.handback && !answered
          ? '岛仔没有接管这条审批，请求被交回客户端（检查设置的「岛上处理审批与提问」与 DSH 来源）'
          : answered ? '决定由客户端兜底处理，岛仔没有作答' : '审批没有在限定时间内拿到岛仔决定';
        return finish(result, result.handback ? 4 : (decision?.timeout ? 3 : 4), message);
      }
      return finish(result, 0);
    }

    if (args.scenario.startsWith('question')) {
      let questions;
      if (args['questions-json']) {
        questions = JSON.parse(fs.readFileSync(args['questions-json'], 'utf8'));
      } else if (args.scenario === 'question-single') {
        questions = [{
          id: 'e2e-single',
          question: args.question || '岛仔 E2E 单选：请选择「甲选项」',
          options: labels(args.options, ['甲选项', '乙选项', '丙选项']).map(label => ({ label })),
        }];
      } else if (args.scenario === 'question-multi') {
        questions = [{
          id: 'e2e-multi',
          question: args.question || '岛仔 E2E 多选：请勾选「多选甲」和「多选丙」',
          multiSelect: true,
          options: labels(args.options, ['多选甲', '多选乙', '多选丙']).map(label => ({ label })),
        }];
      } else {
        questions = [
          {
            id: 'e2e-turn-1',
            question: args.question || '岛仔 E2E 多轮 1/2：请选择「第一组甲」',
            options: labels(args.options, ['第一组甲', '第一组乙']).map(label => ({ label })),
          },
          {
            id: 'e2e-turn-2',
            question: '岛仔 E2E 多轮 2/2：请选择「第二组甲」',
            options: ['第二组甲', '第二组乙'].map(label => ({ label })),
          },
        ];
      }
      result.sent = {
        questions: questions.map(q => ({
          id: q.id, question: q.question, multi: !!(q.multiSelect ?? q.multi_select),
          options: (q.options ?? []).map(o => o.label),
        })),
      };
      const timeout = timer(args.wait, { timeout: true });
      let answer;
      try {
        answer = await Promise.race([ctx.get('userQuestions').ask({ agent, questions }), timeout]);
      } finally { timeout.cancel(); }
      const reply = lastReply('question');
      result.decision = answer?.timeout ? 'timeout' : answer;
      result.handback = handbacks.question > 0;
      const valid = !answer?.timeout && reply?.decision === 'answer' && Array.isArray(answer?.answers)
        && answer.answers.length === questions.length;
      if (!valid || result.handback) {
        return finish(result, answer?.timeout ? 3 : 4,
          result.handback ? '岛仔没有接管这条提问，请求被交回客户端（检查设置的「岛上处理审批与提问」与 DSH 来源）'
            : '提问没有在限定时间内拿到岛仔有效回答');
      }
      return finish(result, 0);
    }

    if (args.scenario === 'done') {
      const text = args.text || [
        'E2E 完成正文：岛仔端到端验收。',
        '- 标记 daozai-e2e-done',
        '- 如果你在岛上看到这段话，说明真实 DSH 宿主 → 桥接 → 岛仔的完成链路是通的。',
      ].join('\n');
      result.sent = { text, length: text.length, sessionId: session.id };
      session.append('assistant/message', { message: { content: [{ type: 'text', text }] } }, { surfaceOp: 'append' });
      session.append('turn/end', { turn: 1 });
      // turn/end 里 bundle 会自己发起 finished exchange；等它拿到 {done:true}。
      const deadline = Date.now() + args.wait;
      for (;;) {
        const entry = result.bridge.find(x => x.kind === 'finished');
        if (entry?.reply?.done) {
          result.decision = 'done';
          return finish(result, 0);
        }
        if (Date.now() > deadline) {
          result.decision = 'timeout';
          return finish(result, 3,
            '桥接没有确认收到完成事件（岛仔是否打开「任务完成提醒」，且「完成内容」不是「仅状态」？）');
        }
        await delay(100);
      }
    }

    return finish(result, 2, `未知场景: ${args.scenario}`);
  } catch (error) {
    return finish(result, 2, `驱动失败: ${error?.stack || error}`);
  } finally {
    for (const fiber of fibers) { try { await fiber.dispose(); } catch { /* 退出路径忽略 */ } }
  }
}

if (require.main === module) main().catch(error => { console.error(error); process.exitCode = 2; });
