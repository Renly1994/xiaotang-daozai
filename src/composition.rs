//! 毛玻璃皮肤的实时背景模糊：Windows 合成引擎（Windows.UI.Composition）。
//!
//! 早先用的 `SetWindowCompositionAttribute` 模糊总是铺满整个窗口矩形、不认窗口区域，
//! 而岛仔窗口是一大块透明画布，于是桌面顶部出现一整块深色方块。
//!
//! 现在的做法：单独建一个只承载合成视觉的「模糊底板」窗口（无重定向位图、不接收鼠标、
//! 不出现在任务栏），与岛仔窗口同位置同大小，并始终紧贴在它**正下方**。底板上的每块视觉
//! 都用宿主背景画刷（Windows 11 亚克力用的系统实时模糊），主体按圆角裁剪，顶部外翻的
//! 肩部按物理像素逐行拼出，模糊因此严格贴合岛体轮廓；岛体的着色、高光与文字仍由 egui
//! 画在上层的岛仔窗口里。（直接挂在岛仔窗口上时，部分显卡会把视觉层叠到内容之上。）

use crate::app::BlurPiece;
use windows::core::Interface;
use windows::Foundation::Numerics::{Vector2, Vector3};
use windows::System::DispatcherQueueController;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{CompositionBrush, CompositionRoundedRectangleGeometry, Compositor, ContainerVisual, SpriteVisual, VisualCollection};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH};
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{CreateDispatcherQueueController, DispatcherQueueOptions, RoInitialize, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, RO_INIT_SINGLETHREADED};

/// 底板窗口：鼠标一律穿透、不激活。
unsafe extern "system" fn backdrop_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn create_backdrop_window() -> windows::core::Result<HWND> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    static CLASS: std::sync::Once = std::sync::Once::new();
    let instance = unsafe { GetModuleHandleW(None)? };
    let class = windows::core::w!("DaozaiGlassBackdrop");
    CLASS.call_once(|| unsafe {
        let wc = WNDCLASSW { lpfnWndProc: Some(backdrop_proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
    });
    const WS_EX_NOREDIRECTIONBITMAP: WINDOW_EX_STYLE = WINDOW_EX_STYLE(0x0020_0000);
    unsafe {
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            class,
            windows::core::w!(""),
            WS_POPUP,
            0, 0, 1, 1,
            None, None, instance, None,
        )
    }
}

pub struct Backdrop {
    hwnd: HWND,
    placed: (RECT, bool),
    // 合成器依赖当前线程的 DispatcherQueue；控制器必须与合成器同生命周期。
    _queue: Option<DispatcherQueueController>,
    compositor: Compositor,
    _target: DesktopWindowTarget,
    root: ContainerVisual,
    children: VisualCollection,
    brush: CompositionBrush,
    pool: Vec<(SpriteVisual, CompositionRoundedRectangleGeometry)>,
    shown: Vec<BlurPiece>,
    visible: bool,
}

impl Backdrop {
    pub fn new() -> windows::core::Result<Self> {
        let hwnd = create_backdrop_window()?;
        unsafe {
            // 主线程已由窗口框架初始化为单线程套间时返回 S_FALSE，忽略即可。
            let _ = RoInitialize(RO_INIT_SINGLETHREADED);
        }
        let queue = unsafe {
            CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })
        }
        .ok();
        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        // 底板窗口没有自己的内容，视觉树就是它的全部画面。
        let target = unsafe { interop.CreateDesktopWindowTarget(hwnd, BOOL::from(true)) }?;
        let root = compositor.CreateContainerVisual()?;
        target.SetRoot(&root)?;
        let children = root.Children()?;
        let brush: CompositionBrush = compositor.CreateHostBackdropBrush()?.cast()?;
        unsafe {
            // Windows 11 需要显式允许宿主背景画刷；Windows 10 不认识该属性，忽略错误。
            let on = BOOL::from(true);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, (&on as *const BOOL).cast(), std::mem::size_of::<BOOL>() as u32);
        }
        root.SetIsVisible(false)?;
        Ok(Self { hwnd, placed: (RECT::default(), false), _queue: queue, compositor, _target: target, root, children, brush, pool: Vec::new(), shown: Vec::new(), visible: false })
    }

    /// 让底板与岛仔窗口重合，并紧贴在它正下方。
    fn follow(&mut self, island: HWND) {
        unsafe {
            let mut rect = RECT::default();
            if GetWindowRect(island, &mut rect).is_err() { return; }
            let below = GetWindow(island, GW_HWNDNEXT).ok() == Some(self.hwnd);
            if self.placed.0 != rect || !below || !self.placed.1 {
                let _ = SetWindowPos(self.hwnd, island, rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                self.placed = (rect, true);
            }
        }
    }

    /// 显示并更新模糊块（物理像素，岛仔窗口坐标）。形状不变时不提交任何改动。
    pub fn show(&mut self, island: HWND, pieces: &[BlurPiece]) -> windows::core::Result<()> {
        self.follow(island);
        if pieces != self.shown.as_slice() {
            while self.pool.len() < pieces.len() {
                let visual = self.compositor.CreateSpriteVisual()?;
                visual.SetBrush(&self.brush)?;
                let geometry = self.compositor.CreateRoundedRectangleGeometry()?;
                let clip = self.compositor.CreateGeometricClipWithGeometry(&geometry)?;
                visual.SetClip(&clip)?;
                self.children.InsertAtTop(&visual)?;
                self.pool.push((visual, geometry));
            }
            for (i, (visual, geometry)) in self.pool.iter().enumerate() {
                match pieces.get(i) {
                    Some(p) => {
                        let size = Vector2 { X: p.w, Y: p.h };
                        visual.SetOffset(Vector3 { X: p.x, Y: p.y, Z: 0.0 })?;
                        visual.SetSize(size)?;
                        geometry.SetSize(size)?;
                        geometry.SetCornerRadius(Vector2 { X: p.radius, Y: p.radius })?;
                        visual.SetIsVisible(true)?;
                    }
                    None => visual.SetIsVisible(false)?,
                }
            }
            self.shown = pieces.to_vec();
        }
        if !self.visible {
            self.root.SetIsVisible(true)?;
            self.visible = true;
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible {
            let _ = self.root.SetIsVisible(false);
            self.visible = false;
        }
        if self.placed.1 {
            unsafe { let _ = ShowWindow(self.hwnd, SW_HIDE); }
            self.placed.1 = false;
        }
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        unsafe { let _ = DestroyWindow(self.hwnd); }
    }
}
