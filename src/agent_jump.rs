//! 会话跳转：把用户从岛上的卡片带回原客户端。
//!
//! 分级降级（对照 island-hud 的 jump-target 顺序，去掉依赖进程探测的不可靠档位）：
//! Codex 线程深链 > 客户端窗口前置 > 客户端协议唤起 > 如实报错。
//!
//! 注意：Client 失败时不回落到「打开工作目录」——按钮写着「切换到客户端」，却弹出资源管理器，
//! 是用错误的兜底骗过调用方；目录兜底只留给 Target::Workspace。
//!
//! 只使用本机调用（ShellExecute / SetForegroundWindow），不新开任何网络端口。
//!
//! 反馈约定（对照 island-hud 的 focusTerminal）：一次跳转必须有一条明确的用户可见结论——
//! 已在前台、切换成功、找不到窗口、切换失败都要说清楚。岛仔窗口带 WS_EX_NOACTIVATE，
//! 点击岛内按钮不会改变前台窗口，所以「目标本来就在前台」这条路径以前是完全静默的，
//! 用户看到的就是「点了没反应」；这里通过 take_feedback 把它交给上层用 toast 展示。

use crate::agent_notice::Provider;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Codex 线程深链，交给系统协议处理器打开。
    DeepLink(String),
    /// 把客户端主窗口带到前台；找不到窗口时用客户端自己的协议唤起，仍失败就如实报错。
    Client { name: &'static str, cwd: String },
    /// 打开工作目录。
    Workspace(String),
    /// 没有可用的跳转信息。
    Unavailable(&'static str),
}

impl Target {
    /// 按钮文案必须说明真正会发生什么，不能让用户以为一定能回到原会话。
    pub fn action_label(&self) -> &'static str {
        match self {
            Self::DeepLink(_) => "回到会话",
            Self::Client { .. } => "切换到客户端",
            Self::Workspace(_) => "打开工作目录",
            Self::Unavailable(_) => "无可用跳转",
        }
    }

    pub fn available(&self) -> bool {
        !matches!(self, Self::Unavailable(_))
    }
}

/// 一次跳转的结果。每种结果都必须能翻译成一句用户看得懂的话，不允许「静默成功」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JumpOutcome {
    /// 目标客户端本来就在最前台：没有任何可见变化，但必须告知用户。
    AlreadyFront { client: String },
    /// 从最小化恢复并切到前台。
    Restored { client: String },
    /// 窗口原本在后台，已切到前台。
    Focused { client: String },
    /// 客户端窗口没找到/切不过去，已用客户端自己的协议（如 dsh://open）请求置前，
    /// 结果由 poll_protocol 在宽限期后再确认。
    ProtocolRequested { client: String },
    /// 深链/目录这类由系统打开成功。
    Opened { note: String },
    /// 找不到客户端窗口，也没有可兜底的目录。
    NoWindow { client: String },
    /// 找到了窗口但系统拒绝切到前台。
    Failed { client: String, reason: String },
    /// 目标本身没有可用的跳转信息。
    Unavailable(String),
}

impl JumpOutcome {
    /// 用户可见的一句话反馈。任何结果都不能返回空串。
    pub fn message(&self) -> String {
        match self {
            Self::AlreadyFront { client } => format!("{client} 已在最前台（无法精确定位到该会话）"),
            Self::Restored { client } => format!("已从最小化恢复并切到 {client}"),
            Self::Focused { client } => format!("已切到 {client}"),
            Self::ProtocolRequested { client } => format!("已请求 {client} 回到前台（dsh://open）"),
            Self::Opened { note } => note.clone(),
            Self::NoWindow { client } => format!("没找到 {client} 的窗口，可能已退出或最小化到托盘"),
            Self::Failed { client, reason } => format!("没能切到 {client}：{reason}"),
            Self::Unavailable(reason) => reason.clone(),
        }
    }

    /// 是否算「用户已经得到他想要的结果」。失败档位需要额外提示。
    pub fn ok(&self) -> bool {
        !matches!(self, Self::NoWindow { .. } | Self::Failed { .. } | Self::Unavailable(_))
    }

    /// 兼容既有调用方（Cards::draw_details 取失败原因存 jump_error）：
    /// 失败时返回给用户看的原因，成功返回 None。
    pub fn err(self) -> Option<String> {
        if self.ok() {
            None
        } else {
            Some(self.message())
        }
    }
}

/// 最近一次跳转的反馈（成功也算）。上层每帧取走一次用 toast 展示。
static FEEDBACK: Mutex<Option<String>> = Mutex::new(None);

/// 协议兜底的宽限期：Electron 收到 open-url 后再 restore + focus 大约 1-2 秒。
const PROTOCOL_GRACE: std::time::Duration = std::time::Duration::from_millis(1600);

struct Pending {
    at: std::time::Instant,
    client: String,
}

/// 协议兜底后待复核的置前请求。
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// 取走一次跳转反馈（成功也要取），交给 App::say 用 toast 显示。
pub(crate) fn take_feedback() -> Option<String> {
    FEEDBACK.lock().ok().and_then(|mut slot| slot.take())
}

/// 协议兜底宽限期到了以后核对一次前台：返回要展示的结论；未到期/没有待复核返回 None。
pub(crate) fn poll_protocol(now: std::time::Instant) -> Option<String> {
    let pending = {
        let mut slot = PENDING.lock().ok()?;
        match slot.as_ref() {
            Some(p) if now.duration_since(p.at) >= PROTOCOL_GRACE => slot.take(),
            _ => None,
        }
    }?;
    let reached = find_window(&pending.client).is_some_and(|(hwnd, _)| unsafe {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        GetForegroundWindow() == hwnd
    });
    Some(protocol_follow_up(reached, &pending.client))
}

/// 纯函数：协议兜底后的结论文案（便于单测）。
pub(crate) fn protocol_follow_up(reached: bool, client: &str) -> String {
    if reached {
        format!("已切到 {client}")
    } else {
        format!("{client} 没有响应置前请求，仍在后台；请用任务栏或 Alt+Tab 手动切换")
    }
}

/// 客户端自己的置前协议。DSH 桌面端注册了 dsh://（app.on("open-url") 里
/// dsh://open 会 restore + focus 主窗口），比自己抢前台更可靠。
pub(crate) fn client_protocol(client: &str) -> Option<&'static str> {
    client.to_lowercase().contains("deepseek harness").then_some("dsh://open")
}

/// 百分比编码：只保留 RFC 3986 的 unreserved 字符。
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 解析跳转目标。session_id 与 cwd 允许为空字符串。
pub fn resolve(provider: Provider, session_id: &str, cwd: &str) -> Target {
    let session = session_id.trim();
    let dir = cwd.trim().to_owned();
    match provider {
        // Codex 官方支持按线程 id 的深链，精确且不依赖进程探测。
        Provider::Codex if !session.is_empty() && session.chars().count() <= 128 => {
            Target::DeepLink(format!("codex://threads/{}", encode(session)))
        }
        // DSH 没有定位具体会话的深链，只能把客户端带到前台（dsh://open 只负责置前）。
        Provider::Dsh => Target::Client { name: "DeepSeek Harness", cwd: dir },
        Provider::Codex if !dir.is_empty() => Target::Workspace(dir),
        _ => Target::Unavailable("客户端没有提供会话或目录信息"),
    }
}

/// 测试期拦截真实跳转：单元/UI 测试不应抢用户正在用的前台窗口。
/// 真机行为由 scripts/agent-e2e.ps1 覆盖，这里只保证「动作 → 反馈」的接线。
#[cfg(test)]
static INTERCEPT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 打开/关闭测试拦截（仅测试构建存在）。
#[cfg(test)]
pub(crate) fn intercept(enabled: bool) {
    INTERCEPT.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// 执行跳转。结果统一记录到反馈通道，上层用 toast 展示，避免静默。
pub fn open(target: &Target) -> JumpOutcome {
    #[cfg(test)]
    if INTERCEPT.load(std::sync::atomic::Ordering::Relaxed) {
        let client = match target {
            Target::Client { name, .. } => (*name).to_owned(),
            Target::DeepLink(_) => "客户端会话".to_owned(),
            Target::Workspace(dir) => format!("工作目录 {dir}"),
            Target::Unavailable(reason) => (*reason).to_owned(),
        };
        return report(JumpOutcome::AlreadyFront { client });
    }
    let outcome = match target {
        Target::DeepLink(uri) => match shell_open(uri) {
            Ok(()) => JumpOutcome::Opened { note: "已打开会话深链".to_owned() },
            Err(reason) => JumpOutcome::Failed { client: "客户端会话".to_owned(), reason },
        },
        Target::Client { name, .. } => open_client(name),
        Target::Workspace(dir) => match shell_open(dir) {
            Ok(()) => JumpOutcome::Opened { note: format!("已打开工作目录 {dir}") },
            Err(reason) => JumpOutcome::Failed { client: "工作目录".to_owned(), reason },
        },
        Target::Unavailable(reason) => JumpOutcome::Unavailable((*reason).to_owned()),
    };
    report(outcome)
}

/// 记录一条用户可见反馈，供上层取走。
fn report(outcome: JumpOutcome) -> JumpOutcome {
    if let Ok(mut slot) = FEEDBACK.lock() {
        *slot = Some(outcome.message());
    }
    outcome
}

fn arm_protocol(client: &str) {
    if let Ok(mut slot) = PENDING.lock() {
        *slot = Some(Pending { at: std::time::Instant::now(), client: client.to_owned() });
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn shell_open(target: &str) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = wide("open");
    let file = wide(target);
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW 约定：返回值大于 32 才算成功。
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("系统不接受这个跳转目标（代码 {}）", result.0 as isize))
    }
}

/// 候选窗口的度量（只留判定需要的部分，便于单测排序）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub titled: bool,
    pub iconic: bool,
    pub area: i64,
}

fn score(c: Candidate) -> i32 {
    let mut s = 0;
    if c.titled {
        s += 100;
    }
    if !c.iconic {
        s += 20;
    }
    // 1x1 / 16x16 这类辅助窗口面积太小，按 10 万像素一档计分（上限 50 分）。
    s + (c.area / 100_000).min(50) as i32
}

/// 为真表示 candidate 比当前最优更值得切。EnumWindows 顺序越靠前越上层，
/// 同分时保留先遇到的。
pub(crate) fn better(candidate: Candidate, current: Option<Candidate>) -> bool {
    match current {
        None => true,
        Some(cur) => score(candidate) > score(cur),
    }
}

/// 拿到窗口后该做什么（纯判定，不碰 Win32）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Missing,
    AlreadyFront,
    Restore,
    Raise,
}

pub(crate) fn decide(found: bool, foreground: bool, iconic: bool) -> Step {
    if !found {
        Step::Missing
    } else if iconic {
        Step::Restore
    } else if foreground {
        Step::AlreadyFront
    } else {
        Step::Raise
    }
}

struct Search<'a> {
    needle: &'a str,
    best: Option<(windows::Win32::Foundation::HWND, Candidate)>,
}

fn process_matches(pid: u32, needle: &str) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut buffer = [0u16; 512];
        let mut size = buffer.len() as u32;
        let matched = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut size).is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        matched && String::from_utf16_lossy(&buffer[..size as usize]).to_lowercase().contains(&needle.to_lowercase())
    }
}

unsafe extern "system" fn visit(hwnd: windows::Win32::Foundation::HWND, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::Foundation::{BOOL, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible};
    let search = &mut *(lp.0 as *mut Search);
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid != 0 && process_matches(pid, search.needle) {
        // 同一进程会有一堆辅助窗口（Electron 的 Chrome_WidgetWin_0 / IME / 托盘宿主），
        // 只看「可见 + 有标题 + 面积大」的那个主窗口，否则可能把前台切给一个 1x1 窗口。
        let mut title = [0u16; 256];
        let titled = GetWindowTextW(hwnd, &mut title) > 0;
        let iconic = IsIconic(hwnd).as_bool();
        let mut rect = RECT::default();
        let area = if GetWindowRect(hwnd, &mut rect).is_ok() {
            i64::from(rect.right - rect.left).max(0) * i64::from(rect.bottom - rect.top).max(0)
        } else {
            0
        };
        let candidate = Candidate { titled, iconic, area };
        if better(candidate, search.best.map(|(_, c)| c)) {
            search.best = Some((hwnd, candidate));
        }
    }
    BOOL(1)
}

/// 找客户端主窗口（可见顶层窗口里按标题/面积挑最像主窗口的一个）。最小化的窗口也算候选。
fn find_window(process_name: &str) -> Option<(windows::Win32::Foundation::HWND, Candidate)> {
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;
    let mut search = Search { needle: process_name, best: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.best
}

fn open_client(client: &str) -> JumpOutcome {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, ShowWindow, SW_RESTORE};
    let found = find_window(client);
    let (hwnd, iconic) = match found {
        Some((hwnd, candidate)) => (hwnd, candidate.iconic),
        None => (windows::Win32::Foundation::HWND::default(), false),
    };
    let foreground = found.is_some() && unsafe { GetForegroundWindow() == hwnd };
    match decide(found.is_some(), foreground, iconic) {
        Step::Missing => protocol_fallback(client, "没找到窗口，可能已退出或最小化到托盘"),
        Step::AlreadyFront => JumpOutcome::AlreadyFront { client: client.to_owned() },
        Step::Restore => {
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            if unsafe { GetForegroundWindow() == hwnd } {
                JumpOutcome::Restored { client: client.to_owned() }
            } else {
                raise_or_protocol(hwnd, client, true)
            }
        }
        Step::Raise => raise_or_protocol(hwnd, client, false),
    }
}

fn raise_or_protocol(hwnd: windows::Win32::Foundation::HWND, client: &str, restored: bool) -> JumpOutcome {
    match raise(hwnd) {
        Ok(()) if restored => JumpOutcome::Restored { client: client.to_owned() },
        Ok(()) => JumpOutcome::Focused { client: client.to_owned() },
        Err(reason) => protocol_fallback(client, &reason),
    }
}

/// 协议兜底（客户端自己 restore + focus）。协议也不可用就如实报错，不拿「打开目录」冒充跳转。
fn protocol_fallback(client: &str, why: &str) -> JumpOutcome {
    match client_protocol(client) {
        Some(protocol) => match shell_open(protocol) {
            Ok(()) => {
                arm_protocol(client);
                JumpOutcome::ProtocolRequested { client: client.to_owned() }
            }
            Err(protocol_error) => JumpOutcome::Failed {
                client: client.to_owned(),
                reason: format!("{why}；{protocol} 也打不开（{protocol_error}）"),
            },
        },
        None => JumpOutcome::Failed { client: client.to_owned(), reason: why.to_owned() },
    }
}

/// 把窗口切到前台。先直接试；被前台锁挡住时把当前线程临时挂到前台线程与目标线程上再试，
/// 最后用 GetForegroundWindow 校验——SetForegroundWindow 返回真但不生效的情况确实存在。
fn raise(hwnd: windows::Win32::Foundation::HWND) -> Result<(), String> {
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        if GetForegroundWindow() == hwnd {
            return Ok(());
        }
        // 前台锁：Windows 只允许前台进程直接抢前台。把当前线程挂到前台线程与目标线程上
        // 再切一次，是任务栏/启动器常用的兜底；无论成败都要解挂。
        let current = GetCurrentThreadId();
        let front = GetForegroundWindow();
        let front_thread = if front.0.is_null() { 0 } else { GetWindowThreadProcessId(front, None) };
        let target_thread = GetWindowThreadProcessId(hwnd, None);
        let mut attached = Vec::new();
        for thread in [front_thread, target_thread] {
            if thread != 0 && thread != current && AttachThreadInput(current, thread, BOOL(1)).as_bool() {
                attached.push(thread);
            }
        }
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        for thread in attached {
            let _ = AttachThreadInput(current, thread, BOOL(0));
        }
        if GetForegroundWindow() == hwnd {
            return Ok(());
        }
    }
    Err("系统拒绝切换前台（前台锁未解除），请用任务栏或 Alt+Tab 手动切换".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_thread_becomes_deep_link(){
        let target = resolve(Provider::Codex, "thread-abc-123", "C:\\work");
        assert_eq!(target, Target::DeepLink("codex://threads/thread-abc-123".to_owned()));
        assert_eq!(target.action_label(), "回到会话");
        assert!(target.available());
    }

    #[test]
    fn codex_without_session_falls_back_to_directory(){
        let target = resolve(Provider::Codex, "", "C:\\work");
        assert_eq!(target, Target::Workspace("C:\\work".to_owned()));
        assert_eq!(target.action_label(), "打开工作目录");
    }

    #[test]
    fn dsh_targets_its_client_window(){
        let target = resolve(Provider::Dsh, "session-1", "C:\\work");
        assert_eq!(target, Target::Client { name: "DeepSeek Harness", cwd: "C:\\work".to_owned() });
        assert_eq!(target.action_label(), "切换到客户端");
    }

    #[test]
    fn nothing_to_jump_to_is_reported(){
        let target = resolve(Provider::Codex, "", "");
        assert!(!target.available());
        assert!(matches!(target, Target::Unavailable(_)));
        let outcome = open(&target);
        assert!(!outcome.ok());
        assert!(outcome.message().contains("没有提供会话或目录信息"), "不可用原因要原样带给用户：{}", outcome.message());
        // 任何结果都要进反馈通道，而且能被取走一次。
        assert!(take_feedback().is_some());
    }

    #[test]
    fn deep_link_escapes_unsafe_characters(){
        let target = resolve(Provider::Codex, "a b/c?d", "");
        assert_eq!(target, Target::DeepLink("codex://threads/a%20b%2Fc%3Fd".to_owned()),
            "会话 id 里的空格与分隔符必须转义，否则深链会被截断");
    }

    #[test]
    fn oversized_session_id_is_not_used_as_deep_link(){
        let long = "x".repeat(129);
        let target = resolve(Provider::Codex, long.as_str(), "C:\\work");
        assert_eq!(target, Target::Workspace("C:\\work".to_owned()), "超长 id 不可信，回落到目录");
    }

    #[test]
    fn blank_session_id_is_trimmed(){
        assert_eq!(resolve(Provider::Codex, "   ", "C:\\work"), Target::Workspace("C:\\work".to_owned()));
    }

    // ---------- 跳转反馈：任何结果都必须有话说 ----------

    #[test]
    fn already_front_message_names_client_and_hints_no_precise_jump(){
        let outcome = JumpOutcome::AlreadyFront { client: "DeepSeek Harness".to_owned() };
        let message = outcome.message();
        assert!(message.contains("DeepSeek Harness"), "要带上客户端名：{message}");
        assert!(message.contains("最前台"), "要说清原因（已在最前台）：{message}");
        assert!(message.contains("无法精确定位"), "不能让用户以为已经跳到具体会话：{message}");
        assert!(outcome.clone().ok(), "已在前台不算失败");
        assert_eq!(outcome.err(), None, "已在前台不应该再当成错误重复提示");
    }

    #[test]
    fn focused_and_restored_messages_confirm_the_switch(){
        let focused = JumpOutcome::Focused { client: "DeepSeek Harness".to_owned() };
        assert!(focused.message().starts_with("已切到"));
        let restored = JumpOutcome::Restored { client: "DeepSeek Harness".to_owned() };
        assert!(restored.message().contains("最小化"), "恢复最小化要说明：{}", restored.message());
        assert!(focused.ok() && restored.ok());
        assert_eq!(focused.clone().err(), None);
        assert_eq!(restored.clone().err(), None);
    }

    #[test]
    fn failure_messages_keep_the_specific_reason(){
        let failed = JumpOutcome::Failed { client: "DeepSeek Harness".to_owned(), reason: "前台锁未解除".to_owned() };
        let message = failed.message();
        assert!(message.contains("DeepSeek Harness") && message.contains("前台锁未解除"), "失败原因不能丢：{message}");
        assert!(!failed.clone().ok());
        assert_eq!(failed.err(), Some(message), "失败必须能交给 jump_error 显示");

        let missing = JumpOutcome::NoWindow { client: "DeepSeek Harness".to_owned() };
        assert!(missing.message().contains("没找到"));
        assert!(!missing.clone().ok());
        assert!(missing.err().is_some());

        let unavailable = JumpOutcome::Unavailable("客户端没有提供会话或目录信息".to_owned());
        assert_eq!(unavailable.message(), "客户端没有提供会话或目录信息");
        assert!(!unavailable.clone().ok());
        assert!(unavailable.err().is_some());
    }

    #[test]
    fn protocol_request_is_pending_not_an_error(){
        let requested = JumpOutcome::ProtocolRequested { client: "DeepSeek Harness".to_owned() };
        assert!(requested.message().contains("dsh://open"), "协议兜底要说明用的是什么：{}", requested.message());
        assert!(requested.ok(), "已发出置前请求不算失败，结论由宽限期后的 poll_protocol 给");
        assert_eq!(requested.clone().err(), None);
    }

    #[test]
    fn client_failure_never_masquerades_as_workspace_jump(){
        // 审计要求：Target::Client 失败不能再打开资源管理器冒充成功——按钮写「切换到客户端」，
        // 弹文件夹就是骗人；失败必须如实返回原因，由 UI 常驻提示。
        let failed = protocol_fallback("Codex", "系统拒绝切换前台（前台锁未解除）");
        assert!(matches!(failed, JumpOutcome::Failed { .. }), "没有协议兜底的客户端必须返回 Failed：{failed:?}");
        assert!(!failed.clone().ok());
        let message = failed.message();
        assert!(message.contains("Codex") && message.contains("前台锁未解除"), "原因要落到文案里：{message}");
    }

    #[test]
    fn every_outcome_has_a_visible_message(){
        let outcomes = [
            JumpOutcome::AlreadyFront { client: "客户端".to_owned() },
            JumpOutcome::Restored { client: "客户端".to_owned() },
            JumpOutcome::Focused { client: "客户端".to_owned() },
            JumpOutcome::ProtocolRequested { client: "客户端".to_owned() },
            JumpOutcome::Opened { note: "已打开工作目录 C:\\work".to_owned() },
            JumpOutcome::NoWindow { client: "客户端".to_owned() },
            JumpOutcome::Failed { client: "客户端".to_owned(), reason: "前台锁未解除".to_owned() },
            JumpOutcome::Unavailable("没有可用信息".to_owned()),
        ];
        for outcome in outcomes {
            assert!(!outcome.message().trim().is_empty(), "每种跳转结果都要有文案：{outcome:?}");
        }
    }

    // ---------- 判定逻辑（纯函数，不依赖真机） ----------

    #[test]
    fn decide_prefers_restore_for_minimized_window(){
        assert_eq!(decide(false, false, false), Step::Missing, "找不到窗口就不能假装成功");
        assert_eq!(decide(true, true, false), Step::AlreadyFront, "已在前台不需要再切");
        assert_eq!(decide(true, false, false), Step::Raise);
        assert_eq!(decide(true, true, true), Step::Restore, "最小化的窗口要先恢复，恢复后才会成为前台");
        assert_eq!(decide(true, false, true), Step::Restore);
    }

    #[test]
    fn candidate_ranking_skips_helper_windows(){
        let helper = Candidate { titled: false, iconic: false, area: 256 };
        let main = Candidate { titled: true, iconic: false, area: 1_296 * 828 };
        assert!(better(main, None), "第一个候选直接采用");
        assert!(better(main, Some(helper)), "有标题、面积大的主窗口应胜出");
        assert!(!better(helper, Some(main)), "辅助窗口不能顶掉主窗口");
        let minimized = Candidate { titled: true, iconic: true, area: 1_296 * 828 };
        assert!(better(minimized, Some(helper)), "最小化的主窗口也比辅助窗口值得切");
    }

    #[test]
    fn dsh_client_uses_its_own_protocol(){
        assert_eq!(client_protocol("DeepSeek Harness"), Some("dsh://open"));
        assert_eq!(client_protocol("deepseek harness"), Some("dsh://open"), "进程名大小写不敏感");
        assert_eq!(client_protocol("Codex"), None, "别的客户端没有注册这个协议");
    }

    #[test]
    fn protocol_follow_up_reports_both_verdicts(){
        let reached = protocol_follow_up(true, "DeepSeek Harness");
        assert!(reached.starts_with("已切到") && reached.contains("DeepSeek Harness"));
        let missed = protocol_follow_up(false, "DeepSeek Harness");
        assert!(missed.contains("没有响应置前请求") && missed.contains("任务栏"), "宽限期后仍失败要给手动兜底：{missed}");
    }
}
