# 岛仔 Agent 弹窗 UI/布局打磨计划（2026-11-20）

> 目标：把「Agent 通知 / 审批 / 提问 / 完成结果」弹窗从当前半成品，打磨成与参考项目 island-hud 同档次的成熟桌面 UI。
> 参考依据：`docs/superpowers/research/2026-11-20-island-hud-card-ui-spec.md`、`docs/superpowers/research/2026-11-20-daozai-agent-ui-current.md`。
> 范围边界：**只动 Agent 通知/审批/提问/完成结果这条表现链路**；协议、来源接入、托盘、其他页面保持不动。发现阻塞本功能的 bug 才顺带修，并记录在案。

---

## 0. 成功标准（可验收）

1. **信息完整**：审批卡的「工具 / 工作目录 / 操作内容 / 申请理由」都能看全；长命令可滚动、不被字符数估算截断；提问的选项描述与自定义答案都在卡内完成；完成通知的「查看」能真正打开可滚动的本轮回复。
2. **状态可见**：等待（呼吸描边 + 倒计时）、队列（`1 / N`）、提交中、失败/结果未知、过期（若宿主提供时限）都有明确视觉反馈，点下去不能「没反应」。
3. **布局节奏**：字号阶梯 13 / 11.5 / 10.5 / 10 / 9.5，行高 1.5–1.6；区块 gap 8–12；按钮统一 30 高、8 圆角、等分、主操作反白；去掉贯穿 header 的分隔线；不再出现「按钮被切、文字压边、两侧空 30pt」。
4. **动效**：卡片入场淡入上移 200ms（延迟 60ms）、详情高度弹簧、hover 140ms、按下 3% 缩放、等待态 1.8s 呼吸、提交后 1.2s 回执胶囊；减弱动效档位可降级。
5. **回归**：`cargo test` 的 Agent 相关用例全绿（允许必要的断言更新，但要保留语义），非 Agent 用例零新增失败；Node bridge 测试全绿。
6. **实机端到端**：用真实 DSH 宿主发起审批与提问 → 岛仔弹出 → 屏幕识别 + 鼠标点击完成「拒绝 / 允许一次 / 作答」→ 原会话收到结果；完成通知「查看」能看到正文。

---

## 1. 关键设计决策

| # | 决策 | 理由 | 风险控制 |
|---|---|---|---|
| D1 | **画布高度按需抬升**：base `WIN_H=270` 不变，Agent 详情/结果态下窗口与画布抬到最高 `MAX_CANVAS_H=520`；`app::canvas_height()` 作为唯一真值，`main.rs`（viewport/win rect）、`native.rs`（canvas_h）、`app.rs`（clamp/region/hit）统一读取 | 参考卡 520×560，当前 419×262 先天不足；所有「挤」的问题都源于此 | 窗口常驻 520 高但 `SetWindowRgn` 只保留岛体+卡片轮廓，命中区仍是 `FrameOut.hit`；加一条 UI 测试守住「画布抬升后 hit/region 仍只覆盖卡片」 |
| D2 | **详情使用整个面板宽度**（`PW=419`，左右 padding 18 → 内容 383），不再 360 居中留 30pt 暗带 | 消除「悬浮在面板中间」的割裂感，且不改面板宽（避免影响其他页） | `detail_size`/`notice_size` 宽度改 419；测试里的坐标改用相对量 |
| D3 | **单一测量真值**：新增 `agent_ui::measure()` 自顶向下游标式布局，返回 `Metrics{height, blocks...}`；`draw` 用同一份测量结果绘制，删除 `detail_size` 里所有字符数除法 | 现有 `detail_size` 与 `draw_details` 两套几何必然漂移 | 用 `Card::wrapped()`（`Painter::layout` 实测 galley 高度）测量文本，不再估行 |
| D4 | **等宽字体家族**：`theme.rs` 增加 `FontFamily::Monospace`（Cascadia Code → Consolas → 系统等宽），倒计时/队列/命令行/数字全部使用，保证逐帧刷新不跳宽 | 参考 `font-variant-numeric: tabular-nums`；当前倒计时会左右抖 | 字体缺失时回退 proportional，不影响启动 |
| D5 | **按钮改型**：新 `agent_ui::button()`（30 高、8 圆角、140ms hover、按下缩放 0.97、禁用 opacity 0.42、主按钮浅底深字），替代 `paint::capsule` 在 Agent 卡里的使用 | 胶囊+蓝底没有主次层级，是「半成品感」最直观来源 | 只新增函数，不改 `capsule` 其他调用方 |
| D6 | **完成结果查看补全**：新增结果查看层（滚动正文 + 复制按钮 + 关闭），复用完成卡头部；正文按受限 markdown 渲染（标题/列表/代码块/段落），纯文本兜底 | 现在「查看」是死按钮（`agent_cards.result` 无绘制代码），属功能缺陷 | 只读渲染，不落盘；正文上限沿用 32000 字符 |
| D7 | **本地状态补全**：`resolved/submitting/error` 三态 + 入场动画时间戳；`reply()` 返回发送结果，失败时保留卡片并显示错误；提交成功后淡出 240ms 再移除 | 「点了没反应」是桌面工具最伤的体验 | 不改 wire 协议；宿主仍以 `cancel/回执` 为最终真值 |
| D8 | **动效参数**：入场 `alpha=smoothstep(0,0.20)` + `dy=-6→0`（200ms，延迟 60ms）；高度用现有 `Spring`（`OPEN_H`）；hover 140ms；按下 0.97；呼吸 1.8s；队列/倒计时等宽右对齐 | 对齐参考 §4 | 全部走 `ctx.animate_*`/`anim::approach`，静止帧不请求重绘 |

**明确不做**（本期边界）：新增 provider（Claude/ZCode 等）、协议字段大改、历史归档、HTTP 导航端口、把 Codex `request_user_input` 搬进岛仔、设置页改版、非 Agent 通知样式。

---

## 2. 目标界面结构

### 2.1 审批卡（宽 419，高 由内容决定，上限 500）

```
┌ header  20,14                                       ┐
│ [DSH icon 22]  DSH · 等待你审批            [切换到客户端] │
│                C:\work\project （cwd，10.5 muted）      │
├ status row  (y≈58, 高 18)                            │
│ ● DSH · 权限请求 · write        1 / 3 待处理  12 秒后回客户端 │
├ body（滚动视口，上下留白 8）                            │
│ 目标            ← 10 muted label                       │
│ ┌ 暖色代码块（radius 9, border #2d271f）────────────┐   │
│ │ file_path: src/auth/session.ts                  │   │
│ └────────────────────────────────────────────────┘   │
│ 操作内容                                              │
│ ┌ 代码块（等宽 10/1.6，可滚动、无行数截断）──────────┐   │
│ │ ...完整 operation...                             │   │
│ └────────────────────────────────────────────────┘   │
│ 申请理由                                              │
│ 应用新的超时与重试策略，发布前需要确认写入生产配置。      │
├ footer（固定，不上滚）                                 │
│ [ 回客户端 ] [ 拒绝 ] [ 仅允许这一次(主) ]             │
└──────────────────────────────────────────────────────┘
```

Native-scope 授权：没有「仅允许这一次」，正文替换为说明 + `[回客户端] [拒绝]`，主按钮仍反白但文案为「回客户端确认」。

### 2.2 提问卡（宽 419，高 上限 500）

```
header（同上，副标题为会话/目录）
status row：● 需要你回答 · 单选      第 1 / 3 题   已选 1 项
body（滚动）：
  题干 13 Semibold
  说明 10.5 muted（可省略）
  ┌ ① 方案 A            （34 高，选中：绿底/绿边/✓）
  │   描述可选 10 muted
  ├ ② 方案 B
  └ ...
  自定义回答（真 TextEdit，位于滚动区末尾，边框/焦点环）
footer：[ 回客户端 / 上一题 ]        [ 下一题 / 提交回答(主) ]
```

- 切题时该题滚动复位；未答满时「提交回答」禁用并有原因提示。
- 最后一题点「检查答案」先进入审阅页（列出每题所答，未答写「未填写（将跳过此题）」），`[返回修改] [确认提交]`；`Ctrl+Enter` 提交。

### 2.3 完成通知与结果查看

- 轻提示 334×44：保持现有形态，但用新按钮与等宽计时，入场淡入。
- 展开卡 419×?：✓ 图标 + 「本轮已结束」+ 回复预览 2 行 + `[查看回复] [关闭]`。
- 结果查看层（点击「查看回复」）：头部「本轮回复」+ 复制按钮 + 关闭；正文按受限 markdown：`#` 标题、`-` 列表、``` 代码块、行内 code、段落；最长 32000 字符，滚动阅读；正文不落盘。

---

## 3. 代码结构与分工

```
src/agent_ui/            ← 新增（唯一表现层入口）
├── mod.rs               入口：measure / draw_details / draw_banner / draw_done / draw_result / state
├── metrics.rs           全部尺寸、字号、颜色 token；自顶向下游标布局的 Metrics/Blocks
├── widgets.rs           wrapped_text、scroll_view（含细滚动条）、button、chip、icon_row、header、status_row
├── approval.rs          审批详情体 + 底部按钮
├── question.rs          提问详情体（多轮/选项/输入/审阅）
├── done.rs              完成通知（轻提示/展开卡）+ 结果查看
src/agent_cards.rs       ← 只留状态机；detail_size/draw_details 变薄壳委托 agent_ui
src/app.rs               ← canvas 高度、详情宽高、入场动画计时、独立详情浮层接线
src/extras.rs            ← draw_notice 的 Agent 分支委托 agent_ui；tick_alerts 自动展开策略
src/theme.rs             ← 增加 Monospace 家族与 Agent 语义色
src/main.rs / native.rs  ← 动态画布高度 520
docs/…                   ← 本计划、调研、验收记录
```

**写权限划分（避免冲突）**

| 归属 | 文件 |
|---|---|
| 主 agent | `src/agent_ui/{mod,metrics,widgets}.rs`、`src/agent_cards.rs`、`src/app.rs`、`src/theme.rs`、`src/main.rs`、`src/native.rs`、`src/lib.rs` |
| 子 agent A（提问卡） | `src/agent_ui/question.rs`（只此一个文件） |
| 子 agent B（完成通知 + 结果查看） | `src/agent_ui/done.rs`（只此一个文件） |
| 子 agent C（验收与 E2E 基建） | `scripts/agent-e2e.*`、`tests/fixtures/agent-*`、`docs/…/2026-11-20-agent-ui-acceptance.md` |
| 只读评审 | `docs/…/2026-11-20-agent-ui-review.md` |

子 agent 可以继续向下游 subagent 下发只读子任务（例如截图逐张评审），但不得改其他归属文件。

---

## 4. 阶段计划与验收

### 阶段 1：地基（主 agent）
- D1 画布抬升、D2 全宽、D3 单一测量、D4 等宽字体、D5 按钮、D7 状态字段。
- 交付：`agent_ui` 可编译、审批卡与提问卡走新布局（功能等价）、`cargo test --lib` 编译通过。
- 验收：导出基准截图（`DAOZAI_AGENT_PREVIEW`）→ 与参考对照；旧 Agent 测试语义保持。

### 阶段 2：三张卡并行（子 agent A/B + 主 agent 审批）
- A：提问卡（多轮、选项描述、选中态、审阅页、输入框在流内）。
- B：完成通知两形态 + 结果查看（受限 markdown）。
- 主：审批卡（目标/操作/理由、滚动、按钮层级、native-scope）。
- 验收：每张卡至少 4 个截图场景（短/长/多/异常），交互测试逐项。

### 阶段 3：动效与打磨
- 入场（200ms/60ms）、切换（弹簧）、hover/press、呼吸描边、回执胶囊、滚动条淡入淡出、数字等宽不跳。
- 验收：逐帧导出 GIF/帧序列检查无跳变；静止帧不重绘。

### 阶段 4：端到端
- 真实 DSH 宿主（Node 模式 + 真实 dsh-agent/approval/user-questions 插件）发起请求 → 岛仔实机显示 → Windows MCP 截图识别 + 点击 → 校验原会话结果。
- 验收记录写入 `docs/superpowers/research/2026-11-20-agent-ui-acceptance.md`（含截图路径、命令、结果）。

### 阶段 5：回归与交付
- `cargo test --all-targets`（Agent 全绿、非 Agent 零新增失败）、`node --test tests/agent-notices.test.mjs`、`git diff --check`。
- release 构建 + 部署到桌面「岛仔－当前测试版」入口，核对哈希与进程路径。

---

## 5. 风险与回滚

| 风险 | 缓解 |
|---|---|
| 画布抬升影响窗口命中/定位/全屏避让 | 只改 3 处常量来源 + 新增一条 hit/region UI 测试；保留 `WIN_H` 作为 base；出问题回退 `canvas_height()` 返回 `WIN_H` |
| 新布局引入坐标回归（旧测试断言固定 y） | 测试优先用「文字存在 + 可点击 + 不越界」断言；固定坐标断言改成相对几何 |
| 评审页/输入框焦点问题 | 输入框改为流内 `TextEdit`（不再 Foreground Area），补 focus 测试 |
| 动效导致静止帧持续重绘 | 用 `smoothstep(age)` + `animate_bool` 看门狗；完成后 `repaint_after` 回到 1s |
| 结果查看渲染 markdown 出错 | 受限解析器只识别 5 种行首标记，任何解析失败回退纯文本 |

---

## 6. 任务板

| ID | 任务 | 归属 | 依赖 | 状态 |
|---|---|---|---|---|
| T1 | 画布 520 + native/窗口/命中 | 主 | — | ✅ 完成（WIN_H=520，帧日志/窗口矩形已核验） |
| T2 | agent_ui 地基（metrics/widgets/measure） | 主 | — | ✅ 完成（新增 `src/agent_ui/`，测量与绘制同源，7 条布局单测） |
| T3 | 审批卡重做 | 主 | T2 | ✅ 完成（全宽、滚动、工具/操作/理由、按钮层级、禁用态、native-scope） |
| T4 | 提问卡重做 | 子A | T2 | ✅ 完成（选项描述/选中态/审阅页/布局稳定/流内输入框） |
| T5 | 完成通知 + 结果查看 | 主 | T2 | ✅ 完成（两形态 + 受限 markdown 结果层 + 复制 + 关闭收起） |
| T6 | 截图基准与逐场景评审 | 子C | T2 | ✅ 完成（`agent_cards_gallery` 导出 11 个场景） |
| T7 | 真实 DSH E2E 驱动 + 屏幕点击验收 | 子C | T3/T4/T5 | ✅ 完成（`-Scenario all` exit 0 / ok=true，三场景桥接校验通过） |
| T8 | 回归、release 构建与交付 | 主 | T3–T7 | ✅ 完成（全量回归 **1335 通过 / 0 失败**；release 构建 + 快捷方式已核对哈希） |

### 实施记录（2026-11-20）

- **画布**：`WIN_H 270 → 520`，窗口/region/命中沿用同一来源；阻塞式 Agent 卡片不再跟随鼠标离开自动收起。
- **单一测量**：`agent_ui::Measurer` 用真实 `LayoutJob` 测量，`measure_*` 与 `draw_*` 共用；删除 `detail_size` 里全部字符数除法。
- **等宽字体**：注册 `mono-regular/semibold/bold` 命名家族，**不改写内置 `FontFamily::Monospace`**（改写会让设置页分段控件文字垂直居中偏移 1.25px，已有回归测试盯住）。
- **踩坑记录**：
  1. 提问卡作答后提示行消失导致按钮上移 20pt → 固定提示槽位，布局前后一致（投影响应子 agent 修复）。
  2. 「查看回复」关闭后没有收起面板 → 留下一个空岛；用 `result_was_active` 检测并立即收起。
  3. 完成卡重复显示「本轮已结束」→ 无正文时摘要行留空。
  4. 画布变高后窗口/区域仍是旧的：`native` 的 canvas_h 与 `place_on_display` 都读 `WIN_H`，改一处即可。
  5. 本机 VS Community 缺桌面 CRT → 必须用 BuildTools 的 `vcvars64.bat`，并把 `CC/CXX/AR` 钉死（已写入 `scripts/dev-msvc-env.ps1`）。
- **开发探针**：`scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> approval|question|question-multi|finished|queue`，用真实 channel.cjs 发请求，配合 Windows 截图做真机验收。
