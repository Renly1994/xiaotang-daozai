# Agent 相关功能复测报告（2026-11-20 第三轮，GitHub 最新版）

> 触发：用户「帮我重新测试下 agent 相关功能是否符合设计预期」。
> 被测构建：本机从 `Angelyeye/xiaotang-daozai` 最新 `main`（`56fd0b1`）重新 `cargo build --release` 的产物。
> 方法：自动化单测/桥接测试 + `scripts\agent-e2e.ps1` / `scripts\agent-e2e-repro.ps1` 真机流程（真实 DSH 宿主驱动 → 屏幕真实鼠标点击 → 桥接决定校验）+ 用户 live 配置（`island_size=100`、`content=0`、`dwell=5`）下的 1:1 截图复核。

---

## 0. 结论先行

| 用户/审计关心点 | 设计预期 | 本轮结果 |
|---|---|---|
| 完成卡「查看回复」点了没反应（bug A，content=0） | 打开本机说明层，说明「未接收正文」并给设置指引，不给「复制内容」 | **通过**（E2E-4 / done-repro；live 100% 配置下 1:1 截图复核同样通过） |
| 提问卡「切换到客户端」没反应（bug B） | 无论成功/已在前台/失败都要有可见结论，且不再用资源管理器冒充成功 | **通过**（E2E-7 两种前后台情形 + E2E-8 收起态 chip 都有 toast 结论，前台确实切到 DSH） |
| 审批 / 提问 / 完成三条主链路 | 屏幕上点的按钮 ↔ 桥接收到 ↔ 宿主最终决定三者互证 | **全部通过**（approval=allowed-once、question 单选/多选/多轮=answers、done=done） |
| 长回复滚动（D03）、卡片内滚动不被页导航吞（D04） | 结果层/审批正文可滚动、不弹回、不翻岛页 | **通过**（E2E-5 4016 字滚到 EOF；E2E-6 60 行命令在卡内滚动、按钮不位移） |
| 子 agent 完成噪声（D05） | 子会话结束不推完成卡 | **通过（单测级）**：`node --test tests/agent-notices.test.mjs` 12/12；真机需重启 DSH 才加载新插件，本轮未重启（会中断当前会话） |
| 测试基建 | 一套脚本可重复给出「屏幕↔桥接」互证 | **发现 1 个脚本缺陷**：`agent-e2e.ps1 -Scenario question-multi-turn` 会点到题干里的同名词（详见 §4），产品本身用手工/probe 复测通过 |

**没有发现产品级回归**；用户报的两处死点击、以及审计中的 D03/D04 均已在最新构建上按设计工作。残留的仍是审计里未修的 P2（D08/D09：桥接断开/宿主撤销时卡片静默消失），本轮在「杀桥接」实验中被复现（§4.3）。

---

## 1. 被测构建

| 项 | 值 |
|---|---|
| 源码 | `main @ 56fd0b1`（与 `origin/main` 一致，工作区干净） |
| exe | `target/x86_64-pc-windows-msvc/release/daozai.exe` |
| 大小 / SHA256 | 22,762,496 B / `B047076AA72591DDB6BB360C9783190D05C7694BF7B437EB581EAE98B56B4681`（2026-11-20 14:35:37） |
| 构建 | `powershell -NoProfile -ExecutionPolicy Bypass -Command ". .\scripts\dev-msvc-env.ps1; cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc"`（本机 VS 18 BuildTools，3m54s） |
| 测试机差异 | 本轮控制台是 1920×1080 @ 96 DPI（文档历史记录为 3440×1440），脚本取景/OCR 仍是「屏幕中心固定竖带」，无需改参数 |

---

## 2. 自动化回归

| 命令 | 结果 |
|---|---|
| `cargo test --lib ui_tests -- --test-threads=1` | **435 passed / 0 failed / 49 ignored**（167s） |
| `cargo test --lib agent -- --test-threads=1` | **57 passed / 0 failed / 3 ignored**（1.4s） |
| `node --test tests/agent-notices.test.mjs` | **12 / 12 passed**（含 `DSH subagent turns never push a completion card`、审批 allow 必须带完整 operation 且无旁路、live broker 并发/取消/隐私） |

---

## 3. 真机 E2E（真实宿主驱动 + 真实鼠标点击 + 桥接校验）

命令统一为（在仓库根目录）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Scenario <场景> -Exe target\x86_64-pc-windows-msvc\release\daozai.exe
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e-repro.ps1 -Scenario <场景> [-DshFront|-DshBack] -Exe target\x86_64-pc-windows-msvc\release\daozai.exe
```

| # | 场景 | 屏幕上发生的事 | 桥接/宿主侧 | 结果 | 证据目录 |
|---|---|---|---|---|---|
| E2E-1 | 审批全链路 | 「等待你审批」→ 自动展开详情 → 点「仅允许这一次」(1143,387) | `reply={"decision":"allow"}` → `decision=allowed-once`，`handback=false` | **通过**（20.9s） | `agent-e2e-shots/20261120-145850/` |
| E2E-2a | 提问单选 | 「需要你回答」→ 点「冒烟测试」→「检查答案」→「确认提交」 | `decision.answers[0].selected=["冒烟测试"]` | **通过**（41s） | 同上 |
| E2E-2b | 提问多选 | 点「行高」+「滚动」→ 审阅 → 确认提交 | answers 含两项，`handback=false` | **通过**（47.6s） | `agent-e2e-shots/20261120-150437/` |
| E2E-2c | 提问多轮 | **脚本失败**：Q1 点到了题干（含「方案一」）而不是选项行 | 未作答 → 驱动超时 | **脚本问题，非产品**（§4.1；手工/probe 复测通过） | `agent-e2e-shots/20261120-150035/` |
| E2E-3 | 完成通知 content=2 | 「本轮已结束」→ 点「查看回复」→ 结果层「本轮回复」 | `reply.done=true`、`mode=2`、正文 83 字、`decision=done` | **通过**（24.4s） | `agent-e2e-shots/20261120-145850/` |
| E2E-4 | 完成卡 content=0（bug A） | 摘要行「仅状态 · 未接收回复正文」→ 点「查看回复」→ **本机说明层**（标题「本轮未接收回复正文」、副标题「本机说明 · 不含回复内容」、设置指引、只有「关闭」、无「复制内容」） | `decision=done`、`mode=0`、`textLen=0`（尊重 content=0 不采集正文） | **通过**（24.2s） | `20261120-150541-done-repro/` |
| E2E-5 | 长回复滚动（D03） | 4016 字结果层滚轮到底，看到末尾标记 `E2E-LONG-TAIL-MARK` | `mode=2`、`textLen=4016`；`pageFlip=false` | **通过** | `20261120-151115-scroll-repro/` |
| E2E-6 | 长命令审批卡内滚动（D04） | 140% 隔离实例、60 行 JSON：卡内滚轮从 line7 滚到 line29，按钮行不位移，岛体未翻页 | —（视觉+截图判定） | **通过** | `%TEMP%\sc-1.png` / `sc-2.png`（本轮手工留档） |
| E2E-7a | 跳转 chip（DSH 不在前台） | 前台 explorer → 点「切换到客户端」(1134,42) | 1.5s 后前台 = DeepSeek Harness；岛下 toast「已切到 DeepSeek Harness」；随后卡片仍可作答，2 题答案正常回桥接 | **通过** | `20261120-150719-jump-repro/` |
| E2E-7b | 跳转 chip（DSH 已在前台） | 点 chip 后前台仍是 DSH | 仍有可见 toast（实测文案「已切到 DeepSeek Harness」而非「已在最前台」，见 §4.2） | **通过（文案有细微偏差）** | `20261120-150844-jump-repro/` |
| E2E-8 | 收起态提示卡 chip（原死点击） | 点收起图标 → 提示卡点 chip (1174,42) | 前台切到 DSH + toast「已切到 DeepSeek Harness」，提示卡保留、未产生决定 | **通过** | `20261120-151023-banner-jump-repro/` |
| E2E-4' | live 配置复核 | 用户真实 `%APPDATA%\Daozai` 配置（island_size=100、content=0、dwell=5）：1:1 截图确认完成卡摘要行 + 说明层文案与 140% 一致 | 桥接 `{"done":true}` | **通过** | `%TEMP%\live-done-1.png`、`live-done-hint2.png` |

---

## 4. 发现的问题

### 4.1【测试基建｜须修脚本】`agent-e2e.ps1 -Scenario question-multi-turn` 会点到题干里的同名词

- 现象：Q1 的题干是「岛仔 E2E 多轮 1/2：请选择「方案一」」，OCR 只识别到题干那一行（y=124），没识别到选项行（y≈173），`-PreferBottom` 自然选了唯一的命中 → 点题干 → 「已答 0」不变 → 「下一题」禁用 → Q2 永远到不了 → 报「没有点到期望的选项」。
- 证据：`20261120-150035/question-multi-turn-03-next.png`（点击后截图仍是「第 1/2 题 · 已答 0」，`01` 与 `02` 两张截图哈希完全相同）。
- 产品侧反证（同一构建、同一驱动场景）：
  1. `agent-card-probe.mjs question-rounds` 两题：点「审批卡」→ 已答 1 → 「下一题」→ 点「保留」→「检查答案」→ 审阅页「作答总览 共 2 题 / 已答 2/2 题」→「确认提交」→ 桥接返回两题 answers；
  2. `agent-e2e-repro.ps1 -Scenario jump-repro`（内部同样跑 `question-multi-turn`）：Q1「冒烟测试」、Q2「确认一」均命中并回桥接。
- 建议：Q1 选项改用「行首 x 坐标/行高 + 文本双条件」，或直接复用 `agent-e2e-repro.ps1` 的 `Find-TextSmart`（它会排除题干行、并在按钮未命中时按 footer 几何兜底）；也可把测试数据里的题干改成不含选项名（如「请选择第一个选项」）。

### 4.2【P3｜文案】DSH 已在前台时跳转结论仍报「已切到」

- `agent_jump.rs::decide()` 的 `AlreadyFront` 分支要求 `GetForegroundWindow() == 选中窗口`；而候选窗口按「有标题 + 面积大」挑，Electron 主窗与前台窗口不一定是同一个 hwnd，于是走 `Focused` → 「已切到 DeepSeek Harness」。用户能看到明确结论、前台也确实在 DSH，体验可接受，但与设计里「已在前台」这一种结论对不上。可考虑把前台判定放宽为「同一 PID 的任一可见窗口在前台」。

### 4.3【已知 P2 复现，未修】桥接断开时卡片静默消失

- 实验：审批详情卡打开时杀掉岛仔桥接的 `node.exe` 子进程，再点「仅允许这一次」→ 卡片直接消失、无任何提示，探针拿到 `null`。这就是审计 D08/D09（「桥接子进程退出/来源关闭时队列静默清空」「宿主撤销时卡片静默消失」），当前仍是预期外体验，需要后续排期（复用 toast/jump_note 通道给出原因）。
- 也正因此，E2E-10（提交失败 → 「提交失败 · 结果未知」）无法用「杀桥接」的方式验证；D06 的修复目前由单测 `allowing_without_a_bridge_keeps_the_card_and_marks_result_unknown` 覆盖。

---

## 5. 本轮未覆盖

1. **E2E-11 真实 DSH 审批**（`/permission read-only` + 真实模型越权重试）：需要切换会话权限预设并重启 DSH 桌面端，本轮不重启宿主（会中断当前会话）。本轮用「真实宿主驱动 + 真实桥接 + 真实点击」覆盖了同一段 UI/协议。
2. **E2E-12 子 agent 完成噪声真机复测**：同上，需要重启 DSH 让新 `index.mjs` 进进程；目前只有 node 回归（12/12）覆盖。
3. 点外关闭 / 收起 / 交回三分（E2E-9）只做了部分：`banner-jump-repro` 验证了「收起后驱动仍 pending、未产生决定」，其余由 `ui_tests` 单测覆盖。

---

## 6. 复现命令

```powershell
# 全场景（审批 + 单选提问 + 完成）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Scenario all

# 多选 / 多轮
... -Scenario question-multi
... -Scenario question-multi-turn   # 已修：选项行按「只比选项词多一个序号」过滤，不再误点题干

# 两处用户报的死点击
... -File scripts\agent-e2e-repro.ps1 -Scenario done-repro
... -File scripts\agent-e2e-repro.ps1 -Scenario jump-repro -DshBack
... -File scripts\agent-e2e-repro.ps1 -Scenario jump-repro -DshFront
... -File scripts\agent-e2e-repro.ps1 -Scenario banner-jump-repro
... -File scripts\agent-e2e-repro.ps1 -Scenario scroll-repro -AgentContent 2
```

> 跑之前确认没有其它 daozai 实例（脚本检测到会退出码 4，不会杀别人的进程）；跑期间请勿操作鼠标键盘（脚本会移动并点击真实指针）。

---

## 7. 修复记录（2026-11-20 第二批次，用户要求「一并修复」）

### 7.1 §4.1 测试脚本误点题干 → 已修

改动（`scripts/agent-e2e.ps1`，`scripts/agent-e2e-repro.ps1` 同步）：

| 位置 | 改动 |
|---|---|
| `Find-LineMatch` / `Find-InDump` | 新增 `-MaxExtra <n>`：只接受「去空格后行内字数 ≤ 目标词字数 + n」的行 |
| `Find-AnyText` / `Find-TextSmart` | 透传 `MaxExtra` |
| 提问卡选项点击 | `-Scales @(3,4,5,6,2) -MaxExtra 2`。实测 140% 缩放下选项行常只有 5~6 倍率能读出（2 倍率只剩题干那一行）；`MaxExtra 2` 把「1方案一」留下、把题干「…请选择「方案一」」滤掉 |
| 点完选项 | 增记一步「选项已选中（已答 N）」，作为选中态生效的留证（不单独判失败） |
| 启动自检 | 新增锁屏检测：`LogonUI.exe` 存在时直接 `exit 2` 并说明「锁屏时截图只有锁屏背景、点击落不到岛上」，避免把锁屏误判成代码回归（本轮实测踩过一次：锁屏下报了「25 秒内没有看到卡片」） |

复跑证据（新构建 `E7223E60…`）：

- `agent-e2e-shots/20261120-155529/`：`question-multi-turn` **通过（51.4s）**。选项 1 命中行文本 `1 《 方 案 一`（不再是题干），并留下「已答 1」、Q2 命中 `1 《 确 认 一`、`已答 2`、审阅页确认提交；桥接 `answers=[{方案一},{确认一}]`。
- `agent-e2e-shots/20261120-161132-jump-repro/`：`jump-repro -DshFront` **全流程通过**，其中 Q2 选项「确认一」修复前读不出、修复后命中（`answers=2 handback=false`）。
- `agent-e2e-shots/20261120-161624-jump-repro/`：`jump-repro -DshBack` **通过**（explorer → DeepSeek Harness，toast「已切到」，两题答案回桥接）。

回归：`-Scenario all`（approval 23.1s / question 42.1s / done 24.7s）在新构建 + 新脚本下 **三场景全绿**，说明选项搜索改动没有影响其它场景。

### 7.2 §4.2 「已在前台却报已切到」→ 查明是测法误判，产品行为正确；顺带加固 + 加诊断

加了 `DAOZAI_AGENT_TRACE=1` 时的跳转决策日志（`%TEMP%\daozai-jump-trace.log`），真机跑出来：

```
[jump] client=DeepSeek Harness found=true hwnd=HWND(0x20128) iconic=false foreground=false fg_hwnd=HWND(0x9d08fe) fg_pid=18612
[jump] step=Raise outcome=Focused { client: "DeepSeek Harness" } fg_hwnd=HWND(0x20128) fg_pid=7848
```

`fg_pid=18612` 是**岛仔自己**（当次被测实例 PID）。也就是说：点 chip 这一下先把岛仔的那个窗口顶到了前台，**跳转发生的那一刻客户端确实不在前台**，所以 `Focused`（「已切到」）是如实结论；脚本记录的「点击前前台 = DSH」是点击之前测的，两者不矛盾。原 §4.2 的判断是测法导致的误判，不是文案 bug。

同时做了两处低风险加固（保留）：

1. `agent_jump::foreground_is_client`：判定从「前台窗口 == 选中窗口」放宽为「前台窗口属于客户端进程」（Electron 多窗口场景更准），并抽了纯函数 `front_counts_as_client` + 单测 `foreground_of_the_client_process_counts_as_already_front`。
2. 上述 `DAOZAI_AGENT_TRACE` 跳转决策日志（不设环境变量时零开销、不落盘），以后遇到「跳转提示与肉眼不符」可直接看 `fg_pid` 判断当时谁在前台。

### 7.3 第二轮验证汇总（新构建 `E7223E60…`，2026-11-20 16:08）

| 项 | 结果 |
|---|---|
| `cargo test --lib`（全量） | **1357 passed / 0 failed / 132 ignored**（278s） |
| `cargo test --lib agent_jump` | 18 passed / 0 failed（含新增用例） |
| `agent-e2e.ps1 -Scenario all` | approval / question / done **全绿** |
| `agent-e2e.ps1 -Scenario question-multi-turn` | **通过**（原失败场景） |
| `agent-e2e-repro.ps1 -Scenario jump-repro -DshFront` | **通过**（含 Q2 选项修复） |
| `agent-e2e-repro.ps1 -Scenario jump-repro -DshBack` | **通过** |
| 脚本 BOM | `scripts/agent-e2e.ps1`、`scripts/agent-e2e-repro.ps1` 均为 `EF BB BF`（改中文脚本后必须核对，本轮已踩过一次 BOM 丢失） |
### 7.4 本机交付入口（原 AGENTS.md 的准确版本，2026-11-20 核对）

- 本机（用户 59531）桌面「岛仔－当前测试版」快捷方式**直接指向** `target/x86_64-pc-windows-msvc/release/daozai.exe`；
  旧 AGENTS.md 写的副本路径 `C:\Users\Administrator\Daozai\daozai.exe` 是另一台机器的，本机不存在。
- 交付流程：`cargo build --release --bin daozai --target x86_64-pc-windows-msvc` → 停掉正在运行的岛仔 →
  启动新 exe → 核对进程路径与 SHA256 与构建产物一致。
- 构建环境用 `scripts/dev-msvc-env.ps1`（自动探测 VS BuildTools、钉死 CC/CXX/AR）。
- 自测入口：双击仓库根目录 `测试Agent功能.cmd`（菜单选场景），证据在 `docs/superpowers/research/agent-e2e-shots/<时间戳>/`。