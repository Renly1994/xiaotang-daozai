//! 会话跳转：把用户从岛上的卡片带回原客户端。
//!
//! 分级降级（对照 island-hud 的 jump-target 顺序，去掉依赖进程探测的不可靠档位）：
//! Codex 线程深链 > 客户端窗口前置 > 工作目录 > 无法跳转。
//!
//! 只使用本机调用（ShellExecute / SetForegroundWindow），不新开任何网络端口。

use crate::agent_notice::Provider;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Codex 线程深链，交给系统协议处理器打开。
    DeepLink(String),
    /// 把客户端主窗口带到前台；窗口找不到时回落到工作目录。
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

/// 解析跳转目标。`session_id` 与 `cwd` 允许为空字符串。
pub fn resolve(provider: Provider, session_id: &str, cwd: &str) -> Target {
    let session = session_id.trim();
    let dir = cwd.trim().to_owned();
    match provider {
        // Codex 官方支持按线程 id 的深链，精确且不依赖进程探测。
        Provider::Codex if !session.is_empty() && session.chars().count() <= 128 => {
            Target::DeepLink(format!("codex://threads/{}", encode(session)))
        }
        // DSH 没有对外深链协议，只能把客户端带到前台；找不到窗口再回落到目录。
        Provider::Dsh => Target::Client { name: "DeepSeek Harness", cwd: dir },
        Provider::Codex if !dir.is_empty() => Target::Workspace(dir),
        _ => Target::Unavailable("客户端没有提供会话或目录信息"),
    }
}

/// 执行跳转。返回可直接展示给用户的中文原因。
pub fn open(target: &Target) -> Result<(), String> {
    match target {
        Target::DeepLink(uri) => shell_open(uri),
        Target::Client { name, cwd } => match foreground(name) {
            Ok(()) => Ok(()),
            Err(window_error) if !cwd.is_empty() => {
                shell_open(cwd).map_err(|dir_error| format!("{window_error}；打开目录也失败：{dir_error}"))
            }
            Err(window_error) => Err(window_error),
        },
        Target::Workspace(dir) => shell_open(dir),
        Target::Unavailable(reason) => Err((*reason).to_owned()),
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

struct Search<'a> {
    needle: &'a str,
    found: Option<windows::Win32::Foundation::HWND>,
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
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindowVisible};
    let search = &mut *(lp.0 as *mut Search);
    if !IsWindowVisible(hwnd).as_bool() {
        return windows::Win32::Foundation::BOOL(1);
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid != 0 && process_matches(pid, search.needle) {
        search.found = Some(hwnd);
        return windows::Win32::Foundation::BOOL(0);
    }
    windows::Win32::Foundation::BOOL(1)
}

fn foreground(process_name: &str) -> Result<(), String> {
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, SetForegroundWindow};
    let mut search = Search { needle: process_name, found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
        let Some(hwnd) = search.found else {
            return Err(format!("没有找到 {process_name} 的窗口，可能已最小化到托盘"));
        };
        if SetForegroundWindow(hwnd).as_bool() {
            Ok(())
        } else {
            Err("系统拒绝切换前台，请手动点击客户端".to_owned())
        }
    }
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
        assert!(open(&target).is_err());
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
}
