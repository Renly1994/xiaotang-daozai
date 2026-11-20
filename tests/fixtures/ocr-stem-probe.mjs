// OCR 回归夹具：发一张「题干里写着选项名」的两题提问卡（例如题干「…请选择「方案一」」+ 选项「方案一」）。
//
// 用途：复现 `agent-e2e.ps1` question-multi-turn 曾经踩到的坑——题干与选项同名时，
// 小字号下 Windows OCR 可能只读出题干那一行，于是「PreferBottom 取最靠下命中」会点到题干上，
// 表现为「已答 0」不变、下一题点不动。脚本侧已用「选项行只比选项词多一个序号徽标」
// （`-MaxExtra 2`）+ 5/6 倍率兜底修掉；本夹具用于以后回归这条 OCR 判定。
//
// 用法：
//   node tests/fixtures/ocr-stem-probe.mjs <DAOZAI_DATA_DIR>     # 岛仔需已启动并导出桥接 kit
//   然后：powershell -NoProfile -File scripts\agent-screen-ocr.ps1 -Region "520,0,1400,780" -Scale 5
//   期望：OCR 行里出现短行「1 方案一」（len≈5），题干行是长行（len≈17~19）。
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { setTimeout as delay } from 'node:timers/promises';

const [, , dataDir] = process.argv;
if (!dataDir) {
  console.error('用法：node tests/fixtures/ocr-stem-probe.mjs <DAOZAI_DATA_DIR>');
  process.exit(2);
}
const bridgeDir = path.join(dataDir, 'agent-notices', 'bridge');
const descriptor = path.join(bridgeDir, 'live.json');
for (let i = 0; i < 200 && !fs.existsSync(descriptor); i++) await delay(100);
const require = createRequire(import.meta.url);
const channel = require(path.join(bridgeDir, 'channel.cjs'));

const payload = {
  sessionId: 'ocr-stem-probe',
  questions: [
    {
      id: 'e2e-turn-1',
      question: '岛仔 E2E 多轮 1/2：请选择「方案一」',
      detail: '',
      multi: false,
      options: [{ label: '方案一' }, { label: '方案二' }],
    },
    {
      id: 'e2e-turn-2',
      question: '岛仔 E2E 多轮 2/2：请选择「确认一」',
      detail: '',
      multi: false,
      options: [{ label: '确认一' }, { label: '确认二' }],
    },
  ],
};
const reply = await channel.exchange('dsh', 'question', () => payload);
console.log(JSON.stringify(reply));
