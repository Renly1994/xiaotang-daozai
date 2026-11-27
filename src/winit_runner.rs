//! winit 事件循环 + D3D11 自绘运行时，替换 eframe::run_native。
//! 主窗口（岛体）+ 独立子 viewport（设置页/邮件阅读器）多窗口管理。

use crate::Shell;
use daozai::d3d11::runtime::{D3d11Runtime, GlassParams, PlainWindow, FrameStats};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    platform::windows::WindowAttributesExtWindows,
    window::{Window, WindowAttributes, WindowId},
};

/// 照搬 eframe 0.35 的 UserEvent：repaint 请求必须带 viewport id / 到期时刻 / 请求时的
/// pass 序号，才能按窗口精确调度并丢弃过期请求（原来只发 `()`，语义全丢）。
#[derive(Debug)]
enum Wakeup {
    Repaint {
        viewport_id: egui::ViewportId,
        when: Instant,
        cumulative_pass_nr: u64,
    },
    /// 子窗口 resize/DPI 变化：立即补跑父窗口布局（eframe 的 RepaintNext(parent)）。
    LayoutDirty,
}

/// egui repaint 唤醒事件循环的代理。
static REPAINT_PROXY: std::sync::OnceLock<winit::event_loop::EventLoopProxy<Wakeup>> = std::sync::OnceLock::new();

thread_local! {
    /// immediate viewport renderer 回调（在 draw 上下文同步执行）把该帧子窗口的
    /// FullOutput 放进此队列；drain_viewports 在事件循环里消费、建/更新 winit 窗口。
    /// 存 output 而非 UI 闭包：immediate 回调借用 app，不能跨帧存；output 可以。
    static VIEWPORT_REQ: RefCell<Vec<(egui::ViewportId, Rc<Window>, egui::FullOutput)>> = RefCell::new(Vec::new());
    /// immediate 回调每帧都触发（show_viewport_immediate 每帧调用），窗口必须复用，
    /// 否则每帧 create 一个 invisible 新窗口（builder with_visible(false)）。
    static VIEWPORT_WINDOWS: RefCell<HashMap<egui::ViewportId, (Rc<Window>, egui::ViewportBuilder)>> = RefCell::new(HashMap::new());
    /// 子窗口的输入（含鼠标/键盘事件）：window_event 累积，draw_main 前取出交给
    /// immediate 回调（回调在主窗口 draw 上下文执行，拿不到 sub.renderer）。
    static VIEWPORT_INPUTS: RefCell<HashMap<egui::ViewportId, egui::RawInput>> = RefCell::new(HashMap::new());
    /// 子窗口右上角 X 的关闭请求（ViewportId）：window_event 记录，immediate 回调
    /// 转成 ViewportInfo.events[Close]，让设置页 UI 的 close_requested() 读到并自行关闭。
    static CLOSE_REQ: RefCell<Vec<egui::ViewportId>> = RefCell::new(Vec::new());
    /// 照搬 eframe 0.35：回调在 draw 上下文执行但 create_window 需要 &ActiveEventLoop，
    /// 用 thread_local 存当前 event loop（guard 保证生命周期）。
    static CURRENT_EVENT_LOOP: std::cell::Cell<Option<*const ActiveEventLoop>> = const { std::cell::Cell::new(None) };
}

/// 在事件处理方法入口建立 event loop 上下文，guard drop 时清除。
fn with_event_loop_context(el: &ActiveEventLoop, f: impl FnOnce()) {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) { CURRENT_EVENT_LOOP.with(|c| c.set(None)); }
    }
    CURRENT_EVENT_LOOP.with(|c| c.set(Some(std::ptr::from_ref::<ActiveEventLoop>(el))));
    let _guard = Guard;
    f();
}
fn with_current_event_loop<F, R>(f: F) -> Option<R>
where F: FnOnce(&ActiveEventLoop) -> R {
    CURRENT_EVENT_LOOP.with(|c| c.get().map(|ptr| {
        #[allow(unsafe_code)] // 指针由 guard 持有，存活期间有效（eframe 同款）
        unsafe { f(&*ptr) }
    }))
}

struct SubWindow {
    window: Rc<Window>,
    renderer: PlainWindow,
    output: Option<egui::FullOutput>,
    id: egui::ViewportId,
    /// 子窗口刚被 resize：内容需要按新尺寸重跑布局，而不是拉伸旧 output。
    /// Windows 拖动边框时进入模态 sizing 循环，about_to_wait 不跑，
    /// 只能靠 RedrawRequested 触发父窗口重算 viewport layout。
    layout_dirty: bool,
}

struct DaozaiApp {
    /// 启动期独显偏好快照，传给 D3D11 runtime 与设置页重启提示。
    prefer_dgpu: bool,
    attrs: WindowAttributes,
    window: Option<Window>,
    runtime: Option<D3d11Runtime>,
    shell: Option<Shell>,
    subs: HashMap<WindowId, SubWindow>,
    sub_ids: HashMap<egui::ViewportId, WindowId>,
    pending_close: Vec<WindowId>,
    /// 跨 suspended/resume 与设备丢失保留的 egui Context。
    /// Shell、后台线程、字体图集和 repaint callback 都绑在它上面，重建 D3D 栈时不能换。
    egui_ctx: Option<egui::Context>,
    /// 下一次需要跑父窗口 UI 的时刻。None = 没有待处理请求（长睡眠等输入事件）。
    next_repaint: Option<Instant>,
    /// draw_main 检测到 Present 报设备丢失后延迟到事件方法末尾重建，
    /// 避免同时持有 &Window 与 &mut self。
    device_lost_pending: bool,
}

impl DaozaiApp {
    fn new(attrs: WindowAttributes, prefer_dgpu: bool) -> Self {
        Self { prefer_dgpu, attrs, window: None, runtime: None, shell: None,
               subs: HashMap::new(), sub_ids: HashMap::new(), pending_close: Vec::new(),
               egui_ctx: None, next_repaint: None, device_lost_pending: false }
    }

    fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(windows::Win32::Foundation::HWND(h.hwnd.get() as _)),
            _ => None,
        }
    }

    /// D3 设备或窗口会话重建后，旧的 PlainWindow 全绑在旧 device/renderer 上，
    /// 必须连同 winit 子窗口缓存一起丢弃；下一帧 show_viewport_immediate 会重建。
    fn clear_viewport_state(&mut self) {
        self.subs.clear();
        self.sub_ids.clear();
        self.pending_close.clear();
        VIEWPORT_REQ.with(|q| q.borrow_mut().clear());
        VIEWPORT_WINDOWS.with(|m| m.borrow_mut().clear());
        VIEWPORT_INPUTS.with(|m| m.borrow_mut().clear());
        CLOSE_REQ.with(|q| q.borrow_mut().clear());
    }

    /// 用保留的 egui Context 重建 D3D11 栈，并让 Shell 重新绑定新 HWND。
    fn recreate_graphics(&mut self, window: &Window) -> bool {
        let Some(ctx) = self.egui_ctx.clone() else { return false };
        let Some(hwnd) = Self::hwnd(window) else { return false };
        match D3d11Runtime::new_with_ctx(window, ctx.clone(), self.prefer_dgpu) {
            Ok(runtime) => {
                self.runtime = Some(runtime);
                if let Some(sh) = self.shell.as_mut() {
                    // native::Window 的 subclass / event hook / 拖放监听都按旧 HWND 注册，
                    // 换窗口后必须重绑定；Drop 会摘掉旧 hook。
                    sh.native = daozai::native::Window::new(hwnd, ctx.clone());
                    sh.placed = false; // 新窗口要重新按目标屏幕定位
                }
                self.clear_viewport_state();
                self.repaint_now();
                true
            }
            Err(e) => {
                eprintln!("[d3d11] 重建图形栈失败，稍后重试: {e:?}");
                self.schedule_repaint(Instant::now() + Duration::from_millis(500));
                false
            }
        }
    }

    /// draw_main 之后统一处理设备丢失：把窗口从 self 里暂时拿出，
    /// 这样重建函数可以同时借用其他字段与 &Window。
    fn flush_device_recovery(&mut self) {
        if !self.device_lost_pending { return; }
        self.device_lost_pending = false;
        let Some(window) = self.window.take() else { return };
        self.recreate_graphics(&window);
        self.window = Some(window);
    }

    /// 预约下一次父窗口绘制；多次请求取最早的那个（与 eframe 的 next_repaint_time 同语义）。
    fn schedule_repaint(&mut self, when: Instant) {
        self.next_repaint = Some(self.next_repaint.map_or(when, |cur| cur.min(when)));
    }

    /// 输入/尺寸变化等需要立刻跟手的绘制。
    fn repaint_now(&mut self) {
        self.schedule_repaint(Instant::now());
    }

    /// 消费 immediate viewport 请求：新建/更新子窗口，并保存该帧 output 供绘制。
    fn drain_viewports(&mut self) {
        let reqs = VIEWPORT_REQ.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for (id, window, output) in reqs {
            let wid = window.id();
            if let Some(&existing) = self.sub_ids.get(&id) {
                if let Some(sub) = self.subs.get_mut(&existing) {
                    sub.window = window;
                    sub.output = Some(output);
                }
                continue;
            }
            let Some(rt) = self.runtime.as_ref() else { continue };
            let renderer = match PlainWindow::new(&window, &rt.egui_ctx, false,
                rt.device(), rt.device_context(), rt.shared_renderer()) {
                Ok(r) => r, Err(e) => { eprintln!("[d3d11] PlainWindow failed: {e}"); continue; }
            };
            eprintln!("[winit] sub viewport {id:?} -> window {wid:?}");
            // builder 里 with_visible(false) 防白屏，但 Visible(true) 命令在首帧发出时
            // 窗口尚未创建（drain 在 render 之后）而丢失，这里窗口建好后主动显示。
            window.set_visible(true);
            self.sub_ids.insert(id, wid);
            self.subs.insert(wid, SubWindow { window, renderer, output: Some(output), id, layout_dirty: false });
        }
    }

    /// 主窗口绘制一帧。在 about_to_wait 的事件循环上下文中调用，
    /// 避免在 RedrawRequested handler 内创建子窗口造成重入死锁。
    fn draw_main(&mut self) {
        let Some(w) = self.window.as_ref() else { return };
        let origin = w.inner_position().map(|p| (p.x as f32, p.y as f32)).unwrap_or((0.0, 0.0));
        // 「岛体大小」旧语义：只缩放主 viewport 输入，不能动全局 zoom（会把设置页一起放大）。
        let scale = self.shell.as_ref()
            .map(|s| s.app.settings.island_scale())
            .unwrap_or(1.0);
        let ppp = w.scale_factor() as f32 * scale;
        let liquid = self.shell.as_ref()
            .is_some_and(|s| s.app.settings.skin == daozai::model::Skin::Liquid);
        let dock = self.shell.as_ref()
            .map(|s| s.app.settings.dock)
            .unwrap_or(daozai::model::Dock::Top);
        let params = build_glass_params(ppp, liquid, dock);
        let mut input = { let w = self.window.as_ref().unwrap(); self.runtime.as_mut().unwrap().take_input(w) };
        scale_root_input(&mut input, scale, w.scale_factor() as f32);
        // 按上游 raw_input_hook：把 ROOT 的 screen_rect 固定成完整画布尺寸。
        // scale_root_input 把 egui-winit 算出的 screen_rect 又除以 island_scale，
        // 逻辑画布高度会不足（如 321 < WIN_H 402）；底部停靠时岛体画在 win 底部
        // y≈354 超出画布被裁掉 → "闪一下消失"。这里覆盖回完整画布。
        if input.viewport_id==egui::ViewportId::ROOT {
            let size=if dock.is_side() {
                egui::vec2(daozai::side_dock::SIDE_W, daozai::side_dock::SIDE_H)
            } else {
                egui::vec2(daozai::app::CANVAS_W, daozai::app::WIN_H)
            };
            input.screen_rect=Some(egui::Rect::from_min_size(egui::Pos2::ZERO,size));
        }
        // 收集子窗口输入（含鼠标/键盘事件）供 immediate 回调使用。
        for sub in self.subs.values_mut() {
            let sin = sub.renderer.egui_winit.take_egui_input(&sub.window);
            VIEWPORT_INPUTS.with(|m| { m.borrow_mut().insert(sub.id, sin); });
        }
        let window = self.window.as_ref().unwrap();
        let stats: FrameStats = {
            let rt = self.runtime.as_mut().unwrap();
            let sh = self.shell.as_mut().unwrap();
            rt.render(window, origin, &params, input, scale, |_ctx, ui| sh.draw(ui))
        };
        if stats.device_lost {
            // 设备已重置：这一帧的 viewport output 全部作废，交给事件方法末尾重建。
            self.device_lost_pending = true;
            return;
        }
        // 把主窗口 output 里子 viewport 的窗口命令（Visible(true)/位置/大小）应用到子窗口。
        let egui_ctx = self.runtime.as_ref().unwrap().egui_ctx.clone();
        for (vid, cmds) in stats.viewport_commands {
            if let Some(&wid) = self.sub_ids.get(&vid) {
                if let Some(sub) = self.subs.get_mut(&wid) {
                    egui_winit::process_viewport_commands(&egui_ctx, &mut sub.renderer.viewport_info,
                        cmds, &sub.window, &mut sub.renderer.actions);
                }
            }
        }
        // app 不再 show_viewport 的子窗口（如设置页 settings_open=false）必须销毁，
        // 否则窗口对象残留、右上角 X 关不掉（eframe remove_viewports_not_in 同款）。
        let stale: Vec<egui::ViewportId> = self.sub_ids.keys()
            .filter(|vid| !stats.active_viewports.contains(vid))
            .copied().collect();
        for vid in stale {
            if let Some(wid) = self.sub_ids.remove(&vid) {
                self.subs.remove(&wid);
                self.pending_close.retain(|w| *w != wid);
                VIEWPORT_WINDOWS.with(|m| { m.borrow_mut().remove(&vid); });
                VIEWPORT_INPUTS.with(|m| { m.borrow_mut().remove(&vid); });
            }
        }
        self.drain_viewports();
        // immediate viewport 由父窗口驱动重绘：父每帧 draw 时通过回调拿到子窗口 output，
        // 这里统一画所有子窗口（含可见的）。原来的"只画 invisible"是内嵌 workaround，
        // 独立窗口后必须每帧画，否则内容区空白。
        let subs: Vec<WindowId> = self.subs.keys().copied().collect();
        for wid in subs { self.render_sub_inner(wid); }
        // 布局已按新尺寸重跑，清除 resize 脏标记。
        for sub in self.subs.values_mut() { sub.layout_dirty = false; }
    }

    /// 绘制子窗口一帧（RedrawRequested 与"不可见窗口直接绘制"共用）。
    fn render_sub(&mut self, wid: WindowId) {
        self.render_sub_inner(wid);
    }
    fn render_sub_inner(&mut self, wid: WindowId) {
        let Some(rt) = self.runtime.as_ref() else { return };
        let ctx = rt.egui_ctx.clone();
        let Some(sub) = self.subs.get_mut(&wid) else { return };
        // immediate viewport 的 UI 在主窗口 draw 时已同步跑过，这里只画存起来的 output。
        if let Some(output) = sub.output.take() {
            sub.renderer.paint_output(&sub.window, &ctx, sub.id, output);
        }
    }
}

impl ApplicationHandler<Wakeup> for DaozaiApp {
    fn user_event(&mut self, el: &ActiveEventLoop, event: Wakeup) {
        match event {
            Wakeup::Repaint { viewport_id, when, cumulative_pass_nr } => {
                // 丢弃过期请求：本窗口已经画过比请求时更新的 pass 了（eframe run.rs:303-332 同款）。
                let current = self.runtime.as_ref()
                    .map(|rt| rt.egui_ctx.cumulative_pass_nr_for(viewport_id))
                    .unwrap_or(0);
                if current == cumulative_pass_nr || current == cumulative_pass_nr + 1 {
                    self.schedule_repaint(when);
                }
            }
            Wakeup::LayoutDirty => {
                // Windows 模态 sizing 循环里 about_to_wait 会停摆，不能只约下一帧：
                // 必须在这里直接补跑父窗口，让 immediate viewport 按新尺寸重排设置页。
                self.next_repaint = None;
                with_event_loop_context(el, || {
                    self.drain_viewports();
                    self.draw_main();
                });
                for sub in self.subs.values() { sub.window.request_redraw(); }
                self.flush_device_recovery();
            }
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let window = match el.create_window(self.attrs.clone()) {
            Ok(w) => w,
            Err(e) => { eprintln!("[winit] create_window failed: {e}"); el.exit(); return; }
        };
        let Some(hwnd) = Self::hwnd(&window) else {
            eprintln!("[winit] not a win32 window"); el.exit(); return;
        };
        // 关键：设备重置 / 系统 suspend 后重建 D3D 栈时沿用同一个 Context，
        // 否则 Shell、后台线程手里的旧 Context 与字体图集/repaint callback 全部脱节。
        let ctx = self.egui_ctx.clone().unwrap_or_else(egui::Context::default);
        let runtime = match D3d11Runtime::new_with_ctx(&window, ctx.clone(), self.prefer_dgpu) {
            Ok(r) => r,
            Err(e) => { eprintln!("[d3d11] runtime init failed: {e}"); el.exit(); return; }
        };

        if self.egui_ctx.is_none() {
            self.egui_ctx = Some(ctx.clone());
            if let Some(proxy) = REPAINT_PROXY.get() {
                let px = proxy.clone();
                ctx.set_request_repaint_callback(move |info| {
                    let _ = px.send_event(Wakeup::Repaint {
                        viewport_id: info.viewport_id,
                        when: Instant::now() + info.delay,
                        cumulative_pass_nr: info.current_cumulative_pass_nr,
                    });
                });
            }
            // 让 show_viewport_immediate 走原生独立窗口（egui 默认内嵌会把设置页画到岛仔本体）。
            // 照搬 eframe 0.35：注册 immediate renderer，回调在 draw 上下文同步执行——
            // 用 event_loop_context 取当前 event loop 建窗口，跑 viewport_ui_cb 拿 output 存队列。
            ctx.set_embed_viewports(false);
            egui::Context::set_immediate_viewport_renderer(|egui_ctx, viewport| {
                let egui::ImmediateViewport { ids, builder, mut viewport_ui_cb } = viewport;
                let id = ids.this;
                // 复用已创建的窗口；只有首次或 builder 变化时才 create。
                let need_create = VIEWPORT_WINDOWS.with(|m| {
                    m.borrow().get(&id).map(|(_, old)| *old != builder).unwrap_or(true)
                });
                if need_create {
                    let attrs = egui_winit::create_winit_window_attributes(egui_ctx, builder.clone());
                    let Some(Ok(window)) = with_current_event_loop(|el| el.create_window(attrs)) else {
                        eprintln!("[winit] viewport {id:?} create_window failed");
                        return;
                    };
                    let window = Rc::new(window);
                    egui_winit::apply_viewport_builder_to_window(egui_ctx, &window, &builder);
                    VIEWPORT_WINDOWS.with(|m| { m.borrow_mut().insert(id, (window, builder.clone())); });
                }
                let Some(window) = VIEWPORT_WINDOWS.with(|m| m.borrow().get(&id).map(|(w, _)| w.clone())) else { return };
                // 子窗口 input：优先用 window_event 累积的真实 input（含鼠标/键盘），
                // 否则（首帧）构造最小 input。viewport_id/viewports 必须配套，
                // 否则设置页 ctx.input(|i| i.viewport()) 会 panic（egui RawInput::viewport）。
                let mut input = VIEWPORT_INPUTS.with(|m| m.borrow_mut().remove(&id)).unwrap_or_default();
                input.viewport_id = id;
                // 子窗口永远用自己窗口的原生 DPI，不能读 egui_ctx.pixels_per_point() /
                // native_pixels_per_point()——那是主 viewport 按「岛体大小」放大后的值。
                let native_ppp = window.scale_factor() as f32;
                if input.screen_rect.is_none() {
                    let size = window.inner_size();
                    input.screen_rect = Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0),
                        egui::vec2(size.width as f32 / native_ppp, size.height as f32 / native_ppp)));
                }
                let outer = window.outer_position().ok().map(|p| (p.x, p.y));
                let size = window.outer_size();
                let monitor = window.current_monitor()
                    .map(|m| (m.size().width, m.size().height));
                let (outer_rect, monitor_size) = child_viewport_geometry(
                    outer, (size.width, size.height), monitor,
                    window.is_minimized() == Some(true), native_ppp,
                );
                let info = input.viewports.entry(id).or_default();
                info.native_pixels_per_point = Some(native_ppp);
                if info.inner_rect.is_none() { info.inner_rect = input.screen_rect; }
                info.outer_rect = outer_rect;
                info.monitor_size = monitor_size;
                info.title = Some(window.title());
                if input.time.is_none() { input.time = Some(egui_ctx.input(|i| i.time)); }
                // 右上角 X：把 CloseRequested 转成 egui ViewportEvent::Close，
                // 设置页 UI 的 viewport().close_requested() 读到后置 settings_open=false。
                let close_now = CLOSE_REQ.with(|q| {
                    let mut q = q.borrow_mut();
                    let hit = q.iter().position(|&v| v == id);
                    hit.map(|i| { q.remove(i); })
                });
                if close_now.is_some() {
                    input.viewports.entry(id).or_default()
                        .events.push(egui::ViewportEvent::Close);
                }
                // 同步跑子窗口 UI（viewport_ui_cb 借用 app，只能在此调用，不能跨帧存）。
                let output = egui_ctx.run_ui(input, |ui| viewport_ui_cb(ui));
                VIEWPORT_REQ.with(|q| q.borrow_mut().push((id, window, output)));
            });
        }

        // Shell 承载全部 app 状态与后台服务：suspend 时保留，resume 只重建窗口绑定。
        if self.shell.is_none() {
            self.shell = Some(Shell::new(ctx.clone(), hwnd, daozai::app::Options {
                liquid_prefer_dgpu_boot: self.prefer_dgpu,
                ..Default::default()
            }));
        } else if let Some(sh) = self.shell.as_mut() {
            sh.native = daozai::native::Window::new(hwnd, ctx.clone());
            sh.placed = false;
        }
        // 旧子窗口的 PlainWindow 绑在已销毁的 device 上，必须整体重来。
        self.clear_viewport_state();
        self.runtime = Some(runtime);
        self.window = Some(window);
        self.repaint_now(); // 首帧 / resume 后第一帧
    }

    fn suspended(&mut self, _el: &ActiveEventLoop) {
        // 只丢与设备/窗口绑定的资源；Shell（app 状态 + 后台服务）和 egui Context 必须留着，
        // 否则 resume 后等于冷启动：通知/邮件/歌词状态全丢，用户数据观感像"重启过"。
        self.window.take();
        self.runtime.take();
        self.clear_viewport_state();
        self.next_repaint = None;
        self.device_lost_pending = false;
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, _cause: winit::event::StartCause) {
        // 绘制统一在 about_to_wait：它按 egui callback 的 delay 决定下一帧时刻，
        // 这里如果再设固定 16ms 唤醒会覆盖精确调度、把空闲态重新变成 60fps 空转。
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        if self.next_repaint.is_some_and(|t| t <= now) {
            // 先消费本次请求；绘制过程中 ctx.request_repaint[_after] 会重新预约下一次。
            self.next_repaint = None;
            with_event_loop_context(el, || {
                self.drain_viewports();
                self.draw_main();
            });
            for sub in self.subs.values() { sub.window.request_redraw(); }
            self.flush_device_recovery();
        }
        // 没有待处理重绘时长睡眠：输入事件/代理消息会立刻唤醒事件循环，
        // 而不是像旧版每 16ms 无条件跑一帧（隐藏态 1fps / 静止 15fps 请求才能生效）。
        let deadline = self.next_repaint.unwrap_or_else(|| Instant::now() + Duration::from_secs(60));
        el.set_control_flow(ControlFlow::WaitUntil(deadline));
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_some_and(|w| w.id() == id) {
            let resized = matches!(&event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. });
            let repaint = self.runtime.as_mut().map(|r| {
                let w = self.window.as_ref().unwrap();
                r.on_window_event(w, &event)
            }).unwrap_or(false);
            // 输入和 resize 必须立刻跟手；旧版在这里丢弃 repaint，只靠 16ms 心跳碰巧刷新。
            if repaint || resized { self.repaint_now(); }
            return;
        }

        // 子 viewport 窗口
        let mut repaint = false;
        if let Some(sub) = self.subs.get_mut(&id) {
            if event == WindowEvent::CloseRequested {
                // 转成 egui 的 Close 事件喂给设置页 UI（它读 viewport().close_requested()
                // 自行把 settings_open 置 false，下一帧不再 show_viewport 窗口即关闭）。
                let vid = sub.id;
                CLOSE_REQ.with(|q| q.borrow_mut().push(vid));
                self.pending_close.push(id);
                return;
            }
            if matches!(&event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
                // 尺寸/DPI 变化必须让 immediate viewport 回调重跑一次布局才能正确重排；
                // 只 request_redraw 会画缓存 output（旧布局被拉伸）。
                // 对应 eframe 原生 run_ui_and_paint 里的 RepaintNext(parent_viewport)：
                // 发 user event，在 user_event handler 里安全地跑 draw_main（避免重入）。
                sub.layout_dirty = true;
                if let Some(proxy) = REPAINT_PROXY.get() { let _ = proxy.send_event(Wakeup::LayoutDirty); }
            }
            repaint = sub.renderer.on_event(&sub.window, &event);
        }
        if repaint {
            // 子窗口输入进了 VIEWPORT_INPUTS，immediate 回调只在父窗口 draw 里消费。
            if let Some(sub) = self.subs.get(&id) { sub.window.request_redraw(); }
            self.repaint_now();
        }
        if let WindowEvent::RedrawRequested = event { self.render_sub(id); }
    }
}

/// 把 winit 子窗口的物理几何换算成 egui 的逻辑点，补齐 ViewportInfo。
/// egui-winit 的 take_egui_input 只填 native_pixels_per_point，outer_rect /
/// monitor_size 为空会让依赖边框宽度的设置页恢复逻辑退化成硬编码估算值。
fn child_viewport_geometry(
    outer_position: Option<(i32, i32)>,
    outer_size: (u32, u32),
    monitor_size: Option<(u32, u32)>,
    minimized: bool,
    native_ppp: f32,
) -> (Option<egui::Rect>, Option<egui::Vec2>) {
    if minimized || !native_ppp.is_finite() || native_ppp <= 0.0 {
        return (None, None);
    }
    let outer = outer_position.map(|(x, y)| {
        egui::Rect::from_min_size(
            egui::pos2(x as f32 / native_ppp, y as f32 / native_ppp),
            egui::vec2(outer_size.0 as f32 / native_ppp, outer_size.1 as f32 / native_ppp),
        )
    });
    let monitor = monitor_size.map(|(w, h)| egui::vec2(w as f32 / native_ppp, h as f32 / native_ppp));
    (outer, monitor)
}

/// 从 app 当前状态构造 HLSL 玻璃 uniform（岛体胶囊布局）。
/// 只把主窗口输入换算到放大/缩小后的逻辑点（复刻旧 vendor eframe 的
/// `scale_root_input`）。`Context::set_zoom_factor` 是全局的，会把设置页一起放大；
/// 这里只改 ROOT viewport 的 ppp/矩形/指针事件，子 viewport 完全不受影响。
fn scale_root_input(raw: &mut egui::RawInput, scale: f32, native_ppp: f32) {
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }
    // egui-winit 的 screen_rect / pointer 事件用的是窗口原生 DPI（window.scale_factor），
    // 不受我们写进 ViewportInfo 的 native_pixels_per_point 影响；所以每帧都固定除一次
    // scale，正好把原生逻辑点换成放大/缩小后的 UI 点。
    let inv = 1.0 / scale;
    let pos = |p: egui::Pos2| egui::pos2(p.x * inv, p.y * inv);
    let rect = |r: egui::Rect| egui::Rect::from_min_max(pos(r.min), pos(r.max));
    if let Some(r) = raw.screen_rect.as_mut() {
        *r = rect(*r);
    }
    if let Some(info) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
        info.native_pixels_per_point = Some(native_ppp * scale);
        info.inner_rect = info.inner_rect.map(rect);
        info.outer_rect = info.outer_rect.map(rect);
        info.monitor_size = info.monitor_size.map(|s| s * inv);
    }
    for event in &mut raw.events {
        match event {
            egui::Event::PointerMoved(p) => *p = pos(*p),
            egui::Event::PointerButton { pos: p, .. } => *p = pos(*p),
            egui::Event::Touch { pos: p, .. } => *p = pos(*p),
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. } => *delta *= inv,
            _ => {}
        }
    }
}

///
/// 坐标系（移植自旧 wgpu 栈 gpu_glass::build_uniform）：app/egui 传进来的几何和
/// 光学强度全是「逻辑点 / CSS px」，而 shader 的全屏三角形铺的是「物理像素」渲染
/// 目标（= 逻辑尺寸 × ppp，如 960×270 × 1.25 = 1200×338）。所有像素单位必须先乘
/// ppp 转成设备像素——漏掉这步会让 shader 认为画布只有实际的 1/1.25，采到的桌面
/// 区域随之缩小 1.25 倍，表现为「玻璃里的背景被放大一圈」（高 DPI 屏尤甚）。
fn build_glass_params(ppp: f32, liquid: bool, dock: daozai::model::Dock) -> GlassParams {
    let u = daozai::gpu_glass::uniforms();
    let mut p = GlassParams::default();
    if !liquid || !u.active {
        // 非液态皮肤 / 液态 pass 未激活：不画玻璃（shader 的 bounds 设到窗口外）。
        // 只信当前皮肤，不信 u.active——切换皮肤时它可能有上一帧的残留值。
        p.bounds = [-10000.0, -10000.0, 1.0, 1.0];
        return p;
    }
    let ppp = ppp.max(0.1);
    // 画布物理尺寸 = egui 逻辑画布 × ppp（与 D3D11 RT / swap 的真实尺寸一致）。
    // 左右停靠时 native.rs 会把窗口 resize 成 SIDE_W×SIDE_H（500×600），
    // 不能再用顶部的 CANVAS_W×WIN_H——否则 y 翻转基准全长短不一，玻璃被翻到窗口外。
    let (canvas_w, canvas_h) = if dock.is_side() {
        (daozai::side_dock::SIDE_W, daozai::side_dock::SIDE_H)
    } else {
        (crate::app::CANVAS_W, crate::app::WIN_H)
    };
    let cw = canvas_w * ppp;
    let h = canvas_h * ppp;
    // 岛体矩形、指针：逻辑点 → 设备像素。y 从 egui(顶原点) 翻到 D3D11(底原点)。
    let b = u.body_rect;
    let bx = b[0] * ppp;
    let by = b[1] * ppp;
    let bw = b[2] * ppp;
    let bh = b[3] * ppp;
    p.bounds = [bx, h - by - bh, bw, bh];
    // extra.w = 停靠方向（0=Top, 1=Left, 2=Right, 3=Bottom），shader 用它把刘海肩部旋到贴边侧。
    let dock_code = match dock {
        daozai::model::Dock::Top => 0.0,
        daozai::model::Dock::Left => 1.0,
        daozai::model::Dock::Right => 2.0,
        daozai::model::Dock::Bottom => 3.0,
    };
    p.extra = [u.radius * ppp, cw, h, dock_code];
    // 光学强度（折射/色散/模糊，单位 CSS px）同样转设备像素；morph/material 无量纲不缩放。
    // params.y = ppp，供 refresh_control_uniforms 反算控件透镜坐标（左右停靠画布高不同，
    // 不能再用 canvas_h/WIN_H 反推）。shader 里 params.y 未参与光学计算，安全复用。
    p.params = [u.chroma * ppp, ppp, u.morph, u.material];
    p.misc = [u.pointer[0] * ppp, h - u.pointer[1] * ppp, u.refract * ppp, 0.0];
    p.look = [u.blur * ppp, u.band * ppp, u.edge_w * ppp, u.edge_op];
    // x=高光跟随, z=岛体投影开关（shader 在玻璃 pass 里画外阴影）
    p.lightcfg = [u.light_follow, 0.0, u.shadow, 0.0];
    // 控件玻璃透镜在 D3d11Runtime::render 里、run_ui 之后填充——
    // app 的控件快照由 run_ui 当帧写入，在这里填会读到上一帧，展开/收起动画时差一帧
    // 会在宠物与时间之间留下一条残留透镜白边。见 refresh_control_uniforms。
    // 深浅材质的玻璃染色：暗色玻璃偏冷灰，浅色偏白。
    p.tint = if u.material > 0.5 { [0.96,0.97,1.0,0.10] } else { [0.55,0.72,0.90,0.10] };
    // x=肩部外扩, y=肩部高度, z=刘海形状开关（0=胶囊）, w=DComp 逐像素 alpha。
    let shape = daozai::gpu_glass::shape_params();
    p.notch = [shape[0] * ppp, shape[1] * ppp, shape[2], 1.0];
    p
}


pub fn run(prefer_dgpu: bool) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::<Wakeup>::with_user_event().build()?;
    let _ = REPAINT_PROXY.set(event_loop.create_proxy());
    let attrs = WindowAttributes::default()
        .with_title("岛仔")
        .with_inner_size(PhysicalSize::new(crate::app::CANVAS_W, crate::app::WIN_H))
        .with_decorations(false)
        .with_transparent(true)
        .with_active(false)
        .with_resizable(false)
        .with_no_redirection_bitmap(true);
    let mut app = DaozaiApp::new(attrs, prefer_dgpu);
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod root_scale_tests {
    use super::*;

    #[test]
    fn root_scale_only_touches_root_viewport() {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(960.0, 270.0),
        ));
        let root = &mut raw.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
        root.native_pixels_per_point = Some(1.0);
        root.inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960.0, 270.0)));
        let child = egui::ViewportId::from_hash_of("settings");
        raw.viewports.insert(child, egui::ViewportInfo {
            native_pixels_per_point: Some(1.0),
            ..Default::default()
        });
        raw.events.push(egui::Event::PointerMoved(egui::pos2(100.0, 50.0)));

        scale_root_input(&mut raw, 1.25, 1.0);

        assert_eq!(raw.screen_rect.unwrap().width(), 768.0);
        assert_eq!(raw.viewports[&egui::ViewportId::ROOT].native_pixels_per_point, Some(1.25));
        assert_eq!(raw.viewports[&child].native_pixels_per_point, Some(1.0));
        assert!(matches!(raw.events[0], egui::Event::PointerMoved(p) if p == egui::pos2(80.0, 40.0)));

        // egui-winit 每帧都用窗口原生 DPI 产生输入，所以第二帧同样要除一次 scale。
        let mut next = egui::RawInput::default();
        next.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960.0, 270.0)));
        next.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point = Some(1.0);
        next.events.push(egui::Event::PointerMoved(egui::pos2(100.0, 50.0)));
        scale_root_input(&mut next, 1.25, 1.0);
        assert_eq!(next.screen_rect.unwrap().width(), 768.0);
        assert_eq!(next.viewports[&egui::ViewportId::ROOT].native_pixels_per_point, Some(1.25));
        assert!(matches!(next.events[0], egui::Event::PointerMoved(p) if p == egui::pos2(80.0, 40.0)));
    }

    #[test]
    fn child_viewport_info_gets_outer_rect_and_monitor_size() {
        let (outer, monitor) = child_viewport_geometry(
            Some((300, 150)), (900, 675), Some((1920, 1080)), false, 1.5,
        );
        assert_eq!(outer, Some(egui::Rect::from_min_size(
            egui::pos2(200.0, 100.0), egui::vec2(600.0, 450.0))));
        assert_eq!(monitor, Some(egui::vec2(1280.0, 720.0)));

        // 最小化窗口 / 非法 DPI 不应该给出假的边框尺寸。
        assert_eq!(child_viewport_geometry(Some((0, 0)), (10, 10), Some((100, 100)), true, 1.0).0, None);
        assert_eq!(child_viewport_geometry(Some((0, 0)), (10, 10), Some((100, 100)), false, 0.0).1, None);
    }

    #[test]
    fn repaint_scheduling_keeps_the_earliest_deadline() {
        let mut app = DaozaiApp::new(WindowAttributes::default(), true);
        let base = Instant::now();
        let later = base + Duration::from_millis(100);
        let earlier = base + Duration::from_millis(20);
        app.schedule_repaint(later);
        app.schedule_repaint(earlier);
        assert_eq!(app.next_repaint, Some(earlier));
        app.schedule_repaint(base + Duration::from_secs(1));
        assert_eq!(app.next_repaint, Some(earlier));

        let now = Instant::now();
        app.repaint_now();
        let due = app.next_repaint.unwrap();
        assert!(due >= now && due <= Instant::now());
    }
}
