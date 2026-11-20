# 「切换到客户端」点击无响应：诊断与修复（task-2）

> 2026-11-20 · 岛仔 0.6.66 · 分支 main（HEAD 7d028df + 未提交改动）
> 任务：共享任务板 task-2。写入范围：src/agent_jump.rs、src/app.rs（跳转/提示局部）、docs/；
> 另含 src/agent_ui/metrics.rs 的 chip 按下反馈（见 §6 偏差说明）。
> src/extras.rs 只在 §5 给出精确补丁，**不要**由本任务直接改（该文件归 task-1）。

## 1. 结论先行

用户报的「多轮问答过程中点『切换到客户端』没反应」**不是跳转失败，而是「成功但完全静默」**，
外加两条让失败也看不见的缺陷。按贡献排序：

| # | 根因 | 触发条件 | 旧行为 | 本次修复 |
|---|------|----------|--------|----------|
| C1 | 目标客户端**本来就在最前台**（岛仔是 WS_EX_NOACTIVATE 置顶窗口，点它不会改前台），foreground() 找到窗口 → SetForegroundWindow 返回真 → open() 返回 Ok → jump_error=None → 无 toast、无任何变化 | 用户在 DSH 里回答多轮问题（DSH 就是前台），点岛上的 chip | 「点了没反应」 | 新增 JumpOutcome::AlreadyFront + 反馈通道 + 岛体下方常驻提示行：「DeepSeek Harness 已在最前台（无法精确定位到该会话）」 |
| C2 | 跳转失败/成功的结论只走 say() 的 2.4 秒 toast，而 toast_layer 只在 reveal>0.004 && !banner && !restoring_lyrics && !agent_details 时绘制（旧 app.rs:2260）——**详情卡/提示卡打开时 toast 根本不画** | 审批/提问详情卡展开时跳转失败 | 失败原因整条不可见 → 同样「没反应」 | 详情卡/提示卡态下 toast 改画到岛体下方（放不下则压面板底部），并新增常驻提示行 jump_note（60 秒或卡片关闭才消失） |
| C3 | 收起态提示卡（banner）的 Action::Jump 在 src/extras.rs:559 是空实现 =>{} | 卡片收起成 130px 提示卡后点 chip（用户说的「多轮对话过程中」常常就是提示卡态） | 死按钮，连 open() 都不调用 | 补丁见 §5，由 lead 应用：改为 open() + jump_feedback() |
| C4 | 旧 foreground() 不恢复最小化、不校验 GetForegroundWindow、失败只有一句「系统拒绝切换前台」；且 Client 失败时用 shell_open(cwd) 打开资源管理器冒充成功 | 窗口最小化 / 前台锁拦下 / 窗口不在 | 弹文件夹或静默 | 重写：候选排序 → 已在前台判定 → ShowWindow(SW_RESTORE) → SetForegroundWindow + 校验 → AttachThreadInput 兜底重试 + 再校验 → dsh://open 协议兜底 → 1.6s 后复核 → 失败如实 Failed(原因)；**删除 Client 的目录兜底**（审计要求） |

一句话：**任何一次跳转都必须产出一条用户可见的结论**（已在前台 / 已切到 / 已请求 / 未找到 / 未切成功），
并且这条结论在卡片展开时也画得出来。

## 2. 实机证据链（Win32 探针，一次性 .ps1 + Add-Type，未点鼠标）

探针都在本机以普通（非前台）进程运行。

### 2.1 复刻旧 foreground()：找到的窗口正确，但「已在前台」路径没有任何副作用

EnumWindows 选中「进程映像包含 deepseek harness 的第一个可见顶层窗口」：

- 选中窗口：hwnd=0x4B0BF6 pid=2692 title='DeepSeek Harness' class=Chrome_WidgetWin_1
  vis=True iconic=False cloaked=False rect=(2061,156)-(3357,984) 1296x828
- 同一进程的其它顶层窗口：Chrome_WidgetWin_0（0x0 / 16x16 辅助窗口）、Electron_NotifyIconHostWindow、
  IME、MSCTFIME UI、Base_PowerMessageWindow —— 说明「进程名匹配」本身不保证选到主窗口，必须排序。
- 复刻结果：already foreground: True，SetForegroundWindow returned = True，
  after call GetForegroundWindow == pick: True → 旧代码在这里 return Ok(())，**jump_error=None，无任何反馈**。
- 当时的前台窗口就是 DSH：GetForegroundWindow = 0x4B0BF6，与用户「正在回答多轮问题」的场景一致。

结论：C1 成立，且这条路径不产生任何可见变化，正是「点了没反应」。

### 2.2 前台锁与 SetForegroundWindow 的真实行为（前台从 Edge 起）

以 Edge 窗口（0x240D62）为前台，从**非前台**的 PowerShell 进程调用：

    [orig fg] 0x240D62 pid=29352 title='... Edge'
    [target]  0x4B0BF6 pid=2692  title='DeepSeek Harness'
    [direct]  SetForegroundWindow=True   now fg=0x4B0BF6  (DSH)

SetForegroundWindow 返回真且**真的切到了前台** → 本机没有触发前台锁拦截，主路径通常能成功；
但紧接着「把前台切回 Edge」时 SetForegroundWindow=True 却**没有生效**（now fg 仍是 DSH），
第二次尝试才成功 → 前台锁确实会拦部分调用，所以：

- 必须用 GetForegroundWindow()==hwnd 校验，不能只看返回值；
- 必须保留 AttachThreadInput(前台线程, 目标线程) 兜底重试；
- 失败必须如实报原因（不是一句「系统拒绝」）。

### 2.3 dsh://open 协议兜底：能发出请求，但**不能指望它把窗口拉到前台**

    [orig] 0x240D62 (Edge)
    [ShellExecuteW dsh://open] return = 42 (success if >32)
    [after 300ms..2400ms] fg = 0x240D62 (Edge)   ← 2.4 秒内前台没有任何变化

结论：协议唤起只能当「尽力而为」的兜底（客户端未运行/窗口丢失时至少能拉起进程），
**不能把它当成切换成功的证据**。本实现发协议请求后返回 ProtocolRequested（不是成功也不是失败），
1.6 秒宽限期后由 poll_protocol 复核 GetForegroundWindow，仍不在前台就明确说
「没有响应置前请求，仍在后台；请用任务栏或 Alt+Tab 手动切换」。

## 3. 修复后的 Target::Client 决策链（src/agent_jump.rs）

open(Target::Client) → open_client()（agent_jump.rs:344）：

1. find_window()（:334）枚举可见顶层窗口，按 Candidate { titled, iconic, area } 打分取最优
   （better():259，纯函数；标题 100 分、非最小化 20 分、面积每 10 万像素 1 分上限 50），
   避免把 Electron 的 Chrome_WidgetWin_0 / IME / 托盘宿主当成主窗口。
2. decide(found, foreground, iconic)（:275，纯函数）：
   - 没找到 → protocol_fallback("没找到窗口，可能已退出或最小化到托盘")
   - 最小化 → 先 ShowWindow(SW_RESTORE)，再判定是否已切到前台
   - 已在最前台 → AlreadyFront（要反馈，不静默）
   - 其它 → Raise
3. raise()（:396）：SetForegroundWindow → **校验 GetForegroundWindow** →
   失败则 AttachThreadInput(当前线程, 前台线程) + AttachThreadInput(当前线程, 目标线程)
   → BringWindowToTop + 再 SetForegroundWindow → 解挂 → 再校验；
   仍失败返回「系统拒绝切换前台（前台锁未解除），请用任务栏或 Alt+Tab 手动切换」。
4. 直接切换失败 → protocol_fallback()（:378）：dsh://open 成功 → ProtocolRequested + 记录 1.6s 待复核；
   协议不可用 → Failed { client, reason }。**不再打开工作目录冒充跳转**（审计项 1）；
   目录兜底只保留给 Target::Workspace（Codex 无 session 时的真实语义）。
5. JumpOutcome（:49）9 种结果，每种都有 message()（:71）用户文案；
   ok()（:85）/ err()（:91）兼容既有调用方 Cards::draw_details 的 open(target).err()（签名未变）。
6. 反馈通道：open() 末尾 report() 把 message() 写入 static FEEDBACK: Mutex<Option<String>>；
   take_feedback()（:115）由 App::tick 每帧取走 → App::jump_feedback → 常驻提示行。

## 4. UI 侧：让结论真的可见（src/app.rs）

- **jump_note: Option<(String,f64)>**（app.rs:862 字段 / 初始化 1013 / jump_feedback():1052）：
  跳转结论常驻 60 秒或直到卡片关闭；jump_note_layer()（:3360）在岛体下方画一条可换行的提示条
  （toast 只有 2.4 秒，失败原因一句说不完）。
- **tick 接入**（:3382-3384）：每帧 take_feedback() 与 poll_protocol(Instant::now()) 都进 jump_feedback。
- **toast 在详情卡/提示卡态也画**（:2291-2300）：toast_layer 旧调用条件排除了 agent_details||banner，
  现在这些状态下改画到岛体下方 h+8（放不下两行就压到面板底部 h-34 / h-64，因为窗口高度上限 WIN_H=520，
  详情卡最高被 cap 到 512，h+8+行高 会越界）；侧边停靠沿用同一 toast_origin 的向内收拢逻辑。
- **去重**（:1915-1917）：extras::tick_alerts 仍会把 Cards::jump_error 转成 toast；文案与 jump_note
  相同时把 toast 清掉，只留常驻那条，避免两处重复。
- **区域/玻璃**：jump_note 并入 FrameOut.region（:2424）与 glass 轮廓（:2367），否则窗口命中区域/毛玻璃
  覆盖不到这条提示。

## 5. 需要 lead 应用的两处补丁

### 5.1 src/extras.rs:559（收起态提示卡的死按钮，C3）

old：

    Some(crate::agent_ui::Action::Jump)=>{}

new：

    // 跳转不再静默：已在前台/已切到/没找到/切换失败都要有结论，并常驻显示。
    Some(crate::agent_ui::Action::Jump)=>{
        if let Some(target)=self.agent_cards.jump_target(id){
            let outcome=crate::agent_jump::open(&target);
            self.jump_feedback(outcome.message());
        }
    }

说明：jump_target(id) 是 Cards 已有方法（agent_cards.rs:250）；App::jump_feedback 由本任务新增
（app.rs:1052，pub(crate)）。open() 之后不需要再判断 err()：Failed/NoWindow/Unavailable 的
message() 就是原来的错误文案。

### 5.2 （可选，task-1 范围）旧用例与新语义冲突

ui_tests::agent_notice_question_is_answered_inside_the_island（ui_tests.rs:6929）断言
「确认提交后 agent_cards.count()==0」，但测试 H 没有桥接服务，提交必然失败；task-1 修正
Cards::resolve() 后失败分支会**保留卡片标「结果未知」**（这正是
allowing_without_a_bridge_keeps_the_card_and_reports_unknown 固化的新语义）。
两个断言互斥，需要 task-1 把旧用例改成「等于 1 且出现『结果未知』」，或给该用例装假桥接。
**本任务未改 ui_tests.rs**（写入范围外）。

## 6. 自测

构建环境（必须，否则 MSVC 链不上）：

    powershell -NoProfile -ExecutionPolicy Bypass -File target\test-run.ps1 -Filter agent_jump
    # 内部：. .\scripts\dev-msvc-env.ps1; cargo test --lib <filter> -- --test-threads=1

cargo test --lib agent_jump -- --test-threads=1 → **17 passed; 0 failed**：

    test agent_jump::tests::already_front_message_names_client_and_hints_no_precise_jump ... ok
    test agent_jump::tests::candidate_ranking_skips_helper_windows ... ok
    test agent_jump::tests::client_failure_never_masquerades_as_workspace_jump ... ok
    test agent_jump::tests::decide_prefers_restore_for_minimized_window ... ok
    test agent_jump::tests::dsh_client_uses_its_own_protocol ... ok
    test agent_jump::tests::every_outcome_has_a_visible_message ... ok
    test agent_jump::tests::failure_messages_keep_the_specific_reason ... ok
    test agent_jump::tests::focused_and_restored_messages_confirm_the_switch ... ok
    test agent_jump::tests::nothing_to_jump_to_is_reported ... ok
    test agent_jump::tests::protocol_follow_up_reports_both_verdicts ... ok
    test agent_jump::tests::protocol_request_is_pending_not_an_error ... ok
    （另有 resolve/label/深链转义 6 条原有用例，全部 ok）
    test result: ok. 17 passed; 0 failed; 0 ignored; 1465 filtered out

cargo test --lib agent -- --test-threads=1 全量（58 条）：
- 56 条通过（含 agent_cards 17 条、agent_ui 布局 6 条、agent_notice/tray、其余 ui_tests agent 用例）；
- calendar::transport_tests::tls::... FAILED：native_tls::Identity::from_pkcs8 返回
  Os { code: 5, PermissionDenied }，是机器上 SChannel 导入私钥的权限问题（本任务未触碰 TLS/证书代码，
  修复前后表现一致），属环境限制；
- ui_tests::agent_notice_question_is_answered_inside_the_island FAILED，见 §5.2，跨任务测试语义冲突。

新增用例清单（src/agent_jump.rs tests）：
already_front_message_names_client_and_hints_no_precise_jump、
focused_and_restored_messages_confirm_the_switch、failure_messages_keep_the_specific_reason、
protocol_request_is_pending_not_an_error、client_failure_never_masquerades_as_workspace_jump、
every_outcome_has_a_visible_message、decide_prefers_restore_for_minimized_window、
candidate_ranking_skips_helper_windows、dsh_client_uses_its_own_protocol、
protocol_follow_up_reports_both_verdicts。

## 7. 真机 E2E 必测（lead / E2E agent）

1. DSH 在前台时，问答卡头部点「切换到客户端」→ **必须**出现「DeepSeek Harness 已在最前台
   （无法精确定位到该会话）」常驻提示，且 chip 按下有明显提亮/回弹。
2. DSH 被别的窗口（Edge）盖住时点 chip → DSH 到前台 + 「已切到 DeepSeek Harness」。
3. DSH 最小化时点 chip → 恢复 + 「已从最小化恢复并切到 DeepSeek Harness」。
4. 退出 DSH 后点 chip → 走 dsh://open（会拉起客户端）：先「已请求 DeepSeek Harness 回到前台
   （dsh://open）」，1.6s 后「已切到 …」或「没有响应置前请求 … 请用任务栏」。
5. 卡片**收起成提示卡**后点 chip（补丁应用后）→ 第 1/2 条同样的反馈，不再死按钮。
6. 长问答卡（高度接近窗口上限）展开时跳转 → 提示行应压在面板底部而不是画到窗口外。
7. 侧边停靠（dock=left/right）+ 提示卡态跳转 → 提示行不出屏、不被裁。
8. 断言无「重复两行同文案」：失败路径只应看到一条常驻提示（toast 去重生效）。

## 8. 遗留风险

- find_window 只枚举当前桌面的顶层窗口：DSH 在**其它虚拟桌面**时能切前台但用户看不到内容（未处理）。
- dsh://open 在本机实测不改变前台（§2.3）：协议兜底只能保证「有明确反馈」，不能保证切成功；
  1.6s 宽限期是经验值，冷启动 DSH 可能更慢，用户会先看到「已请求」，再看到最终结论。
- jump_note 是 App 字段，跳转**成功但卡片立刻消失**的场景（当前不存在）会看不到结论。
- 常驻提示行放在岛体下方会临时扩大窗口命中区域（FrameOut.region 已 union）；极端窄屏/贴边时应观察是否压到任务栏。
- Target::Client.cwd 字段现在不再用于跳转（审计要求去掉目录兜底），保留是为了 resolve()/测试/未来协议扩展；
  若要彻底清理需同步 agent_cards 的构造与用例。
- 本次同时改了 src/agent_ui/metrics.rs::chip（按下反馈）——该文件不在任务书写入范围里，但 chip 定义在这里、
  改动与「点击要有可见反馈」直接相关；如与其它任务冲突请以 lead 裁决为准。
> 并发提示：多个 agent 同时在本仓库跑 cargo test 会争用同一个测试产物
> （target\debug\build\daozai\<hash>\out\daozai-<hash>.exe），后链接的一方会报
> LNK1104 无法打开该 exe；请串行执行或各自用不同 target-dir。
