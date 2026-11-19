# 岛仔 Agent 通知 / 审批 / 提问 UI 现状勘察

- 日期：2026-11-20
- 范围：只读勘察 `src/agent_cards.rs`、`src/app.rs`、`src/extras.rs`、`src/agent_notice.rs`、`src/agent_jump.rs`、`src/paint.rs`、`src/theme.rs`、`src/anim.rs`、`src/model.rs`、`src/ui_tests.rs`、`docs/AGENT-NOTICES.md`、`docs/PROJECT-HANDOFF.md`
- 方式：静态读码。**未运行 `cargo build` / `cargo test` / 未启动程序**，所有尺寸取自代码常量与算式，行为结论取自控制流。
- 目的：为「把 Agent 弹窗从半成品做成成熟产品」提供精确现状基线。

---

## 1. 绘制路径地图

### 1.1 总流程图

```
DSH / Codex 客户端
   │  命名管道（bridge/channel.cjs，HMAC 握手）
   ▼
channel.cjs ──stdin/stdout NDJSON──▶  Service 线程      src/agent_cards.rs:58-72
   │                                    │ sync_channel(96)
   │                                    ▼
   │                        Cards::update(settings, ctx)      src/agent_cards.rs:354-377
   │                        ← 每帧由 main.rs:104 调用
   │                                    │
   ├── kind=="finished" ────────────────┤ 返回 (Provider, text)
   │                                    │
   │                       App::agent_finished(p, text)       src/extras.rs:423-433
   │                                    ├─ Cmd::Notify（可选，不带正文） extras.rs:427-429
   │                                    └─ enqueue_alert(Alert{AgentDone})   extras.rs:431
   │                                              │
   │                                       tick_alerts()      src/extras.rs:441-508
   │                                              │ 置于 self.notice
   │                                              ▼
   │                                    draw_notice(...)      src/extras.rs:509
   │                                       └─ AgentDone 分支 extras.rs:531-546
   │                                          360×108 展开卡 / 334×44 轻提示
   │
   └── kind=="approval" | "question" ───▶ Cards::receive(v, settings)  src/agent_cards.rs:328-353
                                              │ 契约校验 → 入队 requests（≤32）
                                              ▼
                                          tick_alerts()      src/extras.rs:478-501
                                              │ 取队首 active() → Alert{AgentCard(provider,id)}
                                              ├─ title=="需要你回答" → detail(id)+open_notice_details()  extras.rs:487-490
                                              ├─ title=="等待你审批" → detail(id)+open_notice_details()  extras.rs:492-496
                                              └─ 其余 → self.open=false                        extras.rs:497
                                              ▼
                               ┌────────────── 渲染分叉（同一帧内）──────────────┐
                               │ app.rs:2109  agent_details = open && notice_details && details_open
                               │     → Card(panel.min + 居中 360, clip=panel)
                               │     → Cards::draw_details                      app.rs:2110-2114
                               │ app.rs:2126  banner = !compact_notices && notice && !open
                               │     → Card(panel.min + pet_space, panel)
                               │     → draw_notice → AgentCard 分支            extras.rs:511-529
                               │         ├─ 详情已开 → draw_details            extras.rs:512-514（实际不可达，见 5.19）
                               │         └─ 否则 360×108 compact 卡            extras.rs:516-528
                               │ app.rs:2131  compact_notices（歌词/极简）时只有右侧 36×30 badge
                               │ app.rs:2178  notice_details_rect 浮层：AgentCard 直接 None
                               └───────────────────────────────────────────────┘
                                              │
                    「回客户端 / 拒绝 / 仅允许这一次 / 提交回答 / 上一轮 / 下一轮」
                                              ▼
                                 Cards::reply / hand_back                   src/agent_cards.rs:133-141
                                              │ NDJSON → Service.out → channel.cjs
                                              ▼
                                  remove(id) → 队列前进 → tick_alerts 取下一个
```

### 1.2 逐段对照表

| 阶段 | 入口函数 | 坐标系 / 容器 | 尺寸来源 | 关键位置 |
|---|---|---|---|---|
| 协议接收（NDJSON 解析、policy 下发、批量取 96 条） | `Cards::update` | 无绘制 | — | `agent_cards.rs:354-377` |
| 契约校验与入队 | `Cards::receive` → `question_violation` | 无绘制 | 队列上限 32 | `agent_cards.rs:328-353`、`:85-99`、`:344` |
| 完成通知入队 | `App::agent_finished` | 无绘制 | `dwell` 秒 | `extras.rs:423-433` |
| 队首转成 Alert、自动展开策略 | `App::tick_alerts` | 无绘制 | — | `extras.rs:478-501` |
| compact 交互卡（banner 形态） | `extras::draw_notice` AgentCard 分支 | 岛体 panel 局部坐标（`Card::new(ui, panel.min+pet_space, panel.intersect(..))`） | `notice_size` → (360,108)，base_size 再加 `pet_space()` 44 | `app.rs:2126-2127`、`app.rs:1782`、`extras.rs:511-529` |
| 审批 / 提问详情（面板形态） | `Cards::draw_details` | island panel 局部坐标，origin 居中：`panel.min + (panel.width-360)/2` | `detail_size()`；面板高 = `expanded_height()` | `app.rs:2109-2114`、`app.rs:1826-1832`、`agent_cards.rs:161-185`、`:187-321` |
| 审批 / 提问详情（banner 形态，理论路径） | `Cards::draw_details`（经 `draw_notice`） | 岛体 panel 局部坐标，左对齐 panel.min | `detail_size()` | `extras.rs:512-514`（现不可达，见 5.19） |
| 右缘常驻 badge | `frame` 的 `notice_rect` 分支 | 独立 `notice_rect`（岛体右侧 +8） | 36×30（`notice_width.pos-8` × `BAR_H`） | `app.rs:2131-2158`、`app.rs:2554-2558` |
| 详情浮层（badge 下方 +6） | `frame` 的 `notice_details_rect` 分支 | 独立 `notice_details_rect` | `notice_size` | `app.rs:2178-2203`、`app.rs:2559-2567`；**对 AgentCard 返回 None**（`:2561`） |
| 完成通知轻提示 | `draw_notice` AgentDone 分支 `wide=false` | 岛体 panel 局部 | `notice_size` → (334,44) | `extras.rs:531-546`、`app.rs:1785` |
| 完成通知展开卡 | 同上 `wide=true` | 岛体 panel 局部 | (360,108) | `extras.rs:533-545`、`app.rs:1784` |
| 结果查看 | **不存在**（只写不画，见 5.1） | — | — | `extras.rs:544` 写 `agent_cards.result`；`app.rs:1879` 只读 `.is_some()` |
| 固定状态提醒（tray/legacy 路径） | `draw_notice` 的 `a.source!=Internal` 分支 | 岛体 panel 局部 | (334,44) | `extras.rs:560-573`、`agent_notice.rs:414-421` |

### 1.3 窗口尺寸如何被 `expanded_height` 决定

- 透明画布：`WIN_W=640`、`WIN_H=270`、`CANVAS_W=960`（`app.rs:50-52`）。测试与录屏的画布就是 640×270（`ui_tests.rs:1195`）。
- 展开面板基准：`PW=419`、`PH=140`（`app.rs:23-24`）；收起药丸 `BAR_W=163 / BAR_H=30`（`app.rs:39-40`）。
- `expanded_height()`（`app.rs:1826-1832`）：
  ```
  if agent_detail_active() → detail_size(id).1.max(PH).min(WIN_H - 8.0)   // 即 [140, 262]
  else if 日历页 → CALENDAR_H ; 天气页 → WEATHER_H ; 页6 → 176 ; 页5 读信 → 236 ; 否则 PH=140
  ```
- `agent_detail_active()` = `self.open && self.notice_details && details_open(队首 id)`（`app.rs:1773-1776`）。**详情高度只有在“面板形态”下才参与窗口高度**；banner 形态的高度走 `base_size() → notice_size()`（`app.rs:1794-1797`、`:1777-1787`）。
- 弹簧：`self.w` / `self.h` 由 `morph()` 驱动（`app.rs:2876-2944`），展开时 `h.target = expanded_height()`（`app.rs:2899`、`:2941`），宽度 `w.target = PW`。因此「详情高度」是通过改变面板弹簧目标实现的，**内容坐标在高度到位之前就已经按最终高度算好**（见 5.6）。
- 帧循环 `animating` 条件包含 `!self.h.settled(self.expanded_height())`（`app.rs:2305`），所以高度变化期间保持 60fps 重绘；稳定后 `repaint_after` 退到 1s（`app.rs:2327`）。

---

## 2. 几何清单（均为逻辑点，代码常量，未实测）

### 2.1 容器

| 容器 | 宽 | 高 | 备注 |
|---|---|---|---|
| 透明画布 | 960（`CANVAS_W`） | 270（`WIN_H`） | 岛体只用左侧 640 定位（`app.rs:50-51`） |
| 展开面板 | 419（`PW`） | 140 ~ 262（`expanded_height`） | 262 = `WIN_H-8`，这是全系统硬上限 |
| 收起药丸 | 163（`BAR_W`） | 30（`BAR_H`） | 极简条 44×4.5 |
| Agent compact 卡（岛体即卡片） | 360 + `pet_space()` 44 | 108 | `app.rs:1782`；`notice_size` + `base_size` `app.rs:1794-1797` |
| 完成通知轻提示 | 334 + pet | 44 | `app.rs:1785` |
| 完成通知展开卡 | 360 + pet | 108 | `app.rs:1784` |
| 右缘 badge | 36（`notice_width.pos-8`，弹簧目标 44） | 30 | `app.rs:2554-2558`；AgentCard 永不出现（见 5.19） |
| 详情浮层 | `notice_size.0` | `notice_size.1` | AgentCard 恒为 None（`app.rs:2561`） |

### 2.2 compact 交互卡（360×108，`extras.rs:511-529`）

| 元素 | 局部坐标 / 尺寸 | 字号 / 字重 |
|---|---|---|
| 来源图标中心 | (26, 24)，直径 26 | — |
| 标题 `"{label} · {title}"` | x 左基线 48，垂直中心 y=23，**不截断** | 13 / Semibold |
| 队列计数 `"1 / N"` | 右对齐 x=340，y=23（`N>1` 才画） | 11 / Regular，`theme::GRAY` |
| 正文（= `data.tool` 或第 1 题题干） | x=18，y=49，`fit(...,320.0)` **单行** | 12 / Regular |
| 自动交回倒计时 `"{s} 秒后交回"` | x=18，y=82 | 10.5 / Regular，GRAY |
| 「回客户端」 | (152, 72) 90×20 | 10.5 / Semibold |
| 「查看并审批」/「查看并回答」 | (250, 72) 90×20 | 10.5 / Semibold |

按钮底 = 92，卡片底 = 108 → 底部留白 16；「回客户端」左边缘 152 与倒计时文本（18 起，约 60~80 宽）之间有大片空白。

### 2.3 审批详情（360 × [214, 242]，`agent_cards.rs:163-174` / `:284-313`）

高度算式（`agent_cards.rs:168-173`）：

```
reason_lines    = ((reason.chars().count() + 47) / 48).min(2)          // 0..2
operation_lines = Σ_每行 ceil((chars+53)/54)，每行至少 1，总和 clamp(1,4)
height          = clamp(160 + reason_lines*13 + operation_lines*14, 214, 262)
```
即实际区间 **214（无 reason、1 行操作）~ 242（2 行 reason、4 行操作）**；`native_scope` 固定 **360×152**（`agent_cards.rs:164`）。

| 元素 | 局部坐标 / 尺寸 | 字号 |
|---|---|---|
| 图标中心 | (26, 18)，直径 22 | — |
| 标题 `"{label} · {title}"` | x=44，y=18，`fit(...,206.0)` | 12 / Semibold |
| 跳转按钮 | (258, 9) 86×21，文案由 `agent_jump::Target::action_label()` 决定 | 10.5 / Semibold |
| 分隔线 | (16,37) → (344,37)，0.6pt，白 alpha 34 | — |
| cwd | x=18，y=55，`fit(...,324.0)` 单行（空则跳过） | 9.5 / Regular，白 alpha 62 |
| 工具名 | x=18，y=78，`fit(...,324.0)` | 12 / Semibold |
| 原因 | x=18，y=98 + i×13，`wrap(...,324.0,2)` | 9.5 / Regular，白 alpha 62 |
| 代码块 | (18, code_top) 324×code_height，圆角 8，填充 `rgb(18,18,20)`，描边白 alpha 28；`code_top = 98 + max(reason_lines,1)*13 + 3` | — |
| `$` 提示 | x=28，y=代码块中心 | 11 / Semibold，`rgb(196,126,42)` |
| 操作正文 | x=40，y=code_top+8 + i×14，`wrap(...,300.0,code_lines)` | 9.0 / Regular，白 alpha 62 |
| 「回客户端」 | (18, height-29) 94×21 | 10.5 / Semibold |
| 「拒绝」 | (118, height-29) 72×21 | 10.5 / Semibold |
| 「仅允许这一次」 | (198, height-29) 144×21，允许时才亮蓝 `rgb(39,94,150)` | 10.5 / Semibold |
| `native_scope` 说明 | x=18，y=55，`fit(...,324.0)` 单行 | 11 / Regular |

**溢出/截断点（同一算式在两处打架）**：`detail_size` 按「48 字符/行」预留 reason、按「54 字符/行」预留 operation，而绘制用 `wrap` 按 324/300px 实测换行。三处换算出来的「每字符宽」互不相同：reason 324/48 ≈ 6.75px、operation 300/54 ≈ 5.55px、question 324/30 ≈ 10.8px。而实际汉字在 9.5pt ≈ 9.5px、9.0pt ≈ 9px，拉丁在 9.5pt ≈ 4.7px、9pt ≈ 5.5px —— 所以 reason 常数两头都不准（中文低估行数 → 截断，拉丁高估行数 → 空白），operation 常数只适配拉丁（中文低估约 1.6 倍），question 常数只适配中文。

以「无 reason + 4 行操作」为例：估算 height = 160+0+56 = 216；绘制时 `code_top = 114`，`code_available = (216-29)-114-8 = 65`，`code_lines = floor((65-12)/14) = 3` → 预算 4 行、实画 3 行，最后一行被 `wrap` 加省略号（`paint.rs:155-159`）。

### 2.4 提问详情（360 × [210, 262]，`agent_cards.rs:176-184` / `:203-283`）

高度算式（`agent_cards.rs:178-183`）：

```
question_lines = ceil(chars / 30).clamp(1,3)
detail_lines   = ceil(chars / 36).min(2)
option_step    = if options > 6 { 20.0 } else { 25.0 }
height         = clamp(112 + q_lines*15 + d_lines*14 + min(6,options)*option_step + 56, 210, 262)
```
例：1 行题干 + 0 说明 + 2 选项 → 233；3 行 + 2 行 + 6 选项 → 391 → 被压到 262。

| 元素 | 局部坐标 / 尺寸 | 字号 |
|---|---|---|
| 头部（图标/标题/跳转） | 与审批详情完全相同 | 12 / 10.5 |
| 「第 n / total 轮」 | 右对齐 x=342，y=52 | 10 / Regular |
| 可滚动区 | `body_top=62`，`input_y=height-60`，`body_height=(input_y-62-8).max(30)`；clip `(0,62,360,body_height)`；内容整体 `shifted(0,-question_scroll)` | — |
| 题干首行 | 全局 y=68 起（比 body_top 多 6px），行高 15 | 11 / Semibold |
| 说明 | 题干后 +3，行高 14 | 9.5 / Regular |
| 选项按钮 | (18, y) 324×(option_step-4)；选中填充 `rgb(39,94,150)`，未选 `theme::BTN` | 10 / Regular，文本 x=30，`fit(...,292.0)`；选中「✓」右对齐 x=334 |
| 自定义输入框 | (18, input_y) 324×26，圆角 7，填充 `BTN`，描边白 alpha 26；内部 `TextEdit` 由独立 `egui::Area`（`Order::Foreground`）覆盖 | 10.5 / Regular；hint「自定义回答」 |
| 「上一轮」/「回客户端」 | (18, height-29) 94×21 | 10.5 / Semibold |
| 「下一轮」/「提交回答」 | (248, height-29) 94×21；中间 136px 空白 | 10.5 / Semibold |

**溢出点**：题干 `wrap(...,3)`、说明 `wrap(...,2)`，选项**不截断条数**（最多 50 个，`agent_cards.rs:91`）。20 个选项 × 20px 步进 = 400px 塞进 `body_height ≈ 80px` 的视口，只能靠滚轮，界面无滚动条、无「还有 N 条」提示。

### 2.5 完成通知（`extras.rs:531-546`）

| 形态 | 宽×高 | 元素 |
|---|---|---|
| 轻提示（`expanded=false`） | 334×44 | 图标 (24,22) d26；标题 x=44,y=14（12/Semibold）；正文 x=44,y=30，`fit(...,176.0)`（11/Regular，GRAY）；「查看」(224,12) 50×20；「关闭」(278,12) 50×20 |
| 展开卡（`expanded=true`） | 360×108 | 图标 (24,22) d26；标题 x=44,y=21；右上「本轮已结束」右对齐 x=340,y=21（11）；正文 x=18,y=49，`fit(...,320.0)` 单行；「查看」(166,72) 50×20（仅 `content>0` 且正文≠「本轮已结束」时）；「关闭」(248,72) 50×20 |

正文预处理：`a.body.split_whitespace().take(100).join(" ")`（`extras.rs:539`）→ 把多行回复压成一行再截断，代码块/列表格式全部丢失。

### 2.6 结果查看

**未实现**：`extras.rs:544` 点击「查看」写入 `agent_cards.result`，全仓库没有任何绘制该字段的代码（只有 `app.rs:1879` 用 `.is_some()` 延长停留）。`docs/AGENT-NOTICES.md:23` 承诺的「本轮回复（至多 32,000 字符，可打开滚动阅读）」不存在。

### 2.7 固定状态提醒（`AlertSource::Agent`，legacy/tray 路径）

334×44，`extras.rs:560-573`：图标 (23,22) d26 → 正文左界 44；标题 x=44,y=14 `fit(...,214-44)` 11/Semibold；正文 x=44,y=30 `fit(...,214-44)` 10.5；「关闭」(226,12) 60×20。**没有任何操作按钮**，只能关闭。

---

## 3. 状态机现状

`Request` 字段（`agent_cards.rs:102-112`）：

| 字段 | 类型 | 作用 | 现状 |
|---|---|---|---|
| `id` | String | wire id | ✓ |
| `provider` | `Provider::{Dsh,Codex}` | 来源 | ✓ |
| `question` | bool | approval / question 二选一 | ✓ |
| `data` | `Data` | tool/reason/cwd/operation/questions/text/deferred/nativeScope/sessionId | ✓ |
| `answers` | Vec<Answer> | 每轮 {id, selected, custom} | ✓ 与 questions 等长 |
| `question_index` | usize | 当前轮 | ✓ 绘制时 `min(len-1)` 兜底 |
| `question_scroll` | f32 | 当前轮滚动 | ✓ 仅提问详情用，切轮归零（`:282`） |
| `shown` | Option<f64> | 倒计时起点 | ✓ |
| `details` | bool | 是否展开详情 | ✓ 纯 bool，无过渡 |

`Cards` 字段（`agent_cards.rs:117-127`）：`service`、`retry`、`requests: VecDeque<Request>`、`dsh_ready`、`hovering`、`result`、`jump_error`，测试用 `handed_back`。

| 能力 | 现状 | 依据 |
|---|---|---|
| 队列 | VecDeque，上限 32；deferred 插到「第一个未展开 deferred」之前，否则尾部 | `agent_cards.rs:344`、`:351` |
| 队首展示 | 只有队首可展开（`active()` 取 `front`） | `:129-131` |
| 队列计数 | 仅 compact 卡右上「1 / N」 | `extras.rs:521` |
| 自动交回 | `countdown(t, handoff, hidden)`；`handoff==0` / hover / `details` / `deferred` / 全屏隐藏 → 不计时；到 0 在绘制路径里直接 `hand_back` | `:323-327`、`extras.rs:517-518` |
| 倒计时语义 | **是「重置」不是「暂停」**：暂停时 `shown=None`，恢复后从满秒重算 | `:325` |
| 倒计时驱动 | 没有定时器，只在重绘帧推进；静止时 `repaint_after=1s` → 可能跳秒、交回最多迟 ~1s | `app.rs:2327` |
| 过期（expired） | **未实现**。只有客户端 `cancel` 与来源关闭时的批量回交 | `:330`、`:372-373` |
| submitting / uncertain | **未实现**。`reply()` 直接 `send` 后立即 `remove`，`send` 失败也不处理（`let _=`） | `:133-136` |
| 提交失败反馈 | 无。管道断开时本地已删、客户端仍在等 → 宿主超时 | `:133-136` |
| 交回 | `hand_back` → `{"decision":"return"}` → remove | `:137-141` |
| 跳转 | `jump_target` → `agent_jump::resolve`；失败原因写入 `jump_error`，由 `tick_alerts` 转成岛上 toast | `:142-157`、`:317-319`、`extras.rs:443` |
| 队列满 / 来源关 / 契约不合法 / 重复 id | 立即 `hand_back`（不静默丢弃） | `:336-345`、`:350` |
| 同 id 重发 | 先 remove 再 insert（队首重发会丢失草稿） | `:350-352` |
| 设置 | `Preferences{interactive, handoff∈{0,5..120}, completion, dwell 2..15, expanded, content 0|1|2, deferred, system_toast}` | `:8-27`、`model.rs:233` |
| 键盘 | 无 Enter 提交、无方向键选项、Escape 只走 `cancel_edit`（不含 agent） | `app.rs:1978`、`:1691-1702` |

---

## 4. 动画 / 过渡现状

### 4.1 有缓动

| 项 | 机制 | 位置 |
|---|---|---|
| 岛体宽/高（含详情高度变化） | 二阶弹簧 `Spring`（半隐式欧拉，子步 ≤1/480s） | `anim.rs:8-51`；`app.rs:2896-2944`，`OPEN_W=(330,0.70)`、`OPEN_H=(290,0.74)`、`CLOSE=(620,1.0)`（`app.rs:93-96`） |
| 右缘 badge 出现 | 弹簧 `notice_width` | `app.rs:1895`、`:2554-2557` |
| HUD 透明度 | `anim::approach` 16/8 | `app.rs:1892` |
| 按钮 hover / 按下 | `ctx.animate_bool_with_time` 0.14s / 0.07s + 缩放 0.95 | `paint.rs:272-277`、`:380-396` |
| 未读胶囊出现 | `smoothstep` + 弹簧 | `app.rs:2386-2389` |
| 皮肤换色、拖拽放大、页面切换 | 各自弹簧/缓动 | `app.rs:1856`、`2856-2861`、`anim.rs:53-93` |

### 4.2 硬切（半成品感主要来源）

| 项 | 现象 | 位置 |
|---|---|---|
| 详情开 / 关 | `details`、`notice_details` 都是 bool，翻面即换内容，无淡入淡出、无位移 | `agent_cards.rs:111`、`app.rs:1000`、`extras.rs:489/495` |
| 详情内容入场 | 只有面板高度在弹，内容按最终坐标画、靠 clip 从下往上「挤出来」 | `agent_cards.rs:209/272`、`app.rs:2103-2113` |
| banner 出现 | 岛体弹簧变形，但内容是硬坐标一次性出现 | `app.rs:2126-2127` |
| 完成通知进入 | `tick_alerts` 直接 `self.notice = Some(a)` | `extras.rs:502-507` |
| 完成通知退出 | `t - a.at > duration` 直接 `notice=None`，无淡出 | `extras.rs:471-477` |
| 选项选中态 | 颜色硬切（`rgb(39,94,150)` ↔ `BTN`），无过渡 | `agent_cards.rs:239` |
| 按钮禁用态 | 颜色硬切，无透明度/图标提示 | `agent_cards.rs:279`、`:308` |
| 轮次切换 | `question_index += 1`，硬切；高度若变则靠弹簧追 | `agent_cards.rs:282`、`:274` |
| 滚动 | 滚轮直接改 `question_scroll`，无惯性、无滚动条、无边缘渐隐 | `agent_cards.rs:215-218` |
| 结果查看 | 不存在 | — |

---

## 5. 问题清单（按严重度排序）

> 每条给出「现象 → 位置 → 原因」。严重度：P0 会造成请求悬挂或功能承诺落空；P1 明显半成品感 / 内容丢失；P2 一致性与打磨。

**5.1【P0】完成通知的「查看」是死路，且会把卡片永久钉在屏幕上。**
`extras.rs:544` 点击写入 `agent_cards.result`，但全仓库无一处绘制它；同时 `app.rs:1879-1881` 在 `result.is_some()` 时每帧执行 `a.at += elapsed`，于是「停留时间」永不耗尽，通知再也不会自动消失（`result` 只在来源关闭/reminder 关闭时被清，`agent_cards.rs:374`）。文案「可打开滚动阅读」（`AGENT-NOTICES.md:23`）完全落空。

**5.2【P0】审批详情完全没有滚动，长操作被静默截断。**
`agent_cards.rs:284-313` 只有一处 `wrap(...,code_lines)`，`code_lines` 由剩余高度反算；高度上限 242 且 `operation_lines` 被 clamp 到 4（`:171`）。超过 4 行的 shell/JSON 操作最后一屏被 `fit` 加省略号（`paint.rs:155-159`），没有「查看更多」、没有滚动、没有复制入口。审批场景下用户被迫做「看不见全貌的授权」判断。

**5.3【P0】高度靠「字符数 ÷ 常数」估算，与实际排版不一致，同一算式存在两处互相矛盾。**
`detail_size`（`agent_cards.rs:168-183`）按 48/54/30/36 字符每行预留，绘制用 `wrap` 实测宽度换行（`:220/226/290/295`）。中文与拉丁每字符宽度差近 2 倍，单个常数不可能同时正确：审批 reason 的 48 换算成 6.75px/字符，对拉丁（≈4.7px）偏大、对中文（≈9.5px）偏小；operation 的 54 只适配拉丁；question 的 30 只适配中文（详见 2.3 的换算）。后果是溢出被省略号吞掉（内容丢失）或高度虚高（大片空白）。

**5.4【P1】详情面板没有自己的表面：无底色、无边框、无遮罩、无内边距。**
`draw_details` 直接把文字画在岛体上（`agent_cards.rs:187-321`，没有任何 `fill`/`stroke` 包一层卡面）。仓库里唯一画卡面的路径 `app.rs:2182-2185`（`shape::shadow` + `rect_filled`）因为 `notice_details_rect` 对 AgentCard 提前返回 `None`（`app.rs:2561`）而永不生效。面板形态下 360 宽的内容居中放进 419 宽岛体（`app.rs:2111-2112`），两侧各 29.5px 空条 → 看起来像「一段浮在岛上的散落文字」。

**5.5【P1】同一个详情被两套坐标系绘制，且其中一套其实是死代码。**
`app.rs:2110-2114`（面板内居中 360）与 `extras.rs:512-514`（banner 内左对齐 panel.min）调用同一个 `draw_details`。经控制流核对：`notice_details` 在没有 `open` 时会被每帧强制清零（`app.rs:1897-1902`），而 AgentCard 的 badge 根本画不出来（`agent_card` 使 `compact_notices()` 为 false，`notice_rect` 返回 None，`app.rs:2554`），所以 `extras.rs:512-514` 这条分支**当前不可达**。这既是「双入口不一致」的来源，也是重构时最容易踩空的坑。

**5.6【P1】详情内容是「按最终高度画 + 靠裁剪挤出来」，不跟随弹簧。**
内容坐标写死 `height-29.0`（`agent_cards.rs:272/304`）、`height-60.0`（`:209`），而容器高度是弹簧（`app.rs:1826-1832`、`:2941`），clip 为 `panel.intersect(ui.max_rect())`（`app.rs:2103/2113`）。展开过程中底部按钮被一刀切掉、上滑出现；收起时反向消失，没有任何透明度或位移补偿。

**5.7【P1】自定义回答输入框是浮在最上层的独立 `egui::Area`，绕过所有卡片约束。**
`agent_cards.rs:256-268`：`Area::new(...).order(Foreground).fixed_pos(input_rect.min)`。它不受 `Card` 的 `clipped()`/`faded()`/`ghost()` 影响 → 岛体收起或做透明度过渡时，输入框仍以 100% 不透明悬在面板外；也不受「旧页只画不点」保护。它是多轮问答唯一真正获得 focus 的控件（`PROJECT-HANDOFF.md` §6 第 3 条），所以不能简单删掉，需要换成 `ui.put`/受裁剪的输入框。

**5.8【P1】滚动没有可见反馈，且审批卡没有滚动。**
提问详情靠 `raw_scroll_delta`（`agent_cards.rs:215-218`）改 `question_scroll`，不画滚动条、不画渐隐遮罩、不显示「还有内容」。选项上限 50（`:91`）、20px 步进 → 内容高可达 1000px，视口约 80px（`body_height = input_y-70`），纯靠盲滚。审批卡则完全无滚动（见 5.2）。

**5.9【P1】三套按钮布局互不对齐，产品内没有统一按钮栅格。**
审批 18 / 118 / 198（宽 94/72/144，`agent_cards.rs:304-311`）；提问 18 / 248（宽 94/94，中间 136px 空白，`:274-278`）；完成卡 166 / 248 或 224 / 278（宽 50，`extras.rs:544-545`）。同一产品内出现「左对齐一组 + 右对齐一组」「两个 50px 小按钮 + 一个大按钮」多种规范，视觉重心不稳。

**5.10【P1】完成通知两种形态是两套独立代码，尺寸/锚点全不一致。**
轻提示 334×44（标题 y=14、正文 y=30、按钮 y=12、正文宽 176）与展开卡 360×108（标题 y=21、正文 y=49、按钮 y=72、正文宽 320）没有任何共享布局函数：`extras.rs:533-545` 用 `if wide` 三元散落 6 处。改一处必然漂移；且展开卡右上「本轮已结束」（`:537`）与正文默认值「本轮已结束」（`:430`）重复。

**5.11【P1】队列只有「1 / N」，没有列表、没有切换、没有类型区分。**
`extras.rs:521` 只在 compact 卡上画一行计数；详情页完全不知道还有几条（`agent_cards.rs:187-321`）。第二个请求只能等第一个被处理后才可见；队列满 32 时第 33 条静默回交（`:344-345`），用户只会看到「卡没了」。

**5.12【P1】提交没有「提交中 / 结果未确认」态，失败无反馈。**
`reply()`（`agent_cards.rs:133-136`）`send` 忽略错误后立即 `remove`：管道断了就本地删掉、客户端继续等；重复点击也不防抖（按钮点击后同帧仍会再次命中，靠 `remove` 提前返回规避）。`hud-transport-contract` 里明确的 `submitting/uncertain` 语义（见 `docs/superpowers/research/hud-renderer-semantics.md:91`）未落地。

**5.13【P2】compact 卡正文永远只有一行，题干被截成一句。**
`extras.rs:522` `fit(&a.body,12.0,W::Regular,320.0)`：题干上限 20000 字符（`agent_cards.rs:90`），320px @12pt 约等于 26 个汉字，之后加省略号。用户只能看到「需要你回答」的半个问题，必须先展开才能理解。

**5.14【P2】字号偏小、行距偏紧，中文可读性差。**
详情里 cwd/reason 9.5、代码 9.0、选项与轮次 10（`agent_cards.rs:208/226/241/288/295/301`），而全局正文基准 `size::BODY = 11.5`（`theme.rs:62`）。行高 15/14/13/14（`:222/228/291/301`）对 9.5~11pt 中文只有 1.3~1.4 倍，行间没有段距，长原因读起来是一坨。

**5.15【P2】完全没有入场/退场动画，只有弹簧改尺寸。**
见第 4.2 节。`Card` 已提供 `faded/shifted/scaled`（`paint.rs:44-61`），但 Agent 绘制路径一次都没用（只用了 `clipped`），属于「有原语没用」。

**5.16【P2】自动交回倒计时是重置而非暂停，且可能跳秒/迟到。**
`agent_cards.rs:325` 暂停时 `shown=None`；`extras.rs:523` 文案「{s} 秒后交回」给用户的预期是「暂停」，实际是「重来」。推进依赖重绘（`app.rs:2327` 1s），静止时数字可能 5→3，交回最多迟 1s。

**5.17【P2】同一个语义事件存在两套完全不同的视觉与能力。**
tray/legacy 路径 `receive_agent_notice`（`extras.rs:414-421`）→ `AlertSource::Agent` → 334×44 提示条、只有「关闭」（`extras.rs:560-573`）；channel 路径 → `AlertSource::AgentCard` → 360×108 交互卡（`extras.rs:511-529`）。同一句「等待你审批，请回到客户端」（`agent_notice.rs:52`）在一种形态下能审批、在另一种形态下只能关掉。

**5.18【P2】未读胶囊在详情期间整块消失。**
`messenger_badge_rect` 在 `agent_detail_active()` 时直接返回 `None`（`app.rs:2426-2428`），有测试锁住（`ui_tests.rs:6890`）。详情展开期间看不到任何未读数，收起后突然出现。

**5.19【P2】compact 卡标题不截断，队列计数与标题共用一行。**
`extras.rs:520` 没有 `fit`，标题从 x=48 起、计数右对齐 340；详情页同一串却 `fit(...,206.0)`（`agent_cards.rs:198`）。当前固定文案不会真正溢出，但这是「两处不一致 + 新增来源/长文案即炸」的潜在缺陷。

**5.20【P2】`native_scope` 授权卡语义被压成一行，且缺主操作。**
`agent_cards.rs:286` `fit(...,324.0)` 单行显示「该授权的有效范围由客户端决定，请回客户端确认。」，11pt 中文约 300px 已到极限；这种卡只有「回客户端」「拒绝」两键，没有「仅允许这一次」，右半区大片空白（`:304-313`）。

**5.21【P2】长文本每帧全量排版，成本随题干长度线性甚至更差。**
`fit`（`paint.rs:109-125`）先 `text_w(全文)` 再二分；compact 卡正文可能是 20000 字符题干（`agent_cards.rs:115` 直接把首题全文塞进 `Alert.body`，之后被 `self.notice` / `hud_notice` / `alerts` 多处 clone）。`Alert.body` 没有长度上限，`app.rs:2121/2127` 每帧 `self.notice.clone()`。功能正确但属于热点浪费。

**5.22【P2】Escape / 键盘路径完全未接。**
`app.rs:1978` 在 `is_editing()`（含 `agent_cards.editing()`，`app.rs:1640`）时按 Escape 只调 `cancel_edit`，而 `cancel_edit`（`app.rs:1691-1702`）不碰 agent → 按 Esc 既关不掉详情也不回交，且因为 `is_editing()` 为真还顺带屏蔽了翻页滚动。没有 Enter 提交、没有方向键选选项、没有 focus ring。

---

## 6. 改动影响面：重构为独立 `agent_ui` 模块

### 6.1 建议的模块边界

```
src/agent_ui/
├── mod.rs          对外唯一入口：draw_banner / draw_details / measure / active 状态查询
├── metrics.rs      ★唯一几何来源：所有常量 + measure(&Request) -> Metrics{w,h,rows,code_lines,...}
├── controls.rs     按钮栅格（Primary/Secondary/Danger/Ghost 四型 + 统一 44/72/94/144 宽档）、focus ring、禁用态
├── banner.rs       compact 交互卡（360×108）+ 固定状态提示（334×44）
├── approval.rs     审批详情（含操作滚动区 + 复制入口 + 全屏查看）
├── question.rs     提问详情（多轮 / 选项 / 自定义输入 / 可见滚动条）
├── done.rs         完成通知两种形态 + 结果查看面板（补 5.1）
├── widgets.rs      缺失原语：scroll_area（滚动条+惯性+边缘渐隐）、text_block（带测量缓存）、mask、spinner
└── settings.rs     Preferences::ui（从 agent_cards.rs:28-52 迁出）
```

保留 `src/agent_cards.rs` 只做**状态机 + 协议**（`Service`/`receive`/`reply`/`hand_back`/`countdown`/`update`/`jump_target`），删除其中所有绘制与几何。

### 6.2 必须改动的文件与函数

| 文件 | 函数/位置 | 改法 |
|---|---|---|
| `src/lib.rs` | `:18` 之后 | 注册 `pub mod agent_ui;`（建议 pub，便于 `ui_tests` 直接断言 `measure`） |
| `src/app.rs` | `notice_size` `:1777-1787` | 改为 `agent_ui::measure(a)` |
| `src/app.rs` | `expanded_height` `:1826-1832` | 改为读 `agent_ui::measure(...).h`，保留 `[PH, WIN_H-8]` 夹取 |
| `src/app.rs` | `agent_detail_active` `:1773-1776` | 改为 `agent_ui::detail_active(...)`（把手势/键盘态一起纳入） |
| `src/app.rs` | `frame` `:2106-2130`（banner/agent_details 分支） | 只调 `agent_ui::draw_banner` / `draw_details`，删除内联 `Card` 构造 |
| `src/app.rs` | `frame` `:2131-2158`（badge） | 若保留 badge，需先让它对 AgentCard 可达（当前不可达），否则删掉死分支 |
| `src/app.rs` | `frame` `:2178-2203`（notice_details_rect） | AgentCard 的早退（`:2561`）改为走 `agent_ui::draw_details` |
| `src/app.rs` | `frame` `:2204-2212`（点外关闭） | 与 6.1 的 `detail_active` 统一，补 Esc / 键盘关闭 |
| `src/app.rs` | `hit_rect_base` `:2368-2383`、`hit_rect` `:2362` | banner 命中区要与新宽度/高度同源（当前 `(w+8,h+6)` 硬编码） |
| `src/app.rs` | `messenger_badge_rect` `:2426-2428` | 保留「详情时不画未读」契约，但改为读统一状态 |
| `src/app.rs` | `open_notice_details` `:2568-2576`、`notice_details_active` `:2577` | 收进 `agent_ui`，供键盘/Esc 复用 |
| `src/app.rs` | `is_editing` `:1640` | 改为 `agent_ui::wants_keys(...)`，避免「问答态屏蔽全页滚动」的副作用 |
| `src/extras.rs` | `draw_notice` `:509-546`、`:560-573` | AgentCard / AgentDone / Agent 三个分支整体委托给 `agent_ui`；删除 `:512-514` 死分支 |
| `src/extras.rs` | `tick_alerts` `:478-501` | 「自动展开」策略搬进 `agent_ui::on_active_changed`，保留行为但去掉对 `self.open` 的直接写入 |
| `src/extras.rs` | `receive_agent_notice` `:414-421`、`agent_finished` `:423-433` | 合并为单一入口，消除 5.17 的双形态 |
| `src/extras.rs` | `notice_button` `:1011-1015`、`agent_icon` `:101-107` | 下沉到 `agent_ui::controls` / `agent_ui::widgets`（`agent_icon` 其他 notice 仍在用，保留 re-export） |
| `src/extras.rs` | `AlertSource` `:9-10`、`allowed` `:446-452` | `Agent` 与 `AgentCard` 可考虑合并，但**必须同步 `ui_tests` 与 `tick_alerts` 的 retain 逻辑** |
| `src/agent_cards.rs` | `detail_size` `:161-185`、`draw_details` `:187-321` | 迁出；`detail_size` 保留为 `measure(...)` 的兼容壳（有测试直接用） |
| `src/agent_cards.rs` | `Preferences::ui` `:28-52` | 迁到 `agent_ui/settings.rs` |
| `src/model.rs` | `:233`、`:379`、`:466` | 类型路径改为 `crate::agent_ui::Preferences`；**serde key 必须仍是 `"agent_cards"`、字段名不变**（配置向后兼容） |
| `src/main.rs` | `:104`、`:248` | 不需要改（入口保持） |

### 6.3 会被影响的测试（明确列出断言关键点）

**`src/ui_tests.rs`（Agent 相关 6 个）**

| 测试 | 行 | 锁定的内容（改 UI 必须保持） |
|---|---|---|
| `agent_notice_respects_each_source_and_master_without_windows_notification_access` | 6777 | `alerts.len()==1`；`notice.body == Status::Finished.label()`；`notice.title == "DeepSeek Harness"`；完全关闭时 `notice/alerts/history` 皆空；不发 `Cmd::Notify` |
| `agent_notice_interactive_queue_outlives_toasts_and_releases_on_disconnect` | 6804 | `notice.source` 为 `AgentCard(_, "first")`；`compact_notices()==false`；`count()==2`；**审批到达自动 `details_open`**；详情开时 `countdown(...)==None`；`cancel` 后自动切到 "second" 且再次自动展开；`hand_back` 后回到完成通知（body `"test completion"`）；`dwell=12` 的过期时刻精确（t+=11 还在、+2 消失） |
| `agent_notice_card_controls_are_visible_and_clickable_in_lyric_mode` | 6831 | 自动展开后**不许**出现文本「查看并审批」；必须出现「拒绝」「仅允许这一次」「回客户端」；`click_text("回客户端")` 后 `count()==0`。注意 `click_text` 要求文字 `opacity_factor>0.9` 且 `clip_rect` 完整包含字形中心（`ui_tests.rs:1252`）→ 任何淡入/半透明/裁切都会让它找不到文字 |
| `agent_detail_expansion_does_not_draw_the_unread_capsule_outside_the_panel` | 6859 | 有未读 vs 无未读两种情况下 `shape_right` 差 <5px（`ui_tests.rs:6890`） |
| `agent_notice_question_is_answered_inside_the_island` | 6901 | 文本「需要选择哪个方案？」「方案 A」「方案 B」「提交回答」「回客户端」必须同屏可见；点「方案 A」→「提交回答」后 `count()==0` |
| `agent_notice_questions_page_one_at_a_time_supports_back_and_custom_input` | 6917 | 第 1 轮可见、第 2 轮不可见；hint 文本「自定义回答」可点击（说明输入框 hint 必须仍是可点文字）；「下一轮」→ 第 2 轮可见、第 1 轮不可见；「上一轮」返回 |

**`src/agent_cards.rs::tests`（15 个，`:381-528`）**

`unknown_provider_is_handed_back...`(394)、`missing_provider_is_handed_back`(403)、`missing_kind_is_handed_back`(411)、`known_provider_approval_is_queued_without_hand_back`(419)、`too_many_options_are_handed_back`(434)、`option_limit_boundary_is_accepted`(442)、`oversized_question_text_is_handed_back`(450)、`duplicate_option_labels_are_handed_back`(458)、`duplicate_question_ids_are_handed_back`(466)、`blank_option_label_is_handed_back`(475)、`duplicate_request_id_replaces_existing_entry`(483)、`jump_target_reads_session_and_cwd`(492)、`jump_target_for_dsh_points_at_its_client`(505)、`long_operation_keeps_detail_card_within_its_width`(515，**直接断言 `detail_size` 宽 360、高 ∈ (0, WIN_H-8]**)。
→ 若把 `detail_size` 改名，这 1 个测试必然要改；其余 14 个只依赖状态机，迁 UI 不应触碰。

**`src/agent_jump.rs::tests`（7 个，`:163-210`）**：深链编码、目录回落、`Unavailable` 不可跳、超长 id 回落、空白 id trim —— 纯逻辑，UI 重构不应影响。

**`src/agent_notice.rs::tests`（8 个，`:203-329`）**：`SITES`/`SIGNALS` 顺序、wire_id/decode、设置开关往返 —— **`SIGNALS` 只能追加不能重排**，UI 改动不得触碰。

**像素基线测试（间接）**：`compact_canvas_preserves_panel_and_notice_pixels`(6740)、`compact_font_atlas_preserves_panel_and_notice_pixels`(6995) 按 `(宽, 页, notice)` 对比像素，虽不含 Agent 卡，但任何对 `WIN_W/WIN_H/CANVAS_W`、`paint.rs` 文字度量的改动都会连带它们。

### 6.4 必须保持不变的契约

1. **多轮问答语义**（`PROJECT-HANDOFF.md:239-263` 第 1–12 条）：逐轮展示、当前轮可滚动、自定义输入必须真获 focus、非多选互斥、下一轮前置校验、最后一轮全答才可提交、第 1 轮左键是「回客户端」而第 2 轮起是「上一轮」、切轮重置滚动、**关闭详情 ≠ 交回**。
2. **交回语义**：只有显式「回客户端」与倒计时归零发 `{"decision":"return"}`；关闭/收起详情不发任何 reply（`agent_cards.rs:137-141`、`:145-147`）。
3. **隐私边界**：请求/回答/回复只在内存；`enqueue_alert` 只把 `AlertSource::Internal` 写入 `history`（`extras.rs:299`）；系统通知只发「本轮已结束」不帶正文（`extras.rs:427-429`）；`docs/PRIVACY.md`。
4. **窗口尺寸上限**：`detail_size` 高度必须 `≤ WIN_H-8 = 262`；宽度基准 360（`agent_cards.rs:162/174/184`）；不能靠加宽到 419/更大来「解决拥挤」。
5. **队列契约**：上限 32；来源/契约不合法立即交回而非丢弃；同 id 重发只保留一条（`agent_cards.rs:336-352`）。
6. **`Alert` 契约**：`AlertSource::{AgentCard(provider,id)}` 的 id 必须始终能在队列里查到（`tick_alerts` 的 `allowed` 依赖它），否则下一次 `retain` 会静默清掉卡片（`extras.rs:450`）。
7. **设置持久化**：`settings.agent` 的 serde key 是 `"agent_cards"`、字段名与取值域（`handoff∈{0,5,10,15,30,60,120}`、`dwell 2..=15`、`content 0..=2`）不可改（`agent_cards.rs:24-27`、`model.rs:379/466`）。
8. **不新增网络端口**（`AGENT-NOTICES.md:32`）；跳转只用 ShellExecuteW / SetForegroundWindow（`agent_jump.rs:67-108`）。
9. **完成通知的「查看」条件**：`content>0` 且正文不是「本轮已结束」（`extras.rs:543`）。

---

## 7. 可用的绘制原语清单

### 7.1 `Card`（`paint.rs:18-278`）能做什么

| 能力 | API | 备注 |
|---|---|---|
| 坐标/矩形 | `pos` `rect` `to_local` | 全部「面板局部逻辑点」，`s` 缩放系数 |
| 派生画布 | `faded(alpha)` `shifted(dx,dy)` `scaled(cx,cy,k)` `ghost()` `clipped(x,y,w,h)` | 已够做入场淡入/位移/挤压/「只画不点」 |
| 文字 | `font` `galley` `text_w` `text`（基线） `centered_text`（可见中心） `fit`（省略号） `wrap`（按宽折行 + 末行省略） | 已内建 RTL/维语/藏文处理（`paint.rs:78-82`） |
| 图形 | `fill` `stroke` `circle` `ring` `seg` `path` `poly` `arc` `glow` `vgrad` `dashed` | 足够画进度条、滚动条轨道、渐变遮罩、分段进度 |
| 交互 | `hit(x,y,w,h,id)` `hit_drag` `hover_t(&Response) -> (hover,press)` | `ghost()` 状态下自动降级为 `Sense::hover()`（`:262`） |
| 圆角按钮 | `paint::capsule(c,x,y,w,h,id,base,hover) -> (Response, Card)` | 悬停提亮 + 按压缩放 0.95，是现有最成熟的交互原语 |

### 7.2 `Card` 缺什么（重构要补的原语）

1. **滚动容器**：没有「视口 + 内容高 + 滚动条 + 惯性」的抽象；现有代码手搓 `clip + shifted + raw_scroll_delta`（`agent_cards.rs:212-218`）且不画滚动条。
2. **文字选择 / 复制 / 富文本块**：代码块、回复正文没有选择和复制；也没有等宽字体族（`theme.rs:196-220` 只装 Proportional / semibold / bold）。
3. **阴影 / 卡面**：`Card` 里没有「抬起的卡片」原语。`shape::shadow` 是 `app.rs` 层面直接对 `painter` 操作（`app.rs:2182`），想给详情画卡面必须从 app 侧传 painter 或用 `Card::painter`（字段是 pub，`paint.rs:20`）。
4. **布局系统**：没有 vbox/hbox/spacer/等宽按钮栅格；一切靠手写 y 偏移（这也是 5.9 的根因）。
5. **控件状态**：没有 disabled/hover/pressed/focus 的统一状态机；`notice_button` 只接受固定底色（`extras.rs:1011`），禁用态是调用方自己换颜色（`agent_cards.rs:279/308`）。
6. **测量缓存**：`text_w` 每次重新 `layout_no_wrap`；`fit` 对超长文本先全量排版再二分。多轮/长题干场景需要「一次测量、多处复用」。
7. **每控件动画状态**：仓库有 `ctx.data`/`animate_bool`，但 Agent 绘制没用；`hover_t` 之外没有「值变化缓动」的通用封装（`anim::approach` 只在 app 侧用）。
8. **遮罩 / 毛玻璃层**：详情想做成独立浮层时需要 backdrop 模糊/暗化，现有只有 `glass.rs` 的整岛取样与 `shape::shadow`。

### 7.3 现有可复用组件（重构直接搬）

| 组件 | 位置 | 可复用点 |
|---|---|---|
| `paint::capsule` | `paint.rs:380-396` | 所有按钮的统一底座（hover/press/圆角） |
| `extras::notice_button` | `extras.rs:1011-1015` | 20 高 / 10.5 号 / 灰底的小按钮，可参数化成按钮型别 |
| `extras::agent_icon` | `extras.rs:101-107` | 来源图标（`Provider::icon()` 图片，`load_for_size` 就绪才画） |
| `extras::notice_badge` / `notice_icon` | `extras.rs:94-108` | 非 Agent 的系统图标兜底 |
| `paint::bell` / `bell_swing` | `paint.rs:589-613` | 提醒铃铛 + 摆动 |
| `theme::{W, font, size}` | `theme.rs:40-65` | 字重/字号档位；建议新增 `size::AGENT_BODY=12.0` 之类语义档 |
| `anim::{Spring, approach, smoothstep, ease_out_cubic, ease_in_out, lerp, remap}` | `anim.rs:8-93` | 入场缓动、高度弹簧、滚动惯性 |
| `anim::Spring` | `anim.rs:8-51` | 帧率无关、可打断，适合「每行高度」「滚动位置」的新弹簧 |
| `agent_jump::Target` | `agent_jump.rs:11-36` | 跳转按钮文案与可用性判定（`action_label`/`available`） |
| `paint::alpha` / `paint::mix` | `paint.rs:280-288` | 半透明正文色（现在用 `from_white_alpha` 硬编码） |

---

## 8. 一句话总结

现有 Agent UI 的**状态机与协议是完整的、隐私与交回语义是对的**；欠的是**表现层**：没有统一度量（字符数除法估算）、没有滚动、没有卡面/遮罩、没有入场退场动画、按钮与字号三套并存、完成通知的「查看」和「结果阅读」是断头路、键盘与提交态完全空白。把这些收进一个 `agent_ui` 模块、把几何收成一份 `measure()`，就能在不碰协议与测试契约的前提下把「半成品」变成产品。
