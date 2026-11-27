//! Windows 的非激活窗口、形状区域与命中测试。
//!
//! 对齐原版实测（`BUILD-ORDER-v1.md` §1/§5）：
//! - `WS_POPUP` + `WS_EX_TOPMOST|TOOLWINDOW|NOACTIVATE`，透明交换链窗口另有 `WS_EX_NOREDIRECTIONBITMAP`；
//! - `SetWindowRgn` 用三种矩形：收起 179×34@(230,0)、展开 512×186@(64,0)、
//!   极简收起 112×14@(264,0)，坐标是 640×190 逻辑点；
//! - 命中区与 region 分开：收起 143×22@(253,5)、展开 416×132@(110,5)，
//!   命中区外 `HTTRANSPARENT`，真实点击落给下层窗口；
//! - 前台窗口切走（`EVENT_SYSTEM_FOREGROUND`，且不是本进程）时通知主线程收起；
//! - 原版没有可用的 Esc 收起入口（§2.5），这里也不注册键盘钩子。

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use crate::poll_clock::PollClock;
use crate::window_bounds::{preserve_overlap, WindowBounds};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CombineRgn, CreatePolygonRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn, HRGN, RGN_OR, WINDING};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState,VK_LBUTTON,VK_RBUTTON};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

#[path = "native_capture.rs"]
mod capture;

const SUBCLASS_ID: usize = 0x4E55_5341; // "DAOZAI"
#[allow(dead_code)]
const WS_EX_NOREDIRECTIONBITMAP: isize = 0x0020_0000;

/// 命中区（物理像素，窗口坐标，left/top/right/bottom）。
static HIT: OnceLock<Mutex<Vec<(i32, i32, i32, i32)>>> = OnceLock::new();
/// 前台窗口切到别的进程时置位，由主线程消费。
static FG_CHANGED: AtomicBool = AtomicBool::new(false);
static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);
// 前台事件在安装钩子的 UI 线程上派发。
thread_local! {
    static FG_CONTEXT: std::cell::RefCell<Option<eframe::egui::Context>> = const { std::cell::RefCell::new(None) };
    static PAGE_WHEEL_TARGET: std::cell::Cell<Option<(HWND,RECT)>> = const { std::cell::Cell::new(None) };
    static RESIZING_CROP: std::cell::Cell<Option<(HWND,bool)>> = const { std::cell::Cell::new(None) };
}

// 仅在切页把鼠标留在岛外时接续垂直滚轮。点击、横滚和移出都会立即交还下层窗口。
unsafe extern "system" fn on_page_wheel(code:i32,wp:WPARAM,lp:LPARAM)->LRESULT {
    if code==HC_ACTION as i32 {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL,VK_MENU,VK_SHIFT};
        let msg=wp.0 as u32;
        let modified=msg==WM_MOUSEWHEEL && [VK_CONTROL,VK_MENU,VK_SHIFT].iter().any(|key|GetAsyncKeyState(key.0 as i32)<0);
        if route_page_wheel(msg,&*(lp.0 as *const MSLLHOOKSTRUCT),modified) {return LRESULT(1);}
    }
    CallNextHookEx(None,code,wp,lp)
}

unsafe fn route_page_wheel(msg:u32,event:&MSLLHOOKSTRUCT,modified:bool)->bool {
    let Some((hwnd,area))=PAGE_WHEEL_TARGET.get() else {return false};
    let p=event.pt;
    let inside=p.x>=area.left && p.x<area.right && p.y>=area.top && p.y<area.bottom;
    if inside && msg==WM_MOUSEMOVE {return false;}
    if inside && msg==WM_MOUSEWHEEL {
        // 新页边缘与续滚范围略有重叠，落在可操作区域内的滚轮仍走原生分发。
        let mut window=RECT::default();
        if GetWindowRect(hwnd,&mut window).is_ok() && inside_hit(p.x-window.left,p.y-window.top) {return false;}
        if !modified {
            let pos=LPARAM(((p.x as u16 as u32) | ((p.y as u16 as u32)<<16)) as isize);
            if PostMessageW(hwnd,WM_MOUSEWHEEL,WPARAM((event.mouseData & 0xffff0000) as usize),pos).is_ok() {return true;}
        }
    }
    PAGE_WHEEL_TARGET.set(None);
    FG_CONTEXT.with_borrow(|ctx|{if let Some(ctx)=ctx {ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);}});
    false
}

// `WS_EX_NOREDIRECTIONBITMAP` 只能在 CreateWindowEx 时设置（运行时 SetWindowLongPtrW
// 会回 ERROR_INVALID_PARAMETER），本项目用 GL 合成窗口，加上这一位后画面上什么都
// 看不到，因此不使用；具体见 `apply_style` 注释与交付说明。

fn hit() -> &'static Mutex<Vec<(i32, i32, i32, i32)>> {
    HIT.get_or_init(|| Mutex::new(Vec::new()))
}

fn inside_hit(x: i32, y: i32) -> bool {
    hit().lock().unwrap_or_else(|e|e.into_inner()).iter().any(|&(l,t,r,b)|x>=l&&x<r&&y>=t&&y<b)
}

fn file_drag_region(region:(i32,i32,i32,i32),zone:Option<(i32,i32,i32,i32)>,capture:bool)->(i32,i32,i32,i32){
    match zone.filter(|_|capture) {
        Some(zone)=>(region.0.min(zone.0),region.1.min(zone.1),region.2.max(zone.2),region.3.max(zone.3)),
        None=>region,
    }
}

#[derive(Default)]
struct DragCapture {
    origin: Option<(i32,i32)>,
    moved: bool,
}

impl DragCapture {
    fn update(&mut self,down:bool,cursor:Option<(i32,i32)>,zone:Option<(i32,i32,i32,i32)>,threshold:(i32,i32))->bool {
        let Some(cursor)=cursor.filter(|_|down && zone.is_some()) else {
            *self=Self::default();
            return false;
        };
        let inside=|(x,y):(i32,i32)|zone.is_some_and(|(l,t,r,b)|x>=l && x<r && y>=t && y<b);
        let origin=*self.origin.get_or_insert(cursor);
        self.moved|=(cursor.0-origin.0).abs()>threshold.0 || (cursor.1-origin.1).abs()>threshold.1;
        // 只接收从感应区外拖进来的文件：在岛下方的标签栏里按下（含连续点击、拖标签、选文字）
        // 不能扩大窗口区域，否则 HTTRANSPARENT 拦不住跨进程点击，下层窗口会点不动。
        self.moved && !inside(origin) && inside(cursor)
    }
}

#[derive(Clone,Copy,Default)]
struct PointerWake {
    canvas: Option<RECT>,
    pill_edge: Option<RECT>,
}

impl PointerWake {
    fn pill_edge_contains(self, cursor:POINT)->bool {
        let Some((canvas,edge))=self.canvas.zip(self.pill_edge) else {return false;};
        let (x,y)=(cursor.x-canvas.left,cursor.y-canvas.top);
        x>=edge.left && x<edge.right && y>=edge.top && y<edge.bottom
    }
}

fn collapsed_pill_edge(hit:eframe::egui::Rect,ppp:f32,dock:crate::model::Dock)->Option<RECT> {
    if !hit.is_positive() || dock.is_side() {return None;}
    let gap=crate::app::PILL_GAP.min(hit.height());
    let (top,bottom)=if dock==crate::model::Dock::Bottom {(hit.bottom()-gap,hit.bottom())}
        else {(hit.top(),hit.top()+gap)};
    let px=|v:f32|(v*ppp).round() as i32;
    Some(RECT{left:px(hit.left()),top:px(top),right:px(hit.right()),bottom:px(bottom)})
}

/// 每 80 ms 检查一次拖放感应区，以及收不到鼠标消息的极简细条和胶囊空隙。
fn watch_drag(hwnd:HWND,ctx:eframe::egui::Context,wake:Arc<Mutex<PointerWake>>){
    let raw=hwnd.0 as isize;
    let mut was_down=false;
    let mut last_hover=None;
    let mut was_over_pill_edge=false;
    let _=std::thread::Builder::new().name("daozai-drag-watch".into()).spawn(move||loop{
        std::thread::sleep(std::time::Duration::from_millis(80));
        unsafe {
            let hwnd=HWND(raw as *mut _);
            if !IsWindow(hwnd).as_bool(){return;}
            let down=GetAsyncKeyState(VK_LBUTTON.0 as i32)<0 || GetAsyncKeyState(VK_RBUTTON.0 as i32)<0;
            // 松开后补一帧，让拖放状态及时复位；否则静止时一秒才画一帧，连续点击会沿用上次的按下位置。
            if was_down && !down {ctx.request_repaint();}
            was_down=down;
            let passive=GetWindowLongPtrW(hwnd,GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as isize != 0;
            let wake=*wake.lock().unwrap_or_else(|e|e.into_inner());
            let mut cursor=POINT::default();
            if (down || passive || wake.pill_edge.is_some()) && GetCursorPos(&mut cursor).is_err(){continue;}
            let over_pill_edge=!down && wake.pill_edge_contains(cursor);
            if over_pill_edge && !was_over_pill_edge {ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);}
            was_over_pill_edge=over_pill_edge;
            if !down && !passive {last_hover=None;continue;}
            let mut rect=RECT::default();
            if GetWindowRect(hwnd,&mut rect).is_err(){continue;}
            if down {
                // 感应区允许从可见框外拖入，缩紧外框后仍按原画布唤醒。
                if let Some(bounds)=wake.canvas {rect=bounds;}
            } else {
                // 细条点穿后收不到鼠标移入事件，在它的可见区域内移动才唤醒绘制。
                let pos=(cursor.x-rect.left,cursor.y-rect.top);
                if last_hover.replace(pos)==Some(pos){continue;}
                let mut bounds=RECT::default();
                windows::Win32::Graphics::Gdi::GetWindowRgnBox(hwnd,&mut bounds);
                if pos.0<bounds.left || pos.0>=bounds.right || pos.1<bounds.top || pos.1>=bounds.bottom {continue;}
            }
            if cursor.x>=rect.left && cursor.x<rect.right && cursor.y>=rect.top && cursor.y<rect.bottom {ctx.request_repaint();}
        }
    });
}

// 图形上下文和原生样式就绪后重新启用逐像素合成；不能只依赖创建 HWND 时的请求。
fn enable_alpha_composition(hwnd:HWND)->windows::core::Result<()> {
    #[cfg(feature = "sdk-material")]
    if eframe::egui_wgpu::winit::direct_composition_active(hwnd.0 as isize) {return Ok(());}
    use windows::Win32::Graphics::Dwm::{DwmEnableBlurBehindWindow,DWM_BLURBEHIND,DWM_BB_ENABLE,DWM_BB_BLURREGION};
    unsafe {
        let region=CreateRectRgn(0,0,-1,-1);
        if region.0.is_null(){return Err(windows::core::Error::from_win32());}
        let blur=DWM_BLURBEHIND{dwFlags:DWM_BB_ENABLE|DWM_BB_BLURREGION,fEnable:true.into(),hRgnBlur:region,..Default::default()};
        let result=DwmEnableBlurBehindWindow(hwnd,&blur);
        let _=DeleteObject(region);
        result
    }
}

// ---------- 毛玻璃：窗口区域 ----------
//
// 兼容后端（无逐像素透明）按轮廓裁剪窗口区域；毛玻璃的实时模糊见 `composition.rs`。

/// Windows「透明效果」开关；读不到时返回 None。
fn system_transparency() -> Option<bool> {
    use windows::{core::w, Win32::System::Registry::*};
    let mut value = 1u32; let mut size = 4u32;
    let ok = unsafe { RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("EnableTransparency"), RRF_RT_REG_DWORD, None, Some((&mut value as *mut u32).cast()), Some(&mut size)) }.is_ok();
    ok.then_some(value != 0)
}

/// 是否使用合成引擎的实时模糊。系统「透明效果」关闭时宿主背景画刷只会画出一块纯色，
/// 却仍要合成器为这扇窗口持续工作；部分核显旧驱动在这种状态下会整机闪屏卡死，因此直接不用，
/// 由岛体自己画半透明底色。注册表每 2 秒最多读一次；状态变化时写一条诊断。
fn compositor_blur_allowed() -> bool {
    use std::cell::Cell;
    thread_local! { static CACHE: Cell<(Option<std::time::Instant>, Option<bool>)> = const { Cell::new((None, None)) }; }
    CACHE.with(|cache| {
        let (at, last) = cache.get();
        if let (Some(at), Some(last)) = (at, last) { if at.elapsed() < Duration::from_secs(2) { return last; } }
        let allowed = system_transparency() != Some(false);
        if last.is_some_and(|last| last != allowed) || last.is_none() && !allowed {
            glass_diagnostic(if allowed { "系统透明效果已开启，恢复合成引擎模糊" } else { "系统透明效果关闭，不使用合成引擎模糊" });
        }
        cache.set((Some(std::time::Instant::now()), Some(allowed)));
        allowed
    })
}

/// 毛玻璃诊断：写到数据目录的 glass-diagnostics.txt（只记录状态变化，不会持续增长）。
fn glass_diagnostic(message: &str) {
    use std::io::Write;
    let path = crate::model::config_dir().join("glass-diagnostics.txt");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 64 * 1024) { let _ = std::fs::write(&path, b""); }
    let transparency = {
        match system_transparency() { Some(true) => "开", Some(false) => "关", None => "未知" }
    };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{} · {} · 系统透明效果：{}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), message, transparency);
    }
}

/// 把逻辑坐标的轮廓换算成物理像素，作为去重的键和区域的输入。
fn glass_key(shapes: &[crate::app::GlassShape], ppp: f32) -> Vec<i32> {
    use crate::app::GlassShape;
    let px = |v: f32| (v * ppp).round() as i32;
    let mut key = Vec::new();
    for shape in shapes {
        match shape {
            GlassShape::Outline(_) | GlassShape::Notch { .. } | GlassShape::Docked { .. } => {
                let points = shape.outline();
                key.push(-1);
                key.push(points.len() as i32);
                for p in &points { key.push(px(p.x)); key.push(px(p.y)); }
            }
            GlassShape::Round(r, radius) => {
                key.extend([-2, px(r.left()), px(r.top()), px(r.right()), px(r.bottom()), px(*radius * 2.0)]);
            }
        }
    }
    key
}

/// 按 `glass_key` 的结果合成窗口区域（多块轮廓取并集）。
unsafe fn glass_region(key: &[i32]) -> Option<HRGN> {
    let mut out: Option<HRGN> = None;
    let mut i = 0;
    while i < key.len() {
        let part = match key[i] {
            -1 => {
                let n = key[i + 1] as usize;
                let points: Vec<POINT> = (0..n).map(|j| POINT { x: key[i + 2 + j * 2], y: key[i + 3 + j * 2] }).collect();
                i += 2 + n * 2;
                if points.len() < 3 { continue; }
                CreatePolygonRgn(&points, WINDING)
            }
            _ => {
                let (l, t, r, b, d) = (key[i + 1], key[i + 2], key[i + 3], key[i + 4], key[i + 5]);
                i += 6;
                CreateRoundRectRgn(l, t, r + 1, b + 1, d, d)
            }
        };
        if part.0.is_null() { continue; }
        match out {
            None => out = Some(part),
            Some(all) => {
                let _ = CombineRgn(all, all, part, RGN_OR);
                let _ = DeleteObject(part);
            }
        }
    }
    out
}

/// 窗口接管设置成功的区域，设置失败时由调用方释放。
pub(crate) fn apply_outline(hwnd:HWND,key:&[i32],redraw:bool)->bool {
    apply_outline_with_feather(hwnd,key,redraw,false,[0,0])
}

fn apply_outline_with_feather(hwnd:HWND,key:&[i32],redraw:bool,feather:bool,[x,y]:[i32;2])->bool {
    use windows::Win32::Graphics::Gdi::{OffsetRgn, RGN_COPY};
    unsafe {
        let Some(rgn)=glass_region(key) else{return false;};
        if feather {
            // 常驻窗口仍按形状让出透明留白，边界保留 2 个物理像素给羽化和坐标取整。
            let edge=CreateRectRgn(0,0,0,0);
            if !edge.0.is_null() {
                for (dx,dy) in [(1,0),(-1,0),(0,1),(0,-1)] {
                    for _ in 0..2 {
                        let _=CombineRgn(edge,rgn,HRGN::default(),RGN_COPY);
                        let _=OffsetRgn(edge,dx,dy);
                        let _=CombineRgn(rgn,rgn,edge,RGN_OR);
                    }
                }
                let _=DeleteObject(edge);
            }
        }
        if x!=0 || y!=0 {let _=OffsetRgn(rgn,-x,-y);}
        if SetWindowRgn(hwnd,rgn,redraw)!=0 {return true;}
        let _=DeleteObject(rgn);
        false
    }
}

/// 命中区外 `HTTRANSPARENT`（鼠标投给下层窗口），命中区内 `HTCLIENT`。
unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg==WM_NCCALCSIZE && wp.0!=0 && lp.0!=0
        && RESIZING_CROP.get().is_some_and(|(window,_)|window==hwnd)
    {
        let params=lp.0 as *mut NCCALCSIZE_PARAMS;
        let previous=(*params).rgrc[2];
        let result=DefSubclassProc(hwnd,msg,wp,lp);
        if let Some(common)=preserve_overlap(previous,(*params).rgrc[0]) {
            // 有效源和目标都是同一块屏幕区域。默认保留客户区左上角会把旧画面搬走。
            (*params).rgrc[1]=common;
            (*params).rgrc[2]=common;
            RESIZING_CROP.set(Some((hwnd,true)));
            return LRESULT(WVR_VALIDRECTS as isize);
        }
        return result;
    }
    if msg==WM_DISPLAYCHANGE || (msg==WM_SETTINGCHANGE && wp.0==SPI_SETWORKAREA.0 as usize) {
        DISPLAY_CHANGED.store(true,Ordering::Relaxed);
    }
    if matches!(msg,WM_DWMCOMPOSITIONCHANGED|WM_DISPLAYCHANGE|WM_DWMCOLORIZATIONCOLORCHANGED)
        || (msg==WM_SHOWWINDOW && wp.0!=0) {
        // 隐藏创建到首次显示、隐藏后恢复时也要恢复透明；等框架处理完窗口状态再应用。
        let result=DefSubclassProc(hwnd,msg,wp,lp);
        let _=enable_alpha_composition(hwnd);
        return result;
    }
    // 保留框架的焦点通知；lParam=-1 禁止系统在激活时重画透明窗口的顶边。
    if msg==WM_NCACTIVATE {return DefSubclassProc(hwnd,msg,wp,LPARAM(-1));}
    if msg==WM_NCPAINT {return LRESULT(0);}
    if msg == WM_MOUSEACTIVATE
        && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_NOACTIVATE.0 as isize != 0
    {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if msg == WM_NCHITTEST {
        let x = (lp.0 & 0xffff) as u16 as i16 as i32;
        let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32;
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let px = x - rect.left;
            let py = y - rect.top;
            return if inside_hit(px, py) {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            };
        }
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND || hwnd.0.is_null() {
        return;
    }
    let mut pid = 0u32;
    let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    // 自己（含托盘窗口）拿到前台不算「切走」
    if pid != 0 && pid != GetCurrentProcessId() {
        PAGE_WHEEL_TARGET.set(None);
        FG_CHANGED.store(true, Ordering::Relaxed);
        FG_CONTEXT.with_borrow(|context| {
            if let Some(ctx)=context {ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);}
        });
    }
}

/// winit 会在显示/改属性时重写窗口样式，所以每帧按需兜一次底。
unsafe fn apply_style(hwnd: HWND, editing: bool, raise: bool) {
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    let want = (style | WS_POPUP.0 as isize)
        & !((WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0)
            as isize);
    let mut frame_changed = false;
    if want != style {
        SetWindowLongPtrW(hwnd, GWL_STYLE, want);
        frame_changed = true;
    }
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    // 透明交换链的 WS_EX_NOREDIRECTIONBITMAP 在创建窗口时设置，这里保留已有的位。
    // 去掉会露在任务栏的 APPWINDOW；拖放由 OLE RegisterDragDrop 处理，也去掉 ACCEPTFILES。
    // 置顶由 SetWindowPos 管理，避免样式标记与实际窗口层级脱节。
    let mut want_ex = (ex | WS_EX_TOOLWINDOW.0 as isize)
        & !(WS_EX_APPWINDOW.0 as isize)
        & !(WS_EX_ACCEPTFILES.0 as isize);
    if editing {
        want_ex &= !(WS_EX_NOACTIVATE.0 as isize);
    } else {
        want_ex |= WS_EX_NOACTIVATE.0 as isize;
    }
    if want_ex != ex {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want_ex);
    }
    let topmost = ex & WS_EX_TOPMOST.0 as isize != 0;
    if frame_changed || !topmost || raise {
        // 只有真的 SetWindowPos(HWND_TOPMOST) 才会让 TOPMOST 位生效
        // 每次展开重新抬高实际层级；标记存在时仍可能被别的窗口盖住。
        let mut flags=SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        // 输入状态只切换激活权限，避免重画无边框窗口的非客户区。
        if frame_changed {flags|=SWP_FRAMECHANGED;}
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            flags,
        );
    }
}

/// 这些渲染路径交给系统的画面可能没有可用的透明通道，透明画布会显示成黑框（#72：AMD RX 6750 GRE 等）。
/// 但同为 AMD，也有不少机器逐像素透明完全正常（#152 RX 5600、#114 AMD 核显）。所以这里只决定
/// 「不直接信任系统合成」：画面改由分层窗口逐像素显示（见 `present_frame`），两类机器都不会有黑框，
/// 边缘也保持抗锯齿；不再像 v0.6.63–0.6.64 那样直接用二值的窗口区域硬裁剪（圆角、肩部全是台阶）。
fn surface_needs_outline_clip(backend:Option<eframe::wgpu::Backend>,vendor:&str,renderer:&str)->bool {
    use eframe::wgpu::Backend;
    if matches!(backend,Some(Backend::Dx12|Backend::Vulkan)){return true;}
    if renderer.to_ascii_lowercase().contains("radeon"){return true;}
    [vendor,renderer].into_iter().any(|name|{
        let name=name.to_ascii_lowercase();
        name.contains("advanced micro devices") || name.split(|c:char|!c.is_ascii_alphanumeric()).any(|word|matches!(word,"amd"|"ati"))
    })
}

/// 只识别同屏顶部的宽条窗口，普通弹窗、全屏窗口和其他屏幕不触发抬高。
unsafe fn top_bar_above(hwnd:HWND,bounds:crate::display_layout::Bounds,target:Option<RECT>)->bool {
    use windows::Win32::Graphics::Gdi::{GetWindowRgnBox, NULLREGION};
    use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

    let mut island=RECT::default();
    if !IsWindowVisible(hwnd).as_bool() || GetWindowRect(hwnd,&mut island).is_err(){return false;}
    if let Some(target)=target {
        if GetWindowRgnBox(hwnd,&mut RECT::default())==NULLREGION {return false;}
        island.left=island.left.max(target.left);
        island.top=island.top.max(target.top);
        island.right=island.right.min(target.right);
        island.bottom=island.bottom.min(target.bottom);
        if island.left>=island.right || island.top>=island.bottom {return false;}
    }
    let mut current=hwnd;
    let click_through=(WS_EX_LAYERED|WS_EX_TRANSPARENT).0 as isize;
    while let Ok(previous)=GetWindow(current,GW_HWNDPREV) {
        current=previous;
        let style=GetWindowLongPtrW(current,GWL_EXSTYLE);
        if style&WS_EX_TOPMOST.0 as isize==0 || style&click_through==click_through
            || !IsWindowVisible(current).as_bool() || !IsWindowEnabled(current).as_bool() {continue;}
        let mut bar=RECT::default();
        if GetWindowRect(current,&mut bar).is_ok()
            && bar.top<=bounds.top && bar.bottom>bounds.top
            && bar.bottom-bar.top<=(bounds.bottom-bounds.top)/8
            && bar.right-bar.left>=(bounds.right-bounds.left)/2
            && bar.left<island.right && bar.right>island.left
            && bar.top<island.bottom && bar.bottom>island.top {
            return true;
        }
    }
    false
}

/// 兼容后端的分层窗口状态。Direct3D 首帧交付前先保持主窗口的区域为空。
#[derive(Default)]
enum Layering {
    Starting(Option<Instant>),
    /// 不需要（透明合成正常）或还没收到第一帧。
    #[default]
    Pending,
    Active(crate::layered::Layered),
    /// 读不到 alpha 或系统拒绝：一直用轮廓裁剪。
    Unavailable,
}

/// 本机验证分层显示的开关：None 不启用；Some(false) 按需要分层的显卡处理；Some(true) 另外假装读回的画面没有透明通道。
fn outline_test()->Option<bool> {
    static VALUE:std::sync::OnceLock<Option<bool>>=std::sync::OnceLock::new();
    *VALUE.get_or_init(||match std::env::var("DAOZAI_OUTLINE_TEST").ok()?.trim().to_ascii_lowercase().as_str() {
        "alpha"=>Some(false),
        "opaque"=>Some(true),
        _=>None,
    })
}

#[derive(Default)]
struct Crop {
    full: Option<RECT>,
    wide_placement: Option<(f32, f32)>,
    applied: Option<RECT>,
    plan: WindowBounds,
    requested: Option<(eframe::egui::Rect,bool)>,
    supported: bool,
    managed_surface: bool,
    canvas_changed: bool,
    gl: Option<Arc<eframe::glow::Context>>,
    front_ready: bool,
    front_pixels: Vec<u8>,
    bridge: Option<crate::layered::Layered>,
    bridge_visible: bool,
    drag_wake: Arc<Mutex<PointerWake>>,
}

pub struct Window {
    hwnd: HWND,
    capture_exclusion: capture::Exclusion,
    crop: Crop,
    layering: Layering,
    region: (i32, i32, i32, i32),
    expanded: bool,
    hook: HWINEVENTHOOK,
    page_wheel_hook: Option<HHOOK>,
    browser_state: (Option<bool>,PollClock),
    drag_capture: DragCapture,
    /// 当前生效的贴合轮廓（物理像素键）、毛玻璃的合成模糊层与其状态。
    glass: Option<Vec<i32>>,
    backdrop: Option<crate::composition::Backdrop>,
    blur: bool,
    blur_unavailable: bool,
    opaque_surface: bool,
    /// 读回画面没有透明通道时，用来补算边缘 alpha 的岛体轮廓（物理像素）。
    outline: ShapeAlpha,
}

/// 按岛体轮廓补出来的 alpha：轮廓每帧随 `update` 更新，蒙版只在轮廓或画面尺寸变化时重算。
#[derive(Default)]
struct ShapeAlpha {
    /// 读回的画面确实没有透明通道，正在用轮廓补 alpha。
    active: bool,
    key: Vec<i32>,
    polygons: Vec<Vec<(f32, f32)>>,
    mask: Option<([usize; 2], Vec<i32>, Vec<u8>)>,
    frame: Vec<u8>,
}

impl ShapeAlpha {
    fn set_shapes(&mut self, shapes: &[crate::app::GlassShape], ppp: f32) {
        let key = glass_key(shapes, ppp);
        if key == self.key { return; }
        self.polygons = shapes.iter().map(|shape| shape.outline().iter().map(|p| (p.x * ppp, p.y * ppp)).collect()).collect();
        self.key = key;
    }

    /// 把读回的画面套上轮廓 alpha，返回交给分层窗口的画面。
    fn apply(&mut self, size: [usize; 2], rgba: &[u8]) -> &[u8] {
        if !self.mask.as_ref().is_some_and(|(s, k, _)| *s == size && *k == self.key) {
            self.mask = Some((size, self.key.clone(), crate::layered::coverage_mask(size, &self.polygons)));
        }
        let (_, _, mask) = self.mask.as_ref().unwrap();
        crate::layered::apply_mask(rgba, mask, &mut self.frame);
        &self.frame
    }
}

impl Window {
    pub fn hwnd(&self)->HWND {self.hwnd}

    pub fn capture_exclusion_supported() -> bool { capture::supported() }

    pub fn set_capture_exclusion(&mut self, desired: bool) {
        let targets = self.capture_targets();
        self.capture_exclusion.set(desired, targets);
    }

    #[cfg(feature = "sdk-material")]
    pub fn ensure_material_capture_exclusion(&mut self) -> Result<bool, String> {
        let targets = self.capture_targets();
        self.capture_exclusion.ensure(targets)
    }

    #[cfg(feature = "sdk-material")]
    pub fn restore_capture_exclusion(&mut self, desired: bool) {
        let targets = self.capture_targets();
        self.capture_exclusion.restore(desired, targets);
    }

    pub fn take_capture_exclusion_error(&mut self) -> Option<String> {
        self.capture_exclusion.take_error()
    }

    fn capture_targets(&self) -> capture::Targets {
        use capture::Target;
        [
            Some(Target { hwnd: self.hwnd, name: "岛仔主窗口" }),
            match &self.layering {
                Layering::Active(layered) => Some(Target { hwnd: layered.display(), name: "岛仔分层显示窗口" }),
                _ => None,
            },
            self.crop.bridge.as_ref().map(|layer| Target { hwnd: layer.display(), name: "岛仔裁剪过渡窗口" }),
            self.backdrop.as_ref().map(|backdrop| Target { hwnd: backdrop.hwnd(), name: "岛仔毛玻璃底板" }),
        ]
    }

    fn apply_capture_to_new_window(&mut self, hwnd: HWND, name: &'static str) {
        let existing = self.capture_targets();
        self.capture_exclusion.add(capture::Target { hwnd, name }, existing);
    }

    pub fn page_scroll_cancelled(&self)->bool {
        self.page_wheel_hook.is_some() && PAGE_WHEEL_TARGET.get().is_none()
    }

    pub fn set_page_scroll_area(&mut self,area:Option<eframe::egui::Rect>,ppp:f32) {
        unsafe {
            if let Some((r,window))=area.zip(self.canvas_rect()) {
                let x=|v:f32|window.left+(v*ppp).round() as i32;
                let y=|v:f32|window.top+(v*ppp).round() as i32;
                if self.page_wheel_hook.is_none() {
                    use windows::Win32::{Foundation::HINSTANCE,System::LibraryLoader::GetModuleHandleW};
                    self.page_wheel_hook=GetModuleHandleW(None).ok().and_then(|m|
                        SetWindowsHookExW(WH_MOUSE_LL,Some(on_page_wheel),HINSTANCE(m.0),0).ok());
                }
                PAGE_WHEEL_TARGET.set(Some((self.hwnd,RECT{left:x(r.left()),top:y(r.top()),right:x(r.right()),bottom:y(r.bottom())})));
            } else if let Some(hook)=self.page_wheel_hook.take() {
                PAGE_WHEEL_TARGET.set(None);
                let _=UnhookWindowsHookEx(hook);
            }
        }
    }

    pub fn holding_through_strip(&self)->bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::VK_MBUTTON;
        unsafe {GetWindowLongPtrW(self.hwnd,GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as isize != 0
            && [VK_LBUTTON,VK_RBUTTON,VK_MBUTTON].iter().any(|key|GetAsyncKeyState(key.0 as i32)<0)}
    }

    /// 顶栏遮挡通知或整岛时，恢复输入窗口及分层画面的层级，保留前台焦点。
    pub fn keep_above_top_bar(&mut self,bounds:crate::display_layout::Bounds,notice:Option<(eframe::egui::Rect,f32)>) {
        let target=if let Some((rect,ppp))=notice {
            let Some(canvas)=self.canvas_rect() else {return;};
            let x=|v:f32|canvas.left+(v*ppp).round() as i32;
            let y=|v:f32|canvas.top+(v*ppp).round() as i32;
            Some(RECT{left:x(rect.left()),top:y(rect.top()),right:x(rect.right()),bottom:y(rect.bottom())})
        } else {None};
        let raised=unsafe{
            top_bar_above(self.hwnd,bounds,target)
                && SetWindowPos(self.hwnd,HWND_TOPMOST,0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).is_ok()
        };
        if raised {
            if let Layering::Active(layered)=&mut self.layering {layered.sync(self.hwnd,true);}
        }
    }

    #[cfg(feature="d3d11-glass")]
    /// D3D11 栈直接传 HWND + egui Context（不再经由 eframe CreationContext）。
    pub fn new(hwnd: windows::Win32::Foundation::HWND, egui_ctx: egui::Context) -> Option<Self> {
        let hook = unsafe {
            apply_style(hwnd, false, false);
            let _ = enable_alpha_composition(hwnd);
            let _ = SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0);
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if !hook.is_invalid() {
            FG_CONTEXT.set(Some(egui_ctx.clone()));
        }
        // 区域用哨兵值：首帧（包括启动即隐藏的空区域）也必须真正调用 SetWindowRgn，
        // 否则整块透明画布会挡住下层程序的点击。
        // D3D11+DComp 有逐像素 alpha，不需要旧 wgpu 时代的轮廓裁剪兜底。
        let opaque_surface = false;
        let wake = Arc::new(Mutex::new(PointerWake::default()));
        watch_drag(hwnd, egui_ctx.clone(), wake);
        Some(Self {
            hwnd,
            capture_exclusion: capture::Exclusion::default(),
            crop: Crop::default(),
            page_wheel_hook: None,
            layering: Layering::Pending,
            region: (i32::MIN, 0, 0, 0),
            expanded: false,
            hook,
            browser_state: (None, PollClock::default()),
            drag_capture: DragCapture::default(),
            glass: None,
            backdrop: None,
            blur: false,
            blur_unavailable: false,
            opaque_surface,
            outline: ShapeAlpha::default(),
        })
    }

    pub fn from_cc(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let RawWindowHandle::Win32(handle) = cc.window_handle().ok()?.as_raw() else { return None };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        #[cfg(feature = "sdk-material")]
        let direct_composition=eframe::egui_wgpu::winit::direct_composition_active(handle.hwnd.get());
        #[cfg(not(feature = "sdk-material"))]
        let direct_composition=false;
        let hook = unsafe {
            apply_style(hwnd, false, false);
            let _=enable_alpha_composition(hwnd);
            let _ = SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0);
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if !hook.is_invalid(){FG_CONTEXT.set(Some(cc.egui_ctx.clone()));}
        // 区域用哨兵值：首帧（包括启动即隐藏的空区域）也必须真正调用 SetWindowRgn，
        // 否则整块透明画布会挡住下层程序的点击。
        let backend=cc.wgpu_render_state.as_ref().map(|state|state.adapter.get_info().backend);
        let (vendor,renderer)=cc.gl.as_ref().map(|gl|{
            use eframe::glow::HasContext;
            unsafe{(gl.get_parameter_string(eframe::glow::VENDOR),gl.get_parameter_string(eframe::glow::RENDERER))}
        }).unwrap_or_default();
        // 本机验证用：DAOZAI_OUTLINE_TEST=alpha 按 AMD 等路径处理；=opaque 再假装读回的画面没有透明通道。
        let opaque_surface=!direct_composition && (surface_needs_outline_clip(backend,&vendor,&renderer) || outline_test().is_some());
        if opaque_surface{glass_diagnostic(&format!("渲染兼容：{vendor} {renderer} {backend:?}，不直接信任系统透明合成，首帧前先按轮廓裁剪"));}
        // 收紧外框只走 OpenGL 前缓冲读回；GPU 材质的透明交换链不收紧。
        let crop=Crop{supported:cc.gl.is_some() && !opaque_surface && !direct_composition,gl:cc.gl.clone(),..Default::default()};
        watch_drag(hwnd,cc.egui_ctx.clone(),Arc::clone(&crop.drag_wake));
        let layering=if direct_composition {Layering::Unavailable}
            else if backend==Some(eframe::wgpu::Backend::Dx12) {Layering::Starting(None)} else {Layering::Pending};
        Some(Self { hwnd, capture_exclusion: capture::Exclusion::default(), crop, layering, region: (i32::MIN, 0, 0, 0), expanded: false, hook, page_wheel_hook:None, browser_state:(None,PollClock::default()), drag_capture:DragCapture::default(), glass:None, backdrop:None, blur:false, blur_unavailable:false, opaque_surface, outline:ShapeAlpha::default() })
    }

    /// 使用目标屏幕的物理坐标定位，并在缩放、岛体大小、水平位置或屏幕连接变化后重新校正。
    /// `scale` 是主窗口的实际像素比（系统 DPI × 岛体大小），画布按它同步调整物理尺寸。
    /// `shift` 是 OLED 像素偏移的位移（物理像素），按当前停靠画面的屏幕余量收窄后叠加。
    pub fn place_on_display(&mut self, scale:f32, bounds:crate::display_layout::Bounds, offset:f32, dock:crate::model::Dock, shift:(i32,i32), horizontal_width:f32)->bool {
        unsafe {
            let horizontal_width=horizontal_width.max(crate::app::WIN_W);
            // 顶部：水平居中 + 水平偏移；左右：贴边 + 竖直偏移，画布换成竖向的大小。
            let origin=if dock.is_side() {
                crate::display_layout::side_origin(bounds,dock,(crate::side_dock::SIDE_W,crate::side_dock::SIDE_H),crate::side_dock::SIDE_HALF_EXTENT,scale,offset)
            } else {crate::display_layout::horizontal_origin_for_width(bounds,dock,scale,offset,horizontal_width)};
            let Some((x,y))=origin else{return false;};
            // OLED 像素偏移只沿不贴边的一轴让开。位置拉满或屏幕太窄时少挪、不挪，
            // 贴边的一侧也保持严丝合缝，刘海不会和屏幕边缘裂开。
            let half=if dock.is_side() {crate::side_dock::SIDE_HALF_EXTENT}
                else {crate::display_layout::horizontal_half_extent(horizontal_width)};
            let (back,forth)=crate::display_layout::shift_slack(bounds,scale,half,offset,dock);
            let (dx,dy)=(shift.0.clamp(-back,forth),shift.1.clamp(-back,forth));
            let (x,y)=if dock.is_side() {(x,y+dy)} else {(x+dx,y)};
            let (canvas_w,canvas_h)=if dock.is_side() {(crate::side_dock::SIDE_W,crate::side_dock::SIDE_H)}
                else {(horizontal_width+crate::app::CANVAS_W-crate::app::WIN_W,crate::app::WIN_H)};
            let (w,h)=((canvas_w*scale).round() as i32,(canvas_h*scale).round() as i32);
            let full=RECT{left:0,top:0,right:w,bottom:h};
            let region=self.crop.applied.unwrap_or(full);
            let (x,y,w,h)=(x+region.left,y+region.top,region.right-region.left,region.bottom-region.top);
            let mut current=RECT::default();
            if GetWindowRect(self.hwnd,&mut current).is_err(){return false;}
            let moved=(current.left,current.top)!=(x,y);
            let old_size=(current.right-current.left,current.bottom-current.top);
            let resized=old_size!=(w,h);
            if moved||resized {
                let flags=SWP_NOACTIVATE|SWP_NOZORDER|if resized{SET_WINDOW_POS_FLAGS(0)}else{SWP_NOSIZE};
                if SetWindowPos(self.hwnd,None,x,y,w,h,flags).is_err(){return false;}
                if !dock.is_side() && offset == 0.0 && self.crop.applied.is_none()
                    && w > old_size.0 && h == old_size.1 && current.top == y
                    && ((current.left - x) - (w - old_size.0) / 2).abs() <= 1 {
                    if let Layering::Active(layered) = &mut self.layering {
                        layered.keep_frame_position_after_widening(x - current.left);
                    }
                }
            }
            self.crop.canvas_changed|=self.crop.full!=Some(full) && self.crop.applied.is_some();
            self.crop.full=Some(full);
            self.crop.wide_placement=(!dock.is_side() && horizontal_width>crate::app::WIN_W)
                .then_some((horizontal_width,scale));
            self.crop.drag_wake.lock().unwrap_or_else(|e|e.into_inner()).canvas=self.canvas_rect();
            true
        }
    }

    pub fn paint_offset(&self)->Option<[i32;2]> {
        self.crop.managed_surface.then(||self.crop.applied.map_or([0,0],|r|[r.left,r.top]))
    }

    fn canvas_rect(&self)->Option<RECT>{
        let mut r=RECT::default();
        unsafe {GetWindowRect(self.hwnd,&mut r).ok()?;}
        if let Some(full)=self.crop.full {
            let [x,y]=self.paint_offset().unwrap_or_default();
            r.left-=x;
            r.top-=y;
            r.right=r.left+full.right;
            r.bottom=r.top+full.bottom;
        }
        Some(r)
    }

    pub fn watch_collapsed_pill_edge(&mut self,hit:Option<eframe::egui::Rect>,ppp:f32,dock:crate::model::Dock) {
        // 空隙仅唤醒悬停，原生裁剪区域仍按可见轮廓处理点击。
        self.crop.drag_wake.lock().unwrap_or_else(|e|e.into_inner()).pill_edge=
            hit.and_then(|hit|collapsed_pill_edge(hit,ppp,dock));
    }

    pub fn request_crop(&mut self, bounds:Option<eframe::egui::Rect>, transitioning:bool) {
        self.crop.requested=bounds.filter(|_|self.crop.supported).map(|bounds|(bounds,transitioning));
    }

    pub fn supports_compact_bounds(&self)->bool {self.crop.supported && self.crop.full.is_some()}

    pub fn begin_crop_frame(&mut self, front_ready:bool) {
        self.crop.front_ready=front_ready;
        if front_ready && self.crop.bridge_visible {
            // 新尺寸的主窗口已完成上一帧的交换，旧画面可以撤下。
            if let Some(layer)=&mut self.crop.bridge {layer.detach(self.hwnd);}
            self.crop.bridge_visible=false;
        }
    }

    fn cover_previous_frame(&mut self, current:RECT)->bool {
        use crate::crop_front_buffer::{mask_outside_rect, read_front};
        let (w,h)=(current.right-current.left,current.bottom-current.top);
        if !self.crop.front_ready || self.crop.bridge_visible || !self.crop.gl.as_ref()
            .is_some_and(|gl|read_front(gl,w,h,&mut self.crop.front_pixels))
        {
            return false;
        }
        let (l,t,r,b)=self.region;
        if !mask_outside_rect(&mut self.crop.front_pixels,w,h,RECT{left:l,top:t,right:r,bottom:b}) {
            return false;
        }
        if self.crop.bridge.is_none() {
            if let Ok(layered) = crate::layered::Layered::new() {
                self.apply_capture_to_new_window(layered.display(), "岛仔裁剪过渡窗口");
                self.crop.bridge = Some(layered);
            }
        }
        let targets = self.capture_targets();
        let hwnd = self.hwnd;
        let Some(layer)=&mut self.crop.bridge else{return false;};
        let pixels = &self.crop.front_pixels;
        let shown = matches!(self.capture_exclusion.render(targets, None, || {
            layer.cover(hwnd, [w as usize, h as usize], pixels)
        }), Ok(true));
        let ready=shown && unsafe{windows::Win32::Graphics::Dwm::DwmFlush().is_ok()};
        if !ready {layer.detach(self.hwnd);}
        self.crop.bridge_visible=ready;
        ready
    }

    fn resize_crop(&mut self, canvas:RECT, current:RECT, target:RECT, frame:RECT)->bool {
        // 上一帧的画面单独留在原屏幕位置，本轮移动的 HWND 只承载新画面与输入。
        if !self.cover_previous_frame(current) {return false;}
        // 改外框之前保留旧画面和当前保守剪区。外框切换后下一帧再安装精确的剪区。
        let current_region=RECT{
            left:self.region.0.saturating_add(current.left),
            top:self.region.1.saturating_add(current.top),
            right:self.region.2.saturating_add(current.left),
            bottom:self.region.3.saturating_add(current.top),
        };
        let cover=RECT{
            left:current_region.left.min(frame.left)-current.left.max(target.left),
            top:current_region.top.min(frame.top)-current.top.max(target.top),
            right:current_region.right.max(frame.right)-current.left.min(target.left),
            bottom:current_region.bottom.max(frame.bottom)-current.top.min(target.top),
        };
        unsafe {
            let region=CreateRectRgn(cover.left,cover.top,cover.right,cover.bottom);
            if SetWindowRgn(self.hwnd,region,false)==0 {
                let _=DeleteObject(region);
                return false;
            }
            self.region=(cover.left,cover.top,cover.right,cover.bottom);
            self.glass=None;
            RESIZING_CROP.set(Some((self.hwnd,false)));
            let result=SetWindowPos(self.hwnd,None,canvas.left+target.left,canvas.top+target.top,
                target.right-target.left,target.bottom-target.top,SWP_NOACTIVATE|SWP_NOZORDER|SWP_FRAMECHANGED);
            let preserved=RESIZING_CROP.replace(None).is_some_and(|(_,preserved)|preserved);
            if result.is_err() {return false;}
            self.crop.applied=(Some(target)!=self.crop.full).then_some(target);
            self.crop.managed_surface=true;
            if !preserved {
                // 合成器未接受同屏幕坐标保留，后续沿用原固定外框。
                self.crop.supported=false;
                self.crop.requested=None;
            }
            true
        }
    }

    fn restore_full_canvas(&mut self, canvas:RECT)->bool {
        if self.crop.bridge_visible {
            if let Some(layer)=&mut self.crop.bridge {layer.detach(self.hwnd);}
            self.crop.bridge_visible=false;
        }
        // 前缓冲不可用时直接恢复外框，旧剪区和输入坐标在本帧重算。
        if unsafe {SetWindowPos(self.hwnd,None,canvas.left,canvas.top,canvas.right-canvas.left,canvas.bottom-canvas.top,
            SWP_NOACTIVATE|SWP_NOZORDER|SWP_NOCOPYBITS)}.is_err() {return false;}
        self.crop.applied=None;
        self.crop.canvas_changed=false;
        self.region=(i32::MIN,0,0,0);
        self.glass=None;
        true
    }

    /// 系统背景模糊是否正在生效（毛玻璃皮肤据此选择着色深浅）。
    pub fn blur_active(&self)->bool {self.blur}
    pub fn opaque_surface(&self)->bool {self.opaque_surface}

    #[cfg(feature = "sdk-material")]
    pub fn material_window(&self) -> Option<HWND> {
        eframe::egui_wgpu::winit::direct_composition_active(self.hwnd.0 as isize).then_some(self.hwnd)
    }

    /// 是否需要每帧读回画面交给分层窗口（兼容后端，且分层窗口仍可用）。
    pub fn wants_frames(&self)->bool {self.opaque_surface && !matches!(self.layering,Layering::Unavailable)}

    pub fn waiting_for_first_layered_frame(&self)->bool {matches!(self.layering,Layering::Starting(_))}

    /// 交一帧读回的画面（RGBA，已预乘）。返回画面是否变化。
    /// 首帧看读回的画面有没有真正的透明像素：有就原样交给分层窗口；没有（部分 AMD 显卡的 OpenGL）
    /// 就按岛体轮廓补算边缘 alpha——普通模式也不再退回逐像素「有或没有」的轮廓裁剪。
    pub fn present_frame(&mut self,size:[usize;2],rgba:&[u8])->bool {
        if !self.wants_frames(){return false;}
        if matches!(self.layering,Layering::Starting(_) | Layering::Pending) {
            let real=crate::layered::has_real_alpha(size,rgba) && outline_test()!=Some(true);
            // 还没收到过岛体轮廓时无从补 alpha，先保持轮廓裁剪，等下一帧。
            if !real && self.outline.polygons.is_empty() {return false;}
            if real && !rgba.chunks_exact(4).any(|pixel|pixel[3]!=0) {return false;}
            match crate::layered::Layered::new() {
                Ok(mut layered)=>{
                    let target = capture::Target { hwnd: layered.display(), name: "岛仔分层显示窗口" };
                    self.apply_capture_to_new_window(target.hwnd, target.name);
                    let targets = self.capture_targets();
                    let hwnd = self.hwnd;
                    let frame=if real {rgba} else {self.outline.apply(size,rgba)};
                    match self.capture_exclusion.render(targets, Some(target), || layered.present(hwnd, size, frame)) {
                        Ok(true)=>{
                            self.outline.active=!real;
                            glass_diagnostic(if real {"渲染兼容：已切换为分层窗口逐像素透明（边缘抗锯齿）"}
                                else {"渲染兼容：读回的画面没有透明通道，按岛体轮廓补算边缘透明度（分层窗口，边缘抗锯齿）"});
                            self.layering=Layering::Active(layered);
                            return true;
                        }
                        Ok(false)=>return false,
                        Err(error)=>{
                            layered.detach(self.hwnd);
                            glass_diagnostic(&format!("渲染兼容：首帧无法交给分层窗口，回到轮廓裁剪：{error:?}"));
                            self.layering=Layering::Unavailable;
                            return false;
                        }
                    }
                }
                Err(error)=>{
                    glass_diagnostic(&format!("渲染兼容：分层显示窗口创建失败：{error:?}"));
                    self.layering=Layering::Unavailable;return false;
                }
            }
        }
        let targets = self.capture_targets();
        let hwnd = self.hwnd;
        let Layering::Active(layered)=&mut self.layering else {return false};
        let frame=if self.outline.active {self.outline.apply(size,rgba)} else {rgba};
        match self.capture_exclusion.render(targets, None, || layered.present(hwnd, size, frame)) {
            // 切到分层窗口后，下一次 update 换回矩形区域（岛仔窗口本身已几乎不可见，只负责命中）。
            Ok(changed)=>changed,
            Err(error)=>{
                glass_diagnostic(&format!("渲染兼容：分层窗口不可用，回到轮廓裁剪：{error:?}"));
                layered.detach(self.hwnd);
                self.layering=Layering::Unavailable;
                self.glass=None;self.region=(i32::MIN,0,0,0);
                false
            }
        }
    }

    pub fn logical_origin(&self,scale:f32)->Option<eframe::egui::Pos2>{
        let r=self.canvas_rect()?;
        Some(eframe::egui::pos2(r.left as f32/scale,r.top as f32/scale))
    }

    pub fn take_display_change(&self)->bool {DISPLAY_CHANGED.swap(false,Ordering::Relaxed)}

    pub fn take_foreground_change(&mut self)->bool {
        let changed=FG_CHANGED.swap(false,Ordering::Relaxed);
        if changed {self.browser_state.1=PollClock::default();}
        changed
    }

    pub fn browser_at_top(&mut self,settings:&crate::model::Settings)->Option<bool> {
        if self.browser_state.1.ready(Duration::from_millis(100)) {
            let bounds=self.canvas_rect().map(|mut rect|{
                if let Some((width,scale))=self.crop.wide_placement {
                    // 浏览器避让以原有面板宽度为判断区间，保持实际屏幕中心和 DPI。
                    rect.left+=((width-crate::app::WIN_W)*scale/2.0).round() as i32;
                    rect.right=rect.left+(crate::app::CANVAS_W*scale).round() as i32;
                }
                rect
            });
            self.browser_state=(bounds.and_then(|r|crate::system::browser_at_top(r,settings)),PollClock::now());
        }
        self.browser_state.0
    }

    /// 区域参数均为画布内的逻辑坐标 (x, y, 宽, 高)。
    pub fn update(
        &mut self,
        region: (f32, f32, f32, f32),
        hit_rect: (f32, f32, f32, f32),
        notice_hit: Option<(f32,f32,f32,f32)>,
        file_drop_zone: Option<(f32,f32,f32,f32)>,
        glass: Option<&[crate::app::GlassShape]>,
        blur: bool,
        ppp: f32,
        editing: bool,
        expanded: bool,
        resident_clip: Option<&[crate::app::GlassShape]>,
    ) -> Option<Duration> {
        let px = |v: f32| (v * ppp).round() as i32;
        let r = (px(region.0), px(region.1), px(region.0 + region.2), px(region.1 + region.3));
        // 仅在拖动进入感应区后扩大 OLE 接收区域，避免普通点击改变透明窗口裁剪。
        let button_down=unsafe{GetAsyncKeyState(VK_LBUTTON.0 as i32)<0 || GetAsyncKeyState(VK_RBUTTON.0 as i32)<0};
        let zone=file_drop_zone.map(|(x,y,w,h)|(px(x),px(y),px(x+w),px(y+h)));
        let canvas=self.canvas_rect();
        let mut cursor=POINT::default();
        let cursor=canvas.filter(|_|unsafe{GetCursorPos(&mut cursor).is_ok()}).map(|r|(cursor.x-r.left,cursor.y-r.top));
        let threshold=unsafe{((GetSystemMetrics(SM_CXDRAG)/2).max(1),(GetSystemMetrics(SM_CYDRAG)/2).max(1))};
        let capture=self.drag_capture.update(button_down,cursor,zone,threshold);
        let r=file_drag_region(r,zone,capture);
        let mut wait=None;
        let mut resized=false;
        if let Some((canvas,full))=canvas.zip(self.crop.full) {
            let current=self.crop.applied.unwrap_or(full);
            let frame=RECT{left:r.0,top:r.1,right:r.2,bottom:r.3};
            // 隐藏时照常清空窗口区域，避免用 1×1 窗口触发显卡重配。
            if frame.left<frame.right && frame.top<frame.bottom {
                let target=if let Some((tight,transitioning))=self.crop.requested.filter(|_|!self.crop.canvas_changed) {
                    let to_px=|v:f32|v*ppp;
                    let tight=RECT{
                        left:(to_px(tight.left()).floor() as i32).max(frame.left).clamp(0,full.right),
                        top:(to_px(tight.top()).floor() as i32).max(frame.top).clamp(0,full.bottom),
                        right:(to_px(tight.right()).ceil() as i32).min(frame.right).clamp(0,full.right),
                        bottom:(to_px(tight.bottom()).ceil() as i32).min(frame.bottom).clamp(0,full.bottom),
                    };
                    let (target,delay)=self.crop.plan.target(current,if capture {full} else {tight},full,transitioning||capture,Instant::now());
                    wait=delay;
                    target
                } else {
                    self.crop.plan=WindowBounds::default();
                    full
                };
                if target!=current {
                    resized=self.crop.supported && self.resize_crop(canvas,current,target,frame);
                    if !resized {
                        self.crop.supported=false;
                        self.crop.requested=None;
                    }
                    // 正常切换在下一帧结束交接，失败时本帧直接恢复固定外框。
                    if resized || self.crop.applied.is_some() && self.restore_full_canvas(canvas) {
                        wait=Some(Duration::ZERO);
                    }
                }
                if target==current || resized {self.crop.canvas_changed=false;}
            }
        }
        let [x,y]=self.paint_offset().unwrap_or_default();
        let local=|(l,t,r,b):(i32,i32,i32,i32)|(l-x,t-y,r-x,b-y);
        // 读回长期不可用时沿用轮廓裁剪，避免岛体一直隐藏。
        let hold_startup=if let Layering::Starting(since)=&mut self.layering {
            let now=Instant::now();
            if now.duration_since(*since.get_or_insert(now))<Duration::from_secs(1) {true}
            else {self.layering=Layering::Pending;false}
        } else {false};
        let r=if hold_startup {(0,0,0,0)} else {local(r)};
        let h = (
            px(hit_rect.0),
            px(hit_rect.1),
            px(hit_rect.0 + hit_rect.2),
            px(hit_rect.1 + hit_rect.3),
        );
        let mut hits=vec![local(h)];
        // 仍有画面但没有点击目标时，让系统直接把输入分派给下层进程。
        let passive=r.2>r.0 && r.3>r.1 && h.2==h.0 && notice_hit.is_none() && !capture;
        if let Some((x,y,w,h))=notice_hit {hits.push(local((px(x),px(y),px(x+w),px(y+h))));}
        *hit().lock().unwrap_or_else(|e| e.into_inner()) = hits;
        // 收起时主窗口和毛玻璃底板都按可见轮廓向下层进程让出鼠标。
        let resident_key=resident_clip.filter(|_|!capture && !hold_startup && r.0<r.2 && r.1<r.3)
            .map(|shapes|glass_key(shapes,ppp));
        // 分层窗口显示的是带真实透明通道的画面时，系统模糊同样能透出来（模糊底板紧贴在岛仔窗口下方，
        // 分层显示窗口在上方）；只有轮廓裁剪或按轮廓补 alpha（内部不透明）时才关掉模糊。
        let layered_alpha=matches!(self.layering,Layering::Active(_)) && !self.outline.active;
        match glass.filter(|g|blur && !passive && !g.is_empty() && !self.blur_unavailable && self.crop.applied.is_none() && (!self.opaque_surface || layered_alpha) && compositor_blur_allowed()) {
            Some(shapes) => {
                if self.backdrop.is_none() {
                    match crate::composition::Backdrop::new() {
                        Ok(backdrop) => {
                            self.apply_capture_to_new_window(backdrop.hwnd(), "岛仔毛玻璃底板");
                            glass_diagnostic("合成引擎模糊底板已创建");
                            self.backdrop = Some(backdrop);
                        }
                        Err(error) => {
                            glass_diagnostic(&format!("合成引擎模糊层创建失败：{error:?}"));
                            self.blur_unavailable = true;
                        }
                    }
                }
                let targets = self.capture_targets();
                let hwnd = self.hwnd;
                if let Some(backdrop) = &mut self.backdrop {
                    let pieces = crate::app::blur_pieces(shapes, ppp);
                    match self.capture_exclusion.render(targets, None, || backdrop.show(hwnd, &pieces, resident_key.as_deref())) {
                        Ok(()) => self.blur = true,
                        Err(error) => {
                            glass_diagnostic(&format!("模糊层更新失败：{error:?}"));
                            backdrop.hide();
                            self.blur = false;
                            self.blur_unavailable = true;
                        }
                    }
                }
            }
            None => {
                if let Some(backdrop) = &mut self.backdrop { backdrop.hide(); }
                self.blur = false;
            }
        }
        // 记下岛体轮廓：读回画面没有透明通道时，分层窗口按它补算边缘 alpha。
        if self.opaque_surface {
            if let Some(shapes)=glass.filter(|g|!g.is_empty()) {self.outline.set_shapes(shapes,ppp);}
        }
        // 不可靠的透明合成路径按真实轮廓裁剪，避免圆角外侧和透明画布留下黑底。
        let outline_clip=self.opaque_surface && !hold_startup && !matches!(self.layering,Layering::Active(_));
        // 可靠的透明合成直接绘制软边，分层显示和不透明兜底仍使用原有的输入轮廓。
        let feather=resident_key.is_some() && !self.opaque_surface;
        match resident_key.or_else(||glass.filter(|_|outline_clip).map(|shapes|glass_key(shapes,ppp))) {
            Some(key) => {
                if !resized && self.glass.as_ref()!=Some(&key) && apply_outline_with_feather(self.hwnd,&key,true,feather,[x,y]) {
                    self.glass=Some(key);
                    // 保留可见外框，供紧凑窗口切换时遮罩上一帧。
                    self.region=r;
                }
            }
            None if !resized && (self.region != r || self.glass.is_some()) => {
                unsafe {
                    let rgn = CreateRectRgn(r.0, r.1, r.2, r.3);
                    if SetWindowRgn(self.hwnd, rgn, true) != 0 {
                        self.region = r;
                        self.glass=None;
                    } else {
                        let _ = DeleteObject(rgn);
                    }
                }
            }
            None => {}
        }
        let raise=expanded && !self.expanded;
        unsafe { apply_style(self.hwnd, editing, raise) };
        if let Layering::Active(layered)=&mut self.layering {layered.sync(self.hwnd,raise);}
        unsafe {
            let ex=GetWindowLongPtrW(self.hwnd,GWL_EXSTYLE);
            let transparent=WS_EX_TRANSPARENT.0 as isize;
            let layered=WS_EX_LAYERED.0 as isize;
            if passive && (ex & (transparent|layered)) != (transparent|layered) {
                SetWindowLongPtrW(self.hwnd,GWL_EXSTYLE,ex|transparent|layered);
                if !self.crop.bridge_visible && !matches!(self.layering,Layering::Active(_)) {
                    let _=SetLayeredWindowAttributes(self.hwnd,windows::Win32::Foundation::COLORREF(0),255,LWA_ALPHA);
                }
            } else if !passive && ex & transparent != 0 {
                let remove=if self.crop.bridge_visible || matches!(self.layering,Layering::Active(_)){transparent}else{transparent|layered};
                SetWindowLongPtrW(self.hwnd,GWL_EXSTYLE,ex & !remove);
            }
        }
        self.expanded = expanded;
        wait
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.set_page_scroll_area(None,1.0);
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(subclass), SUBCLASS_ID);
            if !self.hook.is_invalid() {
                let _ = UnhookWinEvent(self.hook);
                FG_CONTEXT.set(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static NATIVE_STATE_LOCK:Mutex<()>=Mutex::new(());

    #[test]
    fn collapsed_pill_wakes_at_the_docked_edge_outside_the_native_outline() {
        use crate::model::Dock;
        use eframe::egui::{pos2,Rect};
        use windows::Win32::Graphics::Gdi::PtInRegion;
        let canvas=RECT{left:-900,top:-200,right:400,bottom:400};
        for ppp in [1.0,1.25,1.5,2.0] {
            let px=|v:f32|(v*ppp).round() as i32;
            for (dock,hit,body,edge,past_edge) in [
                (Dock::Top,Rect::from_min_max(pos2(200.0,0.0),pos2(400.0,40.0)),
                    Rect::from_min_max(pos2(200.0,6.0),pos2(400.0,36.0)),0,-1),
                (Dock::Bottom,Rect::from_min_max(pos2(200.0,160.0),pos2(400.0,200.0)),
                    Rect::from_min_max(pos2(200.0,164.0),pos2(400.0,194.0)),px(200.0)-1,px(200.0)),
            ] {
                // 裁紧后的 HWND 可以挪到胶囊边框内，唤醒坐标仍使用原画布起点。
                let wake=PointerWake{canvas:Some(canvas),pill_edge:collapsed_pill_edge(hit,ppp,dock)};
                let point=POINT{x:canvas.left+px(300.0),y:canvas.top+edge};
                assert!(wake.pill_edge_contains(point),"{dock:?} / {ppp}：屏幕边缘应唤醒");
                assert!(!wake.pill_edge_contains(POINT{x:canvas.left+px(199.0),..point}));
                assert!(!wake.pill_edge_contains(POINT{y:canvas.top+past_edge,..point}));
                let disabled=PointerWake{pill_edge:None,..wake};
                assert!(!disabled.pill_edge_contains(point));
                let key=glass_key(&[crate::app::GlassShape::Round(body,15.0)],ppp);
                unsafe {
                    let native_outline=glass_region(&key).unwrap();
                    let inside=PtInRegion(native_outline,px(300.0),edge).as_bool();
                    let _=DeleteObject(native_outline);
                    assert!(!inside,"唤醒区域没有扩大原生点击范围");
                }
            }
        }
    }

    #[test]
    fn page_wheel_forwards_only_until_click_or_departure(){unsafe{
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW,windows::core::w!("STATIC"),None,WS_POPUP,0,0,1,1,None,None,None,None).unwrap();
        let target=Some((hwnd,RECT{left:-101,top:50,right:-89,bottom:80}));
        let mut window=RECT::default();
        GetWindowRect(hwnd,&mut window).unwrap();
        let panel=(-110-window.left,40-window.top,-80-window.left,58-window.top);
        let saved=std::mem::replace(&mut *hit().lock().unwrap(),vec![panel]);
        let event=MSLLHOOKSTRUCT{pt:POINT{x:-95,y:65},mouseData:(-30i16 as u16 as u32)<<16,..Default::default()};
        for (cancel,modified) in [(WM_LBUTTONDOWN,false),(WM_MOUSEHWHEEL,false),(WM_MOUSEMOVE,false),(WM_MOUSEWHEEL,true)] {
            PAGE_WHEEL_TARGET.set(target);
            let overlap=MSLLHOOKSTRUCT{pt:POINT{x:-95,y:55},..event};
            assert!(!route_page_wheel(WM_MOUSEWHEEL,&overlap,false),"移入重叠处时滚轮由岛内原生控件接收");
            assert!(route_page_wheel(WM_MOUSEWHEEL,&event,false));
            let mut message=MSG::default();
            assert!(PeekMessageW(&mut message,hwnd,WM_MOUSEWHEEL,WM_MOUSEWHEEL,PM_REMOVE).as_bool());
            assert_eq!((message.wParam.0>>16) as i16,-30);
            let point=if cancel==WM_MOUSEMOVE {POINT{x:-88,y:65}}else{event.pt};
            assert!(!route_page_wheel(cancel,&MSLLHOOKSTRUCT{pt:point,..event},modified));
            assert!(!route_page_wheel(WM_MOUSEWHEEL,&event,false));
            assert!(!PeekMessageW(&mut message,hwnd,WM_MOUSEWHEEL,WM_MOUSEWHEEL,PM_REMOVE).as_bool());
        }
        *hit().lock().unwrap()=saved;
        DestroyWindow(hwnd).unwrap();
    }}

    #[test]
    fn opaque_surface_clips_black_background_even_without_system_blur(){
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        use windows::Win32::Graphics::Gdi::{GetWindowRgn,PtInRegion};
        for (backend,vendor,renderer) in [
            (Some(eframe::wgpu::Backend::Dx12),"",""),
            (Some(eframe::wgpu::Backend::Vulkan),"",""),
            (None,"ATI Technologies Inc.","AMD Radeon RX 6750 GRE 12GB"),
            (None,"Advanced Micro Devices, Inc.","Radeon RX 7900 XT"),
        ] {for blur_unavailable in [true,false] {for ppp in [1.0,1.25,1.5,2.0] {unsafe {
            let px=|v:f32|(v*ppp).round() as i32;
            let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP,-10000,-10000,px(400.0),px(200.0),None,None,None,None).unwrap();
            let saved=hit().lock().unwrap_or_else(|e|e.into_inner()).clone();
            {
                let mut native=Window{hwnd,capture_exclusion:Default::default(),crop:Crop::default(),layering:Layering::Pending,region:(0,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),glass:None,backdrop:None,blur:false,blur_unavailable,opaque_surface:surface_needs_outline_clip(backend,vendor,renderer),outline:ShapeAlpha::default()};
                let shapes=[crate::app::GlassShape::Round(eframe::egui::Rect::from_min_max(eframe::egui::pos2(80.0,60.0),eframe::egui::pos2(320.0,160.0)),25.0)];
                native.update((0.0,0.0,400.0,200.0),(80.0,60.0,240.0,100.0),None,None,Some(&shapes),true,ppp,false,false,None);
                let region=CreateRectRgn(0,0,0,0);
                GetWindowRgn(hwnd,region);
                assert!(PtInRegion(region,px(200.0),px(100.0)).as_bool());
                assert!(!PtInRegion(region,px(10.0),px(10.0)).as_bool(),"兼容后端不能保留画布上的黑色背景：{backend:?} / {vendor} / {renderer}");
                assert!(!PtInRegion(region,px(81.0),px(61.0)).as_bool(),"圆角外侧必须透出桌面");
                assert!(!native.blur);
                assert!(native.backdrop.is_none(),"轮廓兼容模式不能再启用独立模糊底板");
                native.update((0.0,0.0,0.0,0.0),(0.0,0.0,0.0,0.0),None,None,None,false,ppp,false,false,None);
                GetWindowRgn(hwnd,region);
                assert!(!PtInRegion(region,px(200.0),px(100.0)).as_bool(),"隐藏时不能留下上一帧轮廓");
                let _=DeleteObject(region);
            }
            *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
            let _=DestroyWindow(hwnd);
        }}}}
    }

    /// 部分 AMD 显卡的 OpenGL：读回的画面没有透明通道。过去这里直接退回轮廓裁剪（圆角、肩部全是锯齿），
    /// 现在要切到分层窗口，并按岛体轮廓补出带过渡的边缘 alpha。
    #[test]
    fn opaque_readback_uses_outline_alpha_instead_of_jagged_clip(){
        use crate::app::GlassShape;
        use eframe::egui::{pos2,Rect};
        use windows::Win32::Graphics::Gdi::{CreateRectRgn,DeleteObject,GetWindowRgn,PtInRegion};
        unsafe {
            let (w,h)=(400usize,200usize);
            let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE,windows::core::w!("BUTTON"),windows::core::w!(""),WS_POPUP|WS_VISIBLE,-10000,-10000,w as i32,h as i32,None,None,None,None).unwrap();
            let saved=hit().lock().unwrap_or_else(|e|e.into_inner()).clone();
            {
                let mut native=Window{hwnd,capture_exclusion:Default::default(),crop:Crop::default(),layering:Layering::Pending,region:(0,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),glass:None,backdrop:None,blur:false,blur_unavailable:false,opaque_surface:true,outline:ShapeAlpha::default()};
                let island=crate::shape::outline(Rect::from_min_max(pos2(80.0,0.0),pos2(320.0,60.0)),12.0,10.0,20.0);
                let shapes=[GlassShape::Outline(island)];
                // 读回的画面：岛体黑色、画布也是黑色，alpha 全部 255——和 AMD OpenGL 读回的一样。
                let opaque=[0u8,0,0,255].repeat(w*h);
                assert!(!native.present_frame([w,h],&opaque),"还没收到轮廓时先保持轮廓裁剪");
                assert!(matches!(native.layering,Layering::Pending));
                native.update((0.0,0.0,w as f32,h as f32),(80.0,0.0,240.0,60.0),None,None,Some(&shapes),true,1.0,false,false,None);
                assert!(native.wants_frames());
                assert!(native.present_frame([w,h],&opaque),"读不到透明通道也要切到分层窗口");
                assert!(matches!(native.layering,Layering::Active(_)) && native.outline.active);
                let frame=&native.outline.frame;
                let alpha=|x:usize,y:usize|frame[(y*w+x)*4+3];
                assert_eq!(alpha(200,30),255,"岛体内部不透明");
                assert_eq!(alpha(10,150),0,"画布空白处透明，不能留黑底");
                let soft=(0..h).flat_map(|y|(0..w).map(move|x|(x,y))).filter(|&(x,y)|(1..255).contains(&alpha(x,y))).count();
                assert!(soft>=80,"圆角和肩部要有抗锯齿过渡像素，实际 {soft}");
                assert!(frame.chunks_exact(4).all(|p|p[0]<=p[3]&&p[1]<=p[3]&&p[2]<=p[3]),"必须是合法的预乘颜色");
                // 切到分层窗口后不再用轮廓裁剪岛仔窗口本身。
                native.update((0.0,0.0,w as f32,h as f32),(80.0,0.0,240.0,60.0),None,None,Some(&shapes),true,1.0,false,false,None);
                let region=CreateRectRgn(0,0,0,0);
                if GetWindowRgn(hwnd,region).0!=0 {assert!(PtInRegion(region,10,150).as_bool(),"分层显示后窗口区域应回到矩形");}
                let _=DeleteObject(region);
                assert!(!native.present_frame([w,h],&opaque),"画面没变时不重复提交");
            }
            *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
            let _=DestroyWindow(hwnd);
        }
    }

    #[test]
    fn frame_control_dx12_startup_hands_off_only_a_valid_visible_frame() {
        use crate::app::GlassShape;
        use eframe::egui::{pos2,Rect};
        use windows::Win32::Graphics::Gdi::{GetWindowRgnBox,NULLREGION};
        unsafe {
            let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP|WS_VISIBLE,-10000,-10000,32,16,None,None,None,None).unwrap();
            let saved=hit().lock().unwrap_or_else(|e|e.into_inner()).clone();
            {
                let mut native=Window{hwnd,capture_exclusion:Default::default(),crop:Crop::default(),layering:Layering::Starting(None),region:(i32::MIN,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),glass:None,backdrop:None,blur:false,blur_unavailable:true,opaque_surface:true,outline:ShapeAlpha::default()};
                let shapes=[GlassShape::Round(Rect::from_min_max(pos2(3.0,1.0),pos2(28.0,14.0)),4.0)];
                let update=|window:&mut Window| window.update((0.0,0.0,32.0,16.0),(3.0,1.0,25.0,13.0),None,None,Some(&shapes),true,1.0,false,false,None);
                let mut frame=[0u8;32*16*4];
                update(&mut native);
                assert_eq!(GetWindowRgnBox(hwnd,&mut RECT::default()),NULLREGION);
                assert!(!native.present_frame([32,16],&frame),"全透明时继续等待");
                frame[(4+4*32)*4..(5+4*32)*4].fill(255);
                assert!(!native.present_frame([16,32],&frame),"定位前读回的旧尺寸不得生效");
                assert!(matches!(native.layering,Layering::Starting(_)));
                update(&mut native);
                assert_eq!(GetWindowRgnBox(hwnd,&mut RECT::default()),NULLREGION);
                native.layering=Layering::Starting(Some(Instant::now()-Duration::from_secs(1)));
                update(&mut native);
                assert!(matches!(native.layering,Layering::Pending) && native.wants_frames());
                assert_ne!(GetWindowRgnBox(hwnd,&mut RECT::default()),NULLREGION,"超时后恢复轮廓显示并继续等待有效读回");
                assert!(!native.present_frame([32,16],&[0u8;32*16*4]),"迟到的空帧不覆盖兜底画面");
                native.layering=Layering::Starting(None);
                update(&mut native);
                assert!(native.present_frame([32,16],&frame));
                let Layering::Active(layered)=&native.layering else {panic!("有效首帧应接管显示")};
                assert!(!IsWindowVisible(layered.display()).as_bool(),"命中区域开放前保持不显示");
                let mut alpha=0;
                GetLayeredWindowAttributes(hwnd,None,Some(&mut alpha),None).unwrap();
                assert_eq!(alpha,1,"开放区域前主交换链已淡化");
                update(&mut native);
                let Layering::Active(layered)=&native.layering else {panic!("分层窗口应保持活动")};
                assert!(IsWindowVisible(layered.display()).as_bool());
                assert_ne!(GetWindowRgnBox(hwnd,&mut RECT::default()),NULLREGION);
            }
            *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
            let _=DestroyWindow(hwnd);
        }
    }

    #[test]
    fn outline_clip_preserves_other_opengl_drivers_and_recognizes_amd_variants(){
        for (vendor,renderer) in [("NVIDIA Corporation","NVIDIA GeForce RTX 4060"),("Intel","Intel(R) UHD Graphics"),("Mesa","llvmpipe (LLVM 20.1.1, 256 bits)"),("Microsoft Corporation","GDI Generic")]{
            assert!(!surface_needs_outline_clip(None,vendor,renderer),"正常驱动不能被错误识别为 AMD：{vendor}");
        }
        for (vendor,renderer) in [("AMD","Radeon RX 6750 GRE"),("ati technologies inc.","Radeon"),("Mesa","AMD Radeon RX 6800 (radeonsi)"),("Mesa","Radeon RX 6750 GRE (radeonsi)")]{
            assert!(surface_needs_outline_clip(None,vendor,renderer),"需要识别 AMD 厂商与渲染器：{vendor} / {renderer}");
        }
    }

    #[test]
    fn glass_region_follows_the_island_outline_instead_of_its_bounding_box(){
        use crate::app::GlassShape;
        use eframe::egui::{pos2,Rect};
        use windows::Win32::Graphics::Gdi::PtInRegion;
        let outline=crate::shape::outline(Rect::from_min_max(pos2(100.0,0.0),pos2(300.0,60.0)),5.0,8.0,20.0);
        let shapes=[GlassShape::Outline(outline),GlassShape::Round(Rect::from_min_max(pos2(320.0,0.0),pos2(360.0,30.0)),10.0)];
        let key=glass_key(&shapes,1.5);
        assert_eq!(key,glass_key(&shapes,1.5),"同一帧轮廓的键必须稳定，避免每帧重设区域");
        assert_ne!(key,glass_key(&shapes,1.25));
        unsafe {
            let rgn=glass_region(&key).expect("轮廓应能生成区域");
            assert!(PtInRegion(rgn,300,45).as_bool(),"岛体中央在区域内");
            assert!(!PtInRegion(rgn,152,88).as_bool(),"底部圆角外侧不能出现模糊");
            assert!(PtInRegion(rgn,510,20).as_bool(),"通知详情等圆角块也在区域内");
            assert!(!PtInRegion(rgn,470,20).as_bool(),"两块之间保持透明");
            let _=DeleteObject(rgn);
        }
    }

    #[test]
    fn resident_region_preserves_antialiasing_without_capturing_transparent_space() {
        use windows::Win32::Graphics::Gdi::{GetWindowRgn,PtInRegion};
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        unsafe {
            let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP,-10000,-10000,1000,200,None,None,None,None).unwrap();
            let mut native=Window{hwnd,capture_exclusion:Default::default(),crop:Crop::default(),layering:Layering::Pending,region:(0,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),glass:None,backdrop:None,blur:false,blur_unavailable:true,opaque_surface:false,outline:ShapeAlpha::default()};
            use eframe::egui::{pos2,vec2,Rect};
            use crate::app::GlassShape;
            for width in [180.0,500.0] {
                let bar=(64.0,7.0,width,30.0);
                let rect=Rect::from_min_size(pos2(bar.0,bar.1),vec2(bar.2,bar.3));
                for shape in [GlassShape::Round(rect,15.0),GlassShape::Notch{rect,flare:5.0,flare_h:8.0,radius:12.5}] {
                    for ppp in [1.0,1.25,1.75] {
                        let shapes=std::slice::from_ref(&shape);
                        let raw=glass_region(&glass_key(shapes,ppp)).unwrap();
                        native.update((32.0,0.0,width+64.0,48.0),bar,None,None,None,false,ppp,false,false,Some(shapes));
                        let rgn=CreateRectRgn(0,0,0,0);
                        let _=GetWindowRgn(hwnd,rgn);
                        for y in [(8.0*ppp).round() as i32,(22.0*ppp).round() as i32,(35.0*ppp).round() as i32] {
                            let center=((bar.0+width/2.0)*ppp).round() as i32;
                            let left=(0..center).find(|&x|PtInRegion(raw,x,y).as_bool()).unwrap();
                            let right=(center..1000).rev().find(|&x|PtInRegion(raw,x,y).as_bool()).unwrap();
                            assert!(PtInRegion(rgn,left-1,y).as_bool() && PtInRegion(rgn,right+1,y).as_bool(),
                                "{width} / {ppp}：窗口不能裁掉轮廓外的抗锯齿像素");
                            let outside_left=((bar.0-8.0)*ppp).round() as i32;
                            let outside_right=((bar.0+width+8.0)*ppp).round() as i32;
                            assert!(!PtInRegion(rgn,outside_left,y).as_bool() && !PtInRegion(rgn,outside_right,y).as_bool(),
                                "{width} / {ppp}：轮廓两端的留白仍应交给下层窗口");
                        }
                        let _=DeleteObject(raw);
                        let _=DeleteObject(rgn);
                    }
                }
            }
            drop(native);
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn drag_capture_ignores_clicks_and_pointer_jitter() {
        let mut capture=DragCapture::default();let zone=Some((110,0,530,120));
        assert!(!capture.update(true,Some((300,60)),zone,(4,4)),"感应区内按下不能直接扩大窗口");
        assert!(!capture.update(true,Some((302,61)),zone,(4,4)),"点击时的小幅抖动不算拖动");
        assert!(!capture.update(false,Some((302,61)),zone,(4,4)));
    }

    #[test]
    fn drag_capture_ignores_repeated_clicks_below_the_island_without_an_observed_release() {
        // 极简模式一秒才画一帧：两次点击之间可能没有一帧看到松开。
        let mut capture=DragCapture::default();let zone=Some((110,0,530,120));
        assert!(!capture.update(true,Some((200,60)),zone,(4,4)));
        assert!(!capture.update(true,Some((380,60)),zone,(4,4)),"在岛下方连续点击不同标签不能被当成拖文件");
        assert!(!capture.update(true,Some((300,30)),zone,(4,4)),"在感应区内拖动标签或选文字也不能扩大窗口");
    }

    #[test]
    fn drag_capture_requires_movement_and_entry_and_resets_after_release() {
        let mut capture=DragCapture::default();let zone=Some((110,0,530,120));
        assert!(!capture.update(true,Some((700,400)),zone,(4,4)));
        assert!(!capture.update(true,Some((650,300)),zone,(4,4)),"远处拖动不能改变岛的区域");
        assert!(capture.update(true,Some((300,60)),zone,(4,4)),"拖到感应区后允许接收文件");
        assert!(!capture.update(true,Some((700,400)),zone,(4,4)),"离开感应区应恢复区域");
        assert!(capture.update(true,Some((300,60)),zone,(4,4)));
        assert!(!capture.update(false,Some((300,60)),zone,(4,4)));
        assert!(!capture.update(true,Some((300,60)),zone,(4,4)),"下一次点击不能沿用上次拖动状态");
        assert!(!capture.update(true,None,zone,(4,4)));
        assert!(!capture.update(true,Some((300,60)),None,(4,4)));
    }

    #[test]
    fn top_bar_mode_restores_z_order_without_raising_over_unrelated_windows(){
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        struct TestWindow(HWND);
        impl Drop for TestWindow {fn drop(&mut self){unsafe{let _=DestroyWindow(self.0);}}}
        unsafe fn above(first:HWND,second:HWND)->bool {
            let mut current=first;
            while let Ok(next)=GetWindow(current,GW_HWNDNEXT){if next==second{return true;}current=next;}
            false
        }
        unsafe {
            let bounds=crate::display_layout::Bounds{left:-10480,top:-10000,right:-8560,bottom:-8920};
            let flags=SWP_NOACTIVATE|SWP_SHOWWINDOW;
            let create=||TestWindow(CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW|WS_EX_TOPMOST,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP|WS_VISIBLE,-10000,-10000,416,140,None,None,None,None).unwrap());
            for layered in [false,true] {
                let island=create();let cover=create();
                let mut native=Window{hwnd:island.0,capture_exclusion:Default::default(),crop:Crop::default(),layering:Layering::Pending,region:(0,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),glass:None,backdrop:None,blur:false,blur_unavailable:false,opaque_surface:false,outline:ShapeAlpha::default()};
                if layered {
                    let mut display=crate::layered::Layered::new().unwrap();
                    display.present(island.0,[416,140],&vec![0;416*140*4]).unwrap();
                    native.layering=Layering::Active(display);
                }
                let display=match &native.layering {Layering::Active(layered)=>layered.display(),_=>island.0};
                let foreground=GetForegroundWindow();
                SetWindowPos(cover.0,HWND_TOPMOST,bounds.left,bounds.top,1920,32,flags).unwrap();
                assert!(top_bar_above(display,bounds,None),"复现顶栏遮挡，兼容渲染={layered}");
                native.keep_above_top_bar(bounds,None);
                assert!(!top_bar_above(display,bounds,None),"收起时也应恢复实际层级，兼容渲染={layered}");
                assert_eq!(GetForegroundWindow(),foreground,"置顶不能抢走输入焦点");
                for (x,y,w,h) in [(-10000,-10000,400,140),(-10480,-10000,1920,300),(-10480,-10000,1920,1080),(-10480,-9600,1920,32),(-12400,-10000,1920,32)] {
                    SetWindowPos(cover.0,HWND_TOPMOST,x,y,w,h,flags).unwrap();
                    assert!(!top_bar_above(display,bounds,None),"普通弹窗、全屏窗口和其他屏幕不触发顶栏适配");
                    native.keep_above_top_bar(bounds,None);
                    assert!(above(cover.0,display),"不应越过无关弹窗争抢置顶");
                }
                SetWindowPos(cover.0,HWND_TOPMOST,bounds.left,bounds.top,1920,32,flags).unwrap();
                let _=ShowWindow(cover.0,SW_HIDE);
                assert!(!top_bar_above(display,bounds,None),"隐藏的顶栏不触发抬高");
            }
        }
    }

    #[test]
    fn collapsed_notice_restores_clicks_when_the_top_bar_covers_it() {
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        struct TestWindow(HWND);
        impl Drop for TestWindow {fn drop(&mut self){unsafe{let _=DestroyWindow(self.0);}}}

        unsafe {
            use eframe::egui::{Pos2,Rect};
            // 屏幕外的可见窗口也参与命中测试，不遮挡实际桌面或移动鼠标。
            let bounds=crate::display_layout::Bounds{left:-12000,top:-10000,right:-10080,bottom:-8920};
            let (left,top)=(bounds.left+900,bounds.top);
            let create=|x,w,h|TestWindow(CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW|WS_EX_TOPMOST,
                windows::core::w!("BUTTON"),windows::core::w!(""),WS_POPUP|WS_VISIBLE,x,top,w,h,None,None,None,None).unwrap());
            let find=|x,y|WindowFromPoint(POINT{x:left+x,y:top+y});
            let flags=SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE;
            let region=(0.0,0.0,300.0,80.0);
            let empty=(0.0,0.0,0.0,0.0);
            let notice=(100.0,12.0,160.0,20.0);
            let target=Rect::from_min_max(Pos2::new(100.0,12.0),Pos2::new(260.0,32.0));
            let saved=hit().lock().unwrap_or_else(|e|e.into_inner()).clone();

            for layered in [false,true] {
                let island=create(left,450,120);
                let cover=create(bounds.left,1920,48);
                SetWindowSubclass(island.0,Some(subclass),SUBCLASS_ID,0).unwrap();
                let mut native=Window{hwnd:island.0,capture_exclusion:Default::default(),crop:Crop::default(),layering:Layering::Pending,region:(0,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),glass:None,backdrop:None,blur:false,blur_unavailable:false,opaque_surface:false,outline:ShapeAlpha::default()};
                if layered {
                    let mut display=crate::layered::Layered::new().unwrap();
                    assert!(display.present(island.0,[450,120],&vec![255;450*120*4]).unwrap());
                    native.layering=Layering::Active(display);
                }

                for compact in [false,true] {
                    let (main_hit,notice_hit)=if compact {(empty,Some(notice))} else {(notice,None)};
                    // 模拟面板收起时，普通通知更新主命中区，紧凑通知使用独立命中区。
                    native.update(region,main_hit,notice_hit,None,None,false,1.5,false,false,None);
                    // 窄顶栏覆盖主窗口左侧，但到不了按 150% 换算后的通知。
                    SetWindowPos(cover.0,HWND_TOPMOST,bounds.left,top,1024,48,SWP_NOACTIVATE).unwrap();
                    native.keep_above_top_bar(bounds,Some((target,1.5)));
                    let short_bar_stayed_above=top_bar_above(island.0,bounds,None);

                    SetWindowPos(cover.0,HWND_TOPMOST,bounds.left,top,1920,48,SWP_NOACTIVATE).unwrap();
                    let obscured=find(255,33);
                    let foreground=GetForegroundWindow();
                    native.keep_above_top_bar(bounds,Some((target,1.5)));
                    let clickable=find(255,33);
                    let outside=find(40,33);
                    let foreground_after=GetForegroundWindow();

                    SetWindowPos(cover.0,HWND_TOPMOST,0,0,0,0,flags).unwrap();
                    let obscured_again=find(255,33);
                    native.keep_above_top_bar(bounds,Some((target,1.5)));
                    let clickable_again=find(255,33);
                    native.update(region,empty,None,None,None,false,1.5,false,false,None);
                    let dismissed=find(255,33);
                    *hit().lock().unwrap_or_else(|e|e.into_inner())=saved.clone();

                    assert!(short_bar_stayed_above,"通知按 150% 换成物理坐标后没有被短顶栏挡住，层级应保持");
                    assert_eq!(obscured,cover.0,"顶栏在前时按钮被挡，兼容渲染={layered}，紧凑通知={compact}");
                    assert_eq!(clickable,island.0,"恢复层级后应命中负责交互的主窗口，兼容渲染={layered}，紧凑通知={compact}");
                    assert_eq!(outside,cover.0,"通知外仍应命中任务栏");
                    assert_eq!(foreground_after,foreground,"恢复通知的交互不能抢走焦点");
                    assert_eq!(obscured_again,cover.0,"任务栏重新置顶后应再次形成遮挡");
                    assert_eq!(clickable_again,island.0,"同一条通知应能恢复二次遮挡");
                    assert_eq!(dismissed,cover.0,"通知关闭后原位置应恢复点穿");
                }
            }
        }
    }

    #[test]
    fn expanding_raises_the_island_without_activating_it(){
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        struct TestWindow(HWND);
        impl Drop for TestWindow {fn drop(&mut self){unsafe{let _=DestroyWindow(self.0);}}}
        unsafe fn above(first:HWND,second:HWND)->bool {
            let mut current=first;
            while let Ok(next)=GetWindow(current,GW_HWNDNEXT){
                if next==second{return true;}
                current=next;
            }
            false
        }
        unsafe {
            // 窗口在屏幕外可见，才能验证实际桌面层级；不激活，也不遮挡用户桌面。
            let create=||TestWindow(CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW|WS_EX_TOPMOST,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP|WS_VISIBLE,-10000,-10000,416,140,None,None,None,None).unwrap());
            let island=create();let cover=create();
            let flags=SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE;
            apply_style(island.0,false,false);
            SetWindowPos(cover.0,HWND_TOPMOST,0,0,0,0,flags).unwrap();
            assert!(above(cover.0,island.0),"先复现其他窗口盖住岛体");
            let foreground=GetForegroundWindow();
            let saved=hit().lock().unwrap_or_else(|e|e.into_inner()).clone();
            let mut native=Window{hwnd:island.0,capture_exclusion:Default::default(),crop:Crop::default(),layering:Layering::Pending,region:(0,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),glass:None,backdrop:None,blur:false,blur_unavailable:false,opaque_surface:false,outline:ShapeAlpha::default()};
            let update=|native:&mut Window,expanded|native.update((0.0,0.0,416.0,140.0),(0.0,0.0,416.0,140.0),None,None,None,false,1.0,false,expanded,None);
            update(&mut native,true);
            assert!(above(island.0,cover.0),"展开时必须重新抬高实际窗口层级，不能只检查置顶标记");
            assert_eq!(GetForegroundWindow(),foreground,"悬停展开不能抢走当前应用的输入焦点");
            SetWindowPos(cover.0,HWND_TOPMOST,0,0,0,0,flags).unwrap();
            update(&mut native,true);
            assert!(above(cover.0,island.0),"展开后不能每帧与其他弹窗争抢层级");
            update(&mut native,false);
            assert!(above(cover.0,island.0),"收起时不抬高层级");
            // 模拟只剩置顶标记、实际顺序落在普通窗口后面的情况。
            SetWindowPos(island.0,HWND_NOTOPMOST,0,0,0,0,flags).unwrap();
            SetWindowPos(cover.0,HWND_NOTOPMOST,0,0,0,0,flags).unwrap();
            SetWindowLongPtrW(island.0,GWL_EXSTYLE,GetWindowLongPtrW(island.0,GWL_EXSTYLE)|WS_EX_TOPMOST.0 as isize);
            assert!(above(cover.0,island.0));
            update(&mut native,true);
            assert!(above(island.0,cover.0),"再次展开也必须恢复实际置顶顺序");
            assert_eq!(GetForegroundWindow(),foreground);
            *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
        }
    }

    #[test]
    fn file_drag_expands_region_only_when_capture_is_active(){
        let island=(230,0,410,42);let zone=(110,0,530,120);
        assert_eq!(file_drag_region(island,Some(zone),false),island,"普通悬停不扩大原生区域");
        assert_eq!(file_drag_region(island,Some(zone),true),zone,"拖入感应区应覆盖岛下方");
        assert_eq!(file_drag_region(island,None,true),island,"关闭置物架或隐藏岛时不接收外围拖放");
        let expanded=(64,0,576,220);
        assert_eq!(file_drag_region(expanded,Some(zone),true),expanded,"展开后的文件区不应被感应区裁掉");
    }

    #[test]
    fn crop_readback_failure_restores_full_canvas_and_hit_coordinates() {
        use windows::Win32::Graphics::Gdi::GetWindowRgnBox;
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        let saved=hit().lock().unwrap_or_else(|e|e.into_inner()).clone();
        unsafe {
            let full=RECT{left:0,top:0,right:600,bottom:300};
            let compact=RECT{left:135,top:9,right:225,bottom:54};
            let canvas=RECT{left:-11000,top:-10000,right:-10400,bottom:-9700};
            let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE,windows::core::w!("STATIC"),None,WS_POPUP,
                canvas.left+compact.left,canvas.top+compact.top,compact.right-compact.left,compact.bottom-compact.top,
                None,None,None,None).unwrap();
            let clip=CreateRectRgn(0,0,90,45);
            assert_ne!(SetWindowRgn(hwnd,clip,false),0);
            let mut native=Window{hwnd,capture_exclusion:Default::default(),
                crop:Crop{full:Some(full),applied:Some(compact),supported:true,managed_surface:true,front_ready:true,..Default::default()},
                layering:Layering::Pending,region:(0,0,90,45),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,
                browser_state:(None,PollClock::now()),drag_capture:Default::default(),glass:None,backdrop:None,
                blur:false,blur_unavailable:false,opaque_surface:false,outline:ShapeAlpha::default()};
            let body=eframe::egui::Rect::from_min_max(eframe::egui::pos2(48.0,12.0),eframe::egui::pos2(248.0,138.0));
            native.request_crop(Some(body),true);
            // 没有可读的 OpenGL 前缓冲，展开时仍须恢复完整画布。
            let bounds=(body.left(),body.top(),body.width(),body.height());
            let repaint=native.update(bounds,bounds,None,None,None,false,1.5,false,true,None);
            let mut actual=RECT::default();
            GetWindowRect(hwnd,&mut actual).unwrap();
            let mut region=RECT::default();
            GetWindowRgnBox(hwnd,&mut region);
            let hit_at_panel=inside_hit(360,190);
            let offset=native.paint_offset();
            let supported=native.supports_compact_bounds();
            drop(native);
            DestroyWindow(hwnd).unwrap();
            *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;

            assert_eq!(actual,canvas);
            assert_eq!(region,RECT{left:72,top:18,right:372,bottom:207});
            assert!(hit_at_panel,"面板右下角应使用完整画布的物理坐标");
            assert_eq!(offset,Some([0,0]));
            assert!(!supported);
            assert_eq!(repaint,Some(Duration::ZERO));
        }
    }

    #[test]
    fn file_capture_below_the_island_still_returns_click_through(){
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        unsafe {
            let saved=std::mem::replace(&mut *hit().lock().unwrap_or_else(|e|e.into_inner()),vec![(230,0,410,42)]);
            let hwnd=CreateWindowExW(WS_EX_NOACTIVATE,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP,0,0,640,270,None,None,None,None).unwrap();
            SetWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID,0).unwrap();
            let region=file_drag_region((230,0,410,42),Some((110,0,530,120)),true);
            let rgn=CreateRectRgn(region.0,region.1,region.2,region.3);assert_ne!(SetWindowRgn(hwnd,rgn,false),0);
            let mut rect=RECT::default();GetWindowRect(hwnd,&mut rect).unwrap();
            let point=|x:i32,y:i32|LPARAM((((rect.top+y) as u32)<<16|((rect.left+x) as u16 as u32)) as isize);
            assert_eq!(SendMessageW(hwnd,WM_NCHITTEST,WPARAM(0),point(500,80)).0,HTTRANSPARENT as isize,"感应区中的普通点击仍须穿透");
            assert_eq!(SendMessageW(hwnd,WM_NCHITTEST,WPARAM(0),point(320,15)).0,HTCLIENT as isize,"岛体本身仍可交互");
            let _=DestroyWindow(hwnd);*hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
        }
    }

    #[test]
    fn passive_strip_yields_native_hits_and_expanded_panel_takes_them_back(){
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        unsafe {
            let create=||CreateWindowExW(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE|WS_EX_TOPMOST,windows::core::w!("BUTTON"),windows::core::w!(""),WS_POPUP|WS_VISIBLE,80,80,80,32,None,None,None,None).unwrap();
            let tab=create();
            let island=create();
            let mut native=Window{hwnd:island,capture_exclusion:Default::default(),crop:Crop::default(),layering:Layering::Pending,region:(0,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),page_wheel_hook:None,browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),glass:None,backdrop:None,blur:false,blur_unavailable:false,opaque_surface:false,outline:ShapeAlpha::default()};
            let saved=hit().lock().unwrap_or_else(|e|e.into_inner()).clone();
            let find=|x,y|std::thread::spawn(move||WindowFromPoint(POINT{x,y}).0 as usize).join().unwrap();
            let region=(0.0,0.0,80.0,32.0);
            native.update(region,(0.0,0.0,0.0,0.0),None,None,None,false,1.0,false,false,None);
            let center=find(120,84);
            let margin=find(130,96);
            native.update(region,region,None,None,None,false,1.0,false,true,None);
            let panel=find(120,84);
            drop(native);
            let _=DestroyWindow(island);
            let _=DestroyWindow(tab);
            *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
            assert_eq!((center,margin),(tab.0 as usize,tab.0 as usize),"细条中心和透明留白都应命中下层窗口");
            assert_eq!(panel,island.0 as usize,"展开后面板应恢复命中");
        }
    }

    #[test]
    fn right_notice_keeps_empty_space_below_the_main_bar_click_through(){
        let _guard=NATIVE_STATE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
        let saved=std::mem::replace(&mut *hit().lock().unwrap_or_else(|e|e.into_inner()),vec![(100,0,400,30),(372,0,760,96)]);
        assert!(inside_hit(600,70));assert!(inside_hit(200,15));assert!(!inside_hit(200,70));
        *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
    }

    #[test]
    fn activation_keeps_focus_messages_but_suppresses_nonclient_paint(){
        #[derive(Default)]
        struct Capture{activation:Vec<(usize,isize)>,paint:u32}
        unsafe extern "system" fn observe(hwnd:HWND,msg:u32,wp:WPARAM,lp:LPARAM,_id:usize,data:usize)->LRESULT{
            let capture=&mut *(data as *mut Capture);
            if msg==WM_NCACTIVATE{capture.activation.push((wp.0,lp.0));}
            if msg==WM_NCPAINT{capture.paint+=1;return LRESULT(0);}
            DefSubclassProc(hwnd,msg,wp,lp)
        }
        unsafe{
            let mut capture=Capture::default();
            let hwnd=CreateWindowExW(WS_EX_NOACTIVATE,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP,0,0,100,50,None,None,None,None).unwrap();
            SetWindowSubclass(hwnd,Some(observe),SUBCLASS_ID+2,&mut capture as *mut Capture as usize).unwrap();
            SetWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID,0).unwrap();
            for active in [1,0]{let result=SendMessageW(hwnd,WM_NCACTIVATE,WPARAM(active),LPARAM(0));if active==0{assert_ne!(result.0,0,"仍须允许切换到其他窗口");}}
            SendMessageW(hwnd,WM_NCPAINT,WPARAM(1),LPARAM(0));
            let _=DestroyWindow(hwnd);
            assert_eq!(capture.activation,[(1,-1),(0,-1)],"仍需通知窗口框架焦点变化，同时阻止系统重画顶边");
            assert_eq!(capture.paint,0,"透明岛体不能交给系统绘制非客户区");
        }
    }

    #[test]
    fn editing_mode_does_not_recalculate_the_native_window_frame(){
        unsafe extern "system" fn count_frame(hwnd:HWND,msg:u32,wp:WPARAM,lp:LPARAM,_id:usize,data:usize)->LRESULT {
            if msg==WM_NCCALCSIZE {*(data as *mut u32)+=1;}
            DefSubclassProc(hwnd,msg,wp,lp)
        }
        struct TestWindow(HWND);
        impl Drop for TestWindow {fn drop(&mut self){unsafe{let _=DestroyWindow(self.0);}}}
        unsafe {
            let mut count=0u32;
            // 与实际岛体一样，从已置顶窗口验证输入状态切换。
            let window=TestWindow(CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOPMOST,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP,0,0,100,50,None,None,None,None).unwrap());
            SetWindowSubclass(window.0,Some(count_frame),SUBCLASS_ID+1,&mut count as *mut u32 as usize).unwrap();
            apply_style(window.0,false,false);count=0;
            for editing in [true,false,true,false] {apply_style(window.0,editing,false);}
            apply_style(window.0,false,true);
            assert_eq!(count,0,"添加和退出输入时不应触发原生边框重算，避免顶端白条闪现");
            assert_ne!(GetWindowLongPtrW(window.0,GWL_EXSTYLE)&WS_EX_TOPMOST.0 as isize,0,"仍应保持岛体置顶");
        }
    }

    #[test]
    fn mouse_click_keeps_overlay_inactive_except_when_editing() {
        struct TestWindow(HWND);
        impl Drop for TestWindow {
            fn drop(&mut self) { unsafe { let _ = DestroyWindow(self.0); } }
        }
        unsafe {
            let window = TestWindow(CreateWindowExW(
                WS_EX_NOACTIVATE, windows::core::w!("STATIC"), windows::core::w!(""),
                WS_POPUP, 0, 0, 100, 50, None, None, None, None,
            ).unwrap());
            SetWindowSubclass(window.0, Some(subclass), SUBCLASS_ID, 0).unwrap();
            let click = LPARAM(((WM_LBUTTONDOWN << 16) | HTCLIENT) as isize);
            assert_eq!(SendMessageW(window.0, WM_MOUSEACTIVATE, WPARAM(0), click).0, MA_NOACTIVATE as isize);
            let style = GetWindowLongPtrW(window.0, GWL_EXSTYLE);
            SetWindowLongPtrW(window.0, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE.0 as isize));
            assert_eq!(SendMessageW(window.0, WM_MOUSEACTIVATE, WPARAM(0), click).0, MA_ACTIVATE as isize);
        }
    }
}
