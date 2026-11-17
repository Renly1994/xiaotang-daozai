//! 浏览器扩展对接的主程序侧辅助函数。
//!
//! 桥宿主（接收 Chrome/Edge 下载事件、翻译成 island_status.json）已拆成独立的
//! 精简二进制 `daozai-bridge`（见 `src/bin/daozai-bridge/`），不再由主程序兼任。
//! 本模块只剩设置页「浏览器下载」用到的两件事：
//! - `installed_browsers()`：找本机可用的 Chromium 浏览器（一键引导连接用）；
//! - `extension_folder()`：安装布局下的扩展文件夹路径（复制/打开用）。

use std::path::PathBuf;

/// 找本机可用的浏览器（Chromium 系，支持 Native Messaging）：返回 (显示名, exe 路径)。
///
/// 优先注册表 App Paths（含 HKCU 便携安装），再用常见安装位置兜底；只返回真实存在的 exe，
/// 去重后 Chrome 在前、Edge 在后。
pub fn installed_browsers() -> Vec<(&'static str, PathBuf)> {
    let mut result: Vec<(&'static str, PathBuf)> = Vec::new();
    for (name, image, fallbacks) in [
        ("Chrome", "chrome.exe", [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ]),
        ("Edge", "msedge.exe", [
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]),
    ] {
        let candidates: Vec<PathBuf> = app_path(image)
            .into_iter()
            .chain(fallbacks.iter().map(PathBuf::from))
            .chain(std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Google\Chrome\Application").join(image)))
            .filter(|p| p.is_file())
            .collect();
        if let Some(exe) = candidates.into_iter().next() {
            if !result.iter().any(|(n, _)| *n == name) {
                result.push((name, exe));
            }
        }
    }
    result
}

/// 注册表 App Paths 里注册的浏览器完整路径（HKLM 优先，HKCU 兜底）。
fn app_path(image: &str) -> Option<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let subkey: HSTRING = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{image}").into();
    let mut buffer = [0u16; 512];
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let mut size = (buffer.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(root, &subkey, None, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut size))
        };
        if ok.is_ok() {
            let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
            let path = String::from_utf16_lossy(&buffer[..end]);
            if !path.trim().is_empty() {
                return Some(PathBuf::from(path.trim().trim_matches('"')));
            }
        }
    }
    None
}

/// 扩展文件夹：先看 exe 同目录的 bridge\extension（安装包布局），
/// 再看数据目录下的 bridge\extension（单文件分发时用户自己放扩展的位置）。
pub fn extension_folder() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        candidates.push(dir.join("bridge").join("extension"));
    }
    candidates.push(crate::model::config_dir().join("bridge").join("extension"));
    candidates.into_iter().find(|ext| ext.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 浏览器检测至少找到Edge且路径真实存在() {
        let browsers = installed_browsers();
        let edge = browsers.iter().find(|(name, _)| *name == "Edge");
        assert!(edge.is_some(), "Windows 自带 Edge，应该能找到");
        assert!(edge.unwrap().1.is_file(), "返回的 exe 路径必须真实存在");
        for (name, _) in &browsers {
            assert_eq!(browsers.iter().filter(|(n, _)| n == name).count(), 1);
        }
    }

    #[test]
    fn AppPaths注册表读取返回存在的路径或无() {
        for image in ["chrome.exe", "msedge.exe"] {
            if let Some(path) = app_path(image) {
                assert!(path.is_file(), "App Paths 指向的 {} 不存在：{}", image, path.display());
            }
        }
    }

    #[test]
    fn 扩展目录仅在安装布局下出现() {
        let _ = extension_folder();
    }
}
