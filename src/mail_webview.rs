//! WebView2 内嵌渲染后端：独立窗口 + 内存供页，邮件 HTML 不落盘。
//!
//! 与 Edge `--app` 的差别：
//! - 页面字节通过 `WebResourceRequested` 从内存直接返回，磁盘上没有 `.html`；
//! - 导航被 `NavigationStarting` 拦下，外链交给系统浏览器，窗口内永远只有一个文档；
//! - `NewWindowRequested` 一律交给系统浏览器，不会弹出无地址栏的子窗口；
//! - 不依赖独立浏览器 profile，只有一个 WebView2 用户数据目录。
//!
//! 任何一步失败都会退回 [`crate::mail_html_view::EdgeAppRenderer`]，
//! 保证「查看完整排版」不会因为 WebView2 出问题而整体失效。

use crate::mail;
use crate::mail_html_view::{EdgeAppRenderer, Renderer};
use crate::mail_render::RenderDoc;
use crate::mail_view::WebTheme;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// 内存供页用的伪域名：不会被真实 DNS 解析，只由 `WebResourceRequested` 应答。
const ORIGIN: &str = "https://daozai-mail.invalid";

pub struct WebViewRenderer;

impl Renderer for WebViewRenderer {
    #[cfg(test)]
    fn name(&self) -> &'static str {
        "webview2"
    }

    fn available(&self) -> bool {
        runtime_available()
    }

    fn show(&self, item: &mail::Item, doc: &RenderDoc, theme: WebTheme) -> Result<(), String> {
        let html = doc.host.clone();
        let title = if item.subject.trim().is_empty() {
            "邮件".to_string()
        } else {
            item.subject.clone()
        };
        let item = item.clone();
        std::thread::Builder::new()
            .name("mail-webview2".into())
            .spawn(move || {
                // 整个启动 + 消息循环都在这个线程里，**绝不阻塞 UI 线程**：
                // 之前在主线程等 12 秒握手，窗口会被 Windows 标成「未响应」。
                // WebView2 的 COM 回调由这个线程的消息循环调起，回调里的 panic
                // 会一路 unwind 到线程入口，这里兜住即可，不再带走整个进程。
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    imp::run_window(&html, &title)
                }));
                if let Ok(Err(error)) = outcome {
                    // 起不来就退回 Edge，不让入口整体失效。
                    let edge = EdgeAppRenderer;
                    if edge.available() {
                        let doc = RenderDoc { host: html, blocked: 0 };
                        let _ = edge.show(&item, &doc, theme);
                    } else {
                        eprintln!("完整排版打开失败：{error}");
                    }
                } else if outcome.is_err() {
                    eprintln!("完整排版窗口异常退出，已隔离，主程序继续运行");
                }
            })
            .map_err(|e| format!("创建 WebView2 线程失败：{e}"))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 内嵌模式：把 WebView2 挂到岛仔自己的窗口上（阅读窗口正文区）
// ---------------------------------------------------------------------------

/// 内嵌状态只能有一份：同一时间只服务一个阅读窗口。
static EMBED: std::sync::Mutex<Option<EmbedState>> = std::sync::Mutex::new(None);

struct EmbedState {
    key: u64,
    rect: std::sync::Arc<std::sync::Mutex<[f32; 4]>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// 内嵌是实验特性，需要 `DAOZAI_MAIL_EMBED=1` 且本机有 WebView2 运行时。
pub fn embed_enabled() -> bool {
    cfg!(windows)
        && std::env::var("DAOZAI_MAIL_EMBED").ok().as_deref() == Some("1")
        && runtime_available()
}

/// 每帧调用：同一封邮件只挂一次，之后只更新矩形。
/// `rect` 是**窗口客户区坐标下的点（points）**，这里按 ppp 换算成物理像素。
pub fn embed_present(
    key: u64,
    title: &str,
    html: impl FnOnce() -> String,
    rect: [f32; 4],
    ppp: f32,
) {
    if !cfg!(windows) {
        return;
    }
    let physical = [
        rect[0] * ppp,
        rect[1] * ppp,
        (rect[2] * ppp).max(40.0),
        (rect[3] * ppp).max(40.0),
    ];
    let mut guard = match EMBED.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if let Some(state) = guard.as_ref() {
        if state.key == key {
            if let Ok(mut slot) = state.rect.lock() {
                *slot = physical;
            }
            return;
        }
        state.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let rect = std::sync::Arc::new(std::sync::Mutex::new(physical));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (html, title) = (html(), title.to_string());
    let (thread_rect, thread_stop) = (rect.clone(), stop.clone());
    #[cfg(windows)]
    {
        let spawned = std::thread::Builder::new()
            .name("mail-webview2-embed".into())
            .spawn(move || {
                // 回调里的 panic 只会 unwind 到这个线程入口，不带走主程序。
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    imp::run_embedded(&title, &html, thread_rect, thread_stop)
                }));
                if let Err(error) = outcome {
                    eprintln!("内嵌 WebView2 异常退出（已隔离）：{}", error.downcast_ref::<&str>().copied().unwrap_or("未知 panic"));
                }
            });
        if spawned.is_err() {
            return;
        }
    }
    *guard = Some(EmbedState { key, rect, stop });
}

/// 阅读窗口关闭时调用，让内嵌线程收掉控制器。
pub fn embed_close() {
    let mut guard = match EMBED.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if let Some(state) = guard.take() {
        state.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// WebView2 运行时（Evergreen Runtime）是否安装。
fn runtime_available() -> bool {
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        let Ok(base) = std::env::var(var) else { continue };
        let dir = PathBuf::from(base).join("Microsoft/EdgeWebView/Application");
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        if entries
            .flatten()
            .any(|entry| entry.path().join("msedgewebview2.exe").is_file())
        {
            return true;
        }
    }
    false
}

#[cfg(windows)]
mod imp {
    use super::*;
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{
        CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
        NavigationStartingEventHandler, NewWindowRequestedEventHandler,
        WebResourceRequestedEventHandler,
    };
    use windows::core::{w, HSTRING, Interface, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::WinRT::EventRegistrationToken;
    use windows::Win32::UI::Shell::{ShellExecuteW, SHCreateMemStream};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const CLASS: PCWSTR = w!("DaozaiMailWebView");

    pub(super) fn run_window(html: &str, title: &str) -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let result = build(html, title);
            CoUninitialize();
            result
        }
    }

    unsafe fn build(html: &str, title: &str) -> Result<(), String> {
        let hwnd = create_window(title)?;
        let env = create_environment()?;
        let controller = create_controller(&env, hwnd)?;
        let webview = controller
            .CoreWebView2()
            .map_err(|e| format!("获取 WebView 失败：{e}"))?;

        tune_settings(&webview);
        install_guards(&env, &webview, html.to_string());
        let _ = controller.SetIsVisible(BOOL(1));
        resize(&controller, hwnd);
        // 让 WM_SIZE 里的回调能找到控制器（run_window 栈上的引用在消息循环期间始终有效）。
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, controller.as_raw() as isize);

        let uri = HSTRING::from(format!("{ORIGIN}/mail"));
        webview.Navigate(&uri).map_err(|e| format!("加载邮件失败：{e}"))?;

        let _ = ShowWindow(hwnd, SW_SHOW);
        // 自检用：设了 DAOZAI_MAIL_WEBVIEW_AUTOCLOSE_MS 就定时关窗，便于自动化验证。
        if let Ok(ms) = std::env::var("DAOZAI_MAIL_WEBVIEW_AUTOCLOSE_MS") {
            if let Ok(ms) = ms.parse::<u64>() {
                // HWND 里的裸指针不是 Send，跨线程只传数值。
                let raw = hwnd.0 as isize;
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                    let target = HWND(raw as *mut std::ffi::c_void);
                    unsafe { let _ = PostMessageW(target, WM_CLOSE, WPARAM(0), LPARAM(0)); }
                });
            }
        }
        message_loop();
        let _ = controller.Close();
        let _ = DestroyWindow(hwnd);
        Ok(())
    }

    /// 内嵌模式：不建自己的窗口，把控制器挂到 `title` 对应的岛仔窗口上。
    pub(super) fn run_embedded(
        title: &str,
        html: &str,
        rect: std::sync::Arc<std::sync::Mutex<[f32; 4]>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let result = (|| {
                let parent = find_window(title)?;
                let env = create_environment()?;
                let controller = create_controller(&env, parent)?;
                let webview = controller
                    .CoreWebView2()
                    .map_err(|e| format!("获取 WebView 失败：{e}"))?;
                tune_settings(&webview);
                install_guards(&env, &webview, html.to_string());
                let _ = controller.SetIsVisible(BOOL(1));
                let mut last = [i32::MIN; 4];
                apply_rect(&controller, &rect, &mut last);
                let uri = HSTRING::from(format!("{ORIGIN}/mail"));
                webview.Navigate(&uri).map_err(|e| format!("加载邮件失败：{e}"))?;
                // 自己泵消息：控制器必须在创建它的线程上被调用，所以尺寸同步也走这里。
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    apply_rect(&controller, &rect, &mut last);
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                let _ = controller.Close();
                Ok(())
            })();
            CoUninitialize();
            result
        }
    }

    /// 按标题找本进程里的窗口：阅读窗口是 egui 的独立 viewport，
    /// eframe 不暴露它的 HWND，只能按标题枚举（同进程 + 标题唯一）。
    unsafe fn find_window(title: &str) -> Result<HWND, String> {
        use windows::Win32::System::Threading::GetCurrentProcessId;

        struct Ctx {
            pid: u32,
            want: Vec<u16>,
            found: Option<HWND>,
        }
        unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            let mut pid = 0u32;
            let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != ctx.pid {
                return BOOL(1);
            }
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut buf);
            if len > 0 && &buf[..len as usize] == ctx.want.as_slice() {
                ctx.found = Some(hwnd);
                return BOOL(0);
            }
            BOOL(1)
        }

        let mut ctx = Ctx {
            pid: GetCurrentProcessId(),
            want: title.encode_utf16().collect(),
            found: None,
        };
        // 窗口刚创建时标题还没设好，重试到 3 秒。
        for _ in 0..150 {
            let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
            if ctx.found.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        ctx.found
            .ok_or_else(|| format!("找不到阅读窗口「{title}」，内嵌取消"))
    }

    /// 尺寸没变就不调 SetBounds，避免每帧打扰渲染进程。
    fn apply_rect(
        controller: &ICoreWebView2Controller,
        rect: &std::sync::Arc<std::sync::Mutex<[f32; 4]>>,
        last: &mut [i32; 4],
    ) {
        let Ok(guard) = rect.lock() else { return };
        let next = [
            guard[0].round() as i32,
            guard[1].round() as i32,
            (guard[0] + guard[2]).round() as i32,
            (guard[1] + guard[3]).round() as i32,
        ];
        drop(guard);
        if next == *last {
            return;
        }
        *last = next;
        unsafe {
            let _ = controller.SetBounds(RECT {
                left: next[0],
                top: next[1],
                right: next[2],
                bottom: next[3],
            });
        }
    }

    unsafe fn create_window(title: &str) -> Result<HWND, String> {
        let instance = GetModuleHandleW(None).map_err(|e| format!("取模块句柄失败：{e}"))?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        // 已注册过不算错误：同一进程第二次开窗口会走到这里。
        let _ = RegisterClassW(&class);
        let title = HSTRING::from(title);
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            &title,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1080,
            820,
            None,
            None,
            HINSTANCE(instance.0),
            None,
        )
        .map_err(|e| format!("创建 WebView2 窗口失败：{e}"))
    }

    unsafe fn create_environment() -> Result<ICoreWebView2Environment, String> {
        let data = crate::model::config_dir().join("mail-webview2");
        std::fs::create_dir_all(&data).map_err(|e| format!("创建 WebView2 数据目录失败：{e}"))?;
        // 回调与等待方在同一个 STA 线程，槽位不需要原子操作。
        let slot: Rc<RefCell<Option<ICoreWebView2Environment>>> = Rc::new(RefCell::new(None));
        let sink = slot.clone();
        let folder = HSTRING::from(data.to_string_lossy().as_ref());
        CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| {
                CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &folder, None, &handler)
                    .map_err(|e| e.into())
            }),
            Box::new(move |ok, env| {
                if ok.is_ok() {
                    *sink.borrow_mut() = env;
                }
                Ok(())
            }),
        )
        .map_err(|e| format!("WebView2 环境初始化失败：{e}"))?;
        let out = slot.borrow_mut().take();
        out.ok_or_else(|| "WebView2 运行时不可用".to_string())
    }

    unsafe fn create_controller(
        env: &ICoreWebView2Environment,
        hwnd: HWND,
    ) -> Result<ICoreWebView2Controller, String> {
        let slot: Rc<RefCell<Option<ICoreWebView2Controller>>> = Rc::new(RefCell::new(None));
        let sink = slot.clone();
        let env = env.clone();
        CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| {
                env.CreateCoreWebView2Controller(hwnd, &handler).map_err(|e| e.into())
            }),
            Box::new(move |ok, controller| {
                if ok.is_ok() {
                    *sink.borrow_mut() = controller;
                }
                Ok(())
            }),
        )
        .map_err(|e| format!("创建 WebView2 控制器失败：{e}"))?;
        let out = slot.borrow_mut().take();
        out.ok_or_else(|| "WebView2 控制器不可用".to_string())
    }

    unsafe fn tune_settings(webview: &ICoreWebView2) {
        let Ok(settings) = webview.Settings() else { return };
        // 邮件里的脚本必须彻底关闭，否则 CSP 被绕过就前功尽弃。
        let _ = settings.SetIsScriptEnabled(BOOL(0));
        let _ = settings.SetAreDefaultContextMenusEnabled(BOOL(0));
        let _ = settings.SetAreDevToolsEnabled(BOOL(0));
        let _ = settings.SetIsStatusBarEnabled(BOOL(0));
        let _ = settings.SetIsZoomControlEnabled(BOOL(1));
    }

    /// 三个闸门：内存供页、导航拦截、新窗口外抛。
    unsafe fn install_guards(
        env: &ICoreWebView2Environment,
        webview: &ICoreWebView2,
        html: String,
    ) {
        // ① 内存供页：命中伪域名的请求由我们直接生成响应，磁盘上没有文件。
        let env = env.clone();
        let body = std::sync::Arc::new(html.into_bytes());
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let bytes = body.clone();
            let Some(stream) = SHCreateMemStream(Some(bytes.as_slice())) else {
                return Ok(());
            };
            if let Ok(response) = env.CreateWebResourceResponse(
                &stream,
                200,
                w!("OK"),
                w!("Content-Type: text/html; charset=utf-8\r\nCache-Control: no-store"),
            ) {
                let _ = args.SetResponse(&response);
            }
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        let _ = webview.AddWebResourceRequestedFilter(
            &HSTRING::from(format!("{ORIGIN}/")),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        );
        let _ = webview.add_WebResourceRequested(&handler, &mut token);

        // ② 导航拦截：只允许我们自己的伪域名，其它一律交给系统浏览器。
        let handler = NavigationStartingEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            let uri = take_uri(|out| args.Uri(out));
            if !uri.is_empty() && !uri.starts_with(ORIGIN) {
                let _ = args.SetCancel(BOOL(1));
                open_external(&uri);
            }
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        let _ = webview.add_NavigationStarting(&handler, &mut token);

        // ③ target=_blank / window.open 一律外抛，窗口内不开第二个文档。
        let handler = NewWindowRequestedEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            let uri = take_uri(|out| args.Uri(out));
            let _ = args.SetHandled(BOOL(1));
            open_external(&uri);
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        let _ = webview.add_NewWindowRequested(&handler, &mut token);
    }

    fn take_uri(mut get: impl FnMut(*mut PWSTR) -> windows::core::Result<()>) -> String {
        let mut raw = PWSTR::null();
        if get(&mut raw).is_err() || raw.is_null() {
            return String::new();
        }
        let text = unsafe { raw.to_string() }.unwrap_or_default();
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(raw.0 as *const _)) };
        text
    }

    fn open_external(uri: &str) {
        if !(uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("mailto:")) {
            return;
        }
        let target = HSTRING::from(uri);
        unsafe {
            // 外链给系统浏览器：WebView2 窗口没有地址栏，留在里面用户无法确认自己去了哪。
            let _ = ShellExecuteW(
                None,
                w!("open"),
                &target,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }

    unsafe fn resize(controller: &ICoreWebView2Controller, hwnd: HWND) {
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let _ = controller.SetBounds(RECT {
            left: 0,
            top: 0,
            right: rect.right,
            bottom: rect.bottom,
        });
    }

    unsafe fn message_loop() {
        let mut msg = MSG::default();
        loop {
            let got = GetMessageW(&mut msg, None, 0, 0);
            if got.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_SIZE => {
                let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const std::ffi::c_void;
                if !raw.is_null() {
                    let controller: &ICoreWebView2Controller =
                        &*(raw as *const ICoreWebView2Controller);
                    resize(controller, hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn run_window(_html: &str, _title: &str) -> Result<(), String> {
        Err("WebView2 仅支持 Windows".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_name_and_availability_are_consistent() {
        let renderer = WebViewRenderer;
        assert_eq!(renderer.name(), "webview2");
        // 没有运行时就报告不可用，而不是 panic。
        assert_eq!(renderer.available(), runtime_available());
    }

    fn probe_item() -> mail::Item {
        mail::Item {
            account: "self@example.test".into(),
            from: "自检".into(),
            subject: "WebView2 自检".into(),
            time: String::new(),
            received_unix: None,
            code: None,
            html: String::new().into(),
            images: Default::default(),
            message_id: String::new(),
            from_addr: "self@example.test".into(),
            summary: String::new().into(),
            links: vec![],
            meeting: None,
            uid: 1,
        }
    }

    /// 真的会弹一个窗口出来，所以默认 ignore：
    /// `cargo test --lib mail_webview -- --ignored --nocapture`
    #[test]
    #[ignore = "会弹出真实窗口，需手动触发"]
    fn opens_a_real_webview_window() {
        if !runtime_available() {
            eprintln!("跳过：本机没有 WebView2 运行时");
            return;
        }
        std::env::set_var("DAOZAI_MAIL_WEBVIEW_AUTOCLOSE_MS", "2500");
        let images = Default::default();
        let request = crate::mail_render::RenderRequest {
            html: "<h1>岛仔 WebView2 自检</h1><p>看到这段文字，说明内存供页与沙箱 iframe 都通了。</p>",
            images: &images,
            allow_remote: false,
            theme: crate::mail_view::WebTheme::Paper,
            meta: "自检 · 内存供页".into(),
            title: "WebView2 自检".into(),
        };
        let doc = crate::mail_render::build(&request);
        // 直接跑窗口函数：`show()` 现在把工作丢进后台线程，观测不到启动失败。
        imp::run_window(&doc.host, "WebView2 自检").expect("WebView2 窗口应当能弹出");
        let _ = probe_item();
    }
}
