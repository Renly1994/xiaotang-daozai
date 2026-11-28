//! 模块开启时独立保留最近一分钟的采样，界面可见时才请求重绘。
use eframe::egui;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Metric {
    Cpu, Gpu, Ram, Vram, GpuTemp, Download, Upload, Read, Write,
}

impl Metric {
    pub(crate) const ALL: [Self; 9] = [
        Self::Cpu, Self::Gpu, Self::Ram, Self::Vram, Self::GpuTemp,
        Self::Download, Self::Upload, Self::Read, Self::Write,
    ];
    pub(crate) const RATE_PAIRS: [[Self; 2]; 2] = [
        [Self::Download, Self::Upload],
        [Self::Read, Self::Write],
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU", Self::Gpu => "GPU", Self::Ram => "内存", Self::Vram => "显存",
            Self::GpuTemp => "显卡温度", Self::Download => "下载", Self::Upload => "上传",
            Self::Read => "磁盘读", Self::Write => "磁盘写",
        }
    }
    pub(crate) fn bit(self) -> u16 { 1 << self as u16 }
    pub(crate) fn rate_label(self) -> Option<&'static str> {
        match self {
            Self::Download => Some("↓"),
            Self::Upload => Some("↑"),
            Self::Read => Some("读"),
            Self::Write => Some("写"),
            _ => None,
        }
    }
    pub(crate) fn view(self) -> View {
        match self {
            Self::Cpu=>View::Cpu, Self::Gpu|Self::Vram|Self::GpuTemp=>View::Gpu,
            Self::Ram=>View::Memory, Self::Download|Self::Upload=>View::Network,
            Self::Read|Self::Write=>View::Disk,
        }
    }
    fn width(self) -> f32 {
        match self {
            Self::Cpu | Self::Gpu | Self::Ram => 52.,
            Self::Vram | Self::GpuTemp => 68.,
            _ => BarStyle::RATE_WIDTH,
        }
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarStyle {
    #[default]
    Gauges,
    Pulse,
    Network,
    Dashboard,
    Custom,
    Cores,
}
impl BarStyle {
    pub(crate) const RATE_WIDTH: f32 = 66.;
    pub(crate) fn value_columns(mask: u16) -> impl Iterator<Item = (Metric, Option<Metric>, f32)> {
        let mut metrics = Metric::ALL.into_iter().filter(move |m| mask & m.bit() != 0 && m.rate_label().is_none());
        let stacked = mask.count_ones() > 3;
        std::iter::from_fn(move || {
            let first = metrics.next()?;
            let second = if stacked { metrics.next() } else { None };
            let width = second.map_or(first.width(), |m| first.width().max(m.width()));
            Some((first, second, width))
        })
    }
    pub(crate) fn rate_columns(mask: u16) -> impl Iterator<Item = [Option<Metric>; 2]> {
        Metric::RATE_PAIRS.into_iter()
            .map(move |pair| pair.map(|m| (mask & m.bit() != 0).then_some(m)))
            .filter(|pair| pair.iter().any(Option::is_some))
    }
    /// 侧边停靠时占的高度：和横条同样的内容竖着排。
    pub(crate) fn side_height(self, mask: u16) -> f32 {
        match self {
            Self::Custom => {
                let values = Metric::ALL.into_iter().filter(|m| mask & m.bit() != 0 && m.rate_label().is_none()).count();
                (values + Self::rate_columns(mask).count()) as f32 * 23.
            }
            Self::Gauges => 90.,
            Self::Dashboard => 158.,
            Self::Cores | Self::Pulse => 46.,
            Self::Network => 34.,
        }
    }
    pub(crate) const ALL: [Self; 6] = [Self::Custom, Self::Gauges, Self::Cores, Self::Pulse, Self::Network, Self::Dashboard];
    pub(crate) fn width(self, mask: u16) -> f32 {
        match self {
            Self::Custom => Self::value_columns(mask).map(|(_, _, w)| w).sum::<f32>()
                + Self::rate_columns(mask).count() as f32 * Self::RATE_WIDTH,
            Self::Gauges => 146.,
            Self::Pulse => 78.,
            Self::Cores => 104.,
            Self::Network => 88.,
            Self::Dashboard => 308.,
        }
    }
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Gauges => "gauges",
            Self::Pulse => "pulse",
            Self::Network => "network",
            Self::Dashboard => "dashboard",
            Self::Custom => "custom",
            Self::Cores => "cores",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Gauges => "三联环",
            Self::Pulse => "CPU 脉冲",
            Self::Network => "上下行",
            Self::Dashboard => "综合监测",
            Self::Custom => "自由搭配",
            Self::Cores => "核心点阵",
        }
    }
    /// 设置里样式卡片上的一句话概括。
    pub(crate) fn summary(self) -> &'static str {
        match self {
            Self::Gauges => "CPU、显卡、内存",
            Self::Pulse => "占用率与负载曲线",
            Self::Network => "下载与上传速度",
            Self::Dashboard => "负载、网速与磁盘",
            Self::Custom => "勾选想看的指标",
            Self::Cores => "每个逻辑处理器一列灯",
        }
    }
    pub(crate) fn parse(key: &str) -> Self {
        Self::ALL.into_iter().find(|v| v.key() == key).unwrap_or_default()
    }
}
/// 展开页：总览，以及五类硬件各自的详情页。
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    #[default]
    Overview,
    Cpu,
    Gpu,
    Memory,
    Disk,
    Network,
}
impl View {
    pub(crate) const ALL: [Self; 6] = [Self::Overview, Self::Cpu, Self::Gpu, Self::Memory, Self::Disk, Self::Network];
    /// 详情页（页签条里的五项）。
    pub(crate) const DETAILS: [Self; 5] = [Self::Cpu, Self::Gpu, Self::Memory, Self::Disk, Self::Network];
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Memory => "memory",
            Self::Disk => "disk",
            Self::Network => "network",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Overview => "总览",
            Self::Cpu => "CPU",
            Self::Gpu => "显卡",
            Self::Memory => "内存",
            Self::Disk => "磁盘",
            Self::Network => "网络",
        }
    }
    /// 旧版的「趋势 / 明细」等未知取值都回到总览。
    pub(crate) fn parse(key: &str) -> Self {
        Self::ALL.into_iter().find(|v| v.key() == key).unwrap_or_default()
    }
}

/// 一个程序（同名进程合并）在最近一秒的处理器占用和私有内存。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Process {
    pub name: String,
    pub cpu: f32,
    pub memory: u64,
    /// 最近一秒的读写字节数（文件、管道等全部读写，不只磁盘）。
    pub io: f32,
    /// 同名进程里内存最多的那个进程号，用来找程序文件取图标。
    pub pid: usize,
    /// 程序图标（32 px），取不到时为空，界面画首字母。
    pub icon: Option<std::sync::Arc<eframe::egui::ColorImage>>,
}
/// 一个本地固定磁盘分区的容量。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Volume {
    pub letter: char,
    pub used: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Reading {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    pub memory: Option<(u64, u64)>,
    pub gpu_name: String,
    pub gpu_temp: Option<f32>,
    pub video_memory: Option<(u64, u64)>,
    /// 共享显存（显卡借用的系统内存）的已用和上限；核显主要用它。
    pub shared_memory: Option<(u64, u64)>,
    /// 各类显卡引擎（3D、视频解码……）的占用率，同类取最忙的那个。
    pub gpu_engines: Vec<(&'static str, f32)>,
    pub network: [Option<f32>; 2],
    pub disk: [Option<f32>; 2],
    /// 处理器时间里内核态所占的百分比（相对全部逻辑处理器）。
    pub cpu_kernel: Option<f32>,
    /// 每个逻辑处理器的占用率。
    pub cores: Vec<f32>,
    pub cpu_mhz: Option<f32>,
    /// 内存构成：系统缓存与内核常驻（非分页池）字节数。
    pub memory_cache: Option<u64>,
    pub memory_kernel: Option<u64>,
    /// 真正空着的内存（空闲 + 已清零页）。可用 = 备用缓存 + 它。
    pub memory_free: Option<u64>,
    /// 磁盘活动时间百分比。
    pub disk_active: Option<f32>,
    /// 当前网卡的链路速率（比特每秒）与网卡启用以来的累计收发字节。
    pub link_speed: Option<u64>,
    pub net_total: Option<[u64; 2]>,
    /// 展开页打开时才统计的程序排行。
    pub by_cpu: Vec<Process>,
    pub by_memory: Vec<Process>,
    pub by_io: Vec<Process>,
    pub volumes: Vec<Volume>,
}
/// 详情列表最多列几名程序：总览只看前四，CPU、内存详情可以滚动看完这些。
pub(crate) const LIST_LEN: usize = 10;

impl Reading {
    /// 程序排行、内存构成和分区只在展开页显示时才统计。岛收起后、或刚展开还没算出新一轮时，
    /// 沿用上一次的结果，展开时列表直接就在，下一秒再换成新的，不会每次从空白加载。
    pub fn keep_lists(&mut self, previous: Reading) {
        if self.by_memory.is_empty() {
            self.by_memory = previous.by_memory;
            self.by_cpu = previous.by_cpu;
            self.by_io = previous.by_io;
            self.memory_cache = self.memory_cache.or(previous.memory_cache);
            self.memory_kernel = self.memory_kernel.or(previous.memory_kernel);
            if self.volumes.is_empty() {
                self.volumes = previous.volumes;
            }
        } else if self.by_cpu.is_empty() && self.by_io.is_empty() {
            // 刚开始统计的第一秒只有内存，CPU 和读写要两次采样才算得出。
            self.by_cpu = previous.by_cpu;
            self.by_io = previous.by_io;
        }
    }

    pub fn ram(&self) -> Option<f32> {
        self.memory
            .filter(|&(_, total)| total > 0)
            .map(|(used, total)| 100. * used as f32 / total as f32)
    }
}

#[derive(Default)]
pub(crate) struct Monitor {
    pub latest: Reading,
    pub history: VecDeque<(Instant, Reading)>,
    /// 岛内正在看的页面；切到别的页面或点常驻指标时清空，下次展开回到设置里选的页面。
    pub view: Option<View>,
    /// 最近看过的详情页，退回总览的过渡里继续画它。
    pub last_detail: View,
    /// CPU、内存详情的程序列表滚过了几名；`list_view` 记着是哪一页的，换页时回到顶部。
    pub list_first: usize,
    pub list_view: View,
    #[cfg(windows)]
    worker: Option<Worker>,
}

#[cfg(windows)]
struct Worker {
    stop: std::sync::mpsc::Sender<()>,
    visible: std::sync::Arc<std::sync::atomic::AtomicBool>,
    detail: std::sync::Arc<std::sync::atomic::AtomicBool>,
    samples: std::sync::Arc<std::sync::Mutex<VecDeque<(Instant, Reading)>>>,
}
#[cfg(windows)]
impl Worker {
    fn start(ctx: egui::Context, visible: bool) -> Self {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc, Mutex,
        };
        let (stop, stopped) = mpsc::channel();
        let samples = Arc::new(Mutex::new(VecDeque::with_capacity(60)));
        let output = Arc::clone(&samples);
        let visible = Arc::new(AtomicBool::new(visible));
        let repaint = Arc::clone(&visible);
        let detail = Arc::new(AtomicBool::new(false));
        let wants_detail = Arc::clone(&detail);
        std::thread::spawn(move || {
            let mut sampler = windows::Sampler::default();
            loop {
                let at = Instant::now();
                let reading = sampler.read(at, wants_detail.load(Ordering::Relaxed));
                {
                    let mut queue = output.lock().unwrap();
                    if queue.len() == 60 {
                        queue.pop_front();
                    }
                    queue.push_back((at, reading));
                }
                if repaint.load(Ordering::Relaxed) {
                    ctx.request_repaint();
                }
                let wait = Duration::from_secs(1).saturating_sub(at.elapsed());
                if !matches!(stopped.recv_timeout(wait), Err(mpsc::RecvTimeoutError::Timeout)) {
                    break;
                }
            }
        });
        Self { stop, visible, detail, samples }
    }
}
#[cfg(windows)]
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

impl Monitor {
    /// `detail`：展开页正在显示，需要逐核、程序排行等较重的统计。
    pub fn tick(&mut self, enabled: bool, visible: bool, detail: bool, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            if !enabled {
                if self.worker.take().is_some() {
                    self.latest = Reading::default();
                    self.history.clear();
                }
                return;
            }
            let worker = self.worker.get_or_insert_with(|| Worker::start(ctx.clone(), visible));
            worker.visible.store(visible, std::sync::atomic::Ordering::Relaxed);
            worker.detail.store(detail, std::sync::atomic::Ordering::Relaxed);
            let mut queue = worker.samples.lock().unwrap();
            let Some((_, last)) = queue.back() else {
                return;
            };
            let previous = std::mem::replace(&mut self.latest, last.clone());
            self.latest.keep_lists(previous);
            let now = Instant::now();
            self.history.extend(queue.drain(..));
            while self
                .history
                .front()
                .is_some_and(|(at, _)| self.history.len() > 60 || now.duration_since(*at) >= Duration::from_secs(60))
            {
                self.history.pop_front();
            }
        }
        #[cfg(not(windows))]
        let _ = (enabled, visible, detail, ctx);
    }
}

#[cfg(all(windows, test))]
mod tests {
    use super::*;
    use ::windows::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };

    fn cpu_seconds() -> f64 {
        let (mut created, mut ended, mut kernel, mut user) = Default::default();
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut ended, &mut kernel, &mut user) }.unwrap();
        let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
        (ticks(kernel) + ticks(user)) as f64 / 1e7
    }

    #[test]
    fn collapsed_readings_keep_the_last_program_lists() {
        let program = |name: &str| Process { name: name.into(), cpu: 1., memory: 1, ..Default::default() };
        let open = Reading { by_cpu: vec![program("a")], by_memory: vec![program("b")], memory_cache: Some(5), ..Default::default() };
        let mut collapsed = Reading::default();
        collapsed.keep_lists(open.clone());
        assert_eq!((collapsed.by_cpu.clone(), collapsed.by_memory.clone(), collapsed.memory_cache), (open.by_cpu.clone(), open.by_memory.clone(), Some(5)));
        let mut warming = Reading { by_memory: vec![program("c")], ..Default::default() };
        warming.keep_lists(open.clone());
        assert_eq!((warming.by_memory[0].name.as_str(), warming.by_cpu.clone()), ("c", open.by_cpu.clone()));
        let mut fresh = Reading { by_memory: vec![program("c")], by_cpu: vec![program("d")], ..Default::default() };
        fresh.keep_lists(open);
        assert_eq!(fresh.by_cpu[0].name, "d");
    }

    #[test]
    #[ignore = "单独运行以测量实际系统采样的 CPU 时间"]
    fn continuous_sampling_cost() {
        let ctx = egui::Context::default();
        let mut monitor = Monitor::default();
        monitor.tick(true, false, false, &ctx);
        std::thread::sleep(Duration::from_secs(3));
        monitor.tick(true, false, false, &ctx);
        let wall = Instant::now();
        let cpu = cpu_seconds();
        std::thread::sleep(Duration::from_secs(45));
        monitor.tick(true, true, true, &ctx);
        let elapsed = wall.elapsed().as_secs_f64();
        let used = cpu_seconds() - cpu;
        let count = monitor.history.iter().filter(|(t, _)| *t >= wall).count();
        println!(
            "elapsed={elapsed:.3} cpu_seconds={used:.4} one_core_pct={:.3} samples={count}",
            100. * used / elapsed
        );
        assert!((43..=46).contains(&count));
        assert!(monitor.latest.cpu.is_some() && monitor.latest.memory.is_some());
    }
}
