//! 播放器的音量：优先控制它自己的音频会话（等同 Windows 音量合成器里那一条，
//! 也就是客户端自己那颗音量条），客户端还没建立音频会话时退回系统主音量。

/// 一次音量读数：百分比（0..100）与静音状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppVolume {
    pub percent: u32,
    pub muted: bool,
}

impl AppVolume {
    pub fn new(percent: u32, muted: bool) -> Self {
        Self { percent: percent.min(100), muted }
    }
    /// 给提示气泡用：静音时说明静音，否则给百分比。
    pub fn label(&self) -> String {
        if self.muted { "静音".into() } else { format!("{}%", self.percent) }
    }
}

/// 滑条位置（0..1）换算成音量百分比。
pub fn percent_from_fraction(fraction: f32) -> u32 {
    (fraction.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// 滚轮一格的步长（单位：百分比）。
pub const WHEEL_STEP: f32 = 4.0;

/// 在当前音量上叠加滚轮增量，结果夹在 0..100。
pub fn shift_percent(percent: u32, steps: f32) -> u32 {
    ((percent as f32 + steps * WHEEL_STEP).clamp(0.0, 100.0)).round() as u32
}

/// 读播放器当前音量；找不到该应用的音频会话时读系统主音量。
pub fn read_for(exe: &str) -> Option<AppVolume> {
    #[cfg(windows)]
    {
        if let Some(volume) = platform::read(exe) {
            return Some(volume);
        }
    }
    let _ = exe;
    crate::system::volume().map(|(percent, muted)| AppVolume::new(percent, muted))
}

/// 设置播放器音量并解除静音；返回设置后的读数。
pub fn set_for(exe: &str, percent: u32) -> Option<AppVolume> {
    #[cfg(windows)]
    {
        if let Some(volume) = platform::set_percent(exe, percent) {
            return Some(volume);
        }
    }
    let _ = exe;
    #[cfg(windows)]
    {
        crate::system::set_volume(percent);
        return crate::system::volume().map(|(percent, muted)| AppVolume::new(percent, muted));
    }
    #[cfg(not(windows))]
    {
        let _ = percent;
        None
    }
}

/// 切换播放器的静音；返回切换后的读数。
pub fn toggle_mute_for(exe: &str) -> Option<AppVolume> {
    #[cfg(windows)]
    {
        if let Some(volume) = platform::toggle_mute(exe) {
            return Some(volume);
        }
    }
    let _ = exe;
    #[cfg(windows)]
    {
        let muted = crate::system::toggle_mute()?;
        let percent = crate::system::volume().map(|(percent, _)| percent).unwrap_or(0);
        return Some(AppVolume::new(percent, muted));
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
mod platform {
    use super::AppVolume;
    use windows::core::{Interface, PWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
        ISimpleAudioVolume, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// 每个线程一次 COM 初始化：WASAPI 会话枚举要求调用线程已进入套间。
    fn init() {
        thread_local! {static DONE:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
        DONE.with(|done| {
            if !done.replace(true) {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }
            }
        });
    }

    fn exe_name(pid: u32) -> String {
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return String::new();
            };
            let mut path = [0u16; 1024];
            let mut length = path.len() as u32;
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut length,
            );
            let _ = CloseHandle(process);
            if result.is_err() {
                return String::new();
            }
            let full = String::from_utf16_lossy(&path[..length as usize]);
            full.rsplit('\\').next().unwrap_or(&full).to_string()
        }
    }

    /// 取该可执行文件的所有音频会话。网易云这类客户端会同时挂两条会话，都要一起改，
    /// 只改一条会出现「拖了没反应」。
    fn sessions_for(exe: &str) -> Vec<ISimpleAudioVolume> {
        let mut found = Vec::new();
        if exe.trim().is_empty() {
            return found;
        }
        init();
        unsafe {
            let Ok(enumerator) =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            else {
                return found;
            };
            let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) else {
                return found;
            };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                return found;
            };
            let Ok(list) = manager.GetSessionEnumerator() else {
                return found;
            };
            let Ok(count) = list.GetCount() else {
                return found;
            };
            let mut names: std::collections::HashMap<u32, String> = Default::default();
            for index in 0..count {
                let Ok(control) = list.GetSession(index) else { continue };
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
                let pid = control2.GetProcessId().unwrap_or(0);
                let name = names.entry(pid).or_insert_with(|| exe_name(pid));
                if name.eq_ignore_ascii_case(exe) {
                    if let Ok(volume) = control.cast::<ISimpleAudioVolume>() {
                        found.push(volume);
                    }
                }
            }
        }
        found
    }

    pub(super) fn read(exe: &str) -> Option<AppVolume> {
        let sessions = sessions_for(exe);
        let first = sessions.first()?;
        unsafe {
            let percent = (first.GetMasterVolume().ok()? * 100.0).round() as u32;
            let muted = sessions
                .iter()
                .all(|session| session.GetMute().map(|m| m.as_bool()).unwrap_or(false));
            Some(AppVolume::new(percent, muted))
        }
    }

    pub(super) fn set_percent(exe: &str, percent: u32) -> Option<AppVolume> {
        let sessions = sessions_for(exe);
        if sessions.is_empty() {
            return None;
        }
        let level = percent.min(100) as f32 / 100.0;
        unsafe {
            for session in &sessions {
                let _ = session.SetMasterVolume(level, std::ptr::null());
                let _ = session.SetMute(false, std::ptr::null());
            }
        }
        Some(AppVolume::new(percent, false))
    }

    pub(super) fn toggle_mute(exe: &str) -> Option<AppVolume> {
        let sessions = sessions_for(exe);
        let first = sessions.first()?;
        unsafe {
            let muted = first.GetMute().ok()?.as_bool();
            let want = !muted;
            for session in &sessions {
                let _ = session.SetMute(want, std::ptr::null());
            }
            let percent = (first.GetMasterVolume().ok()? * 100.0).round() as u32;
            Some(AppVolume::new(percent, want))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_maps_onto_percent_bounds() {
        assert_eq!(percent_from_fraction(0.0), 0);
        assert_eq!(percent_from_fraction(0.5), 50);
        assert_eq!(percent_from_fraction(1.0), 100);
        assert_eq!(percent_from_fraction(-0.4), 0, "拖到滑条左边不能出负数");
        assert_eq!(percent_from_fraction(2.0), 100, "拖出右边按满音量算");
    }

    #[test]
    fn wheel_steps_stay_inside_the_range() {
        assert_eq!(shift_percent(50, 1.0), 54);
        assert_eq!(shift_percent(50, -1.0), 46);
        assert_eq!(shift_percent(98, 3.0), 100, "不能超过 100");
        assert_eq!(shift_percent(1, -3.0), 0, "不能低于 0");
    }

    #[test]
    fn volume_label_reads_naturally() {
        assert_eq!(AppVolume::new(62, false).label(), "62%");
        assert_eq!(AppVolume::new(62, true).label(), "静音");
        assert_eq!(AppVolume::new(180, false).percent, 100, "读数超过 100 要夹住");
    }
}
