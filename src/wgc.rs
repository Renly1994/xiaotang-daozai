//! Windows Graphics Capture 实时抓屏（液态玻璃试验）。
//!
//! GDI BitBlt 读的是「桌面 DC」，`WDA_EXCLUDEFROMCAPTURE` 在那里只会把窗口区域
//! 变成黑色（或异步失效后抓到窗口自己），拿不到干净桌面。WGC 走 DWM 的独立
//! 合成路径：被排除的窗口不进入合成树，帧里露出的是它后面的真实桌面。
//!
//! 这里抓整个显示器的合成帧，只把岛体外扩矩形 `CopySubresourceRegion` 到一张
//! staging 纹理再回读，避免每帧全屏 CPU 拷贝。任何一步失败都会永久回退到
//! GDI + 干净底片，不影响岛仔其它功能。

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use windows::core::{factory, Interface};
use windows::Graphics::SizeInt32;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Foundation::{BOOL, LPARAM, POINT, RECT};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

struct Wgc {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    _item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    staging: ID3D11Texture2D,
    /// 显示器左上角在屏幕坐标系里的位置（多显示器时可能非 0）。
    mon_x: i32,
    mon_y: i32,
    sw: i32,
    sh: i32,
    stage_w: u32,
    stage_h: u32,
}

static WGC: Mutex<Option<Wgc>> = Mutex::new(None);
static FAILED: AtomicBool = AtomicBool::new(false);
static TRIED: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU64 = AtomicU64::new(0);
/// 抓屏源代次：每次因显示器变化重建会话时 +1。GPU 层的干净底片据此判断
/// 桌面源是否已换屏，换屏后必须重抓，不能复用旧显示器的底片内容。
static EPOCH: AtomicU64 = AtomicU64::new(0);
/// 当前 WGC 会话绑定的显示器身份：左上角原点 + 分辨率。切屏时与实时查询值比对，
/// 不一致就重建会话。用 i32 的 (mon_x,mon_y,sw,sh) 作为显示器身份（多屏下原点唯一）。
static BOUND_MONITOR: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);
/// **整个桌面拓扑**的签名（所有显示器的 rect 集合）。只比较「抓屏矩形属于哪块屏」
/// 挡不住「仅主屏显示 → 扩展」这种切换：主屏 rect 没变，但 DWM 已经重建了合成器，
/// 旧会话出的是重建期的坏帧，玻璃会花掉（表现为岛内缩进一块壁纸 + 四周发黑）。
/// 所以每几帧算一次全桌面签名，变了就一律重置。
static TOPO_SIG: AtomicU64 = AtomicU64::new(0);
/// 拓扑变化后的稳定期：这段时间内既不抓屏也不重建底片。切显示模式时桌面有
/// 1 秒左右的「向内收拢、四周变黑、再复原」动画，抓到中间态会污染底片。
static SETTLE_UNTIL: Mutex<Option<std::time::Instant>> = Mutex::new(None);
/// 拓扑查询降频计数器：EnumDisplayMonitors 不便宜，约每 100ms 查一次足够。
static TOPO_TICK: AtomicU64 = AtomicU64::new(0);
/// 「这台机器切换显示模式后，实际需要多久桌面才稳定」——跑几次就学会了。
/// 每次底片干净建成时回写，下次切换直接从这里起步，不用固定值。
static LEARNED_SETTLE_MS: AtomicU64 = AtomicU64::new(0);
/// 本次切换的时刻，用于计算实际稳定耗时。
static TOPO_CHANGED_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// 拓扑已变化、等待抓着 WGC 锁的那一方把旧会话 drop 掉。用原子标志而不是在
/// `poll_topology` 里直接拿 WGC 锁 —— 调用它的 `grab` 可能正持着那把锁，会死锁。
static NEED_RESET: AtomicBool = AtomicBool::new(false);

fn log(msg: &str) {
    use std::io::Write;
    let dir = crate::model::config_dir().join("diagnostics");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("liquid-glass.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::write(&path, b"");
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", msg);
    }
}

/// WGC 是否已经成功产出过帧；供诊断显示。
pub fn frames() -> u64 { FRAMES.load(Ordering::Relaxed) }

/// 抓屏源代次；换屏重建会话后递增。GPU 底片用它判断是否失效。
pub fn epoch() -> u64 { EPOCH.load(Ordering::Relaxed) }

/// 查询矩形中心点所在显示器的句柄与身份 (mon_x, mon_y, sw, sh)。用矩形中心而非
/// 左上角，避免岛体落在两屏交界时左上角被判给隔壁屏。
fn monitor_for_rect(x: i32, y: i32, w: i32, h: i32) -> Option<(HMONITOR, (i32, i32, i32, i32))> {
    unsafe {
        let monitor = MonitorFromPoint(
            POINT { x: x + w / 2, y: y + h / 2 }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut mi).as_bool() { return None; }
        let (mx, my) = (mi.rcMonitor.left, mi.rcMonitor.top);
        Some((monitor, (mx, my, mi.rcMonitor.right - mx, mi.rcMonitor.bottom - my)))
    }
}

/// 收集各显示器 rect 的回调（EnumDisplayMonitors 要求 C 调用约定）。
unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let out = &mut *(data.0 as *mut Vec<(i32, i32, i32, i32)>);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        let (l, t, r, b) = (
            info.rcMonitor.left,
            info.rcMonitor.top,
            info.rcMonitor.right,
            info.rcMonitor.bottom,
        );
        out.push((l, t, r - l, b - t));
    }
    // 继续枚举（TRUE = 1）。
    BOOL(1)
}

/// 整个桌面的拓扑签名：所有显示器 rect 排序后做 FNV。显示器增减、位置或分辨率
/// 变化都会改变签名；单块屏内部的窗口变化不会。
fn desktop_signature() -> u64 {
    let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut rects as *mut _ as isize),
        );
    }
    if rects.is_empty() {
        return 0;
    }
    rects.sort_unstable();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (l, t, w, hh) in rects {
        for v in [l, t, w, hh] {
            h ^= v as u32 as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// 检测桌面拓扑是否刚变过（每约 100ms 实查一次，其余帧返回 false）。
///
/// 公开给 gpu_glass 的抓屏入口调用：`capture` 会在 settling() 期间直接返回 None，
/// 若不在这里先检测，稳定期内的再次切换就永远发现不了（窗口无法续期）。
pub fn poll_topology() -> bool {
    if TOPO_TICK.fetch_add(1, Ordering::Relaxed) % 6 != 0 {
        return false;
    }
    let sig = desktop_signature();
    if sig == 0 {
        return false;
    }
    let previous = TOPO_SIG.swap(sig, Ordering::Relaxed);
    let changed = previous != 0 && previous != sig;
    // 首次建立基线、以及每次变化都打一行，便于实机确认检测是否真的在工作。
    if previous == 0 || changed {
        log(&format!("desktop: sig={:016x} rects={}", sig, monitor_rects_desc()));
    }
    if changed {
        // **必须在这里就把重置登记下来**，不能靠返回值：调用方可能是 `App::frame`
        // （它只关心节拍、不处理返回值），真正 drop 会话的 `grab` 那一帧又可能被
        // 降频跳过，结果稳定期从未设置 —— 切换瞬间的全黑帧就被存成底片，玻璃永久发黑。
        NEED_RESET.store(true, Ordering::Relaxed);
        EPOCH.fetch_add(1, Ordering::Relaxed);
        *BOUND_MONITOR.lock().unwrap_or_else(|e| e.into_inner()) = None;
        FAILED.store(false, Ordering::Relaxed);
        TRIED.store(false, Ordering::Relaxed);
        let settle = settle_ms();
        let now = std::time::Instant::now();
        *TOPO_CHANGED_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
        *SETTLE_UNTIL.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(now + std::time::Duration::from_millis(settle));
        log(&format!("desktop: 显示器拓扑变化，等待 {}ms 稳定后重抓底片", settle));
    }
    changed
}

/// 当前显示器 rect 的文字描述（仅诊断日志用）。
fn monitor_rects_desc() -> String {
    let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut rects as *mut _ as isize),
        );
    }
    rects.sort_unstable();
    rects
        .iter()
        .map(|(l, t, w, h)| format!("({},{} {}x{})", l, t, w, h))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 拓扑变化后的稳定期时长（毫秒）。默认 500；可用 `DAOZAI_SETTLE_MS` 覆盖，
/// 方便在实机上不重新编译就试不同时长。
fn settle_ms() -> u64 {
    // 优先级：环境变量（调试用）> 本机学到的值 > 首次默认 500（快机器友好）。
    // 学到的值 = 上次切换的实际稳定耗时 ×0.85，所以慢机器会自己收敛到更大的窗口，
    // 快机器稳定在 400ms 左右——不需要为不同电脑手调。
    if let Some(v) = std::env::var("DAOZAI_SETTLE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        return v.clamp(0, 30_000);
    }
    let learned = {
        let mem = LEARNED_SETTLE_MS.load(Ordering::Relaxed);
        if mem > 0 { mem } else { load_learned_settle() }
    };
    if learned > 0 { learned.clamp(400, 6000) } else { 500 }
}

/// 学到的稳定时长落盘位置（放 diagnostics，不污染用户的 settings）。
fn learned_settle_path() -> std::path::PathBuf {
    crate::model::config_dir().join("diagnostics").join("settle-learned.txt")
}

fn load_learned_settle() -> u64 {
    std::fs::read_to_string(learned_settle_path())
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn save_learned_settle(ms: u64) {
    let path = learned_settle_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, ms.to_string());
}

/// 底片干净建成时回调：记录「从拓扑变化到拿到干净底片」的实际耗时，下次用它起步。
/// 乘 0.85 留余量，避免卡在临界值上反复丢帧。
pub fn note_plate_ready() {
    // 只认「拓扑变化后的第一张干净底片」：5s 刷新窗口里还会反复重建，
    // 若每次都算，耗时越累越大、学习值越学越慢（第 10 次会学到 5 秒）。
    let start = {
        let mut guard = TOPO_CHANGED_AT.lock().unwrap_or_else(|e| e.into_inner());
        guard.take()
    };
    let Some(start) = start else { return };
    let used = start.elapsed().as_millis() as u64;
    if used == 0 { return; }
    let next = ((used as f64 * 0.85) as u64).clamp(400, 6000);
    LEARNED_SETTLE_MS.store(next, Ordering::Relaxed);
    save_learned_settle(next);
    log(&format!("desktop: 本次稳定耗时 {}ms，下次起步 {}ms（已落盘，重启后仍生效）", used, next));
}

/// 桌面拓扑刚变过、还在等 DWM 稳定：这段时间内**既不抓屏也不重建底片**。
///
/// 必须由 `App::frame` 每帧调用 `poll_topology`，不能只靠抓屏路径 —— 用户切显示
/// 模式时岛仔通常是**收起态**（不抓屏），只在开玻璃时检测会整段错过，展开后用的
/// 还是旧拓扑的底片，直接花屏。
pub fn settling() -> bool {
    SETTLE_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|t| std::time::Instant::now() < t)
}

fn staging(device: &ID3D11Device, w: u32, h: u32) -> windows::core::Result<ID3D11Texture2D> {    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut out: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut out)) }?;
    out.ok_or_else(|| windows::core::Error::from(windows::core::HRESULT(0x8000_4005u32 as i32)))
}

fn init(_x: i32, _y: i32, w: u32, h: u32,
        monitor: HMONITOR, ident: (i32, i32, i32, i32)) -> windows::core::Result<Wgc> {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None, D3D_DRIVER_TYPE_HARDWARE, windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION,
            Some(&mut device), None, Some(&mut context),
        )?;
        let device = device.ok_or_else(|| windows::core::Error::from(windows::core::HRESULT(0x8000_4005u32 as i32)))?;
        let context = context.ok_or_else(|| windows::core::Error::from(windows::core::HRESULT(0x8000_4005u32 as i32)))?;

        // WinRT 侧要的是 IDirect3DDevice，用 CreateDirect3D11DeviceFromDXGIDevice 从
        // 同一个 D3D11 设备包装出来，保证帧池的纹理和我们的 staging 在同一个 GPU 上。
        let dxgi: IDXGIDevice = device.cast()?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?;
        let d3d_device: IDirect3DDevice = inspectable.cast()?;

        let (mon_x, mon_y, sw, sh) = ident;

        let interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = interop.CreateForMonitor(monitor)?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2,
            SizeInt32 { Width: sw, Height: sh },
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        session.StartCapture()?;
        // Windows 11 在显示器被 WGC 捕获时会在屏幕四周画一圈黄色提示边框。
        // 尝试关掉它；这一步可能需要系统授权，失败就保持原样（不影响抓屏）。
        match session.SetIsBorderRequired(false) {
            Ok(()) => log("wgc: 已关闭系统捕获黄框"),
            Err(e) => log(&format!("wgc: 关闭捕获黄框失败 {:?}", e)),
        }

        let stage = staging(&device, w, h)?;
        Ok(Wgc {
            _device: device, context, _item: item, pool, session,
            staging: stage, mon_x, mon_y, sw, sh, stage_w: w, stage_h: h,
        })
    }
}

/// 抓屏结果。必须区分「没有新帧」和「WGC 不可用」：
/// 前者应当复用上一帧纹理，后者才回退 GDI —— 否则会 GDI/WGC 交替导致闪烁。
pub enum Grab {
    Frame(Vec<u8>, u32, u32),
    NoFrame,
    Unavailable,
}

/// 抓屏幕物理矩形 (x,y,w,h) 的**实时 DWM 合成帧**，返回自上而下的 RGBA。
pub fn grab(x: i32, y: i32, w: i32, h: i32) -> Grab {
    if w <= 0 || h <= 0 || w > 2048 || h > 1024 { return Grab::Unavailable; }
    let (bw, bh) = (w as u32, h as u32);
    // 多显示器：岛仔被移到另一块屏（或主屏切换 / 拔线改了屏幕布局）后，抓屏矩形
    // 所属显示器会变。WGC 会话是在第一次 grab 时按当时的 (x,y) 绑定的，mon_x/mon_y/
    // sw/sh 永久固定；若继续用旧会话，`x - mon_x` 会算出错位甚至越界的坐标，轻则抓到
    // 隔壁屏内容、重则直接 Unavailable 回退 GDI + 旧屏底片 —— 表现就是玻璃横向拉伸、
    // 半块拱形残留。这里每帧按矩形中心实时查显示器，身份变了就销毁旧会话重建。
    let current = monitor_for_rect(x, y, w, h);
    // 先查整桌面拓扑：切显示模式（仅主屏 → 扩展 / 复制 / 主屏互换）时主屏 rect
    // 可能不变，只看「矩形属于哪块屏」会漏掉。拓扑一变，旧会话与旧底片全部作废。
    let topology_changed = poll_topology();
    let mut guard = WGC.lock().unwrap_or_else(|e| e.into_inner());
    let _ = topology_changed; // 重置登记已在 poll_topology 内完成，这里只负责 drop 旧会话
    if NEED_RESET.swap(false, Ordering::Relaxed) {
        *guard = None;
        log("desktop: 旧抓屏会话已销毁，等稳定期结束后重建");
    }
    // 稳定期内不出帧：切显示模式时桌面有「向内收拢、四周变黑、再复原」的动画，
    // 抓到中间态会写进底片，之后玻璃一直是花的（就是用户看到的那种）。
    if settling() {
        return Grab::NoFrame;
    }
    if let Some((monitor, ident)) = current {
        let bound = *BOUND_MONITOR.lock().unwrap_or_else(|e| e.into_inner());
        let changed = guard.as_ref()
            .map(|s| (s.mon_x, s.mon_y, s.sw, s.sh) != ident)
            .unwrap_or_else(|| bound.is_some_and(|b| b != ident));
        if changed {
            // drop 旧 Wgc：session/pool/device 的 RAII 析构会自动停止旧屏捕获并释放 COM 资源。
            *guard = None;
            EPOCH.fetch_add(1, Ordering::Relaxed);
            log(&format!("wgc: 显示器变化 {:?} → {:?}，重建会话",
                         bound.map(|b| (b.0, b.1, b.2, b.3)), ident));
            // 换屏后给新显示器一次全新机会：清掉旧屏的失败/已尝试标志。
            FAILED.store(false, Ordering::Relaxed);
            TRIED.store(false, Ordering::Relaxed);
        }
    }
    if FAILED.load(Ordering::Relaxed) { return Grab::Unavailable; }
    if guard.is_none() {
        if !TRIED.swap(true, Ordering::Relaxed) {
            let Some((monitor, ident)) = current else { return Grab::Unavailable; };
            match init(x, y, bw, bh, monitor, ident) {
                Ok(s) => {
                    log(&format!("wgc: init OK monitor=({},{} {}x{}) cap={}x{}", s.mon_x, s.mon_y, s.sw, s.sh, bw, bh));
                    *BOUND_MONITOR.lock().unwrap_or_else(|e| e.into_inner()) = Some(ident);
                    *guard = Some(s);
                }
                Err(e) => {
                    FAILED.store(true, Ordering::Relaxed);
                    log(&format!("wgc: init FAILED {:?}，回退 GDI + 底片", e));
                    return Grab::Unavailable;
                }
            }
        } else { return Grab::Unavailable; }
    }
    let Some(s) = guard.as_mut() else { return Grab::Unavailable };
    // 展开动画里抓屏矩形会变，staging 纹理跟着重建（只在尺寸变化时）。
    if s.stage_w != bw || s.stage_h != bh {
        match staging(&s._device, bw, bh) {
            Ok(t) => { s.staging = t; s.stage_w = bw; s.stage_h = bh; }
            Err(e) => { log(&format!("wgc: staging resize FAILED {:?}", e)); return Grab::Unavailable; }
        }
    }
    let (sx, sy) = (x - s.mon_x, y - s.mon_y);
    if sx < 0 || sy < 0 || sx + w > s.sw || sy + h > s.sh { return Grab::Unavailable; }
    let src_box = D3D11_BOX {
        left: sx as u32, top: sy as u32, front: 0,
        right: (sx + w) as u32, bottom: (sy + h) as u32, back: 1,
    };
    unsafe {
        // 帧池是异步的，一次只取一帧会拿到滞后画面；连取到没有新帧为止，
        // 用最新的一帧（最多丢 4 个旧帧）。
        let mut frame = s.pool.TryGetNextFrame().ok();
        for _ in 0..4 {
            match s.pool.TryGetNextFrame() { Ok(f) => frame = Some(f), Err(_) => break }
        }
        let Some(frame) = frame else { return Grab::NoFrame };
        let surface = match frame.Surface() { Ok(v) => v, Err(_) => return Grab::NoFrame };
        let access: IDirect3DDxgiInterfaceAccess = match surface.cast() { Ok(v) => v, Err(_) => return Grab::Unavailable };
        let src_tex: ID3D11Texture2D = match access.GetInterface() { Ok(v) => v, Err(_) => return Grab::Unavailable };
        s.context.CopySubresourceRegion(&s.staging, 0, 0, 0, 0, &src_tex, 0, Some(&src_box));
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if s.context.Map(&s.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).is_err() { return Grab::Unavailable; }
        // 帧格式本来就是 B8G8R8A8，和 wgpu 纹理的 Bgra8Unorm 一致，
        // 按行整段拷贝即可 —— 之前每帧做 4 次/像素的逐字节转换，在展开态
        // （820x291≈24 万像素）是明显开销。
        let mut rgba = vec![0u8; (bw * bh * 4) as usize];
        if !mapped.pData.is_null() {
            let ptr = mapped.pData as *const u8;
            let pitch = mapped.RowPitch as usize;
            let row_bytes = (bw * 4) as usize;
            for row in 0..bh as usize {
                let src = std::slice::from_raw_parts(ptr.add(row * pitch), row_bytes);
                rgba[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(src);
            }
        }
        s.context.Unmap(&s.staging, 0);
        drop(frame);
        let n = FRAMES.fetch_add(1, Ordering::Relaxed);
        if n == 0 || n % 600 == 0 { log(&format!("wgc: frame {} cap={}x{}", n, bw, bh)); }
        if mapped.pData.is_null() { return Grab::NoFrame; }
        Grab::Frame(rgba, bw, bh)
    }
}
