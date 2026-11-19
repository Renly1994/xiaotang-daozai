# 岛仔 Agent 界面 E2E 验收（真实 DSH 宿主 → 屏幕点击 → 桥接校验）

- 日期：2026-11-20
- 相关任务：真实 DSH 端到端验收基建（审批 / 提问 / 完成通知）
- 相关文件：
  - `tests/fixtures/agent-drive.cjs`：真实宿主驱动（发请求 + 收决定）
  - `scripts/agent-e2e.ps1`：一键流程（起岛仔 → 发请求 → OCR → 点击 → 校验 → 清理）
  - `scripts/agent-screen-ocr.ps1`：截图 + Windows OCR（输出文字/坐标 JSON）
  - `scripts/agent-card-probe.mjs`：**只发请求、不验证宿主**的轻量探针（UI 迭代期用，本任务复用不重写）
  - 截图与结果：`docs/superpowers/research/agent-e2e-shots/<时间戳>/`（正式跑产生）

---

## 1. 这套验收在验什么

不是“模拟桥接回复”，而是把整条链路真跑一遍：

```
已安装的 DeepSeek Harness（ELECTRON_RUN_AS_NODE=1，Node 模式另起进程）
   ├─ 真实插件：dsh-session / dsh-session-projection / dsh-agent /
   │            dsh-user-approval / dsh-user-questions
   ├─ 真实 bundle：@daozai/agent-notices（岛仔导出的 index.mjs → channel.cjs 客户端）
   └─ 命名管道 ──► 岛仔（daozai.exe 自己 spawn 的 channel.cjs 服务端）
                        │
                        ├─ 屏幕：卡片弹出（egui 自绘，无 UIA 文本）
                        ├─ 本套基建：截图 → Windows OCR → 定位 → 真鼠标点击
                        └─ 决定写回管道 ──► 驱动进程打印 JSON
```

判据是“**屏幕上点的按钮**”与“**桥接侧收到的决定**”必须相互印证：

| 场景 | 屏幕上做什么 | 桥接侧必须收到 | 宿主最终结果 |
|---|---|---|---|
| approval | 点「仅允许这一次」 | `{"decision":"allow"}` | `allowed-once` |
| question | 选一个选项 → 点「提交回答」 | `{"decision":"answer","answers":[{"id":"…","selected":["甲选项"]}]}` | `{answers:[…]}` |
| done | （无需点击）确认完成卡带正文 | 完成事件被接受 `{"done":true}` | 驱动 `decision="done"` |

任何一步“卡片没弹出来 / 点不到 / 桥接没收到 / 收到的不一致”，脚本都会以非 0 退出并在 `summary.json` 里写清原因。

---

## 2. 前置条件

1. 已安装 DeepSeek Harness 桌面端：
   `%LOCALAPPDATA%\Programs\DeepSeek Harness\DeepSeek Harness.exe`
2. 岛仔 release exe 已构建：
   `target\x86_64-pc-windows-msvc\release\daozai.exe`
   （构建：`powershell -NoProfile -Command ". .\scripts\dev-msvc-env.ps1; cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc"`）
3. **没有其它 daozai 实例在跑**（单实例互斥体 `Local\Daozai.SingleInstance`，两边会互踢；脚本检测到会直接退出码 4，不会杀别人的进程）。
4. 系统有中文 OCR 语言包（Win10/11 自带 `zh-Hans-CN`；脚本启动时会自检并打印语言）。
5. 屏幕缩放 100%（本机 3440×1440 @ 96 DPI，实测坐标 1:1）。

脚本自己会：建独立 `DAOZAI_DATA_DIR`（不碰 `%APPDATA%\Daozai`）、独立 `DSH_HOME`/`CODEX_HOME`（不碰真实的 DSH profile）、把 exe 复制成副本再运行（不锁构建产物）。

---

## 3. 一键跑法

```powershell
# 全场景：审批 + 单选提问 + 完成通知
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1

# 只跑审批
... -Scenario approval

# 指定 exe / 数据目录 / 等待上限
... -Scenario approval -Exe target\x86_64-pc-windows-msvc\release\daozai.exe -DataDir D:\tmp\daozai-e2e -WaitSeconds 120

# 跑完保留岛仔进程与数据目录，便于人工看现场（默认跑完即关）
... -KeepAlive

# 只验证“卡片弹出来 + OCR 能读到”，不点击（UI 迭代期用）
... -Scenario all -NoClick
```

可用场景：`approval`、`question`（单选）、`question-multi`（多选）、`question-multi-turn`（两题多轮）、`done`、`all`（= approval + question + done）。

退出码：

| 码 | 含义 |
|---|---|
| 0 | 全部场景通过 |
| 2 | 前置条件不满足（找不到 exe/DSH/OCR、未知场景） |
| 3 | 场景失败（卡片没出现、按钮识别不到、桥接没收到预期决定、驱动超时） |
| 4 | 已有 daozai 实例在跑（单实例冲突） |

---

## 4. 产物与判读

每次运行写入 `docs/superpowers/research/agent-e2e-shots/<时间戳>/`：

| 文件 | 内容 |
|---|---|
| `<场景>-driver.json` | 驱动输出：`sent`（发给岛仔的原文）、`bridge`（桥接请求与回复）、`decision`（宿主拿到的决定）、`handback` |
| `<场景>-driver-input.json` | 传给驱动的场景参数（走 `--json-in`，避免命令行引号二次解析） |
| `<场景>-driver.err.log` | 驱动 stderr（为空才算干净） |
| `<场景>-01-*.png` | 卡片刚出现时的 1:1 截图 |
| `<场景>-02/03-*.png` | 详情/选中态/结果层截图 |
| `summary.json` | 汇总：`ok`、每个场景的 `steps`（步名、是否命中、OCR 原文）、`failures`、`log` |

驱动输出字段（`tests/fixtures/agent-drive.cjs`）：

| 字段 | 含义 |
|---|---|
| `sent` | 本轮请求的原文（tool/operation/reason、questions、完成正文） |
| `bridge[]` | 每一次桥接往返：`provider`、`kind`、`mode`（`policy.content`：0 仅状态 / 1 摘要 / 2 全文）、`sent`（真正写进管道的 payload）、`reply`（岛仔写回的决定） |
| `decision` | 宿主插件最终拿到的结果：`allowed-once` / `rejected` / `{answers:[…]}` / `done` / `timeout` |
| `handback` | 岛仔没有接管、请求被交回客户端（E2E 里算失败） |

驱动退出码：`0` 成功；`2` 初始化失败（profile/插件/live.json）；`3` 等待超时；`4` 请求被交回客户端。

判读要点：

- `steps[].ok` 全为 true 且场景 `ok=true`，才算通过；`fatal=false` 的步骤只是辅助证据（比如“审批正文包含操作内容”），不参与判定。
- `driver.bridge[0].reply` 是**岛仔写回管道的内容**，`driver.decision` 是**宿主最终拿到的值**，两者与屏幕上点的按钮必须一致：
  - 审批：`reply.decision="allow"` → `decision="allowed-once"`（宿主只在“岛仔 allow 且操作内容匹配”时才给这个值，所以它本身就是强证据）；
  - 提问：`reply.decision="answer"` 且 `selected` 含屏幕上点的选项；
  - 完成：`reply.done=true`、`mode=2`（全文）、`sent.text` 非空 → `decision="done"`。
- `driver.handback=true` 表示岛仔没接管、请求被交回客户端——这在 E2E 里算失败（屏幕侧根本点不到）。

已做的**负路径**验证（不需要启动岛仔，只跑驱动）：

| 场景 | 期望 | 实测 |
|---|---|---|
| 没有 `live.json`（岛仔没导出/没起） | 退出码 2 + 明确提示 | `error="live.json 未出现或岛仔没有打开 DSH 来源…"`，exit 2 |
| `live.json` 指向已消失的管道（岛仔被强杀） | 退出码 4 + `handback=true` | `decision="rejected"`、`handback=true`、`bridge[0].reply=null`，exit 4，提示“岛仔没有接管这条审批，请求被交回客户端” |

---

## 5. 屏幕识别与点击方案

### 5.1 为什么必须 OCR

岛仔是 egui 自绘窗口，没有 Win32/UIA 文本节点，`mcp__windows__Snapshot` 的 UI Tree 里看不到按钮文字；只能“截图 + 文字识别”。

### 5.2 用什么 OCR

Windows 自带的 `Windows.Media.Ocr`（WinRT），经 Windows PowerShell 5.1 调用：

- 本机离线、无第三方依赖、无需联网/装包；
- 本机可用识别语言：`zh-Hans-CN`（脚本会打印确认）；
- 输出每行/每词的矩形框 → 换算成**屏幕绝对坐标**，可直接点击。

`scripts/agent-screen-ocr.ps1` 的用法见文件头注释，核心参数：

```powershell
# 一次截图 + 多倍率轮试找文字，命中即返回中心坐标
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-screen-ocr.ps1 `
  -Region 1272,0,2616,560 -Find "仅允许这一次" -FindScales 2,3,4,6 -SaveImage shot.png
```

### 5.3 关键结论：100% 界面缩放下 OCR 不可用，140% 可用

- **岛仔默认 `island_size=100`（界面 100%）时**：卡片正文/按钮只有 11–12px，Windows OCR 把「查看并审批」读成「0 看 艹」、「回客户端」读成「酩户端」，2 字按钮基本读不出来 → 无法可靠定位点击。
- 把 `island_size=140`（设置里的界面尺寸上限）后，字号 ×1.4，配合 `-FindScales 2`：
  - 「等待你审批」「回客户端」「查看并审批」→ **稳定识别**
  - 「仅允许这一次」「拒绝」「甲选项」「提交回答」「本轮已结束」→ **稳定识别**
- 因此 `scripts/agent-e2e.ps1` 在隔离数据目录里写 `island_size=140` 再起岛仔。
  **这是“显示缩放”设置，不改变任何功能逻辑**；但正式验收如果需要 100% 外观，请以 `-NoClick` 方式跑截图，点击闭环用 140% 跑（两者都记录在案）。

### 5.4 为什么多倍率（`-FindScales`）

同一按钮在不同放大倍率下成功率不同：

- 长按钮（4–6 字，如「仅允许这一次」「查看并审批」）：2–4 倍最稳；
- 短按钮（2 字，如完成卡上的「查看」「关闭」）：要 6–8 倍才偶尔命中。

脚本会按 `2,3,4,6`（必要时 `2,3,4,5,6,8`）依次尝试，命中即返回；每轮只截一次图，避免重复截图开销。

### 5.5 点击方式

`scripts/agent-e2e.ps1` 用 `SetCursorPos` + `mouse_event`（真正的鼠标输入）点击 OCR 给出的坐标，不依赖 MCP 工具，脚本可独立重复执行。

点击链路单独做过验证（不涉及岛仔）：临时建一个 430×150 的 WinForms 窗口（黑底、12px 中文标签「仅允许这一次」），

1. 用同一套 `EnumWindows` + PID 找到窗口 rect（`2990,1222,3420,1372`）；
2. `agent-screen-ocr.ps1` 在 2 倍率识别出「仅允许这一次」，给出中心 `(3102,1274)`；
3. `SetCursorPos` + `mouse_event` 点击该坐标，窗口收到点击的坐标正好是 `(3102,1274)`。

同一个验证里还得到一个对照结论：**普通 Win32 12px 中文字在 2 倍率下可稳定识别**；而岛仔 100% 缩放下 10.5–11px 的灰字胶囊按钮（浅灰字 + 深灰底 + 抗锯齿）不可识别——这正是要用 140% 的原因。

### 5.6 定位失败时的兜底

- 优先用 OCR 的**行内字符比例**估算目标中心（中英混排也够用）；
- 同一个词在题干和选项里都出现时（如「请选择「冒烟测试」」+ 选项「冒烟测试」），取**最靠下**的命中（选项一定在题干下面）；
- 每一步失败都会留截图 + `summary.json` 里的 OCR 原文，便于人工复核。

### 5.7 OCR 区域与截图隐私

- **不能按窗口中心取区域**：岛仔窗口画布宽 `CANVAS_W=960`（逻辑），岛体只有 `WIN_W=640`，两侧是透明区；实测窗口中心 1944，而卡片中心 1712 ≈ **屏幕中心 1720**。所以 OCR 区域取「屏幕中心 ±（419×scale/2 + 70）」的竖带、y 到窗口底（≤640），既覆盖卡片又不带两侧桌面。
- `-SaveImage` 默认再配合 `-CropSave`：按本次 OCR 文字行的并集再裁一次（±28px），验收文档里的截图只包含卡片区域，不会把浏览器/1Panel 等桌面内容带进去。
- **踩过的坑**：按窗口中心取区域会把卡片裁掉一半；MCP 截图的 `save_path` 只存缩略图，1:1 截图必须用脚本自己的 GDI 抓屏。

---

## 6. PoC 结论（已手工实测通过的部分）

以下闭环在 2026-11-20 01:56–02:12 用**同一个真实宿主驱动**手工跑通（后来由 `scripts/agent-e2e.ps1` 固化成一条命令）：

1. **审批闭环（allow）**
   - 宿主发出：`tool=Bash`、`operation={"command":"echo daozai-e2e"}`、`reason=…`
   - 屏幕：卡片弹出「DeepSeek Harness · 等待你审批」→ 点「查看并审批」→ 详情页 → 点「仅允许这一次」
   - 桥接侧收到：`{"decision":"allow"}`；宿主结果：`decision="allowed-once"`、`handback=false`、`ok=true`
2. **提问闭环（单选）**
   - 宿主发出：`questions=[{id:"e2e-single",question:"…",options:[甲选项,乙选项,丙选项]}]`
   - 屏幕：卡片弹出「需要你回答」→ 点「甲选项」→ 点「提交回答」
   - 桥接侧收到：`{"answers":[{"custom":"","id":"e2e-single","selected":["甲选项"]}],"decision":"answer"}`
3. **完成通知闭环**
   - 宿主发出：`assistant/message` + `turn/end`（正文含唯一标记 `daozai-e2e-done`）
   - 桥接侧收到：`{"done":true}`；屏幕：卡片显示「本轮已结束」+ 正文摘要（OCR 读到「E2E 完成正文：岛仔端到端验收·标记 daozai-e2e-done」）

已留档的 PoC 证据（`docs/superpowers/research/agent-e2e-shots/manual-poc/`）：

- `banner-approval-100pct-mcp-preview.png`：100% 缩放下的审批卡（人工可读，OCR 不可读）
- `banner-approval-140pct-mcp-preview.png`：140% 缩放下的审批卡
- `done-card-140pct-1to1.png` + `done-card-ocr.json`：140% 完成卡 1:1 截图与 OCR 结果（`-Find daozai-e2e-done` 1 倍率即命中）

> 说明：上面两张 banner 图是 MCP 截图的**缩放副本**（MCP 的 `save_path` 存的是预览尺寸），只作外观留档；坐标与识别结论均来自脚本自己的 1:1 GDI 截图。正式跑会在 `<时间戳>/` 下生成全流程 1:1 截图。

**正式跑记录（2026-11-20 05:06，最终构建）**

- 二进制：`target\x86_64-pc-windows-msvc\release\daozai.exe`
  - 大小 **22,748,672** 字节
  - SHA256 **9789C59E62AB775A501F77B0FF30AE50734AB5FBA08A04EA07433CFB37022DC1**
  - 构建时间 2026-11-20 04:52:48
- 命令：`powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Scenario all -Exe target\x86_64-pc-windows-msvc\release\daozai.exe`
- 结果：**exit 0，`summary.json` 的 `ok=true`，三个场景全绿**（approval 32.6s / question 46.0s / done 31.1s，总计约 110s）
- 证据目录：`docs/superpowers/research/agent-e2e-shots/20261120-050612/`
  - `summary.json`：每步 `steps[].ok=true`，`bridgeOk=true`，`driver` 字段带完整 JSON
  - `approval-driver.json`：`bridge[0].reply={"decision":"allow"}`、`decision="allowed-once"`、`handback=false`
  - `question-driver.json`：`bridge[0].reply={"answers":[{"selected":["冒烟测试"]}],"decision":"answer"}`、`decision.answers[0].selected=["冒烟测试"]`
  - `done-driver.json`：`bridge[0].reply={"done":true}`、`mode=2`、`sent.text` 全文 83 字、`decision="done"`
  - 截图（已裁到卡片，1:1）：`approval-01-banner / 02-detail / 02-operation`、`question-01-question / 02-option / 05-submit / 06-review`、`done-01-done / 02-review / 03-result / 03b-body`
- 这一轮的 UI 交互路径（最终卡片）：
  - 审批：卡片自动展开详情 → OCR 读「等待你审批 / DSH · 权限请求 / 操作内容 / 申请理由」→ 点「仅允许这一次」(1903,387) → 宿主 `allowed-once`
  - 提问：OCR 读「需要你回答 / 第 1/1 题 · 单选 · 已答 0」→ 点选项「冒烟测试」→ 点「检查答案」→ 审阅页点「确认提交」→ 宿主 `selected=["冒烟测试"]`
  - 完成：卡片「本轮已结束 + 正文」→ 点「查看回复」→ 结果层「本轮回复」（无序列表已是圆点）→ OCR 命中 `daozai-e2e-done`

**跑通前的最后修正**（都是这一轮之前发现的脆弱点）：

1. done 的「查看回复 / 结果层」改为非致命步骤（完成通知本身不点也算 done），判定 = 卡片出现 + 正文含 `daozai-e2e-done`（结果层优先，卡片兜底）+ 桥接 `done:true`；
2. 场景通过条件改为 `无致命错误 && 桥接校验通过` 两个都满足（修掉「ok=true 但 error 有值」的不一致）；
3. OCR 区域取不到窗口矩形时回退「上一次可用矩形 → 屏幕中心竖带」，OCR 子进程失败不再中断整个脚本；
4. OCR 从「每个候选词各跑一遍、每遍多倍率」改为**每次轮询只跑 1-2 遍单倍率扫描**（一次子进程 + 本地匹配），点击前耗时从 10s+ 降到 2s 内，避免完成卡在 15s 停留窗口内过期；
5. 截图裁边：`-CropSave` 按文字包围盒 + `-CropCard` 按「卡片连续深色行列」收边（列统计只在卡片行范围内做，否则矮卡片裁不掉），验收截图只剩卡片本身。

**历史记录（已被上面覆盖）**：03:52 那轮（exe SHA256 9D871E87…678F）也用同一套判据跑通了三场景，用于验证判定逻辑本身；最终交付证据以 05:06 这轮为准。
## 7. 正式跑之前的检查清单

1. 退出所有 daozai 实例：`Get-Process daozai*` 应为空；
2. 确认要验收的 exe 是最新构建（脚本会打印 exe 时间戳）；
3. 桌面无人操作（脚本会移动鼠标点击，约 2–3 分钟/场景）；
4. 跑：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Scenario all
```

5. 看输出：`summary.json` 的 `ok=true`、`scenarios[].steps[].ok` 全 true；
6. 把 `<时间戳>` 目录作为本次验收证据，更新本文件第 6 节的正式跑记录（含 exe 哈希）。

---

## 8. 已知限制与坑（都踩过）

| 现象 | 原因 | 处理 |
|---|---|---|
| 两个 daozai 互相踢 | 单实例互斥体 `Local\Daozai.SingleInstance` | 脚本跑前检查 `daozai*` 进程，有就退出码 4；不会杀别人的 |
| `FindWindowW(null,'岛仔')` 找不到窗口 | 中文标题跨进程匹配不稳 | 改为 `EnumWindows` + `GetWindowThreadProcessId` 按 PID 找窗口 |
| MCP 截图的 `save_path` 只有缩略图 | MCP 存的是预览尺寸（3440 宽缩到 1920、区域 850×560 缩到 637×420） | OCR 一律用脚本自己的 GDI 1:1 截图；MCP 图只作外观留档 |
| 100% 缩放 OCR 读不出按钮 | 11–12px 中文 + 深色胶囊按钮 | 验收用 `island_size=140`；或退回“已知布局坐标”混合方案（本脚本未采用，因为 UI 迭代中布局会变） |
| 完成卡的「查看/关闭」识别不出 | 轻提示形态只有 10.5px 灰字小按钮 | 完成场景以“正文标记 OCR 命中”为准；能读到「查看回复/查看」才点（当前构建可能读不到，`summary.json` 会标 `fatal=false` 的步骤失败） |
| 按钮文案随构建变化（如「提交回答」/「检查答案」/「确认提交」） | 主 agent 正在改 UI | 脚本对每个动作都给候选文案列表，命中任一即可；OCR 结果留在 steps 里便于对照 |
| 驱动 `assistant/message` 报 surfaceOp 错误 | DSH 会话事件需要 surface 标记 | 驱动用 `session.append(..., {surfaceOp:'append'})` |
| 驱动在 `--wait` 内拿不到决定 | 屏幕侧没人点 | 超时退出码 3，`decision="timeout"`，卡片会在管道断开后自动撤掉 |
| 多行正文/带引号的 operation 被命令行吃掉 | Windows 参数引号二次解析 | 驱动支持 `--json-in <file>`，场景参数走 JSON 文件 |
| `driver:null` + “驱动没有输出 JSON”，但 `*-driver.json` 里明明有内容 | 脚本在 Windows PowerShell 5.1 里跑，`Get-Content -Raw` 默认按 **ANSI** 解码 UTF-8 驱动输出，中文被读花 → `ConvertFrom-Json` 失败 | 改用 `[System.IO.File]::ReadAllText(..., UTF8)` 读 + 5 秒重试（同时兜住重定向文件落盘的毫秒级竞态） |
| 读到的驱动 JSON 是空的/解析失败 | `Start-Process -RedirectStandardOutput` 的文件在进程退出瞬间可能还没 flush | 同上：先等进程退出，再按 200ms 间隔重试解析，最多 5 秒 |
| 选项「甲选项」无论如何都点不到 | Windows OCR 把「项」稳定识别成「顶」（甲选顶），`-Find 甲选项` 永不命中 | 测试数据改用 OCR 友好词（冒烟测试/完整回归/先不验证、行高/进度/滚动、方案一/方案二、确认一/确认二）；真实用户数据不要为了 OCR 改 |
| 选项和题干同时命中，点到了题干 | 题干里也写着选项名 | `-PreferBottom`：取最靠下的命中（选项一定在题干下方） |
| 完成通知 OCR 只读到「本轮已」（丢掉「结束」） | 行检测在小字号/半透明背景上截断 | 判定改成多候选 `本轮已结束 / 本轮已 / 查看回复`，正文用去空格子串匹配 |

---

## 9. 快速复现命令

```powershell
# 只看 OCR 能不能读一张已保存的截图（离线、不碰桌面岛仔）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-screen-ocr.ps1 `
  -ImagePath docs\superpowers\research\agent-e2e-shots\manual-poc\done-card-140pct-1to1.png `
  -Region 1400,0,2200,400 -Find "daozai-e2e-done" -FindScales 1,2,3

# 只发一条审批/提问/完成请求，人工在岛上点（轻量探针，不验证宿主）
node scripts\agent-card-probe.mjs <DAOZAI_DATA_DIR> approval
node scripts\agent-card-probe.mjs <DAOZAI_DATA_DIR> question-multi
node scripts\agent-card-probe.mjs <DAOZAI_DATA_DIR> finished

# 真实宿主驱动（一般由 agent-e2e.ps1 调用；也可手动跑）
$env:ELECTRON_RUN_AS_NODE='1'
& "$env:LOCALAPPDATA\Programs\DeepSeek Harness\DeepSeek Harness.exe" `
  tests\fixtures\agent-drive.cjs --profile <临时 DSH profile> --scenario approval --wait 90000
```

---

## 10. 实测输出样例（手工跑通时抓的，字段结构以正式跑为准）

审批（屏幕上点「仅允许这一次」）：

```json
{
  "scenario": "approval",
  "sent": {"tool": "Bash", "operation": "{\"command\":\"echo daozai-e2e\"}", "reason": "岛仔端到端验收：请在岛上点击「仅允许这一次」"},
  "bridge": [{"provider": "dsh", "kind": "approval", "mode": 2,
              "sent": {"tool": "Bash", "operation": "{\"command\":\"echo daozai-e2e\"}", "sessionId": "e2e-approval"},
              "reply": {"decision": "allow"}}],
  "decision": "allowed-once", "handback": false, "ok": true
}
```

提问（屏幕上点「甲选项」→「提交回答」）：

```json
{
  "scenario": "question-single",
  "bridge": [{"provider": "dsh", "kind": "question",
              "reply": {"answers": [{"custom": "", "id": "e2e-single", "selected": ["甲选项"]}], "decision": "answer"}}],
  "decision": {"answers": [{"custom": "", "id": "e2e-single", "selected": ["甲选项"]}]},
  "handback": false, "ok": true
}
```

完成通知（岛上显示「本轮已结束」+ 正文）：

```json
{
  "scenario": "done",
  "sent": {"text": "E2E 完成正文：岛仔端到端验收。\n- 标记 daozai-e2e-done\n…", "length": 83},
  "bridge": [{"provider": "dsh", "kind": "finished", "mode": 2, "reply": {"done": true}}],
  "decision": "done", "handback": false, "ok": true
}
```
