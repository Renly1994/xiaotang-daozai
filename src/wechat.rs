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
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() || GetWindow(hwnd,GW_OWNER).is_ok() || !matches(hwnd,paths){return None;}
    let mut class=[0u16;128];
    let n=GetClassNameW(hwnd,&mut class).max(0) as usize;
    client_main_frame(source,&String::from_utf16_lossy(&class[..n]),GetWindowLongW(hwnd,GWL_STYLE) as u32,GetWindowLongW(hwnd,GWL_EXSTYLE) as u32).then(||window_pid(hwnd))
}

pub fn client_in_foreground(source:crate::app::Messenger)->bool{
    unsafe{foreground_main_pid(source,&client_paths(source)).is_some()}
}

/// 当前前台的微信 / QQ 主窗口：来源与账号进程。
pub fn foreground_client(sources:&[crate::app::Messenger])->Option<(crate::app::Messenger,u32)>{
    sources.iter().find_map(|&source|unsafe{foreground_main_pid(source,&client_paths(source))}.map(|pid|(source,pid)))
}

/// 跳到某条未读对应的窗口；窗口已不在时退回到打开该客户端。
pub fn open_unread(unread:crate::app::Unread)->Result<(),String>{
    let window=HWND(unread.window as *mut _);
    unsafe{
        if unread.window!=0 && IsWindow(window).as_bool() && window_pid(window)==unread.pid && matches(window,&client_paths(unread.source)){
            if IsIconic(window).as_bool(){let _=ShowWindow(window,SW_RESTORE);}
            else if !IsWindowVisible(window).as_bool(){let _=ShowWindow(window,SW_SHOW);}
            if SetForegroundWindow(window).as_bool(){return Ok(());}
        }
    }
    open_client(unread.source)
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
            if IsIconic(window).as_bool(){let _=ShowWindow(window,SW_RESTORE);}
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
