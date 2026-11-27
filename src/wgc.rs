//! Windows 桌面合成帧的辅助状态：拓扑变化稳定期 + 背景亮度采样。
//!
//! 现行液态玻璃抓屏走 `d3d11/wgc_capture.rs`（与主交换链同一个 D3D11 设备，
//! CopyResource 纯 GPU、无 CPU 读回）。本模块只保留两件仍被现行栈使用的能力：
//! 1. 显示器拓扑变化后的稳定期，避免切显示模式时抓到 DWM 过渡黑帧；
//! 2. 从抓屏纹理的小块区域读回亮度，决定液态玻璃深/浅材质。

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};

/// 主题亮度两次采样之间的最小间隔。按时间而非按帧节流：无论帧率高低
/// （静止深休眠可低至个位数 fps），主题色滞后都不超过 500ms，且静止态不空转。
const THEME_SAMPLE_INTERVAL_MS: u64 = 500;
const THEME_STAGE_W: u32 = 1024; // ≥ 520CSS × 常见 ppp 上限，固定不变
const THEME_STAGE_H: u32 = 256;
static THEME_STAGE: Mutex<Option<ID3D11Texture2D>> = Mutex::new(None);
static THEME_RECT: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);
/// 内容区（展开态下半部分）的采样框：和上缘分开，避免「上缘亮、内容暗」时被平均掉。
static THEME_CONTENT_RECT: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);
static THEME_LUMA: AtomicU32 = AtomicU32::new(0x3f20_0000); // 0.625 相对亮度（≈sRGB 0.82），默认浅色
static THEME_LUMA_CONTENT: AtomicU32 = AtomicU32::new(0x3f20_0000);
/// 从浅到深的大幅突变待确认：连续两次采样都低才真正采纳，挡掉开机过渡暗帧。
static THEME_DARK_PENDING: AtomicBool = AtomicBool::new(false);
static THEME_DARK_PENDING_CONTENT: AtomicBool = AtomicBool::new(false);
static THEME_LAST_SAMPLE: Mutex<Option<Instant>> = Mutex::new(None);
/// 毫秒时间戳的进程级原点（OnceLock 保证只初始化一次，跨平台安全）。
static THEME_EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
fn theme_epoch() -> Instant { *THEME_EPOCH.get_or_init(Instant::now) }

/// 纯判断：距上次采样是否已过 interval。用于把「按时间节流」逻辑单独单测，
/// 锁定「低帧率下延迟不放大、静止态不空转」。now/last 都是毫秒时间戳。
fn theme_sample_due(now_ms: u64, last_ms: Option<u64>, interval_ms: u64) -> bool {
    match last_ms {
        None => true,
        Some(last) => now_ms.saturating_sub(last) >= interval_ms,
    }
}

/// D3D11 渲染循环每帧设置主题采样框（相对抓屏缓冲坐标）；None 表示不采样。
pub fn set_theme_rect(rect: Option<(i32, i32, i32, i32)>) {
    if let Ok(mut g) = THEME_RECT.lock() { *g = rect; }
}
/// 上缘与内容区分别设置。内容区采样框单独存在，避免两区亮度差被平均掉。
pub fn set_theme_rects(top: Option<(i32, i32, i32, i32)>, content: Option<(i32, i32, i32, i32)>) {
    set_theme_rect(top);
    if let Ok(mut g) = THEME_CONTENT_RECT.lock() { *g = content; }
}
pub fn theme_luma_top() -> f32 { f32::from_bits(THEME_LUMA.load(Ordering::Relaxed)) }
pub fn theme_luma_content() -> f32 { f32::from_bits(THEME_LUMA_CONTENT.load(Ordering::Relaxed)) }

/// 决定主题的最终亮度。
///
/// 两区亮度差小于 0.25 时取平均（和一整块采样差不多，但抗离群点）；
/// 差距明显时**内容区优先**——展开态下半部分才是文字/数据所在，
/// 上缘一小条亮区不该把整个主题拉成浅色、让暗背景上的内容看不清。
/// 这与 WCAG 的原则一致：对比度要按文字实际覆盖的局部背景评估。
pub(crate) fn effective_luma(top: f32, content: f32) -> f32 {
    // 0.16 相对亮度差 ≈ 跨过整个 0.23~0.39 滞回带，足以让主题翻转。
    const DIVERGENCE: f32 = 0.16;
    if (top - content).abs() >= DIVERGENCE { content } else { (top + content) * 0.5 }
}

/// 取最近一次采样到的背景亮度（f32 bits）。无采样时返回默认值，不影响调用方。
pub fn theme_luma() -> f32 { effective_luma(theme_luma_top(), theme_luma_content()) }
/// 用壁纸文件亮度播种主题锚值（启动时调用，开机第一帧就不依赖抓屏）。
/// 入参是 `wallpaper::luma()` 的 sRGB 均值，转成 WCAG 相对亮度后再播种，口径与实时采样一致。
pub fn seed_theme_luma(v: f32) {
    let v = srgb_linear_f32(v.clamp(0.0, 1.0)).clamp(0.0, 1.0).to_bits();
    THEME_LUMA.store(v, Ordering::Relaxed);
    THEME_LUMA_CONTENT.store(v, Ordering::Relaxed);
}

/// D3D11 液态玻璃栈用：从抓屏纹理采样玻璃背后亮度，按时间节流（500ms）。
/// sx/sy = 抓屏缓冲在全屏帧里的原点；sw/sh = 全屏帧尺寸。失败/未到期静默返回。
pub fn sample_theme_luma_d3d11(
    device: &ID3D11Device, context: &ID3D11DeviceContext,
    src_tex: &ID3D11Texture2D, sx: i32, sy: i32, sw: i32, sh: i32,
) {
    let now = Instant::now();
    let due = match THEME_LAST_SAMPLE.lock() {
        Ok(mut guard) => {
            let epoch = theme_epoch();
            let now_ms = now.duration_since(epoch).as_millis() as u64;
            let last_ms = (*guard).map(|t| t.duration_since(epoch).as_millis() as u64);
            if theme_sample_due(now_ms, last_ms, THEME_SAMPLE_INTERVAL_MS) {
                *guard = Some(now);
                true
            } else { false }
        }
        Err(_) => false,
    };
    if due { sample_theme_luma(device, context, src_tex, sx, sy, sw, sh); }
}

/// 纯边界计算：把主题采样框 rect（相对抓屏缓冲坐标）换算成源 box（相对显示器帧）
/// 与读回尺寸 (left,top,right,bottom,rw,rh)。任何退化/越界/溢出都返回 None，
/// 调用方据此静默跳过。抽成不依赖 COM 的纯函数，专门单测「尺寸剧烈变化时绝不越界」。
/// - rw/rh 先夹到固定 staging 上限（1024×256），杜绝 from_raw_parts 越界；
/// - 源 box 必须完整落在显示器帧 [0,sw)×[0,sh) 内，否则 CopySubresourceRegion 会失败；
/// - 全程用 i64 中间运算，极端 i32 坐标也不会溢出 panic（debug 构建尤其重要）。
fn theme_sample_box(
    rect: Option<(i32, i32, i32, i32)>, sx: i32, sy: i32, sw: i32, sh: i32,
) -> Option<(i32, i32, i32, i32, u32, u32)> {
    let (rx0, ry0, rx1, ry1) = rect?;
    if rx1 <= rx0 || ry1 <= ry0 { return None; }
    let rw = ((rx1 as i64 - rx0 as i64).min(THEME_STAGE_W as i64) as u32).max(0);
    let rh = ((ry1 as i64 - ry0 as i64).min(THEME_STAGE_H as i64) as u32).max(0);
    if rw == 0 || rh == 0 { return None; }
    if sw <= 0 || sh <= 0 { return None; }
    let left = sx as i64 + rx0 as i64;
    let top = sy as i64 + ry0 as i64;
    let right = left + rw as i64;
    let bottom = top + rh as i64;
    if left < 0 || top < 0 || right > sw as i64 || bottom > sh as i64 { return None; }
    // 能走到这里说明 right<=sw<=i32::MAX 且 left>=0，安全收窄回 i32。
    Some((left as i32, top as i32, right as i32, bottom as i32, rw, rh))
}

/// 主题亮度要不要落盘：大幅「浅→深」突变要求连续两次采样都低才采纳。
/// `anchor` 是壁纸文件亮度（Some 时）：抓屏值与它差 >0.25 视为开机过渡帧/污染帧，
/// 直接拒绝——这是比"连续两次"更直接的判据，因为壁纸文件是稳定的真值来源。
/// 返回 (要写入的新亮度, 更新后的待确认标记)。
pub(crate) fn theme_luma_commit(prev: f32, luma: f32, pending: bool, anchor: Option<f32>) -> (Option<f32>, bool) {
    // 壁纸锚点校验：抓屏与壁纸亮度严重不符 → 采到的是过渡帧/抓到自身，不采纳。
    if anchor.is_some_and(|a| (luma - a).abs() > 0.25) {
        return (None, false);
    }
    if prev - luma > 0.30 {
        if pending { (Some(luma), false) } else { (None, true) }
    } else {
        (Some(luma), false)
    }
}

fn srgb_linear_f32(s: f32) -> f32 {
    if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}
fn srgb_linear(v: u8) -> f32 { srgb_linear_f32(v as f32 / 255.0) }

/// WCAG 相对亮度：sRGB 线性化后按 Rec.709 权重，不再用 (R+G+B)/3 的 sRGB 均值。
pub(crate) fn relative_luma(r: u8, g: u8, b: u8) -> f32 {
    (0.2126 * srgb_linear(r) + 0.7152 * srgb_linear(g) + 0.0722 * srgb_linear(b)).clamp(0.0, 1.0)
}

/// 10%~90% 截尾后取中位数。算术平均会被一小块高亮窗口拉高，
/// 中位数对这种离群点不敏感，更适合判断「文字背后到底是什么底色」。
/// 用 `select_nth_unstable_by` 只定位分位点（O(n)），不做整数组排序（O(n log n)）。
pub(crate) fn trimmed_median(mut values: Vec<f32>) -> f32 {
    if values.is_empty() { return 0.625; }
    let n = values.len();
    let lo = n / 10;
    let hi = n - lo;
    let span = hi - lo;
    if span == 1 { return values[lo].clamp(0.0, 1.0); }
    let mid = lo + span / 2;
    let cmp = |a: &f32, b: &f32| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal);
    let v = if span % 2 == 0 {
        *values.select_nth_unstable_by(mid, cmp).1
    } else {
        // 偶数个样本要中间两位的平均：先定第 mid+1 小，再在前 mid+1 个里定第 mid 小。
        values.select_nth_unstable_by(mid + 1, cmp);
        let lower = *values[..=mid].select_nth_unstable_by(mid, cmp).1;
        let upper = values[mid + 1];
        (lower + upper) * 0.5
    };
    v.clamp(0.0, 1.0)
}

/// 从干净 WGC 帧的一个采样框算相对亮度。失败路径静默返回 None。
fn sample_rect_luma(
    device: &ID3D11Device, context: &ID3D11DeviceContext, stage: &ID3D11Texture2D,
    src_tex: &ID3D11Texture2D, sx: i32, sy: i32, sw: i32, sh: i32,
    rect: Option<(i32, i32, i32, i32)>,
) -> Option<f32> {
    let (left, top, right, bottom, rw, rh) = theme_sample_box(rect, sx, sy, sw, sh)?;
    let box_src = D3D11_BOX {
        left: left as u32, top: top as u32, front: 0, right: right as u32, bottom: bottom as u32, back: 1,
    };
    unsafe {
        // 复制采样框到固定 staging 左上角；目标尺寸 rw×rh 恒 ≤ staging 尺寸。
        context.CopySubresourceRegion(stage, 0, 0, 0, 0, src_tex, 0, Some(&box_src));
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if context.Map(stage, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).is_err() { return None; }
        let mut values = Vec::with_capacity((rw / 4) as usize * (rh / 4) as usize + 1);
        if !mapped.pData.is_null() {
            let ptr = mapped.pData as *const u8;
            let pitch = mapped.RowPitch as usize;
            // 每行只读 rw*4 字节；rw ≤ THEME_STAGE_W，pitch ≥ THEME_STAGE_W*4，绝不越界。
            if pitch >= THEME_STAGE_W as usize * 4 {
                for y in (0..rh as usize).step_by(4) {
                    let row = std::slice::from_raw_parts(ptr.add(y * pitch), rw as usize * 4);
                    for x in (0..rw as usize).step_by(4) {
                        let i = x * 4;
                        values.push(relative_luma(row[i + 2], row[i + 1], row[i])); // BGRA
                    }
                }
            }
        }
        let _ = context.Unmap(stage, 0);
        if values.is_empty() { None } else { Some(trimmed_median(values)) }
    }
}

/// 从干净 WGC 帧里分别采「玻璃上缘」和「内容区」，写两个 THEME_LUMA。
/// 全部失败路径静默——主题色只是视觉微调，绝不能影响主渲染或导致崩溃。
fn sample_theme_luma(
    device: &ID3D11Device, context: &ID3D11DeviceContext,
    src_tex: &ID3D11Texture2D, sx: i32, sy: i32, sw: i32, sh: i32,
) {
    let top_rect = THEME_RECT.lock().ok().and_then(|g| *g);
    let content_rect = THEME_CONTENT_RECT.lock().ok().and_then(|g| *g);
    let stage = {
        let Ok(mut guard) = THEME_STAGE.lock() else { return; };
        if guard.is_none() {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: THEME_STAGE_W, Height: THEME_STAGE_H, MipLevels: 1, ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING, BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, MiscFlags: 0,
            };
            let mut tex: Option<ID3D11Texture2D> = None;
            if unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }.is_err() { return; }
            *guard = tex;
        }
        let Some(stage) = guard.as_ref() else { return; };
        stage.clone()
    };
    // 上缘用壁纸锚点挡掉开机过渡暗帧；内容区常有真实的深色窗口，不能用壁纸锚点判假。
    // 壁纸亮度是 sRGB 均值，先转到相对亮度再和采样值比较，口径必须一致。
    let anchor = crate::wallpaper::luma().map(srgb_linear_f32);
    for (rect, prev_atomic, pending_atomic, region_anchor) in [
        (top_rect, &THEME_LUMA, &THEME_DARK_PENDING, anchor),
        (content_rect, &THEME_LUMA_CONTENT, &THEME_DARK_PENDING_CONTENT, None),
    ] {
        let Some(luma) = sample_rect_luma(device, context, &stage, src_tex, sx, sy, sw, sh, rect) else { continue; };
        // 开机/切屏早期桌面还在合成，大幅变暗要连续两次确认；平滑变化/变亮立即采纳。
        let prev = f32::from_bits(prev_atomic.load(Ordering::Relaxed));
        let pending = pending_atomic.load(Ordering::Relaxed);
        let (commit, new_pending) = theme_luma_commit(prev, luma, pending, region_anchor);
        if let Some(v) = commit { prev_atomic.store(v.to_bits(), Ordering::Relaxed); }
        pending_atomic.store(new_pending, Ordering::Relaxed);
    }
}

pub fn log(msg: &str) {
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

/// 当前 WGC 会话绑定的拓扑签名；变化后立即登记稳定期。
static TOPO_SIG: AtomicU64 = AtomicU64::new(0);
/// 拓扑变化代数（单调递增）。D3D11 抓屏栈据此判断「当前 WGC 会话/背景纹理已过期，
/// 必须重建」——换分辨率/换显示模式后 item.Size() 和主屏 HMONITOR 都会变，
/// 旧会话继续 CopyResource 到旧尺寸纹理会静默失败，表现为玻璃永久定格。
static TOPO_EPOCH: AtomicU64 = AtomicU64::new(0);
/// 拓扑变化后的稳定期：这段时间内液态玻璃回退纯色，不抓过渡帧。
static SETTLE_UNTIL: Mutex<Option<std::time::Instant>> = Mutex::new(None);
/// 拓扑查询降频计数器：EnumDisplayMonitors 不便宜，约每 100ms 查一次足够。
static TOPO_TICK: AtomicU64 = AtomicU64::new(0);
/// 切换显示模式后等待桌面稳定的时长，固定 1000ms（原先的自学习逻辑已移除：
/// 学习值全局共享，快机器学出的 400ms 不够慢机器/复制模式的重构动画用）。
const SETTLE_MS: u64 = 1000;

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

/// 检测桌面拓扑是否刚变过（每约 100ms 实查一次，其余帧返回 false）。
/// 调用方：`App::frame` 在液态皮肤下每帧调用；收起态也要检测，否则只在
/// 展开抓屏时检测会整段错过显示模式切换。
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
    if previous == 0 || changed {
        log(&format!("desktop: sig={:016x} rects={}", sig, monitor_rects_desc()));
    }
    if changed {
        // 必须在检测到变化时就登记稳定期，不能等抓屏路径：用户切显示模式时
        // 岛仔通常是收起态，抓到中间态会让玻璃永久发黑。
        *SETTLE_UNTIL.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(SETTLE_MS));
        // 递增代数：D3D11 抓屏栈据此重建 WGC 会话 + 背景纹理。顺序上先 bump epoch
        // 再登记稳定期，抓屏栈读到的新 epoch 会让它丢掉旧会话、等稳定期过再抓新帧。
        TOPO_EPOCH.fetch_add(1, Ordering::SeqCst);
        log(&format!("desktop: 显示器拓扑变化 epoch={}，等待 {}ms 稳定后重建抓屏会话并重抓背景",
                     TOPO_EPOCH.load(Ordering::Relaxed), SETTLE_MS));
    }
    changed
}

/// 当前拓扑代数。D3D11 抓屏栈记录自己会话创建时的代数，不等就重建。
pub fn topo_epoch() -> u64 { TOPO_EPOCH.load(Ordering::SeqCst) }

/// 桌面拓扑刚变过、还在等 DWM 稳定：这段时间内**既不抓屏也不重建底片**。
/// 液态皮肤在 `paint_body` 里据此临时回退纯色，比继续用旧画面花掉要好。
pub fn settling() -> bool {
    SETTLE_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|t| std::time::Instant::now() < t)
}

// ===== 单测：锁定主题取样边界逻辑，确保尺寸变化时绝不越界 =====
#[cfg(test)]
mod theme_sample_tests {
    use super::{theme_sample_box, theme_sample_due, theme_luma_commit, THEME_STAGE_W, THEME_STAGE_H, THEME_SAMPLE_INTERVAL_MS};
    use super::{effective_luma, relative_luma, trimmed_median};

    #[test]
    fn relative_luma_is_perceptual_and_rec709_weighted(){
        assert!(relative_luma(255,255,255)>0.999);
        assert!(relative_luma(0,0,0)<0.001);
        let (red,green,blue)=(relative_luma(255,0,0),relative_luma(0,255,0),relative_luma(0,0,255));
        assert!(green>red*3.0 && green>blue*4.0,"绿色权重 0.7152 必须占主导：{red}/{green}/{blue}");
        assert!((relative_luma(128,128,128)-0.216).abs()<0.01,"sRGB 128 线性化后约 0.216");
    }

    #[test]
    fn trimmed_median_resists_small_bright_or_dark_patches(){
        let mut bright=vec![0.2;90]; bright.extend(vec![1.0;10]);
        assert!(trimmed_median(bright)<0.25,"10% 高亮块不该把整块亮度拉高");
        let mut dark=vec![0.8;90]; dark.extend(vec![0.0;10]);
        assert!(trimmed_median(dark)>0.75,"10% 暗块同样不该拉低");
    }

    #[test]
    fn effective_luma_prefers_content_when_regions_diverge(){
        assert_eq!(effective_luma(0.9,0.2),0.2,"上亮下暗：内容区优先，不能让亮上缘把主题拉浅");
        assert_eq!(effective_luma(0.2,0.9),0.9,"上暗下亮：内容区优先");
        assert!((effective_luma(0.6,0.5)-0.55).abs()<1e-6,"差异小于阈值取平均，行为与整块采样一致");
        assert_eq!(effective_luma(0.39,0.20),0.20,"跨过滞回带的差异就让内容区优先");
    }

    #[test]
    fn 开机过渡暗帧不立即采纳(){
        // 没有壁纸锚点时走连续两次确认
        assert_eq!(theme_luma_commit(0.85,0.2,false,None),(None,true));
        assert_eq!(theme_luma_commit(0.85,0.2,true,None),(Some(0.2),false));
        assert_eq!(theme_luma_commit(0.85,0.7,false,None),(Some(0.7),false));
        assert_eq!(theme_luma_commit(0.2,0.8,false,None),(Some(0.8),false));
        assert_eq!(theme_luma_commit(0.85,0.83,true,None),(Some(0.83),false));
    }
    #[test]
    fn 壁纸锚点挡掉过渡暗帧(){
        // 壁纸是浅色 0.9，抓屏采到 0.2（开机过渡帧）→ 直接拒绝，不进待确认
        assert_eq!(theme_luma_commit(0.85,0.2,false,Some(0.9)),(None,false));
        // 差 0.04（正常采样区间差异）→ 采纳
        assert_eq!(theme_luma_commit(0.85,0.86,false,Some(0.9)),(Some(0.86),false));
        // 真深色壁纸 0.15，抓屏采到 0.18 → 锚点一致；prev 已接近(0.17)，平滑采纳
        assert_eq!(theme_luma_commit(0.17,0.18,false,Some(0.15)),(Some(0.18),false));
        // prev 仍是旧浅色时，深色壁纸也要连续两次确认（防过渡帧）
        assert_eq!(theme_luma_commit(0.85,0.18,false,Some(0.15)),(None,true));
        assert_eq!(theme_luma_commit(0.85,0.18,true,Some(0.15)),(Some(0.18),false));
        // 恰好 0.25 边界 → 采纳（严格大于才拒绝）
        assert_eq!(theme_luma_commit(0.85,0.65,false,Some(0.9)),(Some(0.65),false));
    }

    // 按时间节流的纯判断：从未采过立即采；未到间隔不采；到间隔采。
    #[test]
    fn theme_sample_due_respects_interval() {
        let iv = THEME_SAMPLE_INTERVAL_MS;
        assert!(theme_sample_due(1000, None, iv), "首次应立即采样");
        assert!(!theme_sample_due(1499, Some(1000), iv), "间隔未满 500ms 不采");
        assert!(theme_sample_due(1500, Some(1000), iv), "正好 500ms 应采样");
        assert!(theme_sample_due(9999, Some(1000), iv), "远超间隔应采样");
    }

    // 低帧率场景：帧数少不该拖慢主题响应——只看时间是否到期。
    #[test]
    fn theme_sample_due_is_frame_rate_independent() {
        // 假设 5fps：200ms 一帧，累计 500ms 才 3 帧。关键是时间到就采，与帧数无关。
        let iv = THEME_SAMPLE_INTERVAL_MS;
        assert!(!theme_sample_due(400, Some(0), iv), "5fps 第 2 帧 400ms 未到期");
        assert!(theme_sample_due(600, Some(0), iv), "5fps 第 3 帧 600ms 已到期，立即采样");
        // 时间倒退（系统时钟回拨）时用 saturating_sub 兜底，不会误判成到期。
        assert!(!theme_sample_due(100, Some(500), iv), "时钟回拨不应触发采样");
    }

    // 正常情况：采样框完整落在显示器帧内，原样换算。
    #[test]
    fn theme_sample_box_normal_within_frame() {
        // rect=(10,20)-(110,80)，抓屏原点(0,0)，显示器 1920x1080
        assert_eq!(
            theme_sample_box(Some((10, 20, 110, 80)), 0, 0, 1920, 1080),
            Some((10, 20, 110, 80, 100, 60))
        );
    }

    // 抓屏原点非零（多显示器，主屏在右）：源 box 要叠加 sx/sy 偏移。
    #[test]
    fn theme_sample_box_respects_capture_origin() {
        // 抓屏原点 sx=100,sy=50；rect=(10,20)-(60,40) → box=(110,70)-(160,90)
        assert_eq!(
            theme_sample_box(Some((10, 20, 60, 40)), 100, 50, 1920, 1080),
            Some((110, 70, 160, 90, 50, 20))
        );
    }

    // 尺寸变化核心场景：采样框宽高超过固定 staging 上限，必须被夹住，
    // 这样读回时 from_raw_parts(ptr, rw*4) 的 rw 恒 ≤ 1024，绝不越界。
    #[test]
    fn theme_sample_box_clamps_to_staging_limit() {
        // 起点在帧内、宽高远超固定 staging 上限：必须夹到 1024×256 而非越界。
        let huge = (
            0,
            0,
            THEME_STAGE_W as i32 * 3,
            THEME_STAGE_H as i32 * 3,
        );
        let got = theme_sample_box(Some(huge), 0, 0, 100000, 100000);
        let (l, t, r, b, rw, rh) = got.expect("宽高夹取后仍应有效");
        assert_eq!(rw, THEME_STAGE_W, "rw 必须夹到 1024");
        assert_eq!(rh, THEME_STAGE_H, "rh 必须夹到 256");
        assert_eq!(r - l, THEME_STAGE_W as i32, "源 box 宽应与 rw 一致");
        assert_eq!(b - t, THEME_STAGE_H as i32, "源 box 高应与 rh 一致");
        // 读回字节数不得超过一行 pitch 下限（1024*4）——这就是防越界的本质。
        assert!((rw as usize) * 4 <= THEME_STAGE_W as usize * 4);
    }

    // 源 box 右下越出显示器帧 → 必须拒绝（CopySubresourceRegion 会失败）。
    #[test]
    fn theme_sample_box_rejects_out_of_frame() {
        // right=2000 超上限夹到 1024 后 right=1034 合法；这里用「夹取后仍越界」的坐标：
        // 起点 1500 + 夹取后的 1024 = 2524 > sw=1920
        assert_eq!(theme_sample_box(Some((1500, 10, 3000, 60)), 0, 0, 1920, 1080), None);
        // 起点 y=1000 + 夹取后的 256 = 1256 > sh=1080
        assert_eq!(theme_sample_box(Some((10, 1000, 60, 3000)), 0, 0, 1920, 1080), None);
        // 叠加非零原点后越界：sx=1900 + left=10 + 宽50 = 1960 > 1920
        assert_eq!(theme_sample_box(Some((10, 10, 60, 60)), 1900, 0, 1920, 1080), None);
    }

    // 负坐标：源 box 左上越出显示器帧 → 拒绝。
    #[test]
    fn theme_sample_box_rejects_negative_origin() {
        assert_eq!(theme_sample_box(Some((-5, 10, 40, 60)), 0, 0, 1920, 1080), None);
        assert_eq!(theme_sample_box(Some((10, -5, 40, 60)), 0, 0, 1920, 1080), None);
    }

    // 退化/非法矩形与非法显示器尺寸 → 拒绝。
    #[test]
    fn theme_sample_box_rejects_degenerate_and_invalid_frame() {
        assert_eq!(theme_sample_box(None, 0, 0, 1920, 1080), None);
        assert_eq!(theme_sample_box(Some((10, 10, 10, 60)), 0, 0, 1920, 1080), None); // 宽=0
        assert_eq!(theme_sample_box(Some((10, 10, 60, 10)), 0, 0, 1920, 1080), None); // 高=0
        assert_eq!(theme_sample_box(Some((60, 60, 10, 10)), 0, 0, 1920, 1080), None); // 翻转
        assert_eq!(theme_sample_box(Some((10, 10, 60, 60)), 0, 0, 0, 0), None);       // 空显示器
        assert_eq!(theme_sample_box(Some((10, 10, 60, 60)), 0, 0, -1920, 1080), None); // 负尺寸
    }

    // 极端 i32 坐标：全程 i64 中间运算，绝不溢出 panic（debug 构建也会过）。
    #[test]
    fn theme_sample_box_never_overflows() {
        // rx1-rx0 接近 i32 上限；斜杠坐标取 i32 极值。
        assert_eq!(
            theme_sample_box(Some((i32::MIN, i32::MIN, i32::MAX, i32::MAX)), 0, 0, 1920, 1080),
            None
        );
        // 抓屏原点为极值，与采样框相加不会 wrap 成"看似合法"的小坐标。
        assert_eq!(
            theme_sample_box(Some((1, 1, 50, 50)), i32::MAX, i32::MAX, 1920, 1080),
            None
        );
        assert_eq!(
            theme_sample_box(Some((1, 1, 50, 50)), i32::MIN, i32::MIN, 1920, 1080),
            None
        );
    }

    // 紧贴下/右边界：等于显示器帧尺寸是合法的（right==sw, bottom==sh）。
    #[test]
    fn theme_sample_box_allows_exact_edge() {
        assert_eq!(
            theme_sample_box(Some((1820, 1020, 1920, 1080)), 0, 0, 1920, 1080),
            Some((1820, 1020, 1920, 1080, 100, 60))
        );
    }

    // 属性式扫描：在多种显示器尺寸下，只要返回 Some，保证 rw/rh 不超上限、
    // box 落在帧内、且 w==rw/h==rh ——覆盖"尺寸频繁变化"的连续取值。
    #[test]
    fn theme_sample_box_invariant_across_sizes() {
        for sw in [640, 1280, 1920, 2560, 3840] {
            for sh in [480, 720, 1080, 1440, 2160] {
                for (rx0, ry0) in [(0, 0), (7, 13), (100, 200)] {
                    for (w, h) in [(50, 30), (1000, 400), (5000, 3000)] {
                        let rect = (rx0, ry0, rx0 + w, ry0 + h);
                        if let Some((l, t, r, b, rw, rh)) =
                            theme_sample_box(Some(rect), 0, 0, sw, sh)
                        {
                            assert!(rw <= THEME_STAGE_W && rh <= THEME_STAGE_H);
                            assert!(l >= 0 && t >= 0 && r <= sw && b <= sh);
                            assert_eq!(r - l, rw as i32);
                            assert_eq!(b - t, rh as i32);
                            assert!((rw as usize) * 4 <= THEME_STAGE_W as usize * 4);
                        }
                    }
                }
            }
        }
    }
}
