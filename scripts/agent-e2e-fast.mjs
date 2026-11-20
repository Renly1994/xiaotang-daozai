#!/usr/bin/env node
// =============================================================================
// 岛仔快速 E2E（ui-map 驱动）：读岛仔自己导出的 UI 布局 JSON，按控件矩形直接坐标点击，
// 用「等 ui-map 状态变化」代替固定 sleep，不再走「截图 + Windows OCR 找字」的慢路径。
//
// 契约：docs/superpowers/research/2026-11-20-agent-ui-map-contract.md（schema = 1）
// 生产端：src/ui_map.rs（环境变量 DAOZAI_UI_MAP = 文件路径或目录）
//
// 设计要点
//   1) 编排与 scripts/agent-e2e.ps1 一致：隔离数据目录（DAOZAI_DATA_DIR）+ settings.txt +
//      exe 副本（避免锁 target 构建产物）+ 真实 DSH 宿主驱动 tests/fixtures/agent-drive.cjs
//      （ELECTRON_RUN_AS_NODE=1 + DSH_HOME），桥接侧 JSON 作为强证据。
//   2) 坐标：ui-map 的 nodes[].rect 是 canvas 逻辑坐标 [x,y,w,h]（egui points），
//      屏幕物理像素 = 客户区左上（Win32 ClientToScreen）+ rect * scale（scale = pixels_per_point）。
//      契约写的是「窗口左上」，本项目窗口是无边框的（with_decorations(false)），
//      窗口左上 == 客户区左上；脚本仍取 ClientToScreen 的精确值并把两者差异记进 summary。
//   3) 等待：所有「点完等一会儿」都换成 waitState / waitMapChange（轮询 ui-map），
//      超时抛带最后状态的人类可读错误。
//   4) 截图：一个常驻 PowerShell 辅助进程做 GDI 屏幕 1:1 截图，按 ui-map 的 card / layer.result
//      矩形裁剪后存 PNG；不启动 OCR 子进程。
//   5) 无需真岛仔也能验证消费者逻辑：`--self-test` 用临时目录 + 假 ui-map.json + 假状态序列，
//      覆盖 解析/schema/新鲜度/取点/换算/超时/断言/截图裁剪/summary/错误码。
//
// 用法
//   node scripts/agent-e2e-fast.mjs                     # 默认 4 个场景全跑
//   node scripts/agent-e2e-fast.mjs --scenario approval
//   node scripts/agent-e2e-fast.mjs --exe target\x86_64-pc-windows-msvc\release\daozai.exe
//   node scripts/agent-e2e-fast.mjs --keep-alive         # 跑完保留岛仔现场
//   node scripts/agent-e2e-fast.mjs --self-test          # 不需要真岛仔（CI/离线自检）
//   node scripts/agent-e2e-fast.mjs --legacy             # 转调旧 OCR 脚本做新旧对照
//
// 退出码
//   0  全部通过
//   2  前置条件/参数错误（缺 exe、缺 DSH、缺文件、参数非法、辅助进程起不来）
//   3  场景失败（节点缺失、状态没按预期变化、等待超时、驱动超时/handback、
//      屏幕点击与桥接决定不一致）
//   4  已有 daozai 实例在运行（单实例互斥体会互踢；脚本不会动别人的岛仔）
//   5  屏幕已锁定（输入桌面不是 Default；锁屏时点击/截图都被安全桌面挡住）
//   6  ui-map 不可用（缺失 / schema≠1 / JSON 解析失败 / 变化后不新鲜）
//   7  驱动失败（驱动进程起不来 / 初始化失败 / 没有输出 JSON）
//   --legacy 模式的退出码沿用 scripts/agent-e2e.ps1（0 / 2 / 3 / 4）。
//
// 约束：Node 原生、无第三方依赖；不改 src/；不 commit。
// =============================================================================

import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..');

// ---------------------------------------------------------------- 常量

const EXIT = Object.freeze({
  OK: 0,
  USAGE: 2,
  SCENARIO: 3,
  INSTANCE: 4,
  LOCKED: 5,
  UIMAP: 6,
  DRIVER: 7,
});

const MAP_SCHEMA = 1;
const MAP_FILE = 'ui-map.json';
const DEFAULT_FRESH_MS = 1500;
const DEFAULT_EXE = path.join(REPO, 'target', 'x86_64-pc-windows-msvc', 'release', 'daozai.exe');
const DRIVE_SCRIPT = path.join(REPO, 'tests', 'fixtures', 'agent-drive.cjs');
const LEGACY_SCRIPT = path.join(REPO, 'scripts', 'agent-e2e.ps1');
const OUT_ROOT = path.join(REPO, 'docs', 'superpowers', 'research', 'agent-e2e-shots');

// 与 scripts/agent-e2e.ps1 完全一致的 settings.txt（island_size=140 对应 App 自身缩放 1.4，
// 注意它和 ui-map.scale（OS DPI / pixels_per_point）是两个不同的缩放，不要混用）。
const AGENT_PREFS = '{"interactive":true,"handoff":0,"completion":true,"dwell":15,"expanded":true,"content":2,"deferred":true,"system_toast":false}';
const SETTINGS_LINES = [
  'notifications=1', 'system_notify=0', 'sound=0', 'read_notifications=0',
  'dsh_notifications=1', 'codex_notifications=0',
  `agent_cards=${AGENT_PREFS}`,
  'hide_fullscreen=0', 'avoid_top_bar=0', 'pet=0', 'island_size=140',
];

// 场景顺序：与旧脚本 all 的区别是把 question-multi-turn 也纳入默认集合（本波交付 4 条）。
const SCENARIO_ORDER = ['approval', 'question', 'question-multi-turn', 'done'];

const SCENARIOS = {
  approval: {
    driver: 'approval',
    request: {
      tool: 'functions.bash',
      operation: '{\n  "command": "git push --force origin main",\n  "timeout": 30000\n}',
      reason: '岛仔端到端验收：请在岛上点击「仅允许这一次」',
    },
  },
  question: {
    driver: 'question-single',
    request: { options: ['冒烟测试', '完整回归', '先不验证'] },
    expectedAnswers: ['冒烟测试'],
  },
  'question-multi-turn': {
    driver: 'question-multi-turn',
    request: { options: ['方案一', '方案二'] },
    expectedAnswers: ['方案一', '确认一'],
  },
  done: {
    driver: 'done',
    request: {
      text: [
        'E2E 完成正文：岛仔端到端验收。',
        '- 标记 daozai-e2e-done',
        '- 如果你在岛上看到这段话，说明真实 DSH 宿主 → 桥接 → 岛仔的完成链路是通的。',
      ].join('\n'),
    },
  },
};

// ---------------------------------------------------------------- 基础工具

class E2eError extends Error {
  constructor(code, message, detail) {
    super(message);
    this.name = 'E2eError';
    this.code = code;
    this.detail = detail;
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const nowMs = () => Date.now();

function elapsed(startedAt) {
  return nowMs() - startedAt;
}

/** 统一日志：进 summary.log，也打到控制台。 */
class Logger {
  constructor(verbose = false) {
    this.lines = [];
    this.verbose = verbose;
  }

  log(...parts) {
    const line = `[fast] ${parts.join(' ')}`;
    this.lines.push(`${new Date().toISOString()} ${line}`);
    console.log(line);
  }

  warn(...parts) {
    const line = `[fast][warn] ${parts.join(' ')}`;
    this.lines.push(`${new Date().toISOString()} ${line}`);
    console.warn(line);
  }

  debug(...parts) {
    if (!this.verbose) return;
    const line = `[fast][debug] ${parts.join(' ')}`;
    this.lines.push(`${new Date().toISOString()} ${line}`);
    console.log(line);
  }
}

function stamp() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

function sanitizeFileName(name) {
  return String(name).replace(/[\\/:*?"<>|\s]+/g, '-').replace(/^-+|-+$/g, '');
}

function sha256File(file) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash('sha256');
    const stream = fs.createReadStream(file);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('end', () => resolve(hash.digest('hex').toUpperCase()));
    stream.on('error', reject);
  });
}

function killTree(pid) {
  if (!pid) return;
  try {
    spawnSync('taskkill', ['/PID', String(pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
  } catch {
    /* 收尾路径忽略 */
  }
}

/** 用 tasklist 列进程（比每次起 PowerShell 快；进程名都是 ASCII）。 */
function listProcesses() {
  const result = spawnSync('tasklist', ['/FO', 'CSV', '/NH'], { windowsHide: true, maxBuffer: 16 * 1024 * 1024 });
  const text = result.stdout ? result.stdout.toString('latin1') : '';
  const out = [];
  for (const line of text.split(/\r?\n/)) {
    const match = line.match(/^"([^"]+)","(\d+)"/);
    if (match) out.push({ name: match[1], pid: Number(match[2]) });
  }
  return out;
}

// ---------------------------------------------------------------- 参数

function usage() {
  return [
    '岛仔快速 E2E（ui-map 驱动）',
    '',
    '用法：node scripts/agent-e2e-fast.mjs [选项]',
    '',
    '选项：',
    '  --scenario <name>     approval | question | question-multi-turn | done | all（默认 all）',
    '  --exe <path>          岛仔 exe（默认 target\\x86_64-pc-windows-msvc\\release\\daozai.exe）',
    '  --dsh-exe <path>      DeepSeek Harness exe（默认 %LOCALAPPDATA%\\Programs\\DeepSeek Harness\\DeepSeek Harness.exe）',
    '  --data-dir <dir>      隔离数据目录（默认 %TEMP%\\daozai-fast-<时间戳>）',
    '  --out-root <dir>      证据根目录（默认 docs\\superpowers\\research\\agent-e2e-shots）',
    '  --wait-seconds <n>    驱动等待上限（默认 90）',
    '  --fresh-ms <n>        ui-map 新鲜度阈值（默认 1500ms，变化后的那次写必须比它新）',
    '  --settle-ms <n>       点击后等界面变化的上限（默认 1200ms；不写死 sleep，只是上限）',
    '  --keep-alive          跑完不杀岛仔（保留现场）',
    '  --nudge               每次截图前抖一下鼠标（屏幕闲置/息屏时唤醒合成器，默认关闭）',
    '  --verbose             打印调试日志（含每个 ui-map 读的 t/age）',
    '  --self-test           自检消费者逻辑（假 ui-map.json，不需要真岛仔）',
    '  --helper-check        自检 Win32 辅助进程（PowerShell/DPI/截图/锁屏，不动岛仔、不点击）',
    '  --driver-check        自检宿主驱动链路（隔离空 profile，预期 exit=2「没有岛仔 bundle」）',
    '  --legacy              转调旧 OCR 脚本 scripts/agent-e2e.ps1（新旧对照）',
    '  --legacy-arg <arg>    追加一个原样传给旧脚本的参数（可重复）',
    '  --help, -h            本帮助',
    '',
    '退出码：0 通过 / 2 前置条件 / 3 场景失败 / 4 已有实例 / 5 锁屏 / 6 ui-map / 7 驱动。',
  ].join('\n');
}

function parseArgs(argv) {
  const opts = {
    scenario: 'all',
    exe: null,
    dshExe: null,
    dataDir: null,
    outRoot: null,
    waitSeconds: 90,
    freshMs: DEFAULT_FRESH_MS,
    settleMs: 1200,
    keepAlive: false,
    nudge: false,
    verbose: false,
    selfTest: false,
    helperCheck: false,
    driverCheck: false,
    legacy: false,
    legacyArgs: [],
    help: false,
  };
  const need = (name, value) => {
    if (value === undefined) throw new E2eError(EXIT.USAGE, `参数 ${name} 需要一个值`);
    return value;
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    switch (arg) {
      case '--help': case '-h': opts.help = true; break;
      case '--self-test': opts.selfTest = true; break;
      case '--helper-check': opts.helperCheck = true; break;
      case '--driver-check': opts.driverCheck = true; break;
      case '--legacy': opts.legacy = true; break;
      case '--keep-alive': opts.keepAlive = true; break;
      case '--nudge': opts.nudge = true; break;
      case '--verbose': opts.verbose = true; break;
      case '--scenario': opts.scenario = need(arg, argv[++i]); break;
      case '--exe': opts.exe = need(arg, argv[++i]); break;
      case '--dsh-exe': opts.dshExe = need(arg, argv[++i]); break;
      case '--data-dir': opts.dataDir = need(arg, argv[++i]); break;
      case '--out-root': opts.outRoot = need(arg, argv[++i]); break;
      case '--wait-seconds': opts.waitSeconds = Number(need(arg, argv[++i])); break;
      case '--fresh-ms': opts.freshMs = Number(need(arg, argv[++i])); break;
      case '--settle-ms': opts.settleMs = Number(need(arg, argv[++i])); break;
      case '--legacy-arg': opts.legacyArgs.push(need(arg, argv[++i])); break;
      default:
        throw new E2eError(EXIT.USAGE, `未知参数：${arg}（--help 看用法）`);
    }
  }
  if (!Number.isFinite(opts.waitSeconds) || opts.waitSeconds <= 0) throw new E2eError(EXIT.USAGE, '--wait-seconds 必须是正数');
  if (!Number.isFinite(opts.freshMs) || opts.freshMs <= 0) throw new E2eError(EXIT.USAGE, '--fresh-ms 必须是正数');
  if (!Number.isFinite(opts.settleMs) || opts.settleMs <= 0) throw new E2eError(EXIT.USAGE, '--settle-ms 必须是正数');
  if (opts.scenario !== 'all' && !SCENARIOS[opts.scenario]) {
    throw new E2eError(EXIT.USAGE, `未知场景 ${opts.scenario}（可选：${[...SCENARIO_ORDER, 'all'].join(' / ')}）`);
  }
  return opts;
}

// ---------------------------------------------------------------- ui-map 纯逻辑（可离线自测）

function parseUiMapText(text) {
  const map = JSON.parse(text);
  if (!map || typeof map !== 'object' || Array.isArray(map)) throw new Error('根节点不是对象');
  return map;
}

/** 契约 schema=1 的必填项校验；返回问题列表（空 = 通过）。 */
function validateUiMap(map) {
  const problems = [];
  if (map.schema !== MAP_SCHEMA) problems.push(`schema=${JSON.stringify(map.schema)}，期望 ${MAP_SCHEMA}`);
  if (!Number.isFinite(map.t)) problems.push('t 不是数字');
  if (!Number.isFinite(map.scale) || map.scale <= 0) problems.push('scale 不是正数');
  if (!map.canvas || !Number.isFinite(map.canvas.w) || !Number.isFinite(map.canvas.h) || map.canvas.w <= 0 || map.canvas.h <= 0) {
    problems.push('canvas.w / canvas.h 无效');
  }
  if (!map.state || typeof map.state !== 'object') problems.push('state 缺失');
  if (!Array.isArray(map.nodes)) problems.push('nodes 不是数组');
  else {
    for (const node of map.nodes) {
      if (!node || typeof node !== 'object' || typeof node.id !== 'string' || !node.id) {
        problems.push('存在缺少 id 的节点');
        continue;
      }
      if (!Array.isArray(node.rect) || node.rect.length !== 4 || !node.rect.every((v) => Number.isFinite(v))) {
        problems.push(`节点 ${node.id} 的 rect 不是 [x, y, w, h]`);
      }
      if (node.enabled !== undefined && typeof node.enabled !== 'boolean') problems.push(`节点 ${node.id} 的 enabled 不是布尔`);
      if (node.label !== undefined && typeof node.label !== 'string') problems.push(`节点 ${node.id} 的 label 不是字符串`);
    }
  }
  return problems;
}

function findNode(map, id) {
  return (map.nodes ?? []).find((node) => node.id === id) ?? null;
}

function nodeIds(map) {
  return (map.nodes ?? []).map((node) => node.id);
}

/**
 * 节点中心 → 屏幕物理像素。
 * @param {number} clientLeft ClientToScreen(0,0).x（物理像素）
 * @param {number} clientTop  ClientToScreen(0,0).y
 * @param {number} coordScale 逻辑点 → 该坐标系像素的缩放（正常 = map.scale）
 * @param {number[]} rect     canvas 逻辑坐标 [x, y, w, h]
 */
function nodeScreenPoint(clientLeft, clientTop, coordScale, rect) {
  const [x, y, w, h] = rect;
  return {
    x: Math.round(clientLeft + (x + w / 2) * coordScale),
    y: Math.round(clientTop + (y + h / 2) * coordScale),
  };
}

function clampRect(rect, screen) {
  if (!screen) return rect;
  const left = Math.max(screen.left, rect.left);
  const top = Math.max(screen.top, rect.top);
  const right = Math.min(screen.right, rect.left + rect.w);
  const bottom = Math.min(screen.bottom, rect.top + rect.h);
  return { left, top, w: Math.max(0, right - left), h: Math.max(0, bottom - top) };
}

/** 证据裁剪矩形：优先 card；结果层场景优先 layer.result；都没有就退回整个客户区。 */
function cropRectPx(map, win, coordScale, prefer = 'card') {
  const order = prefer === 'result' ? ['layer.result', 'card'] : ['card', 'layer.result'];
  for (const id of order) {
    const node = findNode(map, id);
    if (!node) continue;
    const [x, y, w, h] = node.rect;
    const pad = 6; // 逻辑点；把卡片阴影/圆角带一点进证据图
    const rect = {
      left: Math.round(win.client.left + (x - pad) * coordScale),
      top: Math.round(win.client.top + (y - pad) * coordScale),
      w: Math.round((w + pad * 2) * coordScale),
      h: Math.round((h + pad * 2) * coordScale),
    };
    return clampRect(rect, win.screen);
  }
  return clampRect({ left: win.client.left, top: win.client.top, w: win.client.w, h: win.client.h }, win.screen);
}

/** 状态 + 节点（id/矩形/可用性）签名：判断「这一屏到底有没有变」。 */
function signatureOf(map) {
  const nodes = (map.nodes ?? [])
    .map((node) => `${node.id}:${node.rect.join(',')}:${node.enabled === false ? '0' : '1'}`)
    .join(';');
  return `${JSON.stringify(map.state)}|${nodes}`;
}

function summarizeState(map) {
  const s = map.state ?? {};
  const card = s.card
    ? `${s.card.kind}(index=${s.card.index}/${s.card.count},details=${s.card.details},submitting=${s.card.submitting}${s.card.error ? `,error=${s.card.error}` : ''})`
    : 'null';
  const result = s.result ? `open(hint=${s.result.hint},title=${JSON.stringify(s.result.title)})` : 'null';
  return { panel: s.panel, details: s.details, notice: s.notice, queue: s.queue, card, result, nodes: nodeIds(map) };
}

function describeMap(map) {
  if (!map) return '（没有 ui-map）';
  const s = map.state ?? {};
  return [
    `panel=${s.panel}`,
    `details=${s.details}`,
    `notice=${s.notice}`,
    `queue=${s.queue}`,
    `card=${s.card ? JSON.stringify({ kind: s.card.kind, index: s.card.index, count: s.card.count, details: s.card.details, submitting: s.card.submitting }) : 'null'}`,
    `result=${s.result ? JSON.stringify({ open: s.result.open, hint: s.result.hint, title: s.result.title }) : 'null'}`,
    `nodes=[${nodeIds(map).join(', ')}]`,
  ].join(' ');
}

/** 结果层是否打开：state.result.open，或结果层独有节点出现（生产端字段可能还没接全）。 */
function isResultLayerOpen(map) {
  return map.state?.result?.open === true
    || !!findNode(map, 'layer.result')
    || !!findNode(map, 'btn.copy')
    || !!findNode(map, 'body.result');
}

/**
 * 桥接侧断言（强证据）：屏幕点了什么，桥接决定就必须是什么。
 * @returns {{ok: boolean, error?: string}}
 */
function checkBridge(name, json, spec) {
  if (!json) return { ok: false, error: '驱动没有输出 JSON（见 *-driver.err.log）' };
  if (json.handback) {
    return {
      ok: false,
      error: '桥接 handback：岛仔没有接管这条请求，请求被交回客户端（检查设置的「岛上处理审批与提问」与 DSH 来源）。'
        + `bridge=${JSON.stringify(json.bridge ?? []).slice(0, 800)}`,
    };
  }
  if (name === 'approval') {
    const entry = (json.bridge ?? []).find((item) => item.kind === 'approval');
    const reply = entry?.reply;
    if (json.ok && reply?.decision === 'allow' && json.decision === 'allowed-once') return { ok: true };
    return {
      ok: false,
      error: `桥接侧没有收到 allow（decision=${JSON.stringify(json.decision)}, reply=${JSON.stringify(reply)}, handback=${json.handback}）`,
    };
  }
  if (name === 'question' || name === 'question-multi-turn') {
    const answers = json.decision?.answers;
    if (!json.ok || !Array.isArray(answers) || answers.length === 0) {
      return { ok: false, error: `桥接侧没有收到有效回答（decision=${JSON.stringify(json.decision)}）` };
    }
    const expected = spec.expectedAnswers ?? [];
    for (let i = 0; i < expected.length; i++) {
      const selected = Array.isArray(answers[i]?.selected) ? answers[i].selected : [];
      if (!selected.includes(expected[i])) {
        return { ok: false, error: `第 ${i + 1} 题桥接答案 ${JSON.stringify(selected)} 与屏幕上点的「${expected[i]}」不一致` };
      }
    }
    return { ok: true };
  }
  if (name === 'done') {
    const entry = (json.bridge ?? []).find((item) => item.kind === 'finished');
    const textLength = entry?.sent?.text?.length ?? 0;
    if (json.ok && json.decision === 'done' && entry?.reply?.done && entry.mode === 2 && textLength > 0) return { ok: true };
    return {
      ok: false,
      error: `桥接侧没有确认完成事件（decision=${JSON.stringify(json.decision)}, mode=${entry?.mode}, textLen=${textLength}, reply=${JSON.stringify(entry?.reply)}）`,
    };
  }
  return { ok: false, error: `未知场景 ${name}` };
}

// ---------------------------------------------------------------- 常驻 PowerShell 辅助进程

// 输入/截图辅助脚本：stdin/stdout 逐行 JSON（op = ping/window/click/move/capture/lock/nudge/keepawake/exit）。
// 保持纯 ASCII（PowerShell 5.1 按代码页解析无 BOM 文件，中文会乱码），也不含反引号/${}，
// 这样嵌在 JS 模板字符串里不会被转义。DPI aware 在脚本启动时设置，坐标统一用物理像素。
const HELPER_PS = `# Daozai fast E2E input/capture helper. Driven by agent-e2e-fast.mjs over stdin/stdout lines.
# Request: one JSON object per line. Response: one JSON object per line (echoes id when present).
$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }
try { [Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$nativeSource = @'
using System;
using System.Runtime.InteropServices;
using System.Threading;
public static class DshE2eNative {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr param);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hwnd, ref POINT p);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint flags);
  [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
  [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr handle);
  [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] public static extern bool GetUserObjectInformationW(IntPtr handle, int index, System.Text.StringBuilder info, int length, out int needed);
  public static bool MakeDpiAware() {
    try { if (SetProcessDpiAwarenessContext(new IntPtr(-4))) { return true; } } catch { }
    try { if (SetProcessDpiAwarenessContext(new IntPtr(-2))) { return true; } } catch { }
    try { return SetProcessDPIAware(); } catch { return false; }
  }
  public static IntPtr FindWindowForProcess(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr hwnd, IntPtr param) {
      uint owner;
      GetWindowThreadProcessId(hwnd, out owner);
      if (owner != pid) { return true; }
      if (!IsWindowVisible(hwnd)) { return true; }
      RECT r;
      if (!GetWindowRect(hwnd, out r)) { return true; }
      if (r.Right - r.Left < 120 || r.Bottom - r.Top < 20) { return true; }
      found = hwnd;
      return false;
    }, IntPtr.Zero);
    return found;
  }
  public static bool Click(int x, int y) {
    if (!SetCursorPos(x, y)) { return false; }
    Thread.Sleep(30);
    mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
    Thread.Sleep(40);
    mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    Thread.Sleep(30);
    return true;
  }
  public static string InputDesktopName() {
    IntPtr handle = OpenInputDesktop(0, false, 0x0100);
    if (handle == IntPtr.Zero) { return ""; }
    try {
      System.Text.StringBuilder sb = new System.Text.StringBuilder(256);
      int needed;
      if (!GetUserObjectInformationW(handle, 2, sb, 512, out needed)) { return ""; }
      return sb.ToString();
    } finally {
      CloseDesktop(handle);
    }
  }
}
'@
Add-Type -TypeDefinition $nativeSource
$script:dpiAware = [DshE2eNative]::MakeDpiAware()
# 保持系统与显示器唤醒：ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
$null = [DshE2eNative]::SetThreadExecutionState([uint32]2147483651)

function Reply($payload) {
  [Console]::Out.WriteLine(($payload | ConvertTo-Json -Compress -Depth 6))
  [Console]::Out.Flush()
}

function Respond($req, $payload) {
  if ($null -ne $req -and $null -ne $req.id) { $payload['id'] = $req.id }
  Reply $payload
}

Reply @{ ok = $true; ready = $true; pid = $PID; dpiAware = $script:dpiAware; ps = $PSVersionTable.PSVersion.ToString() }
$running = $true
while ($running) {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line) { break }
  $line = $line.Trim()
  if ($line.Length -eq 0) { continue }
  $req = $null
  try {
    $req = $line | ConvertFrom-Json
    $op = [string]$req.op
    if ($op -eq 'ping') {
      Respond $req @{ ok = $true; ready = $true; pid = $PID }
    } elseif ($op -eq 'window') {
      $targetPid = [uint32]$req.pid
      $hwnd = [DshE2eNative]::FindWindowForProcess($targetPid)
      if ($hwnd -eq [IntPtr]::Zero) {
        Respond $req @{ ok = $false; error = 'no-window'; pid = $targetPid }
      } else {
        $wr = New-Object DshE2eNative+RECT
        $cr = New-Object DshE2eNative+RECT
        $pt = New-Object DshE2eNative+POINT
        $null = [DshE2eNative]::GetWindowRect($hwnd, [ref]$wr)
        $null = [DshE2eNative]::GetClientRect($hwnd, [ref]$cr)
        $pt.X = 0
        $pt.Y = 0
        $null = [DshE2eNative]::ClientToScreen($hwnd, [ref]$pt)
        $vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
        Respond $req @{
          ok = $true
          hwnd = [int64]$hwnd
          dpi = [int][DshE2eNative]::GetDpiForWindow($hwnd)
          dpiAware = $script:dpiAware
          window = @{ left = $wr.Left; top = $wr.Top; right = $wr.Right; bottom = $wr.Bottom }
          client = @{ left = $pt.X; top = $pt.Y; w = ($cr.Right - $cr.Left); h = ($cr.Bottom - $cr.Top) }
          screen = @{ left = $vs.Left; top = $vs.Top; right = $vs.Right; bottom = $vs.Bottom }
        }
      }
    } elseif ($op -eq 'click') {
      $ok = [DshE2eNative]::Click([int]$req.x, [int]$req.y)
      Respond $req @{ ok = $ok; x = [int]$req.x; y = [int]$req.y }
    } elseif ($op -eq 'move') {
      $ok = [DshE2eNative]::SetCursorPos([int]$req.x, [int]$req.y)
      Respond $req @{ ok = $ok; x = [int]$req.x; y = [int]$req.y }
    } elseif ($op -eq 'cursor') {
      $pt = New-Object DshE2eNative+POINT
      $ok = [DshE2eNative]::GetCursorPos([ref]$pt)
      Respond $req @{ ok = $ok; x = $pt.X; y = $pt.Y }
    } elseif ($op -eq 'nudge') {
      $null = [DshE2eNative]::SetCursorPos(2, 2)
      $null = [DshE2eNative]::SetCursorPos(4, 4)
      Respond $req @{ ok = $true }
    } elseif ($op -eq 'keepawake') {
      $null = [DshE2eNative]::SetThreadExecutionState([uint32]2147483651)
      Respond $req @{ ok = $true }
    } elseif ($op -eq 'lock') {
      $name = [DshE2eNative]::InputDesktopName()
      $logonui = @(Get-Process -Name LogonUI -ErrorAction SilentlyContinue).Count
      Respond $req @{ ok = $true; desktop = $name; locked = ($name -ne 'Default'); logonui = $logonui }
    } elseif ($op -eq 'capture') {
      $x = [int]$req.x
      $y = [int]$req.y
      $w = [int]$req.w
      $h = [int]$req.h
      $target = [string]$req.path
      if ($w -le 0 -or $h -le 0) { throw ('capture 区域为空: ' + $w + 'x' + $h) }
      $dir = [System.IO.Path]::GetDirectoryName($target)
      if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
      $bmp = New-Object System.Drawing.Bitmap($w, $h)
      $ink = 0
      try {
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        try {
          $g.CopyFromScreen($x, $y, 0, 0, (New-Object System.Drawing.Size($w, $h)), [System.Drawing.CopyPixelOperation]::SourceCopy)
        } finally { $g.Dispose() }
        # Sample the INTERIOR (skip a margin so desktop slivers at the crop edge do not count):
        # "ink" = number of bright samples (text/buttons), "spread" = luminance range.
        # LockBits + Marshal.Copy, NOT Bitmap.GetPixel: in Windows PowerShell 5.1 a nested
        # GetPixel loop is pathologically slow (600 calls took > 25s here, looked like a hang).
        $rect = New-Object System.Drawing.Rectangle(0, 0, $w, $h)
        $bits = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        try {
          $stride = $bits.Stride
          $pixels = New-Object byte[] ($stride * $h)
          [System.Runtime.InteropServices.Marshal]::Copy($bits.Scan0, $pixels, 0, $pixels.Length)
          $minL = 255
          $maxL = 0
          $bright = 0
          $mx = [int]($w * 0.08)
          $my = [int]($h * 0.08)
          $stepX = [Math]::Max(1, [int](($w - 2 * $mx) / 24))
          $stepY = [Math]::Max(1, [int](($h - 2 * $my) / 24))
          for ($sx = $mx; $sx -lt ($w - $mx); $sx += $stepX) {
            for ($sy = $my; $sy -lt ($h - $my); $sy += $stepY) {
              $o = $sy * $stride + $sx * 4
              $l = [int](([int]$pixels[$o] + [int]$pixels[$o + 1] + [int]$pixels[$o + 2]) / 3)
              if ($l -lt $minL) { $minL = $l }
              if ($l -gt $maxL) { $maxL = $l }
              if ($l -ge 140) { $bright = $bright + 1 }
            }
          }
          $ink = $bright
          $spread = $maxL - $minL
        } finally { $bmp.UnlockBits($bits) }
        $bmp.Save($target, [System.Drawing.Imaging.ImageFormat]::Png)
      } finally { $bmp.Dispose() }
      Respond $req @{ ok = $true; path = $target; x = $x; y = $y; w = $w; h = $h; ink = $ink; spread = $spread }
    } elseif ($op -eq 'exit') {
      Respond $req @{ ok = $true }
      $running = $false
    } else {
      Respond $req @{ ok = $false; error = ('unknown-op: ' + $op) }
    }
  } catch {
    Respond $req @{ ok = $false; error = $_.Exception.Message }
  }
}
`;

class WinHelper {
  constructor({ scriptPath, logger }) {
    this.scriptPath = scriptPath;
    this.logger = logger;
    this.child = null;
    this.buffer = '';
    this.seq = 0;
    this.pending = new Map();
    this.hello = null;
    this.stderrTail = '';
    this.ready = false;
  }

  async start({ timeoutMs = 15000 } = {}) {
    fs.writeFileSync(this.scriptPath, HELPER_PS, 'utf8');
    this.child = spawn(
      'powershell.exe',
      ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', this.scriptPath],
      { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true },
    );
    this.child.stdout.setEncoding('utf8');
    this.child.stdout.on('data', (chunk) => this._onData(chunk));
    this.child.stderr.setEncoding('utf8');
    this.child.stderr.on('data', (chunk) => {
      this.stderrTail = (this.stderrTail + chunk).slice(-4000);
    });
    this.child.on('error', (error) => this._failAll(new E2eError(EXIT.USAGE, `输入辅助进程启动失败：${error.message}`)));
    this.child.on('exit', (code) => {
      this.ready = false;
      this._failAll(new E2eError(EXIT.USAGE, `输入辅助进程退出（code=${code}）：${this.stderrTail.slice(-800)}`));
    });
    const hello = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.hello = null;
        reject(new E2eError(EXIT.USAGE, `输入辅助进程 ${timeoutMs}ms 内没有就绪：${this.stderrTail.slice(-800)}`));
      }, timeoutMs);
      this._helloResolver = { resolve, reject, timer };
    });
    if (!hello.ready) throw new E2eError(EXIT.USAGE, '输入辅助进程没有返回 ready');
    this.hello = hello;
    this.ready = true;
    return hello;
  }

  _onData(chunk) {
    this.buffer += chunk;
    for (;;) {
      const index = this.buffer.indexOf('\n');
      if (index < 0) break;
      const line = this.buffer.slice(0, index).trim();
      this.buffer = this.buffer.slice(index + 1);
      if (!line) continue;
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        this.logger?.warn(`输入辅助进程输出非 JSON：${line.slice(0, 200)}`);
        continue;
      }
      if (message.id === undefined) {
        const resolver = this._helloResolver;
        if (resolver) {
          clearTimeout(resolver.timer);
          this._helloResolver = null;
          resolver.resolve(message);
        }
        continue;
      }
      const entry = this.pending.get(message.id);
      if (!entry) continue;
      this.pending.delete(message.id);
      clearTimeout(entry.timer);
      entry.resolve(message);
    }
  }

  _failAll(error) {
    const resolver = this._helloResolver;
    if (resolver) {
      clearTimeout(resolver.timer);
      this._helloResolver = null;
      resolver.reject(error);
    }
    for (const [id, entry] of this.pending) {
      this.pending.delete(id);
      clearTimeout(entry.timer);
      entry.reject(error);
    }
  }

  async request(payload, { timeoutMs = 15000 } = {}) {
    if (!this.child || !this.ready) throw new E2eError(EXIT.USAGE, '输入辅助进程不可用');
    if (this.child.exitCode !== null) {
      throw new E2eError(EXIT.USAGE, `输入辅助进程已经退出（code=${this.child.exitCode}）：${this.stderrTail.slice(-800)}`);
    }
    const id = ++this.seq;
    const line = `${JSON.stringify({ ...payload, id })}\n`;
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new E2eError(EXIT.USAGE, `输入辅助进程请求超时（op=${payload.op}, ${timeoutMs}ms）`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      this.child.stdin.write(line, 'utf8', (error) => {
        if (!error) return;
        clearTimeout(timer);
        this.pending.delete(id);
        reject(new E2eError(EXIT.USAGE, `写入输入辅助进程失败：${error.message}`));
      });
    });
  }

  async stop() {
    this.ready = false;
    try {
      await this.request({ op: 'exit' }, { timeoutMs: 1500 });
    } catch {
      /* 收尾路径忽略 */
    }
    const child = this.child;
    if (!child) return;
    try {
      child.stdin?.end();
    } catch {
      /* 忽略 */
    }
    killTree(child.pid);
    await new Promise((resolve) => {
      if (child.exitCode !== null) return resolve();
      const timer = setTimeout(resolve, 1500);
      child.once('exit', () => {
        clearTimeout(timer);
        resolve();
      });
    });
  }
}

// ---------------------------------------------------------------- 岛仔视图（ui-map 消费者）

class IslandView {
  constructor({ mapFile, io, freshMs, logger, pid }) {
    this.mapFile = mapFile;
    this.io = io;
    this.freshMs = freshMs;
    this.logger = logger;
    this.pid = pid;
    this.win = null;
    this.anchor = null;
    this.reads = 0;
    this.lastAgeMs = null;
    this.lastLoggedSignature = null;
    this.driver = null; // 场景运行期间挂上宿主驱动：它初始化失败会退出，等卡片时顺带报清楚
  }

  /**
   * 读一帧 ui-map：JSON 解析失败会重读（原子写有毫秒级窗口）。
   * 注意：契约没有心跳字段，生产端「内容没变不重写」，所以静态屏的 mtime 会一直变旧；
   * 新鲜度只在「确认发生了变化之后」强制校验（见 waitMapChange），否则会误判。
   */
  async read() {
    let raw = null;
    let map = null;
    let lastError = null;
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        raw = await this.io.readMapFile(this.mapFile);
      } catch (error) {
        raw = null;
        lastError = error;
      }
      if (raw && typeof raw.text === 'string') {
        try {
          map = parseUiMapText(raw.text);
          const problems = validateUiMap(map);
          if (problems.length) throw new Error(problems.join('；'));
          break;
        } catch (error) {
          map = null;
          lastError = error;
        }
      }
      if (attempt < 2) await this.io.sleep(3);
    }
    if (!raw) {
      throw new E2eError(
        EXIT.UIMAP,
        `读不到 ui-map：${this.mapFile}（岛仔还没写出第一帧 / DAOZAI_UI_MAP 没生效）${lastError ? `；${lastError.message}` : ''}`,
        'missing',
      );
    }
    if (!map) {
      throw new E2eError(EXIT.UIMAP, `ui-map 不符合契约 schema=${MAP_SCHEMA}：${lastError?.message ?? '未知解析错误'}（文件：${this.mapFile}）`);
    }
    const ageMs = Math.max(0, nowMs() - raw.mtimeMs);
    this.reads++;
    this.lastAgeMs = ageMs;
    if (this.anchor === null) {
      this.anchor = { t: map.t, wall: nowMs() };
      this.logger?.debug(`ui-map 锚定：t=${map.t} scale=${map.scale} canvas=${map.canvas.w}x${map.canvas.h}`);
    } else if (map.t + 0.5 < this.anchor.t) {
      this.logger?.warn(`ui-map 的 t 回退（${this.anchor.t} → ${map.t}），岛仔可能重启过；重新锚定`);
      this.anchor = { t: map.t, wall: nowMs() };
    }
    // 只在「内容真的变了」时打 debug：--verbose 下轮询每秒几十次，逐条打会把日志淹掉（12KB+ 噪声）。
    const signature = signatureOf(map);
    if (this.lastLoggedSignature !== signature) {
      this.lastLoggedSignature = signature;
      this.logger?.debug(`ui-map 读 #${this.reads}：t=${map.t} age=${Math.round(ageMs)}ms ${describeMap(map)}`);
    }
    return { map, mtimeMs: raw.mtimeMs, ageMs };
  }

  /** 「确认发生变化」后的强制新鲜度校验（静态屏不校验，见 read 的注释）。 */
  assertFresh(ageMs) {
    if (ageMs > this.freshMs) {
      throw new E2eError(
        EXIT.UIMAP,
        `ui-map 不新鲜：这一帧变化后的文件 mtime 落后 ${ageMs}ms（阈值 ${this.freshMs}ms）：写入方卡住或时钟异常`,
      );
    }
  }

  /** 等 ui-map 文件出现且可解析（启动阶段用；会给生产端未接入的情况一句人话）。 */
  async waitMapFile({ timeoutMs = 15000 } = {}) {
    const started = nowMs();
    let lastError = null;
    for (;;) {
      try {
        return await this.read();
      } catch (error) {
        lastError = error;
        if (elapsed(started) > timeoutMs) {
          throw new E2eError(
            EXIT.UIMAP,
            `岛仔没有在 ${timeoutMs}ms 内写出可用的 ui-map（${this.mapFile}）。`
              + 'DAOZAI_UI_MAP 已经设置成数据目录，所以这通常是「该 exe 不含 src/ui_map.rs 接入」或生产端暂未完成；'
              + `契约见 docs/superpowers/research/2026-11-20-agent-ui-map-contract.md。最后一次错误：${lastError?.message}`,
          );
        }
      }
      await this.io.sleep(50);
    }
  }

  /** 按 PID 找窗口并缓存；窗口随卡片展开/收起变化，必要时重取。 */
  async refreshWindow({ timeoutMs = 15000 } = {}) {
    const started = nowMs();
    for (;;) {
      const info = await this.io.window(this.pid);
      if (info && info.ok !== false && info.client) {
        const previous = this.win;
        this.win = info;
        if (previous) {
          this.logger?.debug(
            `窗口更新：client=${info.client.w}x${info.client.h}@(${info.client.left},${info.client.top}) `
              + `window=${info.window.left},${info.window.top},${info.window.right},${info.window.bottom} dpi=${info.dpi} dpiAware=${info.dpiAware}`,
          );
        }
        return info;
      }
      if (elapsed(started) > timeoutMs) {
        throw new E2eError(
          EXIT.SCENARIO,
          `找不到岛仔窗口（PID ${this.pid}）${timeoutMs}ms：进程是否已退出？`
            + '（已有实例时单实例互斥体会让它激活旧实例后退出）',
        );
      }
      await this.io.sleep(50);
    }
  }

  /**
   * 坐标换算基准：客户区左上 + scale。正常 = map.scale（DPI = pixels_per_point）。
   * 兼容两种异常：窗口尺寸和 canvas 对不上（动画中/刚 resize → 重取一次）；
   * 辅助进程没成功设 DPI aware（客户区等于 canvas 逻辑尺寸 → 按 1 换算并告警）。
   */
  async geometry(map) {
    const close = (a, b, tol = 2) => Math.abs(a - b) <= tol;
    const expectW = map.canvas.w * map.scale;
    const expectH = map.canvas.h * map.scale;
    if (!this.win) await this.refreshWindow();
    let win = this.win;
    // 卡片展开/收起会让窗口 resize，动画期间会短暂不一致：有界重取几次再降级。
    for (let attempt = 0; attempt < 5; attempt++) {
      if (close(win.client.w, expectW) && close(win.client.h, expectH)) return { win, coordScale: map.scale };
      await this.refreshWindow();
      win = this.win;
      await this.io.sleep(20);
    }
    if (close(win.client.w, map.canvas.w) && close(win.client.h, map.canvas.h)) {
      this.logger?.warn(
        `辅助进程的客户区尺寸 ${win.client.w}x${win.client.h} 等于 canvas 逻辑尺寸：DPI aware 可能没生效，按 coordScale=1 换算（map.scale=${map.scale}）`,
      );
      return { win, coordScale: 1 };
    }
    const measured = win.client.w / map.canvas.w;
    this.logger?.warn(
      `ui-map canvas(${map.canvas.w}x${map.canvas.h})*scale(${map.scale}) 与窗口客户区(${win.client.w}x${win.client.h}) 不一致，按实测比例 ${measured.toFixed(3)} 换算`,
    );
    return { win, coordScale: measured };
  }

  /**
   * 轮询到 pred(map) 为真；超时抛带最后状态的错误（不写死 sleep）。
   * ui-map 文件还没出现是「卡片还没弹出来」的正常中间态（生产端只在有卡片/面板时落盘），
   * 所以继续等；schema/新鲜度这类真错误立刻抛。
   */
  async waitState(pred, { timeoutMs = 8000, label = '界面状态', intervalMs = 20 } = {}) {
    const started = nowMs();
    let lastError = null;
    for (;;) {
      let read = null;
      try {
        read = await this.read();
      } catch (error) {
        if (!(error instanceof E2eError && error.code === EXIT.UIMAP && error.detail === 'missing')) throw error;
        lastError = error;
      }
      if (read && pred(read.map, read)) return read;
      // 驱动初始化失败会自己退出（exit=2）：别把「宿主没起来」拖成「卡片没弹出来」的假象。
      if (this.driver?.exited && elapsed(started) > 200) {
        throw driverFailureMessage(this.driver.exit, this.driver);
      }
      if (elapsed(started) > timeoutMs) {
        if (lastError && !read) {
          throw new E2eError(
            EXIT.UIMAP,
            `等待「${label}」超时（${timeoutMs}ms），期间 ui-map 一直没有出现：${lastError.message}。`
              + 'DAOZAI_UI_MAP 已设置成数据目录却有 15s 以上没有落盘，通常是该 exe 不含 src/ui_map.rs 接入，或生产端（task-2）还没接完',
          );
        }
        throw new E2eError(EXIT.SCENARIO, `等待「${label}」超时（${timeoutMs}ms）：最后状态 ${describeMap(read.map)}`);
      }
      await this.io.sleep(intervalMs);
    }
  }

  /** 等「这一屏发生变化」（t 前进或节点/状态签名变化）——点击后的唯一等待方式。 */
  async waitMapChange(previous, { timeoutMs = 2000, label = '界面响应', intervalMs = 15 } = {}) {
    const started = nowMs();
    const previousSignature = signatureOf(previous.map);
    const previousT = previous.map.t;
    for (;;) {
      const read = await this.read();
      const changed = read.map.t > previousT + 1e-6 || signatureOf(read.map) !== previousSignature;
      if (changed) {
        this.assertFresh(read.ageMs);
        return { ...read, waitedMs: elapsed(started) };
      }
      if (elapsed(started) > timeoutMs) {
        throw new E2eError(EXIT.SCENARIO, `等待${label}超时（${timeoutMs}ms）：ui-map 没有任何变化；最后状态 ${describeMap(read.map)}`);
      }
      await this.io.sleep(intervalMs);
    }
  }

  /** 等某个节点出现（可要求 enabled≠false）。返回 {node, map}。 */
  async waitNode(id, { timeoutMs = 8000, label = null, enabled = false, optional = false } = {}) {
    try {
      const read = await this.waitState(
        (map) => {
          const node = findNode(map, id);
          if (!node) return false;
          return enabled ? node.enabled !== false : true;
        },
        { timeoutMs, label: label ?? `节点 ${id} 出现` },
      );
      return { node: findNode(read.map, id), map: read.map, read };
    } catch (error) {
      if (optional && error instanceof E2eError && error.code === EXIT.SCENARIO) return null;
      let available = '';
      try {
        available = nodeIds((await this.read()).map).join(', ');
      } catch {
        /* 读不到就不再追加 */
      }
      throw new E2eError(
        EXIT.SCENARIO,
        `ui-map 里找不到可用节点 ${id}：${error.message}；当前节点=[${available}]。`
          + '节点 id 见契约 docs/superpowers/research/2026-11-20-agent-ui-map-contract.md（确认卡片已展开、按钮确实在屏上）',
      );
    }
  }

  /** GDI 1:1 截图（全屏抓取后按 ui-map 的 card / layer.result 矩形裁剪）。 */
  async shot(outDir, name, { prefer = 'card', stable = true } = {}) {
    // 结果层展开时高度是动画增长的：先等布局稳定，否则会只截到「半张」结果层。
    const read = stable ? await this.waitStable({ timeoutMs: 800 }) : await this.read();
    const geo = await this.geometry(read.map);
    const rect = cropRectPx(read.map, geo.win, geo.coordScale, prefer);
    if (!rect || rect.w < 8 || rect.h < 8) {
      this.logger?.warn(`跳过截图 ${name}：裁剪区域为空（${JSON.stringify(rect)}）`);
      return null;
    }
    const file = path.join(outDir, `${sanitizeFileName(name)}.png`);
    let result = null;
    let verified = null;
    // 卡片/结果层有淡入动画：刚打开的那一帧可能截到纯色。按「亮度跨度 + 亮点数」判空并重试（最多 4 次），
    // 仍然为空就保留文件但告警（证据质量不该让功能断言失败）。
    // 判定用 ink（无文字时 ≈0）：只看 spread 会被裁剪边缘带进来的桌面像素骗过。
    for (let attempt = 0; attempt < 4; attempt++) {
      if (this.io.nudge && this.io.nudgeEnabled !== false) await this.io.nudge();
      result = await this.io.capture(file, rect);
      if (result && result.ok === false) throw new E2eError(EXIT.SCENARIO, `截图失败（${name}）：${result.error}`);
      verified = await this.io.verifyPng(file);
      if (!verified.ok) throw new E2eError(EXIT.SCENARIO, `截图文件无效（${name}）：${verified.error}（${file}）`);
      const ink = typeof result?.ink === 'number' ? result.ink : null;
      if (ink === null || ink >= 4) break; // 至少 4 个亮点 = 有文字/按钮
      if (attempt < 3) await this.io.sleep(80);
    }
    if (result && typeof result.ink === 'number' && result.ink < 4) {
      this.logger?.warn(`截图 ${name} 内容近乎纯色（ink=${result.ink}, spread=${result.spread}）：可能是淡入动画瞬间或裁剪区域为空，已保留文件 ${file}`);
    }
    this.logger?.debug(`截图 ${file}（${rect.w}x${rect.h} @ ${rect.left},${rect.top}, ink=${result?.ink ?? 'n/a'}, spread=${result?.spread ?? 'n/a'}, ${verified.bytes} bytes）`);
    return { path: file, rect, bytes: verified.bytes, ink: result?.ink ?? null, spread: result?.spread ?? null };
  }

  /**
   * 等「这一屏稳定下来」：连续两次读到的状态签名一致且保持了 holdMs。
   * 卡片/结果层有淡入+位移动画，动画帧里读到的 rect 点下去可能已经挪位置了。
   */
  async waitStable({ timeoutMs = 600, holdMs = 60 } = {}) {
    const started = nowMs();
    let signature = null;
    let since = 0;
    for (;;) {
      const read = await this.read();
      const current = signatureOf(read.map);
      if (current !== signature) {
        signature = current;
        since = nowMs();
      } else if (nowMs() - since >= holdMs) {
        return read;
      }
      if (elapsed(started) > timeoutMs) return read;
      await this.io.sleep(20);
    }
  }

  /**
   * 点一个节点：等布局稳定 → 取矩形中心 → 真鼠标点击（SetCursorPos + mouse_event）→ 等 ui-map 变化。
   * - requireChange=true：关键按钮（允许/下一题/确认/关闭）没有效果直接失败，且只点一次（防重复提交）；
   * - requireChange=false：布局动画中可能点空，允许「重读布局后重试一次」（只在不产生任何界面变化时）。
   */
  async clickNode(id, { settleMs = 1200, requireChange = false, label = null, shotName = null, shotDir = null, prefer = 'card' } = {}) {
    const maxAttempts = requireChange ? 1 : 2;
    let last = null;
    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      const read = await this.waitStable();
      const node = findNode(read.map, id);
      if (!node) {
        throw new E2eError(EXIT.SCENARIO, `ui-map 里没有节点 ${id}（当前节点=[${nodeIds(read.map).join(', ')}]）`);
      }
      if (node.enabled === false) {
        throw new E2eError(EXIT.SCENARIO, `节点 ${id} 当前不可用（enabled=false，label=${JSON.stringify(node.label ?? null)}）`);
      }
      const geo = await this.geometry(read.map);
      const point = nodeScreenPoint(geo.win.client.left, geo.win.client.top, geo.coordScale, node.rect);
      const shot = attempt === 0 && shotName && shotDir ? await this.shot(shotDir, shotName, { prefer, stable: false }) : null;
      const started = nowMs();
      const result = await this.io.click(point.x, point.y);
      if (result && result.ok === false) throw new E2eError(EXIT.SCENARIO, `真鼠标点击失败（${id} @ ${point.x},${point.y}）`);
      try {
        const change = await this.waitMapChange(read, { timeoutMs: settleMs, label: `点击「${label ?? id}」后的界面变化` });
        return { node, point, shot, changed: true, waitedMs: change.waitedMs, attempts: attempt + 1 };
      } catch (error) {
        if (requireChange) throw error;
        last = { node, point, shot, changed: false, waitedMs: elapsed(started), attempts: attempt + 1 };
        if (attempt + 1 < maxAttempts) {
          this.logger?.debug(`点击「${label ?? id}」没有引起界面变化（疑似淡入动画中布局位移），重读布局后重试一次`);
          await this.io.sleep(30);
        }
      }
    }
    return last;
  }
}

// ---------------------------------------------------------------- 宿主驱动

class Driver {
  constructor({ name, scenario, request, dshExe, profile, waitMs, cwd, shots, env, logger }) {
    this.name = name;
    this.scenario = scenario;
    this.request = request;
    this.dshExe = dshExe;
    this.profile = profile;
    this.waitMs = waitMs;
    this.cwd = cwd;
    this.shots = shots;
    this.env = env;
    this.logger = logger;
    this.child = null;
    this.exit = null;
    this.startError = null;
    this.jsonIn = path.join(shots, `${name}-driver-input.json`);
    this.stdoutFile = path.join(shots, `${name}-driver.json`);
    this.stderrFile = path.join(shots, `${name}-driver.err.log`);
  }

  get exited() {
    return !!this.exit || !!this.startError;
  }

  start() {
    // 明确的前置校验：dshExe 为空/路径不存在时要报「驱动起不来」，不要让 spawn(null) 的 TypeError
    // 把真正的场景错误盖掉（真机联调第一轮就踩了这个：opts.dshExe 是 null，TypeError 覆盖了全部信息）。
    if (typeof this.dshExe !== 'string' || this.dshExe.length === 0) {
      this.startError = new Error('dshExe 为空（内部错误：没有把解析好的 DeepSeek Harness 路径传进 Driver）');
      throw new E2eError(EXIT.DRIVER, `启动宿主驱动失败：${this.startError.message}`);
    }
    if (!fs.existsSync(this.dshExe)) {
      this.startError = new Error(`找不到 DeepSeek Harness：${this.dshExe}`);
      throw new E2eError(EXIT.DRIVER, `启动宿主驱动失败：${this.startError.message}`);
    }
    if (!fs.existsSync(DRIVE_SCRIPT)) {
      this.startError = new Error(`找不到驱动脚本：${DRIVE_SCRIPT}`);
      throw new E2eError(EXIT.DRIVER, `启动宿主驱动失败：${this.startError.message}`);
    }
    fs.writeFileSync(this.jsonIn, `${JSON.stringify(this.request, null, 2)}\n`, 'utf8');
    const out = fs.openSync(this.stdoutFile, 'w');
    const err = fs.openSync(this.stderrFile, 'w');
    try {
      this.child = spawn(
        this.dshExe,
        [DRIVE_SCRIPT, '--profile', this.profile, '--scenario', this.scenario, '--wait', String(this.waitMs), '--json-in', this.jsonIn],
        { cwd: this.cwd, env: { ...process.env, ...this.env }, stdio: ['ignore', out, err], windowsHide: true },
      );
    } catch (error) {
      this.startError = error;
      throw new E2eError(EXIT.DRIVER, `启动宿主驱动失败（${this.dshExe}）：${error.message}`);
    } finally {
      fs.closeSync(out);
      fs.closeSync(err);
    }
    this.child.on('exit', (code, signal) => {
      this.exit = { code, signal, at: nowMs() };
    });
    this.child.on('error', (error) => {
      this.startError = error;
    });
    this.logger.log(`  驱动已启动 PID=${this.child.pid}（场景 ${this.scenario}）`);
  }

  async waitExit({ timeoutMs = 60000 } = {}) {
    const started = nowMs();
    while (!this.exited && elapsed(started) < timeoutMs) await sleep(50);
    return this.exit ?? (this.startError ? { code: null, signal: null, error: this.startError.message } : null);
  }

  async readJson({ timeoutMs = 5000 } = {}) {
    const deadline = nowMs() + timeoutMs;
    for (;;) {
      try {
        const text = fs.readFileSync(this.stdoutFile, 'utf8');
        const line = text.split(/\r?\n/).map((item) => item.trim()).filter((item) => item.startsWith('{')).pop();
        if (line) {
          try {
            return JSON.parse(line);
          } catch {
            /* 正在写，重试 */
          }
        }
      } catch {
        /* 文件还没落盘，重试 */
      }
      if (nowMs() > deadline) {
        throw new E2eError(EXIT.DRIVER, `驱动没有输出 JSON（${this.stdoutFile}，stderr 见 ${this.stderrFile}）`);
      }
      await sleep(100);
    }
  }

  stderrTail(max = 800) {
    try {
      return fs.readFileSync(this.stderrFile, 'utf8').slice(-max).trim();
    } catch {
      return '';
    }
  }

  kill() {
    if (this.child?.pid) killTree(this.child.pid);
  }
}

// ---------------------------------------------------------------- 场景（只依赖 IslandView + steps，自测可直接复用）

function step(steps, name, extra = {}) {
  const item = { step: name, ok: extra.ok !== false, ...extra };
  steps.push(item);
  return item;
}

/** 卡片若还是收起态（banner），先点「查看」把它展开。 */
async function ensureCardExpanded(ctx, kind) {
  const { island, steps, timeouts } = ctx;
  const read = await island.read();
  if (read.map.state?.card?.details === true) {
    step(steps, '卡片详情已展开（无需点「查看」）', { state: summarizeState(read.map) });
    return read.map;
  }
  step(steps, '卡片是收起态，先点「查看」展开', { state: summarizeState(read.map) });
  await island.clickNode('btn.view', {
    requireChange: false,
    settleMs: timeouts.settle,
    label: '查看',
    shotName: `${kind}-01-expand`,
    shotDir: ctx.shots,
  });
  const expanded = await island.waitState((map) => map.state?.card?.details === true, {
    timeoutMs: timeouts.node,
    label: '卡片展开（state.card.details=true）',
  });
  step(steps, '卡片已展开', { state: summarizeState(expanded.map) });
  return expanded.map;
}

async function scenarioApproval(ctx) {
  const { island, steps, shots, timeouts } = ctx;
  await island.waitState((map) => map.state?.card?.kind === 'approval', {
    timeoutMs: timeouts.card,
    label: '审批卡片出现（state.card.kind=approval）',
  });
  step(steps, '等到审批卡片', { state: summarizeState((await island.read()).map) });

  const map = await ensureCardExpanded(ctx, 'approval');
  const operationNode = findNode(map, 'body.operation');
  step(steps, '审批卡包含操作内容区（body.operation）', {
    ok: !!operationNode,
    fatal: false,
    label: operationNode?.label ?? null,
  });

  const allow = await island.waitNode('btn.allow', { timeoutMs: timeouts.node, enabled: true, label: '「仅允许这一次」按钮' });
  step(steps, '找到「仅允许这一次」按钮', { node: 'btn.allow', label: allow.node.label ?? null });

  const click = await island.clickNode('btn.allow', {
    requireChange: true,
    settleMs: timeouts.settle * 2,
    label: '仅允许这一次',
    shotName: 'approval-02-allow',
    shotDir: shots,
  });
  step(steps, '点击「仅允许这一次」', {
    node: 'btn.allow',
    rect: allow.node.rect,
    screen: click.point,
    waitedMs: click.waitedMs,
    shot: click.shot?.path ?? null,
  });

  const after = await island.waitState((m) => !m.state?.card || m.state.card.submitting === true, {
    timeoutMs: timeouts.node,
    label: '审批提交后卡片收起（card=null 或 submitting=true）',
  });
  step(steps, '提交后 ui-map 状态变化（卡片收起/提交中）', { state: summarizeState(after.map) });
}

async function scenarioQuestion(ctx) {
  const { island, steps, shots, timeouts } = ctx;
  await island.waitState((map) => map.state?.card?.kind === 'question', {
    timeoutMs: timeouts.card,
    label: '提问卡片出现（state.card.kind=question）',
  });
  step(steps, '等到提问卡片', { state: summarizeState((await island.read()).map) });

  await ensureCardExpanded(ctx, 'question');
  const option = await island.waitNode('option.0', { timeoutMs: timeouts.node, enabled: true, label: '选项 0' });
  step(steps, '找到第 1 个选项（option.0）', { label: option.node.label ?? null });

  const optionClick = await island.clickNode('option.0', {
    requireChange: false,
    settleMs: timeouts.settle,
    label: option.node.label ?? 'option.0',
    shotName: 'question-02-option-0',
    shotDir: shots,
  });
  step(steps, '点击第 1 个选项', {
    label: option.node.label ?? null,
    screen: optionClick.point,
    changed: optionClick.changed,
    waitedMs: optionClick.waitedMs,
    shot: optionClick.shot?.path ?? null,
  });

  const submit = await island.waitNode('btn.submit', { timeoutMs: timeouts.node, enabled: true, label: '提交/检查答案按钮' });
  const submitClick = await island.clickNode('btn.submit', {
    requireChange: false,
    settleMs: timeouts.settle * 1.5,
    label: submit.node.label ?? 'btn.submit',
    shotName: 'question-03-submit',
    shotDir: shots,
  });
  step(steps, '点击提交类按钮', {
    label: submit.node.label ?? null,
    screen: submitClick.point,
    changed: submitClick.changed,
    shot: submitClick.shot?.path ?? null,
  });

  // 单题：可能直接提交，也可能进「检查答案 → 确认提交」审阅页；两种都兼容。
  const confirm = await island.waitNode('btn.confirm', { timeoutMs: timeouts.review, optional: true, enabled: true });
  if (confirm) {
    const confirmClick = await island.clickNode('btn.confirm', {
      requireChange: true,
      settleMs: timeouts.settle * 2,
      label: confirm.node.label ?? 'btn.confirm',
      shotName: 'question-04-confirm',
      shotDir: shots,
    });
    step(steps, '审阅页点击「确认提交」', {
      label: confirm.node.label ?? null,
      screen: confirmClick.point,
      waitedMs: confirmClick.waitedMs,
      shot: confirmClick.shot?.path ?? null,
    });
  } else {
    step(steps, '没有审阅页（提交即生效）', { fatal: false });
  }

  const after = await island.waitState((m) => !m.state?.card || m.state.card.submitting === true, {
    timeoutMs: timeouts.node,
    label: '提问提交后卡片收起（card=null 或 submitting=true）',
  });
  step(steps, '提交后 ui-map 状态变化', { state: summarizeState(after.map) });
}

async function scenarioQuestionMultiTurn(ctx) {
  const { island, steps, shots, timeouts } = ctx;
  await island.waitState((map) => map.state?.card?.kind === 'question', {
    timeoutMs: timeouts.card,
    label: '多轮提问卡片出现',
  });
  step(steps, '等到多轮提问卡片', { state: summarizeState((await island.read()).map) });

  await ensureCardExpanded(ctx, 'question');

  const first = await island.waitNode('option.0', { timeoutMs: timeouts.node, enabled: true, label: '第 1 题选项 0' });
  const firstClick = await island.clickNode('option.0', {
    requireChange: false,
    settleMs: timeouts.settle,
    label: first.node.label ?? 'option.0',
    shotName: 'question-multi-turn-02-option-1',
    shotDir: shots,
  });
  step(steps, '点击第 1 题选项', {
    label: first.node.label ?? null,
    screen: firstClick.point,
    changed: firstClick.changed,
    shot: firstClick.shot?.path ?? null,
  });

  const next = await island.waitNode('btn.next', { timeoutMs: 2500, enabled: true, optional: true, label: '「下一题」按钮' });
  if (next) {
    const nextClick = await island.clickNode('btn.next', {
      requireChange: true,
      settleMs: timeouts.settle * 1.5,
      label: next.node.label ?? '下一题',
      shotName: 'question-multi-turn-03-next',
      shotDir: shots,
    });
    step(steps, '点击「下一题」', {
      label: next.node.label ?? null,
      screen: nextClick.point,
      waitedMs: nextClick.waitedMs,
      shot: nextClick.shot?.path ?? null,
    });
  } else {
    // 兼容「没有下一题按钮、回答完自动进下一题」的 UI 行为；两者都没有才算失败。
    const auto = await island
      .waitState((map) => map.state?.card?.index === 1, { timeoutMs: 1500, label: '自动切到第 2 题' })
      .catch(() => null);
    if (!auto) {
      throw new E2eError(
        EXIT.SCENARIO,
        '既没有「下一题」按钮（btn.next）也没有自动切到第 2 题（state.card.index 仍不是 1）：'
          + '请确认契约节点 btn.next 已接入，或 UI 行为已变化',
      );
    }
    step(steps, '没有「下一题」按钮：UI 自动切到第 2 题', { state: summarizeState(auto.map) });
  }

  const secondPage = await island.waitState((map) => map.state?.card?.index === 1, {
    timeoutMs: timeouts.node,
    label: '切到第 2 题（state.card.index=1）',
  });
  step(steps, 'ui-map 确认已切到第 2 题（index=1）', { state: summarizeState(secondPage.map) });

  const second = await island.waitNode('option.0', { timeoutMs: timeouts.node, enabled: true, label: '第 2 题选项 0' });
  const secondClick = await island.clickNode('option.0', {
    requireChange: false,
    settleMs: timeouts.settle,
    label: second.node.label ?? 'option.0',
    shotName: 'question-multi-turn-04-option-2',
    shotDir: shots,
  });
  step(steps, '点击第 2 题选项', {
    label: second.node.label ?? null,
    screen: secondClick.point,
    changed: secondClick.changed,
    shot: secondClick.shot?.path ?? null,
  });

  const submit = await island.waitNode('btn.submit', { timeoutMs: timeouts.node, enabled: true, label: '「检查答案」按钮' });
  const submitClick = await island.clickNode('btn.submit', {
    requireChange: false,
    settleMs: timeouts.settle * 1.5,
    label: submit.node.label ?? 'btn.submit',
    shotName: 'question-multi-turn-05-submit',
    shotDir: shots,
  });
  step(steps, '点击「检查答案」', {
    label: submit.node.label ?? null,
    screen: submitClick.point,
    changed: submitClick.changed,
    shot: submitClick.shot?.path ?? null,
  });

  const confirm = await island.waitNode('btn.confirm', { timeoutMs: timeouts.review, label: '审阅页「确认提交」' });
  const confirmClick = await island.clickNode('btn.confirm', {
    requireChange: true,
    settleMs: timeouts.settle * 2,
    label: confirm.node.label ?? '确认提交',
    shotName: 'question-multi-turn-06-confirm',
    shotDir: shots,
  });
  step(steps, '审阅页点击「确认提交」', {
    label: confirm.node.label ?? null,
    screen: confirmClick.point,
    waitedMs: confirmClick.waitedMs,
    shot: confirmClick.shot?.path ?? null,
  });

  const after = await island.waitState((m) => !m.state?.card || m.state.card.submitting === true, {
    timeoutMs: timeouts.node,
    label: '提交后卡片收起（card=null 或 submitting=true）',
  });
  step(steps, '提交后 ui-map 状态变化', { state: summarizeState(after.map) });
}

async function scenarioDone(ctx) {
  const { island, steps, shots, timeouts } = ctx;
  await island.waitState((map) => map.state?.card?.kind === 'done', {
    timeoutMs: timeouts.card,
    label: '完成卡出现（state.card.kind=done）',
  });
  step(steps, '等到完成通知（完成卡）', { state: summarizeState((await island.read()).map) });

  const read = await island.read();
  if (findNode(read.map, 'btn.view')) {
    const viewClick = await island.clickNode('btn.view', {
      requireChange: false,
      settleMs: timeouts.settle,
      label: '查看回复',
      shotName: 'done-02-view',
      shotDir: shots,
    });
    step(steps, '点击「查看回复」', {
      screen: viewClick.point,
      changed: viewClick.changed,
      waitedMs: viewClick.waitedMs,
      shot: viewClick.shot?.path ?? null,
    });
  } else {
    step(steps, '完成卡没有 btn.view（可能已直接展开结果层）', { fatal: false, state: summarizeState(read.map) });
  }

  // 结果层打开是「证据/增强断言」，不阻断：轻提示卡（btn.dismiss）或生产端还没接 result 状态时
  // 仍然可以点完成卡自己的「关闭」，主断言是「关闭后一次收起」。
  const opened = await island
    .waitState((map) => isResultLayerOpen(map), {
      timeoutMs: Math.min(timeouts.node, 3000),
      label: '结果层打开（state.result.open / layer.result / btn.copy）',
    })
    .catch((error) => {
      steps.push({ step: '结果层没有打开（继续用完成卡的「关闭」验证收起）', ok: false, fatal: false, error: error.message });
      return null;
    });
  if (opened) {
    step(steps, '结果层打开', { state: summarizeState(opened.map) });
    step(steps, '结果层是本轮回复（不是本机说明层）', {
      ok: opened.map.state?.result?.hint !== true,
      fatal: false,
      title: opened.map.state?.result?.title ?? null,
    });
    const resultShot = await island.shot(shots, 'done-03-result', { prefer: 'result' });
    step(steps, '结果层截图（layer.result 矩形裁剪）', { ok: !!resultShot, fatal: false, shot: resultShot?.path ?? null });
  }

  const close = await island.waitNode('btn.close', { timeoutMs: timeouts.node, label: '「关闭」按钮' });
  step(steps, '找到「关闭」按钮', { label: close.node.label ?? null });

  const closeClick = await island.clickNode('btn.close', {
    requireChange: true,
    settleMs: timeouts.settle * 2,
    label: close.node.label ?? '关闭',
    shotName: 'done-04-close',
    shotDir: shots,
  });
  step(steps, '点击「关闭」', {
    screen: closeClick.point,
    waitedMs: closeClick.waitedMs,
    shot: closeClick.shot?.path ?? null,
  });

  const collapsed = await island.waitState(
    (map) => !map.state?.card && !map.state?.panel && !map.state?.result,
    { timeoutMs: timeouts.collapse, label: '关闭后一次收起（card=null 且 panel=false 且 result=null）' },
  );
  step(steps, '关闭后一次收起：state.card=null / panel=false / result=null', { state: summarizeState(collapsed.map) });

  await island.io.sleep(250);
  const after = await island.read();
  step(steps, '收起后没有再次弹出卡片', {
    ok: !after.map.state?.card && !after.map.state?.panel,
    state: summarizeState(after.map),
  });
}

const SCENARIO_FUNCS = {
  approval: scenarioApproval,
  question: scenarioQuestion,
  'question-multi-turn': scenarioQuestionMultiTurn,
  done: scenarioDone,
};

// ---------------------------------------------------------------- 单场景执行

async function waitCardCleared(island, logger, timeoutMs = 1200) {
  try {
    await island.waitState((map) => !map.state?.card, { timeoutMs, label: '上一场景卡片退场（card=null）' });
    return true;
  } catch (error) {
    logger.warn(`上一场景的卡片没有在超时内退场：${error.message}`);
    return false;
  }
}

function driverFailureMessage(exit, driver) {
  const stderr = driver ? driver.stderrTail() : '';
  const code = exit?.code;
  if (code === 2) {
    return new E2eError(
      EXIT.DRIVER,
      `驱动初始化失败（exit=2）：profile/插件/live.json 未就绪。检查岛仔是否导出了 DSH kit、设置的 DSH 来源是否打开。${stderr ? `stderr：${stderr}` : ''}`,
    );
  }
  if (code === 4) {
    return new E2eError(EXIT.SCENARIO, `桥接 handback（exit=4）：岛仔没有接管这条请求，被交回客户端。${stderr}`);
  }
  if (code === 3) {
    return new E2eError(EXIT.SCENARIO, `驱动等待超时（exit=3）：屏幕侧没有在限定时间内完成作答。${stderr}`);
  }
  return new E2eError(EXIT.SCENARIO, `驱动异常退出（exit=${code}，signal=${exit?.signal ?? null}）：${stderr || '没有 stderr'}`);
}

async function runScenario(name, ctx) {
  const { logger, island, shots, opts, timeouts } = ctx;
  const spec = SCENARIOS[name];
  const started = nowMs();
  const steps = [];
  const verdict = {
    scenario: name,
    driverScenario: spec.driver,
    ok: false,
    steps,
    driver: null,
    bridgeOk: false,
    error: null,
    duration: 0,
    durationMs: 0,
    driverStartupMs: null,
    interactiveMs: 0,
    shotCount: 0,
  };
  let driver = null;
  logger.log(`===== 场景 ${name}（驱动场景 ${spec.driver}）=====`);
  try {
    // 第一个场景是全新数据目录，不可能有上一张卡片；而且启动阶段 ui-map 本来就可能还没落盘，
    // 不必在这里空等（浪费 1.2s 还会刷一条误导性的 warning）。
    if (ctx.scenarios.length > 0) await waitCardCleared(island, logger);
    await island.refreshWindow();

    driver = new Driver({
      name,
      scenario: spec.driver,
      request: spec.request,
      dshExe: ctx.dshExe ?? resolveDshExe(opts), // 真机第一轮踩过：这里用 opts.dshExe（null）→ spawn(null) TypeError
      profile: path.join(ctx.dataDir, 'dsh-home', 'profiles', 'desktop'),
      waitMs: opts.waitSeconds * 1000,
      cwd: REPO,
      shots,
      env: { ELECTRON_RUN_AS_NODE: '1', DSH_HOME: path.join(ctx.dataDir, 'dsh-home') },
      logger,
    });
    // 量一下「驱动冷启动」：性能预算 30s 不含这段（每个场景都要起一次真实宿主）。
    let baseline = null;
    try {
      baseline = await island.read();
    } catch {
      /* 首场景可能还没有 ui-map（没有卡片时不落盘），拿下面的「第一次出现」当冷启动终点 */
    }
    const driverStartAt = nowMs();
    driver.start();
    island.driver = driver; // 驱动初始化失败会立即退出：等卡片时顺带盯它，别把初始化问题报成「UI 没弹卡」
    const startupBudgetMs = Math.min(timeouts.card, 15000);
    try {
      if (baseline) {
        await island.waitMapChange(baseline, { timeoutMs: startupBudgetMs, label: '驱动冷启动后界面变化' });
        verdict.driverStartupMs = elapsed(driverStartAt);
      } else {
        await island.waitMapFile({ timeoutMs: startupBudgetMs });
        verdict.driverStartupMs = elapsed(driverStartAt);
      }
    } catch (error) {
      // 冷启动没测到就交给下面的场景函数给出具体错误（它自己会等卡片并报状态）。
      logger.warn(`没有测到驱动冷启动完成：${error.message}`);
    }

    await SCENARIO_FUNCS[name]({ island, steps, shots, timeouts, opts, dataDir: ctx.dataDir });

    const exit = await driver.waitExit({ timeoutMs: (opts.waitSeconds + 5) * 1000 });
    if (!exit) {
      driver.kill();
      throw new E2eError(EXIT.SCENARIO, `驱动在 ${opts.waitSeconds + 5}s 内没有退出（卡片一直没作答？）`);
    }
    const json = await driver.readJson();
    verdict.driver = json;
    const bridge = checkBridge(name, json, spec);
    verdict.bridgeOk = bridge.ok;
    if (!bridge.ok) throw new E2eError(EXIT.SCENARIO, bridge.error);
    verdict.ok = true;
  } catch (error) {
    verdict.error = error?.message ?? String(error);
    verdict.errorCode = error instanceof E2eError ? error.code : EXIT.SCENARIO;
    // 失败原因不能被二次故障覆盖：把完整堆栈留给 --verbose（上一次 TypeError 就是没堆栈才难查）。
    if (opts.verbose) logger.warn(`场景 ${name} 失败堆栈：\n${error?.stack ?? error}`);
    // 失败现场：先把驱动收掉，再截一张整窗图（尽力而为）。
    try {
      if (driver && !driver.exited) {
        await driver.waitExit({ timeoutMs: 3000 });
        if (!driver.exited) driver.kill();
      }
      if (driver?.child) {
        try {
          verdict.driver = await driver.readJson({ timeoutMs: 2000 });
        } catch {
          /* 保留 driver=null */
        }
      }
      const failShot = await island.shot(shots, `${name}-fail-window`, { prefer: 'card' });
      verdict.failShot = failShot?.path ?? null;
    } catch (inner) {
      logger.warn(`失败现场收集失败：${inner.message}`);
    }
  } finally {
    island.driver = null;
    if (driver && !driver.exited) driver.kill();
    verdict.durationMs = elapsed(started);
    verdict.duration = Math.round(verdict.durationMs / 100) / 10;
    verdict.driverStartupMs = verdict.driverStartupMs ?? null;
    verdict.interactiveMs = Math.max(0, verdict.durationMs - (verdict.driverStartupMs ?? 0));
    verdict.shotCount = steps.filter((item) => item.shot).length;
  }
  return verdict;
}

// ---------------------------------------------------------------- 前置检查 / 编排

function resolveDshExe(opts) {
  if (opts.dshExe) return path.resolve(opts.dshExe);
  return path.join(process.env.LOCALAPPDATA ?? path.join(os.homedir(), 'AppData', 'Local'), 'Programs', 'DeepSeek Harness', 'DeepSeek Harness.exe');
}

function preflight({ opts, logger }) {
  const dshExe = resolveDshExe(opts);
  if (!fs.existsSync(dshExe)) throw new E2eError(EXIT.USAGE, `找不到 DeepSeek Harness：${dshExe}`);
  const exe = path.resolve(opts.exe ?? DEFAULT_EXE);
  if (!fs.existsSync(exe)) {
    throw new E2eError(
      EXIT.USAGE,
      `找不到岛仔 exe：${exe}\n先构建：powershell -NoProfile -Command ". .\\scripts\\dev-msvc-env.ps1; cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc"`,
    );
  }
  if (!fs.existsSync(DRIVE_SCRIPT)) throw new E2eError(EXIT.USAGE, `缺少驱动脚本：${DRIVE_SCRIPT}`);

  const running = listProcesses().filter((proc) => /^daozai/i.test(proc.name));
  if (running.length) {
    throw new E2eError(
      EXIT.INSTANCE,
      `已有岛仔在运行（PID ${running.map((proc) => `${proc.pid}:${proc.name}`).join(', ')}），单实例互斥体会互踢。\n`
        + '请先退出现有实例（Stop-Process -Id <pid>）再跑 E2E；脚本不会动别人的岛仔。',
    );
  }
  logger.log(`DSH 宿主：${dshExe}`);
  logger.log(`岛仔 exe：${exe}`);
  return { dshExe, exe };
}

/** 锁屏检测：输入桌面不是 Default（或打不开）就是锁定状态。LogonUI 进程只作为参考。 */
function lockError(info) {
  if (!info || info.ok === false) return null;
  if (info.desktop === 'Default') return null;
  const desktop = info.desktop || '不可读（OpenInputDesktop 失败）';
  return new E2eError(
    EXIT.LOCKED,
    `屏幕已锁定（输入桌面 = ${desktop}${info.logonui ? `，LogonUI 进程数 ${info.logonui}` : ''}）。`
      + '锁屏时点击与截图都会被安全桌面挡住；请解锁后重跑。',
  );
}

async function waitBridgeReady({ dataDir, islandProc, logger, timeoutMs = 30000 }) {
  const live = path.join(dataDir, 'agent-notices', 'bridge', 'live.json');
  const started = nowMs();
  for (;;) {
    if (islandProc.exitCode !== null) {
      throw new E2eError(EXIT.SCENARIO, `岛仔启动后退出（exit=${islandProc.exitCode}）：exe 起不来或单实例互斥体被占用`);
    }
    try {
      const entry = JSON.parse(fs.readFileSync(live, 'utf8'));
      if (entry?.policy?.dsh) {
        logger.log(`桥接就绪：${live}`);
        return entry;
      }
    } catch {
      /* 还没写，重试 */
    }
    if (elapsed(started) > timeoutMs) {
      throw new E2eError(
        EXIT.SCENARIO,
        `岛仔没有在 ${timeoutMs / 1000}s 内发布桥接通道（${live}）：检查设置里 DSH 来源是否打开、exe 是否能正常启动`,
      );
    }
    await sleep(50);
  }
}

function realIo({ helper, islandProc, opts, logger }) {
  return {
    pid: islandProc.pid,
    logger,
    // 默认不动用户光标：只有 --nudge 才在截图前抖一下鼠标唤醒合成器。
    nudgeEnabled: !!opts.nudge,
    async readMapFile(file) {
      try {
        const stat = await fsp.stat(file);
        const text = await fsp.readFile(file, 'utf8');
        return { text, mtimeMs: stat.mtimeMs };
      } catch (error) {
        if (error.code === 'ENOENT') return null;
        throw error;
      }
    },
    async window(pid) {
      return await helper.request({ op: 'window', pid }, { timeoutMs: 5000 });
    },
    async click(x, y) {
      return await helper.request({ op: 'click', x, y }, { timeoutMs: 5000 });
    },
    async capture(file, rect) {
      return await helper.request(
        { op: 'capture', x: rect.left, y: rect.top, w: rect.w, h: rect.h, path: file },
        { timeoutMs: 15000 },
      );
    },
    async nudge() {
      try {
        await helper.request({ op: 'nudge' }, { timeoutMs: 3000 });
      } catch {
        /* 抖动失败不影响主流程 */
      }
    },
    async verifyPng(file) {
      try {
        const buffer = await fsp.readFile(file);
        if (buffer.length < 24) return { ok: false, error: `文件太小（${buffer.length} bytes）` };
        const signature = buffer.subarray(0, 8).toString('hex');
        if (signature !== '89504e470d0a1a0a') return { ok: false, error: `不是 PNG（签名 ${signature}）` };
        const width = buffer.readUInt32BE(16);
        const height = buffer.readUInt32BE(20);
        if (!width || !height) return { ok: false, error: 'PNG 宽高为 0' };
        return { ok: true, bytes: buffer.length, w: width, h: height };
      } catch (error) {
        return { ok: false, error: error.message };
      }
    },
    async sleep(ms) {
      await sleep(ms);
    },
  };
}

function timeoutProfile(opts) {
  return {
    card: 25000,
    node: Math.max(8000, opts.settleMs * 2),
    settle: opts.settleMs,
    review: 4000,
    collapse: 5000,
  };
}

async function runE2e(opts, logger) {
  const stampId = stamp();
  const dataDir = path.resolve(opts.dataDir ?? path.join(os.tmpdir(), `daozai-fast-${stampId}`));
  const outRoot = path.resolve(opts.outRoot ?? OUT_ROOT);
  const shots = path.join(outRoot, stampId);
  const ctx = {
    opts,
    logger,
    dataDir,
    shots,
    scenarios: [],
    failures: [],
    processes: [],
    startedAt: nowMs(),
    helper: null,
    island: null,
    islandProc: null,
    islandPid: null,
    exe: null,
    runCopy: null,
    sha256: null,
    dshExe: null,
    windowInfo: null,
    fatal: null,
  };

  try {
    // 1) 前置检查 + 目录
    const resolved = preflight({ opts, logger });
    ctx.dshExe = resolved.dshExe;
    ctx.exe = resolved.exe;
    fs.mkdirSync(dataDir, { recursive: true });
    fs.mkdirSync(path.join(dataDir, 'dsh-home', 'profiles', 'desktop'), { recursive: true });
    fs.mkdirSync(path.join(dataDir, 'codex-home'), { recursive: true });
    fs.mkdirSync(shots, { recursive: true });
    fs.writeFileSync(path.join(dataDir, 'dsh-home', 'profiles', 'desktop', 'package.json'), '{"private":true}', 'utf8');
    fs.writeFileSync(path.join(dataDir, 'settings.txt'), `${SETTINGS_LINES.join('\n')}\n`, 'utf8');
    logger.log(`数据目录：${dataDir}`);
    logger.log(`证据目录：${shots}`);

    // 2) exe 副本（不锁 target 构建产物）+ 指纹
    ctx.runCopy = path.join(dataDir, 'daozai-e2e.exe');
    fs.copyFileSync(ctx.exe, ctx.runCopy);
    ctx.sha256 = await sha256File(ctx.exe);
    logger.log(`运行副本：${ctx.runCopy}（sha256=${ctx.sha256.slice(0, 16)}…）`);

    // 3) 清掉可能残留的 ui-map（--data-dir 复用时它来自上一次运行，会让「新鲜度」判断失真）
    try {
      fs.unlinkSync(path.join(dataDir, MAP_FILE));
    } catch {
      /* 不存在就忽略 */
    }

    // 4) 输入/截图辅助进程 + 锁屏检查
    ctx.helper = new WinHelper({ scriptPath: path.join(dataDir, 'agent-input-helper.ps1'), logger });
    const hello = await ctx.helper.start();
    ctx.helperPid = hello.pid;
    logger.log(`输入辅助进程就绪 PID=${hello.pid}（DPI aware=${hello.dpiAware}）`);
    const lock = await ctx.helper.request({ op: 'lock' }, { timeoutMs: 5000 });
    const lockErr = lockError(lock);
    if (lockErr) throw lockErr;

    // 5) 启动岛仔（隔离数据目录 + ui-map 输出到数据目录）
    const env = {
      ...process.env,
      DAOZAI_DATA_DIR: dataDir,
      DSH_HOME: path.join(dataDir, 'dsh-home'),
      CODEX_HOME: path.join(dataDir, 'codex-home'),
      DAOZAI_UI_MAP: dataDir, // 目录形态 → <dir>/ui-map.json
    };
    ctx.islandProc = spawn(ctx.runCopy, [], { cwd: dataDir, env, stdio: 'ignore', windowsHide: false });
    ctx.islandPid = ctx.islandProc.pid;
    if (!ctx.islandPid) throw new E2eError(EXIT.USAGE, `启动岛仔副本失败（spawn 没有拿到 PID）：${ctx.runCopy}`);
    ctx.processes.push({ what: '启动岛仔（被测实例）', pid: ctx.islandPid, at: new Date().toISOString() });
    logger.log(`启动岛仔 PID=${ctx.islandPid}（DAOZAI_UI_MAP=${dataDir}）`);
    ctx.islandProc.on('exit', (code) => {
      // 正常收尾（我们 taskkill 的）不算异常，别把它打成「岛仔崩了」误导排查。
      if (ctx.stoppingIsland) logger.debug(`岛仔已按计划停止（code=${code}）`);
      else logger.warn(`岛仔进程意外退出（code=${code}）：exe 起不来 / 单实例互斥体被占用 / 启动即崩`);
    });

    // 6) 桥接通道就绪 → ui-map（可能还没有：没有卡片/面板时生产端不落盘）→ 窗口就绪
    await waitBridgeReady({ dataDir, islandProc: ctx.islandProc, logger });
    ctx.island = new IslandView({
      mapFile: path.join(dataDir, MAP_FILE),
      io: realIo({ helper: ctx.helper, islandProc: ctx.islandProc, opts, logger }),
      freshMs: opts.freshMs,
      logger,
      pid: ctx.islandPid,
    });
    // 启动阶段没有 ui-map 是正常的（生产端只在有卡片/面板时落盘），这里只探一次做记录，不空等。
    try {
      const early = await ctx.island.read();
      ctx.uiMapAtStart = true;
      logger.log(`启动时 ui-map 已有内容：scale=${early.map.scale} canvas=${early.map.canvas.w}x${early.map.canvas.h} ${describeMap(early.map)}`);
    } catch (error) {
      ctx.uiMapAtStart = false;
      logger.log('启动时还没有 ui-map（没有卡片/面板时生产端不落盘，正常；第一张卡片出现后再等）');
      logger.debug(`启动探测：${error.message}`);
    }
    ctx.windowInfo = await ctx.island.refreshWindow({ timeoutMs: 15000 });
    logger.log(
      `窗口：client=${ctx.windowInfo.client.w}x${ctx.windowInfo.client.h}@(${ctx.windowInfo.client.left},${ctx.windowInfo.client.top}) `
        + `window=(${ctx.windowInfo.window.left},${ctx.windowInfo.window.top})-(${ctx.windowInfo.window.right},${ctx.windowInfo.window.bottom}) dpi=${ctx.windowInfo.dpi}`,
    );

    // 7) 场景
    const names = opts.scenario === 'all' ? SCENARIO_ORDER : [opts.scenario];
    const timeouts = timeoutProfile(opts);
    const scenarioCtx = { ...ctx, timeouts };
    for (const name of names) {
      const verdict = await runScenario(name, scenarioCtx);
      ctx.scenarios.push(verdict);
      if (!verdict.ok) {
        ctx.failures.push(`${name}: ${verdict.error}`);
        logger.warn(`场景 ${name} 失败（${verdict.duration}s）：${verdict.error}`);
        // ui-map 不可用属于环境级错误：后面的场景没有意义，整轮按退出码 6 收尾。
        if (verdict.errorCode === EXIT.UIMAP) {
          ctx.fatal = new E2eError(EXIT.UIMAP, verdict.error);
          break;
        }
      } else {
        logger.log(`场景 ${name} 通过（${verdict.duration}s，${verdict.steps.length} 步）`);
      }
    }
  } catch (error) {
    ctx.fatal = error;
    logger.warn(`执行中断：${error.message}`);
  } finally {
    // 收尾：停岛仔（除非 --keep-alive）、停辅助进程
    if (ctx.islandPid && !opts.keepAlive) {
      ctx.stoppingIsland = true;
      killTree(ctx.islandPid);
      ctx.processes.push({ what: '停止岛仔（被测实例）', pid: ctx.islandPid, at: new Date().toISOString() });
      await sleep(300);
    } else if (ctx.islandPid && opts.keepAlive) {
      logger.log(`--keep-alive：保留岛仔 PID=${ctx.islandPid}（数据目录 ${dataDir}）`);
    }
    if (ctx.helper) {
      try {
        await ctx.helper.stop();
      } catch (error) {
        logger.warn(`停止输入辅助进程失败：${error.message}`);
      }
    }
  }

  const summary = buildSummary(ctx);
  writeSummary(summary, shots);
  printSummary(summary);

  if (ctx.fatal) return ctx.fatal instanceof E2eError ? ctx.fatal.code : EXIT.SCENARIO;
  return ctx.failures.length ? EXIT.SCENARIO : EXIT.OK;
}

function buildSummary(ctx) {
  const totalMs = elapsed(ctx.startedAt);
  return {
    ok: !ctx.fatal && ctx.failures.length === 0,
    at: new Date().toISOString(),
    mode: 'ui-map',
    exe: ctx.exe,
    sha256: ctx.sha256,
    runCopy: ctx.runCopy,
    dataDir: ctx.dataDir,
    shots: ctx.shots,
    uiMap: path.join(ctx.dataDir, MAP_FILE),
    dshExe: ctx.dshExe,
    exeScaleHint: 'settings.island_size=140 是 App 缩放（1.4）；ui-map.scale 是 OS DPI（pixels_per_point），两者不同',
    env: {
      helperPid: ctx.helperPid ?? null,
      islandPid: ctx.islandPid ?? null,
      uiMapAtStart: ctx.uiMapAtStart ?? null,
      window: ctx.windowInfo
        ? {
          client: ctx.windowInfo.client,
          window: ctx.windowInfo.window,
          dpi: ctx.windowInfo.dpi,
          dpiAware: ctx.windowInfo.dpiAware,
        }
        : null,
      freshMs: ctx.opts.freshMs,
    },
    scenarios: ctx.scenarios,
    failures: ctx.failures,
    fatal: ctx.fatal ? { code: ctx.fatal.code ?? null, message: ctx.fatal.message } : null,
    performance: {
      totalMs,
      budgetMs: 30000,
      // 预算只算「界面交互」部分：每个场景都要起一次真实 DSH 宿主，冷启动不属于被测对象。
      interactiveTotalMs: ctx.scenarios.reduce((sum, item) => sum + (item.interactiveMs ?? 0), 0),
      metBudget: ctx.scenarios.reduce((sum, item) => sum + (item.interactiveMs ?? 0), 0) <= 30000,
      scenarios: ctx.scenarios.map((item) => ({
        scenario: item.scenario,
        durationMs: item.durationMs,
        driverStartupMs: item.driverStartupMs ?? null,
        interactiveMs: item.interactiveMs ?? 0,
      })),
    },
    processes: ctx.processes,
    log: ctx.logger.lines,
  };
}

function writeSummary(summary, shots) {
  try {
    fs.mkdirSync(shots, { recursive: true });
    fs.writeFileSync(path.join(shots, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`, 'utf8');
  } catch (error) {
    console.error(`[fast][warn] 写 summary.json 失败：${error.message}`);
  }
}

function printSummary(summary) {
  console.log('');
  console.log(`[fast] ${summary.ok ? '通过' : '失败'}：${summary.scenarios.length} 个场景，`
    + `总耗时 ${(summary.performance.totalMs / 1000).toFixed(2)}s；`
    + `界面交互合计 ${((summary.performance.interactiveTotalMs ?? summary.performance.totalMs) / 1000).toFixed(2)}s`
    + `（预算 30s：${summary.performance.metBudget ? '达标' : '超标'}）`);
  for (const item of summary.scenarios) {
    console.log(`  - ${item.scenario}: ${item.ok ? 'OK' : 'FAIL'} ${(item.durationMs / 1000).toFixed(2)}s`
      + `（交互 ${((item.interactiveMs ?? item.durationMs) / 1000).toFixed(2)}s + 驱动冷启动 ${((item.driverStartupMs ?? 0) / 1000).toFixed(2)}s，`
      + `${item.steps.length} 步，桥接=${item.bridgeOk ? 'OK' : '未通过'}）${item.error ? `  ${item.error}` : ''}`);
  }
  if (summary.fatal) console.log(`[fast] 致命错误（退出码 ${summary.fatal.code ?? '?'}）：${summary.fatal.message}`);
  if (summary.failures.length) console.log(`[fast] failures：\n  - ${summary.failures.join('\n  - ')}`);
  console.log(`[fast] 证据与 summary：${summary.shots}`);
}

// ---------------------------------------------------------------- --legacy：转调旧 OCR 脚本

/** --legacy 的参数映射（单独抽出来，自测可以直接验证，不用真的跑旧脚本）。 */
function buildLegacyArgs(opts) {
  const args = ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', LEGACY_SCRIPT, '-Scenario', opts.scenario];
  if (opts.exe) args.push('-Exe', path.resolve(opts.exe));
  if (opts.dataDir) args.push('-DataDir', path.resolve(opts.dataDir));
  if (opts.outRoot) args.push('-OutRoot', path.resolve(opts.outRoot));
  args.push('-WaitSeconds', String(opts.waitSeconds));
  if (opts.keepAlive) args.push('-KeepAlive');
  for (const extra of opts.legacyArgs ?? []) args.push(extra);
  return args;
}

async function runLegacy(opts) {
  if (!fs.existsSync(LEGACY_SCRIPT)) throw new E2eError(EXIT.USAGE, `缺少旧脚本：${LEGACY_SCRIPT}`);
  const args = buildLegacyArgs(opts);
  console.log(`[fast] --legacy 转调：powershell ${args.map((item) => (item.includes(' ') ? `"${item}"` : item)).join(' ')}`);
  const started = nowMs();
  const result = spawnSync('powershell.exe', args, { stdio: 'inherit', windowsHide: false });
  const code = result.status ?? 2;
  console.log(`[fast] --legacy 旧脚本退出码 ${code}，耗时 ${((elapsed(started)) / 1000).toFixed(2)}s（新旧对照请比对 summary.json 里的桥接 decision）`);
  return code;
}

// ---------------------------------------------------------------- 自测（不需要真岛仔）

function testNode(id, rect, extra = {}) {
  return { id, rect, ...extra };
}

function testMap(overrides = {}) {
  return {
    schema: 1,
    t: 1,
    scale: 1.4,
    canvas: { w: 960, h: 520 },
    state: { panel: true, details: true, notice: 'agent', queue: 1, card: null, result: null },
    nodes: [],
    ...overrides,
  };
}

function approvalMap({ details = true, allow = true } = {}) {
  const nodes = [
    testNode('island.bar', [0, 0, 640, 44]),
    testNode('card', [80, 44, 419, 372]),
  ];
  if (allow) nodes.push(testNode('btn.allow', [400, 400, 120, 32], { label: '仅允许这一次' }));
  if (allow) nodes.push(testNode('body.operation', [120, 140, 340, 120], { label: '操作内容' }));
  if (!details) nodes.push(testNode('btn.view', [300, 10, 80, 28], { label: '查看并审批' }));
  return testMap({
    state: {
      panel: details,
      details,
      notice: 'agent',
      queue: 1,
      card: { kind: 'approval', id: 'e2e', details, index: 0, count: 0, submitting: false, error: null },
      result: null,
    },
    nodes,
  });
}

function questionMap({ index = 0, count = 2, submit = true, selected = false } = {}) {
  const nodes = [
    testNode('island.bar', [0, 0, 640, 44]),
    testNode('card', [80, 44, 419, 372]),
    testNode('option.0', [120, 160, 340, 34], { label: index === 0 ? '方案一' : '确认一' }),
    testNode('btn.next', [430, 380, 60, 30], { label: '下一题' }),
  ];
  if (selected) nodes.push(testNode('input.custom', [120, 240, 340, 30], { label: '自定义回答' }));
  if (submit) nodes.push(testNode('btn.submit', [360, 380, 60, 30], { label: '检查答案' }));
  return testMap({
    state: {
      panel: true,
      details: true,
      notice: 'agent',
      queue: 1,
      card: { kind: 'question', id: 'e2e', details: true, index, count, submitting: false, error: null },
      result: null,
    },
    nodes,
  });
}

function doneMap({ collapsed = false } = {}) {
  if (collapsed) {
    return testMap({
      state: { panel: false, details: false, notice: 'none', queue: 0, card: null, result: null },
      nodes: [testNode('island.bar', [0, 0, 640, 44])],
    });
  }
  return testMap({
    state: {
      panel: true,
      details: true,
      notice: 'agent',
      queue: 0,
      card: { kind: 'done', id: 'e2e', details: true, index: 0, count: 0, submitting: false, error: null },
      result: null,
    },
    nodes: [
      testNode('island.bar', [0, 0, 640, 44]),
      testNode('card', [80, 44, 419, 300]),
      testNode('btn.view', [360, 300, 80, 30], { label: '查看回复' }),
      testNode('btn.close', [450, 300, 40, 30], { label: '关闭' }),
    ],
  });
}

function resultLayerMap() {
  return testMap({
    canvas: { w: 960, h: 880 },
    state: {
      panel: true,
      details: true,
      notice: 'agent',
      queue: 0,
      card: null,
      result: { open: true, hint: false, title: '本轮回复', scroll: 0, maxScroll: 512 },
    },
    nodes: [
      testNode('island.bar', [0, 0, 640, 44]),
      testNode('layer.result', [80, 60, 800, 700]),
      testNode('body.result', [96, 120, 768, 560]),
      testNode('btn.copy', [700, 800, 80, 30], { label: '复制内容' }),
      testNode('btn.close', [790, 800, 60, 30], { label: '关闭' }),
    ],
  });
}

/** 假 io：用一帧帧脚本化 ui-map 驱动真实 IslandView / 真实场景函数。 */
function fakeIo({ win, frames, onClick = null, textQueue = null, mapAgeMs = 0, ageAfterClick = null, autoClient = false, missingReads = 0 }) {
  let currentIndex = 0;
  let missingLeft = missingReads;
  const io = {
    pid: 4242,
    clicks: [],
    captures: [],
    frames,
    age: mapAgeMs,
    async readMapFile() {
      if (missingLeft > 0) {
        missingLeft -= 1;
        return null;
      }
      if (textQueue) {
        const entry = textQueue.length > 1 ? textQueue.shift() : textQueue[0];
        return { text: entry.text, mtimeMs: nowMs() - (entry.ageMs ?? 0) };
      }
      const frame = frames[currentIndex];
      return { text: JSON.stringify(frame), mtimeMs: nowMs() - io.age };
    },
    async window() {
      if (!autoClient) return win;
      // 真实岛仔窗口会随卡片展开/收起 resize：客户区 = 当前帧 canvas * scale。
      const frame = frames[currentIndex];
      const client = {
        left: win.client.left,
        top: win.client.top,
        w: Math.round(frame.canvas.w * frame.scale),
        h: Math.round(frame.canvas.h * frame.scale),
      };
      return { ...win, client, window: { left: client.left, top: client.top, right: client.left + client.w, bottom: client.top + client.h } };
    },
    async click(x, y) {
      io.clicks.push({ x, y });
      let next;
      if (onClick) next = onClick({ x, y, index: currentIndex, frame: frames[currentIndex] });
      else next = true; // 默认：每点一次进下一帧（假状态机）
      if (typeof next === 'number') currentIndex = Math.min(next, frames.length - 1);
      else if (next) currentIndex = Math.min(currentIndex + 1, frames.length - 1);
      if (ageAfterClick !== null) io.age = ageAfterClick;
      return { ok: true };
    },
    async capture(file, rect) {
      io.captures.push({ file, rect });
      return { ok: true, path: file };
    },
    async verifyPng() {
      return { ok: true, bytes: 2048, w: 100, h: 100 };
    },
    async nudge() {
      return { ok: true };
    },
    async sleep(ms) {
      await sleep(Math.min(ms, 5));
    },
  };
  return io;
}

function fakeWindow({ client = { left: 100, top: 50, w: 1344, h: 728 }, scale = 1.4 } = {}) {
  return {
    ok: true,
    hwnd: 1,
    dpi: Math.round(scale * 96),
    dpiAware: true,
    window: { left: 100, top: 50, right: 100 + client.w, bottom: 50 + client.h },
    client,
    screen: { left: 0, top: 0, right: 3440, bottom: 1440 },
  };
}

async function selfTest(opts) {
  const logger = new Logger(opts.verbose);
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'daozai-fast-selftest-'));
  const checks = [];
  const check = async (name, fn) => {
    const started = nowMs();
    try {
      await fn();
      checks.push({ name, ok: true, ms: elapsed(started) });
      console.log(`  PASS  ${name}（${elapsed(started)}ms）`);
    } catch (error) {
      checks.push({ name, ok: false, ms: elapsed(started), error: error?.message ?? String(error) });
      console.error(`  FAIL  ${name}（${elapsed(started)}ms）\n        ${error?.message ?? error}`);
    }
  };
  const expect = (condition, message) => {
    if (!condition) throw new Error(message);
  };
  const expectCode = async (fn, code) => {
    try {
      await fn();
    } catch (error) {
      expect(error instanceof E2eError, `期望 E2eError，实际 ${error?.name}: ${error?.message}`);
      expect(error.code === code, `期望退出码 ${code}，实际 ${error.code}（${error.message}）`);
      return error;
    }
    throw new Error(`期望抛出退出码 ${code} 的错误，但没有抛`);
  };
  const tiny = { card: 120, node: 120, settle: 60, review: 80, collapse: 120 };
  const islandFor = (io, mapFile = path.join(tempDir, 'ui-map.json')) => new IslandView({ mapFile, io, freshMs: 1500, logger, pid: 4242 });

  console.log(`[fast] --self-test（临时目录 ${tempDir}，不需要真岛仔）`);

  try {
    await check('01 ui-map 解析与 schema 校验', async () => {
      const map = approvalMap();
      expect(validateUiMap(map).length === 0, `合法 map 被拒绝：${validateUiMap(map).join('；')}`);
      expect(validateUiMap({ ...map, schema: 2 }).some((item) => item.includes('schema')), 'schema≠1 没被发现');
      expect(validateUiMap({ ...map, nodes: [{ id: 'x' }] }).some((item) => item.includes('rect')), '节点缺 rect 没被发现');
      expect(validateUiMap({ ...map, state: undefined }).some((item) => item.includes('state')), 'state 缺失没被发现');
      expect(validateUiMap({ ...map, scale: 0 }).some((item) => item.includes('scale')), 'scale=0 没被发现');
      expect(nodeIds(map).includes('btn.allow'), 'nodeIds 丢了节点');
    });

    await check('02 读 ui-map：JSON 截断（原子写窗口）重试后成功', async () => {
      const good = JSON.stringify(approvalMap());
      const io = fakeIo({
        win: fakeWindow(),
        frames: [approvalMap()],
        textQueue: [{ text: '{"schema":1,' }, { text: '{"schema":1,"t":' }, { text: good }],
      });
      const island = islandFor(io);
      const read = await island.read();
      expect(read.map.schema === 1, '重试后没有读到合法 map');
    });

    await check('03 读 ui-map：schema 不符 → 退出码 6', async () => {
      const io = fakeIo({ win: fakeWindow(), frames: [testMap({ schema: 9 })] });
      const island = islandFor(io);
      const error = await expectCode(() => island.read(), EXIT.UIMAP);
      expect(error.message.includes('schema'), `错误信息没有点明 schema：${error.message}`);
    });

    await check('04 节点取点 + 屏幕换算（客户区左上 + rect*scale）', async () => {
      const io = fakeIo({ win: fakeWindow(), frames: [approvalMap()] });
      const island = islandFor(io);
      const read = await island.read();
      const geo = await island.geometry(read.map);
      expect(geo.coordScale === 1.4, `coordScale 期望 1.4，实际 ${geo.coordScale}`);
      const allow = findNode(read.map, 'btn.allow');
      const point = nodeScreenPoint(geo.win.client.left, geo.win.client.top, geo.coordScale, allow.rect);
      // rect [400,400,120,32] 中心 (460,416)：100+460*1.4=744，50+416*1.4=632.4→632
      expect(point.x === 744 && point.y === 632, `取点错误：${JSON.stringify(point)}`);
    });

    await check('05 DPI 不感知兜底：客户区=canvas 逻辑尺寸 → coordScale=1', async () => {
      const io = fakeIo({ win: fakeWindow({ client: { left: 100, top: 50, w: 960, h: 520 } }), frames: [approvalMap()] });
      const island = islandFor(io);
      const read = await island.read();
      const geo = await island.geometry(read.map);
      expect(geo.coordScale === 1, `coordScale 期望 1（兜底），实际 ${geo.coordScale}`);
      const allow = findNode(read.map, 'btn.allow');
      const point = nodeScreenPoint(geo.win.client.left, geo.win.client.top, geo.coordScale, allow.rect);
      expect(point.x === 560 && point.y === 466, `兜底取点错误：${JSON.stringify(point)}`);
    });

    await check('06 截图裁剪：card / layer.result 矩形（1:1 物理像素）', async () => {
      const io = fakeIo({ win: fakeWindow(), frames: [approvalMap()] });
      const island = islandFor(io);
      const shot = await island.shot(tempDir, 'selftest-card');
      // card [80,44,419,372] + pad 6，scale 1.4 → 100+(74*1.4)=203.6→204，50+(38*1.4)=103.2→103
      expect(shot.rect.left === 204 && shot.rect.top === 103, `card 裁剪点位错误：${JSON.stringify(shot.rect)}`);
      expect(shot.rect.w === Math.round((419 + 12) * 1.4), `card 裁剪宽度错误：${shot.rect.w}`);
      const io2 = fakeIo({ win: fakeWindow({ client: { left: 100, top: 50, w: 1344, h: 1232 } }), frames: [resultLayerMap()] });
      const island2 = islandFor(io2);
      const shot2 = await island2.shot(tempDir, 'selftest-result', { prefer: 'result' });
      expect(shot2.rect.w === Math.round((800 + 12) * 1.4), `结果层裁剪宽度错误：${shot2.rect.w}（应优先 layer.result）`);
      expect(io.captures.length === 1 && io2.captures.length === 1, '截图没有走 io.capture');
    });

    await check('07 等状态变化：点击后 ui-map 变了 → changed=true 且换帧', async () => {
      const frames = [approvalMap(), approvalMap({ allow: false }), testMap({
        state: { panel: false, details: false, notice: 'none', queue: 0, card: null, result: null },
        nodes: [testNode('island.bar', [0, 0, 640, 44])],
      })];
      const io = fakeIo({ win: fakeWindow(), frames });
      const island = islandFor(io);
      const click = await island.clickNode('btn.allow', { requireChange: true, settleMs: 400 });
      expect(click.changed, '没有检测到 ui-map 变化');
      expect(click.waitedMs < 400, `等待时间异常：${click.waitedMs}ms`);
      expect(io.clicks.length === 1 && io.clicks[0].x === 744 && io.clicks[0].y === 632, `真鼠标坐标错误：${JSON.stringify(io.clicks)}`);
      const after = await island.read();
      expect(findNode(after.map, 'btn.allow') === null, '没有换到下一帧（btn.allow 应该消失）');
    });

    await check('08 等状态变化超时 → 退出码 3 + 最后状态', async () => {
      const io = fakeIo({ win: fakeWindow(), frames: [approvalMap()] });
      const island = islandFor(io);
      const error = await expectCode(
        () => island.waitState((map) => map.state.card?.kind === 'done', { timeoutMs: 60, label: '完成卡出现' }),
        EXIT.SCENARIO,
      );
      expect(error.message.includes('完成卡出现'), `错误里没有等待目标：${error.message}`);
      expect(error.message.includes('card='), `错误里没有最后状态：${error.message}`);
    });

    await check('09 点击无变化 + requireChange → 退出码 3（关键按钮必须有效果）', async () => {
      const io = fakeIo({ win: fakeWindow(), frames: [approvalMap()] });
      const island = islandFor(io);
      const error = await expectCode(
        () => island.clickNode('btn.allow', { requireChange: true, settleMs: 80 }),
        EXIT.SCENARIO,
      );
      expect(error.message.includes('ui-map 没有任何变化'), `错误信息不对：${error.message}`);
    });

    await check('10 节点缺失 → 退出码 3 + 可用节点清单', async () => {
      const io = fakeIo({ win: fakeWindow(), frames: [approvalMap()] });
      const island = islandFor(io);
      const error = await expectCode(() => island.waitNode('btn.deny', { timeoutMs: 60 }), EXIT.SCENARIO);
      expect(error.message.includes('btn.deny'), `错误里没有节点 id：${error.message}`);
      expect(error.message.includes('btn.allow'), `错误里没有可用节点清单：${error.message}`);
    });

    await check('11 变化后 ui-map 不新鲜 → 退出码 6', async () => {
      const io = fakeIo({
        win: fakeWindow(),
        frames: [approvalMap(), approvalMap({ allow: false })],
        ageAfterClick: 9000,
      });
      const island = islandFor(io);
      const error = await expectCode(
        () => island.clickNode('btn.allow', { requireChange: true, settleMs: 200 }),
        EXIT.UIMAP,
      );
      expect(error.message.includes('不新鲜'), `错误里没有新鲜度说明：${error.message}`);
    });

    await check('12 场景 approval（假状态机）：展开→点允许→收起，坐标与截图都对', async () => {
      const frames = [approvalMap({ details: false, allow: false }), approvalMap(), testMap({
        state: { panel: false, details: false, notice: 'none', queue: 0, card: null, result: null },
        nodes: [testNode('island.bar', [0, 0, 640, 44])],
      })];
      const io = fakeIo({ win: fakeWindow(), frames });
      const island = islandFor(io);
      const steps = [];
      await scenarioApproval({ island, steps, shots: tempDir, timeouts: tiny, opts: {}, dataDir: tempDir });
      expect(steps.length >= 5, `步骤太少：${steps.length}`);
      expect(steps.every((item) => item.ok !== false), `有步骤失败：${JSON.stringify(steps.filter((item) => item.ok === false))}`);
      // 第一次点 btn.view（展开），第二次点 btn.allow
      expect(io.clicks.length === 2, `点击次数期望 2，实际 ${io.clicks.length}`);
      expect(io.clicks[1].x === 744 && io.clicks[1].y === 632, `允许按钮坐标错：${JSON.stringify(io.clicks[1])}`);
      expect(io.captures.length >= 2, '没有存点击前截图');
    });

    await check('13 场景 question（假状态机）：选项→提交→审阅页确认', async () => {
      const reviewMap = questionMap({ index: 0, count: 1, submit: false });
      reviewMap.nodes.push(testNode('btn.confirm', [430, 380, 60, 30], { label: '确认提交' }));
      const frames = [
        questionMap({ index: 0, count: 1 }), // 0 选项页
        questionMap({ index: 0, count: 1, selected: true }), // 1 点完选项（选中态：多出 input.custom）
        reviewMap, // 2 点「检查答案」后进审阅页
        testMap({ // 3 点「确认提交」后收起
          state: { panel: false, details: false, notice: 'none', queue: 0, card: null, result: null },
          nodes: [testNode('island.bar', [0, 0, 640, 44])],
        }),
      ];
      const io = fakeIo({ win: fakeWindow(), frames });
      const island = islandFor(io);
      const steps = [];
      await scenarioQuestion({ island, steps, shots: tempDir, timeouts: tiny, opts: {}, dataDir: tempDir });
      expect(steps.every((item) => item.ok !== false), `有步骤失败：${JSON.stringify(steps.filter((item) => item.ok === false))}`);
      expect(steps.some((item) => item.step.includes('确认提交')), '没有走到审阅页确认提交');
      expect(io.clicks.length === 3, `点击次数期望 3（选项/检查答案/确认提交），实际 ${io.clicks.length}`);
    });

    await check('14 场景 question-multi-turn（假状态机）：下一题 index→1', async () => {
      const confirmMap = questionMap({ index: 1, count: 2, submit: false });
      confirmMap.nodes.push(testNode('btn.confirm', [430, 380, 60, 30], { label: '确认提交' }));
      const frames = [
        questionMap({ index: 0, count: 2 }), // 0 第 1 题
        questionMap({ index: 0, count: 2, selected: true }), // 1 点完第 1 题选项
        questionMap({ index: 1, count: 2 }), // 2 点「下一题」后
        questionMap({ index: 1, count: 2, selected: true }), // 3 点完第 2 题选项
        confirmMap, // 4 点「检查答案」后进审阅页
        testMap({ // 5 点「确认提交」后收起
          state: { panel: false, details: false, notice: 'none', queue: 0, card: null, result: null },
          nodes: [testNode('island.bar', [0, 0, 640, 44])],
        }),
      ];
      const io = fakeIo({ win: fakeWindow(), frames });
      const island = islandFor(io);
      const steps = [];
      await scenarioQuestionMultiTurn({ island, steps, shots: tempDir, timeouts: tiny, opts: {}, dataDir: tempDir });
      expect(steps.every((item) => item.ok !== false), `有步骤失败：${JSON.stringify(steps.filter((item) => item.ok === false))}`);
      expect(steps.some((item) => item.step.includes('index=1')), '没有断言 index 切到 1');
      expect(io.clicks.length === 5, `点击次数期望 5（选项1/下一题/选项2/检查答案/确认提交），实际 ${io.clicks.length}`);
    });

    await check('15 场景 done（假状态机）：查看回复→结果层→关闭后一次收起', async () => {
      const frames = [doneMap(), resultLayerMap(), doneMap({ collapsed: true })];
      const io = fakeIo({ win: fakeWindow(), frames, autoClient: true });
      const island = islandFor(io);
      const steps = [];
      await scenarioDone({ island, steps, shots: tempDir, timeouts: tiny, opts: {}, dataDir: tempDir });
      expect(steps.every((item) => item.ok !== false), `有步骤失败：${JSON.stringify(steps.filter((item) => item.ok === false))}`);
      const collapsed = steps.find((item) => item.step.includes('关闭后一次收起'));
      expect(collapsed && collapsed.state.panel === false, '没有断言 panel=false');
      expect(io.clicks.length === 2, `点击次数期望 2（查看回复 + 关闭），实际 ${io.clicks.length}`);
    });

    await check('16 场景 done 反例：关闭后没有收起 → 退出码 3', async () => {
      // 点关闭后结果层消失、但岛体没有收起（panel 仍为 true）——真实的「没有一次收起」回归。
      const stuck = testMap({
        state: { panel: true, details: false, notice: 'none', queue: 0, card: null, result: null },
        nodes: [testNode('island.bar', [0, 0, 640, 44])],
      });
      const frames = [doneMap(), resultLayerMap(), stuck];
      const io = fakeIo({ win: fakeWindow(), frames, autoClient: true });
      const island = islandFor(io);
      const steps = [];
      const error = await expectCode(
        () => scenarioDone({ island, steps, shots: tempDir, timeouts: { ...tiny, settle: 40 }, opts: {}, dataDir: tempDir }),
        EXIT.SCENARIO,
      );
      expect(error.message.includes('一次收起'), `错误信息没有点明收起断言：${error.message}`);
    });

    await check('17 桥接断言：approval / question / 多轮 / done / handback', async () => {
      const allow = {
        ok: true, decision: 'allowed-once', handback: false,
        bridge: [{ provider: 'dsh', kind: 'approval', mode: 1, sent: { tool: 'Bash' }, reply: { decision: 'allow' } }],
      };
      expect(checkBridge('approval', allow, SCENARIOS.approval).ok, 'approval allow 被误判失败');
      expect(!checkBridge('approval', { ...allow, decision: 'denied' }, SCENARIOS.approval).ok, 'approval denied 被误判通过');
      expect(!checkBridge('approval', { ...allow, handback: true }, SCENARIOS.approval).ok, 'handback 被误判通过');
      expect(checkBridge('approval', { ...allow, handback: true }, SCENARIOS.approval).error.includes('handback'), 'handback 错误信息不对');

      const question = {
        ok: true, handback: false, decision: { answers: [{ id: 'e2e-single', selected: ['冒烟测试'] }] },
        bridge: [{ kind: 'question', mode: 3, sent: {}, reply: { decision: 'answer' } }],
      };
      expect(checkBridge('question', question, SCENARIOS.question).ok, 'question 正确回答被误判失败');
      expect(!checkBridge('question', { ...question, decision: { answers: [{ selected: ['完整回归'] }] } }, SCENARIOS.question).ok, 'question 错答案被误判通过');

      const multi = {
        ok: true, handback: false,
        decision: { answers: [{ selected: ['方案一'] }, { selected: ['确认一'] }] },
        bridge: [{ kind: 'question', reply: { decision: 'answer' } }],
      };
      expect(checkBridge('question-multi-turn', multi, SCENARIOS['question-multi-turn']).ok, '多轮正确回答被误判失败');
      expect(!checkBridge('question-multi-turn', { ...multi, decision: { answers: [{ selected: ['方案二'] }, { selected: ['确认一'] }] } }, SCENARIOS['question-multi-turn']).ok, '多轮错答案被误判通过');

      const finished = {
        ok: true, decision: 'done', handback: false,
        bridge: [{ kind: 'finished', mode: 2, sent: { text: 'x'.repeat(40) }, reply: { done: true } }],
      };
      expect(checkBridge('done', finished, SCENARIOS.done).ok, 'done 被误判失败');
      expect(!checkBridge('done', { ...finished, bridge: [{ kind: 'finished', mode: 0, sent: { text: 'x' }, reply: { done: true } }] }, SCENARIOS.done).ok, 'done mode≠2 被误判通过');
      expect(!checkBridge('approval', null, SCENARIOS.approval).ok, '没有驱动 JSON 却判通过');
    });

    await check('18 负路径分类：已有实例 / 锁屏 / 驱动退出码', async () => {
      const procs = [{ name: 'daozai.exe', pid: 1 }, { name: 'daozai-abc.exe', pid: 2 }, { name: 'explorer.exe', pid: 3 }];
      const daozai = procs.filter((proc) => /^daozai/i.test(proc.name));
      expect(daozai.length === 2, `实例识别错误：${JSON.stringify(daozai)}`);
      expect(lockError({ ok: true, desktop: 'Default', logonui: 1 }) === null, 'Default 桌面被误判为锁屏');
      expect(lockError({ ok: true, desktop: 'Winlogon', logonui: 1 })?.code === EXIT.LOCKED, 'Winlogon 桌面没判锁屏');
      expect(lockError({ ok: true, desktop: '', logonui: 1 })?.code === EXIT.LOCKED, '打不开输入桌面没判锁屏');
      const fakeDriver = { stderrTail: () => 'boom' };
      expect(driverFailureMessage({ code: 2 }, fakeDriver).code === EXIT.DRIVER, 'exit=2 应映射到 7');
      expect(driverFailureMessage({ code: 4 }, fakeDriver).code === EXIT.SCENARIO, 'exit=4（handback）应映射到 3');
      expect(driverFailureMessage({ code: 3 }, fakeDriver).code === EXIT.SCENARIO, 'exit=3（超时）应映射到 3');
    });

    await check('19 summary.json 结构与字段（ok/scenarios[].steps[]/duration/exe/sha256/dataDir/shots）', async () => {
      const shots = path.join(tempDir, 'shots');
      fs.mkdirSync(shots, { recursive: true });
      const ctx = {
        opts: { freshMs: 1500 },
        logger,
        dataDir: tempDir,
        shots,
        scenarios: [{
          scenario: 'approval', driverScenario: 'approval', ok: true,
          steps: [{ step: '等到审批卡片', ok: true }, { step: '点击允许', ok: true, shot: 'x.png' }],
          driver: { ok: true }, bridgeOk: true, error: null, duration: 1.2, durationMs: 1200, shotCount: 1,
        }],
        failures: [],
        processes: [],
        startedAt: nowMs() - 1200,
        exe: 'C:\\x\\daozai.exe',
        runCopy: 'C:\\tmp\\daozai-e2e.exe',
        sha256: 'A'.repeat(64),
        dshExe: 'C:\\dsh\\DeepSeek Harness.exe',
        windowInfo: null,
        helperPid: 1,
        islandPid: 2,
        fatal: null,
      };
      const summary = buildSummary(ctx);
      expect(summary.ok === true && summary.mode === 'ui-map', 'summary.ok/mode 不对');
      expect(summary.scenarios[0].steps.length === 2, 'scenarios[].steps[] 丢了');
      expect(typeof summary.scenarios[0].duration === 'number', 'duration 不是数字');
      expect(summary.sha256.length === 64 && summary.dataDir === tempDir && summary.shots === shots, 'exe/sha256/dataDir/shots 字段不对');
      expect(summary.performance.metBudget === true && summary.performance.totalMs > 0, 'performance 字段不对');
      writeSummary(summary, shots);
      const reread = JSON.parse(fs.readFileSync(path.join(shots, 'summary.json'), 'utf8'));
      expect(reread.scenarios[0].scenario === 'approval', 'summary.json 落盘后读不回来');
    });

    await check('20 场景失败也要产出 summary（失败原因可读）', async () => {
      const shots = path.join(tempDir, 'shots-fail');
      fs.mkdirSync(shots, { recursive: true });
      const summary = buildSummary({
        opts: { freshMs: 1500 }, logger, dataDir: tempDir, shots, scenarios: [], failures: ['done: 等待关闭后收起超时'],
        processes: [], startedAt: nowMs(), exe: 'x', runCopy: 'y', sha256: 'Z'.repeat(64), dshExe: 'd',
        windowInfo: null, helperPid: null, islandPid: null, fatal: new E2eError(EXIT.UIMAP, 'ui-map 不新鲜'),
      });
      expect(summary.ok === false && summary.fatal.code === EXIT.UIMAP, '失败 summary 结构不对');
      writeSummary(summary, shots);
      expect(fs.existsSync(path.join(shots, 'summary.json')), '失败时没有写 summary.json');
    });
    await check('21 ui-map 文件还没出现：等卡片时容忍，最终出现就继续；一直不出现 → 退出码 6', async () => {
      const io = fakeIo({ win: fakeWindow(), frames: [approvalMap()], missingReads: 4 });
      const island = islandFor(io);
      const read = await island.waitState((map) => map.state.card?.kind === 'approval', { timeoutMs: 500, label: '审批卡片出现' });
      expect(read.map.state.card.kind === 'approval', '文件出现后没有等到卡片');

      const io2 = fakeIo({ win: fakeWindow(), frames: [approvalMap()], missingReads: 99999 });
      const island2 = islandFor(io2);
      const error = await expectCode(
        () => island2.waitState((map) => map.state.card?.kind === 'approval', { timeoutMs: 120, label: '审批卡片出现' }),
        EXIT.UIMAP,
      );
      expect(error.message.includes('一直没有出现'), `错误没说明文件没落盘：${error.message}`);
    });

    await check('22 驱动初始化失败（exit=2）→ 不误报成弹卡超时（退出码 7）', async () => {
      const io = fakeIo({ win: fakeWindow(), frames: [approvalMap()], missingReads: 99999 });
      const island = islandFor(io);
      island.driver = { exit: { code: 2 }, stderrTail: () => 'profile boom', exited: true };
      const error = await expectCode(
        () => island.waitState((map) => map.state.card?.kind === 'approval', { timeoutMs: 5000, label: '审批卡片出现' }),
        EXIT.DRIVER,
      );
      expect(error.message.includes('初始化失败'), `错误没点明初始化失败：${error.message}`);
    });

    await check('23 结果层没有卡片（契约字面形态）也能走完 done 场景', async () => {
      const resultNoCard = resultLayerMap();
      const frames = [doneMap(), resultNoCard, doneMap({ collapsed: true })];
      const io = fakeIo({ win: fakeWindow(), frames, autoClient: true });
      const island = islandFor(io);
      const steps = [];
      await scenarioDone({ island, steps, shots: tempDir, timeouts: tiny, opts: {}, dataDir: tempDir });
      expect(steps.every((item) => item.ok !== false), `有步骤失败：${JSON.stringify(steps.filter((item) => item.ok === false))}`);
      expect(steps.some((item) => item.step.includes('一次收起')), '没有验证收起');
    });

    await check('24 没有 card 节点时截图退回客户区（不会截到整屏）', async () => {
      const noCard = testMap({ nodes: [testNode('island.bar', [0, 0, 640, 44])] });
      const io = fakeIo({ win: fakeWindow(), frames: [noCard] });
      const island = islandFor(io);
      const shot = await island.shot(tempDir, 'selftest-fallback');
      expect(shot.rect.left === 100 && shot.rect.top === 50, `退回客户区坐标错误：${JSON.stringify(shot.rect)}`);
      expect(shot.rect.w === 1344 && shot.rect.h === 728, `退回客户区尺寸错误：${JSON.stringify(shot.rect)}`);
    });

    await check('25 done 场景容错：结果层打不开也能用完成卡「关闭」验证一次收起', async () => {
      // 「查看回复」只让卡片闪一下（换了别的节点），结果层始终不开：不应判失败，仍要继续验证收起。
      const noisy = doneMap();
      noisy.nodes.push(testNode('chip.jump', [500, 10, 80, 24], { label: '切换到客户端' }));
      const frames = [doneMap(), noisy, doneMap({ collapsed: true })];
      const io = fakeIo({ win: fakeWindow(), frames, autoClient: true });
      const island = islandFor(io);
      const steps = [];
      await scenarioDone({ island, steps, shots: tempDir, timeouts: tiny, opts: {}, dataDir: tempDir });
      const resume = steps.find((item) => item.step.includes('结果层没有打开'));
      expect(!!resume, '结果层打不开时应记录一条非致命步骤');
      expect(steps.some((item) => item.step.includes('一次收起')), '仍然必须验证一次收起');
      expect(io.clicks.length === 2, `点击次数期望 2（查看回复 + 关闭），实际 ${io.clicks.length}`);
    });

    await check('26 驱动启动参数缺失 → 明确退出码 7（不是 spawn(null) 的 TypeError）', async () => {
      const make = (dshExe) => new Driver({
        name: 'selftest', scenario: 'approval', request: {}, dshExe, profile: tempDir, waitMs: 1000,
        cwd: REPO, shots: tempDir, env: {}, logger,
      });
      const missing = make(null);
      const error = await expectCode(async () => missing.start(), EXIT.DRIVER);
      expect(error.message.includes('dshExe'), `错误没点明参数：${error.message}`);
      expect(missing.exited === true, '启动失败后应标记 exited（失败现场不要再盲等 3s）');
      const notFound = make(path.join(tempDir, 'no-such-dsh.exe'));
      const error2 = await expectCode(async () => notFound.start(), EXIT.DRIVER);
      expect(error2.message.includes('找不到 DeepSeek Harness'), `错误信息不对：${error2.message}`);
    });

    await check('27 --legacy 参数映射（不真的跑旧脚本）', async () => {
      const args = buildLegacyArgs({
        scenario: 'all', exe: 'target/x.exe', dataDir: 'C:/tmp/d', outRoot: 'C:/out',
        waitSeconds: 42, keepAlive: true, legacyArgs: ['-NoClick'],
      });
      expect(args.includes('-File') && args[args.indexOf('-File') + 1] === LEGACY_SCRIPT, '旧脚本路径不对');
      expect(args[args.indexOf('-Scenario') + 1] === 'all', '场景参数错');
      expect(args[args.indexOf('-WaitSeconds') + 1] === '42', '等待参数错');
      expect(args.includes('-KeepAlive') && args.includes('-NoClick'), '透传参数错');
      expect(args[args.indexOf('-Exe') + 1] === path.resolve('target/x.exe'), 'exe 参数没转绝对路径');
      expect(args[args.indexOf('-DataDir') + 1] === path.resolve('C:/tmp/d'), 'dataDir 参数没转绝对路径');
    });
  } finally {
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      /* 忽略 */
    }
  }

  const passed = checks.filter((item) => item.ok).length;
  const failed = checks.length - passed;
  console.log('');
  console.log(`[fast] SELF-TEST ${failed === 0 ? 'OK' : 'FAILED'}：${passed}/${checks.length} 通过（${checks.reduce((sum, item) => sum + item.ms, 0)}ms）`);
  if (failed) {
    for (const item of checks.filter((entry) => !entry.ok)) console.error(`  - ${item.name}: ${item.error}`);
    return 1;
  }
  return 0;
}

// ---------------------------------------------------------------- --helper-check（真机联调第 0 步）

/**
 * 只验证 Win32 辅助进程（PowerShell + Add-Type + DPI aware + GDI 截图 + 锁屏检测），
 * 不启动岛仔、不点击、不改任何配置；鼠标只移动后立刻还原。
 */
async function helperCheck(opts) {
  const logger = new Logger(opts.verbose);
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'daozai-fast-helper-'));
  const helper = new WinHelper({ scriptPath: path.join(tempDir, 'agent-input-helper.ps1'), logger });
  let failed = 0;
  const ok = (label, condition, extra = '') => {
    console.log(`  ${condition ? 'PASS' : 'FAIL'}  ${label}${extra ? `（${extra}）` : ''}`);
    if (!condition) failed++;
  };
  try {
    const hello = await helper.start();
    ok('辅助进程启动（PowerShell + Add-Type）', hello.ready === true, `pid=${hello.pid} dpiAware=${hello.dpiAware} ps=${hello.ps}`);
    const lock = await helper.request({ op: 'lock' });
    ok('锁屏检测（OpenInputDesktop）', lock.ok === true, `desktop=${JSON.stringify(lock.desktop)} locked=${lock.locked} logonui=${lock.logonui}`);
    const cursor = await helper.request({ op: 'cursor' });
    ok('GetCursorPos', cursor.ok === true, `${cursor.x},${cursor.y}`);

    const explorer = listProcesses().find((proc) => proc.name.toLowerCase() === 'explorer.exe');
    ok('tasklist 找到 explorer（拿一个真实窗口做坐标/截图验证）', !!explorer, explorer ? `pid=${explorer.pid}` : '没有 explorer');
    if (explorer) {
      const win = await helper.request({ op: 'window', pid: explorer.pid });
      ok(
        'GetWindowRect / GetClientRect / ClientToScreen',
        win.ok === true && !!win.client,
        win.ok
          ? `window=(${win.window.left},${win.window.top})-(${win.window.right},${win.window.bottom}) `
            + `client=${win.client.w}x${win.client.h}@(${win.client.left},${win.client.top}) dpi=${win.dpi} dpiAware=${win.dpiAware}`
          : win.error,
      );
      if (win.ok) {
        const target = { x: win.client.left + Math.min(80, Math.max(2, win.client.w - 2)), y: win.client.top + Math.min(80, Math.max(2, win.client.h - 2)) };
        const moved = await helper.request({ op: 'move', x: target.x, y: target.y });
        const after = await helper.request({ op: 'cursor' });
        ok('SetCursorPos（只移动不点击）', moved.ok && after.x === target.x && after.y === target.y, `期望 ${target.x},${target.y} 实际 ${after.x},${after.y}`);
        if (cursor.ok) await helper.request({ op: 'move', x: cursor.x, y: cursor.y }); // 还原鼠标位置
        const rect = { left: win.client.left, top: win.client.top, w: Math.min(320, win.client.w), h: Math.min(180, win.client.h) };
        const file = path.join(tempDir, 'helper-check.png');
        const shot = await helper.request({ op: 'capture', x: rect.left, y: rect.top, w: rect.w, h: rect.h, path: file });
        const exists = fs.existsSync(file);
        const head = exists ? fs.readFileSync(file).subarray(0, 8) : Buffer.alloc(0);
        ok(
          'GDI 1:1 截图（PNG 签名 + 尺寸 + 内容采样）',
          shot.ok === true && exists && head.toString('hex') === '89504e470d0a1a0a' && ((shot.ink ?? 0) > 0 || (shot.spread ?? 0) > 0),
          exists ? `${fs.statSync(file).size} bytes ${rect.w}x${rect.h} ink=${shot.ink} spread=${shot.spread}` : '文件不存在',
        );
      }
    }
  } catch (error) {
    console.error(`[fast] helper check 异常：${error.message}`);
    failed++;
  } finally {
    try {
      await helper.stop();
    } catch {
      /* 忽略 */
    }
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      /* 忽略 */
    }
  }
  console.log(`[fast] HELPER CHECK ${failed ? 'FAILED' : 'OK'}`);
  return failed ? EXIT.USAGE : EXIT.OK;
}

// ---------------------------------------------------------------- --driver-check（真机联调第 1 步）

/**
 * 只验证「宿主驱动」链路：用隔离空 profile 启动 tests/fixtures/agent-drive.cjs，
 * 确认 DSH 能按 ELECTRON_RUN_AS_NODE 跑、stdout/stderr 重定向可读、失败信息可读。
 * 不启动岛仔、不点击、不读写用户的 DSH_HOME（用临时目录）。
 * 预期失败：空 profile 里没有岛仔 bundle → 驱动 exit=2「profile 里没有安装岛仔 bundle」，
 * 这正是「链路通了、只差岛仔接入」的证据。
 */
async function driverCheck(opts) {
  const logger = new Logger(opts.verbose);
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'daozai-fast-driver-'));
  const shots = path.join(tempDir, 'shots');
  const profile = path.join(tempDir, 'dsh-home', 'profiles', 'desktop');
  fs.mkdirSync(shots, { recursive: true });
  fs.mkdirSync(profile, { recursive: true });
  fs.writeFileSync(path.join(profile, 'package.json'), '{"private":true}', 'utf8');
  let failed = 0;
  const ok = (label, condition, extra = '') => {
    console.log(`  ${condition ? 'PASS' : 'FAIL'}  ${label}${extra ? `（${extra}）` : ''}`);
    if (!condition) failed++;
  };
  let driver = null;
  try {
    const dshExe = resolveDshExe(opts);
    ok('找到 DeepSeek Harness', fs.existsSync(dshExe), dshExe);
    ok('找到驱动脚本 agent-drive.cjs', fs.existsSync(DRIVE_SCRIPT), DRIVE_SCRIPT);
    if (failed) return EXIT.USAGE;
    const started = nowMs();
    driver = new Driver({
      name: 'driver-check',
      scenario: 'approval',
      request: SCENARIOS.approval.request,
      dshExe,
      profile,
      waitMs: 5000,
      cwd: REPO,
      shots,
      env: { ELECTRON_RUN_AS_NODE: '1', DSH_HOME: path.join(tempDir, 'dsh-home') },
      logger,
    });
    driver.start();
    const exit = await driver.waitExit({ timeoutMs: 30000 });
    const startupMs = elapsed(started);
    ok('驱动进程启动并退出（未挂死）', !!exit, `${startupMs}ms exit=${exit?.code}`);
    const json = await driver.readJson({ timeoutMs: 3000 }).catch(() => null);
    ok('驱动 stdout JSON 可读', !!json, json ? `ok=${json.ok} error=${JSON.stringify(json.error ?? null)}` : '读不到 JSON');
    ok(
      '空 profile 的预期失败可读（说明宿主+插件加载链路是通的）',
      !!json && json.ok === false && typeof json.error === 'string' && json.error.length > 0,
      json?.error ?? '没有 error 字段',
    );
    console.log(`  INFO  驱动冷启动约 ${startupMs}ms；空 profile 失败属预期。`);
    console.log(`  INFO  重定向样例：${driver.stdoutFile}`);
  } catch (error) {
    console.error(`[fast] driver check 异常：${error.message}`);
    failed++;
  } finally {
    if (driver && !driver.exited) driver.kill();
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      /* 忽略 */
    }
  }
  console.log(`[fast] DRIVER CHECK ${failed ? 'FAILED' : 'OK'}`);
  return failed ? EXIT.USAGE : EXIT.OK;
}

// ---------------------------------------------------------------- 入口

async function main(argv) {
  let opts;
  try {
    opts = parseArgs(argv);
  } catch (error) {
    console.error(`[fast] ${error.message}`);
    return error.code ?? EXIT.USAGE;
  }
  if (opts.help) {
    console.log(usage());
    return EXIT.OK;
  }
  const logger = new Logger(opts.verbose);
  try {
    if (opts.selfTest) return await selfTest(opts);
    if (opts.helperCheck) return await helperCheck(opts);
    if (opts.driverCheck) return await driverCheck(opts);
    if (opts.legacy) return await runLegacy(opts);
    return await runE2e(opts, logger);
  } catch (error) {
    if (error instanceof E2eError) {
      console.error(`[fast] 失败（退出码 ${error.code}）：${error.message}`);
      return error.code;
    }
    console.error(`[fast] 内部错误：${error?.stack ?? error}`);
    return EXIT.SCENARIO;
  }
}

main(process.argv.slice(2)).then((code) => {
  process.exitCode = code;
});
