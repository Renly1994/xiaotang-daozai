//! 只接收微信任务栏提醒；复用主窗口消息循环，不读取聊天内容。
use std::{cell::RefCell, path::PathBuf, time::{Duration, Instant}};
use windows::{core::{w, PCWSTR, PWSTR}, Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM},
    System::{Registry::{RegGetValueW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, RRF_RT_REG_SZ},
        Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION}},
    UI::{Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass}, WindowsAndMessaging::*},
}};

const SUBCLASS_ID: usize = 0x5745_4348;
const FLASH: usize = 0x8006;

#[derive(Default)]
struct Burst { last: Option<Instant> }
impl Burst {
    fn attention(&mut self, now: Instant) -> bool {
        // 连续闪动只提醒一次；停止闪动五秒后，下一轮才算新提醒。
        self.last.replace(now).is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5))
    }
}

fn normalized(path: &str) -> String {
    path.trim_start_matches(r"\\?\").replace('/', "\\").to_lowercase()
}

fn registry_string(root:HKEY,key:PCWSTR,name:PCWSTR)->Option<String>{
    let mut data=[0u16;2048];let mut size=(data.len()*2) as u32;
    unsafe {RegGetValueW(root,key,name,RRF_RT_REG_SZ,None,Some(data.as_mut_ptr().cast()),Some(&mut size))}.ok().ok()?;
    let length=data.iter().position(|c|*c==0).unwrap_or(data.len());
    Some(String::from_utf16_lossy(&data[..length]))
}

// 只提取协议处理程序的路径，绝不执行注册表里的命令或参数。
fn protocol_executable(command:&str)->Option<PathBuf>{
    let command=command.trim();
    let exe=if let Some(quoted)=command.strip_prefix('"'){quoted.split_once('"')?.0}else{
        command.strip_suffix(" \"%1\"").or_else(||command.strip_suffix(" %1")).unwrap_or(command)
    };
    let path=PathBuf::from(exe);
    let name=path.file_name()?.to_str()?;
    (path.is_absolute() && (name.eq_ignore_ascii_case("Weixin.exe") || name.eq_ignore_ascii_case("WeChat.exe"))).then_some(path)
}

fn installed_paths() -> Vec<String> {
    let mut paths=Vec::new();
    for (key,exe) in [(w!("Software\\Tencent\\Weixin"),"Weixin.exe"),(w!("Software\\Tencent\\WeChat"),"WeChat.exe")] {
        if let Some(folder)=registry_string(HKEY_CURRENT_USER,key,w!("InstallPath")){paths.push(PathBuf::from(folder).join(exe));}
    }
    if let Some(path)=registry_string(HKEY_CLASSES_ROOT,w!("weixin\\shell\\open\\command"),PCWSTR::null()).and_then(|cmd|protocol_executable(&cmd)){paths.push(path);}
    paths.into_iter().filter(|p|p.is_absolute() && p.is_file()).filter_map(|p|p.canonicalize().ok()).map(|p|normalized(&p.to_string_lossy())).collect()
}

unsafe fn matches(hwnd: HWND, paths: &[String]) -> bool {
    if hwnd.0.is_null(){return false;}
    let mut pid=0;GetWindowThreadProcessId(hwnd,Some(&mut pid));
    let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid) else{return false};
    let mut path=[0u16;32768];let mut length=path.len() as u32;
    let result=QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,PWSTR(path.as_mut_ptr()),&mut length);
    let _=CloseHandle(process);
    result.is_ok() && paths.contains(&normalized(&String::from_utf16_lossy(&path[..length as usize])))
}

fn client_paths(source:crate::app::Messenger)->Vec<String>{
    if source==crate::app::Messenger::Wechat{return installed_paths();}
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let mut candidates=Vec::new();
    for root in [HKEY_CURRENT_USER,HKEY_LOCAL_MACHINE]{
        for key in [w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\QQ"),w!("Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\QQ")]{
            if let Some(icon)=registry_string(root,key,w!("DisplayIcon")){
                let exe=icon.rsplit_once(',').filter(|(_,index)|index.trim().parse::<i32>().is_ok()).map_or(icon.as_str(),|(path,_)|path);
                candidates.push(PathBuf::from(exe.trim().trim_matches('"')));
            }
        }
    }
    // QQ NT 的协议处理器在版本子目录中，主程序在安装根目录。
    if let Some(command)=registry_string(HKEY_CLASSES_ROOT,w!("tencent\\shell\\open\\command"),PCWSTR::null()){
        let helper=command.trim().trim_start_matches('"').split('"').next().unwrap_or("");
        let helper=std::path::Path::new(helper);
        if helper.is_absolute() && helper.file_name().is_some_and(|n|n.eq_ignore_ascii_case("timwp.exe")){
            if let Some(versions)=helper.ancestors().find(|p|p.file_name().is_some_and(|n|n.eq_ignore_ascii_case("versions"))){
                if let Some(root)=versions.parent(){candidates.push(root.join("QQ.exe"));}
            }
        }
    }
    candidates.into_iter().filter(|p|p.is_absolute() && p.file_name().is_some_and(|n|n.eq_ignore_ascii_case("QQ.exe")) && p.is_file())
        .filter_map(|p|p.canonicalize().ok()).map(|p|normalized(&p.to_string_lossy())).collect()
}

fn client_main_frame(source:crate::app::Messenger,class:&str,style:u32,extended:u32)->bool{
    let known=match source {
        crate::app::Messenger::Qq=>matches!(class,"Chrome_WidgetWin_1"|"TXGuiFoundation"),
        crate::app::Messenger::Wechat=>class=="WeChatMainWndForPC" || class.starts_with("Qt") && class.ends_with("QWindowIcon"),
    };
    let frame=(WS_SYSMENU|WS_MINIMIZEBOX).0;
    let resizable_frame=(WS_CAPTION|WS_THICKFRAME|WS_MINIMIZEBOX).0;
    let custom_frame=class.starts_with("Qt") || class=="Chrome_WidgetWin_1";
    known && (style & frame==frame || custom_frame && style & resizable_frame==resizable_frame) && extended & (WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE).0==0
}

fn window_pid(hwnd:HWND)->u32{
    let mut pid=0;
    if !hwnd.0.is_null(){unsafe{GetWindowThreadProcessId(hwnd,Some(&mut pid));}}
    pid
}

/// 前台是该客户端的主窗口时返回它所在的进程（多开时每个账号一个进程）。
unsafe fn foreground_main_pid(source:crate::app::Messenger,paths:&[String])->Option<u32>{
    let hwnd=GetForegroundWindow();
    if IsIconic(hwnd).as_bool() || !visible_main_frame(hwnd,source) || !matches(hwnd,paths){return None;}
    Some(window_pid(hwnd))
}

unsafe fn visible_main_frame(hwnd:HWND,source:crate::app::Messenger)->bool{
    if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd,GW_OWNER).is_ok(){return false;}
    let mut class=[0u16;128];
    let n=GetClassNameW(hwnd,&mut class).max(0) as usize;
    client_main_frame(source,&String::from_utf16_lossy(&class[..n]),GetWindowLongW(hwnd,GWL_STYLE) as u32,GetWindowLongW(hwnd,GWL_EXSTYLE) as u32)
}

pub fn client_in_foreground(source:crate::app::Messenger)->bool{
    unsafe{foreground_main_pid(source,&client_paths(source)).is_some()}
}

/// 当前前台的微信 / QQ 主窗口：来源与账号进程。
pub fn foreground_client(sources:&[crate::app::Messenger])->Option<(crate::app::Messenger,u32)>{
    sources.iter().find_map(|&source|unsafe{foreground_main_pid(source,&client_paths(source))}.map(|pid|(source,pid)))
}

/// 跳到某条未读对应的窗口。
pub fn open_unread(unread:crate::app::Unread)->Result<(),String>{
    open_unread_with_paths(unread,&client_paths(unread.source))
}

fn open_unread_with_paths(unread:crate::app::Unread,paths:&[String])->Result<(),String>{
    let window=HWND(unread.window as *mut _);
    if unread.source==crate::app::Messenger::Wechat {
        // 微信 4 闪动的句柄常是隐藏的 Qt 辅助窗口：从外部显示它会出现白屏、拖死微信。
        // 只切换到该账号进程里已可见的主窗口；绝不显示隐藏窗口，也不另起 Weixin.exe。
        unsafe {
            let target=if unread.pid==0 {None}
                else if window_pid(window)==unread.pid && visible_main_frame(window,unread.source) && matches(window,paths) {Some(window)}
                else {visible_main_window_of(unread.pid,unread.source,paths)};
            if let Some(target)=target {
                // 异步恢复：微信卡住时也不能拖住岛仔的界面线程。
                if IsIconic(target).as_bool(){ShowWindowAsync(target,SW_RESTORE).ok().map_err(|_|"请从任务栏打开微信".to_string())?;}
                if SetForegroundWindow(target).as_bool(){return Ok(());}
            }
        }
        return Err("请从任务栏或托盘打开微信".into());
    }
    unsafe{
        if unread.window!=0 && IsWindow(window).as_bool() && window_pid(window)==unread.pid && matches(window,paths){
            if IsIconic(window).as_bool(){let _=ShowWindowAsync(window,SW_RESTORE);}
            else if !IsWindowVisible(window).as_bool(){let _=ShowWindowAsync(window,SW_SHOW);}
            if SetForegroundWindow(window).as_bool(){return Ok(());}
        }
    }
    open_client(unread.source)
}

/// 该账号缩在托盘：没有可见主窗口，但有自己的托盘图标。
pub fn wechat_in_tray(unread:crate::app::Unread)->bool{
    if unread.source!=crate::app::Messenger::Wechat || unread.pid==0 {return false;}
    let paths=client_paths(unread.source);
    unsafe{visible_main_window_of(unread.pid,unread.source,&paths).is_none() && tray_icon(unread.pid,&paths).is_some()}
}

/// 该账号进程自己的托盘消息窗口与唯一的图标编号。
unsafe fn tray_icon(pid:u32,paths:&[String])->Option<(HWND,u32)>{
    use windows::Win32::Foundation::BOOL;
    struct Find<'a>{pid:u32,paths:&'a [String],hosts:Vec<HWND>}
    unsafe extern "system" fn visit(hwnd:HWND,data:LPARAM)->BOOL{
        let find=&mut *(data.0 as *mut Find);
        if window_pid(hwnd)==find.pid {
            let mut class=[0u16;128];
            let n=GetClassNameW(hwnd,&mut class).max(0) as usize;
            if String::from_utf16_lossy(&class[..n]).ends_with("TrayIconMessageWindowClass") && matches(hwnd,find.paths){find.hosts.push(hwnd);}
        }
        true.into()
    }
    let mut find=Find{pid,paths,hosts:Vec::new()};
    let _=EnumWindows(Some(visit),LPARAM((&mut find as *mut Find) as isize));
    let [host]=find.hosts.as_slice() else {return None};
    let mut found=None;
    for id in 0..=256 {
        if tray_rect(*host,id).is_some(){ if found.is_some(){return None;} found=Some(id); }
    }
    found.map(|id|(*host,id))
}

unsafe fn tray_rect(host:HWND,id:u32)->Option<windows::Win32::Foundation::RECT>{
    use windows::Win32::UI::Shell::{Shell_NotifyIconGetRect,NOTIFYICONIDENTIFIER};
    let icon=NOTIFYICONIDENTIFIER{cbSize:std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,hWnd:host,uID:id,..Default::default()};
    Shell_NotifyIconGetRect(&icon).ok()
}

fn rect_center_inside(inner:windows::Win32::Foundation::RECT,outer:windows::Win32::Foundation::RECT)->bool{
    let (x,y)=((inner.left+inner.right)/2,(inner.top+inner.bottom)/2);
    outer.right>outer.left && outer.bottom>outer.top && x>=outer.left && x<outer.right && y>=outer.top && y<outer.bottom
}

/// 微信缩在托盘时：通过无障碍接口按下该账号自己的托盘按钮，由微信自己唤起主窗口。
/// 不移动鼠标、不向微信发送内部消息、不注入、不读取内容；定位不到唯一按钮就放弃。
/// 会阻塞几百毫秒，必须在后台线程调用。
pub fn open_wechat_from_tray(unread:crate::app::Unread)->Result<(),String>{
    use windows::{core::Interface,Win32::{System::Com::*,UI::{Accessibility::*,HiDpi::*}}};
    const FAIL:&str="未能定位微信托盘图标，请从托盘打开微信";
    if unread.source!=crate::app::Messenger::Wechat || unread.pid==0 {return Err(FAIL.into());}
    let paths=client_paths(unread.source);
    unsafe {
        let _=SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _=CoInitializeEx(None,COINIT_MULTITHREADED);
        let (host,id)=tray_icon(unread.pid,&paths).ok_or(FAIL)?;
        let ui:IUIAutomation=CoCreateInstance(&CUIAutomation8,None,CLSCTX_INPROC_SERVER).map_err(|_|FAIL)?;
        if let Ok(timed)=ui.cast::<IUIAutomation2>(){let _=timed.SetConnectionTimeout(800);let _=timed.SetTransactionTimeout(800);}
        let all=ui.CreateTrueCondition().map_err(|_|FAIL)?;
        // 在某个窗口里找覆盖该图标中心、指定编号的托盘按钮。
        let button_at=|window:HWND,icon:windows::Win32::Foundation::RECT,automation_id:&str|->Option<IUIAutomationElement>{
            let root=ui.ElementFromHandle(window).ok()?;
            let items=root.FindAll(TreeScope_Descendants,&all).ok()?;
            let mut hit=None;
            for i in 0..items.Length().ok()? {
                let item=items.GetElement(i).ok()?;
                if item.CurrentAutomationId().ok()?.to_string()!=automation_id {continue;}
                if rect_center_inside(icon,item.CurrentBoundingRectangle().ok()?){ if hit.is_some(){return None;} hit=Some(item); }
            }
            hit
        };
        let invoke=|element:&IUIAutomationElement|->bool{
            element.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId).and_then(|p|p.Invoke()).is_ok()
        };
        let taskbar=FindWindowW(w!("Shell_TrayWnd"),PCWSTR::null()).map_err(|_|FAIL)?;
        let icon=tray_rect(host,id).ok_or(FAIL)?;
        // 图标固定在任务栏上：直接按下。
        if let Some(button)=button_at(taskbar,icon,"NotifyItemIcon"){
            return if invoke(&button){Ok(())}else{Err(FAIL.into())};
        }
        // 图标收在折叠区：此时系统报告的位置就是「显示隐藏的图标」按钮。
        let chevron=button_at(taskbar,icon,"SystemTrayIcon").ok_or(FAIL)?;
        if !invoke(&chevron){return Err(FAIL.into());}
        let overflow_open=||FindWindowW(w!("TopLevelWindowForOverflowXamlIsland"),PCWSTR::null()).ok().filter(|h|IsWindowVisible(*h).as_bool());
        let started=Instant::now();
        while started.elapsed()<Duration::from_millis(1500) {
            std::thread::sleep(Duration::from_millis(80));
            let Some(overflow)=overflow_open() else {continue};
            // 折叠区展开后，系统报告的才是图标在折叠区里的真实位置；多开时据此区分账号。
            let Some(inside)=tray_rect(host,id).filter(|r|!rect_center_inside(*r,icon)) else {continue};
            if let Some(button)=button_at(overflow,inside,"NotifyItemIcon"){
                if invoke(&button){return Ok(());}
                break;
            }
        }
        // 没按成：把展开的折叠区收回去，不留在屏幕上。
        if overflow_open().is_some(){let _=invoke(&chevron);}
        Err(FAIL.into())
    }
}

/// 某个账号进程里已可见的主窗口（多开时每个进程一个）。
unsafe fn visible_main_window_of(pid:u32,source:crate::app::Messenger,paths:&[String])->Option<HWND>{
    use windows::Win32::Foundation::BOOL;
    struct Find<'a>{pid:u32,source:crate::app::Messenger,paths:&'a [String],found:Option<HWND>}
    unsafe extern "system" fn visit(hwnd:HWND,data:LPARAM)->BOOL{
        let find=&mut *(data.0 as *mut Find);
        if window_pid(hwnd)==find.pid && visible_main_frame(hwnd,find.source) && matches(hwnd,find.paths){find.found=Some(hwnd);return false.into();}
        true.into()
    }
    let mut find=Find{pid,source,paths,found:None};
    let _=EnumWindows(Some(visit),LPARAM((&mut find as *mut Find) as isize));
    find.found
}

pub fn open_client(source:crate::app::Messenger)->Result<(),String>{
    use windows::Win32::Foundation::{BOOL,RECT};
    struct Search {paths:Vec<String>,best:Option<(i64,HWND)>,tray:Vec<HWND>,qq:bool}
    unsafe extern "system" fn visit(hwnd:HWND,data:LPARAM)->BOOL{
        let search=&mut *(data.0 as *mut Search);
        if search.qq{
            let mut class=[0u16;64];
            let count=GetClassNameW(hwnd,&mut class).max(0) as usize;
            let class=String::from_utf16_lossy(&class[..count]);
            if class=="Electron_NotifyIconHostWindow" && matches(hwnd,&search.paths){search.tray.push(hwnd);}
            if !matches!(class.as_str(),"Chrome_WidgetWin_1"|"TXGuiFoundation"){return true.into();}
        }
        if IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd,GW_OWNER).is_err() && matches(hwnd,&search.paths){
            let mut r=RECT::default();
            if GetWindowRect(hwnd,&mut r).is_ok(){
                let area=i64::from(r.right-r.left)*i64::from(r.bottom-r.top);
                if area>0 && search.best.is_none_or(|(old,_)|area>old){search.best=Some((area,hwnd));}
            }
        }
        true.into()
    }
    let mut search=Search{paths:client_paths(source),best:None,tray:Vec::new(),qq:source==crate::app::Messenger::Qq};
    let exe=search.paths.first().ok_or_else(||format!("未找到{}的安装路径",source.name()))?.clone();
    unsafe{
        let _=EnumWindows(Some(visit),LPARAM((&mut search as *mut Search) as isize));
        if let Some((_,window))=search.best{
            if IsIconic(window).as_bool(){let _=ShowWindowAsync(window,SW_RESTORE);}
            if SetForegroundWindow(window).as_bool(){return Ok(());}
        }
        if let [host]=search.tray.as_slice(){
            use windows::Win32::UI::Shell::{Shell_NotifyIconGetRect,NOTIFYICONIDENTIFIER};
            let mut found=None;
            for id in 2..=256 {
                let icon=NOTIFYICONIDENTIFIER{cbSize:std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,hWnd:*host,uID:id,..Default::default()};
                if Shell_NotifyIconGetRect(&icon).is_ok(){
                    if found.is_some(){found=None;break;} found=Some(id);
                }
            }
            if let Some(id)=found {
                for event in [WM_LBUTTONDOWN,WM_LBUTTONUP,WM_LBUTTONDBLCLK,WM_LBUTTONUP]{
                    if !PostMessageW(*host,WM_APP+1,WPARAM(id as usize),LPARAM(event as isize)).is_ok(){return Err("QQ 托盘暂时无法响应，请重试".into());}
                }
                return Ok(());
            }
        }
    }
    std::process::Command::new(exe).spawn().map(|_|()).map_err(|e|format!("无法打开{}：{e}",source.name()))
}

struct State {
    message: u32,
    taskbar_created: u32,
    registered: bool,
    paths: Vec<String>,
    /// 每个微信进程（账号）各自合并连续闪动。
    burst: Vec<(u32,Burst)>,
    /// 尚未取走的提醒：账号进程与闪动的窗口。
    pending: Vec<(u32,isize)>,
    ctx: eframe::egui::Context,
}

unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM, _: usize, data: usize) -> LRESULT {
    {
        let slot=&*(data as *const RefCell<State>);
        if let Ok(mut state)=slot.try_borrow_mut(){
            if message==state.taskbar_created && message!=0 {
                state.registered=RegisterShellHookWindow(hwnd).as_bool();
                state.ctx.request_repaint();
            }
            if message==state.message && matches!(wp.0, FLASH | 4 | 0x8004) {
                let target=HWND(lp.0 as *mut _);
                if matches(target,&state.paths){
                    let pid=window_pid(target);
                    if foreground_main_pid(crate::app::Messenger::Wechat,&state.paths)==Some(pid){
                        // 正在看这个账号：它的提醒作废，其它账号不受影响。
                        state.burst.retain(|(p,_)|*p!=pid);state.pending.retain(|(p,_)|*p!=pid);
                    }else if wp.0==FLASH {
                        let fresh={
                            if !state.burst.iter().any(|(p,_)|*p==pid){state.burst.push((pid,Burst::default()));}
                            state.burst.iter_mut().find(|(p,_)|*p==pid).is_some_and(|(_,burst)|burst.attention(Instant::now()))
                        };
                        if fresh {
                            match state.pending.iter_mut().find(|(p,_)|*p==pid){Some(entry)=>entry.1=target.0 as isize,None=>state.pending.push((pid,target.0 as isize))}
                            state.ctx.request_repaint();
                        }
                    }
                }
            }
        }
    }
    if message==WM_NCDESTROY {
        let _=DeregisterShellHookWindow(hwnd);
        let _=RemoveWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID);
    }
    DefSubclassProc(hwnd,message,wp,lp)
}

pub struct Listener { hwnd: HWND, state: Box<RefCell<State>> }
impl Listener {
    pub fn new(ctx: eframe::egui::Context) -> Result<Self,String> {
        let paths=installed_paths();
        if paths.is_empty(){return Err("未找到微信安装路径，请安装微信后重新开启提醒".into());}
        unsafe {
            let message=RegisterWindowMessageW(w!("SHELLHOOK"));
            if message==0{return Err("无法注册微信提醒事件".into());}
            let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!("DaozaiWechatListener"),WS_POPUP,0,0,0,0,None,None,None,None).map_err(|_|"无法创建微信监听窗口")?;
            let state=Box::new(RefCell::new(State{message,taskbar_created:RegisterWindowMessageW(w!("TaskbarCreated")),registered:true,paths,burst:Vec::new(),pending:Vec::new(),ctx}));
            let listener=Self{hwnd,state};
            SetWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID,(&*listener.state as *const RefCell<State>) as usize).ok().map_err(|_|"无法接入微信提醒事件")?;
            RegisterShellHookWindow(hwnd).ok().map_err(|_|"无法监听微信任务栏提醒")?;
            Ok(listener)
        }
    }
    pub fn registered(&self)->bool{self.state.borrow().registered}
    /// 取走新提醒：每项是（账号进程，闪动的窗口）。
    pub fn take(&self) -> Vec<(u32,isize)> {std::mem::take(&mut self.state.borrow_mut().pending)}
}
impl Drop for Listener {
    fn drop(&mut self){unsafe{
        let _=DeregisterShellHookWindow(self.hwnd);
        let _=RemoveWindowSubclass(self.hwnd,Some(subclass),SUBCLASS_ID);
        let _=DestroyWindow(self.hwnd);
    }}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clicking_wechat_flash_never_shows_hidden_helper(){unsafe{
        let window=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!(""),WS_POPUP,0,0,0,0,None,None,None,None).unwrap();
        let paths=[normalized(&std::env::current_exe().unwrap().to_string_lossy())];
        let unread=crate::app::Unread{source:crate::app::Messenger::Wechat,pid:std::process::id(),window:window.0 as isize};
        let result=open_unread_with_paths(unread,&paths);
        let visible=IsWindowVisible(window).as_bool();
        DestroyWindow(window).unwrap();
        assert!(!visible && result.is_err(),"点击提醒不应显示隐藏的辅助窗口");
    }}
    #[test]
    fn clicking_wechat_flash_never_shows_hidden_qt_frame(){unsafe{
        // 复现 v0.6.60 白屏：微信 4 隐藏的 Qt 窗口带系统菜单 / 最小化按钮，过去会被 SW_SHOW 显示出来。
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        unsafe extern "system" fn proc(h:HWND,m:u32,w:WPARAM,l:LPARAM)->LRESULT{DefWindowProcW(h,m,w,l)}
        let instance=GetModuleHandleW(None).unwrap().into();
        let class=WNDCLASSW{lpfnWndProc:Some(proc),hInstance:instance,lpszClassName:w!("Qt51514QWindowIcon"),..Default::default()};
        RegisterClassW(&class);
        let style=WINDOW_STYLE(0xC60B0000 & !WS_CHILD.0);
        let window=CreateWindowExW(WINDOW_EX_STYLE(0),w!("Qt51514QWindowIcon"),w!("Weixin"),style,0,0,300,200,None,None,instance,None).unwrap();
        let paths=[normalized(&std::env::current_exe().unwrap().to_string_lossy())];
        let pid=std::process::id();
        let flashed=open_unread_with_paths(crate::app::Unread{source:crate::app::Messenger::Wechat,pid,window:window.0 as isize},&paths);
        let unknown=open_unread_with_paths(crate::app::Unread{source:crate::app::Messenger::Wechat,pid:0,window:window.0 as isize},&paths);
        let visible=IsWindowVisible(window).as_bool();
        DestroyWindow(window).unwrap();
        assert!(!visible,"隐藏的微信窗口不得被显示");
        assert!(flashed.is_err() && unknown.is_err());
    }}
    #[test]
    #[ignore="需要本机已启动的微信；会把微信主窗口切到前台"]
    fn local_wechat_hidden_windows_stay_hidden_when_opened(){unsafe{
        use windows::Win32::Foundation::BOOL;
        unsafe extern "system" fn collect(hwnd:HWND,data:LPARAM)->BOOL{
            let (paths,out)=&mut *(data.0 as *mut (Vec<String>,Vec<(HWND,bool)>));
            if matches(hwnd,paths){out.push((hwnd,IsWindowVisible(hwnd).as_bool()));}
            true.into()
        }
        let mut found=(installed_paths(),Vec::new());
        let _=EnumWindows(Some(collect),LPARAM((&mut found as *mut (Vec<String>,Vec<(HWND,bool)>)) as isize));
        assert!(!found.1.is_empty(),"本机没有微信窗口");
        for &(hwnd,_) in &found.1 {
            let unread=crate::app::Unread{source:crate::app::Messenger::Wechat,pid:window_pid(hwnd),window:hwnd.0 as isize};
            let result=open_unread_with_paths(unread,&found.0);
            std::thread::sleep(Duration::from_millis(300));
            for &(other,was_visible) in &found.1 {
                assert!(was_visible || !IsWindowVisible(other).as_bool(),"隐藏窗口 {other:?} 被显示（目标 {hwnd:?}，结果 {result:?}）");
            }
            println!("目标 {hwnd:?} 可见={} → {result:?}",found.1.iter().any(|(h,v)|*h==hwnd && *v));
        }
    }}
    #[test]
    fn tray_buttons_match_by_icon_center(){
        use windows::Win32::Foundation::RECT;
        let icon=RECT{left:1531,top:1017,right:1581,bottom:1067};
        assert!(rect_center_inside(icon,RECT{left:1532,top:1018,right:1582,bottom:1068}));
        assert!(!rect_center_inside(icon,RECT{left:1582,top:1018,right:1632,bottom:1068}),"相邻图标不得命中");
        assert!(!rect_center_inside(icon,RECT::default()),"空矩形不得命中");
    }
    #[test]
    fn tray_open_rejects_unknown_accounts(){
        assert!(open_wechat_from_tray(crate::app::Unread::of(crate::app::Messenger::Wechat)).is_err());
        assert!(open_wechat_from_tray(crate::app::Unread{source:crate::app::Messenger::Qq,pid:1,window:0}).is_err());
        assert!(!wechat_in_tray(crate::app::Unread{source:crate::app::Messenger::Wechat,pid:std::process::id(),window:0}),"没有托盘图标的进程不算缩在托盘");
    }
    #[test]
    #[ignore="需要本机已登录的微信；会把微信关到托盘再唤起"]
    fn local_wechat_opens_from_tray(){unsafe{
        let paths=installed_paths();
        let pid=std::process::id();let _=pid;
        let main=[0u32].into_iter().find_map(|_|{
            use windows::Win32::Foundation::BOOL;
            unsafe extern "system" fn visit(hwnd:HWND,data:LPARAM)->BOOL{
                let (paths,out)=&mut *(data.0 as *mut (Vec<String>,Option<HWND>));
                if visible_main_frame(hwnd,crate::app::Messenger::Wechat) && matches(hwnd,paths){*out=Some(hwnd);return false.into();}
                true.into()
            }
            let mut found=(paths.clone(),None);
            let _=EnumWindows(Some(visit),LPARAM((&mut found as *mut (Vec<String>,Option<HWND>)) as isize));
            found.1
        }).expect("先打开微信主窗口");
        let unread=crate::app::Unread{source:crate::app::Messenger::Wechat,pid:window_pid(main),window:main.0 as isize};
        PostMessageW(main,WM_SYSCOMMAND,WPARAM(SC_CLOSE as usize),LPARAM(0)).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        assert!(!IsWindowVisible(main).as_bool(),"微信应已缩到托盘");
        assert!(wechat_in_tray(unread));
        assert!(open_unread_with_paths(unread,&paths).is_err(),"缩在托盘时普通跳转不碰任何窗口");
        let result=open_wechat_from_tray(unread);
        std::thread::sleep(Duration::from_secs(2));
        let overflow=FindWindowW(w!("TopLevelWindowForOverflowXamlIsland"),PCWSTR::null()).ok().is_some_and(|h|IsWindowVisible(h).as_bool());
        println!("托盘唤起 → {result:?}；主窗口可见={}；前台={}；折叠区仍展开={overflow}",IsWindowVisible(main).as_bool(),GetForegroundWindow()==main);
        assert!(result.is_ok() && IsWindowVisible(main).as_bool() && !IsHungAppWindow(main).as_bool() && !overflow);
    }}
    #[test]
    fn capture_overlays_do_not_count_as_viewing_a_messenger(){
        use crate::app::Messenger;
        let main=(WS_SYSMENU|WS_MINIMIZEBOX).0;
        assert!(client_main_frame(Messenger::Qq,"Chrome_WidgetWin_1",main,0));
        assert!(client_main_frame(Messenger::Wechat,"Qt51514QWindowIcon",main,0));
        assert!(!client_main_frame(Messenger::Qq,"Chrome_WidgetWin_1",WS_POPUP.0,WS_EX_TOPMOST.0));
        assert!(!client_main_frame(Messenger::Wechat,"Qt51514QWindowIcon",WS_POPUP.0,0));
        assert!(!client_main_frame(Messenger::Wechat,"Screenshot",main,0));
    }
    #[test]
    fn weixin_qt_main_window_without_system_menu_counts_as_viewed(){
        use crate::app::Messenger;
        assert!(client_main_frame(Messenger::Wechat,"Qt51514QWindowIcon",0x96c70000,0x100));
        assert!(!client_main_frame(Messenger::Wechat,"Qt51514QWindowIcon",WS_POPUP.0,0x100));
    }
    #[test]
    fn qq_nt_main_window_without_system_menu_counts_as_viewed(){
        use crate::app::Messenger::Qq;
        assert!(client_main_frame(Qq,"Chrome_WidgetWin_1",0x14c70000,0x100));
        assert!(!client_main_frame(Qq,"Chrome_WidgetWin_1",0x04c20000,0x80108));
        assert!(!client_main_frame(Qq,"Chrome_WidgetWin_1",0x04030000,0x80000));
    }
    #[test]
    #[ignore="需要本机 QQ 主窗口实际位于前台"]
    fn local_qq_main_window_is_viewed(){
        assert!(client_in_foreground(crate::app::Messenger::Qq));
    }
    #[test]
    fn protocol_registration_requires_an_absolute_wechat_executable(){
        assert_eq!(protocol_executable(r#"D:\Weixin\Weixin.exe "%1""#),Some(PathBuf::from(r"D:\Weixin\Weixin.exe")));
        assert_eq!(protocol_executable(r#""D:\Program Files\WeChat.exe" "%1""#),Some(PathBuf::from(r"D:\Program Files\WeChat.exe")));
        assert!(protocol_executable("Weixin.exe %1").is_none());
        assert!(protocol_executable(r#""C:\Windows\cmd.exe" /c Weixin.exe"#).is_none());
    }
    #[test]
    #[ignore="需要已启动并位于后台的微信窗口；仅向测试窗口发送模拟事件"]
    fn local_wechat_background_window(){unsafe{
        let target:isize=std::env::var("DAOZAI_TEST_WECHAT_HWND").unwrap().parse().unwrap();
        let listener=Listener::new(Default::default()).unwrap();
        let hwnd=listener.hwnd;
        let message=RegisterWindowMessageW(w!("SHELLHOOK"));
        SendMessageW(hwnd,message,WPARAM(FLASH),LPARAM(target));assert!(!listener.take().is_empty());
        SendMessageW(hwnd,message,WPARAM(FLASH),LPARAM(target));assert!(listener.take().is_empty());
        drop(listener);assert!(!IsWindow(hwnd).as_bool());
    }}
    #[test]
    fn shell_notifications_filter_targets_merge_flashes_and_detach(){unsafe{
        let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!(""),WS_POPUP,0,0,0,0,None,None,None,None).unwrap();
        let message=RegisterWindowMessageW(w!("SHELLHOOK"));
        let state=Box::new(RefCell::new(State{message,taskbar_created:RegisterWindowMessageW(w!("TaskbarCreated")),registered:true,paths:vec![],burst:Vec::new(),pending:Vec::new(),ctx:Default::default()}));
        SetWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID,(&*state as *const RefCell<State>) as usize).ok().unwrap();
        RegisterShellHookWindow(hwnd).ok().unwrap();
        let listener=Listener{hwnd,state};
        SendMessageW(hwnd,message,WPARAM(FLASH),LPARAM(hwnd.0 as isize));
        assert!(listener.take().is_empty(),"其他程序的闪动不得触发微信提醒");
        listener.state.borrow_mut().paths=vec![normalized(&std::env::current_exe().unwrap().to_string_lossy())];
        SendMessageW(hwnd,message,WPARAM(FLASH),LPARAM(0));assert!(listener.take().is_empty());
        SendMessageW(hwnd,message,WPARAM(FLASH),LPARAM(hwnd.0 as isize));
        assert_eq!(listener.take(),[(std::process::id(),hwnd.0 as isize)],"提醒带上账号进程和闪动的窗口");
        SendMessageW(hwnd,message,WPARAM(FLASH),LPARAM(hwnd.0 as isize));assert!(listener.take().is_empty());
        SendMessageW(hwnd,message,WPARAM(4),LPARAM(hwnd.0 as isize));
        SendMessageW(hwnd,message,WPARAM(FLASH),LPARAM(hwnd.0 as isize));assert!(listener.take().is_empty());
        listener.state.borrow_mut().burst.clear();
        SendMessageW(hwnd,message,WPARAM(FLASH),LPARAM(hwnd.0 as isize));
        SendMessageW(hwnd,message,WPARAM(0x8004),LPARAM(hwnd.0 as isize));
        assert!(!listener.take().is_empty(),"后台窗口的激活事件应保留尚未消费的提醒");
        DeregisterShellHookWindow(hwnd).ok().unwrap();
        SendMessageW(hwnd,RegisterWindowMessageW(w!("TaskbarCreated")),WPARAM(0),LPARAM(0));
        assert!(listener.registered());
        drop(listener);assert!(!IsWindow(hwnd).as_bool());
    }}
    #[test]
    fn continuous_flashes_are_one_reminder_and_quiet_gap_rearms(){
        let start=Instant::now();let mut burst=Burst::default();
        assert!(burst.attention(start));
        for second in 1..30 {assert!(!burst.attention(start+Duration::from_secs(second)));}
        assert!(burst.attention(start+Duration::from_secs(34)));
        burst=Burst::default();
        assert!(burst.attention(start+Duration::from_secs(35)));
    }
    #[test]
    fn paths_match_full_install_location(){
        assert_eq!(normalized(r"\\?\D:\Apps\Weixin.exe"),normalized("d:/apps/weixin.exe"));
        assert_ne!(normalized(r"D:\Apps\Weixin.exe"),normalized(r"D:\Other\Weixin.exe"));
    }
}
