# 完成卡「查看回复」死按钮 + 审批提交失败吞卡：根因与修复（2026-11-20）

协作 agent：fix-result（任务板 task-1）。分支 main，基线 HEAD `7d028df` + 本轮未提交改动。
用户报障 A：「本轮结束点击查看回复没有反应」。审计附带 B：「审批提交失败时卡片直接消失，宿主却还在等」。

## 0. 结论（先行）

**A：死按钮确实存在，且是配置相关的静默 no-op。**
用户真实配置是 `agent_cards={"expanded":true,"content":0,...}`（展开结果卡 + 完成内容=仅状态）。
`agent_finished()` 在 content=0 时把正文换成状态文案「本轮已结束」（src/extras.rs:430），
而展开式完成卡无条件渲染 [查看回复][关闭]（src/agent_ui/done.rs:124），
点击后 `draw_notice` 的 `Action::ShowResult` 分支要求 `content>0 && body != "本轮已结束"` 才 `open_result`，
否则整段什么都不做（旧代码 src/extras.rs:576-581）→ 点击被吞，界面零变化。

**修法：让「查看回复」在任何配置下都有可见结果，同时不伪造正文。**
桥接侧在 content=0 时压根不采集 assistant 文本（lead 勘察 integrations/agent-notices/bridge/index.mjs），
岛上手里没有正文可显示，所以不能「点开就有正文」，只能诚实说明原因与下一步：
- 完成卡上先给一行轻提示「仅状态 · 未接收回复正文」（点之前就知道里面没有正文）。
- 点「查看回复」→ 打开结果层，标题「本轮未接收回复正文」，正文位置是本机生成的说明
  （当前「完成内容」=仅状态 → 去 设置 → Agent → 完成内容 改成「回复摘要/本轮回复」），
  底部**只有 [关闭]**（没有可复制的正文，不显示「复制内容」），副标题「本机说明 · 不含回复内容」。
- content=1/2 行为完全不变（正文、复制内容、关闭）。
- 隐私边界：content 从 1/2 改成 0 时，已打开的**正文层**立刻关闭；本机说明层不含正文，保留（否则点开就消失又成假死）。

**D03（审计新增）：结果层滚动位置不保存，滚轮等于没用。**
`draw_result` 里 `let scroll = viewport.wheel(&c, v.scroll, w);` 只用于本帧绘制，
函数返回 `Option<Action>`，而 `Cards::result_scroll` 只在 `open_result/close_result` 归零、从不写回，
于是长回复每帧都从 0 开始（滚一下、下一帧跳回顶部）。
**修法**：`draw_result` 改为返回 `(Option<Action>, f32)`，`Cards::draw_result` 里写回 `self.result_scroll=scroll`，
与审批详情（`r.scroll`）、提问正文同一套模式。

**B：审批/提问提交失败路径是坏的（State::Uncertain 是死代码）。**
`draw_details` 先把请求从队列 `remove`（src/agent_cards.rs:324），再走分支里的 `decide(&r.id,...)`；
`decide` 的失败分支用 `requests.front_mut().filter(|r|r.id==id)` 找它（修复前写法），
此时队列里已经没有这条请求 → 永远 None → 既不置 `Uncertain`、也不写 error，
随后 `draw_details` `return false`，卡片直接消失，而宿主仍在等回复。
**修法**：把已经取出的请求交给新的 `resolve(r, reply)`；发送失败时 `mark_uncertain(r)`
（state=Uncertain + error「提交失败，结果未知。请回客户端确认后再处理。」+ details=true）并 `push_front` 放回，
`resolve` 返回 true 表示继续显示卡片。允许/拒绝/提交答案三条路径都走它。

## 1. 改动清单（文件:行号）

| 文件 | 位置 | 改动 |
| --- | --- | --- |
| src/agent_ui/done.rs | 15-21 | 新增 `NO_BODY_HINT`（卡上轻提示）与 `has_detail()`（统一「有没有正文」判定） |
| src/agent_ui/done.rs | 24-33 | `measure_done` 把说明行高度计入卡片高度（否则按钮压字） |
| src/agent_ui/done.rs | 83-90 | 轻提示形态（compact）无正文时在摘要位显示说明，而不是留空/丢「查看」 |
| src/agent_ui/done.rs | 115-123 | 展开卡无正文时显示「仅状态 · 未接收回复正文」，footer 仍渲染 [查看回复][关闭] |
| src/agent_ui/done.rs | 246-259, 306-315 | 结果层：`hint` 时副标题「本机说明 · 不含回复内容」、footer 只有 [关闭]、不返回 CopyResult |
| src/extras.rs | 575-585 | `Action::ShowResult`：有正文 `open_result`，无正文 `open_result_no_body`，**两条路径都 `open_agent_panel()`** |
| src/agent_cards.rs | 155-172 | 新增 `NO_BODY_RESULT_TITLE/TEXT`（本机说明文案，不含任何宿主正文）+ `result_kicker()` |
| src/agent_cards.rs | 174-181 | `ResultCard` 增加 `hint` 字段，区分「正文层」与「本机说明层」 |
| src/agent_cards.rs | 413-424 | `open_result` 置 hint=false；新增 `open_result_no_body` 置 hint=true |
| src/agent_cards.rs | 225-249 | `decide` 失败分支改为取出队首交给 `mark_uncertain`；新增 `mark_uncertain`/`resolve` |
| src/agent_cards.rs | 376-385 | `draw_details` 的拒绝/允许/提交答案改走 `resolve`（失败必留卡片） |
| src/agent_cards.rs | 500 | 结果层隐私保护：`content==0 && !hint` 才关闭（正文层立刻关，说明层保留） |
| src/agent_ui/done.rs | 246-319 | `draw_result` 返回 `(Option<Action>, f32)`（新滚动偏移），不再把偏移丢在局部变量里 |
| src/agent_cards.rs | 440-444 | `Cards::draw_result` 把新偏移写回 `self.result_scroll`（修 D03：滚轮每帧跳回顶部） |
| src/agent_ui/mod.rs | 134 | `ResultView` 增加 `hint: bool`（唯一一处出写入范围的改动，见 §5-1） |
| src/ui_tests.rs | 1192-1199 | `H::frame` 与生产 main.rs:104 对齐：先 `agent_cards.update`，再把 finished 交给 `agent_finished` |
| src/ui_tests.rs | 9394-9399 | 画廊「结果查看」用例显式设 content=2（说明层素材另行导出） |
| src/ui_tests.rs | 9432-9590 | 新增 5 条回归用例 + 1 处既有断言修正（见 §4） |

## 2. 行为矩阵（点击「查看回复」）

| 配置 | 点之前卡片 | 点了之后 | 层内按钮 | 关闭后 |
| --- | --- | --- | --- | --- |
| 展开卡 + content=0（用户现状） | 「仅状态 · 未接收回复正文」 | 本机说明层（原因 + 下一步） | [关闭] | close_result + 收起面板 |
| 轻提示 + content=0 | 摘要位「仅状态 · 未接收回复正文」，无「查看」气泡 | 无可点（不是死按钮） | — | — |
| 展开卡 + content=1/2（有正文） | 正文摘要 | 结果层正文（markdown/滚动/复制） | [复制内容][关闭] | 同上 |
| content 2→0（层已打开） | — | 正文层立刻关闭（隐私） | — | 面板收起 |

## 3. 为什么不是「点开就有正文」
桥接 `integrations/agent-notices/bridge/index.mjs:17-19,30-34` 只在 `channel.wantsText('dsh')` 时收集 assistant 文本；
content=0 时策略不下发采集，turn/end 的 finished payload 里 text 为空，`receive()` 只拿到空串（src/agent_cards.rs:466），
`agent_finished` 再把 body 落成状态文案（src/extras.rs:430）。所以岛上确实没有正文，
任何「点开显示正文」的实现都只能靠伪造，违背要求 2。说明层文案全部来自本机常量，可审计。

## 4. 测试证据

新增/调整的用例（src/ui_tests.rs）：

1. `done_without_body_still_opens_an_explanatory_result_layer`（9432）：content=0 + 展开卡
   → 卡上有「查看回复」与轻提示；点击后 `result_active()==true`、标题「本轮未接收回复正文」、
   说明里含「完成内容」、只有 [关闭] 没有 [复制内容]、**不出现**被丢弃的正文、关闭后 `result_active()==false` 且 `!open`。
2. `done_with_body_keeps_the_full_reply_in_the_result_layer`（9465）：content=2 正文完整（条目 + 最后一段）、
   标题仍是「本轮回复」、保留 [复制内容]，关闭后收起。
3. `turning_completion_content_off_closes_an_open_reply_layer`（9493）：content=2 打开正文层后把设置改成 0，
   逐帧（经 `H::frame` 内的 `agent_cards.update`）后正文层必须关闭且面板收起。
4. `allowing_without_a_bridge_keeps_the_card_and_reports_unknown`（9514）：走真实 UI 路径点「仅允许这一次」
   （无桥接子进程）→ 请求数仍为 1、详情卡仍在、文案含「提交结果未知」与「提交失败」。
5. `result_layer_keeps_its_scroll_between_frames`（9538）：content=2 打开 40 段长回复，
   直接断言「最后一段标记」的绘制矩形进入视口（用 shape.clip_rect.contains(center) 判断可见）：
   打开时不可见 → 指针停在正文上滚 20 次后可见 → 再空跑 3 帧仍然可见（修复前每帧从 0 开始，这里必红）。
6. 既有用例修正：`agent_notice_question_is_answered_inside_the_island`（6928-6935，ui_tests.rs:6932 的新断言）原本断言「确认提交后 count()==0」，
   这正好是 B 的 bug 行为（无桥接时提交失败也把卡片删掉）。现改为断言 count()==1 + 「提交结果未知」，
   与修复后的真实语义一致；该用例的前提（测试环境无桥接）没变。

命令与结果：

```
# 新增 5 条
cargo test --lib -- --test-threads=1 done_without_body... done_with_body... turning_completion... allowing_without_a_bridge... result_layer_keeps_its_scroll
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1478 filtered out; finished in 0.75s

# 全量 agent 相关（filter=agent）
cargo test --lib agent -- --test-threads=1
<结果见 §4.1>

# 全部 ui_tests（filter=ui_tests::）
cargo test --lib ui_tests:: -- --test-threads=1
<结果见 §4.2>
```

### 4.1 `cargo test --lib agent -- --test-threads=1`

```
test result: FAILED. 55 passed; 1 failed; 3 ignored; 0 measured; 1423 filtered out; finished in 1.21s
---- 唯一失败：calendar::transport_tests::tls::calendar_agent_uses_system_tls_and_rejects_untrusted_certificate_before_http
thread 'calendar::transport_tests::tls::...' panicked at src\calendar\transport_tests.rs:62:63:
called `Result::unwrap()` on an `Err` value: Os { code: 5, kind: PermissionDenied, message: "拒绝访问。" }
```

该用例只是名字里含 "agent"（calendar_agent_...），与 Agent 卡片无关：它在
`native_tls::Identity::from_pkcs8(include_str!("tls_test_certificates.json"))` 处被本机拒绝访问（环境/证书权限问题）。
跳过这两条 TLS 环境用例后：

```
cargo test --lib agent -- --test-threads=1 --skip calendar_agent_uses_system_tls --skip trusted_private_ca
test result: ok. 55 passed; 0 failed; 3 ignored; 0 measured; 1424 filtered out; finished in 1.17s
```

### 4.2 `cargo test --lib ui_tests:: -- --test-threads=1`

```
test result: ok. 430 passed; 0 failed; 49 ignored; 0 measured; 1004 filtered out; finished in 167.98s
```

覆盖全部 H harness 的视觉/交互用例（含新加的 5 条与修正过的提问提交用例），0 失败。

### 4.3 全量 `cargo test --lib -- --test-threads=1`（含所有模块）

```
test result: FAILED. 1347 passed; 4 failed; 132 ignored; 0 measured; 0 filtered out; finished in 282.20s
```

4 条失败全部是环境依赖、且都不在本轮改动文件内（见 §6）：2 条 calendar TLS、1 条 Shell_NotifyIconW、
1 条 DPAPI。这是修复 D03 之后的最终全量结果；第一轮曾中途进程异常退出一次（未复现，判定为与本改动无关的偶发）。
Agent 卡片 / 审批 / 提问 / 完成相关用例 **0 失败**。

## 5. 遗留风险与未覆盖

1. **`src/agent_ui/mod.rs` 一行出界**：`ResultView` 加 `hint: bool` 是「说明层只有 [关闭]」的最干净实现。
   写范围里没有 mod.rs，但不加就得用「文案标记/标题匹配」这类脆弱判断，故做了这一处纯新增字段（3 处构造器同步），
   无行为影响。若 lead 不接受可回退：删字段 + done.rs 按 `v.kicker` 判断（不推荐）。
2. **Uncertain 卡片没有出口（需要 lead 决定是否本轮修）**：`approval.rs:208` 用
   `draw_footer_with_enabled(..., !busy, |id| id != "agent-allow" || allow_ready)`，
   `question.rs:525` 同样以 `!busy` 作 any_enabled；`state.busy()` 时整排 footer 全禁用（含「回客户端」），
   而 `countdown` 对 busy 请求返回 None（src/agent_cards.rs:407-409 的 `r.state.busy()`）。
   于是提交失败后卡片会一直停在岛上，只能去客户端取消或关掉 Agent 通知。建议把「回客户端」在 busy 时保持可用
   （它走 hand_back，不依赖桥接），改动 2 行，文件 approval.rs / question.rs **不在本任务写入范围，未改**：

   ```rust
   // src/agent_ui/approval.rs:208
   widgets::draw_footer_with_enabled(&c, footer_y, w, &layout.footer, true, |id| {
       id == "agent-return" || (!busy && (id != "agent-allow" || allow_ready))
   })
   // src/agent_ui/question.rs:525 同理：先放行 "agent-return"，其余仍要 !busy
   ```
3. content=1 桥接最多 1200 字（既有策略，未改）；说明层不含正文，不会放大隐私面。
4. 结果层「关闭」后收起面板依赖 app.rs 的 `result_was_active`（未改，`agent_completion_result_opens...` 已覆盖）。
5. 未做真机 GUI 鼠标验证（由 lead / e2e-operator 执行）；本报告结论来自 egui 无头测试 + 代码路径。
6. 全量 `cargo test --lib` 在本机有 4 条与本次改动无关的环境失败（TLS 系统证书、Shell_NotifyIcon、DPAPI），
   见 §6；另有一次未复现的整进程异常退出说明也见 §6。

## 6. 环境相关失败（非本次改动引入，复跑稳定复现 4 条）
- `calendar::transport_tests::tls::calendar_agent_uses_system_tls_and_rejects_untrusted_certificate_before_http`
  与 `...trusted_private_ca_allows_caldav_discovery_and_keeps_certificate_checks`：
  均在 transport_tests.rs:62 `native_tls::Identity::from_pkcs8(...).unwrap()` 处
  `Os { code: 5, kind: PermissionDenied, message: "拒绝访问。" }`（本机证书/私钥访问受限）。
- `messenger::qq::tests::qq_single_tray_account_ignores_its_auxiliary_main_windows`：qq.rs:130
  `assertion failed: Shell_NotifyIconW(NIM_ADD, &fixture.icon)`（需要真实任务栏/explorer 交互）。
- `quota::api::tests::custom_script_storage_is_encrypted_and_can_refresh_after_reload`：api_quota.rs:252（DPAPI/加密存储受限）。

以上文件与模块均未被本轮改动触及；两轮全量跑结果一致。第一轮全量跑还遇到一次整进程异常退出
（发生在 `island_meshes_stay_valid_after_long_uptime_and_repeated_hover`，该用例单独跑 10.21s 通过），
第二轮未复现，判定为与本改动无关的偶发。

## 7. 手工验收（给 E2E）
1) 设置：Agent → 显示方式=展开结果卡，完成内容=仅状态；跑一轮后点「查看回复」→ 出现说明层，文案含「仅状态」，只有 [关闭]；关闭后岛体收起。
2) 改成「本轮回复」再跑一轮 → 点「查看回复」看到正文 + [复制内容][关闭]。
3) 断开桥接（或直接在有审批时停掉 channel）点「仅允许这一次」→ 卡片保留并显示「提交结果未知 · 请回客户端确认」。
