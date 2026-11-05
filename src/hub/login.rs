//! 取额度用的无痕登录：后台运行官方 `claude auth login --claudeai`，
//! 登录页经 BROWSER 变量交给一个小脚本，直接在 Edge / Chrome 的无痕窗口打开（不受默认浏览器里已登录账号影响），
//! 授权后由本机回调自动完成。岛不接触授权码或令牌，登录结果由 claude 自己保存在该账号的目录里。
use super::{proc, store};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn private_browser() -> Option<(PathBuf, &'static str)> {
    let bases: Vec<PathBuf> = ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"].iter().filter_map(|k| std::env::var_os(k).map(PathBuf::from)).collect();
    for b in &bases {
        let edge = b.join("Microsoft").join("Edge").join("Application").join("msedge.exe");
        if edge.is_file() {
            return Some((edge, "--inprivate"));
        }
    }
    for b in &bases {
        let chrome = b.join("Google").join("Chrome").join("Application").join("chrome.exe");
        if chrome.is_file() {
            return Some((chrome, "--incognito"));
        }
    }
    None
}

/// 启动脚本（与 claude-hub 相同）：每次被调用只记一行时间（不记链接）
fn launcher() -> Option<PathBuf> {
    let (exe, flag) = private_browser()?;
    let hub = store::hub_dir();
    let file = hub.join("open-private.cmd");
    let log = hub.join("logs").join("open-private.log");
    let body = format!("@echo off\r\necho %date% %time% opened>> \"{}\"\r\nstart \"\" \"{}\" {flag} %1\r\n", log.display(), exe.display());
    if std::fs::read_to_string(&file).ok().as_deref() != Some(body.as_str()) {
        std::fs::create_dir_all(&hub).ok()?;
        std::fs::write(&file, &body).ok()?;
    }
    Some(file)
}

pub fn log(id: &str, phase: &str, extra: serde_json::Value) {
    let mut e = json!({"at": chrono::Utc::now().to_rfc3339(), "id": id, "phase": phase, "from": "island"});
    if let (Some(o), Some(x)) = (e.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            o.insert(k.clone(), v.clone());
        }
    }
    store::write_log("login.log", e);
}

/// 阻塞到登录结束（最多 6 分钟）。返回 claude 的退出码是否为 0
pub fn run(id: &str, dir: &Path, email: &str) -> Result<(), String> {
    let launcher = launcher().ok_or("没找到 Edge 或 Chrome，开不了无痕窗口")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("无法建登录目录：{e}"))?;
    let mut line = String::from("claude auth login --claudeai");
    if proc::safe_email(email) {
        line.push_str(" --email ");
        line.push_str(email.trim());
    }
    let mut c = proc::shell(&line);
    c.env("CLAUDE_CONFIG_DIR", dir).env("BROWSER", &launcher);
    log(id, "start", json!({"dir": dir}));
    let out = proc::run(c, Duration::from_secs(360));
    log(id, "exit", json!({"code": out.code}));
    match out.code {
        Some(0) => Ok(()),
        None => Err("等了 6 分钟还没完成，已取消".into()),
        Some(_) => Err("登录没有完成".into()),
    }
}
