// 本机开发探针：像真实 DSH 插件一样，通过岛仔导出的 channel.cjs 发一条 Agent 请求，
// 用于在真实窗口里检查卡片外观与点击闭环（不依赖 DSH 宿主）。
//
// 用法：
//   node scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> approval
//   node scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> question
//   node scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> question-multi`n//   node scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> question-rounds
//   node scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> finished
//   node scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> queue
//
// 输出：桥接返回的决定（JSON）。若岛仔未运行或卡片未接管，输出 null。
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { setTimeout as delay } from 'node:timers/promises';

const [, , dataDir, scenario = 'approval'] = process.argv;
if (!dataDir) {
  console.error('用法：node scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> [approval|question|question-multi|question-rounds|finished|queue]');
  process.exit(2);
}
const bridgeDir = path.join(dataDir, 'agent-notices', 'bridge');
const descriptor = path.join(bridgeDir, 'live.json');
for (let i = 0; i < 200 && !fs.existsSync(descriptor); i++) await delay(100);
if (!fs.existsSync(descriptor)) {
  console.error(`岛仔没有发布通道描述：${descriptor}（先启动 daozai）`);
  process.exit(1);
}
const require = createRequire(import.meta.url);
const channel = require(path.join(bridgeDir, 'channel.cjs'));

const approval = () => ({
  tool: process.env.PROBE_TOOL || 'write',
  reason: process.env.PROBE_REASON || '应用新的超时与重试策略，发布前需要确认写入生产配置。',
  cwd: process.env.PROBE_CWD || 'C:\\work\\api-gateway',
  operation: process.env.PROBE_OPERATION
    || '{\n  "file_path": "config/gateway.yaml",\n  "content": "upstream.timeout: 30s -> 12s\\nretry: 2\\nroutes: 6"\n}',
  sessionId: 'probe-session',
});
const question = (multi) => ({
  sessionId: 'probe-session',
  questions: [
    {
      id: 'q-choice',
      question: '发布前用哪种验证策略？',
      detail: '选择后本轮会按该策略继续执行，可在下一步再调整。',
      multi,
      options: [
        { label: '仅跑冒烟测试', description: '约 2 分钟，覆盖登录与支付主链路' },
        { label: '跑完整回归', description: '约 18 分钟，覆盖 1317 个用例' },
        { label: '先不验证', description: '直接进入发布确认' },
      ],
    },
  ],
});
// 两题多轮：检查「第 1/2 题 → 下一题 → 检查答案 → 确认提交」的分页与审阅页。
const questionRounds = () => ({
  sessionId: 'probe-session',
  questions: [
    {
      id: 'q-first',
      question: '这次改动要包含哪些模块？',
      detail: '可以多选，也可以在下面补充说明。',
      multi: true,
      options: [
        { label: '审批卡', description: '目标 / 操作内容 / 申请理由，长命令可滚动' },
        { label: '提问卡', description: '选项描述、审阅页与自定义回答' },
        { label: '完成通知', description: '本轮回复的受限 Markdown 阅读' },
      ],
    },
    {
      id: 'q-second',
      question: '要不要保留旧的缩写按钮？',
      detail: '',
      multi: false,
      options: [
        { label: '保留', description: '沿用 v0.6 的位置' },
        { label: '去掉', description: '只留岛内详情卡' },
      ],
    },
  ],
});

const send = (kind, make) => channel.exchange('dsh', kind, make);

let result;
switch (scenario) {
  case 'approval':
    result = await send('approval', approval);
    break;
  case 'question':
    result = await send('question', () => question(false));
    break;
  case 'question-multi':
    result = await send('question', () => question(true));
    break;
  case 'question-rounds':
    result = await send('question', questionRounds);
    break;
  case 'finished':
    result = await send('finished', () => ({ text: process.env.PROBE_TEXT || '## 本轮完成\n- 更新网关超时与重试\n- 跑了 12 个回归用例\n\n```bash\ncargo test --lib\n```\n\n全部通过。' }));
    break;
  case 'queue':
    // 三条审批排队：检查队列计数与切换。
    result = [];
    for (let i = 0; i < 3; i++) {
      const p = channel.exchange('dsh', 'approval', () => ({
        tool: ['read', 'write', 'bash'][i],
        reason: `第 ${i + 1} 条排队请求`,
        cwd: 'C:\\work\\api-gateway',
        operation: `{"step": ${i + 1}}`,
        sessionId: 'probe-session',
      }));
      result.push(p);
    }
    result = await Promise.all(result);
    break;
  default:
    console.error(`未知场景：${scenario}`);
    process.exit(2);
}
console.log(JSON.stringify(result));
