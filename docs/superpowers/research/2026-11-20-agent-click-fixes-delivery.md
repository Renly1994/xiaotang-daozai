# Agent 卡片「点了没反应」修复与真机验收（2026-11-20 第二轮）

> 用户报告：①「本轮结束点击查看回复没有反应」；②「在回答多轮对话过程中点击切换到客户端没响应」。
> 要求：排查 agent 问答、审批相关功能是否都符合预期；测试必须真实在 DSH 中发起、用 GUI 鼠标点击完成。
> 本轮用 4 个协作 agent 并行：完成卡修复（task-1）、跳转修复（task-2）、全链路交互审计（task-3）、真机 GUI E2E（task-4）。

---

## 1. 结论先行

| 编号 | 用户看到的现象 | 根因 | 修复 |
|---|---|---|---|
| A | 完成卡上点「查看回复」没有任何反应 | 用户配置是「展开结果卡 + 完成内容=仅状态」。桥接侧 content=0 时**根本不采集回复正文**（`bridge/index.mjs:17-19`），完成卡却无条件渲染 `[查看回复][关闭]`（`agent_ui/done.rs`），而 `extras.rs` 的 `ShowResult` 分支要求 `content>0` 才打开结果层，否则整段静默 → 死按钮 | 无正文时也打开**本机说明层**（标题「本轮未接收回复正文」，正文解释原因并给出「设置 → Agent → 完成内容」的下一步，副标题「本机说明 · 不含回复内容」，底部只有「关闭」）；完成卡摘要行先给轻提示「仅状态 · 未接收回复正文」。content=1/2 行为不变 |
| B | 多轮问答中点「切换到客户端」没反应 | 岛仔主窗口是 `WS_EX_NOACTIVATE | TOPMOST | TOOLWINDOW`（`native.rs`/`layered.rs`）→ 点岛上按钮不会让岛仔成为前台进程，**DSH 本来就在前台时 `SetForegroundWindow` 直接成功但没有任何可见变化**；失败原因走 `say()→toast`，而卡片展开态 toast 根本不绘制 → 成功/失败都静默。另外收起态提示卡的 `Action::Jump` 是空实现`extras.rs:559` | 跳转结果统一成 `JumpOutcome`（已在前台/已恢复最小化/已切到/已请求协议置前/找不到窗口/切换失败/不可用），每种都有中文结论；常驻提示行 + 卡片态 toast 也可绘制；`Target::Client` 不再用「打开资源管理器」冒充成功；`dsh://open` 作为客户端原生兜底（DSH 桌面端注册了该协议 → `open-url` → `focusPrimaryWindow`）并在 1.6s 后复核；收起态提示卡的跳转接线修好 |

**关键证据（同一台机器、真实鼠标、真实 DSH 宿主）**：旧构建上两处都复现（A：点击后 `open=false nd=false`，没有任何结果层；B：前台从 Edge 切到 DSH 但岛上零反馈；B2：DSH 已在前台时点击前后画面完全不变）。新构建上：A 点开出现说明层文案；B 两种前后台情形都出现「已切到 DeepSeek Harness」提示；收起态提示卡跳转同样有提示。详见 §5 与 `docs/superpowers/research/2026-11-20-agent-click-e2e-report.md`。

---

## 2. 修改清单

| 文件 | 改动 |
|---|---|
| `src/agent_ui/done.rs` | 无正文时摘要行/说明行；结果层支持 `hint` 模式（标题、副标题、只有「关闭」）；`draw_result` 返回并携带新的滚动偏移 |
| `src/agent_cards.rs` | `open_result_no_body`＋`ResultCard.hint`；`decide()` 拆成 `decide/resolve/mark_uncertain`（见 §3-D06）；`result_scroll` 写回 |
| `src/extras.rs` | `AgentDone → ShowResult` 两条路径都打开面板；收起态提示卡 `Action::Jump` 接线（原为空实现） |
| `src/agent_jump.rs` | `JumpOutcome` + `message()/ok()/err()`；候选窗口排序（标题/非最小化/面积，避免切给 Electron 辅助窗口）；`ShowWindow(SW_RESTORE)`＋`SetForegroundWindow`＋`GetForegroundWindow` 校验＋`AttachThreadInput` 兜底；`dsh://open` 协议兜底＋宽限复核；删除 Client 的目录回落；测试拦截开关 |
| `src/app.rs` | `jump_note` 常驻结论行（岛下方，放不下时进面板内、底部按钮行上方）；卡片态 toast 画到岛体下方；同文案去重；滚轮翻页让位给卡片滚动 |
| `src/agent_ui/{approval,question}.rs` | 结果未知时「回客户端」保持可点（其余按钮仍禁用，避免重复提交） |
| `src/agent_ui/metrics.rs` | 跳转 chip 按下提亮/回弹：点了至少立刻有视觉反馈 |
| `integrations/agent-notices/bridge/index.mjs` | turn/end 过滤 `session.header.origin === 'subagent'`（见 §3-D05） |
| `src/ui_tests.rs` | H::frame 与生产一致先跑 `Cards::update`；新增 8 条回归（完成卡说明层、content=2 原文、滚动写回、跳转接线、提示行绘制与不压按钮、结果层不翻页等） |
| `tests/agent-notices.test.mjs` | 新增「子 agent 轮次结束不推完成卡」 |

---

## 3. 顺带修掉的关联缺陷（审计 task-3 发现）

| ID | 问题 | 影响 | 处置 |
|---|---|---|---|
| D03 | 结果层滚动偏移只存在局部变量，`result_scroll` 从不写回 | 长回复每帧回弹，「查看回复」实际不可读长文 | `draw_result` 返回新偏移并写回 |
| D04 | 面板打开时滚轮被「滚轮翻页」逻辑吞掉并清零 | 审批长命令/结果层滚不动，反而把后台页面翻走 | 页导航条件加 `&& !agent_panel_active()` |
| D05 | 桥接 `turn/end` 不区分会话来源 | 每个 subagent 结束都推一张「本轮已结束」完成卡 | `index.mjs` 过滤 `origin==='subagent'`（清理 last/calls 之后再 return，审批/提问路径不受影响）＋node 回归 |
| D06 | `draw_details` 先取走请求，`decide()` 失败分支再去队首找它 → 永远找不到 | 提交失败时卡片静默消失、`State::Uncertain` 是死代码 | 取出的请求走 `resolve()`，失败 `mark_uncertain` 并放回队首、继续显示 |
| D07 | UI 测试的 H::frame 不跑 `Cards::update` | 依赖 update 的保护逻辑测不到，用例假绿 | 与 `main.rs:104` 顺序对齐 |
| D19 | 跳转提示行在超高卡片上会压住底部按钮 | 允许/拒绝被挡住 | 岛下方放不下时改画在面板内、按钮行上方（单测断言不相交） |
| — | 结果未知时整排按钮被禁用（含「回客户端」） | 提示「请回客户端确认」但按钮点不动 | 放行 `agent-return` |

---

## 4. 验证

### 4.1 自动化

| 项 | 结果 |
|---|---|
| `cargo test --lib ui_tests -- --test-threads=1` | **ok. 430 passed / 0 failed**（修复前同命令亦全绿，说明新增用例确实指出了旧行为） |
| `cargo test --lib agent -- --test-threads=1` | **55 passed / 0 failed**（跳过两条本机 TLS 环境失败用例） |
| 全量 `cargo test --lib` | 1347 passed / 4 failed / 132 ignored；4 条均为本机环境依赖（calendar TLS ×2 的 `native_tls Identity::from_pkcs8` → `Os code 5 拒绝访问`、qq `Shell_NotifyIconW`、quota DPAPI），与本次改动文件无关 |
| `node --test tests/agent-notices.test.mjs` | **12 / 12 passed** |
| 新增回归 | `done_without_body_still_opens_an_explanatory_result_layer`、`done_with_body_keeps_the_full_reply_in_the_result_layer`、`turning_completion_content_off_closes_an_open_reply_layer`、`result_layer_keeps_its_scroll_between_frames`、`result_layer_scroll_persists_and_never_flips_the_island_page`、`banner_jump_chip_always_reports_a_result`、`jump_note_is_actually_drawn_under_the_banner_and_inside_the_panel`、`allowing_without_a_bridge_keeps_the_card_and_marks_result_unknown` |

### 4.2 真机（旧构建复现 → 新构建回归，真实鼠标点击）

| 场景 | 旧构建 | 新构建 |
|---|---|---|
| 完成卡（content=0）点「查看回复」 | 点击成功但 `open=false nd=false`，无结果层、无提示（死按钮） | 结果层出现「改成「回复摘要」或「本轮回复」」说明文案；桥接 `decision=done mode=0` |
| 提问详情卡点「切换到客户端」（DSH 不在前台） | 前台 Edge → DSH，但岛上零反馈 | 出现「已切到 DeepSeek Harness」；前后台 hwnd 已记录 |
| 提问详情卡点「切换到客户端」（DSH 已在前台） | 前后台相同，画面完全无变化 | 出现「已在最前台 / 已切到」结论行 |
| 收起态提示卡点「切换到客户端」 | 空实现，死点击 | 点击后 0.5/1.5/3.3s 三次 OCR 均读到「已切到 DeepSeek Harness」，提示卡仍在 |
| 结果层滚动 | — | 滚动后未翻岛页（`pageFlip=false`）；末行可见性受 OCR 限制未能证明，滚动写回逻辑由单测覆盖 |
| 多轮问答 | — | 点 chip 后卡片仍可作答：冒烟测试 → 下一题 → 确认一 → 检查答案 → 确认提交，桥接 `answers=2 handback=false` |

证据目录：`docs/superpowers/research/agent-e2e-shots/`（`20261120-1208*-done-repro`、`20261120-123919-jump-repro`、`20261120-124013-jump-repro`、`20261120-124820-banner-jump-repro`、`20261120-124137-scroll-repro` 等）。

### 4.3 真实 DSH GUI 端到端

见 `docs/superpowers/research/2026-11-20-agent-click-e2e-report.md` 阶段3（新建会话 → ask_user_question 多轮 → 岛上作答 → 会话拿到答案 → 完成卡/查看回复 → 审批卡）。

---

## 5. 交付

- 构建：`cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc`（`scripts/dev-msvc-env.ps1`，LTO，本机约 4 分钟）
- 交付 exe：`target/x86_64-pc-windows-msvc/release/daozai.exe` 与桌面快捷方式实际指向的 `C:/Users/Administrator/Daozai/daozai.exe` **同一份文件**
  - size **22,707,200** bytes｜SHA256 `AC5941E21CBC8752D03CF6D3F2FAC047E860B096FF64001DAABD7D38A1983027`（2026-11-20 12:34）
- 交付前已停掉旧实例（旧哈希 `E629D8CB…`），覆盖后重新启动并核对进程路径与哈希。

### 需要用户注意

1. **子 agent 完成卡刷屏的修复在 DSH 插件侧**（`%APPDATA%/Daozai/agent-notices/bridge/index.mjs`）。岛仔启动时会把这个文件更新成新版，但 **DSH 桌面端进程里已加载的是旧插件，需要重启一次 DSH 桌面端才生效**（重启后子任务结束不再各推一张「本轮已结束」）。
2. 完成内容仍是「仅状态」时，岛上不会有回复正文；想直接在岛上读长回复，请到 设置 → Agent → 完成内容 选「回复摘要」或「本轮回复」。

---

## 6. 仍未处理（P2/P3，已审计留档）

`D08` 桥接退出/来源关闭时队列静默清空、`D09` 宿主撤销时卡片静默消失、`D10` hand_back 不校验发送结果、`D11` 队列满 32 静默交回、`D12` kit 不一致时静默不启动、`D13` 收起卡摘要恒为第 1 题、`D14` Esc 对卡片无效、`D15-D18` 文案/文档/死代码清理。完整清单与修法见 `docs/superpowers/research/2026-11-20-agent-ui-interaction-audit.md`。
