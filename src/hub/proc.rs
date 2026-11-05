//! 运行官方命令（claude / codex）和系统命令：不弹黑窗口，带超时，
//! 并去掉从 Claude 桌面端会话继承来的变量，免得子进程被当成桌面端的那个账号。
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const NO_WINDOW: u32 = 0x0800_0000;

pub struct Out {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn inherited(key: &str) -> bool {
    key.starts_with("CLAUDE_CODE_") || key == "CLAUDECODE" || key.starts_with("CLAUDE_AGENT_SDK_") || key.starts_with("ANTHROPIC_")
}

/// 去掉继承变量后的新命令
pub fn command(program: &str) -> Command {
    let mut c = Command::new(program);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if inherited(&k) {
            c.env_remove(&k);
        }
    }
    #[cfg(windows)]
    c.creation_flags(NO_WINDOW);
    c
}

/// claude / codex 可能是 .cmd 包装，统一经 cmd 运行。`line` 只能包含我们自己拼的、校验过的参数。
pub fn shell(line: &str) -> Command {
    let mut c = command("cmd.exe");
    #[cfg(windows)]
    c.raw_arg(format!("/d /s /c \"{line}\""));
    #[cfg(not(windows))]
    c.arg("-c").arg(line);
    c
}

/// 运行到结束或超时（超时结束整个进程树）
pub fn run(mut c: Command, timeout: Duration) -> Out {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let Ok(mut child) = c.spawn() else {
        return Out { code: None, stdout: String::new(), stderr: "无法启动命令".into() };
    };
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t1 = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = so.as_mut() {
            let _ = o.take(16 * 1024 * 1024).read_to_string(&mut s);
        }
        s
    });
    let t2 = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = se.as_mut() {
            let _ = o.take(1024 * 1024).read_to_string(&mut s);
        }
        s
    });
    let start = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                kill_tree(child.id());
                let _ = child.wait();
                break None;
            }
        }
    };
    Out { code, stdout: t1.join().unwrap_or_default(), stderr: t2.join().unwrap_or_default() }
}

pub fn kill_tree(pid: u32) {
    let mut c = command("taskkill");
    c.args(["/F", "/T", "/PID", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null());
    let _ = c.status();
}

/// 不等结果地启动（打开桌面端、浏览器等）
pub fn spawn_detached(program: &str, args: &[&str]) -> bool {
    let mut c = Command::new(program);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c.spawn().is_ok()
}

/// 邮箱参数只放行安全字符，避免拼进命令行出问题
pub fn safe_email(email: &str) -> bool {
    let e = email.trim();
    !e.is_empty()
        && e.len() < 200
        && e.chars().filter(|c| *c == '@').count() == 1
        && e.chars().all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn email_filter_rejects_shell_characters() {
        assert!(safe_email("a.b+c@example.com"));
        assert!(!safe_email("a@b.com & calc"));
        assert!(!safe_email("a\"@b.com"));
        assert!(!safe_email("no-at.example.com"));
    }
    #[test]
    fn inherited_desktop_variables_are_detected() {
        assert!(inherited("CLAUDE_CODE_SESSION_ID"));
        assert!(inherited("ANTHROPIC_API_KEY"));
        assert!(inherited("CLAUDECODE"));
        assert!(!inherited("CLAUDE_CONFIG_DIR"));
        assert!(!inherited("PATH"));
    }
}
