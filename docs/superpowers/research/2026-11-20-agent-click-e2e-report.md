# 岛仔 Agent 卡片真机点击 E2E 报告（2026-11-20）

> 执行者：e2e-operator（子 agent）。唯一允许操作鼠标/键盘与启停岛仔的 agent。
> 环境：Windows 11，主屏 1920x1080（岛仔在顶边居中），副屏 1920x1080。
> 方法：**真鼠标**（SetCursorPos + mouse_event，非 DOM 注入、非伪造桥接）+ 屏幕 OCR（scripts/agent-screen-ocr.ps1）+ 场景驱动（tests/fixtures/agent-drive.cjs，真实 dsh-session/agent/user-approval/user-questions 插件）+ DAOZAI_AGENT_TRACE=1 trace。
> 脚本：新增 `scripts/agent-e2e-repro.ps1`（done-repro / jump-repro / scroll-repro / banner-jump-repro）、`scripts/agent-click-island.ps1`（对运行中的岛仔 OCR+点击）、`scripts/dsh-type.ps1`（不经 windows-MCP 的键盘输入）。原 `scripts/agent-e2e.ps1` 未改动。

## 0. 两个构建与交付核对

| 项 | 旧构建（阶段1 复现） | 新构建（阶段2/3 回归） |
|---|---|---|
| exe | target/x86_64-pc-windows-msvc/release/daozai.exe | 同路径（重建） |
| SHA256 | E629D8CB89F3CC1313EBBB46A131D2F94AE8AD0414F76F490BA6D28D48D05194 | AC5941E21CBC8752D03CF6D3F2FAC047E860B096FF64001DAABD7D38A1983027 |
| size | 22,692,352 | 22,707,200 |
| mtime | 2026-11-20 11:01:41 | 2026-11-20 12:34:00 |
| 交付副本 | C:/Users/Administrator/Daozai/daozai.exe（旧哈希） | 同路径，lead 已覆盖并核对 SAME=True |

测试方式（阶段1/2）：把 exe 复制成 `daozai-e2e.exe` 跑在独立数据目录（避免单实例互斥体踢掉用户 live 实例，也不锁交付 exe），settings 用**用户真实配置**
`agent_cards={"interactive":true,"handoff":0,"completion":true,"dwell":15,"expanded":true,"content":0,"deferred":false,"system_toast":false}` + `island_size=140`（OCR 用）。

---

## 1. 阶段1：旧构建复现（A / B 均成功复现）

### 1a. 完成卡点「查看回复」→ 死按钮（复现成功）

- 驱动：agent-drive `done`（content=0 时桥接 payload mode=0、正文为空）
- 步骤/期望/实际：

| # | 步骤 | 期望 | 实际 | 证据 |
|---|---|---|---|---|
| 1 | 启动被测岛仔 + 驱动 | 完成卡出现 | 出现「本轮已结束 / DeepSeek Harness · 不代表任务成功 / 查看回复 / 关闭」 | `20261120-120813-done-repro/done-repro-01-card-card.png` |
| 2 | OCR 定位「查看回复」并真鼠标点击 | 出现结果层 | 点击成功（按钮进入 hover 高亮），(823,114) | 同上 + `03-after-0.9s` / `04-after-3s` |
| 3 | 点击后 0.9s / 3.0s 两次 OCR | 出现「本轮回复 / 复制内容 / 可滚动阅读」 | **全部没有**（resultMarkers null） | `03-after-0.9s-ocr.json`、`04-after-3s-ocr.json` |
| 4 | trace 观察 | 打开结果层 | 全程 `open=false nd=false`，面板连打开动作都没发生（命中 extras.rs 里 content>0 才 open_result 的分支） | `done-repro-trace-tail.log` |
| 5 | 对照组：同卡点「关闭」 | 卡片消失 | 立即消失（cardGoneAfterClose=true）→ 鼠标链路正常 | `06-after-close-card.png` |
| 6 | 桥接侧 | content=0 仍回 done | decision=done mode=0 textLen=0 done=true | `done-repro-driver.json` |

**结论：A 复现成功。** 不是点歪/被遮挡，是按钮静默 no-op（陷阱分支在 `src/extras.rs` ShowResult 处理里）。

### 1b. 提问卡点头部「切换到客户端」→ 跳转成功但零反馈（复现成功）

- 驱动：agent-drive `question-multi-turn`（选项用 OCR 友好词：冒烟测试 / 完整回归）
- OCR 关键坑（已固化进脚本）：头部 chip 常被 Windows OCR 读成「切客户端」，而卡片底部「回客户端」按钮也含「客户端」→ 必须「优先含『切』+ 取最靠上一条 + 校验落在卡片顶部 140px 内」，否则会点到 footer。

| # | 场景 | 点击前前台 | 点击后 1.5s / 3.3s 前台 | 岛仔可见反馈 | 证据 |
|---|---|---|---|---|---|
| B1 | DSH 不在前台（msedge/explorer） | msedge / explorer | **DeepSeek Harness (hwnd 4918262, pid 2692)** | **无任何 toast/提示**（最前台/已切到/已请求 均未命中） | `20261120-123000-jump-repro/` |
| B2 | DSH 已在前台 | DeepSeek Harness | DeepSeek Harness（画面完全无变化） | **无任何提示** | `20261120-123221-jump-repro/` |

- B1 在点完 chip 后卡片仍可作答：冒烟测试 → 下一题 → 确认一 → 检查答案 → 确认提交 全部点通，桥接 `answers=2 first=冒烟测试 handback=false`。
- 代码事实（HEAD 版本）：详情卡 `Action::Jump` 只在失败时记录 jump_error；**收起态提示卡的 `Action::Jump => {}` 是空实现**（`git show HEAD:src/extras.rs` L559 附近）。

**结论：B 复现成功**（跳转成功完全静默；DSH 已在前台时更是零变化）。

### 1c. 「收起态提示卡 chip 点完后卡片消失」疑问的澄清（不是 bug）

lead 质疑 12:41 那组（`20261120-124155-banner-jump-repro`）出现「点完卡片消失」的矛盾，实测查明是**点偏**：

- 该次点击坐标 (831,142)：banner 卡片 y≈0..181，头部 chip 在 y≈42，**底部按钮行在 y≈118..160**，y=142 落在 footer 按钮行（x 方向也落在左按钮「回客户端」上）。OCR 兜底命中的「客户端」来自 footer 的「回客户端」。
- trace 佐证：t=8.12（收起点击）之后到 t=19.51 一直是 `open=false nd=false notice=AgentCard count=1`；若真点到「回客户端」应立即 hand_back → count=0，实际没变，说明那一下等价于点空。
- 修正后（`20261120-124820-banner-jump-repro`）：改为 OCR 只在「头部带 + 卡片右半区」接受命中，否则用几何位置 `(卡片右边-78, 卡片顶+42)`，并在点击前校验坐标落在岛仔窗口 rect 内 → 点击 (1174,42)、insideIsland=True，0.5s/1.5s/3.3s 都出现「已切到 DeepSeek Harness」，banner 仍在（bannerStillVisible=true）。

---

## 2. 阶段2：新构建回归（A / B 均修复）

### 2a. 完成卡（content=0）→ 点「查看回复」出现说明层（通过）

- `20261120-123754-done-repro`
- 卡片上出现新的一行说明（截图可见）：`仅状态 · 未接收回复正文`（`done-repro-01-card-card.png`；OCR 对该 12px 小字读成「仅…」未能整句识别，属 OCR 限制，代码侧有单测覆盖）。
- 点「查看回复」→ 结果层出现，OCR 读到 "改成「回复摘要」或「本轮回复」"（即 `NO_BODY_RESULT_TEXT` 说明文案）；`resultHasCopyButton=false`（说明层没有「复制内容」）、`resultHasSettingsHint=true`（含「完成内容/设置」路径）。
- 桥接仍是 decision=done mode=0 textLen=0（符合 content=0 预期）。
- 对照组差异：结果层的「关闭」只关结果层、不关完成通知卡（点击后回到完成卡）——**属预期语义**，脚本里那条 "点关闭后卡片消失" 断言不适用于结果层，已在结论中修正。

### 2b. 提问卡 / 收起态提示卡跳转（通过）

| 场景 | 点击前前台 | 点击后前台 | 可见反馈 | 证据 |
|---|---|---|---|---|
| 详情卡 chip，DSH 已前台 | DeepSeek Harness | DeepSeek Harness | **「已切到 DeepSeek Harness」toast（0.5/1.5/3.3s 均在）** | `20261120-123919-jump-repro` |
| 详情卡 chip，DSH 在后台（msedge） | msedge | DeepSeek Harness (4918262) | **「已切到 DeepSeek Harness」toast** | `20261120-124013-jump-repro` |
| 收起态提示卡 chip（旧版空实现） | daozai-e2e | DeepSeek Harness | **「已切到 DeepSeek Harness」toast，banner 仍在** | `20261120-124820-banner-jump-repro` |

- 两种详情卡场景点完 chip 后卡片仍可作答，桥接 `answers=2 first=冒烟测试 handback=false`。
- 说明：实测里「点击岛仔」本身会让岛仔窗口拿到前台，所以跳转执行时客户端已不是前台，提示走「已切到」分支；若不经点击（纯代码）才会命中「已在最前台」。这是行为事实，不是缺陷。

### 2c. 长回复结果层滚动（通过）

- `20261120-124957-scroll-repro`（content=2，80 行长回复 + 末行标记）
- 结果层打开（kicker「本轮回复」）→ 光标移到正文中部向下滚 40 格 → **OCR 读到末行标记**「E2E-LONG-TAIL-MARK 这是最后一行」，`pageFlippedAfterScroll=false`（没翻岛页），桥接 mode=2 textLen=4016。

### 2d. 子 agent 完成噪声（部署侧通过，运行态待 DSH 重启）

- 交付核对：`C:/Users/Administrator/Daozai/daozai.exe` 的 Path/哈希与 target 新构建一致；`%APPDATA%/Daozai/agent-notices/bridge/index.mjs` 启动岛仔后 mtime 更新为 12:50:55，并出现新过滤行 `if (session?.header?.origin === 'subagent') return;`（L34）。
- 注意：DSH 桌面端进程当前加载的仍是 11/19 部署的旧插件，过滤要等 DSH 重启后生效——按 lead 要求**未重启 DSH**。

### 2e. 审批卡回归（**失败，未通过**）

- `scripts/agent-e2e.ps1 -Scenario approval`（真实 dsh-user-approval 插件链，期望岛上出现审批卡并点「仅允许这一次」拿到 allowed-once）：
  - 结果：**25 秒内没有在岛上看到审批卡**（exit 3），driver 无 stdout/stderr（被脚本超时杀掉）。
  - 证据：`20261120-132639/`（`approval-01-timeout.png` 里该区域只有桌面/DSH 与浏览器，没有岛仔卡片；`summary.json`）。
  - 说明：同一构建下 question 卡（jump-repro）与完成卡（done-repro）都正常，只有 approval 场景没出卡；未进一步定位（担心长时间占用桌面）。**建议 lead 复核**：可能是新构建审批卡回归，也可能是该场景 banner 关键词/OCR 差异，需要看 driver 侧是否真的发出了 approval exchange。

---

## 3. 阶段3：真实 DSH 桌面端端到端

### 3a. 交付核对（通过）

| 项 | 结果 |
|---|---|
| 进程 | pid 42612（首启）/ 41304（收尾重启），path = `C:\Users\Administrator\Daozai\daozai.exe` |
| 哈希 | AC5941E21CBC8752D03CF6D3F2FAC047E860B096FF64001DAABD7D38A1983027（= 新构建） |
| 部署插件 | index.mjs 12:50:55 更新，含 `origin === 'subagent'` 过滤行 |

### 3b. 真实 DSH 新会话 → ask_user_question 两题 → 岛上真鼠标作答（通过）

- 在 DSH 桌面端点「新会话」新建独立会话（未触碰 lead 正在用的会话），发指令让它调用 `ask_user_question` 问 2 个带选项问题。
- 岛上出现多轮提问卡 → 真鼠标依次点：第 1 题「冒烟测试」→ 下一题 → 第 2 题「要」→ 检查答案 → 确认提交。
- 会话语义验证：DSH 会话里出现「提问 · 2/2 已回答」及答案卡片（测试范围=冒烟测试；文档=要），随后助手回复「你选择的是先跑冒烟测试，并且要一起更新文档。」→ **会话确实收到了答案**。
- 证据：`docs/superpowers/research/agent-e2e-shots/phase3-real-dsh/`（01-question1-before、02-state 已答1、03-next、04-question2、05-q2-answered 已答2、06-review）、`phase3-sent.png`、`phase3-answers-received.png`。

### 3c. 该轮完成卡 → 点「查看回复」（**未观察到完成卡，open item**）

- 该轮结束后岛上**没有出现完成卡**；随后又触发 2 次短轮（「请只回复：收到」等，会话都在 ~3s 内完成）并在 30~45s 内每 250ms~1.3s 轮询 OCR，仍未出现「本轮已结束 / 查看回复」。
- 用户真实配置 `dwell=5`（settings.txt 实测），轮询间隔足以覆盖 5s 展示窗。
- 同时刻提问卡链路正常（同一 bridge、同一插件），因此**不是桥接整体不通**。怀疑：DSH 桌面端进程加载的还是 11/19 的旧插件（lead 说不要重启），完成卡链路与新 exe 期望不一致；需要重启 DSH 后复测。
- 证据：`phase3-real-dsh/12-frame-*.png`、`13-frame-*.png`、`14-frame-*.png` 均为「只有时钟胶囊、无卡片」。

### 3d. 真实审批（/permission read-only → 升权操作 → 岛上拒绝）（**未执行**）

- 未做：完成卡问题与 approval 场景失败占用了时间，lead 指示不要长时间占用桌面；且真实审批需要切换会话权限预设（改动用户环境）。改为用 `agent-drive approval` 场景做回归 → 该场景在 2e 中失败（未出卡）。
- 当前用户环境权限预设未被修改（无需还原）。

### 3e. 子 agent 噪声真机验证（未做）

- 需要 DSH 重启后插件才生效，本次未重启 DSH；仅完成部署文件核对（2d）。

---

## 4. 通过 / 失败汇总

| # | 验收项 | 构建 | 结论 | 证据目录 |
|---|---|---|---|---|
| 1 | A：完成卡「查看回复」死按钮（复现） | 旧 | ✅ 复现 | `agent-e2e-shots/20261120-120813-done-repro/` |
| 2 | B：详情卡 chip 静默跳转（复现，DSH 后台） | 旧 | ✅ 复现 | `…/20261120-123000-jump-repro/` |
| 3 | B：详情卡 chip 静默（复现，DSH 前台无任何变化） | 旧 | ✅ 复现 | `…/20261120-123221-jump-repro/` |
| 4 | A：点「查看回复」出现说明层（content=0） | 新 | ✅ 通过 | `…/20261120-123754-done-repro/` |
| 5 | 完成卡摘要行「仅状态 · 未接收回复正文」 | 新 | ✅ 通过（截图可见；OCR 对小字受限） | 同上 `done-repro-01-card-card.png` |
| 6 | B：详情卡 chip 有可见 toast（DSH 前台） | 新 | ✅ 通过 | `…/20261120-123919-jump-repro/` |
| 7 | B：详情卡 chip 有 toast 且真把 DSH 切前台 | 新 | ✅ 通过 | `…/20261120-124013-jump-repro/` |
| 8 | 收起态提示卡 chip 有结论（旧版空实现） | 新 | ✅ 通过 | `…/20261120-124820-banner-jump-repro/` |
| 9 | 长回复结果层滚到底且不翻页 | 新 | ✅ 通过 | `…/20261120-124957-scroll-repro/` |
| 10 | 点 chip 后卡片仍可作答（两轮答案回桥） | 新 | ✅ 通过 | 5/7 同目录 |
| 11 | 子 agent 过滤文件已随新 exe 部署 | 新 | ✅ 通过（运行态需 DSH 重启） | `%APPDATA%/Daozai/agent-notices/bridge/index.mjs` |
| 12 | 审批卡 allowed-once（agent-drive 场景） | 新 | ❌ 失败（25s 未出卡） | `…/20261120-132639/` |
| 13 | 真实 DSH 新会话两题作答并回传 | 新 | ✅ 通过 | `agent-e2e-shots/phase3-real-dsh/`、`phase3-answers-received.png` |
| 14 | 真实 DSH 完成卡 + 查看回复 | 新 | ⚠️ 未观察到（open） | `phase3-real-dsh/1[234]-frame-*.png` |
| 15 | 真实审批（read-only → 拒绝 → 还原） | 新 | ⛔ 未执行 | — |
| 16 | 子 agent 完成噪声真机验证 | 新 | ⛔ 未执行（需 DSH 重启） | — |

## 5. 进程与环境记录（每次启停都留痕）

| 时间 | 动作 | PID | 路径 |
|---|---|---|---|
| 12:01:50 | 停止用户 live（旧构建） | 11228 | C:/Users/Administrator/Daozai/daozai.exe |
| 12:02:49 | 启动被测副本（旧构建） | 9488 | %TEMP%/daozai-repro-…/daozai-e2e.exe |
| 12:33:53 | 恢复用户 live（旧构建） | 27312 | C:/Users/Administrator/Daozai/daozai.exe |
| 12:37:5x | 停止旧 live，开始阶段2 | — | — |
| 12:38:0x~13:26 | 阶段2 各场景均用 target 新构建的临时副本 | 多 | %TEMP%/daozai-repro-…/daozai-e2e.exe |
| 12:50:55 | 启动交付新构建 | 42612 | C:/Users/Administrator/Daozai/daozai.exe（hash AC5941E2…） |
| 13:26:39 | 为 approval 场景临时停 live；场景用副本 | 42556 | %TEMP%/daozai-e2e-…/daozai-e2e.exe |
| 13:28:15 | 恢复 live（新构建，交付路径） | 41304 | C:/Users/Administrator/Daozai/daozai.exe |

* 未覆盖交付 exe、未改 src/、未 cargo 构建、未 kill DSH 桌面端、未重启 DSH。

## 6. 遗留问题与建议

1. **审批卡未出现（最高优先级）**：新构建下 agent-drive approval 场景 25s 未出卡，而 question/done 卡正常。建议 lead 先本地跑同一场景（或看 driver 是否真的发出 exchange）；若确认新构建审批卡回归，需要修。
2. **真实 DSH 完成卡未观察到**：怀疑 DSH 加载的是 11/19 旧插件（lead 要求不重启 DSH）。建议重启 DSH 后复测「完成卡 + 查看回复」与「子 agent 不刷屏」两项。
3. **OCR 限制**：完成卡 12px 说明行、banner 上 9.5px chip 在 100% 缩放下 Windows OCR 识别率低。脚本已用「几何坐标 + 落点校验」补偿；纯 OCR 结论需结合截图判读。
4. **真实审批**：建议用 `/permission read-only` 触发一次真实审批卡（岛上拒绝），再还原 danger-full-access；或在 DSH 重启后重跑 agent-drive approval。

---

## 7. 13:26 approval 失败的初步判断与桌面时间线（桌面使用权说明）

### 7.1 approval 失败的三类可能（我的初判，未进一步定位）

本次失败表现为「25 秒内没有在岛上看到审批卡」，但同一构建、同一 bridge 下 question 卡与 done 卡都正常，所以我倾向于**不是 bridge 整体不通**。按可能性排序：

1. **卡片没弹**（新构建审批卡回归 / 该场景没发起 exchange）：驱动进程被脚本超时杀掉后 stdout/stderr 都是 0 字节，因此**无法从 driver 侧确认它到底有没有发出 approval exchange**——这是我判断的缺口。若要一次定性，最短路径是给 `agent-drive.cjs --scenario approval` 加 `--pretty` 并把 `--wait` 调大后单独跑一次，看 `bridge[]` 里有没有 `kind:"approval"` 条目。
2. **OCR 没找到**：脚本按 `等待你审批 / 等待审批` 找 banner，若新构建把审批卡的首帧渲染顺序/字号变了（或该场景直接进详情卡而非 banner），关键词就可能不命中；`approval-01-timeout.png` 是超时时刻的整块区域截图，里面只有桌面/DSH/浏览器，没有岛仔卡片，所以更像「没弹」而不是「弹了没读到」。
3. **桌面被抢占**：13:26 前后桌面上同时有 DSH 桌面端、Edge、以及 windows-MCP 的「AI is controlling this computer」悬浮层；该悬浮层曾多次盖住岛仔卡片顶部区域（阶段3 提问卡时实测遮挡 y≈106..180）。审批卡高度更矮，理论上更不容易被盖住，但不能完全排除截图/OCR 时被盖。

补充证据：同一时刻 windows-MCP 一度返回 `USER_CONTROL`（策略 relaxed、等待 9.9s），说明**用户/其他人当时接管过键鼠**，我的 GUI 操作在那段时间是被闸门暂停的——这也支持「13:26 前后桌面并不只由我一个 agent 独占」。

### 7.2 时间线（我这边留痕的动作，均可从 PID 记录复核）

| 时间 | 事件 | 我的动作 |
|---|---|---|
| 13:20~13:25 | 真实 DSH 会话两轮短轮询、抓完成卡帧（12/13/14-frame-*.png） | 我用 raw 鼠标/OCR 轮询，无完成卡 |
| 13:26:39 | 启动 `agent-e2e.ps1 -Scenario approval`（先停 live，用临时副本跑） | 我的脚本动作；13:27:19 判失败，13:27:20 停副本 |
| 13:28:15 | **启动 live 实例 PID 41304**（C:/Users/Administrator/Daozai/daozai.exe，新构建） | **是我**按 lead「尽快把用户 live 实例回到可用状态」的指示恢复的，不是另一个 agent |
| 13:28~13:33 | 仅读文件/写报告，**没有任何鼠标键盘操作** | — |
| 13:33:23 核对 | 观察到 live 实例 PID 变成 **40940**（同一路径） | 不是我启动的 → 说明期间有其他进程/人重启过 live 实例（可能是 lead 自己重跑 harness 前后的启停） |

结论：13:28 那个 live 实例是我恢复的，符合 lead 先前的指示；13:33 看到的 40940 不是我启动的。13:26 之后我不再操作桌面，桌面已释放。

### 7.3 给 lead 复跑 approval 的最小建议命令

```
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Scenario approval -KeepAlive
```
- 先确认没有任何 daozai 进程（单实例互斥体），脚本开头会自检并报 exit 4。
- 若想拿到「到底有没有发 exchange」的强证据，可临时用 `-WaitSeconds 30` 并把 driver 换成手动执行：
  `"DeepSeek Harness.exe" tests\fixtures\agent-drive.cjs --profile <tempProfile> --scenario approval --wait 30000 --pretty`，
  看输出的 `bridge[]` 是否含 `kind:"approval"`、`sent.tool` 是否为 `functions.bash`。

---

## 8. side take-over 复核（13:36–13:45，主导 agent）

> 接管目标：把 §2e（审批卡未出卡）与 §3c（真实 DSH 完成卡未出现）两个 open item 定性，并确保用户 live 实例与配置回到原状。

### 8.1 §2e「审批卡没弹」的定性：**卡片其实弹了，是取证侧漏看**

用同样命令复跑一次（`-Scenario approval -WaitSeconds 40 -DataDir %TEMP%\daozai-approval-check`），结果仍是脚本侧报「25 秒内没有在岛上看到审批卡片」；但**同时开启 `DAOZAI_AGENT_TRACE=1`**，trace 给出决定性证据（`%TEMP%\daozai-agent-trace.log`，该实例 13:36:51 启动）：

```
t=2.95 notice=Some("AgentCard(Dsh, "18ecc79d62ae75532edf3c5d05a0788ec3ce3bf992c7c7b9")") open=true nd=true count=1 \
       active=Some("18ecc79d…:details=true") hover=false
t=3.06 …（同一状态稳定持续到 t=10.03）
```

解读：
- `notice=AgentCard(Dsh, …)` → 审批请求**已经到达岛上并入队**（不是没发起、也不是交回客户端）；
- `open=true nd=true … details=true` → 面板已展开、详情已打开（审批到达即自动展开）；
- 因此 §7.1 的候选 1「卡片没弹 / 没发 exchange」在该次运行中**不成立**，问题在候选 2/3（截图区域/OCR/遮挡）。

同一次运行的超时截图（`20261120-133651/approval-01-timeout.png`，728×616）里只有浏览器与 DSH 面板、**看不到岛仔**，与 trace 矛盾 → 说明 `Get-IslandRect` 算出的截图区域当时并没有覆盖到岛仔窗口（脚本对窗口矩形的回退 + 桌面被其它窗口占据导致）。**结论：这是 E2E 取证脚本的取景问题，不是审批卡功能回归。**

### 8.2 顺带验证「第三方直接连桥接」不可行（省下后来人一次试错）

直接用 `channel.cjs` 客户端向 live 实例发 `exchange('dsh','approval',…)`，约 3 秒后返回 `REPLY null`（未答复）。原因是桥接的 `dsh` presence socket 由已挂载的 DSH 插件持有，外部客户端无法冒充；**审批/提问只能通过「真实 DSH 插件链 + 驱动」或真实会话发起**，不能绕过。脚本：`target/approval-request.cjs`（临时，未入库）。

### 8.3 §3c「真实 DSH 完成卡未出现」的判断

未再复跑（复跑需要结束本轮真实会话，时间成本高）。已有证据支持「插件版本」解释：DSH 桌面端当前加载的插件是 11/19 部署的旧版，而新 exe 启动时才把 `index.mjs` 刷新到 12:50 版本（§2d）；**DSH 不重启，加载的仍是旧插件**。完成卡链路在隔离实例（同一 exe）上已验证通过（§2a），所以更可能是「宿主侧旧插件 + dwell=5s 太短」的组合，而非新构建回归。

### 8.4 环境恢复（已完成）

| 项 | 状态 |
|---|---|
| 用户 live 配置 | 测试期被改成 `content=1`（回复摘要），**已还原为 `content=0`（仅状态）**，其余字段未动 |
| live 实例 | 重启并核对路径 `C:/Users/Administrator/Daozai/daozai.exe`（PID 29584），`live.json` policy：`content:0, dsh:true, interactive:true` |
| 备份 | `%APPDATA%/Daozai/settings.txt.before-restore`（还原前快照） |
| 桌面 | 已释放，无残留点击/脚本进程 |

### 8.5 给后续真机取证的三个硬要求

1. 取证时把 `agent_cards.dwell` 临时调到 **15s**（当前 5s，完成卡几乎来不及点）。
2. 截图必须**裁到卡片本体**（`agent-screen-ocr.ps1 -CropCard`），整屏在 1920 宽屏下 9.5–11px 文字不可读。
3. 取景区域不要依赖窗口矩形回退：直接固定为「主屏中心 ± 420px、y 0..560」，并在点击前用一次全屏截图确认岛仔在画面里。

