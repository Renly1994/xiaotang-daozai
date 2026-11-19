# island-hud 卡片 UI/排版/动效规范（daozai 实施手册）

日期：2026-11-20
调研对象：`C:\Users\59531\Documents\DeepSeek\refs\island-hud`（只读）
对照目标：`C:\Users\59531\Documents\DeepSeek\xiaotang-daozai`（只读）
关联文档：`docs/superpowers/research/hud-renderer-semantics.md`（行为语义，本文件不重复）、`docs/superpowers/specs/2026-11-19-island-hud-parity-design.md`（总体方案）

本文件只回答一个问题：**「审批卡 / 提问卡 / 完成通知 / 结果正文」这四个界面，怎样才能做得和 island-hud 一样精致、顺滑**。所有数值都来自参考项目源码的真实 CSS/JS，不臆造；每条结论带 `文件:行号`。

---

## 0. 结论速览（先看这 8 条）

1. **精致感的第一来源不是配色，而是「呼吸」**：参考卡宽 520、内边距 20、区块 gap 12、卡高上限 560，内容高度由 DOM 实测（`island.css:1`、`island.css:15`、`island.js:310-317`）。daozai 的详情卡只有 **360×≤262**（`agent_cards.rs:161-185`、`app.rs:52`、`app.rs:1826-1832`），空间先天不足，这是「半成品感」的最大来源。
2. **参考的层次是「大标题 + 副标题 + 状态行 + 正文块 + 按钮行」五段式**，daozai 是「图标 + 单行标题 + 一条横线 + 内容 + 按钮」（`agent_cards.rs:196-202`），缺副标题与状态行，信息密度低而拥挤。
3. **正文字号阶梯严格按比例**：标题 15 / 正文 13 / 次级 12 / 标签 11 / 元信息 10（`island.css:1`、`island.css:18`、`island.css:23`），比例 1.15 : 1 : 0.92 : 0.85 : 0.77；行高统一 1.5–1.6。
4. **主次按钮靠明度反转，不靠色相**：主按钮 `#e5e5e5` 底 / `#080808` 字，危险与次要 `#262626` 底 / `#eee` 字（`island.css:24`）。daozai 用同一灰底 + 蓝色强调（`agent_cards.rs:308-311`），主次弱。
5. **动效按「场景」分档，不是一刀切**：300ms 常规、420ms 悬停、440ms 收起、560ms 回条形，曲线 `cubic-bezier(.2,.75,.25,1)`；内容进场另有 200ms 延迟 60ms 的位移淡入（`island.js:294-296`、`island.css:17`）。
6. **滚动是第二层精致感**：正文区 `overflow:auto` + 细滚动条 + 代码块 `pre` 可选中（`island.css:23`、`island.css:26`）。daozai 审批正文用「字符数估行数 + 截断」（`agent_cards.rs:168-174`），长命令直接被吞掉。
7. **状态必须可视**：`submitting / uncertain / expired / reviewing / queue 1/N / 剩余 m:ss / N 秒后回客户端`（`native-requests.js:22-67`、`97-106`）。daozai 只有「能否允许」一种禁用（`agent_cards.rs:307`）。
8. **完成结果卡在 daozai 是断的**：`agent_cards.result` 只被赋值（`extras.rs:544`）和判空（`app.rs:1879`），**全仓没有任何地方绘制它**（全文 grep 证实），点「查看」等于无反应。参考有完整的 `jump` 场景（`island.js:226-253`）。

---

## 1. 界面结构

### 1.1 渲染模型：单场景互斥 + 七个 layer

参考项目是「一个容器 `#notch` + 七个互斥 layer」，任一时刻只有一个 `.layer.active`（`index.html:11-21`、`island.js:302`）：

| 场景名 | 角色 | 尺寸来源 | 代码 |
|---|---|---|---|
| `compact` | 常驻条（空闲 / 请求被收起 / 完成 glance） | 宽 320×高 34 | `island.js:108-124`、`island.css:16` |
| `overview` | 会话总览（悬停 150ms 或手动） | 宽 520×内容高 | `island.js:147-172` |
| `all` | 全部会话列表 | 宽 520×内容高（列表内滚） | `island.js:173-194` |
| `jump` | **完成结果正文** | 宽 520×内容高（正文内滚） | `island.js:226-253` |
| `approval` | **审批卡** | 宽 520×内容高 | `native-requests.js:112-139` |
| `ask` | **提问卡**（含审阅子态） | 宽 520×内容高 | `native-requests.js:152-179`、`140-151` |
| `approved` | 操作回执胶囊（1.2s） | 宽 100%×高 54 | `island.js:326-333`、`island.css:16` |

尺寸统一由 `measure()` 决定：**宽度取设置，高度取 `min(maxHeight, layer.scrollHeight)`**（`island.js:310-317`），`maxHeight` 默认 560、夹在 320–640（`presentation.js:7-10`、`23`）。也就是说「卡片永远刚好装下内容，装不下才滚动」——daozai 需要复刻的是这个机制，而不是一组固定高度。

`#notch` 本体：320×34 起，`border-radius: 0 0 16px 16px`，背景 `#000` + `linear-gradient(180deg,#101012,#000)`，双层投影 `0 4px 12px #0005, 0 14px 28px #0002`（`island.css:3`）。展开后圆角 16px，条形态 15px（`island.css:16` 的 `.s-compact`、`island.js:319` 的 `r` 值）。

### 1.2 审批卡（approval）层次结构

自上而下 **5 段**，段间由 layer 的 `gap:12px` 分隔（`island.css:15`），**没有横向分隔线**：

| # | 区块 | 承载信息 | 结构与数值 | 出处 |
|---|---|---|---|---|
| 1 | `header.session-header` | 状态波形 / 「项目 · 标题」/ 副标题「你：上一句」或「DeepSeek Harness」/ provider chip / 收起按钮 | 高 min 48，`gap:12`；h1 15px/650/1.4 单行省略；p 12px `#b3b3b3` 单行省略，`margin-top:3` | `native-requests.js:88-89`、`presentation.js:169-182`、`island.css:18` |
| 2 | `div.native-head` | 橙点 + 「DSH · 权限请求」+ 右对齐「剩余 1:50」+「1 / N 待处理」 | 高 min 18，`gap:6`，12px/600，色 `--vi-alert` | `native-requests.js:90-95`、`island.css:23` |
| 3 | `div.native-scroll.approval-content`（可滚） | 三段式正文：`⚠ 工具名` → 标签「目标」+ 代码块 → 标签「操作内容」+ 代码块 → 标签「申请理由」+ 段落 → `<details>请求详情</details>` | 区块 gap 8；标签 11px `#95918e`；代码块 12px/1.6 等宽、圆角 9、内边距 10/12、底色 `#14110d`、描边 `#2d271f` | `native-requests.js:114-129`、`island.css:23` |
| 4 | `div.appr-btns.native-buttons` | 「返回客户端」「拒绝」「允许一次」（ZCode 才有「始终允许」） | flex 等分，`gap:8`，按钮高 34、圆角 8、12px/600 | `native-requests.js:130-138`、`island.css:24` |
| 5 | `p.request-error`（`aria-live`，空时 `display:none`） | 「结果未知，请在 DSH 确认。答案草稿已保留。」等 | 11px `#ffb090` | `native-requests.js:181-183`、`island.css:24` |

**没有 footer 状态栏**；过期/队列/倒计时全部塞进第 2 段右侧（`native-requests.js:93-94`），这是它「信息紧但不挤」的关键手法。

### 1.3 提问卡（ask）层次结构

默认（答题）态 **6 段**：

| # | 区块 | 内容 | 数值/出处 |
|---|---|---|---|
| 1 | header | 同上 | `native-requests.js:88-89` |
| 2 | native-head | 「▤」+「DSH · 需要回答后继续」/「可继续工作 · 待回答」+ 倒计时 + 队列 | `native-requests.js:90-95` |
| 3 | `div.native-scroll` > `fieldset` | legend「题干」(13px/600/1.6) → 可选 detail（12px `#b4aaa0`）→ 选项列表 → 答案标签（11px `#aaa`）→ 文本域（min-height 72，圆角 8，13px/1.6） | `native-requests.js:156-172`、`island.css:25` |
| 4 | `div.question-progress` | 左「第 1 / 3 题 · 单选 / 文本」，右「已选 n 项」（多选才显示，色 `#98cba0`） | `native-requests.js:173-176`、`island.css:25` |
| 5 | `div.appr-btns.native-buttons` | 首题「返回客户端」，`page>0` 后变「上一题」；右侧「下一题」/ 最后一题「检查答案」 | `native-requests.js:177` |
| 6 | `p.request-error` | 同审批 | `native-requests.js:181-183` |

审阅（reviewing）态替换 3–5 段：

| # | 区块 | 内容 | 数值/出处 |
|---|---|---|---|
| 3 | `native-scroll.native-review` | 标题「检查答案 · 共 N 题」（`.ask-q`，`margin-bottom:12`）→ 逐题 `div.answer-review`（`border-top:1px solid #282828`，`padding:10px 0`）：h3 13px/600「1. 题干」，p 12px `#bfbfbf`「选项A；选项B；自定义」或「未填写（将跳过此题）」 | `native-requests.js:140-151`、`island.css:25` |
| 5 | 按钮 | 「返回修改」「确认提交（主）」 | 同上 |

### 1.4 完成通知：两种揭示形态

| 形态 | 触发 | 结构 | 出处 |
|---|---|---|---|
| `glance`（轻提示） | `completionReveal==='glance'`（默认） | **不占独立场景**，直接把 compact 条标题改成「已完成 · + 纯文本摘要」，并用 `completed` 语义色描边呼吸（`#34c759`） | `island.js:110-114`、`island.css:8` |
| `expanded`（展开结果卡） | `completionReveal==='expanded'` | 切到 `jump` 场景，见 §1.5 | `presentation.js:209`、`island.js:255-262` |

停留计时：`completionDwellMs` 默认 5000、夹 2000–15000（`presentation.js:7-10`、`24`），仅在「未悬停且未受保护」时扣时（`presentation.js:203-207`）；完成事件只在 `now - completedAt <= 5000` 内算「新完成」（`presentation.js:194`）。

### 1.5 结果正文（jump）层次结构

| # | 区块 | 内容 | 数值/出处 |
|---|---|---|---|
| 1 | header | 会话波形 + 项目·标题 + 副标题 + chip + 归档/删除/设置/收起 | `island.js:244` |
| 2 | `div.native-head.completed-head` | 「✓ 本回合已完成」（绿色 12px/600） | `island.js:245`、`island.css:26` |
| 3 | `div.completion-body.native-scroll` | 受限 Markdown 渲染：h2/h3/h4、p、ul/ol、blockquote、`code`、`pre`、a | `island.js:237-246`、`presentation.js:113-141`、`island.css:26` |
| 4 | 可选 `p.hint` | 「内容已截断 · 在客户端查看完整回复」（摘要 ≥1200 字或全文 `truncated`） | `island.js:247` |
| 5 | `footer.appr-btns` | 「复制内容」「在 DSH 查看 ↗」（后者为主按钮反白） | `island.js:248-251` |

正文容器高度：摘要态 `max-height:120px`，全文档态 `calc(--max-height - 175px)`（560−175=385px）（`island.css:26`）。三种非正常态文案：「正在读取本回合完整回复…」「本回合完整回复暂不可用，请在客户端查看。」「客户端未提供结果正文。」（`island.js:240-242`）。

Markdown 渲染规则（`presentation.js:113-141`，供 egui 复刻）：
- `#`–`######` → `h2`–`h4`（`Math.min(4, n+1)`，即 `#`→h2）；
- ``` 围栏代码块 → `pre > code`，语言名白名单 `[a-zA-Z0-9_+-]` 截 24 字；
- `-`/`*`/`+`/`1.` → `ul`/`ol`；`>` → `blockquote`；空行断段，段内换行插 `<br>`；
- 行内：`` `code` ``、`**strong**`、`*em*`、`[text](url)`（只允许 http/https/mailto）、`![alt](url)` 降级为占位文字「alt（图片未加载）」；
- 单行 >4096 字符直接原样输出，避免正则回溯（`presentation.js:94`）。

### 1.6 出现 / 消失 / 子状态矩阵

| 状态 | 触发条件 | 界面变化 | 退出 | 出处 |
|---|---|---|---|---|
| `collapsed`（收起） | 点 header 的 `−` | 整个卡退到 `compact` 条，**请求仍在队列**；compact 显示「待处理 N」胶囊（`#33200f` 底 / `#f6ab63` 字） | 悬停 150ms 或点击条 → `native.expand()` | `native-requests.js:258-259,302`、`island.js:109-123`、`island.css:20` |
| `reviewing`（审核步） | 最后一题点「检查答案」/ 或中间题点「下一题」推进 | 正文换成答案汇总；按钮换成「返回修改/确认提交」 | 「返回修改」→ 回答题末题；`Ctrl+Enter` 直接提交 | `native-requests.js:140-151,275,314` |
| `queue`（队列） | `requests.length > 1` | native-head 右侧「1 / N 待处理」10px `#bfb4a8`；**无抢占**，新请求不顶替当前卡 | 当前请求结束 → 换下一条，并复位 `page/reviewing/collapsed/editing/submitting/uncertain` | `native-requests.js:94,101,220-235`、`island.css:23` |
| `expired`（过期） | `receivedAt + timeoutMs` 到点（1s 轮询，收起态不判） | 所有控件禁用、只提示一次 toast「请求已过期，请返回客户端处理」；native-head 右侧文案变「已过期」 | 宿主撤销或用户返回客户端 | `native-requests.js:22-45,99` |
| `submitting`（提交中） | 点允许/拒绝/提交 | 全控件禁用（除收起），`aria-busy=true`；按钮 `opacity:.42` | 收到 `delivered` 移除；`failed` 转 `uncertain` | `native-requests.js:97-106,236-257`、`island.css:2` |
| `uncertain`（结果未知） | reply 返回 `failed` 或抛异常 | 全控件禁用 + 错误行「结果未知，请在 DSH 确认。答案草稿已保留。」草稿保留 | 宿主 `confirmation` 到达 / 换请求 | `native-requests.js:251,256,308` |
| `handoff`（交回倒计时） | 设置 `approvalHandoffSeconds > 0`（0=一直等） | native-head 右侧显示「N 秒后回客户端」→「已交回客户端」；**编辑/收起/审阅/提交中/结果未知时暂停并重置整段** | 到点自动 `submit('return')` | `native-requests.js:46-67`、`presentation.js:27` |
| `approved`（回执） | 决定已发出且当前无待处理请求 | 独立 54px 高胶囊：✓/「−」+ 文案，绿/橙语义色 | 1200ms 后自动 `refresh()` | `island.js:326-333`、`island.css:16,26` |
| `attention`（呼吸描边） | 进入 waitingForApproval/waitingForAnswer；完成转 completed | 1px 内描边呼吸：橙色 `#f09748`→`#ffb45c`，绿色 `#34c759`；`animation: attention-pulse 1.8s ease-in-out infinite` | 阶段变化 | `island.css:4-9` |

**时序常量**：悬停展开 150ms、移出收起 900ms（`presentation.js:201-202`）、回执 1200ms（`island.js:332`）、toast 3200ms / 回执类 1200ms（`island.js:325,327`）、过期轮询 1000ms（`native-requests.js:285`）、相对时间刷新 1000ms（`island.js:355`）。

---

## 2. 视觉 token（真实数值 → 来源选择器）

> 约定：下面所有 px 均为 CSS px，来自源码；`--x` 表示根变量。

### 2.1 根变量与全局

| token | 值 | 来源 |
|---|---|---|
| `--panel-width` / `--expanded-width` | 520px | `island.css:1` |
| `--max-height` | 560px（夹 320–640） | `island.css:1`、`presentation.js:23` |
| `--panel-padding` | 20px（紧凑密度 16px） | `island.css:1`、`presentation.js:52` |
| `--body-size` / `--title-size` / `--secondary-size` | 13 / 15 / 12px（紧凑 12 / 14 / 11） | `island.css:1`、`presentation.js:51-52` |
| `--option-height` / `--button-height` | 44 / 34px（紧凑 40 / 32） | `island.css:1`、`presentation.js:52` |
| 正文字体 | `var(--sans)` = `'Segoe UI Variable Text','Segoe UI','Microsoft YaHei',system-ui`，13px/1.5 | `island.css:1-2` |
| 等宽字体 | `var(--mono)` = `'Cascadia Code',Consolas,monospace` | `island.css:1` |
| 全局 | `box-sizing:border-box`；`user-select:none`；`-webkit-font-smoothing:antialiased` | `island.css:2` |
| 禁用按钮 | `opacity:.42`；`cursor:default` | `island.css:2` |
| 焦点环 | `outline:2px solid #86acff; outline-offset:2px`（button/summary `:focus-visible`） | `island.css:2` |
| 文字色 | `--text:#f2f2f2`、`--muted:#b3b3b3`、`--subtle:#777` | `island.css:1` |
| 语义色 | work `#4e80ed`、idle `#34c759`、alert `#f09748`、question `#f09748`、ended `#777` | `island.css:1` |

### 2.2 卡片容器 / 最大宽高 / 滚动

| 元素 | 数值 | 来源 |
|---|---|---|
| `#notch` 条形态 | 320×34，`border-radius:0 0 16px 16px` | `island.css:3` |
| `#notch` 背景 | `#000` + `linear-gradient(180deg,#101012,#000)` | `island.css:3` |
| `#notch` 投影 | `0 4px 12px #0005, 0 14px 28px #0002` | `island.css:3` |
| `.layer` | `width:520px; max-height:560px; padding:18px 20px 16px; gap:12px; overflow:hidden` | `island.css:15` |
| compact layer | `width:100%; height:34px; padding:0 12px; gap:9px`，行向 | `island.css:16` |
| approved layer | `width:100%; height:54px; padding:0 14px; gap:9px`，12px | `island.css:16` |
| 条形态圆角覆盖 | `.s-compact{border-radius:0 0 15px 15px!important}` | `island.css:16` |
| 列表滚动 | `.overview-list,.all-list{overflow:auto; scrollbar-width:thin; scrollbar-color:#333 transparent}` | `island.css:21` |
| overview 列表上限 | `max-height:220px` | `island.css:21` |
| all 列表上限 | `calc(560 - 112)=448px` | `island.css:47` |
| 正文滚动 | `.native-scroll{overflow:auto; scrollbar-width:thin; scrollbar-color:#444 transparent; flex:0 1 auto}` | `island.css:23` |

### 2.3 头部（header）

| 元素 | 数值 | 来源 |
|---|---|---|
| `.session-header` | `display:flex; gap:12px; align-items:center; min-height:48px` | `island.css:18` |
| 标题 h1 | 15px / 650 / 行高 1.4 / 单行省略 | `island.css:18` |
| 副标题 p | 12px / `#b3b3b3` / `margin-top:3px` / 单行省略 | `island.css:18` |
| `.source-chip` | 10px / `line-height:1.6` / `padding:2px 6px` / 圆角 5 / 底 `#242424` / 字 `#bfbfbf` | `island.css:18` |
| `.provider-chip` | `min-height:22px; padding:2px 6px 2px 4px`；hover 过渡 `background/color 160ms` | `island.css:18` |
| `.provider-mark` | 16×16 / 圆角 5 / 底 `#444` / 700 10px 等宽；hover 与 header 联动 `rotate(-8deg) scale(1.08)` 180ms `cubic-bezier(.2,.8,.2,1)` + `0 0 0 2px #ffffff12` | `island.css:18` |
| `.icon-button, .collapse-request` | 25×28 / 17px / `#999`（hover `#222` 底 `#fff` 字）/ 圆角 5 | `island.css:18` |
| `.status-wave` | 27×23，`gap:2px`，5 根 3px 宽竖条，高度 `[7,13,19,11,6]px`，`--delay:-i*.17s`，`animation: wave 840ms ease-in-out infinite alternate`（`scaleY(.3)→scaleY(1)`） | `island.css:19`、`presentation.js:165-168` |

### 2.4 状态行（native-head）

| 元素 | 数值 | 来源 |
|---|---|---|
| `.native-head` | `gap:6px; min-height:18px; font-size:12px; font-weight:600; color:var(--vi-alert)` | `island.css:23` |
| `.appr-dot` | 6×6 圆点，`--vi-alert` | `island.css:23` |
| `.request-expiry` | `margin-left:auto; font:10px var(--mono); font-weight:400; color:#aaa` | `island.css:23` |
| `.appr-queue` | 10px / `#bfb4a8` | `island.css:23` |
| `.completed-head` | `color:var(--vi-idle)`（绿） | `island.css:26` |

### 2.5 正文区（审批 / 提问 / 结果共用）

| 元素 | 数值 | 来源 |
|---|---|---|
| `.approval-content` | `display:flex; flex-direction:column; gap:8px` | `island.css:23` |
| `.appr-tool` | `gap:7px`；`.appr-name/.appr-icon` = `600 13px var(--mono)`，`--vi-alert` | `island.css:23` |
| `.detail-label` | 11px / `#95918e` | `island.css:23` |
| `.native-detail`（代码块） | `background:#14110d; border:1px solid #2d271f; border-radius:9px; padding:10px 12px; font:12px/1.6 var(--mono); color:#e0d9cf; white-space:pre-wrap; overflow-wrap:anywhere`；`user-select:text` | `island.css:23` |
| `.approval-reason` | 12px / `#b4aaa0` / 行高 1.6 / `pre-wrap` | `island.css:23` |
| `.request-meta`（`<details>`） | 10px / `#999` / `margin:5px 0`；`summary` 可点 `#aaa`；`pre` `padding:8px 0; font:10px/1.6 var(--mono)` | `island.css:23` |
| `.completion-body` | 13px / 行高 1.6 / `overflow-wrap:anywhere`；`user-select:text` | `island.css:26` |
| `.completion-body h2/h3/h4` | 16 / 14 / 13px，`margin:0 0 8px` / `8px 0` / `6px 0` | `island.css:26` |
| 段落与列表 | `p,ul,ol,blockquote,pre{margin:0 0 9px}`；列表 `padding-left:22px`；`li+li{margin-top:4px}` | `island.css:26` |
| 行内 code | `12px var(--mono); border-radius:4px; background:#242424; padding:1px 4px` | `island.css:26` |
| 代码块 pre | `padding:10px; border:1px solid #303030; border-radius:7px; background:#161616; overflow:auto` | `island.css:26` |
| 引用块 | `border-left:3px solid #555; padding-left:10px; color:#bbb` | `island.css:26` |
| 链接 | `#8db3ff` + 下划线 | `island.css:26` |
| 空态 | `.empty-state{padding:28px 12px; color:#aaa; font-size:12px}`，h2 15px `#ddd` | `island.css:27` |
| 结果图标 | 19px，绿；rejected 用 `--vi-alert` | `island.css:26` |

### 2.6 按钮

| 元素 | 数值 | 来源 |
|---|---|---|
| 容器 `.appr-btns, .native-buttons` | `display:flex; gap:8px; flex:0 0 auto` | `island.css:24` |
| 按钮本体 | `flex:1; min-height:34px; padding:6px 10px; border-radius:8px; font-size:12px; font-weight:600; background:#262626; color:#eee; white-space:nowrap; transition:background 140ms` | `island.css:24` |
| hover | 背景 `#353535` | `island.css:24` |
| 主按钮 `.btn-allow` | 底 `#e5e5e5` / 字 `#080808`；hover 底 `#fff` | `island.css:24` |
| 危险/次要 `.btn-deny` | 与默认同（`#262626`），**不红、不加粗** | `island.css:24` |
| 禁用 | `opacity:.42`（全局 `button:disabled`） | `island.css:2` |
| 文字按钮 `.text-button` | 11px / `#bbb`；hover `#fff` | `island.css:21` |
| 分页按钮 | `border:1px solid #303030; background:#191919; color:#ccc; padding:5px 10px; border-radius:6px` | `island.css:22` |

### 2.7 提问专用

| 元素 | 数值 | 来源 |
|---|---|---|
| `.ask-q`（题干） | 13px / 600 / 行高 1.6 / `pre-wrap` / `margin-bottom:11px` | `island.css:25` |
| `.question-detail` | 12px / `#b4aaa0` / `margin-bottom:10px` | `island.css:25` |
| `.ask-opts` | `flex-column; gap:6px` | `island.css:25` |
| `.ask-opt`（选项行） | `min-height:44px; border-radius:8px; border:1px solid transparent; background:#24170b; padding:8px 11px; gap:10px; transition:background 140ms,border-color 140ms` | `island.css:25` |
| 选项 hover | `background:#3a2919` | `island.css:25` |
| `.ask-num`（序号块） | 23×23 / 圆角 5 / 底 `#563b21` / 字 `#ffc184` / 11px 等宽 | `island.css:25` |
| `.option-copy strong` | 13px / 600 | `island.css:25` |
| `.option-copy small`（描述） | 11px / `#b3aaa0` / 行高 1.5 / `margin-top:2px` | `island.css:25` |
| 选中态 | 行底 `#162a1a`、边框 `#376c43`；序号块底 `#376c43` 字 `#ecfff0`；右侧 ✓ `#6bd877` 显示 | `island.css:25` |
| 选项键盘焦点 | `outline:2px solid #9bc2ff; outline-offset:1px` | `island.css:25` |
| `.answer-label` | 11px / `#aaa` / `margin:12px 0 6px` | `island.css:25` |
| `.answer-text`（文本域） | `min-height:72px; width:100%; border-radius:8px; border:1px solid #343434; background:#191919; color:#f5f5f5; padding:10px 11px; font-size:13px; line-height:1.6` | `island.css:25` |
| 文本域 focus | `outline:2px solid #986d3e; outline-offset:-1px`；placeholder `#858585` | `island.css:25` |
| `.question-progress` | `space-between; gap:10px; color:#aaa; font-size:11px` | `island.css:25` |
| `.selection-count` | `#98cba0` | `island.css:25` |
| `.answer-review` | `padding:10px 0; border-top:1px solid #282828`；h3 13px/600；p 12px `#bfbfbf` / `margin-top:5px` | `island.css:25` |
| 只读预览卡 `.readonly-card` | 底 `#22170d`、边框 `#322417`、圆角 9、内边距 12；h2 14px/600 `margin-bottom:8px`；`.readonly-option` `gap:10px; padding:6px 0; color:#cbbba9`；`strong` 12px、`p` 11px `#a89c90`；`.hint` 11px/1.6 `#999` `margin-top:7px` | `island.css:25` |

### 2.8 语义色与强调色汇总

| 语义 | 色值 | 出现位置 |
|---|---|---|
| 审批 / 提问（警示橙） | `#f09748`；呼吸时更亮 `#ffb45c` | `island.css:1,5-7,19,23` |
| 警示底（按钮） | `#33200f` / 字 `#f6ab63` | 待处理计数胶囊 `island.css:20` |
| 审批正文暖底 | `#24170b`（选项）、`#14110d`（代码块）、`#22170d`（只读卡） | `island.css:23,25` |
| 成功 / 完成 | `#34c759`；浅绿字 `#9be8ad` | `island.css:1,8,19,21` |
| 运行中 | `#4e80ed`；浅蓝字 `#9fbeff` | `island.css:1,21` |
| 空闲 / 结束 | `#777`；标签字 `#b6b6b6` | `island.css:1,21` |
| 焦点蓝 | `#86acff`（全局）/ `#9bc2ff`（选项） | `island.css:2,25` |
| 链接 | `#8db3ff` | `island.css:26` |
| 错误文字 | `#ffb090` | `island.css:24` |

**状态标签公式**（`.state-label`，`island.css:21`）：胶囊 `min-height:20px; padding:2px 7px; border-radius:999px; font-size:10px; border:1px solid #ffffff18; background:#ffffff09`；再按 phase 覆盖 `color/border-color/background`（running `#9fbeff/#4e80ed55/#4e80ed18`、waiting `#ffc27f/#f0974855/#f0974818`、completed `#9be8ad/#34c75955/#34c75918`、idle `#b6b6b6/#ffffff08`）。这套「同色 33% 描边 + 9% 底」的配方直接可复用到 daozai 的 egui 里。

### 2.9 边框与分隔线透明度一览

| 位置 | 值 | 说明 |
|---|---|---|
| 选项未选中边框 | `1px solid transparent` | 用透明占位，避免选中时跳 2px（`island.css:25`） |
| 选中选项边框 | `1px solid #376c43` | 同上，宽度不变 |
| 答案文本域边框 | `1px solid #343434` | `island.css:25` |
| 代码块边框 | `1px solid #2d271f` / `#303030` / `#322417` | 暖色正文 vs 冷色结果正文，两种体系 |
| 审阅分题分隔线 | `1px solid #282828`（`border-top`） | `island.css:25` |
| 状态标签描边 | `#ffffff18`（≈9%） | `island.css:21` |
| 状态标签底 | `#ffffff09`（≈3.5%） | `island.css:21` |
| 头部 attention 内描边 | `1px` 语义色，透明度由 `opacity .55~1` 呼吸 | `island.css:4-9` |
| 卡片投影不透明度 | `#0005` + `#0002` 双层 | `island.css:3` |

> 注意参考项目**没有横向分隔线**把 header 和正文切开；daozai 现在在 `y=37` 画了一条全宽 0.6px、白 13% 的线（`agent_cards.rs:202`），这是「网页表格感」而不是「原生卡片感」的来源之一。

---

## 3. 排版与层级

### 3.1 字号阶梯与比例（以正文 13px 为 1.0）

| 层级 | island-hud | 相对正文 | 行高 | 字重 | daozai 建议（正文 11.5） |
|---|---|---|---|---|---|
| 结果正文大标题 h2 | 16px | 1.23 | 1.4 | 600 | 13.5 |
| 卡片主标题 | 15px | 1.15 | 1.4 | 650 | 13 (Semibold) |
| 结果正文 h3 | 14px | 1.08 | 1.4 | 600 | 12 |
| 正文 / 题干 / 按钮 | 13px | 1.00 | 1.6 | 400 / 600 | 11.5 |
| 次级信息（副标题、理由、描述） | 12px | 0.92 | 1.5–1.6 | 400 | 10.5 |
| 区块标签 / 进度 / 错误 | 11px | 0.85 | 1.5–1.6 | 400 | 10 |
| 元信息（过期、队列、请求详情、chip） | 10px | 0.77 | 1.3–1.6 | 400 | 9.5 |

参考的「字号差一档、颜色差一档、字重差一档」是层级感的全部来源：**不要靠加粗和放大来拉层级，靠同时降一级颜色**（`#f2f2f2` → `#b3b3b3` → `#95918e` → `#777`）。

### 3.2 行高与区块间距

| 项 | 值 | 来源 |
|---|---|---|
| 正文行高 | 1.5（全局 `font:13px/1.5`） | `island.css:2` |
| 长文本正文行高 | 1.6（`.ask-q`/`.native-detail`/`.approval-reason`/`.completion-body`） | `island.css:23,25,26` |
| 标签行高 | 1.6（10px 的 chip / 11px 的 hint） | `island.css:18,25` |
| 列表项间距 | `li+li margin-top:4px`；列表整体 `margin-bottom:9px` | `island.css:26` |
| 段落间距 | `margin-bottom:9px` | `island.css:26` |
| 区块间距（顶层） | `gap:12px` | `island.css:15` |
| 区块间距（审批正文内） | `gap:8px` | `island.css:23` |
| 选项之间 | `gap:6px` | `island.css:25` |
| 按钮之间 | `gap:8px` | `island.css:24` |
| 标题与副标题 | `margin-top:3px` | `island.css:18` |
| 选项 label 与描述 | `margin-top:2px` | `island.css:25` |

### 3.3 长文本处理策略（逐字段）

| 字段 | 策略 | 实现 |
|---|---|---|
| 标题 | 单行 + 省略号 | `white-space:nowrap; overflow:hidden; text-overflow:ellipsis`（`island.css:18`） |
| 副标题（上一句用户话） | 单行 + 省略号 | 同上 |
| 代码块 / 理由 / 题干 / 详情 | **完整换行，不截断，靠容器滚动** | `white-space:pre-wrap; overflow-wrap:anywhere` + `.native-scroll{overflow:auto}`（`island.css:23`） |
| 结果正文 | 完整渲染，`max-height` 内滚动 | `island.css:26` |
| 摘要式结果 | 硬截 1200 字符 + 「内容已截断」提示 | `presentation.js:143`、`island.js:247` |
| 错误行 | 单行段，`:empty` 时整行不占位 | `island.css:24` 的 `.request-error:empty{display:none}` |
| 空态文案 | 居中，`padding:28px 12px` | `island.css:27` |

**关键**：参考项目几乎不做「行数截断」，只做「容器限高 + 滚动」。行数截断只出现在列表行的标题/副标题上。daozai 现在把 `reason` 限 2 行、`operation` 限 4 行（`agent_cards.rs:168-174`、`290-296`），等于永久丢信息。

### 3.4 中英数混排的高度处理

- 参考全站给文字留的行高是 `1.5~1.6 × font-size`，**不按字符高度贴边排**；所以 12px 等宽代码行实际占 19.2px，13px 正文行占 19.5px。
- 中文单行按钮：文字 12px，按钮高 34，垂直居中用 flex，不依赖 `line-height` 对齐（`island.css:24`）。
- 数字用 `font-variant-numeric:tabular-nums` 保证「剩余 1:50」「1 / 3 待处理」「15s/42s」逐帧刷新时宽度不跳（`island.css:20,21`）。**这条对「丝滑」贡献极大，egui 里必须用等宽数字字体或在格式化时定宽补齐。**
- 工具名、路径、命令行统一走等宽字体，中文回退到微软雅黑后仍是等宽对齐（`--mono` 只声明拉丁，`island.css:1`）。

---

## 4. 动效（毫秒级参数）

### 4.1 场景尺寸过渡（最核心）

`applyScene()` 每次切换都改两个 CSS 变量，然后 `#notch` 的 `height/width` transition 读取它们（`island.js:294-297`、`island.css:3`）：

```js
const duration = layout.reduced ? 150
  : name === 'compact' ? (options.reason === 'leave' ? 440 : 560)
  : options.reason === 'hover' ? 420 : 300;
notch.style.setProperty('--motion-duration', duration + 'ms');
notch.style.setProperty('--motion-curve',
  options.reason === 'hover' && !layout.reduced ? 'cubic-bezier(.2,.8,.2,1.02)' : 'cubic-bezier(.2,.75,.25,1)');
```

| 场景切换 | 时长 | 曲线 |
|---|---|---|
| 常规展开/切场景 | 300ms | `cubic-bezier(.2,.75,.25,1)` |
| 悬停触发展开 | 420ms | `cubic-bezier(.2,.8,.2,1.02)`（带 2% 过冲） |
| 离开自动收起（→compact） | 440ms | 常规曲线 |
| 手动/ESC 收起（→compact） | 560ms | 常规曲线 |
| 减弱动效模式 | 150ms | 常规 |

同时 `#notch` 还有 `box-shadow 220ms ease, background 220ms ease`（`island.css:3`）。

### 4.2 进出场关键帧

| 动画 | 参数 | 关键帧 | 来源 |
|---|---|---|---|
| `notch-expand` | 随 `--motion-duration`，`--motion-curve` | `0%{scaleX(.975); brightness(.92)} 58%{scaleX(1.012); brightness(1.04)} 100%{scaleX(1)}` | `island.css:11,13` |
| `notch-collapse` | 同上 | `0%{scaleX(1.008)} 100%{scaleX(1)}` | `island.css:12,14` |
| `content-enter`（每个 layer 激活时） | `200ms ease-out 60ms both` | `from{opacity:0; translateY(-3px)} to{opacity:1}` | `island.css:16-17` |
| `attention-pulse`（等待态描边） | `1.8s ease-in-out infinite` | `0%,100%{opacity:.55} 50%{opacity:1}` | `island.css:6,9` |
| `wave`（状态竖条） | `840ms ease-in-out var(--delay) infinite alternate`，delay `-i*0.17s` | `scaleY(.3) → scaleY(1)` | `island.css:19` |

「content-enter 延迟 60ms」这个细节很关键：**容器先开始变形，内容晚 60ms 再淡入**，避免文字在尺寸变化中被拉扯。

### 4.3 hover / press / focus 反馈

| 元素 | 反馈 | 参数 | 来源 |
|---|---|---|---|
| 按钮 | 背景变亮 | `transition:background 140ms`；`#262626 → #353535`；主按钮 `#e5e5e5 → #fff` | `island.css:24` |
| 选项行 | 背景 + 边框 | `background 140ms, border-color 140ms`；`#24170b → #3a2919` | `island.css:25` |
| 会话行 | 位移 + 左侧亮条 + 投影 | `background 140ms, transform 180ms cubic-bezier(.2,.8,.2,1), box-shadow 180ms ease`；hover 时 `translateX(2px)` + `inset 2px 0 0 var(--vi-work)` | `island.css:22` |
| 行内箭头 | 淡入 | `opacity 140ms`，hover/focus 时 0→1 | `island.css:22` |
| 行内隐藏按钮 | 淡入 | `opacity 120ms`，仅 hover/focus-within 显示 | `island.css:33,36` |
| provider 图标 | 旋转 + 放大 + 光圈 | `transform 180ms cubic-bezier(.2,.8,.2,1), box-shadow 180ms ease` | `island.css:18` |
| 图标按钮 | 底色 | 无 transition（瞬时），`#222` 底 `#fff` 字 | `island.css:18` |
| hero 可点击块 | 底色/边框 | `background 140ms, border-color 140ms` | `island.css:21` |
| 全部会话「跳转」块 | 箭头淡入 | `opacity 140ms` | `island.css:22` |
| toast | 淡入 + 上移 | `opacity 150ms, transform 150ms`；`translateY(-3px) → 0` | `island.css:27` |
| 焦点环 | 无过渡（瞬时出现） | `2px solid` + `offset 2px` | `island.css:2` |

**参考项目的 hover 几乎全部是 140/180ms**，没有 300ms 以上的交互反馈——快速、克制；缓慢只用在场景尺寸上。daozai 的 `hover_t` 是 0.14s（`paint.rs:274`）和 0.07s（`paint.rs:275`），量级一致，方向是对的，问题在于**只有 capsule 用了它**。

### 4.4 减弱动效（必须实现）

| 规则 | 来源 |
|---|---|
| `#notch` transition 降到 150ms | `island.css:28` |
| `.layer.active`、`#notch::after`、expand/collapse 动画全部取消 | `island.css:28-29` |
| `.status-wave i` 停止 | `island.css:19,28` |
| `.session-row`、`.provider-mark` 禁用 transition 且 `transform:none!important` | `island.css:28,30` |
| 判定 | 设置 `motionMode ∈ {system,reduced,full}` 且系统 `prefers-reduced-motion`（`presentation.js:9,54`） |

---

## 5. 交互细节清单（「丝滑感」的具体来源）

1. **按钮顺序**：审批 `[返回客户端][拒绝][允许一次]`（`native-requests.js:130-138`）；提问 `[返回客户端/上一题][下一题/检查答案]`（`177`）；审阅 `[返回修改][确认提交]`（`150`）；结果 `[复制内容][在 DSH 查看]`（`248-251`）。**危险操作永远在中间、用最弱的样式；主操作永远在最右、反白。**
2. **危险操作弱化**：拒绝 = 默认深灰按钮（`island.css:24` 的 `.btn-deny` 与默认同值），不加红、不加粗、不占更大宽度。
3. **禁用态**：所有 `button,input,textarea` 统一 `disabled`（收起按钮例外，永远可点），并置 `aria-busy`（`native-requests.js:102-105`）；视觉 `opacity:.42`（`island.css:2`）。
4. **hover / active / focus 三态齐全**：hover 见 §4.3；active 由 CSS 默认位移无额外样式；focus-visible 有 2px 蓝环（`island.css:2`）。
5. **倒计时**：`剩余 m:ss`（等宽数字，右对齐，`island.css:23`）；交回倒计时优先显示 `N 秒后回客户端` / `已交回客户端`（`native-requests.js:25-31,51-54`）。
6. **队列计数**：`1 / N 待处理`，仅在 N>1 时出现（`native-requests.js:94,101`）。
7. **收起**：header 右侧 `−`，25×28，hover 有底色；收起后请求保留在队列，compact 条显示「待处理 N」胶囊（`island.js:120`）。
8. **详情展开**：`<details class="request-meta">` 里的「请求详情」，展开/收起都会触发重排 + `changed()`（`native-requests.js:184`）；`details[open]` 会进入 `protected()` 阻止自动收起（`native-requests.js:308`）。
9. **审核页**：最后一题不直接提交，先给「检查答案」，未填项明写「未填写（将跳过此题）」（`native-requests.js:146`）。
10. **键盘**：`Escape` = 收起（`island.js:424`）；审阅页 `Ctrl+Enter` = 提交（`native-requests.js:314`）；`all` 场景 `Home/End/PageUp/PageDown` 滚动列表（`island.js:425-433`）；输入法组合期间全部忽略（`native-requests.js:313`、`island.js:423`）。
11. **两阶段提交的确认感**：提交后不立即移除；宿主回执到达后弹 toast「已发送，等待客户端确认」→「客户端已确认允许一次」（`native-requests.js:254,290-295`）；决定完成且无待处理时显示 1.2s 回执胶囊（`island.js:326-333`）。
12. **跳转失败不静默**：`focusTerminal()` 四种结果都有 toast（已打开 / 已失效 / 失败原因 / 无法精确跳转）（`island.js:368-375`）。
13. **多个相似请求不叠加**：同 id 重发只保留一条；换请求时先存草稿再换层（`native-requests.js:220-235`）。
14. **草稿保留**：按 `[provider, session|request, questions]` 做 key，TTL 24h、最多 32 条、200ms 防抖写盘（`question-drafts.js:4,15-19,26`）。
15. **焦点与选中态**：正文 `user-select:text`（代码块、结果正文），其余 `user-select:none`（`island.css:2,23,26`）——用户只能选「该选的东西」。

---

## 6. 与 egui 的落地映射建议

### 6.0 换算假设

- egui 以逻辑点（point）为单位，`pixels_per_point` = 系统 DPI 缩放。**在 Windows 100% 缩放下 1 CSS px = 1 egui pt**；125%/150% 时整体等比放大，所以 CSS 数值可以直接当 pt 用，无需额外换算。
- **但两边的「卡片宽度」不相等**：参考 520px（内容宽 480），daozai 详情卡 360px（内容宽 360−2×18=324），面板 `PW=419`（`app.rs:23`）。机械照抄会让 13px 正文在 324px 内只放得下 24 个汉字/行，比参考少 1/3。
- 因此建议的分档规则：**间距、圆角、控件高度按 0.767（368/480）缩放；字号按 0.88 缩放并设 9.5pt 下限**（可读性不能等比缩）。
- 若愿意把详情卡放宽到 **400pt**（内容宽 368），则间距几乎可以按 0.9 缩放，观感最接近参考。

### 6.1 先说清硬约束：daozai 画布只有 270 高

| 事实 | 数值 | 位置 |
|---|---|---|
| 纵向画布 | `WIN_H = 270` | `app.rs:52` |
| 横向画布 | `CANVAS_W = WIN_W + 320 = 960` | `app.rs:51` |
| 窗口尺寸 | `with_inner_size([CANVAS_W, WIN_H])`，运行时不改尺寸 | `main.rs:1093`、`main.rs:606` |
| 详情卡高度上限 | `clamp(..., WIN_H - 8.0)` = **262** | `agent_cards.rs:173,183` |
| 详情卡宽度 | `detail_width = 360.min(panel.width())` | `app.rs:2111` |

参考项目对应的是 **宽 `宽+48`、高 `maxHeight+64` = 568×624** 的透明画布，且高度按内容实时 `setBounds`（`main.js:462-472`、`island.js:318-321`）。

**结论**：想在 270 高的画布里做出 560 高的卡片观感是不可能的。实施时必须二选一：

- **方案 A（推荐）**：Agent 详情/结果态把画布高度动态抬到 420–480。
  改动点：`native.rs:559` 的 `canvas_h` 从常量改为按状态取值（例如 `app::canvas_height()`），`main.rs:606` 同步，窗口用 `ViewportCommand::InnerSize` 或 `SetWindowPos` 改高；命中区域（`FrameOut.hit`）与 `place_on_display` 一起改。风险集中在「岛体贴屏幕顶部 + 命中区域上下沿」，需要一条 UI 测试守住。
- **方案 B（保守）**：保持 270，详情卡做成「固定头尾 + 中间滚动」的 419×262 卡片。观感接近「紧凑密度」，但必须补滚动与裁剪，否则永远比参考挤。

下面所有像素建议按 **方案 B 的 400×262 卡片 + 方案 A 的 400×420 可选高度** 给出。

### 6.2 视觉规律 → Card 原语映射表

| 参考规律 | egui/Card 落点 | 建议数值（pt） | 注意 |
|---|---|---|---|
| 卡片底 + 顶部微渐变 | `c.fill`（纯色）或 `c.vgrad(x,y,w,h,r, top, bottom)` | 底 `#101012` → `#000`；或沿用现有面板黑 | `vgrad` 已实现（`paint.rs:224-237`），注意它内部 tessellate 一次，别每帧对整卡调用多次 |
| 16px 底部圆角 | `fill(..., r=16)` | 详情卡 12–14（因为卡小） | 参考只有下方两角圆，`rect_filled` 是四角；可用 `Shape::rect_filled` 后叠一条覆盖条，或接受四角 |
| 头部三行（波形/标题/副标题） | `paint::wave`（需新增）+ `c.text` + `c.text` | header 高 40；标题 13 Semibold；副标题 10.5 Regular muted，基线 +14 | 波形：5 根 3×[7,13,19,11,6]（×0.8）竖条，`c.fill(..., 1.5, h)`，相位用 `sin(t*2π/0.84 - i*0.17)` |
| provider chip | `c.fill(r=5)` + `c.centered_text(10 Semibold)` | 高 18，字 9，内边距 5 | 参考 chip 底色 `#242424` |
| 状态行（点 + 文案 + 右侧计时/队列） | `c.circle(2.5)` + `c.centered_text` ×3，右侧 `H::Right` | 行高 16，字 10 Semibold alert 色；右侧 9 等宽 | 右侧两组要右对齐排布：先算计时宽，再算队列宽，从右往左排 |
| 三段式正文（标签 + 块） | `c.centered_text`（标签 10 muted）+ `c.fill(r=9)` + `c.stroke(1px, #2d271f)` + 等宽文字 | 代码块内边距 9/10，字 10 等宽，行高 15 | 用 `c.clipped()` 做滚动视口；块高 = 行数×15+18 |
| 长文本换行 | **改用 `painter.layout(text, font, color, wrap_width)` 一次排版**，取 `galley.size().y` 做高度 | — | 现有 `wrap()` 是逐字符 `text_w` 二分（`paint.rs:128-161`），1000 字要几千次 layout，长命令会掉帧 |
| 选中/未选中选项 | `fill` + `stroke(1px, transparent/选中色)` + 序号块 `fill(r=5)` + 右侧 ✓ `paint::check` | 行高 34，圆角 7，序号 20×20 圆角 5 | 参考用透明边框占位避免跳动，egui 同理：未选中也画 `stroke` 但 alpha=0 |
| 选项描述 | 第二行 `c.text` 10 Regular / `#b3aaa0` | 行高 13 | daozai 现在完全没画 `Choice.description`（`agent_cards.rs:235-250`），信息丢失 |
| 主按钮反白 | 新写 `paint::button(c,x,y,w,h,id,primary:bool,disabled:bool)`；不要用 `capsule` | 高 28–30，圆角 7，字 11 Semibold | `capsule` 是 h/2 全圆角（`paint.rs:394`），参考按钮是 8px 圆角，形状差异明显 |
| hover/press 过渡 | `c.hover_t(&r)` 已有 0.14/0.07s；把返回值接到**底色 mix** 和 **按下缩放 0.97** | hover 140ms 等效；press 缩 3% | 参考按下无缩放，只有背景变化；如要做缩放，幅度别超 4% |
| 焦点环 | `ui.memory` 记住焦点 id，绘制 `c.stroke(2px, #86acff)` 外扩 2pt | — | 自绘控件不会自动获得 egui 焦点环；`Tab` 切焦点需要自己维护顺序 |
| 出现动画（内容） | 场景计时 `age`：`alpha=smoothstep(0,0.2,age)`，`dy=-3*(1-ease_out_cubic(age/0.2))` | 200ms，延迟 60ms | 用 `c.shifted(0,dy).faded(alpha)`（`paint.rs:44-53`） |
| 场景尺寸动画 | 复用 `app.rs` 的 `Spring`（`anim.rs:8-51`）；不同场景用不同 stiffness/zeta | 300ms 等效 ≈ `stiffness 260~320, zeta .85`；悬停 420ms ≈ `stiffness 180, zeta .8`（带轻微过冲） | 参考是 CSS transition（固定时长），egui 是弹簧；想严格对齐时长就用 `ease_out_cubic(age/duration)` 驱动，别用弹簧 |
| 等待态呼吸描边 | `c.stroke(1px, alpha(alert_color, .55+.45*sin))` | 周期 1800ms | 参考是描边 + 外发光，egui 可再加 `c.glow`（`paint.rs:204-222`），透明度 0.10 以内 |
| 细滚动条 | 自己画 2pt 宽圆角条，右侧内缩 2pt，alpha .25，滚动时 .45 | — | egui 不会给自绘内容自动画滚动条 |
| 等宽数字 | `theme` 增加 `FontFamily::Monospace`（Cascadia Code → Consolas），计数/计时全用它 | 9.5–10 | 现在 `theme.rs` 只有 proportional/semibold/bold 三档（`theme.rs:196-200`），无等宽家族 |
| 减弱动效 | 读取系统 `prefers-reduced-motion`（egui 无直接 API，可查 Windows `SPI_GETCLIENTAREAANIMATION`），降到 150ms 并关闭呼吸/波形 | — | 至少提供一个设置项 |

### 6.3 egui 里最容易做成「半成品」的 10 个坑

1. **两份尺寸真值**：`detail_size()` 与 `draw_details()` 各算一遍布局，任何文案变化都要同时改两处，必然漂移（`agent_cards.rs:161-185` vs `187-321`）。**必须改成一次 measure → 返回 `Layout { blocks: Vec<Block> }` → 同一份结构再画**。
2. **字符数估高**：`(chars+47)/48`、`(chars+53)/54`、`chars/30`（`agent_cards.rs:168-184`）对中英混排误差可达 2 倍；参考是浏览器真实排版后读 `scrollHeight`（`island.js:314`）。egui 里等价物是 `galley.size().y` 与 `painter.layout(..., wrap_width)`。
3. **固定坐标**：`y=18/37/55/78/98`、`height-29.0`、`height-60.0`（`agent_cards.rs:198-311`）把布局写死；一旦加一行副标题或一段详情就全错位。改成自上而下的游标 `y += block_h + gap`。
4. **没有滚动裁剪**：审批分支整段不裁剪（`agent_cards.rs:285-303`），只有提问分支有 `clipped`（`212-218`）。结果就是长操作只能靠估算行数硬塞。
5. **裁剪后点击区域错位**：`hit()` 用 `rect.intersect(clip_rect)`（`paint.rs:261,268`），滚动内容滚出视口后仍可能命中不可见区域，或反过来可见区域命中不到。滚动列表必须「先算可见区间，再只对可见项 hit」。
6. **逐字符排版**：`wrap()` 对每个字符调用 `text_w` → `layout_no_wrap`（`paint.rs:128-161`），复杂度 O(n²) 次 galley 构建。仓库里已有测试守住「长链接不逐字重排」（`paint.rs:321-338`），但 `wrap()` 仍是这种写法。
7. **动画只用瞬变**：除了 `hover_t`，所有状态切换（选中、错误、禁用、展开）都是单帧跳变（`agent_cards.rs:242-248, 281-283`）。egui 提供 `ctx.animate_bool_with_time`（`paint.rs:274` 已在用）和 `anim::approach`（`anim.rs:91`），成本极低，收益极大。
8. **`faded()` 叠乘**：`multiply_opacity` 是乘算（`paint.rs:44-48`），父层 `<1` 时子层的「正常色」会整体变暗，参考的 CSS `opacity` 也是乘算，但参考从不在同一个视觉组上叠两层透明度。ghost 层 + faded 层同时用要核对着色。
9. **固定画布高度**：`WIN_H=270`（`app.rs:52`）写进窗口创建与命中计算（`main.rs:1093`、`native.rs:559`）。想加高卡片必须同步改 4 处，否则出现「画出来了但点不到 / 点得到但画不出」。
10. **文字不可选中/不可复制**：参考的代码块与结果正文是真实文本节点（`user-select:text`，`island.css:23,26`），用户能直接选词复制；egui 自绘 `galley` 无法选择。要么用 `egui::TextEdit::multiline` 只读（占位、可选择、可滚动），要么保留自绘但提供「复制全部」按钮——参考两条都给了（按钮 `复制内容`，`island.js:249`）。

---

## 7. 差距清单（对照 daozai 现有实现）

> 目标代码位置以当前工作区为准。参考位置给到 `文件:行号`。

| # | 「半成品」现象 | daozai 位置 | 参考做法 | 影响 |
|---|---|---|---|---|
| G1 | 详情卡只有「图标 + 一行标题 + 一条横线」，没有副标题、没有状态行 | `agent_cards.rs:196-202` | header 三元素：波形 +（h1 15/650 + 副标题 12 muted）+ chip + 收起；`island.css:18`、`presentation.js:169-182` | 卡片顶部空、信息少、不像「会话卡」像「对话框」 |
| G2 | 缺「审批/提问」语义状态行：没有橙点、没有「DSH · 权限请求」、没有右对齐的剩余时间与队列计数 | `agent_cards.rs:203-208`（只有「第 x / y 轮」） | `.native-head` + `.appr-dot` + `.request-expiry` + `.appr-queue`；`island.css:23`、`native-requests.js:90-95` | 用户不知道这是阻塞请求、还剩多久、后面还排了几个 |
| G3 | 标题与正文之间有一条全宽 0.6px 白 13% 横线 | `agent_cards.rs:202` | 参考不用横线，靠 12px gap + 区块底色分层；`island.css:15` | 表格感，割裂上下 |
| G4 | 审批正文缺「目标」字段（`tool.target` 根本没进 `Data`），三段变两段 | `agent_cards.rs:285-303`；`Data` 定义 `agent_cards.rs:75-77` 无 `target` | 「目标 / 操作内容 / 申请理由」三段，各带 11px label；`native-requests.js:120-127` | 用户看不到要改哪个文件，必须回客户端 |
| G5 | 审批正文不可滚动、不可选中，长命令按 4 行硬截断 | `agent_cards.rs:293-302`（`code_lines` 由 `code_available/14` 算出，`wrap` 限行） | `.native-scroll{overflow:auto}` + `pre` `white-space:pre-wrap` + `user-select:text`；`island.css:23` | 长命令/大段 JSON 被吞，无法核对 |
| G6 | 高度用字符数估算（`/48`、`/54`、`/30`） | `agent_cards.rs:168-184` | DOM 实测 `scrollHeight` 后 `min(maxHeight, …)`；`island.js:310-317` | 中英混排高度不准，时而留白时而挤压 |
| G7 | 按钮行：21pt 高、全圆角胶囊、宽度 94/72/144 不等、主按钮只是蓝底 | `agent_cards.rs:15-19`（`notice_action`）、`272-283`、`304-313`；`paint.rs:380-396` | 34px 高、8px 圆角、flex 等分（gap 8）、主按钮明度反转反白、危险弱化；`island.css:24` | 按钮像「药丸标签」，主次不分，三按钮不对齐 |
| G8 | 没有 submitting / uncertain / expired / 禁用态 | `agent_cards.rs:307`（只有 `allowed` 一种判据） | 全控件 disabled + `aria-busy` + `opacity:.42` + `.request-error` 文案 + 过期一次性 toast；`native-requests.js:97-106,251,256` | 点了没反应时用户会连续点，可能重复提交 |
| G9 | 提问选项完全不画 `description` 副行 | `agent_cards.rs:235-250`（只画 `o.label`） | `.option-copy strong` + `small`（11px `#b3aaa0`）；`island.css:25`、`native-requests.js:163-166` | 选项解释信息直接丢失 |
| G10 | 选项选中态只有整块变蓝，没有边框、序号块变色、右侧 ✓ | `agent_cards.rs:237-244` | 行底 `#162a1a` + 边框 `#376c43` + 序号块 `#376c43` + ✓ `#6bd877`；`island.css:25` | 选中反馈弱，多选时看不出选了哪些 |
| G11 | 提问无「检查答案/审阅页」，最后一题直接提交；无 `Ctrl+Enter`；无「未填写（将跳过此题）」提示 | `agent_cards.rs:276-283`；无 keydown 处理 | `reviewing` 页 + 「返回修改/确认提交」+ `Ctrl+Enter`；`native-requests.js:140-151,314` | 多题误答无法复核 |
| G12 | 完成通知把 markdown 压成一行 `fit`（宽 320 / 176） | `extras.rs:539-541` | 独立 `jump` 场景渲染 markdown：h2/h3/p/ul/code/pre + 120/385 限高滚动；`island.js:226-253`、`island.css:26` | 回复里的列表、代码、标题全部丢失 |
| G13 | 点「查看」把正文存进 `agent_cards.result`，但**全仓没有绘制该字段的代码** | 赋值 `extras.rs:544`；判空 `app.rs:1879`；`result` 定义 `agent_cards.rs:121` | 完成结果卡是独立场景，含头部、✓ 行、正文、截断提示、复制/跳转按钮；`island.js:243-252` | **按钮点了没反应**，是最刺眼的半成品 |
| G14 | 没有 markdown / 代码块 / 链接渲染能力 | 全仓无实现 | `presentation.js:113-141` 的受限 markdown（16 条规则见 §1.5） | 结果正文只能纯文本单行 |
| G15 | 卡片内部零动效：选中、错误、禁用、区块出现全部瞬变；无入场淡入 | `agent_cards.rs` 全文件只有 `capsule` 的 hover/press | `content-enter 200ms/60ms`、`notch-expand 300ms`、按钮 140ms、行 180ms；§4 | 点下去「啪」地变，没有过渡 |
| G16 | 没有焦点环、没有键盘操作（Esc 收起 / Ctrl+Enter 提交 / PageUp 翻页） | 全仓无 | `:focus-visible` 2px `#86acff`；`island.js:422-435`、`native-requests.js:312-315` | 键盘用户不可用，且看起来「像图片不像控件」 |
| G17 | 队列换人时状态复位不全：`question_scroll` 不复位（`Request` 新建时是 0，但切题时 `281-282` 在 index 前进时复位、`274` 后退时未复位） | `agent_cards.rs:274`（`r.question_index-=1` 未清 `question_scroll`） | `sync()` 换身份统一复位 `page/reviewing/collapsed/...`；`native-requests.js:227` | 后退到上一题时停在半截位置，像 bug |
| G18 | 详情卡宽度写死 360，与面板 419 之间留 59pt 空隙；`notice_size` 又写死 `(360,108)` | `app.rs:2111`、`app.rs:1777-1782` | 参考卡宽 = 面板宽 520（`island.css:1`） | 卡片「悬浮」在面板中间，两侧暗带 |
| G19 | 队列上限 32、超出直接交回，但界面没有任何「还有 N 条」的可见性（只有 `1 / N`） | `agent_cards.rs:344`、`extras.rs:521` | 队列计数 + 收起后的「待处理 N」胶囊；`island.js:120` | 用户不知道溢出会导致交回 |
| G20 | 字体层级：标题 12、正文 11/10、标签 9.5，层级差只有 1–1.5pt，且缺少等宽家族 | `agent_cards.rs:198,208,220,241,289`；`theme.rs:56-65` | 15/13/12/11/10 五档 + 独立 mono 家族；§3.1 | 层级「糊」在一起，数字宽度还会跳 |

---

## 8. 实施优先级（建议顺序）

| 阶段 | 内容 | 对应差距 | 理由 |
|---|---|---|---|
| **P0** | ①`detail_size`/`draw_details` 合并为一次布局测量（游标式 + galley 实测高度）；②补 header 第二行与 native-head 状态行；③按钮改为统一 28–30pt、8pt 圆角、等分、主按钮反白；④补 submitting/uncertain/expired/disabled 四态视觉 | G1 G2 G6 G7 G8 G3 | 这四项决定「是不是同一个产品」，且改动集中在 `agent_cards.rs` 一个文件，风险低 |
| **P0'** | 完成结果卡真正渲染（先纯文本 + 滚动，再上 markdown） | G13 G12 G14 | 当前「查看」是死按钮，属于功能缺陷级别 |
| **P1** | 正文滚动 + 裁剪 + 细滚动条；补「目标」字段；选项画描述与完整选中态 | G4 G5 G9 G10 | 信息完整性和长内容可用性 |
| **P1'** | 审阅页 + Ctrl+Enter + 焦点环 + Esc 收起 | G11 G16 | 「丝滑」的键盘路径 |
| **P2** | 动效分级：入场 200ms/60ms、hover 140ms、press、呼吸描边、场景尺寸时长分档 | G15 | 观感提升最大但依赖 P0 的结构稳定 |
| **P2'** | 画布高度动态化（方案 A），详情态 420–480 | G6 G18 | 收益最大、风险也最大（窗口/命中/定位联动），放在结构稳定后做 |
| **P3** | 等宽字体家族、数字 `tabular-nums`、密度档位（comfortable/compact） | G20 | 打磨项，最后做 |

**如果只能做三件事**：先让「详情卡高度/按钮形态/状态行」看起来像参考（P0），再把完成结果卡画出来（P0'），最后补动效分级（P2）。

---

## 附录 A：参考文件索引（按查阅频率）

| 文件 | 关注点 |
|---|---|
| `src/renderer/island.css:1` | 全部尺寸/颜色变量，先看这一行 |
| `src/renderer/island.css:15-27` | layer/header/status/正文/按钮/提问/结果，全部数值所在 |
| `src/renderer/presentation.js:45-55` | `geometry()`：宽高/字号/间距/控件高度如何由密度推导 |
| `src/renderer/presentation.js:165-182` | `wave()` / `header()` 的结构 |
| `src/renderer/native-requests.js:87-179` | 审批/审阅/提问三种正文的实际 DOM 结构 |
| `src/renderer/native-requests.js:97-106` | 禁用/队列/aria-busy 的唯一真值 |
| `src/renderer/island.js:263-317` | 场景切换、时长、measure/setBounds |
| `src/renderer/island.js:326-333` | 1.2s 回执胶囊 |
| `screenshots/approval.png` | 审批卡最终形态（520×约 450） |
| `screenshots/overview.png` / `sessions.png` | 头部、状态行、列表行 hover 的形态 |

## 附录 B：daozai 侧关键位置

| 位置 | 内容 |
|---|---|
| `src/agent_cards.rs:161-185` | `detail_size()` 高度估算（改造起点） |
| `src/agent_cards.rs:187-321` | `draw_details()` 全部硬编码坐标 |
| `src/extras.rs:509-547` | `draw_notice()` 的 AgentCard / AgentDone 两个分支（折叠卡与完成通知） |
| `src/extras.rs:1011-1015` | `notice_button()`（20pt 高胶囊，按钮风格源头） |
| `src/app.rs:1777-1787` | `notice_size()` |
| `src/app.rs:2109-2114`、`2178-2203` | 详情层绘制与外部点击关闭 |
| `src/app.rs:51-52` | `CANVAS_W=960` / `WIN_H=270` |
| `src/paint.rs:260-277` | `hit()` / `hover_t()` |
| `src/paint.rs:380-396` | `capsule()` |
| `src/theme.rs:12-32`、`56-65` | 颜色与字号表 |
| `src/anim.rs:8-93` | Spring / easing / approach |
