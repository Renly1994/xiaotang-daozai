//! 硬件实时监控采样：CPU 使用率与频率、内存、显卡（NVML）、磁盘占用。
//!
//! 采样统一放在一个后台线程里（600ms 一次），只在数值变化时唤醒界面；
//! 显卡走系统自带的 nvml.dll 按需加载，不新增 crate、不新增常驻进程，
//! 未装 NVIDIA 驱动时整块自动隐藏。

use std::sync::mpsc::{self, Receiver, TrySendError};
use std::time::Duration;

#[derive(Clone, Default, PartialEq)]
pub struct Disk {
    pub letter: char,
    pub used: u64,
    pub total: u64,
}

impl Disk {
    pub fn ratio(&self) -> f32 {
        if self.total == 0 { 0.0 } else { self.used as f32 / self.total as f32 }
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct Gpu {
    pub name: String,
    /// 使用率（0..100）。
    pub util: Option<f32>,
    /// 核心温度（摄氏度）。
    pub temp: Option<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    /// 整卡功耗（瓦）。
    pub power: Option<f32>,
    /// 风扇转速（百分比）。
    pub fan: Option<f32>,
}

impl Gpu {
    pub fn mem_ratio(&self) -> f32 {
        if self.mem_total == 0 { 0.0 } else { self.mem_used as f32 / self.mem_total as f32 }
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct Sample {
    pub cpu: Option<f32>,
    pub cpu_freq: Option<u32>,
    pub cores: u32,
    pub threads: u32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub gpu: Option<Gpu>,
    pub disks: Vec<Disk>,
    /// 采样到数据的时间（秒，进程启动起算），用于界面判断数据是否新鲜。
    pub at: f64,
}

impl Sample {
    pub fn mem_ratio(&self) -> f32 {
        if self.mem_total == 0 { 0.0 } else { self.mem_used as f32 / self.mem_total as f32 }
    }
    pub fn cpu_value(&self) -> f32 { self.cpu.unwrap_or(0.0) }
    /// 高负载提醒：返回最需要提示的那一项（CPU / 内存 / 显卡），没有则 None。
    pub fn hot(&self) -> Option<(&'static str, &'static str)> {
        if self.cpu_value() >= 85.0 {
            return Some(("cpu", "CPU 高负载"));
        }
        if self.mem_ratio() >= 0.9 {
            return Some(("mem", "内存吃紧"));
        }
        match &self.gpu {
            Some(gpu) if gpu.util.unwrap_or(0.0) >= 90.0 || gpu.temp.unwrap_or(0.0) >= 80.0 => {
                Some(("gpu", "显卡高负载"))
            }
            _ => None,
        }
    }
}

/// 启动采样线程，返回接收端。界面每帧顺手 try_recv 即可。
pub fn start(ctx: eframe::egui::Context) -> Receiver<Sample> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("daozai-monitor".into())
        .spawn(move || {
            let clock = std::time::Instant::now();
            let mut prev_cpu: Option<(u64, u64, u64)> = None;
            let mut delivered: Option<Sample> = None;
            let mut gpu = GpuReader::new();
            loop {
                let mut sample = read(&mut prev_cpu, clock.elapsed().as_secs_f64());
                sample.gpu = gpu.read();
                if delivered.as_ref() != Some(&sample) {
                    match tx.try_send(sample.clone()) {
                        Ok(()) => {
                            delivered = Some(sample);
                            ctx.request_repaint();
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                        Err(TrySendError::Full(_)) => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(600));
            }
        })
        .ok();
    rx
}

/// 采集一次（不含显卡）。
pub fn read(prev_cpu: &mut Option<(u64, u64, u64)>, at: f64) -> Sample {
    #[cfg(windows)]
    {
        let mut sample = Sample { at, ..Default::default() };
        let (cores, threads) = cpu_counts();
        sample.cores = cores;
        sample.threads = threads;
        sample.cpu = win::cpu_usage(prev_cpu);
        sample.cpu_freq = win::cpu_mhz(cores);
        if let Some((used, total)) = win::memory() {
            sample.mem_used = used;
            sample.mem_total = total;
        }
        sample.disks = win::disks();
        sample
    }
    #[cfg(not(windows))]
    {
        let _ = (prev_cpu, at);
        Sample::default()
    }
}

/// 逻辑核数（线程）与物理核数。
fn cpu_counts() -> (u32, u32) {
    #[cfg(windows)]
    {
        win::cpu_counts()
    }
    #[cfg(not(windows))]
    {
        (0, 0)
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut FILETIME, kernel: *mut FILETIME, user: *mut FILETIME) -> i32;
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            free_for_caller: *mut u64,
            total: *mut u64,
            free_total: *mut u64,
        ) -> i32;
    }
    #[link(name = "powrprof")]
    extern "system" {
        fn CallNtPowerInformation(
            level: i32,
            input: *mut c_void,
            input_len: u32,
            output: *mut c_void,
            output_len: u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MEMORYSTATUSEX) -> i32;
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct FILETIME {
        pub low: u32,
        pub high: u32,
    }
    impl FILETIME {
        fn value(self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }

    #[repr(C)]
    struct MEMORYSTATUSEX {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct PROCESSOR_POWER_INFORMATION {
        number: u32,
        max_mhz: u32,
        current_mhz: u32,
        mhz_limit: u32,
        max_idle_state: u32,
        current_idle_state: u32,
    }

    /// 两次调用之间的忙时占比；首次调用只建立基线。
    pub fn cpu_usage(prev: &mut Option<(u64, u64, u64)>) -> Option<f32> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        // kernel 时间已包含 idle。
        let now = (idle.value(), kernel.value(), user.value());
        let last = prev.replace(now);
        let Some(last) = last else { return None };
        let idle_delta = now.0.saturating_sub(last.0) as f32;
        let total_delta = (now.1.saturating_sub(last.1) + now.2.saturating_sub(last.2)) as f32;
        if total_delta <= 0.0 {
            return None;
        }
        Some(((total_delta - idle_delta) / total_delta * 100.0).clamp(0.0, 100.0))
    }

    /// 当前主频（MHz）：取所有核心的平均值。
    pub fn cpu_mhz(cores: u32) -> Option<u32> {
        let n = cores.max(1) as usize;
        let mut buffer = vec![PROCESSOR_POWER_INFORMATION::default(); n];
        let size = (std::mem::size_of::<PROCESSOR_POWER_INFORMATION>() * n) as u32;
        // 11 = ProcessorInformation
        let status = unsafe {
            CallNtPowerInformation(11, std::ptr::null_mut(), 0, buffer.as_mut_ptr() as *mut c_void, size)
        };
        if status < 0 {
            return base_mhz();
        }
        let sum: u64 = buffer.iter().map(|info| info.current_mhz as u64).sum();
        if sum > 0 {
            return Some((sum / n as u64) as u32);
        }
        base_mhz()
    }

    /// 注册表里的额定频率（MHz），取不到实时主频时兜底。
    fn base_mhz() -> Option<u32> {
        let sub: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\0".encode_utf16().collect();
        let name: Vec<u16> = "~MHz\0".encode_utf16().collect();
        use ::windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
        let mut value: u32 = 0;
        let mut size: u32 = 4;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                ::windows::core::PCWSTR(sub.as_ptr()),
                ::windows::core::PCWSTR(name.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut c_void),
                Some(&mut size),
            )
        };
        (status.is_ok() && value > 0).then_some(value)
    }

    /// 物理核数与逻辑核数。
    pub fn cpu_counts() -> (u32, u32) {
        use ::windows::Win32::System::SystemInformation::{
            GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        };
        let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
        let mut size = 0u32;
        unsafe {
            let _ = GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut size);
        }
        if size == 0 {
            return (threads, threads);
        }
        // 用 u64 分配，保证结构体所需的对齐。
        let mut buffer = vec![0u64; size as usize / 8 + 1];
        let ptr = buffer.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
        if unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, Some(ptr), &mut size) }.is_err() {
            return (threads, threads);
        }
        let mut offset = 0usize;
        let mut cores = 0u32;
        while offset + std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() <= size as usize {
            let entry = unsafe { &*(buffer.as_ptr().add(offset / 8) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX) };
            cores += 1;
            if entry.Size == 0 {
                break;
            }
            offset += entry.Size as usize;
        }
        (cores.max(1), threads.max(cores))
    }

    pub fn memory() -> Option<(u64, u64)> {
        let mut status = MEMORYSTATUSEX {
            length: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return None;
        }
        Some((status.total_phys.saturating_sub(status.avail_phys), status.total_phys))
    }

    /// 固定磁盘的占用（最多取 3 块，够看又不会把界面撑乱）。
    pub fn disks() -> Vec<crate::monitor::Disk> {
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for index in 0..26u32 {
            if mask & (1 << index) == 0 {
                continue;
            }
            let letter = (b'A' + index as u8) as char;
            let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(std::iter::once(0)).collect();
            // 3 = DRIVE_FIXED
            if unsafe { GetDriveTypeW(root.as_ptr()) } != 3 {
                continue;
            }
            let (mut free_for_caller, mut total, mut free_total) = (0u64, 0u64, 0u64);
            if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut free_for_caller, &mut total, &mut free_total) } == 0 {
                continue;
            }
            if total == 0 {
                continue;
            }
            out.push(crate::monitor::Disk { letter, used: total.saturating_sub(free_for_caller), total });
            if out.len() == 3 {
                break;
            }
        }
        out
    }
}

// ---------------------------------------------------------------
// 显卡：nvml.dll 按需加载（GetProcAddress 逐个取函数，缺一个就整块放弃）
// ---------------------------------------------------------------
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct NvUtilization {
    gpu: u32,
    memory: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct NvMemory {
    total: u64,
    free: u64,
    used: u64,
}

struct Nvml {
    _lib: isize,
    handle: unsafe extern "system" fn(u32, *mut *mut std::ffi::c_void) -> i32,
    utilization: unsafe extern "system" fn(*mut std::ffi::c_void, *mut NvUtilization) -> i32,
    temperature: unsafe extern "system" fn(*mut std::ffi::c_void, i32, *mut u32) -> i32,
    memory: unsafe extern "system" fn(*mut std::ffi::c_void, *mut NvMemory) -> i32,
    power: unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32) -> i32,
    fan: unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32) -> i32,
    name: unsafe extern "system" fn(*mut std::ffi::c_void, *mut u8, u32) -> i32,
    device: *mut std::ffi::c_void,
}

unsafe impl Send for Nvml {}

impl Nvml {
    fn load() -> Option<Self> {
        use ::windows::core::PCWSTR;
        use ::windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let path: Vec<u16> = "nvml.dll\0".encode_utf16().collect();
        let lib = unsafe { LoadLibraryW(PCWSTR(path.as_ptr())) }.ok()?;
        let sym = |name: &str| -> Option<usize> {
            let full: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
            unsafe { GetProcAddress(lib, ::windows::core::PCSTR(full.as_ptr())) }.map(|f| f as usize)
        };
        let init: unsafe extern "system" fn() -> i32 = unsafe { std::mem::transmute(sym("nvmlInit_v2")?) };
        if unsafe { init() } != 0 {
            return None;
        }
        let handle: unsafe extern "system" fn(u32, *mut *mut std::ffi::c_void) -> i32 =
            unsafe { std::mem::transmute(sym("nvmlDeviceGetHandleByIndex_v2")?) };
        let mut device: *mut std::ffi::c_void = std::ptr::null_mut();
        if unsafe { handle(0, &mut device) } != 0 || device.is_null() {
            return None;
        }
        Some(Self {
            _lib: lib.0 as isize,
            handle,
            utilization: unsafe { std::mem::transmute(sym("nvmlDeviceGetUtilizationRates")?) },
            temperature: unsafe { std::mem::transmute(sym("nvmlDeviceGetTemperature")?) },
            memory: unsafe { std::mem::transmute(sym("nvmlDeviceGetMemoryInfo")?) },
            power: unsafe { std::mem::transmute(sym("nvmlDeviceGetPowerUsage")?) },
            fan: unsafe { std::mem::transmute(sym("nvmlDeviceGetFanSpeed")?) },
            name: unsafe { std::mem::transmute(sym("nvmlDeviceGetName")?) },
            device,
        })
    }
}

/// 显卡读取器：加载失败后不再反复尝试，避免每分钟都去碰驱动。
pub struct GpuReader {
    inner: Option<Nvml>,
    failed: bool,
    name: String,
}

impl GpuReader {
    pub fn new() -> Self {
        Self { inner: None, failed: false, name: String::new() }
    }

    pub fn read(&mut self) -> Option<Gpu> {
        if self.failed {
            return None;
        }
        if self.inner.is_none() {
            match Nvml::load() {
                Some(nvml) => {
                    let mut buffer = [0u8; 96];
                    let mut name = String::new();
                    if unsafe { (nvml.name)(nvml.device, buffer.as_mut_ptr(), buffer.len() as u32) } == 0 {
                        let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
                        name = String::from_utf8_lossy(&buffer[..end]).trim().to_string();
                        // 「NVIDIA GeForce RTX 5070」在岛上显示太长，去掉厂商标识。
                        name = name.replace("NVIDIA GeForce ", "").replace("NVIDIA ", "");
                    }
                    self.name = name;
                    self.inner = Some(nvml);
                }
                None => {
                    self.failed = true;
                    return None;
                }
            }
        }
        let nvml = self.inner.as_ref()?;
        let mut gpu = Gpu { name: self.name.clone(), ..Default::default() };
        let mut utilization = NvUtilization::default();
        if unsafe { (nvml.utilization)(nvml.device, &mut utilization) } == 0 {
            gpu.util = Some(utilization.gpu as f32);
        }
        let mut temp = 0u32;
        // 0 = NVML_TEMPERATURE_GPU
        if unsafe { (nvml.temperature)(nvml.device, 0, &mut temp) } == 0 {
            gpu.temp = Some(temp as f32);
        }
        let mut memory = NvMemory::default();
        if unsafe { (nvml.memory)(nvml.device, &mut memory) } == 0 && memory.total > 0 {
            gpu.mem_used = memory.used;
            gpu.mem_total = memory.total;
        }
        let mut power = 0u32;
        if unsafe { (nvml.power)(nvml.device, &mut power) } == 0 && power > 0 {
            // 毫瓦 → 瓦
            gpu.power = Some(power as f32 / 1000.0);
        }
        let mut fan = 0u32;
        if unsafe { (nvml.fan)(nvml.device, &mut fan) } == 0 {
            gpu.fan = Some(fan as f32);
        }
        let _ = nvml.handle;
        Some(gpu)
    }
}

#[cfg(test)]
mod nvml_probe {
    #[test]
    fn steps() {
        use ::windows::core::PCSTR;
        use ::windows::Win32::System::LibraryLoader::GetProcAddress;
        use ::windows::Win32::System::LibraryLoader::LoadLibraryW;
        let path: Vec<u16> = "nvml.dll\0".encode_utf16().collect();
        let lib = unsafe { LoadLibraryW(::windows::core::PCWSTR(path.as_ptr())) };
        println!("loadlib = {:?}", lib.is_ok());
        let lib = match lib {
            Ok(lib) => lib,
            Err(error) => {
                println!("loadlib err = {error:?}");
                return;
            }
        };
        let mut missing: Vec<&str> = Vec::new();
        for name in [
            "nvmlInit_v2",
            "nvmlDeviceGetHandleByIndex_v2",
            "nvmlDeviceGetUtilizationRates",
            "nvmlDeviceGetTemperature",
            "nvmlDeviceGetMemoryInfo",
            "nvmlDeviceGetPowerUsage",
            "nvmlDeviceGetFanSpeed",
            "nvmlDeviceGetName",
        ] {
            let full: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
            if unsafe { GetProcAddress(lib, PCSTR(full.as_ptr())) }.is_none() {
                missing.push(name);
            }
        }
        println!("missing = {missing:?}");
        println!("reader = {:?}", super::GpuReader::new().read().map(|g| (g.name, g.util, g.temp)));
    }
}