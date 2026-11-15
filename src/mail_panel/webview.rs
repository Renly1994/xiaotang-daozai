//! WebView2 宿主线程：把面板页面挂在岛仔主窗口上，双向收发 JSON。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::WinRT::EventRegistrationToken;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

const PAGE: &str = include_str!("page.html");

pub(super) fn run_panel(
    rect: super::Rect,
    stop: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    outbox: mpsc::Receiver<String>,
    inbox: mpsc::Sender<String>,
) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = (|| {
            let parent = find_window("岛仔")?;
            let env = create_environment()?;
            let controller = create_controller(&env, parent)?;
            let webview = controller
                .CoreWebView2()
                .map_err(|e| format!("获取 WebView 失败：{e}"))?;
            tune(&webview);
            listen(&webview, inbox.clone())?;
            guard_navigation(&webview, inbox)?;
            let _ = controller.SetIsVisible(BOOL(1));
            webview
                .NavigateToString(&HSTRING::from(PAGE))
                .map_err(|e| format!("加载面板失败：{e}"))?;

            let mut last = [i32::MIN; 4];
            let mut zoom = f32::NAN;
            let mut shown = true;
            while !stop.load(Ordering::Relaxed) {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                // 折叠/切页时先隐藏，避免动画期间残影；再展开时同步尺寸。
                let want = visible.load(Ordering::Relaxed);
                if want != shown {
                    shown = want;
                    let _ = controller.SetIsVisible(BOOL(if want { 1 } else { 0 }));
                }
                if want {
                    resize(&controller, &rect, &mut last);
                    sync_zoom(&controller, parent, &rect, &mut zoom);
                }
                while let Ok(json) = outbox.try_recv() {
                    let _ = webview.PostWebMessageAsJson(&HSTRING::from(json.as_str()));
                }
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            let _ = controller.Close();
            Ok(())
        })();
        CoUninitialize();
        result
    }
}

unsafe fn listen(webview: &ICoreWebView2, inbox: mpsc::Sender<String>) -> Result<(), String> {
    let handler = WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
        if let Some(args) = args {
            // take_uri 内部会 CoTaskMemFree：COM 返回的字符串不释放就是每条消息漏一份。
            let text = take_uri(|out| args.TryGetWebMessageAsString(out));
            if !text.is_empty() {
                let _ = inbox.send(text);
            }
        }
        Ok(())
    }));
    let mut token = EventRegistrationToken::default();
    webview
        .add_WebMessageReceived(&handler, &mut token)
        .map_err(|e| format!("注册面板消息失败：{e}"))?;
    Ok(())
}

/// 两层导航拦截：顶层 + `target=_blank`。
///
/// 正文跑在沙箱 iframe 里（CSP `script-src 'none'`），里面注入不了任何脚本，
/// 所以 mail_render 给正文加了 `<base target="_blank">`：正文里每次点链接都是
/// 一次「新窗口请求」，这里拦下来开系统浏览器，正文自己不会被导航掉。
/// 两层截到的 URI 都包成前端命令 JSON 回传，走和页面完全相同的 `Command::parse`
/// 白名单，不存在绕过路径。
unsafe fn guard_navigation(webview: &ICoreWebView2, inbox: mpsc::Sender<String>) -> Result<(), String> {
    // ① 顶层导航：面板自己不许跳出（错误页、拖入文件、脚本跳转都走这里）。
    let sink = inbox.clone();
    let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else { return Ok(()) };
        let uri = take_uri(|out| args.Uri(out));
        if allowed_inside(&uri) {
            return Ok(());
        }
        let _ = args.SetCancel(BOOL(1));
        let _ = sink.send(super::bridge::link_command(&uri));
        Ok(())
    }));
    let mut token = EventRegistrationToken::default();
    webview
        .add_NavigationStarting(&handler, &mut token)
        .map_err(|e| format!("注册顶层导航拦截失败：{e}"))?;

    // ② target=_blank / window.open 一律外抛，面板内不开第二个文档。
    let handler = NewWindowRequestedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else { return Ok(()) };
        let uri = take_uri(|out| args.Uri(out));
        let _ = args.SetHandled(BOOL(1));
        if !uri.is_empty() {
            let _ = inbox.send(super::bridge::link_command(&uri));
        }
        Ok(())
    }));
    let mut token = EventRegistrationToken::default();
    webview
        .add_NewWindowRequested(&handler, &mut token)
        .map_err(|e| format!("注册新窗口拦截失败：{e}"))?;
    Ok(())
}

/// 面板自己产生的导航（初次 `NavigateToString`、`srcdoc` 装载）放行。
fn allowed_inside(uri: &str) -> bool {
    uri.is_empty()
        || uri.starts_with("about:")
        || uri.starts_with("data:text/html")
        || uri.starts_with("blob:")
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

unsafe fn tune(webview: &ICoreWebView2) {
    let Ok(settings) = webview.Settings() else { return };
    let _ = settings.SetAreDefaultContextMenusEnabled(BOOL(0));
    let _ = settings.SetAreDevToolsEnabled(BOOL(0));
    let _ = settings.SetIsStatusBarEnabled(BOOL(0));
    let _ = settings.SetIsZoomControlEnabled(BOOL(0));
}

fn resize(controller: &ICoreWebView2Controller, rect: &super::Rect, last: &mut [i32; 4]) {
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

/// egui 的 pixels_per_point 和窗口 DPI 可能不一致（多屏拖动、用户改了缩放），
/// 差多少就补多少 zoom，保证 HTML 里的 px 和岛内其它界面同一个视觉尺寸。
fn sync_zoom(controller: &ICoreWebView2Controller, hwnd: HWND, rect: &super::Rect, last: &mut f32) {
    let ppp = rect.lock().ok().map(|guard| guard[4]).unwrap_or(1.0);
    let dpi = unsafe { GetDpiForWindow(hwnd) } as f32;
    if dpi <= 0.0 {
        return;
    }
    let target = (ppp / (dpi / 96.0)).clamp(0.5, 2.5);
    if last.is_finite() && (target - *last).abs() < 0.01 {
        return;
    }
    *last = target;
    unsafe {
        let _ = controller.SetZoomFactor(target as f64);
    }
}

/// 按标题找本进程的窗口：eframe 不暴露 HWND，只能枚举。
unsafe fn find_window(title: &str) -> Result<HWND, String> {
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
    let mut ctx = Ctx { pid: GetCurrentProcessId(), want: title.encode_utf16().collect(), found: None };
    for _ in 0..150 {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
        if ctx.found.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    ctx.found.ok_or_else(|| format!("找不到窗口「{title}」"))
}

unsafe fn create_environment() -> Result<ICoreWebView2Environment, String> {
    let data = crate::model::config_dir().join("mail-webview2");
    std::fs::create_dir_all(&data).map_err(|e| format!("创建数据目录失败：{e}"))?;
    // COM 回调在同一个 STA 线程里同步执行，槽位不需要原子操作；
    // 用 Rc<RefCell> 表达「只在本线程共享」，也不会被误当成可跨线程的值。
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
    .map_err(|e| format!("环境初始化失败：{e}"))?;
    let out=slot.borrow_mut().take();
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
    .map_err(|e| format!("控制器创建失败：{e}"))?;
    let out=slot.borrow_mut().take();
    out.ok_or_else(|| "WebView2 控制器不可用".to_string())
}
