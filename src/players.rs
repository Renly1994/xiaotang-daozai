//! 找出本机装了哪些音乐播放器，供「暂无音乐」时点封面选择并启动。
//!
//! 只查注册表，不扫磁盘：`App Paths` 给出最常见的安装位置，卸载项里的
//! `DisplayIcon` / `InstallLocation` 兜住那些不写 App Paths 的客户端
//! （例如 QQ 音乐会装在 `Tencent\QQMusic\QQMusic<版本号>\` 这种带版本号的目录里）。
//! 两条来源都是一次读几十个键，放在点开列表时同步跑，耗时可忽略。

use std::path::PathBuf;

/// 一个可启动的播放器。
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    /// 界面上显示的名字，和 `smtc::app_label` 的说法保持一致。
    pub name: String,
    /// 进程名，和 SMTC 的 `SourceAppUserModelId` 对得上。
    pub exe: String,
    /// 可执行文件的绝对路径。
    pub path: PathBuf,
}

/// 支持的客户端：显示名 + 进程名。顺序就是列表里的顺序。
const CATALOG: &[(&str, &str)] = &[
    ("网易云音乐", "cloudmusic.exe"),
    ("QQ音乐", "QQMusic.exe"),
    ("酷狗音乐", "KuGou.exe"),
    ("酷我音乐", "KwMusic.exe"),
    ("汽水音乐", "SodaMusic.exe"),
    ("Spotify", "Spotify.exe"),
];

/// 列出本机已安装且文件确实存在的播放器。
pub fn scan() -> Vec<Player> {
    #[cfg(windows)]
    {
        let icons = windows::uninstall_icons();
        let mut players = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for (name, exe) in CATALOG {
            let Some(path) = windows::app_paths(exe).or_else(|| icons.get(&exe.to_lowercase()).cloned()) else { continue };
            if !path.is_file() {
                continue;
            }
            let key = path.to_string_lossy().to_lowercase();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            players.push(Player { name: (*name).into(), exe: (*exe).into(), path });
        }
        players
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// 启动选中的客户端。已经开着的话，Windows 会把它的窗口叫到前台。
pub fn launch(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // 播放器都是 GUI 程序，直接起进程即可；不带控制台，不会闪黑框。
        std::process::Command::new(path).spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "当前系统不支持启动播放器"))
    }
}

/// 从卸载项的 `DisplayIcon` 里取出 exe 路径：`"C:\a\b.exe",0` → `C:\a\b.exe`。
///
/// 有些客户端这里的值是图标资源（`.ico` 或 `dll,3`），拿不到 exe 时返回空。
fn icon_path(raw: &str) -> Option<PathBuf> {
    let text = raw.trim().trim_matches('"');
    // 逗号后面是图标索引，可能是负数（-123）。
    let text = match text.rsplit_once(',') {
        Some((head, tail)) if tail.trim().parse::<i32>().is_ok() => head.trim().trim_matches('"'),
        _ => text,
    };
    let lower = text.to_lowercase();
    if !lower.ends_with(".exe") {
        return None;
    }
    Some(PathBuf::from(text))
}

#[cfg(windows)]
mod windows {
    use super::{icon_path, PathBuf};
    use std::collections::HashMap;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::System::Registry::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 读一个字符串值；值不存在或类型不对时返回 `None`。
    fn value(key: HKEY, sub: &str, name: &str) -> Option<String> {
        let sub = wide(sub);
        let name = wide(name);
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let flags = RRF_RT_REG_SZ | RRF_NOEXPAND;
        unsafe {
            if RegGetValueW(key, PCWSTR(sub.as_ptr()), PCWSTR(name.as_ptr()), flags, Some(&mut kind), None, Some(&mut size)).is_err() || size == 0 {
                return None;
            }
            let mut buffer = vec![0u16; (size as usize + 1) / 2];
            if RegGetValueW(key, PCWSTR(sub.as_ptr()), PCWSTR(name.as_ptr()), flags, Some(&mut kind), Some(buffer.as_mut_ptr() as *mut _), Some(&mut size)).is_err() {
                return None;
            }
            let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..length]))
        }
    }

    fn subkeys(key: HKEY, sub: &str) -> Vec<String> {
        let name = wide(sub);
        let mut opened = HKEY::default();
        unsafe {
            if RegOpenKeyExW(key, PCWSTR(name.as_ptr()), 0, KEY_READ, &mut opened).is_err() {
                return Vec::new();
            }
        }
        let mut out = Vec::new();
        let mut index = 0u32;
        loop {
            let mut buffer = [0u16; 256];
            let mut length = buffer.len() as u32;
            let result = unsafe { RegEnumKeyExW(opened, index, PWSTR(buffer.as_mut_ptr()), &mut length, None, PWSTR::null(), None, None) };
            if result.is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&buffer[..length as usize]));
            index += 1;
        }
        unsafe {
            let _ = RegCloseKey(opened);
        }
        out
    }

    const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";
    const UNINSTALL: [&str; 3] = [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    /// 优先读 `App Paths\<exe>` 的默认值：这是客户端自己登记的可执行文件路径。
    pub fn app_paths(exe: &str) -> Option<PathBuf> {
        for (key, sub) in [
            (HKEY_CURRENT_USER, format!(r"{APP_PATHS}\{exe}")),
            (HKEY_LOCAL_MACHINE, format!(r"{APP_PATHS}\{exe}")),
            (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\{exe}")),
        ] {
            if let Some(path) = value(key, &sub, "") {
                let path = PathBuf::from(path.trim().trim_matches('"'));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        None
    }

    /// 扫一遍卸载项，按「进程名 → 可执行文件路径」建索引。
    ///
    /// 拿不到 `DisplayIcon` 的用 `InstallLocation` 拼一下，两者都要文件真的存在。
    pub fn uninstall_icons() -> HashMap<String, PathBuf> {
        let mut out = HashMap::new();
        for (key, root) in [
            (HKEY_LOCAL_MACHINE, UNINSTALL[0]),
            (HKEY_LOCAL_MACHINE, UNINSTALL[1]),
            (HKEY_CURRENT_USER, UNINSTALL[2]),
        ] {
            for entry in subkeys(key, root) {
                let sub = format!(r"{root}\{entry}");
                let icon = value(key, &sub, "DisplayIcon").and_then(|raw| icon_path(&raw));
                let located = icon.or_else(|| {
                    let location = value(key, &sub, "InstallLocation")?;
                    let directory = PathBuf::from(location.trim().trim_matches('"'));
                    let name = std::fs::read_dir(&directory).ok()?.flatten().find_map(|item| {
                        let path = item.path();
                        let name = path.file_name()?.to_string_lossy().to_lowercase();
                        name.ends_with(".exe").then_some(path)
                    })?;
                    Some(name)
                });
                let Some(path) = located else { continue };
                let Some(name) = path.file_name().map(|name| name.to_string_lossy().to_lowercase()) else { continue };
                if path.is_file() {
                    out.entry(name).or_insert(path);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_icon_drops_the_icon_index() {
        assert_eq!(icon_path(r#""C:\Program Files\a\b.exe",0"#), Some(PathBuf::from(r"C:\Program Files\a\b.exe")));
        assert_eq!(icon_path(r"C:\x\QQMusic.exe,-123"), Some(PathBuf::from(r"C:\x\QQMusic.exe")));
        assert_eq!(icon_path(r"C:\x\app.ico"), None, "图标资源不是可执行文件");
        assert_eq!(icon_path(""), None);
    }

    #[test]
    fn catalog_never_lists_a_client_twice() {
        let mut seen = std::collections::HashSet::new();
        for (name, exe) in CATALOG {
            assert!(seen.insert(name), "显示名重复：{name}");
            assert!(exe.to_lowercase().ends_with(".exe"), "进程名要带 .exe：{exe}");
        }
        assert!(scan().iter().all(|player| player.path.is_file()), "列表里不能出现已经不存在的路径");
    }

    /// 只在本机手动跑：确认注册表确实能读出装好的播放器。
    #[test]
    #[ignore = "读本机注册表，结果随机器而变"]
    fn live_scan_prints_installed_players() {
        for player in scan() {
            println!("{} | {} | {}", player.name, player.exe, player.path.display());
        }
    }
}
