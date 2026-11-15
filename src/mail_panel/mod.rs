//! 邮件面板（WebView 试点）：列表与阅读都交给 HTML 页面，egui 只画外壳。
//!
//! 开关：`DAOZAI_MAIL_PANEL=webview`（强制）、`auto`（可用就用）、`native`（关）。
//! 不可用/未开启时调用方继续走 egui 版本，所以这里所有失败路径都只返回 `false`，
//! 不 panic、不影响主界面。

mod bridge;
#[cfg(windows)]
mod webview;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

pub use bridge::{Command, State, StateAccount, StateItem};

/// 面板矩形（窗口客户区坐标，points），第 5 位是 pixels_per_point。
type Rect = Arc<Mutex<[f32; 5]>>;

/// 页面加载给的耐心：5 秒还不发 `ready` 就判定失败，本会话不再重试。
const READY_TIMEOUT: Duration = Duration::from_millis(5000);
/// 隐藏超过这个时长就彻底关掉宿主线程，避免长期白占一份 WebView2 内存。
const HIDE_LINGER: Duration = Duration::from_secs(90);

struct Panel {
    rect: Rect,
    stop: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    outbox: mpsc::Sender<String>,
    inbox: mpsc::Receiver<String>,
    /// 上一份 state 的「指纹 + JSON」：指纹相同连序列化都省掉，
    /// 指纹不同再比一次 JSON 兜底（防止哈希碰撞把界面卡住）。
    last: Mutex<Option<(u64, String)>>,
    started: Instant,
    ready: AtomicBool,
    hidden_since: Mutex<Option<Instant>>,
}

static PANEL: Mutex<Option<Panel>> = Mutex::new(None);
/// 本进程内已经失败过一次：不再重试，避免每次切页都闪一下白框。
static DEAD: AtomicBool = AtomicBool::new(false);
/// 面板当前打开的邮件 `(uid, account)`：切远程图开关时要据此重推正文。
static CURRENT: Mutex<Option<(u32, String)>> = Mutex::new(None);

/// 三级降级的第一级判定。
pub fn enabled() -> bool {
    if !cfg!(windows) {
        return false;
    }
    if DEAD.load(Ordering::Relaxed) {
        return false;
    }
    match std::env::var("DAOZAI_MAIL_PANEL").ok().as_deref() {
        Some("webview") => true,
        // 灰度入口：运行时可用的机器自动启用，其余仍走 egui。
        Some("auto") => true,
        Some("native") => false,
        // 未指定：保持现状（egui 版），默认值切换等 M5 数据出来再定。
        _ => false,
    }
}

/// 每帧调用：首帧拉起宿主线程，之后只更新矩形。返回 false 表示不可用，调用方回退。
pub fn present(rect: [f32; 4], ppp: f32) -> bool {
    if !enabled() {
        return false;
    }
    let ppp = if ppp.is_finite() && ppp > 0.1 { ppp } else { 1.0 };
    let physical = [
        rect[0] * ppp,
        rect[1] * ppp,
        (rect[2] * ppp).max(0.0),
        (rect[3] * ppp).max(0.0),
    ];
    let mut guard = match PANEL.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    if let Some(panel) = guard.as_ref() {
        // 超时未 ready：判定失败，本次运行不再尝试，让调用方继续走 egui。
        if !panel.ready.load(Ordering::Relaxed) && panel.started.elapsed() > READY_TIMEOUT {
            DEAD.store(true, Ordering::Relaxed);
            eprintln!("邮件面板 5 秒内未就绪，本次运行回退 egui 邮件页");
            if let Some(panel) = guard.take() {
                panel.stop.store(true, Ordering::Relaxed);
            }
            return false;
        }
        if let Ok(mut slot) = panel.rect.lock() {
            slot[..4].copy_from_slice(&physical);
            slot[4] = ppp;
        }
        panel.visible.store(true, Ordering::Relaxed);
        if let Ok(mut since) = panel.hidden_since.lock() {
            *since = None;
        }
        return true;
    }
    let rect: Rect = Arc::new(Mutex::new([physical[0], physical[1], physical[2], physical[3], ppp]));
    let stop = Arc::new(AtomicBool::new(false));
    let visible = Arc::new(AtomicBool::new(true));
    let (outbox, rx) = mpsc::channel::<String>();
    let (tx, inbox) = mpsc::channel::<String>();
    #[cfg(windows)]
    {
        let (thread_rect, thread_stop, thread_visible) = (rect.clone(), stop.clone(), visible.clone());
        let spawn = std::thread::Builder::new()
            .name("mail-panel-webview".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    webview::run_panel(thread_rect, thread_stop, thread_visible, rx, tx)
                }));
                if let Err(error) = outcome {
                    // panic 已经被关在这一条线程里，主界面不受影响；只记一句，交给超时降级处理。
                    let reason = error
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_string())
                        .or_else(|| error.downcast_ref::<String>().cloned())
                        .unwrap_or_default();
                    eprintln!("邮件面板异常退出（已隔离）：{reason}");
                }
            });
        if spawn.is_err() {
            return false;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (rx, tx, visible);
        return false;
    }
    #[allow(unreachable_code)]
    {
        *guard = Some(Panel {
            rect,
            stop,
            visible,
            outbox,
            inbox,
            last: Mutex::new(None),
            started: Instant::now(),
            ready: AtomicBool::new(false),
            hidden_since: Mutex::new(None),
        });
        true
    }
}

/// 不在邮件页时每帧调用：先隐藏，久了再回收线程。幂等，无副作用。
pub fn hide() {
    let Ok(mut guard) = PANEL.lock() else { return };
    let Some(panel) = guard.as_mut() else { return };
    panel.visible.store(false, Ordering::Relaxed);
    let linger = match panel.hidden_since.lock() {
        Ok(mut since) => *since.get_or_insert_with(Instant::now),
        Err(_) => return,
    };
    if linger.elapsed() > HIDE_LINGER {
        if let Some(panel) = guard.take() {
            panel.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// 下发一份完整状态（前端整表重绘）。
pub fn push(state: &State) {
    let Ok(guard) = PANEL.lock() else { return };
    let Some(panel) = guard.as_ref() else { return };
    let Ok(mut last) = panel.last.lock() else { return };
    let fingerprint = state.fingerprint();
    if last.as_ref().is_some_and(|(seen, _)| *seen == fingerprint) {
        return;
    }
    let Ok(json) = serde_json::to_string(&bridge::Envelope::wrap("state", &state.to_value())) else { return };
    let same = last.as_ref().is_some_and(|(_, text)| *text == json);
    *last = Some((fingerprint, json.clone()));
    drop(last);
    if same {
        return;
    }
    let _ = panel.outbox.send(json);
}

/// 推送阅读页正文。`kind` 为 `text` 时按纯文本渲染（只有 text/plain 的老邮件）。
pub fn push_body(uid: u32, account: &str, kind: &str, body: &str, allow_remote: bool, blocked: usize) {
    if let Ok(mut slot) = CURRENT.lock() {
        *slot = Some((uid, account.to_string()));
    }
    let Ok(guard) = PANEL.lock() else { return };
    let Some(panel) = guard.as_ref() else { return };
    let payload = serde_json::json!({
        "uid": uid,
        "account": account,
        "kind": kind,
        "html": if kind == "html" { body } else { "" },
        "text": if kind == "text" { body } else { "" },
        "allow_remote": allow_remote,
        "blocked": blocked,
    });
    if let Ok(json) = serde_json::to_string(&bridge::Envelope::wrap("body", &payload)) {
        let _ = panel.outbox.send(json);
    }
}

/// 顶部小提示（3 秒消失）。用于「链接已交给浏览器」这类不需要打断的反馈。
pub fn toast(level: &str, text: &str) {
    let Ok(guard) = PANEL.lock() else { return };
    let Some(panel) = guard.as_ref() else { return };
    let payload = serde_json::json!({ "level": level, "text": text });
    if let Ok(json) = serde_json::to_string(&bridge::Envelope::wrap("toast", &payload)) {
        let _ = panel.outbox.send(json);
    }
}

/// 面板正在阅读的邮件。返回 `None` 表示还停在列表页。
pub fn current() -> Option<(u32, String)> {
    CURRENT.lock().ok().and_then(|slot| slot.clone())
}

/// 回到列表页时清掉阅读态。
pub fn clear_current() {
    if let Ok(mut slot) = CURRENT.lock() {
        *slot = None;
    }
}

/// 取走前端发来的命令，交给调用方处理。
pub fn drain() -> Vec<Command> {
    let Ok(guard) = PANEL.lock() else { return Vec::new() };
    let Some(panel) = guard.as_ref() else { return Vec::new() };
    let mut out = Vec::new();
    // 单帧上限，防止页面异常刷屏打爆 IMAP。
    for _ in 0..32 {
        match panel.inbox.try_recv() {
            Ok(raw) => match Command::parse(&raw) {
                Ok(command) => {
                    if matches!(command, Command::Ready) {
                        panel.ready.store(true, Ordering::Relaxed);
                    }
                    out.push(command);
                }
                Err(reason) => eprintln!("邮件面板：忽略非法命令（{reason}）"),
            },
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_defaults_to_native() {
        // 环境变量在多测试并行下不可靠，这里只验证「未设置时是 false」这一条约定，
        // 且不修改进程环境（其他测试也可能读它）。
        if std::env::var("DAOZAI_MAIL_PANEL").is_err() {
            assert!(!enabled(), "未指定开关时必须回退 egui 版");
        }
    }

    #[test]
    fn hide_without_panel_is_noop() {
        // 没启用面板时每帧都会调用 hide()，不能 panic、不能占锁。
        hide();
        assert!(drain().is_empty());
    }
}
