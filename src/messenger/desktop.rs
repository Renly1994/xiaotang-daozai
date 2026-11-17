//! 办公客户端只监听任务栏事件；唤起时仅处理已确认的原窗口。
use super::{Messenger,OpenError,Target,Unread};
use super::windows::*;
use windows::{core::Owned,Win32::{Foundation::{BOOL,HWND,LPARAM},System::Threading::{OpenProcess,PROCESS_QUERY_LIMITED_INFORMATION},UI::WindowsAndMessaging::*}};

pub(crate) const WECOM:NativeAdapter=NativeAdapter{
    paths:||paths(Messenger::WeCom),main_frame:|class,style,extended|main_frame(Messenger::WeCom,class,style,extended),
    accounts:None,ready,strict_window:true,open,taskbar:Some(|ctx|start(Messenger::WeCom,ctx)),
};
pub(crate) const FEISHU:NativeAdapter=NativeAdapter{
    paths:||paths(Messenger::Feishu),main_frame:|class,style,extended|main_frame(Messenger::Feishu,class,style,extended),
    accounts:None,ready,strict_window:true,open,taskbar:Some(|ctx|start(Messenger::Feishu,ctx)),
};
pub(crate) const DINGTALK:NativeAdapter=NativeAdapter{
    paths:||paths(Messenger::DingTalk),main_frame:|class,style,extended|main_frame(Messenger::DingTalk,class,style,extended),
    accounts:None,ready,strict_window:true,open,taskbar:Some(|ctx|start(Messenger::DingTalk,ctx)),
};
fn paths(client:Messenger)->Vec<String>{client.adapter().process_names.iter().map(|name|format!("*{}",name.to_ascii_lowercase())).collect()}
fn start(client:Messenger,ctx:eframe::egui::Context)->Result<Box<dyn TaskbarSource>,String>{
    Ok(Box::new(crate::wechat::Listener::for_client(client,ctx)?))
}
fn main_frame(client:Messenger,class:&str,style:u32,extended:u32)->bool{
    if style&WS_CHILD.0!=0 || extended&(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE).0!=0{return false;}
    match client{
        Messenger::WeCom=>class=="WeWorkWindow",
        Messenger::Feishu=>class=="Chrome_WidgetWin_1" && frame_style(class,style,extended),
        Messenger::DingTalk=>(class=="StandardFrame_DingTalk" || class.starts_with("Qt") && class.ends_with("QWindowIcon")) && frame_style(class,style,extended),
        _=>false,
    }
}
pub(crate) unsafe fn main_window(window:HWND,client:Messenger)->bool{
    use windows::Win32::Foundation::RECT;
    if !IsWindow(window).as_bool() || GetWindow(window,GW_OWNER).is_ok()
        || !client_main_frame(client,&class_name(window),GetWindowLongW(window,GWL_STYLE)as u32,GetWindowLongW(window,GWL_EXSTYLE)as u32){return false;}
    if IsIconic(window).as_bool(){return true;}
    let mut rect=RECT::default();
    GetWindowRect(window,&mut rect).is_ok() && rect.right-rect.left>=300 && rect.bottom-rect.top>=300
}
fn candidates(client:Messenger,paths:&[String])->Option<Vec<Target>>{unsafe{
    struct Search<'a>{client:Messenger,paths:&'a [String],windows:Vec<Unread>}
    unsafe extern "system" fn visit(window:HWND,data:LPARAM)->BOOL{
        let search=&mut *(data.0 as *mut Search);
        if main_window(window,search.client) && matches(window,search.paths){
            search.windows.push(Unread{source:search.client,pid:window_pid(window),window:window.0 as isize});
        }
        true.into()
    }
    let mut search=Search{client,paths,windows:Vec::new()};
    EnumWindows(Some(visit),LPARAM((&mut search as *mut Search)as isize)).ok()?;
    search.windows.into_iter().map(|unread|{
        let process=Owned::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,unread.pid).ok()?);
        Some(Target{unread,started:process_started(*process)?})
    }).collect()
}}
/// 只接受原事件窗口或其明确的所属主窗口，同进程的其他窗口不能借用。
fn select_target(client:Messenger,window:isize,accounts:&[Target])->Unread{
    let Some(target)=accounts.iter().find(|target|target.unread.source==client && target.unread.window==window)else{return Unread::of(client);};
    if accounts.iter().filter(|other|other.unread.source==client && other.unread.pid==target.unread.pid).count()!=1{return Unread::of(client);}
    target.unread
}
pub(crate) fn flash_target(client:Messenger,window:HWND,paths:&[String])->Option<Unread>{unsafe{
    if !matches(window,paths){return None;}
    let Some(accounts)=candidates(client,paths)else{return Some(Unread::of(client));};
    Some(select_target(client,GetAncestor(window,GA_ROOTOWNER).0 as isize,&accounts))
}}
fn ready(target:Target)->bool{
    ready_with_paths(target,&client_paths(target.unread.source))
}
fn ready_with_paths(target:Target,paths:&[String])->bool{
    candidates(target.unread.source,paths).is_some_and(|accounts|{
        let mine=accounts.iter().filter(|current|current.unread.pid==target.unread.pid).collect::<Vec<_>>();
        matches!(mine.as_slice(),[one] if **one==target)
    })
}
fn open(unread:Unread,paths:&[String],_:bool)->Result<(),OpenError>{unsafe{
    let window=HWND(unread.window as *mut _);
    if unread.pid==0 || window_pid(window)!=unread.pid || !main_window(window,unread.source) || !matches(window,paths){return Err(OpenError::Unavailable);}
    // 不执行协议、托盘回调或鼠标键盘模拟，只恢复该消息保存的主窗口。
    if IsIconic(window).as_bool(){let _=ShowWindowAsync(window,SW_RESTORE);}
    else if !IsWindowVisible(window).as_bool(){let _=ShowWindowAsync(window,SW_SHOW);}
    if SetForegroundWindow(window).as_bool(){Ok(())}else{Err(OpenError::Unavailable)}
}}

#[cfg(test)]
mod tests{
    use super::*;
    use windows::{core::w,Win32::{Foundation::{LRESULT,WPARAM},System::{LibraryLoader::GetModuleHandleW,Threading::GetCurrentProcess}}};
    fn target(client:Messenger,pid:u32,window:isize)->Target{Target{unread:Unread{source:client,pid,window},started:1}}
    #[test]
    fn three_or_more_instances_never_rebind_to_another_or_the_last_instance(){
        for client in Messenger::OFFICE{
            let (a,b,c)=(target(client,100,11),target(client,200,22),target(client,300,33));
            assert_eq!(select_target(client,11,&[a,b,c]),a.unread);
            assert_eq!(select_target(client,22,&[a,b,c]),b.unread);
            assert_eq!(select_target(client,11,&[b,c]),Unread::of(client));
            assert_eq!(select_target(client,11,&[c]),Unread::of(client));
            assert_eq!(select_target(client,11,&[a,Target{unread:Unread{window:44,..a.unread},..a}]),Unread::of(client),"同进程多窗口不能当作唯一账号");
        }
    }
    #[test]
    fn main_window_rules_exclude_login_popups_and_cross_client_windows(){
        let frame=WS_OVERLAPPEDWINDOW.0;
        assert!(main_frame(Messenger::WeCom,"WeWorkWindow",WS_POPUP.0,0));
        assert!(main_frame(Messenger::Feishu,"Chrome_WidgetWin_1",frame,0));
        assert!(main_frame(Messenger::DingTalk,"Qt51511QWindowIcon",frame,0));
        assert!(main_frame(Messenger::DingTalk,"StandardFrame_DingTalk",0x360f0000,0x100),"支持实际日志中的已登录钉钉主窗口和最小化状态");
        for client in Messenger::OFFICE{
            assert!(!main_frame(client,"WeChatLogin",frame,0));
            assert!(!main_frame(client,"Chrome_WidgetWin_0",frame,0));
            assert!(!main_frame(client,"PerryShadowWnd",frame,0));
        }
        assert!(!main_frame(Messenger::Feishu,"Chrome_WidgetWin_1",WS_POPUP.0,0));
        assert!(!main_frame(Messenger::DingTalk,"Qt51511QWindowIcon",0x96020000,0));
        assert!(!main_frame(Messenger::DingTalk,"StandardFrame_DingTalk",WS_POPUP.0,0));
        assert!(!main_frame(Messenger::DingTalk,"StandardFrame_DingTalk",frame,WS_EX_TOOLWINDOW.0));
        assert!(!main_frame(Messenger::DingTalk,"StandardFrame_DingTalk",frame|WS_CHILD.0,0));
        assert!(!main_frame(Messenger::Feishu,"StandardFrame_DingTalk",frame,0));
        assert!(!main_frame(Messenger::WeCom,"WeWorkWindow",WS_POPUP.0,WS_EX_TOOLWINDOW.0));
    }
    #[test]
    fn synthetic_native_windows_keep_original_handles_and_reject_ambiguity(){unsafe{
        unsafe extern "system" fn proc(h:HWND,m:u32,w:WPARAM,l:LPARAM)->LRESULT{DefWindowProcW(h,m,w,l)}
        let instance=GetModuleHandleW(None).unwrap().into();let pid=std::process::id();
        let paths=[normalized(&std::env::current_exe().unwrap().to_string_lossy())];
        let started=process_started(GetCurrentProcess()).unwrap();
        for (client,class) in [(Messenger::WeCom,w!("WeWorkWindow")),(Messenger::Feishu,w!("Chrome_WidgetWin_1")),(Messenger::DingTalk,w!("Qt51511QWindowIcon")),(Messenger::DingTalk,w!("StandardFrame_DingTalk"))]{
            RegisterClassW(&WNDCLASSW{lpfnWndProc:Some(proc),hInstance:instance,lpszClassName:class,..Default::default()});
            let create=||CreateWindowExW(WINDOW_EX_STYLE(0),class,w!("Daozai synthetic client"),WS_OVERLAPPEDWINDOW,-10000,-10000,400,500,None,None,instance,None).unwrap();
            let first=create();let original=Target{unread:Unread{source:client,pid,window:first.0 as isize},started};
            assert!(ready_with_paths(original,&paths),"唯一原窗口必须可识别");
            assert_eq!(flash_target(client,first,&paths),Some(original.unread));
            let helper=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!(""),WS_POPUP,-10000,-10000,400,500,None,None,instance,None).unwrap();
            let ambiguous=flash_target(client,helper,&paths);DestroyWindow(helper).unwrap();
            assert_eq!(ambiguous,Some(Unread::of(client)),"同进程但没有窗口归属关系的辅助窗口不能借用仅剩的主窗口");
            let owned=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!(""),WS_POPUP,-10000,-10000,400,500,first,None,instance,None).unwrap();
            let bound=flash_target(client,owned,&paths);DestroyWindow(owned).unwrap();
            assert_eq!(bound,Some(original.unread),"窗口明确属于原主窗口时可以识别");
            let second=create();
            assert!(!ready_with_paths(original,&paths),"同进程新增主窗口后不得继续按进程猜账号");
            assert_eq!(flash_target(client,first,&paths),Some(Unread::of(client)));
            DestroyWindow(first).unwrap();
            assert!(!ready_with_paths(original,&paths),"不能换成仍存活的其他窗口");
            assert_eq!(open(original.unread,&paths,true),Err(OpenError::Unavailable));
            assert!(!IsWindowVisible(second).as_bool(),"不能打开其他窗口");
            DestroyWindow(second).unwrap();
        }
    }}
    #[test]
    fn hidden_helpers_are_never_shown(){unsafe{
        let paths=[normalized(&std::env::current_exe().unwrap().to_string_lossy())];
        let window=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!(""),WS_POPUP,-10000,-10000,400,500,None,None,None,None).unwrap();
        for source in Messenger::OFFICE{
            assert_eq!(open(Unread{source,pid:std::process::id(),window:window.0 as isize},&paths,true),Err(OpenError::Unavailable));
            assert!(!IsWindowVisible(window).as_bool());
        }
        DestroyWindow(window).unwrap();
    }}
}
