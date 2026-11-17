//! 全局快捷控制：音量、静音、麦克风、亮度、Wi-Fi、蓝牙、勿扰、截图、录屏。
//!
//! 慢查询（Wi-Fi / 蓝牙 / 亮度）统一放在后台线程低频刷新，界面只读快照，
//! 点击动作在调用线程直接下发，不阻塞绘制。

use std::sync::mpsc::{self, Receiver, TrySendError};
use std::time::Duration;

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Panel {
    /// 默认输出主音量 0..100。
    pub volume: u32,
    pub muted: bool,
    pub mic_muted: bool,
    /// Wi-Fi 是否连上（含 SSID）。
    pub wifi_on: bool,
    pub wifi_ssid: String,
    pub bluetooth_on: bool,
    pub dnd: bool,
    /// 主显示器亮度 0..100；不支持 DDC/CI 时为 None。
    pub brightness: Option<u32>,
}

/// 启动状态刷新线程（音量 1.2s，其余 6s 一轮），界面每帧 try_recv。
pub fn start(ctx: eframe::egui::Context) -> Receiver<Panel> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("daozai-quickctl".into())
        .spawn(move || {
            let mut slow: Option<(Panel, std::time::Instant)> = None;
            let mut delivered: Option<Panel> = None;
            loop {
                let mut panel = fast();
                let stale = slow
                    .as_ref()
                    .map_or(true, |(_, at)| at.elapsed() >= Duration::from_secs(6));
                if stale {
                    let extra = slow_state();
                    panel.wifi_on = extra.wifi_on;
                    panel.wifi_ssid = extra.wifi_ssid.clone();
                    panel.bluetooth_on = extra.bluetooth_on;
                    panel.dnd = extra.dnd;
                    panel.brightness = extra.brightness;
                    slow = Some((extra, std::time::Instant::now()));
                } else if let Some((cached, _)) = &slow {
                    panel.wifi_on = cached.wifi_on;
                    panel.wifi_ssid = cached.wifi_ssid.clone();
                    panel.bluetooth_on = cached.bluetooth_on;
                    panel.dnd = cached.dnd;
                    panel.brightness = cached.brightness;
                }
                if delivered.as_ref() != Some(&panel) {
                    match tx.try_send(panel.clone()) {
                        Ok(()) => {
                            delivered = Some(panel);
                            ctx.request_repaint();
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                        Err(TrySendError::Full(_)) => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(1200));
            }
        })
        .ok();
    rx
}

/// 快查询：音量与两路静音。
pub fn fast() -> Panel {
    let state = crate::audio_switch::state();
    Panel {
        volume: state.volume,
        muted: state.muted,
        mic_muted: state.mic_muted,
        ..Default::default()
    }
}

/// 慢查询：Wi-Fi / 蓝牙 / 勿扰 / 亮度。
pub fn slow_state() -> Panel {
    #[cfg(windows)]
    {
        win::slow_state()
    }
    #[cfg(not(windows))]
    {
        Panel::default()
    }
}

pub fn set_volume(percent: u32) {
    crate::audio_switch::set_volume(percent);
}
pub fn toggle_mute() -> bool {
    let now = crate::audio_switch::state().muted;
    crate::audio_switch::set_mute(!now);
    !now
}
pub fn toggle_mic() -> bool {
    let now = crate::audio_switch::state().mic_muted;
    crate::audio_switch::set_mic_mute(!now);
    !now
}
pub fn set_dnd(on: bool) -> bool {
    #[cfg(windows)]
    {
        win::set_dnd(on)
    }
    #[cfg(not(windows))]
    {
        let _ = on;
        false
    }
}
pub fn toggle_wifi(on: bool) -> bool {
    #[cfg(windows)]
    {
        win::toggle_wifi(on)
    }
    #[cfg(not(windows))]
    {
        let _ = on;
        false
    }
}
pub fn toggle_bluetooth(on: bool) -> bool {
    #[cfg(windows)]
    {
        win::toggle_bluetooth(on)
    }
    #[cfg(not(windows))]
    {
        let _ = on;
        false
    }
}
pub fn set_brightness(value: u32) -> bool {
    #[cfg(windows)]
    {
        win::set_brightness(value)
    }
    #[cfg(not(windows))]
    {
        let _ = value;
        false
    }
}

/// 亮度下发走独立线程：DDC/CI 或 WMI 单次就可能是几百毫秒，绝不能在绘制线程里等。
/// 拖动会连续下发，队列只保留最新一次，天然合并。
pub fn request_brightness(percent: u32) {
    static TX: std::sync::OnceLock<mpsc::SyncSender<u32>> = std::sync::OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<u32>(1);
        let _ = std::thread::Builder::new()
            .name("daozai-bright".into())
            .spawn(move || {
                let mut retry_at = std::time::Instant::now();
                while let Ok(mut target) = rx.recv() {
                    while let Ok(next) = rx.try_recv() {
                        target = next;
                    }
                    // 机器上没有任何可用亮度通道时（本机外接屏无 DDC/CI、也无 WMI 内置屏），
                    // 冷却一段时间再试，免得拖动亮度时每秒拉起好几次 PowerShell。
                    if std::time::Instant::now() < retry_at {
                        continue;
                    }
                    if !set_brightness(target) {
                        retry_at = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    }
                }
            });
        tx
    });
    let _ = tx.try_send(percent);
}

/// 截当前主屏，存到「图片\岛仔截图」。返回保存路径。
/// 现在作为兜底能力保留：快捷控制页的「截图」按钮已改走系统原生截图。
#[allow(dead_code)]
pub fn screenshot() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        win::screenshot()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 拉起 Windows 原生截图工具（等价 Win+Shift+S）：可自由框选、选窗口、调节选区、批注，
/// 截完进剪贴板并在通知里可保存。返回是否成功唤起。
pub fn screen_clip() -> bool {
    #[cfg(windows)]
    {
        win::screen_clip()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 切到相邻虚拟桌面：模拟系统原生 Win+Ctrl+←/→。
/// 不走 IVirtualDesktopManagerInternal——那套私有 COM 每个 Windows 版本都变，一升级就废。
pub fn desktop_switch(next: bool) -> bool {
    #[cfg(windows)]
    {
        win::desktop_switch(next)
    }
    #[cfg(not(windows))]
    {
        let _ = next;
        false
    }
}

/// 一键锁屏：直接调系统 LockWorkStation，不模拟按键、不依赖任何第三方工具。
pub fn lock_screen() -> bool {
    #[cfg(windows)]
    {
        win::lock_screen()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 截图前先让岛仔自己隐退：隐藏本进程所有可见窗口（含毛玻璃底板）→ 拉起系统原生截图 →
/// 等截图工具覆盖层关闭后自动把岛仔收回来，避免岛仔被框进自己的截图里。
pub fn screen_clip_hide_self() -> bool {
    #[cfg(windows)]
    {
        win::screen_clip_hide_self()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 岛仔是否正处于「为截图临时隐退」状态，供毛玻璃底板等模块跳过本帧的显示动作。
pub fn hidden_for_clip() -> bool {
    HIDDEN_FOR_CLIP.load(std::sync::atomic::Ordering::Relaxed)
}

static HIDDEN_FOR_CLIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 发送 Win+Alt+R 触发 Xbox Game Bar 录屏。
pub fn record_toggle() -> bool {
    #[cfg(windows)]
    {
        win::record_toggle()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
mod win {
    use super::Panel;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn powershell(script: &str) -> String {
        // 走 -Command 时命令行会把反引号当转义符吃掉（WinRT 的 IAsyncOperation`1 会坏），
    // 所以落盘成 .ps1 再 -File 执行，顺便带 BOM 保证中文不乱码。
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "daozai-ctl-{}-{}.ps1",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut body = String::from("\u{feff}");
    body.push_str(script);
    if std::fs::write(&path, body).is_err() {
        return String::new();
    }
    let mut command = Command::new("powershell");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&path)
        .creation_flags(CREATE_NO_WINDOW);
    // child_process::spawn 会把 stdout 接到 null，这里要读结果，所以自己起进程。
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let output = match command.spawn() {
        Ok(child) => child.wait_with_output().ok().map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string()).unwrap_or_default(),
        Err(_) => String::new(),
    };
    let _ = std::fs::remove_file(&path);
    output
}

    /// 一次脚本取回 Wi-Fi / 蓝牙 / 勿扰 / SSID。无线电开关状态走 WinRT Radio，
    /// 与「设置 → 网络和 Internet」里那颗开关完全一致。
    const STATE_SCRIPT: &str = r#"
Add-Type -AssemblyName System.Runtime.WindowsRuntime -ErrorAction SilentlyContinue
$null = [Windows.Devices.Radios.Radio,Windows.System.Devices,ContentType=WindowsRuntime]
function Await($t,$ty){ $m=([System.WindowsRuntimeSystemExtensions].GetMethods()|Where-Object{$_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'})[0]; $g=$m.MakeGenericMethod($ty); $n=$g.Invoke($null,@($t)); $n.Wait(-1)|Out-Null; $n.Result }
$wifi = '0'; $bt = '0'; $ssid = ''
try {
  $radios = Await ([Windows.Devices.Radios.Radio]::GetRadiosAsync()) ([System.Collections.Generic.IReadOnlyList[Windows.Devices.Radios.Radio]])
  $wr = $radios | Where-Object { $_.Kind -eq 'WiFi' } | Select-Object -First 1
  $br = $radios | Where-Object { $_.Kind -eq 'Bluetooth' } | Select-Object -First 1
  if ($wr -and $wr.State -eq 'On') { $wifi = '1' }
  if ($br -and $br.State -eq 'On') { $bt = '1' }
} catch {}
if ($wifi -eq '1') {
  $line = netsh wlan show interfaces | Select-String '^\s*SSID\s*:' | Select-Object -First 1
  if ($line) { $ssid = ($line.ToString() -split ':',2)[1].Trim() }
}
$dnd = '0'
try { $v = (Get-ItemProperty -Path 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Notifications\Settings' -Name 'NOC_GLOBAL_SETTING_TOASTS_ENABLED' -ErrorAction Stop).NOC_GLOBAL_SETTING_TOASTS_ENABLED; if ($v -eq 0) { $dnd = '1' } } catch {}
"$wifi`t$ssid`t$bt`t$dnd"
"#;

    pub fn slow_state() -> Panel {
        let mut panel = Panel::default();
        let output = powershell(STATE_SCRIPT);
        let line = output.lines().last().unwrap_or("").trim();
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 {
            panel.wifi_on = parts[0] == "1";
            panel.wifi_ssid = parts[1].to_string();
            panel.bluetooth_on = parts[2] == "1";
            panel.dnd = parts[3] == "1";
        }
        panel.brightness = brightness();
        panel
    }

    pub fn set_dnd(on: bool) -> bool {
        let script = format!(
            "$k='HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings'; if(-not(Test-Path $k)){{New-Item -Path $k -Force|Out-Null}}; try{{ Set-ItemProperty -Path $k -Name 'NOC_GLOBAL_SETTING_TOASTS_ENABLED' -Value {} -Type DWord -ErrorAction Stop; 'ok' }}catch{{ 'fail' }}",
            if on { 0 } else { 1 }
        );
        powershell(&script).contains("ok")
    }

    fn radio_script(kind: &str, state: &str) -> String {
        format!(
            r#"
Add-Type -AssemblyName System.Runtime.WindowsRuntime -ErrorAction SilentlyContinue
try {{
  $null = [Windows.Devices.Radios.Radio,Windows.System.Devices,ContentType=WindowsRuntime]
  function Await($t,$ty){{ $m=([System.WindowsRuntimeSystemExtensions].GetMethods()|Where-Object{{$_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'}})[0]; $g=$m.MakeGenericMethod($ty); $n=$g.Invoke($null,@($t)); $n.Wait(-1)|Out-Null; $n.Result }}
  $radios = Await ([Windows.Devices.Radios.Radio]::GetRadiosAsync()) ([System.Collections.Generic.IReadOnlyList[Windows.Devices.Radios.Radio]])
  $radio = $radios | Where-Object {{ $_.Kind -eq '{kind}' }} | Select-Object -First 1
  if (-not $radio) {{ 'none'; exit }}
  $status = Await ($radio.SetStateAsync([Windows.Devices.Radios.RadioState]::{state})) ([Windows.Devices.Radios.RadioAccessStatus])
  if ($status -eq 'Allowed') {{ 'ok' }} else {{ 'denied' }}
}} catch {{ 'fail' }}
"#
        )
    }

    pub fn toggle_wifi(on: bool) -> bool {
        let state = if on { "On" } else { "Off" };
        powershell(&radio_script("WiFi", state)).contains("ok")
    }

    pub fn toggle_bluetooth(on: bool) -> bool {
        let state = if on { "On" } else { "Off" };
        powershell(&radio_script("Bluetooth", state)).contains("ok")
    }

    // ---- 亮度：DDC/CI ----
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[derive(Clone, Copy)]
    #[repr(C)]
    struct PhysicalMonitor {
        handle: isize,
        description: [u16; 128],
    }

    #[link(name = "user32")]
    extern "system" {
        fn MonitorFromPoint(point: Point, flags: u32) -> isize;
        fn GetDC(hwnd: isize) -> isize;
        fn ReleaseDC(hwnd: isize, dc: isize) -> i32;
        fn GetSystemMetrics(index: i32) -> i32;
    }
    #[link(name = "dxva2")]
    extern "system" {
        fn GetNumberOfPhysicalMonitorsFromHMONITOR(monitor: isize, count: *mut u32) -> i32;
        fn GetPhysicalMonitorsFromHMONITOR(monitor: isize, size: u32, monitors: *mut PhysicalMonitor) -> i32;
        fn GetMonitorBrightness(monitor: isize, min: *mut u32, cur: *mut u32, max: *mut u32) -> i32;
        fn SetMonitorBrightness(monitor: isize, value: u32) -> i32;
        fn DestroyPhysicalMonitor(monitor: isize);
    }

    fn with_monitor(mut apply: impl FnMut(isize) -> bool) -> bool {
        unsafe {
            let monitor = MonitorFromPoint(Point { x: 0, y: 0 }, 1);
            if monitor == 0 {
                return false;
            }
            let mut count = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count) == 0 || count == 0 {
                return false;
            }
            let mut monitors = vec![PhysicalMonitor { handle: 0, description: [0; 128] }; count as usize];
            if GetPhysicalMonitorsFromHMONITOR(monitor, count, monitors.as_mut_ptr()) == 0 {
                return false;
            }
            let mut ok = false;
            for item in &monitors {
                if item.handle != 0 && apply(item.handle) {
                    ok = true;
                }
                DestroyPhysicalMonitor(item.handle);
            }
            ok
        }
    }

    fn brightness() -> Option<u32> {
        let mut value = None;
        with_monitor(|handle: isize| {
            let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
            let ok = unsafe { GetMonitorBrightness(handle, &mut min, &mut cur, &mut max) } != 0;
            if ok && max > min {
                value = Some(((cur.saturating_sub(min)) as f32 / (max - min) as f32 * 100.0).round() as u32);
            }
            ok
        });
        // 外接屏走 DDC/CI；笔记本内置屏大多不支持，再试 WMI 的 WmiMonitorBrightness。
        value.or_else(wmi_brightness)
    }

    /// 内置屏亮度读取（DDC/CI 之外的第二条通道）。
    fn wmi_brightness() -> Option<u32> {
        // 先探一次"这台机器有没有 WMI 亮度接口"并缓存结论：没有内置屏的台式机
        // 不必每轮采样都拉一次 PowerShell，笔记本上则每次都读真实值。
        static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let supported = *SUPPORTED.get_or_init(|| {
            let out = powershell(
                "$b=(Get-CimInstance -Namespace root/wmi -ClassName WmiMonitorBrightness -ErrorAction SilentlyContinue | Select-Object -First 1).CurrentBrightness\nif($null -ne $b){ 1 }",
            );
            out.trim() == "1"
        });
        if !supported {
            return None;
        }
        let out = powershell(
            "$b=(Get-CimInstance -Namespace root/wmi -ClassName WmiMonitorBrightness -ErrorAction SilentlyContinue | Select-Object -First 1).CurrentBrightness\nif($null -ne $b){ [int]$b }",
        );
        out.trim().parse::<u32>().ok().map(|v| v.min(100))
    }

    pub fn set_brightness(percent: u32) -> bool {
        let ddc = with_monitor(|handle| {
            let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
            if unsafe { GetMonitorBrightness(handle, &mut min, &mut cur, &mut max) } == 0 || max <= min {
                return false;
            }
            let target = min + ((max - min) as f32 * (percent.min(100) as f32 / 100.0)) as u32;
            (unsafe { SetMonitorBrightness(handle, target) }) != 0
        });
        // 两条通道都不通才算真失败：外接屏 DDC/CI，笔记本内置屏 WMI。
        ddc || wmi_set_brightness(percent)
    }

    /// 内置屏亮度设置：第二个参数是"等几秒落定"，0 = 立刻生效。
    fn wmi_set_brightness(percent: u32) -> bool {
        let v = percent.min(100);
        let out = powershell(&format!(
            "$m=Get-CimInstance -Namespace root/wmi -ClassName WmiMonitorBrightnessMethods -ErrorAction SilentlyContinue | Select-Object -First 1\nif($null -ne $m){{ $m.WmiSetBrightness(1,{v}); 'ok' }}"
        ));
        out.trim().eq_ignore_ascii_case("ok")
    }

    // ---- 截图：GDI 抓主屏存 PNG ----
    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_ppm: i32,
        y_ppm: i32,
        clr_used: u32,
        clr_important: u32,
    }
    #[repr(C)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 3],
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleDC(dc: isize) -> isize;
        fn CreateCompatibleBitmap(dc: isize, width: i32, height: i32) -> isize;
        fn SelectObject(dc: isize, object: isize) -> isize;
        fn BitBlt(dst: isize, x: i32, y: i32, w: i32, h: i32, src: isize, sx: i32, sy: i32, rop: u32) -> i32;
        fn GetDIBits(dc: isize, bitmap: isize, start: u32, lines: u32, bits: *mut u8, info: *mut BitmapInfo, usage: u32) -> i32;
        fn DeleteObject(object: isize) -> i32;
        fn DeleteDC(dc: isize) -> i32;
    }

    pub fn screenshot() -> Option<std::path::PathBuf> {
        unsafe {
            let width = GetSystemMetrics(0);
            let height = GetSystemMetrics(1);
            if width <= 0 || height <= 0 {
                return None;
            }
            let screen = GetDC(0);
            let memory = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            if bitmap == 0 {
                ReleaseDC(0, screen);
                DeleteDC(memory);
                return None;
            }
            let old = SelectObject(memory, bitmap);
            let copied = BitBlt(memory, 0, 0, width, height, screen, 0, 0, 0x00CC_0020);
            let mut buffer = vec![0u8; (width * height * 4) as usize];
            let mut info = BitmapInfo {
                header: BitmapInfoHeader {
                    size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    width,
                    height: -height,
                    planes: 1,
                    bit_count: 32,
                    compression: 0,
                    size_image: 0,
                    x_ppm: 0,
                    y_ppm: 0,
                    clr_used: 0,
                    clr_important: 0,
                },
                colors: [0; 3],
            };
            let lines = if copied != 0 { GetDIBits(screen, bitmap, 0, height as u32, buffer.as_mut_ptr(), &mut info, 0) } else { 0 };
            SelectObject(memory, old);
            DeleteObject(bitmap);
            DeleteDC(memory);
            ReleaseDC(0, screen);
            if lines == 0 {
                return None;
            }
            for pixel in buffer.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            let image = image::RgbaImage::from_raw(width as u32, height as u32, {
                let mut rgba = Vec::with_capacity(buffer.len());
                for pixel in buffer.chunks_exact(4) {
                    rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                }
                rgba
            })?;
            let folder = image_folder()?;
            let name = format!("岛仔截图_{}.png", chrono::Local::now().format("%Y%m%d_%H%M%S"));
            let path = folder.join(name);
            image.save(&path).ok()?;
            Some(path)
        }
    }

    fn image_folder() -> Option<std::path::PathBuf> {
        let pictures = std::env::var("USERPROFILE").ok().map(|home| std::path::Path::new(&home).join("Pictures"))?;
        let folder = pictures.join("岛仔截图");
        std::fs::create_dir_all(&folder).ok()?;
        Some(folder)
    }

    // ---- 录屏：模拟 Win+Alt+R（Xbox Game Bar） ----
    #[link(name = "user32")]
    extern "system" {
        fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    }

    pub fn record_toggle() -> bool {
        const KEYUP: u32 = 0x0002;
        let (win, alt, r) = (0x5Bu8, 0xA4u8, 0x52u8);
        unsafe {
            keybd_event(win, 0, 0, 0);
            keybd_event(alt, 0, 0, 0);
            keybd_event(r, 0, 0, 0);
            keybd_event(r, 0, KEYUP, 0);
            keybd_event(alt, 0, KEYUP, 0);
            keybd_event(win, 0, KEYUP, 0);
        }
        true
    }

    // ---- 原生截图：优先用 ms-screenclip: 协议拉起「截图工具」，失败再模拟 Win+Shift+S ----
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(hwnd: isize, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
    }

    fn wide(text: &str) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        std::ffi::OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn screen_clip() -> bool {
        // ShellExecuteW 返回值 >32 才算成功，直接拉起系统「截图工具」覆盖层。
        let launched = unsafe {
            let op = wide("open");
            let target = wide("ms-screenclip:");
            ShellExecuteW(0, op.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), 1) as isize > 32
        };
        if launched {
            return true;
        }
        // 兜底：模拟 Win+Shift+S 热键，走系统同一套截图入口。
        const KEYUP: u32 = 0x0002;
        let (win, shift, s) = (0x5Bu8, 0x10u8, 0x53u8);
        unsafe {
            keybd_event(win, 0, 0, 0);
            keybd_event(shift, 0, 0, 0);
            keybd_event(s, 0, 0, 0);
            keybd_event(s, 0, KEYUP, 0);
            keybd_event(shift, 0, KEYUP, 0);
            keybd_event(win, 0, KEYUP, 0);
        }
        true
    }

    // ---- 虚拟桌面切换：模拟 Win+Ctrl+←/→，交给系统自己处理动画与跨桌面 ----
    pub fn desktop_switch(next: bool) -> bool {
        const KEYUP: u32 = 0x0002;
        let (win_key, ctrl) = (0x5Bu8, 0xA2u8);
        // VK_LEFT=0x25 / VK_RIGHT=0x27
        let arrow = if next { 0x27u8 } else { 0x25u8 };
        unsafe {
            keybd_event(win_key, 0, 0, 0);
            keybd_event(ctrl, 0, 0, 0);
            keybd_event(arrow, 0, 0, 0);
            keybd_event(arrow, 0, KEYUP, 0);
            keybd_event(ctrl, 0, KEYUP, 0);
            keybd_event(win_key, 0, KEYUP, 0);
        }
        true
    }

    // ---- 一键锁屏 ----
    #[link(name = "user32")]
    extern "system" {
        fn LockWorkStation() -> i32;
    }

    pub fn lock_screen() -> bool {
        unsafe { LockWorkStation() != 0 }
    }

    // ---- 截图时岛仔自动隐退：先藏自己再拉截图工具，截完自动收回 ----
    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(callback: unsafe extern "system" fn(isize, isize) -> i32, param: isize) -> i32;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn QueryFullProcessImageNameW(process: isize, flags: u32, buffer: *mut u16, size: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    const SW_HIDE: i32 = 0;
    const SW_SHOWNOACTIVATE: i32 = 4;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    /// 扫描状态：本进程 pid、本进程可见窗口、是否出现截图工具窗口。
    struct ClipScan {
        self_pid: u32,
        own: Vec<isize>,
        snipping: bool,
    }

    /// 取进程可执行文件名（小写），用于识别截图工具进程。
    fn process_exe(pid: u32) -> Option<String> {
        unsafe {
            if pid == 0 {
                return None;
            }
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle == 0 {
                return None;
            }
            let mut buffer = [0u16; 512];
            let mut size = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&buffer[..size as usize]);
            Some(path.rsplit('\\').next().unwrap_or_default().to_ascii_lowercase())
        }
    }

    unsafe extern "system" fn scan_own(hwnd: isize, param: isize) -> i32 {
        let scan = &mut *(param as *mut ClipScan);
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == scan.self_pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            scan.own.push(hwnd);
        }
        1
    }

    unsafe extern "system" fn scan_snipping(hwnd: isize, param: isize) -> i32 {
        let scan = &mut *(param as *mut ClipScan);
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 || pid == scan.self_pid {
            return 1;
        }
        if let Some(name) = process_exe(pid) {
            // Win11 的「截图工具 / 截图宿主」两种进程名都算截图进行中。
            if name == "snippingtool.exe" || name == "screenclippinghost.exe" {
                scan.snipping = true;
                return 0;
            }
        }
        1
    }

    pub fn screen_clip_hide_self() -> bool {
        let self_pid = std::process::id();
        // 1) 收齐岛仔自己的可见窗口（岛体 + 毛玻璃底板），先全部藏起来，免得被框进画面。
        let mut scan = ClipScan { self_pid, own: Vec::new(), snipping: false };
        unsafe { EnumWindows(scan_own, &mut scan as *mut _ as isize) };
        let own = scan.own;
        super::HIDDEN_FOR_CLIP.store(true, std::sync::atomic::Ordering::Relaxed);
        for hwnd in &own {
            unsafe { ShowWindow(*hwnd, SW_HIDE) };
        }

        // 2) 叫起系统原生截图。
        let launched = screen_clip();

        // 3) 后台盯着截图工具：覆盖层一出现就记下，等它退场（截完/取消）再把岛仔收回来。
        let handles = own.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut seen_overlay = false;
            // 截图工具没拉起来（协议被禁用/被拦），别让岛仔白躲一场，半秒后直接回来。
            while launched {
                std::thread::sleep(std::time::Duration::from_millis(120));
                // 上限先行：覆盖层若一直不退（卡住/常驻），岛仔也得自己回来，不能长睡不醒。
                if started.elapsed() >= std::time::Duration::from_secs(180) {
                    break;
                }
                let mut probe = ClipScan { self_pid, own: Vec::new(), snipping: false };
                unsafe { EnumWindows(scan_snipping, &mut probe as *mut _ as isize) };
                if probe.snipping {
                    seen_overlay = true;
                    continue;
                }
                // 覆盖层已退场；若迟迟没等到它（工具加载慢），也给个兜底时限。
                if seen_overlay || started.elapsed() >= std::time::Duration::from_secs(6) {
                    break;
                }
            }
            if !launched {
                std::thread::sleep(std::time::Duration::from_millis(320));
            }
            std::thread::sleep(std::time::Duration::from_millis(180));
            for hwnd in &handles {
                unsafe { ShowWindow(*hwnd, SW_SHOWNOACTIVATE) };
            }
            super::HIDDEN_FOR_CLIP.store(false, std::sync::atomic::Ordering::Relaxed);
        });

        launched
    }
}
