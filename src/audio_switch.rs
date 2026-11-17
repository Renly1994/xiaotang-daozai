//! 音频设备一键切换：枚举播放/录音端点、读当前默认设备、切默认设备、读音量与静音。
//!
//! 设备名走注册表 MMDevices 的 DeviceDesc，不额外引入 windows crate feature；
//! 切换默认设备走系统自带的 IPolicyConfig（与「声音设置」里点选等效）。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Output,
    Input,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Device {
    /// 端点 id（切换时原样回传）。
    pub id: String,
    pub name: String,
}

#[derive(Clone, Default, PartialEq, Debug)]
pub struct AudioState {
    pub outputs: Vec<Device>,
    pub inputs: Vec<Device>,
    pub current_output: Option<String>,
    pub current_input: Option<String>,
    /// 默认输出的主音量 0..100。
    pub volume: u32,
    pub muted: bool,
    /// 默认输入是否静音。
    pub mic_muted: bool,
    /// 默认输出设备是否是耳机/头戴（用于岛上的图标）。
    pub headphones: bool,
}

impl AudioState {
    pub fn current_output_name(&self) -> String {
        self.current_output
            .as_ref()
            .and_then(|id| self.outputs.iter().find(|device| &device.id == id))
            .map(|device| device.name.clone())
            .unwrap_or_else(|| "未找到设备".into())
    }
    pub fn current_input_name(&self) -> String {
        self.current_input
            .as_ref()
            .and_then(|id| self.inputs.iter().find(|device| &device.id == id))
            .map(|device| device.name.clone())
            .unwrap_or_else(|| "未找到设备".into())
    }
}

/// 读一次完整状态（含设备清单）。设备清单不长，切页时调一次即可。
pub fn state() -> AudioState {
    #[cfg(windows)]
    {
        win::state()
    }
    #[cfg(not(windows))]
    {
        AudioState::default()
    }
}

/// 把默认输出/输入切到指定端点 id。
pub fn set_default(kind: Kind, id: &str) -> bool {
    #[cfg(windows)]
    {
        win::set_default(kind, id)
    }
    #[cfg(not(windows))]
    {
        let _ = (kind, id);
        false
    }
}

/// 设默认输出音量（0..100）。
pub fn set_volume(percent: u32) -> bool {
    #[cfg(windows)]
    {
        win::set_volume(percent)
    }
    #[cfg(not(windows))]
    {
        let _ = percent;
        false
    }
}

/// 切换默认输出静音。
pub fn set_mute(muted: bool) -> bool {
    #[cfg(windows)]
    {
        win::set_mute(muted)
    }
    #[cfg(not(windows))]
    {
        let _ = muted;
        false
    }
}

/// 切换默认输入（麦克风）静音。
pub fn set_mic_mute(muted: bool) -> bool {
    #[cfg(windows)]
    {
        win::set_mic_mute(muted)
    }
    #[cfg(not(windows))]
    {
        let _ = muted;
        false
    }
}

#[cfg(windows)]
mod win {
    use super::{AudioState, Device, Kind};
    use std::ffi::c_void;
    use windows::core::PWSTR;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, eRender, IMMDevice, IMMDeviceCollection, IMMDeviceEnumerator, MMDeviceEnumerator,
        DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    thread_local! {
        static ENUMERATOR: Option<IMMDeviceEnumerator> = unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()
        };
    }

    fn enumerator() -> Option<IMMDeviceEnumerator> {
        ENUMERATOR.with(|enumerator| enumerator.clone())
    }

    pub fn state() -> AudioState {
        let Some(enumerator) = enumerator() else { return AudioState::default() };
        let mut state = AudioState::default();
        state.outputs = list(&enumerator, eRender);
        state.inputs = list(&enumerator, eCapture);
        state.current_output = default_id(&enumerator, eRender);
        state.current_input = default_id(&enumerator, eCapture);
        if let Some((volume, muted)) = endpoint_volume(&enumerator, eRender) {
            state.volume = volume;
            state.muted = muted;
        }
        if let Some((_, muted)) = endpoint_volume(&enumerator, eCapture) {
            state.mic_muted = muted;
        }
        state.headphones = state
            .current_output_name()
            .to_lowercase()
            .contains("耳机");
        state
    }

    fn list(enumerator: &IMMDeviceEnumerator, flow: windows::Win32::Media::Audio::EDataFlow) -> Vec<Device> {
        let Ok(collection) = (unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }) else {
            return Vec::new();
        };
        let count = unsafe { collection.GetCount() }.unwrap_or(0);
        let mut out = Vec::with_capacity(count as usize);
        for index in 0..count {
            let Ok(device) = (unsafe { collection.Item(index) }) else { continue };
            let Some(id) = device_id(&device) else { continue };
            // 只保留真实存在的端点，跳过多余的虚拟线（名字里带 @ 的驱动占位）。
            let name = friendly_name(&id).unwrap_or_else(|| id.clone());
            out.push(Device { id, name });
        }
        out
    }

    fn device_id(device: &IMMDevice) -> Option<String> {
        let raw: PWSTR = unsafe { device.GetId() }.ok()?;
        let text = unsafe { raw.to_string() }.ok()?;
        Some(text)
    }

    fn default_id(
        enumerator: &IMMDeviceEnumerator,
        flow: windows::Win32::Media::Audio::EDataFlow,
    ) -> Option<String> {
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }.ok()?;
        device_id(&device)
    }

    /// 主音量与静音（0..100）。
    fn endpoint_volume(
        enumerator: &IMMDeviceEnumerator,
        flow: windows::Win32::Media::Audio::EDataFlow,
    ) -> Option<(u32, bool)> {
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }.ok()?;
        let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }.ok()?;
        let scalar = unsafe { volume.GetMasterVolumeLevelScalar() }.ok()?;
        let muted = unsafe { volume.GetMute() }.ok()?.as_bool();
        Some(((((scalar * 100.0).round() as u32).min(100)), muted))
    }

    fn with_default_volume(flow: windows::Win32::Media::Audio::EDataFlow, apply: impl Fn(&IAudioEndpointVolume)) -> bool {
        let Some(enumerator) = enumerator() else { return false };
        let Ok(device) = (unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }) else {
            return false;
        };
        let Ok(volume) = (unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }) else {
            return false;
        };
        apply(&volume);
        true
    }

    pub fn set_volume(percent: u32) -> bool {
        let scalar = (percent.min(100) as f32) / 100.0;
        with_default_volume(eRender, |volume| unsafe {
            let _ = volume.SetMasterVolumeLevelScalar(scalar, std::ptr::null());
        })
    }

    pub fn set_mute(muted: bool) -> bool {
        with_default_volume(eRender, |volume| unsafe {
            let _ = volume.SetMute(muted, std::ptr::null());
        })
    }

    pub fn set_mic_mute(muted: bool) -> bool {
        with_default_volume(eCapture, |volume| unsafe {
            let _ = volume.SetMute(muted, std::ptr::null());
        })
    }

    // ---- 设备友好名：读 MMDevices 注册表，避免为 IPropertyStore 增开 feature ----
    fn friendly_name(id: &str) -> Option<String> {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
        };
        // 端点 id 形如 {0.0.0.00000000}.{guid}，注册表只用后半段 GUID。
        let guid = id.rsplit('{').next()?.trim_end_matches('}');
        let is_input = id.starts_with("{0.0.1.");
        let flow = if is_input { "Capture" } else { "Render" };
        let path = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\{flow}\\{{{guid}}}\\Properties");
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut key = HKEY::default();
        let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(wide.as_ptr()), 0, KEY_READ, &mut key) };
        if status.is_err() {
            return None;
        }
        // PKEY_Device_DeviceDesc 比 FriendlyName 更稳定（不受「默认设备」后缀影响）。
        let names = [
            "{b3f8fa53-0004-438e-9003-51a46e139bfc},6",
            "{a45c254e-df1c-4efd-8020-67d146a850e0},2",
        ];
        let mut found = None;
        for name in names {
            let value: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut kind = REG_SZ;
            let mut bytes = 0u32;
            if unsafe {
                RegQueryValueExW(key, PCWSTR(value.as_ptr()), None, Some(&mut kind), None, Some(&mut bytes))
            }
            .is_err()
                || bytes == 0
            {
                continue;
            }
            let mut buffer = vec![0u16; bytes as usize / 2 + 1];
            if unsafe {
                RegQueryValueExW(
                    key,
                    PCWSTR(value.as_ptr()),
                    None,
                    Some(&mut kind),
                    Some(buffer.as_mut_ptr() as *mut u8),
                    Some(&mut bytes),
                )
            }
            .is_err()
            {
                continue;
            }
            let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
            let text = String::from_utf16_lossy(&buffer[..end]).trim().to_string();
            if !text.is_empty() {
                found = Some(short_name(&text));
                break;
            }
        }
        unsafe { let _ = RegCloseKey(key); };
        found
    }

    /// 「扬声器 (Realtek(R) Audio)」→「扬声器 · Realtek」，太长的主板型号直接截断。
    fn short_name(raw: &str) -> String {
        let cleaned = raw.trim();
        match (cleaned.find('('), cleaned.find(')')) {
            (Some(open), Some(close)) if close > open => {
                let head = cleaned[..open].trim();
                let tail = cleaned[open + 1..close].trim();
                let tail = tail.split(['(', ')']).next().unwrap_or(tail).trim();
                if head.is_empty() {
                    return tail.to_string();
                }
                if tail.is_empty() {
                    return head.to_string();
                }
                format!("{head} · {tail}")
            }
            _ => cleaned.to_string(),
        }
    }

    // ---- 切换默认设备：IPolicyConfig ----
    #[repr(C)]
    struct GuidRaw {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const CLSID_POLICY_CONFIG: GuidRaw = GuidRaw {
        data1: 0x870af99c,
        data2: 0x171d,
        data3: 0x4f9e,
        data4: [0xaf, 0x0d, 0xe6, 0x3d, 0xf4, 0x0c, 0x2b, 0xc9],
    };
    const IID_POLICY_CONFIG: GuidRaw = GuidRaw {
        data1: 0xf8679f50,
        data2: 0x850a,
        data3: 0x41cf,
        data4: [0x9c, 0x72, 0x43, 0x0f, 0x29, 0x02, 0x90, 0xc8],
    };

    #[link(name = "ole32")]
    extern "system" {
        #[link_name = "CoCreateInstance"]
        fn CoCreateInstanceRaw(
            rclsid: *const GuidRaw,
            outer: *mut c_void,
            context: u32,
            riid: *const GuidRaw,
            out: *mut *mut c_void,
        ) -> i32;
    }

    /// vtable 里 SetDefaultEndpoint 是 IUnknown(3) 之后第 11 个方法 → 索引 13。
    const SET_DEFAULT_ENDPOINT: usize = 13;
    const RELEASE: usize = 2;

    pub fn set_default(kind: Kind, id: &str) -> bool {
        let _ = kind; // 端点 id 自己带流向，不需要额外区分。
        unsafe {
            let mut object: *mut c_void = std::ptr::null_mut();
            let status = CoCreateInstanceRaw(
                &CLSID_POLICY_CONFIG,
                std::ptr::null_mut(),
                23, // CLSCTX_ALL
                &IID_POLICY_CONFIG,
                &mut object,
            );
            if status < 0 || object.is_null() {
                return false;
            }
            let vtable = *(object as *mut *const usize);
            let set_default: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32 =
                std::mem::transmute(*vtable.add(SET_DEFAULT_ENDPOINT));
            let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
            // 三个角色都设，游戏/语音/普通播放才不会各走各的。
            let mut ok = true;
            for role in 0..3 {
                if set_default(object, wide.as_ptr(), role) < 0 {
                    ok = false;
                }
            }
            let release: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtable.add(RELEASE));
            release(object);
            ok
        }
    }

    /// 供后续扩展：把当前默认设备名里的关键型号抽出来做图标判断。
    #[allow(dead_code)]
    pub fn is_headphones(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.contains("耳机") || lower.contains("headphone") || lower.contains("headset")
    }

    #[allow(dead_code)]
    fn unused(_: &IMMDeviceCollection) {}
}
#[cfg(test)]
mod probe {
    #[test]
    fn audio_snapshot() {
        let state = super::state();
        println!("音量和静音 = {} / {}", state.volume, state.muted);
        println!("麦克风静音 = {}", state.mic_muted);
        println!("当前输出 = {}", state.current_output_name());
        println!("当前输入 = {}", state.current_input_name());
        for device in &state.outputs {
            println!("输出端点: {}", device.name);
        }
        for device in &state.inputs {
            println!("输入端点: {}", device.name);
        }
    }

    #[test]
    fn monitor_snapshot() {
        let mut prev = None;
        let _ = crate::monitor::read(&mut prev, 0.0);
        std::thread::sleep(std::time::Duration::from_millis(700));
        let sample = crate::monitor::read(&mut prev, 1.0);
        println!("CPU {:?}% {}MHz {}C{}T", sample.cpu, sample.cpu_freq.unwrap_or(0), sample.cores, sample.threads);
        println!("内存 {}/{} GB", sample.mem_used / 1073741824, sample.mem_total / 1073741824);
        if let Some(gpu) = &sample.gpu {
            println!("显卡 {} 占用 {:?} 温度 {:?} 功耗 {:?} 风扇 {:?} 显存 {}/{} MB",
                gpu.name, gpu.util, gpu.temp, gpu.power, gpu.fan, gpu.mem_used / 1048576, gpu.mem_total / 1048576);
        } else {
            println!("显卡 无数据");
        }
        for disk in &sample.disks {
            println!("磁盘 {} {}/{} GB", disk.letter, disk.used / 1073741824, disk.total / 1073741824);
        }
        let mut reader = crate::monitor::GpuReader::new();
        let _ = reader.read();
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("显卡复采 {:?}", reader.read().map(|g| (g.util, g.temp, g.power, g.fan)));
    }

    #[test]
    fn quick_snapshot() {
        let panel = crate::quick_ctl::slow_state();
        println!("音量 {} 静音 {} 麦克风 {}", panel.volume, panel.muted, panel.mic_muted);
        println!("WiFi {} SSID {}", panel.wifi_on, panel.wifi_ssid);
        println!("蓝牙 {} 勿扰 {} 亮度 {:?}", panel.bluetooth_on, panel.dnd, panel.brightness);
    }
}
