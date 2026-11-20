# Agent 问答 / 审批 / 完成 / 跳转 全链路交互审计（2026-11-20）

> 任务：共享任务板 **task-3**（只读代码审计，产出可执行缺陷清单与测试建议）。
> 审计基线：**HEAD 7d028df + 工作区未提交改动**（task-1「完成卡无正文说明层」、task-2「跳转反馈」正在同一棵树上进行）。
> 行号以 2026-11-20 11:5x 工作区快照为准，每条都附锚点原文；行号漂移时按锚点重新定位。
> 方法：只读源码 + 只读 Win32 探针 + 真实 DSH 宿主包（app.asar）取证 + 只读运行 cargo/node。未点击鼠标键盘、未启停 daozai、未覆盖交付 exe。
> 复核：2026-11-20 12:1x 二次快照（含 lead 的 D05 修复与 task-1/task-2 的在途改动），结论以本文件为准；接口基线为 §0 的三条主结论 + §3 的 D01-D19。

---

## 0. 结论先行

1. **用户报的 A（「查看回复」无反应）**：根因是 `src/extras.rs` 的 `ShowResult` 分支要求 `content>0` 才 `open_result`，用户配置 `content=0` → 完全静默。**工作区已修**（新增 `open_result_no_body` 本机说明层 + 卡片轻提示），本次实测两条用例通过（§7.4）。
2. **用户报的 B（「切换到客户端」无反应）**：两层叠加 —— (a) 岛仔窗口 `WS_EX_NOACTIVATE + TOPMOST + TOOLWINDOW`（本机探针实测），点它不会让进程成为前台进程，`SetForegroundWindow` 属后台抢前台，受前景锁限制失败是常态；(b) 失败原因走 `say()→toast`，而 toast 只在 `!agent_details && !banner` 的内容层里绘制，**卡片打开时根本不画**。成功也没反馈（已在前台=无可见变化）→ 用户看到的就是「没反应」。**task-2 工作区已修**（常驻提示行 + 取消资源管理器回落 + chip 按下反馈），但审计补出新风险 D19。
3. **未发现 P0**：无崩溃、越权、隐私泄露、永久卡死。共 7 个 P1（D01/D02/D03/D04/D05/D06/D07）+ 1 个 task-2 修法引入的新风险 D19。其中 **D01/D02/D05 已修（D05 由 lead 修复并回归），D03/D04/D06/D07 仍未修**：
   - **D03 结果层滚动位置不保存** → 长回复滚一下回弹，「查看回复」实际不可读长文。
   - **D04 审批卡/结果层正文滚轮被顶层页导航吞掉并清零** → 长命令滚不动，反而把后台页面翻走。
   - **D05 桥接 turn/end 未过滤子 agent** → 每个子任务结束推一张「本轮已结束」（已用真实 DSH 包实测证明）。**已由 lead 修复**：`integrations/agent-notices/bridge/index.mjs:30-38` 在 turn/end 清理后 `if (session?.header?.origin === 'subagent') return;`，并加回归用例；本审计复核 `node --test tests/agent-notices.test.mjs` **12/12 绿**（§5.4/§7.4）。
   - **D06 `decide()` 失败分支不可达** → 「提交失败·结果未知」是死代码，提交失败时卡片静默消失。
   - **D07 UI 测试不跑 `Cards::update`** → 依赖 update 的行为全测不到，task-1 新增用例当前 **FAILED**（§7.4 实测）。
   - **D01/D02** 已由 task-2 修复（待验收），**D19** 是其修法引入的遮挡风险。

---

## 1. 审计范围与取证

| 层 | 文件 | 审计点 |
|---|---|---|
| 表现层 | `src/agent_ui/{mod,approval,question,done,banner,widgets,metrics}.rs` | Action 产生点、禁用态、槽位、滚动 |
| 状态机 | `src/agent_cards.rs` | 队列、去重、deferred、32 上限、decide/hand_back、桥接子进程、结果层 |
| 接线 | `src/extras.rs`（draw_notice/tick_alerts）、`src/app.rs`（面板、toast、点外关闭、滚动输入） | Action 消费、可见反馈、不变量 |
| 跳转 | `src/agent_jump.rs` | 四档 Target、Win32 序列、错误可见性 |
| 协议 | `integrations/agent-notices/bridge/{index.mjs,channel.cjs,notify.cjs}` | content 0/1/2、cancel/超时/断开/重连、子会话 |
| 宿主模型 | `resources/app.asar/dsh/node_modules/@deepseek-ai/{dsh-session,cordis,dsh-user-approval,dsh-permission-presets}` | `session/event` 的 `{global:true}` 语义、`header.origin`、审批触发条件 |
| 测试 | `src/ui_tests.rs`、`tests/agent-notices.test.mjs`、`tests/fixtures/*`、`scripts/agent-e2e.ps1` | 覆盖缺口 |

### 1.1 本机探针证据（只读，无输入注入）

| 窗口 | pid | hwnd | 状态 | ex style |
|---|---|---|---|---|
| 岛仔 | 11228 | 0x3B0958 | 960x520 可见 | **0x08000088 = NOACTIVATE + TOPMOST + TOOLWINDOW** |
| DeepSeek Harness（唯一匹配） | 2692 | 0x4B0BF6 | 1296x828，showCmd=1 未最小化，可见 | 0x00000100 普通 |

结论：
- 目标窗口能唯一命中，本机此刻**不是**「最小化导致 SetForegroundWindow 无效」；但 task-2 要求的 SW_RESTORE/最小化兜底对其它机器仍必要。
- 岛仔是**非激活置顶工具窗**：点击不激活进程 → `SetForegroundWindow` 属典型后台抢前台，返回 false 是常态；即便返回 true，「客户端已在前台」也没有任何可见变化。
- 所以 bug B 的修法必须自己产生可见结论（成功/已在前台/失败三种都要说），不能依赖「跳转本身有视觉变化」。

---

## 2. Action 变体 × 消费点覆盖矩阵

`Action` 共 10 个变体（`src/agent_ui/mod.rs:49-70`）。

| Action | 产生处 | 消费处 | 状态 |
|---|---|---|---|
| Return | approval.rs:214、question.rs:533、banner.rs:97 | agent_cards.rs:362（详情）、extras.rs:554（收起卡） | OK |
| Deny | approval.rs:213 | agent_cards.rs:363 | OK（失败分支见 D06） |
| Allow | approval.rs:212 | agent_cards.rs:364-370 | OK（守卫失败静默，见 D15） |
| Answer | question.rs:550/558 | agent_cards.rs:371-375 | OK（失败分支见 D06） |
| Jump | approval.rs:153、question.rs:490、banner.rs:67 | agent_cards.rs:358-360（详情）、extras.rs:559（收起卡） | 详情 OK（反馈 D01/D19）；收起卡原被吞 → task-2 已分派 |
| Review | banner.rs:97 | extras.rs:555-558 | OK |
| Collapse | approval.rs:154、question.rs:491 | agent_cards.rs:361 | OK |
| Dismiss | done.rs:97/126、done.rs:311 | extras.rs:585、agent_cards.rs:448 | OK |
| ShowResult | done.rs:94/126 | extras.rs:576-584 | OK（原死点击，task-1 已修） |
| CopyResult | done.rs:311 | agent_cards.rs:443-447 | OK（有「已复制到剪贴板」） |

**所有 `_ => {}` / 空实现 / 未使用返回值（穷举）**

| 位置 | 内容 | 会吞真实动作？ | 处置 |
|---|---|---|---|
| src/extras.rs:559 | `Some(Action::Jump)=>{}` | **会**（收起卡 chip 死点击） | task-2 已分派；建议删 `_=>{}` 让编译器穷尽检查 |
| src/extras.rs:560 | `_=>{}`（banner 其余） | 当前不会（只产 Jump/Return/Review） | 同上，删兜底 |
| src/extras.rs:586 | `_=>{}`（完成卡其余） | 当前不会 | 同上 |
| src/agent_cards.rs:449 | `_=>{}`（结果层其余） | 当前不会 | 同上 |
| src/agent_cards.rs:377 | `_=>{}`（draw_details 其余） | 当前不会（详情只产 6 种） | 同上 |
| src/agent_ui/question.rs:551 | `_=>{}`（footer id 兜底） | 不会（id 是字符串） | 保留 |
| approval.rs:155、question.rs:492 | `HeadHit::None => {}` | 正常 | — |
| approval.rs:217、done.rs:128/316、banner.rs:99 | `let _ = t;` 未用动画时间 | 无 | P3 清理 |
| banner.rs:89 | `let _ = summary_w;` | 无 | P3 清理 |
| question.rs:363/431/456 | `QuestionResult.input_changed` 产出后无人消费（agent_cards.rs:333-338 只读 index/reviewing/scroll/action） | 功能无损（egui 输入自带重绘） | 删除或用于「手输后暂停交回」 |
| agent_cards.rs:308 | `draw_details(..)->bool` 三个调用点（extras.rs:530、app.rs:2150、app.rs:2229）**全部丢弃返回值** | 丢弃「详情已不再是当前」的信号，靠 tick_alerts 兜底 | 改 `()` 或断言记录 |
| widgets.rs | `hit_any`、`placeholder`、`footer_y`、`height_for`、`head_height_public`；metrics.rs 的 `attention`/`pos`/`edge` | 从未被调用（8 条 never used 警告，§7.4） | P3 删除（尤其 `hit_any` 会让人误以为有悬停暂停实现） |
| agent_cards.rs:284/299/328/348 | `ApprovalView.handoff`/`QuestionView.handoff` 恒为 `None` | 详情卡「N 秒后回客户端」永不显示（倒计时只在收起卡） | P3：删字段或详情态也显示 |

---

## 3. 缺陷清单（按严重度）

**总表（ID / 严重度 / 位置 / 触发 / 期望 vs 实际 / 修法）**

| ID | 严重度 | 文件:行号 | 触发 | 期望 → 实际 | 建议修法 |
|---|---|---|---|---|---|
| D01 | P1 | extras.rs:443、app.rs:1045-1048、app.rs:2280 | 卡片打开时点跳转 chip 且置前失败 | 岛上有原因 → **完全无提示** | 卡内/岛下常驻结论行（task-2 已修，待验收） |
| D02 | P1 | agent_jump.rs:70-76 | 同上且有 cwd | 只切窗口 → **弹资源管理器并返回 Ok** | 删回落（task-2 已修） |
| D03 | P1 | done.rs:278、done.rs:246、agent_cards.rs:441 | 查看长回复并滚轮 | 可滚动 → **每帧回弹** | draw_result 返回 scroll 并写回 result_scroll |
| D04 | P1 | app.rs:2029-2078 | 审批卡/结果层正文滚轮 | 卡内滚动 → **被页导航吞掉+清零** | 条件加 `&& !self.agent_panel_active()` |
| D05 | P1 | bridge/index.mjs:14-38 | 子 agent 轮次结束 | 无卡片 → **每子任务一张完成卡** | 过滤 header.origin==='subagent'（**lead 已修+回归**） |
| D06 | P1 | agent_cards.rs:310-311/224-233/:362-374 | 管道写失败时点允许/拒绝/提交 | 保留卡片+未知态 → **卡片静默消失** | decide 接收已取走的 Request，失败放回并 return true |
| D07 | P1 | ui_tests.rs:1192-1236 vs main.rs:104 | 任何依赖 update 的行为 | 测试可验证 → **测不到（用例已 FAILED）** | H::frame 前置调用 Cards::update |
| D19 | P1 | app.rs:2291-2299、3360-3378 | h>448 的卡片上点跳转 chip | 提示不挡操作 → **盖住底部按钮行** | 提示移入卡片头部下方或预留插槽 |
| D08 | P2 | agent_cards.rs:482-485 | 桥接退出/来源关闭 | 提示已交回 → 静默清空 | 复用 toast/jump_note 通道 |
| D09 | P2 | agent_cards.rs:455 | 宿主撤销/超时 | 提示已撤销 → 静默消失 | 仅 Pending 且未作答时轻提示 |
| D10 | P2 | agent_cards.rs:234-239 | 点回客户端但发送失败 | 有反馈 → 静默移除 | 复用反馈通道 |
| D11 | P2 | agent_cards.rs:461-470 | 队列满 32/非法数据 | 交回并在岛上可查 → 静默 | trace 日志 + 轻提示 |
| D12 | P2 | agent_cards.rs:64-65 | 导出 kit 与内置不一致 | 自愈且说明 → 静默不启动 | Service::start 返回原因 |
| D13 | P2 | agent_cards.rs:132-138 | 多题答到第 3 题收起 | 显示当前进度 → 恒显示第 1 题 | 用 question_index |
| D14 | P2 | app.rs:2026-2028 | 卡片打开按 Esc | 收起详情（不交回） → 无反应 | Esc → close_detail |
| D15 | P3 | agent_cards.rs:364-370 | 守卫条件将来漂移 | 有反馈 → 静默 | 失败写 error |
| D16 | P3 | 文档 vs 实现 | — | 文档承诺 → 实现不符 | 同步 AGENT-NOTICES.md:13/23 |
| D17 | P3 | app.rs:1931-1949、extras.rs:529 | 收起瞬间 | 不画错形状 → 单帧裁剪闪烁 | extras.rs:529 加 agent_detail_active() |
| D18 | — | notify.cjs:27 | Codex 提问 | 只提示不作答 | 无需修（已确认） |

### D01【P1｜task-2 已修，待验收】跳转失败提示在卡片打开时结构性不可见

- 位置（HEAD）：`extras.rs:443`（`take_jump_error → self.say(error)`）、`app.rs:1045-1048`（`say()` 置 `toast_below=false`）、`app.rs:2280`（`if !self.toast_below {self.toast_layer(&c,106.0);}` 位于 `if reveal>0.004 && !banner && !restoring_lyrics && !agent_details` 内）。
- 触发：审批/提问详情卡打开时点「切换到客户端」且 SetForegroundWindow 失败（本机常态，§1.1）。
- 期望：`docs/AGENT-NOTICES.md:13`「跳转失败会在岛上提示具体原因，不会点了没反应」。
- 实际：jump_error 写入 → say() 设 toast → **任何 Agent 卡片形态下都不绘制**，2.4s 静默过期。
- 修法（已采用）：`App.jump_note` + `jump_feedback()` 常驻提示行；浮层 toast 条件改为 `(toast_below || agent_details || banner)`。遗留风险 D19。

### D02【P1｜task-2 已修，待验收】`Target::Client` 失败回落打开资源管理器且返回成功

- 位置（HEAD）：`agent_jump.rs:70-76` —— `Err(window_error) if !cwd.is_empty() => shell_open(cwd)`。
- 期望：按钮「切换到客户端」只切窗口（同文件头注释自己承诺「文案必须说明真正会发生什么」）。
- 实际：弹出资源管理器，`open()` 返回 `Ok(())` → 连错误都不产生。
- 修法：删除回落（工作区已改为 `JumpOutcome` + 明确原因，`agent_jump.rs:6-14` 新注释一致）。**文档 `AGENT-NOTICES.md:13` 的「回落到打开工作目录」需同步删除。**

### D03【P1｜未修】结果层滚动位置不保存 → 长回复「滚一下就回弹」

- 位置：`src/agent_ui/done.rs:278` `let scroll = viewport.wheel(&c, v.scroll, w);`（局部变量，**不返回**）；`draw_result` 签名 `done.rs:246 -> Option<Action>`；状态侧 `agent_cards.rs:441` 传 `self.result_scroll`，而该字段只在 `:415`/`:422`/`:425` 被写成 `0.0`，**没有任何写回路径**。
- 对比：审批卡 `approval.rs:167 + (action, scroll)` → `agent_cards.rs:354 r.scroll=scroll`；提问卡 `question.rs:498 + QuestionResult.scroll` → `agent_cards.rs:337`。只有结果层漏了。
- 触发：content=本轮回复 → 一轮结束 → 查看回复 → 滚轮读长文。
- 期望：可滚动阅读（交付说明 §1、`AGENT-NOTICES.md:23` 均承诺）。
- 实际：每帧从 0 开始，滚一帧弹回，长文只看得见第一屏且抖动。
- 修法（最小）：`draw_result` 返回 `(Option<Action>, f32)`（与 approval 一致），`agent_cards.rs:442` 写回 `self.result_scroll = scroll`。改动只在 done.rs + agent_cards.rs 两处（**与 task-1 的 done.rs 写入范围冲突，需 lead 排期**）。

### D04【P1｜未修】审批卡/结果层正文滚轮被顶层页导航吞掉（并清零 raw_scroll_delta）

- 位置：`src/app.rs:2029-2078` —— `if (!self.is_editing() || self.todo_choosing) && page_hover && self.open && self.reveal>0.95 { ... }` 读 `i.raw_scroll_delta.y` 做翻页/列表滚动，并在 `app.rs:2074` **无条件清零** `raw_scroll_delta` 与 `smooth_scroll_delta`。该块**没有排除 `agent_panel_active()`**。
- 关键事实：`Cards::editing()` 只对「展开的提问卡」返回 true（`agent_cards.rs:257-259`）→ 提问卡幸运绕开；**审批卡与结果层没有**。`inside`（`app.rs:2008`）在展开态覆盖整张卡片（命中矩形 `app.rs:2461`），所以滚轮落在卡片正文上时 `page_hover=true`。
- 触发：operation 为 60 行 JSON 的审批卡在正文区滚轮 → 命令不动、后台页被翻走；结果层同样（即使修了 D03 也会被清零）。
- 修法（最小）：`app.rs:2029` 条件加 `&& !self.agent_panel_active()`（顺带禁掉卡片打开时左右方向键翻页），把滚轮留给 `Viewport::wheel`。

### D05【P1｜协议级｜**已由 lead 修复 + 回归**】桥接把子 agent 的轮次结束当成「用户这一轮结束」

- 位置：`integrations/agent-notices/bridge/index.mjs:14-35` —— `ctx.on('session/event', …, { global: true })`，`turn/end` 分支（:30-34）无条件 `channel.exchange('dsh','finished', …)`。
- 事实链（已取证）：
  1. cordis 的 `global` 监听器**绕过所有过滤器**：`cordis/src/events.ts:169-170` `.filter(hook => hook.global || !filter || filter.call(thisArg, hook.ctx))`；
  2. 子会话存在且 `session.header.origin === 'subagent'`：`dsh-session/lib/index.js:1097` 校验该字段；`dsh-headless/lib/index.js:228`、`dsh-api-session-controller/lib/index.js:129`、`dsh-agent-team/lib/index.js:763` 都用它判定子会话；
  3. **实测**（Electron 以 Node 模式挂真实 `dsh-session`，用与 bundle 相同的 `{global:true}` 注册方式）：

     ```
     events received by global listener:
        {"id":"normal-1","origin":null,"type":"turn/start"}
        {"id":"sub-1","origin":"subagent","type":"turn/start"}
        {"id":"normal-1","origin":null,"type":"assistant/message"}
        {"id":"sub-1","origin":"subagent","type":"assistant/message"}
        {"id":"normal-1","origin":null,"type":"turn/end"}
        {"id":"sub-1","origin":"subagent","type":"turn/end"}
     RESULT subagent turn/end reaches global listener = true
     RESULT subagent assistant/message reaches global listener = true
     ```
- 后果：每个子 agent（Agent Teams / subagent 工具）每轮结束都发一次 finished。content=0 时正文都是「本轮已结束」，被 `extras.rs:309-317` 的 same_event 去重挤成一张（仍会重弹/重置停留）；content=1/2 时正文各不相同 → 最多堆 8 张（`extras.rs:319-321`），真正的本轮完成卡被淹没。
- 修法与状态（2026-11-20 复核）：`index.mjs:30-38` 已加 `if (session?.header?.origin === 'subagent') return;`（位置在 `last/calls` 清理之后、`exchange('dsh','finished')` 之前），审批/提问不受影响（它们走 `approval/request`、`user-questions/request`，子会话的 `tool/call` 仍记录以保留操作内容）；回归用例 `DSH subagent turns never push a completion card to the island` 已加入 `tests/agent-notices.test.mjs:86`。
- 复核证据（本审计自己跑的）：`node --test tests/agent-notices.test.mjs` → **tests 12 / pass 12 / fail 0**，含该用例（见 §7.4）。
- 残余（P3，无功能影响）：`index.mjs:17-19` 的 `assistant/message` 采集仍无 subagent 守卫，子会话正文会在内存里短暂进 `last`，下一行 `turn/end` 即删除；如需彻底，可把该条件也加上 `!subagent &&`。

### D06【P1（概率低，后果是阻塞请求静默丢失）｜未修】`decide()` 失败分支不可达

- 位置：`agent_cards.rs:310-311` `let mut r = self.requests.remove(index)`（**先移出队列**）；`agent_cards.rs:224-233` `decide()` 失败分支 `else if let Some(r)=self.requests.front_mut().filter(|r|r.id==id)` —— 此刻队列已无该 id → 恒 None → 既不置 `State::Uncertain` 也不写 error；调用点 `:362/:363/:368/:374` 无论成败都 `return false` → 卡片消失。
- 唯一触发窗口：Service 写线程因管道写失败退出（`agent_cards.rs:71` 附近 `if writeln!(input,…).is_err(){break;}`）而子进程尚未被 `try_wait` 回收（`agent_cards.rs:483-485` 一检测到退出就 clear）。
- 期望（交付说明 §1 状态行承诺）：卡片保留 + 状态行「提交结果未知 · 请回客户端确认」+ 错误行原因。
- 实际：`State::Uncertain` 从 UI 路径永远走不到；`agent_cards.rs:640-652` 的单测**直接调 decide 且没有先 remove**，测的是真实路径不存在的状态 → 假绿。
- 修法：`decide` 接收「已取走的 Request」：send 失败则置 Uncertain/error/details/detail_at 后 `push_front` 放回，调用点 `return true` 保持显示；单测改走 `draw_details` 路径。

### D07【P1｜测试设施】UI 测试不执行 `Cards::update`

- 位置：`src/ui_tests.rs:1192-1236`（`H::frame` 只调 `app.frame`）；生产顺序是 `main.rs:104`（`agent_cards.update(...)`）**先于** `app.frame`。
- 后果：policy 同步、retired 交回、结果层 content=0 自动关闭、桥接退出清队列等 update 行为，1479 条 UI 用例**一条都覆盖不到**；task-1 新增的 `turning_completion_content_off_closes_an_open_reply_layer` 实测 **FAILED**。
- 修法：`H::frame` 里按生产顺序补 `self.app.agent_cards.update(&self.app.settings, &self.ctx);`（或新增 `frame_ticked()`），补完一次性跑 `cargo test --lib agent`。

### D08【P2】桥接子进程退出/来源关闭时队列静默清空

- 位置：`agent_cards.rs:482`（两来源都关）、`:483-485`（`child.try_wait()` 有值）→ `requests.clear(); result=None;`。
- 期望：至少一条「连接已断开，未处理请求已交回」提示。实际：卡片直接消失，用户不知处理了还是丢了。

### D09【P2】宿主撤销（`cancel`）到达时卡片静默消失

- 位置：`agent_cards.rs:455`。注意 `tests/fixtures/dsh-host.cjs:70/87/97/104/109` 证明**每次成功作答后也会收到一条 cancel**（服务端 `channel.cjs:111-115` 在 socket 关闭时统一发），所以不能一律当失败提示；建议只在「仍 Pending 且用户未作决定」时给轻提示。
- 真机常见：提问超时被宿主撤销 → 卡片凭空消失。

### D10【P2】`hand_back` 不校验发送结果就移除

- 位置：`agent_cards.rs:234-239`（send 的 bool 被丢弃）。设计如此（单测 `hand_back_removes_even_without_a_bridge` 明确要求），风险是「点回客户端但宿主没收到」时岛上已无卡片，只能靠宿主超时回落。建议复用修好后的反馈通道提示一次。

### D11【P2】队列满 32 / 非法数据 → 静默交回

- 位置：`agent_cards.rs:469-470`（`len()>=32` 等）、`:461-465`（未知来源/类型/解析失败）。行为本身正确（交回原生 UI），但岛上无任何提示，排查时容易误判「卡片该出现却没出现」。建议复用 `jump_note`/toast 通道或在 trace 日志补一行原因。

### D12【P2】`Service::start` 要求 `channel.cjs` 与内置副本逐字节相等，否则静默不启动

- 位置：`agent_cards.rs:64-65`。升级后若导出的 kit 是旧版 → 桥接永远起不来 → 岛上完全不出现审批/提问卡，唯一线索是设置页文案。
- 缓解：`Setup::reconcile` 每次启动都会重新 export 覆盖，多数自愈；但失败原因不落日志/提示，且每 2s 重建一次（`agent_cards.rs:486-488`）。建议 `Service::start` 返回带原因的 Result 并显示到设置页。

### D13【P2】收起态提示卡摘要恒为「第 1 题 / 工具名」

- 位置：`agent_cards.rs:132-138`（`Request::summary()` 用 `data.questions.first()`）→ `extras.rs:547` banner summary。
- 触发：多题提问答到第 3 题点「收起」→ 提示卡显示第 1 题原文。建议用 `question_index` 或增加「第 3/5 题 · 已答 2」。

### D14【P2】Escape 对 Agent 卡片完全无效

- 位置：`app.rs:2026-2028`（只处理 `is_editing()` 的 `cancel_edit()`，不动 `agent_cards`）。不是死点击（界面无 Esc 提示），但阻塞卡缺键盘退出路径。建议 Esc 先 `close_detail`（不交回）。

### D15【P3】`Action::Allow` 守卫失败时静默落空

- 位置：`agent_cards.rs:364-370`。守卫与 `approval.rs:203/208-210` 的禁用条件目前一致（**当前不可达**），但条件一旦漂移就是「点了没反应」。建议守卫失败写 `r.error` 并 `details=true`。

### D16【P3】文案/文档与实现偏差

| 文档承诺 | 实现 |
|---|---|
| AGENT-NOTICES.md:13「跳转失败会在岛上提示具体原因」 | D01（已修待验收） |
| 同「找不到窗口时回落到打开工作目录」 | D02 已删回落 → 文档需同步 |
| AGENT-NOTICES.md:23「结果层…可滚动阅读」 | D03/D04 → 目前不可滚动 |
| 交付说明 §1「长命令在卡内滚动，不再截断」 | D04 → 滚轮被页导航吞掉 |
| 交付说明 §1「提交失败『结果未知』…卡片保留」 | D06 → 不可达 |

### D17【P3】收起瞬间 `notice_details` 残留一帧，把详情画进 44pt 提示卡

- 机制：`app.rs:1931` 计算 `agent_details` 时 `open` 仍为 true（`morph` 在 2081 才改），绘制时 open 已 false → 走 banner 分支 → `extras.rs:529` 因 `notice_details && details_open` 仍成立而 `draw_details`，被提示卡尺寸裁剪。单帧闪烁，下一帧 `app.rs:1949` 自愈。建议 `extras.rs:529` 加 `self.agent_detail_active()`。

### D18【确认无问题，列此避免重复排查】Codex 提问只提示不作答

- `notify.cjs:27`：`if (status==='question') return notify('codex',status)` —— 岛上是 `AlertSource::Agent`（非 AgentCard），走通用 bell 分支只有「关闭」→ 不存在死点击；文档 `AGENT-NOTICES.md:17` 已声明。

### D19【P1｜task-2 修法引入，验收必查】`jump_note` 在长卡片上压住底部按钮行

- 位置（工作区）：`app.rs:2291-2299` + `app.rs:3360-3378`。
- 计算：`stacked_under = side || h+8+64 <= WIN_H(520)` → **仅 h ≤ 448 才画在岛体下方**；否则 `note_y=(h-64).max(44)`，而卡片底部按钮行在 `h-46 .. h-16`（`FOOT_PAD=16`、`BTN_H=30`）。一条 40 字中文错误（BODY 11px 两行约 42pt）会占 `h-64 .. h-22`，**完全盖住「仅允许这一次 / 拒绝 / 回客户端」那一排**。
- 触发：任何 `detail_size().1 > 448` 的卡片（长命令审批、多题审阅页、长结果层）上点跳转 chip。
- 建议：①把 `base_y` 移到卡片头部下方并让 `expanded_height()` 预留该行；或 ②优先用卡片内错误行（`ApprovalView.error` 已有槽位），浮层只服务收起卡。
- 这是「修完之后可能遮挡操作」的新风险，请 E2E 用「长命令审批 + 点跳转 chip」专项验证。

---

## 4. 四视图交互矩阵

### 4.1 审批卡

| 交互 | 应发生 | 实际 | 边界反馈 |
|---|---|---|---|
| 仅允许这一次 | 发 allow → 移除 | agent_cards.rs:364-370 | 缺 operation/tool → 按钮禁用 + 错误行原因（approval.rs:115-121）OK；发送失败 → D06 |
| 拒绝 | 发 deny → 移除 | :363 | D06 |
| 回客户端 | 发 return → 移除 | :362 | D10 |
| 收起 | details=false 回提示卡，**不交回** | :361 | OK（有单测） |
| 点卡片外 | 关详情，**不交回** | app.rs:2257-2267 | OK |
| 长命令滚动 | 卡内滚动 | **被页导航吞掉**（D04） | 失败 |
| 队列 1/N | 状态行右侧 `1 / N 待处理` | approval.rs:57 | OK |
| 交回倒计时 | 收起卡显示；悬停/详情/deferred/busy/hidden 暂停 | agent_cards.rs:373-377 + extras.rs:533-535 | OK |
| 提交中 | 状态行「已提交 · 等待客户端确认」+ 进度条 + 禁用 | approval.rs:51-52/204-207 | OK（正常路径几乎看不到） |
| 提交失败 | 「结果未知」+ 错误行 + 保留卡片 | 设计有、路径不可达 | D06 |

### 4.2 提问卡

| 交互 | 应发生 | 实际 |
|---|---|---|
| 单选/多选 | 单选互斥自定义，多选累加 | OK（question.rs:429-445、:451-464） |
| 自定义输入 | 真 focus、跟随滚动、被裁剪 | OK（:372-393） |
| 下一题 | 要求本题已答，否则禁用 + 原因 | OK（:525-530、:317-323；槽位恒定 :324-329） |
| 上一题 | index>0 才出现 | OK（:336） |
| 检查答案→确认提交 | 先审阅，all_ready 才可提交 | OK（:337-343、:525-530） |
| Ctrl+Enter | 仅审阅页 + 全答 + 非 busy | OK（:555-559） |
| 滚动 | 卡内滚动 | OK（is_editing() 为 true 跳过页导航） |
| 提交失败 | 保留卡片 + 未知态 | D06 |
| 收起/点外/交回 | 同审批 | OK |

### 4.3 完成卡 + 结果层

| 交互 | 应发生 | 实际 |
|---|---|---|
| 轻提示「查看」（有正文） | 打开结果层 | OK（无正文时不出「查看」，只给说明行） |
| 展开卡「查看回复」（content=0） | 打开说明层 | 工作区已修，实测通过 |
| 结果层「复制内容」 | 复制 + 1.4s「已复制到剪贴板」 | OK（说明层无此按钮） |
| 结果层「关闭」 | 关层 + 岛体收起（无空岛） | OK（app.rs:1939-1944） |
| 结果层滚动 | 卡内滚动 | **失败**：D03 + D04 |
| 停留时间 | dwell 计满消失；悬停/结果层/全屏/面板开时暂停 | OK（app.rs:1910-1912） |
| 与其它通知抢占 | 待处理卡优先，完成卡退回队列 | OK（extras.rs:482-501/519-524） |
| 完成卡去重 | 同源同题同正文替换 | OK（extras.rs:309-317；content=0 时不同轮次正文相同会被合并，配合 D05 会掩盖子会话噪声） |

### 4.4 跳转

| 视图 | chip 文案 | 接到 open() | 反馈 |
|---|---|---|---|
| 审批详情 | 切换到客户端/回到会话/打开工作目录 | OK（agent_cards.rs:358-360） | D01（已修）+ D19 |
| 提问详情 | 同上 | OK（同一 match） | 同上 |
| 收起提示卡 | 同上 | 原被吞（extras.rs:559）→ task-2 已修 | D19 |
| Unavailable | 不画 chip（Request::jump() 过滤 available()） | — | OK，无死点击 |

---

## 5. 桥接协议审计

### 5.1 finished 在 content=0/1/2 的行为（逐段）

| 环节 | content=0 | content=1 | content=2 |
|---|---|---|---|
| channel.cjs:121 握手回 content | 0 | 1 | 2 |
| channel.cjs:40-43 wantsText | false（不采集正文） | true | true |
| index.mjs:17-19 采集 | 不采集 | ≤1200 | ≤32000 |
| index.mjs:30-34 payload | text:'' | slice(0,1200) | 全文 |
| 岛 agent_cards.rs:466-467 | String::new() | 再切 1200 | 再切 32000 |
| extras.rs:430 | 空 → 「本轮已结束」 | 原样 | 原样 |
| 完成卡（task-1 后） | 说明行 + 说明层 | 摘要 + 结果层 | 摘要 + 结果层 |
| 桥接等待 | channel.cjs:86 2s → 岛回 done（:134）即成功；岛未运行 → notify 兜底 | 同 | 同 |

结论：三处截断一致，**不存在凭空正文**。唯一缺口：content=1/2 但该轮宿主没有 assistant 文本（真实 DSH 空轮次）时 body 也等于 label，行为退化到 content=0（走说明层），可接受，建议文档补一句。

### 5.2 cancel / 超时 / 断开 / 重连

| 场景 | 宿主侧 | 岛仔侧 | 一致性 |
|---|---|---|---|
| 用户作答成功 | channel.cjs:151-152 回写 → :111-115 socket 关闭时发 `{cancel:id}` | 先 decide 移除，再收到 cancel 无害 | OK（fixture 固化） |
| 宿主超时/中断 | abort → socket destroy → cancel | 卡片静默消失 | D09 无解释 |
| 岛仔回决定但宿主已取消 | socket 已出 `open` → `if (socket)` 为假 → **决定被静默丢弃** | 岛已移除卡片（send 只保证写进管道） | 协议可接受，缺可见反馈 |
| 桥接子进程退出 | 所有 socket 关闭 → 客户端 null → 回落原生 | update 清队列 | D08 静默 |
| 岛仔未运行 | exchange 失败/1.2s 超时 → null → notify 系统提醒 | --agent-notice 入岛（main.rs:1050-1057） | OK |
| DSH 插件卸载 | channel.cjs:113 最后 mounted 断开 → `{dshReady:false}` | agent_cards.rs:454 置 false，设置页文案变 | OK |
| 重连 | channel.cjs:49-52 每 3s 重连 ready；岛 :486-488 每 2s 重启子进程 | OK | 无提示（D08） |
| 并发上限 | channel.cjs:131 `open.size>=32` destroy；岛 :469 ≥32 交回 | 上限一致 | OK |
| 帧上限 | channel.cjs:8 192KB、:19/24 超限 destroy；index.mjs args ≤24k | 岛无额外上限 | OK |

### 5.3 tests/agent-notices.test.mjs 覆盖缺口

1. finished 的 content=1/2 采集与截断（现有只覆盖 content=0 的 `{text:''}`）；
2. 未知 id 的 reply 被丢弃时不炸（channel.cjs:151-152 的 `if (socket)`）；
3. 桥接 32 并发上限；
4. wantsText=false 时**不**采集 assistant/message（隐私边界）；
5. approval 在 call.args 缺失/超长（>24000）时只能 deny/return，不能 allow（index.mjs:22-28、:41）。

> 已于 2026-11-20 补齐：「子会话 turn/end 不推完成」已有实现与断言（`DSH subagent turns never push a completion card to the island`）。

### 5.4 子 agent 过滤补丁（**已由 lead 应用**，此处保留 diff 供复核/回退）

实际落地位置 `integrations/agent-notices/bridge/index.mjs:30-38`（与下方 diff 等价，但守卫放在 `last/calls` 清理之后，以便先释放捕获内容）：

```diff
   ctx.on('session/event', (session, event) => {
     const data=event?.data ?? {};
+    // 子 agent 会话不是用户那一轮：不推完成通知，否则每个子任务结束都会弹一张「本轮已结束」。
+    // 依据：@deepseek-ai/dsh-session 校验的 session.header.origin === 'subagent'。
+    const subagent=session?.header?.origin==='subagent';
     if (event.type === 'turn/start') { last.delete(session); calls.delete(session); }
-    if (event.type === 'assistant/message' && channel.wantsText('dsh')) {
+    if (!subagent && event.type === 'assistant/message' && channel.wantsText('dsh')) {
       const content=data.message?.content ?? data.content;
       ...
     if (event.type === 'turn/end') {
       const text=last.get(session) ?? ''; last.delete(session); calls.delete(session);
+      if (subagent) return;
       void channel.exchange('dsh','finished', ...)
     }
```

要点：
- **不要**过滤 `tool/call`/`tool/result` —— 子会话发起的工具审批仍需要 `calls` 里的操作内容（index.mjs:22-28 是 approval 取 operation 的唯一来源）。
- `turn/start` 的清理保留（WeakMap 按 session 分键，无跨会话污染）。

### 5.5 审批「真实触发」路径（本机 DSH 现状）

`~/.dsh/profiles/desktop/cordis.patch.yml:524-537` 实测：

```yaml
- id: permission
  name: "@deepseek-ai/dsh-permission-presets"
  config:
    presets:
      read-only:          { sandbox: read-only,         approval: ask   }
      workspace-write:    { sandbox: workspace-write,   approval: ask   }
      danger-full-access: { sandbox: danger-full-access, approval: never }
    defaultPreset: danger-full-access
```

- `approval: never` 在 `dsh-user-approval` 内部、**waterfall 分发之前**确定性拒绝（该包 README.zh：`never` 策略在服务内部、waterfall 分发之前执行，因此之后以 prepend 注册的监听器也无法绕过）→ **当前默认预设下永远不会发审批请求**，岛上也就永远不会出现审批卡。这也解释了为什么本轮只能验证「提问/完成」。
- 切到 `read-only` 或 `workspace-write`（均 ask）后，超出沙箱文件权限的操作会经 `ctx.approval.request` 询问。**代码级证据**（不是只靠文档）：
  1. `dsh-tool-bash/lib/index.js:677-683`：`const approvedMode = args.sandbox_permissions !== void 0 && args.justification !== void 0 ? await approveBashEscalation(...) : void 0;` —— 只有「带 sandbox_permissions + justification 的重试」才会走审批；
  2. 同文件 `:377-386` `approveBashEscalation` 里 `approver: ctx.get("approval")` + `justification` → 发审批请求；
  3. 拒绝时工具结果追加 `[sandbox: file access denied under <mode> mode]` 与同轮次升权提示（同文件 `:169` 的 `escalationHintMarker`、`:44-52` `processOutcome`）。
- 因此链路是：**命令被沙箱拒绝 → 模型重试一次带升权 → 这次重试才发审批 → 岛仔接管**。
- 因此可复现路径 = **切换当前会话预设 + 让模型做一次越权文件写**，见 E2E-11。

---

## 6. 不变量核对

| 不变量 | 代码证据 | 结论 |
|---|---|---|
| measure/draw 同源 | 各卡 measure 与 draw 共用同一 Layout | 成立 |
| 不覆盖内置 Monospace | theme::mono 只注册命名家族 | 成立 |
| 阻塞卡不参与「鼠标离开自动收起」 | app.rs:2986-2995 `if self.open && self.agent_panel_active()` 清 leave_since 并保持展开 | 成立 |
| 结果层关闭后不留空岛 | app.rs:1939-1944 result_was_active | 成立（有用例） |
| 点击卡片外关闭（不交回） | app.rs:2257-2267 只 close_detail/close_result | 成立 |
| 关闭详情≠交回 | agent_cards.rs:361 只改 details；单测 collapsing_a_card… | 成立 |
| 自动交回倒计时暂停条件 | agent_cards.rs:373-377 `seconds==0 \|\| hovering \|\| details \|\| deferred \|\| hidden \|\| busy` | 成立（hovering 由 extras.rs:533/566、app.rs:2181 每帧刷新，滞后 1 帧可接受） |

---

## 7. 建议补的自动化断言

### 7.1 前置改造

- **T0** `src/ui_tests.rs::H::frame`（:1192）在 `app.frame` 之前补 `self.app.agent_cards.update(&self.app.settings, &self.ctx);`（与 main.rs:104 同序），或新增 `frame_ticked()`。否则 T2/T6/T7 无法成立。

### 7.2 Rust（src/ui_tests.rs，需 lead 统一排期）

| # | 测试名 | 断言点 | 缺陷 |
|---|---|---|---|
| T1 | agent_result_layer_scroll_persists | open_result(长文) → 正文区 h.wheel(-120) → 断言滚动值 >0（需 `#[cfg(test)] fn result_scroll()`），再跑 2 帧仍 >0 | D03 |
| T2 | approval_body_wheel_scrolls_card_not_page | 长 operation 审批卡 → 记 page → 正文区 wheel → 断言 page 未变 且卡片滚动 >0 | D04 |
| T3 | jump_failure_is_visible_while_card_is_open | 触发不可用/失败跳转 → 断言 strings() 含原因关键词，且卡片打开期间 jump_note 不清空 | D01/D19 |
| T4 | submit_failure_keeps_the_card_uncertain | 注入「死桥接」（`#[cfg(test)] fn drop_service()`）→ 点允许 → 断言 count==1、state==Uncertain、有错误行 | D06 |
| T5 | queue_over_32_hands_back_the_new_request | 32 条后第 33 条 → count==32 且 handed_back 含第 33 个 id | D11 |
| T6 | content_zero_explanation_layer_survives_update | open_result_no_body → 跑 update → 断言 result_open() | D07 |
| T7 | bridge_child_exit_shows_a_reason | 模拟子进程退出 → count==0 且出现可见提示 | D08 |
| T8 | banner_summary_tracks_current_question | 多题答到第 3 题收起 → banner 摘要不是第 1 题原文 | D13 |
| T9 | escape_closes_detail_without_handing_back | Esc → !agent_detail_active() 且 count 不变、handed_back 为空 | D14 |
| T10 | Action 穷尽性护栏 | 删 extras.rs:560/586、agent_cards.rs:377/449 的 `_=>{}`，让新增变体编译报错 | 结构 |

### 7.3 Node（tests/agent-notices.test.mjs，node --test，无需 DSH 运行时）

| # | 测试名 | 断言点 |
|---|---|---|
| N1 | ~~DSH subagent turns never push a completion notice~~ **已落地**（lead 命名 `DSH subagent turns never push a completion card to the island`，tests/agent-notices.test.mjs:86） | mock exchange/notify；`handlers['session/event']({id:'sub',header:{origin:'subagent'}},{type:'turn/end',data:{}})` → finished 调用 0 次、notify 未调用；再用普通 session（`header:{cwd}`）触发 → 恰好 1 次 |
| N2 | DSH completion text follows the content mode | 捕获 `build(mode)`：0 → text ''；1 → ≤1200；2 → ≤32000 且等于采集文本 |
| N3 | DSH skips assistant capture when content is off | wantsText=false → 触发 assistant/message + turn/end → payload text==='' |
| N4 | broker drops replies for unknown ids and keeps serving | 直连 channel.cjs：未知 id reply 后仍能完成一个正常请求 |
| N5 | broker caps concurrent open requests at 32 | 33 并发 → 第 33 个 resolve null |
| N6 | DSH approval never grants when call args are missing | 不 append tool/call 就发 approval → 岛回 allow 时宿主落回 next() |

### 7.4 本次实测记录

task-1 两条新用例通过：

```
running 4 tests
test ui_tests::done_with_body_keeps_the_full_reply_in_the_result_layer ... ok
test ui_tests::done_without_body_still_opens_an_explanatory_result_layer ... ok
test result: ok. 3 passed; 0 failed; 1 ignored
```

task-1 第三条（依赖 Cards::update）当前失败 —— D07 的直接证据：

```
test ui_tests::turning_completion_content_off_closes_an_open_reply_layer ... FAILED
thread '...' panicked at src\ui_tests.rs:9499:5:
content 改成仅状态后正文层必须关闭
```

桥接侧（D05 修复后）本审计复核：

```
✔ DSH subagent turns never push a completion card to the island (27.2ms)
...
ℹ tests 12
ℹ pass 12
ℹ fail 0
```

同一轮 cargo 输出中的死代码警告（D15/字段表证据）：`hit_any`、`attention`、`pos`、`edge`、`head_height_public`、`footer_y`、`height_for`、`placeholder` —— 8 条 `is never used`。

---

## 8. 真机 E2E 必测清单（步骤级）

> 前置：只保留一个 daozai 实例；DSH「设置 → 消息提醒 → AI 助手 → DeepSeek Harness」=**交互插件已连接**；岛仔「岛上处理审批与提问」开、「自动交回 = 不自动交回」；系统有中文 OCR 包。E2E-1~10、12 可用 `scripts/agent-e2e.ps1` / `scripts/agent-card-probe.mjs` 驱动，不需要真实模型；E2E-11 需要真实模型。

| # | 场景 | 步骤 | 通过判据 |
|---|---|---|---|
| E2E-1 | 审批全链路 | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Scenario approval` | exit 0；驱动 JSON decision=allowed-once；截图见「仅允许这一次」被点中 |
| E2E-2 | 提问单选/多选/多轮 | `-Scenario question / question-multi / question-multi-turn` | 驱动拿到 answers；多轮走完「下一题 → 检查答案 → 确认提交」 |
| E2E-3 | 完成通知（content=2） | `-Scenario done` | decision=done；点「查看回复」出现结果层 +「复制内容」 |
| E2E-4 | 完成卡无正文（用户真实配置 content=0 + 展开结果卡） | 设置→Agent→显示方式「展开结果卡」、完成内容「仅状态」；真实 DSH 发一轮对话让它结束 | 摘要行出现「仅状态 · 未接收回复正文」；点「查看回复」出现「本轮未接收回复正文」说明层（含设置指引）、**没有**「复制内容」；关闭后岛体收起无空岛 |
| E2E-5 | 长回复滚动 | content=2，DSH 回复 ≥1500 字 → 查看回复 → 正文区滚轮 3 下 | 内容持续下移不弹回（未修则记失败 = D03） |
| E2E-6 | 长命令审批滚动 + 后台页不被翻走 | probe approval 场景 `PROBE_OPERATION` 给 60 行 JSON；先把岛切到音乐页；正文区滚轮 | 命令卡内滚动；后台页不变（未修则失败 = D04） |
| E2E-7 | 跳转 chip（详情卡） | 提问卡第 1 题点右上「切换到客户端」 | DSH 到前台；失败时卡片/岛体下方出现具体原因；再次点击（已在前台）也有结论，且**不再打开资源管理器** |
| E2E-8 | 跳转 chip（收起卡） | 审批卡「收起」→ 提示卡点 chip | 同 E2E-7（原为死点击） |
| E2E-9 | 收起/点外/交回三分 | 详情 → 收起 → 回提示卡（驱动仍无决定）→ 查看并审批 → 回客户端 | 收起与点外都不产生决定；只有「回客户端」产生 return/回落 |
| E2E-10 | 提交失败可见 | 结束桥接 node.exe（或关设置里的 DSH 来源）后点「仅允许这一次」 | 卡片保留 + 「提交失败，结果未知」（D06 未修则记为已知缺陷，勿判为 exe 回归） |
| E2E-11 | **真实 DSH 审批（真实触发路径）** | ① DSH 当前会话输入 `/permission read-only`（或点输入框附近权限控件选「仅可查看」）——注意通用设置里的「默认权限」只影响新会话；② 发：「用 bash 在 C:\Windows\Temp 下创建 daozai-e2e.txt，内容 hello」；③ 沙箱先拒绝（`[sandbox: file access denied under read-only mode]`），模型按提示用 `sandbox_permissions`+`justification` 重试同一命令 → 岛仔弹审批卡（tool=bash、operation=命令 JSON、理由=justification）；④ 在岛上点「拒绝」；⑤ 会话里应看到升级被拒、命令未执行；⑥ 结束后 `/permission danger-full-access` 还原 | 岛仔出现审批卡且能点是；DSH 工具结果体现 rejected；无正文落盘。若 DSH 自己弹审批（岛上没接管）→ 桥接未连接，先查设置页 |
| E2E-12 | 子 agent 完成噪声（D05 修复后） | 让主会话调用一次 subagent / Agent Teams 任务 | 岛上只出现**一张**主会话「本轮已结束」，无子任务完成卡 |

> E2E-11 安全提醒：`read-only` 下所有写操作都要审批，测完必须切回 `danger-full-access`，否则后续自动化会被审批卡住。

---

## 9. 已确认无问题（避免重复排查）

1. 10 个 Action 变体均有消费点；真正会吞动作的只有 `extras.rs:559`（Jump，已分派 task-2）与 `extras.rs:576`（ShowResult content=0，已修）。
2. 禁用态均不可点击且都有可见原因：审批缺 operation（approval.rs:115-121）、提问未答满（question.rs:317-334 + 槽位恒定）、提交中/未知全禁用（approval.rs:208-216、question.rs:525-530）。
3. 提问卡作答前后高度不跳、按钮不位移（agent_ui/mod.rs 单测 + question.rs:324-329）。
4. 五条不变量全部成立（§6）。
5. 去重/上限：同 id 重发只保留一条且索引在移除后计算（agent_cards.rs:473-477）、题量/选项/题干上限与去重（:86-100）、deferred 排序（:476）。
6. 隐私边界：content=0 时桥接不采集正文（channel.cjs:40-43）、岛不显示正文、只展示事件文本、不读历史、不落盘；extras.rs:457-462 还会二次截断。
7. 桥接鉴权与本地性：命名管道 + HMAC 双向证明（channel.cjs:9/33-35/77-80）、192KB 帧上限、无网络端口。
8. Codex 提问只提示不作答（文档已声明）；Codex 审批 allow 必须 operation 完整且 ≤24000（notify.cjs:42-50）。
9. `--agent-notice` CLI 只接受固定 id 表（main.rs:1050-1057 + agent_notice.rs:69-83），不接收任意文本。

---

## 10. 遗留风险 / 交接

1. **工作区正在被 task-1/task-2 修改**（审计时 `git diff --stat`：agent_cards.rs +76、agent_jump.rs +458、app.rs +71、done.rs、extras.rs、metrics.rs、ui_tests.rs）。D19 是针对 task-2 新代码的验收风险，**必须在真机长卡片上验证**。
2. **D03/D04 的写入范围**跨 `src/agent_ui/done.rs`（task-1）与 `src/app.rs`（task-2），需 lead 排期或指定一人改，避免并发冲突。
3. **D05 已合入**（`integrations/agent-notices/bridge/index.mjs:30-38` + 回归用例，12/12 绿）；只剩 P3 残余：`index.mjs:17-19` 的 `assistant/message` 采集仍无 subagent 守卫（内存短暂、turn/end 即删，无功能影响）。
4. **D07 是「测试绿但生产红」的根源**，建议先做 T0，再让 task-1 那条用例转绿；否则本轮所有「已验证」结论都要打折。
5. 本轮未做（超范围）：真机点击、release 构建、交付 exe 覆盖、鼠标键盘自动化。

---

### 附：取证命令（只读，可复现）

```powershell
# 1) 窗口样式 / 前台：Add-Type Win32 (EnumWindows / GetWindowLongPtrW / GetForegroundWindow)，只读
#    实测岛仔 ex=0x08000088（NOACTIVATE+TOPMOST+TOOLWINDOW），DSH 主窗 ex=0x00000100 普通、未最小化
# 2) 真实宿主 session/event 全局语义（含 subagent）
$env:ELECTRON_RUN_AS_NODE='1'
& '<DSH>\DeepSeek Harness.exe' <临时脚本>   # 挂 dsh-session + {global:true} 收 subagent turn/end（§D05）
# 3) 本机权限预设（审批是否可能发出）
Get-Content "$env:USERPROFILE\.dsh\profiles\desktop\cordis.patch.yml" | Select-Object -Skip 523 -First 15
# 4) 单测
powershell -NoProfile -ExecutionPolicy Bypass -Command ". ./scripts/dev-msvc-env.ps1; cargo test --lib --target x86_64-pc-windows-msvc turning_completion_content_off_closes -- --test-threads=1"
```
