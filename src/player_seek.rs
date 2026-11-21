//! 系统跳转不可用时，定位播放器自己的进度控件并向该窗口发送消息。
use windows::{core::{Interface,VARIANT},Win32::{Foundation::*,System::{Com::*,Threading::*},UI::{Accessibility::*,WindowsAndMessaging::*},Graphics::Gdi::ScreenToClient}};
use windows::Win32::UI::HiDpi::{GetDpiForWindow,SetThreadDpiAwarenessContext,DPI_AWARENESS_CONTEXT,DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

struct DpiContext(DPI_AWARENESS_CONTEXT);
impl Drop for DpiContext{fn drop(&mut self){if !self.0.0.is_null(){unsafe{SetThreadDpiAwarenessContext(self.0);}}}}

fn seconds(text:&str)->Option<f64>{
    let parts=text.trim().split(':').map(str::parse::<u32>).collect::<Result<Vec<_>,_>>().ok()?;
    match parts.as_slice(){[m,s] if *s<60=>Some((m*60+s)as f64),[h,m,s] if *m<60&&*s<60=>Some((h*3600+m*60+s)as f64),_=>None}
}

fn point(rect:RECT,position:f64,duration:f64,inset:f64)->Option<POINT>{
    let width=(rect.right-rect.left)as f64;
    if !position.is_finite() || !duration.is_finite() || duration<=0.0 || position<0.0 || position>duration
        || width<=inset*2.0+10.0 || rect.bottom<=rect.top{return None;}
    Some(POINT{x:((rect.left as f64+inset+(width-2.0*inset)*position/duration).round()as i32).min(rect.right-1),y:rect.top+(rect.bottom-rect.top)/2})
}

unsafe fn matches_player(hwnd:HWND,source:&str)->bool{
    let mut pid=0;GetWindowThreadProcessId(hwnd,Some(&mut pid));
    let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid)else{return false;};
    let mut name=[0u16;32768];let mut len=name.len()as u32;
    let result=QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,windows::core::PWSTR(name.as_mut_ptr()),&mut len);
    let _=CloseHandle(process);
    result.is_ok() && String::from_utf16_lossy(&name[..len as usize]).rsplit('\\').next().is_some_and(|name|name.eq_ignore_ascii_case(source))
}

unsafe fn window(source:&str)->Option<HWND>{
    struct Search<'a>{source:&'a str,apple:bool,found:Option<HWND>}
    unsafe extern "system" fn visit(hwnd:HWND,param:LPARAM)->BOOL{
        let search=&mut *(param.0 as *mut Search<'_>);
        let mut name=[0u16;80];let n=if search.apple{GetWindowTextW(hwnd,&mut name)}else{GetClassNameW(hwnd,&mut name)};
        let name=String::from_utf16_lossy(&name[..n.max(0)as usize]);
        if name==(if search.apple{"Apple Music"}else{"TXGuiFoundation"}) && IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd,GW_OWNER).is_err() && GetWindowTextLengthW(hwnd)>0 && matches_player(hwnd,search.source){search.found=Some(hwnd);return BOOL(0);}
        BOOL(1)
    }
    let apple=super::apple_music::is_source(source);
    let mut search=Search{source:if apple{"AppleMusic.exe"}else{source},apple,found:None};let _=EnumWindows(Some(visit),LPARAM(&mut search as *mut _ as isize));search.found
}

unsafe fn qq_bar(ui:&IUIAutomation,root:&IUIAutomationElement,duration:f64)->Option<IUIAutomationElement>{
    let condition=ui.CreatePropertyCondition(UIA_ControlTypePropertyId,&VARIANT::from(UIA_TextControlTypeId.0)).ok()?;
    let texts=root.FindAll(TreeScope_Descendants,&condition).ok()?;
    let walker=ui.ControlViewWalker().ok()?;let all=ui.CreateTrueCondition().ok()?;
    for i in 0..texts.Length().ok()? {
        let text=texts.GetElement(i).ok()?;
        if !seconds(&text.CurrentName().ok()?.to_string()).is_some_and(|v|(v-duration).abs()<1.5){continue;}
        let parent=walker.GetParentElement(&text).ok()?;
        let children=parent.FindAll(TreeScope_Children,&all).ok()?;
        if children.Length().ok()?!=3{continue;}
        let first=children.GetElement(0).ok()?;let last=children.GetElement(2).ok()?;
        if seconds(&first.CurrentName().ok()?.to_string()).is_none() || !ui.CompareElements(&last,&text).ok()?.as_bool(){continue;}
        let bar=children.GetElement(1).ok()?;
        let r=bar.CurrentBoundingRectangle().ok()?;
        let left=first.CurrentBoundingRectangle().ok()?;let right=last.CurrentBoundingRectangle().ok()?;
        if r.left>=left.right && r.right<=right.left && r.bottom-r.top<=60 && r.right-r.left>40{return Some(bar);}
    }
    None
}

unsafe fn automation()->Option<IUIAutomation>{
    let ui:IUIAutomation=CoCreateInstance(&CUIAutomation8,None,CLSCTX_INPROC_SERVER).ok()?;
    if let Ok(timed)=ui.cast::<IUIAutomation2>(){let _=timed.SetConnectionTimeout(800);let _=timed.SetTransactionTimeout(800);}
    Some(ui)
}

/// 波点的 Flutter 进度滑块通过 MSAA 提供位置和百分比，不能直接设置值。
pub(super) fn bodian_progress(window: HWND, current: f64, target: f64, duration: f64) -> Option<()> {
    unsafe {
        let _dpi = DpiContext(SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2));
        let view = FindWindowExW(window, HWND::default(), windows::core::w!("FLUTTERVIEW"), windows::core::PCWSTR::null()).ok()?;
        let mut raw = std::ptr::null_mut();
        AccessibleObjectFromWindow(view, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut raw).ok()?;
        if raw.is_null() { return None; }
        let root = IAccessible::from_raw(raw);

        let mut client = RECT::default();
        GetClientRect(view, &mut client).ok()?;
        let mut queue = std::collections::VecDeque::from([root]);
        let this = VARIANT::from(CHILDID_SELF as i32);
        let mut budget = 256;
        while let Some(node) = queue.pop_front() {
            if node.get_accRole(&this).ok().and_then(|role| u32::try_from(&role).ok()) == Some(ROLE_SYSTEM_SLIDER) {
                let value = node.get_accValue(&this).ok()?.to_string();
                let percent = value.strip_suffix('%').and_then(|value| value.parse::<u32>().ok());
                let (mut x, mut y, mut width, mut height) = (0, 0, 0, 0);
                node.accLocation(&mut x, &mut y, &mut width, &mut height, &this).ok()?;
                let mut origin = POINT { x, y };
                if !ScreenToClient(view, &mut origin).as_bool() { return None; }
                let bounds = RECT {
                    left: origin.x, top: origin.y,
                    right: origin.x.checked_add(width)?, bottom: origin.y.checked_add(height)?,
                };
                if bodian_playback_bar(bounds, client, percent, current, duration) {
                    // 1.1.7 的实际轨道在无障碍滑块两端各缩进 5 个逻辑像素。
                    let inset = 5.0 * f64::from(GetDpiForWindow(view)) / 96.0;
                    let p = point(bounds, target, duration, inset)?;
                    let lp = LPARAM(((p.y as u32) << 16 | p.x as u32) as isize);
                    let send = |message, buttons| SendMessageTimeoutW(view, message, WPARAM(buttons), lp,
                        SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT, 300, None).0 != 0;
                    if !send(WM_MOUSEMOVE, 0) { return None; }
                    let pressed = send(WM_LBUTTONDOWN, 1);
                    // Flutter 必须先处理按下事件，再处理释放。
                    if pressed { std::thread::sleep(std::time::Duration::from_millis(150)); }
                    let released = send(WM_LBUTTONUP, 0);
                    return (pressed && released).then_some(());
                }
            }
            let children = node.accChildCount().unwrap_or_default().clamp(0, budget);
            budget -= children;
            for child in 1..=children {
                if let Ok(child) = node.get_accChild(&VARIANT::from(child)).and_then(|child| child.cast()) {
                    queue.push_back(child);
                }
            }
        }
        None
    }
}

fn bodian_playback_bar(bar: RECT, client: RECT, percent: Option<u32>, current: f64, duration: f64) -> bool {
    let width = client.right - client.left;
    let height = bar.bottom - bar.top;
    // 歌曲进度条横跨窗口底边，音量滑块和页面内的其他滑块不符合这个边界。
    bar.left >= client.left && bar.right <= client.right && bar.right - bar.left >= width * 3 / 4
        && bar.top >= client.top
        && (1..=48).contains(&height) && (0..=64).contains(&(client.bottom - bar.bottom))
        && client.right <= i16::MAX as i32 && client.bottom <= i16::MAX as i32
        && percent.is_some_and(|value| value <= 100
            && (duration * f64::from(value) / 100.0 - current).abs() <= 2.0 + duration / 100.0)
}
unsafe fn apple_bar(ui:&IUIAutomation,root:&IUIAutomationElement)->Option<IUIAutomationRangeValuePattern>{
    let condition=ui.CreatePropertyCondition(UIA_AutomationIdPropertyId,&VARIANT::from("LCDScrubber")).ok()?;
    root.FindFirst(TreeScope_Descendants,&condition).ok()?.GetCurrentPattern(UIA_RangeValuePatternId).ok()?.cast().ok()
}
fn apple_value(range:&IUIAutomationRangeValuePattern,duration:f64)->Option<(f64,f64)>{
    unsafe {
        let (min,max,value)=(range.CurrentMinimum().ok()?,range.CurrentMaximum().ok()?,range.CurrentValue().ok()?);
        (duration.is_finite() && duration>0.0 && min==0.0 && max.is_finite() && (max-duration).abs()<1.5
            && value.is_finite() && (min..=max).contains(&value)).then_some((value,max))
    }
}
pub(super) fn seek(source:&str,position:f64,duration:f64)->bool{
    let apple=super::apple_music::is_source(source);
    if !apple && !super::is_qqmusic(source){return false;}
    // 窗口按进程映像名核对：新版的会话身份名（Tencent.QQMusic）不是文件名，统一换成 QQMusic.exe。
    let source=if apple {source} else {"QQMusic.exe"};
    unsafe { (||->Option<()> {
        let _dpi=DpiContext(SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2));
        let hwnd=window(source)?;
        if !IsWindowEnabled(hwnd).as_bool(){return None;}
        let ui=automation()?;
        let root=ui.ElementFromHandle(hwnd).ok()?;
        if apple {
            let range=apple_bar(&ui,&root)?;let (_,max)=apple_value(&range,duration)?;
            if range.CurrentIsReadOnly().ok()?.as_bool() || !position.is_finite() || !(0.0..=max).contains(&position){return None;}
            // 直接设置原生滑块；无需移动鼠标、激活窗口或扫描歌词区域。
            return range.SetValue(position).ok();
        }
        let bar=qq_bar(&ui,&root,duration)?;
        let target=hwnd;
        // 坐标来自无障碍控件，不依赖窗口位置；不移动鼠标，也不激活播放器。
        let mut p=point(bar.CurrentBoundingRectangle().ok()?,position,duration,5.0)?;
        if !ScreenToClient(target,&mut p).as_bool(){return None;}
        let mut client=RECT::default();GetClientRect(target,&mut client).ok()?;
        if p.x<0 || p.y<0 || (!IsIconic(hwnd).as_bool() && (p.x>=client.right || p.y>=client.bottom)){return None;}
        let lp=LPARAM(((p.y as u32&0xffff)<<16 | (p.x as u32&0xffff))as isize);
        let send=|msg,buttons|SendMessageTimeoutW(target,msg,WPARAM(buttons),lp,SMTO_ABORTIFHUNG|SMTO_ERRORONEXIT,500,None).0!=0;
        if !send(WM_MOUSEMOVE,0){return None;}
        let down=send(WM_LBUTTONDOWN,1);
        let up=send(WM_LBUTTONUP,0);
        (down&&up).then_some(())
    })().is_some() }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test] fn progress_coordinates_follow_control_bounds_and_reject_invalid_input(){
        for left in [-31000,100,2300]{let r=RECT{left,top:500,right:left+260,bottom:524};
            assert_eq!(point(r,0.0,200.0,5.0).unwrap().x,left+5);
            assert_eq!(point(r,100.0,200.0,5.0).unwrap().x,left+130);
            assert_eq!(point(r,200.0,200.0,5.0).unwrap().x,left+255);
            assert_eq!(point(r,200.0,200.0,0.0).unwrap().x,r.right-1);
            for p in [f64::NAN,-1.0,201.0]{assert!(point(r,p,200.0,5.0).is_none());}
            assert!(point(r,0.0,0.0,5.0).is_none());
        }
        assert_eq!(seconds(" 04:41"),Some(281.0));assert_eq!(seconds("01:02:03"),Some(3723.0));
        for text in ["播放","04:99","", "1:2:3:4"]{assert!(seconds(text).is_none());}
    }
    #[test]
    fn bodian_requires_a_bottom_slider_matching_playback() {
        let client = RECT { left: 0, top: 0, right: 1569, bottom: 1077 };
        let bar = RECT { left: 6, top: 1057, right: 1563, bottom: 1071 };
        assert!(bodian_playback_bar(bar, client, Some(25), 50.0, 200.0));
        assert!(!bodian_playback_bar(bar, client, Some(75), 50.0, 200.0));
        assert!(!bodian_playback_bar(RECT { right: 100, ..bar }, client, Some(25), 50.0, 200.0));
        assert!(!bodian_playback_bar(RECT { top: 50, bottom: 64, ..bar }, client, Some(25), 50.0, 200.0));
    }
}
