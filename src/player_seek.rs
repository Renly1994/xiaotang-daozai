//! 系统跳转不可用时，定位播放器自己的进度控件并向该窗口发送消息。
use windows::{core::{Interface,VARIANT},Win32::{Foundation::*,System::{Com::*,Threading::*},UI::{Accessibility::*,WindowsAndMessaging::*},Graphics::Gdi::ClientToScreen}};
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext,DPI_AWARENESS_CONTEXT,DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
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

/// QQ 音乐可能同时有主窗口、迷你播放器等多个顶层窗口；全部收集，再由调用方挑出含进度条的那个。
unsafe fn windows(source:&str)->Vec<HWND>{
    struct Search<'a>{source:&'a str,found:Vec<HWND>}
    unsafe extern "system" fn visit(hwnd:HWND,param:LPARAM)->BOOL{
        let search=&mut *(param.0 as *mut Search<'_>);
        let mut name=[0u16;80];let n=GetClassNameW(hwnd,&mut name);
        let class=String::from_utf16_lossy(&name[..n.max(0)as usize]);
        if class=="TXGuiFoundation" && IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd,GW_OWNER).is_err()
            && GetWindowTextLengthW(hwnd)>0 && matches_player(hwnd,search.source){search.found.push(hwnd);}
        BOOL(1)
    }
    let mut search=Search{source,found:Vec::new()};
    let _=EnumWindows(Some(visit),LPARAM(&mut search as *mut _ as isize));
    search.found
}

unsafe fn automation()->Option<IUIAutomation>{
    let ui:IUIAutomation=CoCreateInstance(&CUIAutomation8,None,CLSCTX_INPROC_SERVER).ok()?;
    if let Ok(timed)=ui.cast::<IUIAutomation2>(){let _=timed.SetConnectionTimeout(800);let _=timed.SetTransactionTimeout(800);}
    Some(ui)
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

unsafe fn qq_bar(ui:&IUIAutomation,root:&IUIAutomationElement,expected:Option<f64>)->Option<(IUIAutomationElement,f64,IUIAutomationElement)>{
    let condition=ui.CreatePropertyCondition(UIA_ControlTypePropertyId,&VARIANT::from(UIA_TextControlTypeId.0)).ok()?;
    let texts=root.FindAll(TreeScope_Descendants,&condition).ok()?;
    let walker=ui.ControlViewWalker().ok()?;let all=ui.CreateTrueCondition().ok()?;
    let mut best:Option<(IUIAutomationElement,f64,IUIAutomationElement)>=None;
    for i in 0..texts.Length().ok()? {
        let text=texts.GetElement(i).ok()?;
        let total=match seconds(&text.CurrentName().ok()?.to_string()){Some(value)=>value,None=>continue};
        // 系统会话给的总时长偶尔与界面显示差几秒，只做宽松校验，实际时长以控件为准。
        if expected.is_some_and(|value|(value-total).abs()>8.0){continue;}
        let parent=walker.GetParentElement(&text).ok()?;
        let children=parent.FindAll(TreeScope_Children,&all).ok()?;
        if children.Length().ok()?!=3{continue;}
        let first=children.GetElement(0).ok()?;let last=children.GetElement(2).ok()?;
        if seconds(&first.CurrentName().ok()?.to_string()).is_none() || !ui.CompareElements(&last,&text).ok()?.as_bool(){continue;}
        let bar=children.GetElement(1).ok()?;
        let r=bar.CurrentBoundingRectangle().ok()?;
        let left=first.CurrentBoundingRectangle().ok()?;let right=last.CurrentBoundingRectangle().ok()?;
        if r.left>=left.right && r.right<=right.left && r.bottom-r.top<=60 && r.right-r.left>40{
            let delta=expected.map(|value|(value-total).abs()).unwrap_or(0.0);
            if delta==0.0{return Some((bar,total,parent));}
            if best.as_ref().map_or(true,|(_,best_delta,_):&(IUIAutomationElement,f64,IUIAutomationElement)|delta<*best_delta){best=Some((bar,total,parent));}
        }
    }
    if best.is_some(){return best;}
    // 回退：版本改版把时间标签挪走时，改在“能读到两个时间”的父控件里找滑块/进度条。
    for control in [UIA_SliderControlTypeId,UIA_ProgressBarControlTypeId,UIA_PaneControlTypeId]{
        let Ok(condition)=ui.CreatePropertyCondition(UIA_ControlTypePropertyId,&VARIANT::from(control.0))else{continue};
        let Ok(bars)=root.FindAll(TreeScope_Descendants,&condition)else{continue};
        for i in 0..bars.Length().ok()?{
            let Ok(bar)=bars.GetElement(i)else{continue};
            let Ok(r)=bar.CurrentBoundingRectangle()else{continue};
            if r.right-r.left<=40 || r.bottom-r.top>60{continue;}
            let Ok(parent)=walker.GetParentElement(&bar)else{continue};
            let Ok(group_texts)=parent.FindAll(TreeScope_Descendants,&condition)else{continue};
            let mut total:Option<f64>=None;
            for k in 0..group_texts.Length().ok()?{
                if let Some(value)=group_texts.GetElement(k).ok().and_then(|e|e.CurrentName().ok()).and_then(|name|seconds(&name.to_string())){
                    total=Some(total.map_or(value,|current|current.max(value)));
                }
            }
            let Some(total)=total else{continue};
            if total<=0.0{continue;}
            if expected.is_some_and(|value|(value-total).abs()>8.0){continue;}
            return Some((bar,total,parent));
        }
    }
    None
}

/// 从进度条那一组控件读当前播放时间；读不到时返回 None，由调用方决定是否继续。
unsafe fn qq_position(group:&IUIAutomationElement,ui:&IUIAutomation)->Option<f64>{
    let all=ui.CreateTrueCondition().ok()?;
    let children=group.FindAll(TreeScope_Children,&all).ok()?;
    if children.Length().ok()?<3{return None;}
    seconds(&children.GetElement(0).ok()?.CurrentName().ok()?.to_string())
}

pub(super) fn seek(source:&str,position:f64,duration:f64)->bool{
    use std::time::{Duration,Instant};
    let apple=super::apple_music::is_source(source);
    let qq=super::qq_music(source);
    if !apple && !qq{return false;}
    unsafe { (||->Option<()> {
        let _dpi=DpiContext(SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2));
        let ui=automation()?;
        // 系统会话里的 AUMID 与进程名不一致，窗口匹配统一用映像名。
        if apple {
            let hwnd=window("AppleMusic.exe")?;
            if !IsWindowEnabled(hwnd).as_bool(){return None;}
            let root=ui.ElementFromHandle(hwnd).ok()?;
            let range=apple_bar(&ui,&root)?;let (_,max)=apple_value(&range,duration)?;
            if range.CurrentIsReadOnly().ok()?.as_bool() || !position.is_finite() || !(0.0..=max).contains(&position){return None;}
            // 直接设置原生滑块；无需移动鼠标、激活窗口或扫描歌词区域。
            return range.SetValue(position).ok();
        }
        let expected=(duration>0.0).then_some(duration);
        let mut chosen=None;
        for hwnd in windows("qqmusic.exe"){
            if !IsWindowEnabled(hwnd).as_bool(){continue;}
            let Ok(root)=ui.ElementFromHandle(hwnd)else{continue};
            if let Some(found)=qq_bar(&ui,&root,expected){chosen=Some((hwnd,found));break;}
        }
        let (hwnd,(bar,total,group))=chosen?;
        let rect=bar.CurrentBoundingRectangle().ok()?;
        let mut client=RECT::default();GetClientRect(hwnd,&mut client).ok()?;
        let origin=if IsIconic(hwnd).as_bool(){
            // 最小化时窗口被移到 -32000，屏幕坐标不可用；用窗口矩形推算客户区原点。
            let mut window_rect=RECT::default();GetWindowRect(hwnd,&mut window_rect).ok()?;
            (window_rect.left,window_rect.top)
        }else{
            let mut point=POINT{x:0,y:0};
            if !ClientToScreen(hwnd,&mut point).as_bool(){return None;}
            (point.x,point.y)
        };
        let mut aim=position.clamp(0.0,total);
        let mut delivered=false;
        // QQ 的进度条有内边距，按比例点击会差几秒；点完读回进度再校正，最多三次。
        for _ in 0..3 {
            let p=point(rect,aim,total,5.0)?;
            let (cx,cy)=(p.x-origin.0,p.y-origin.1);
            if cx<0 || cy<0 || cx>=client.right || cy>=client.bottom{return None;}
            let lp=LPARAM(((cy as u32&0xffff)<<16 | (cx as u32&0xffff))as isize);
            let send=|msg,buttons|SendMessageTimeoutW(hwnd,msg,WPARAM(buttons),lp,SMTO_ABORTIFHUNG|SMTO_ERRORONEXIT,500,None).0!=0;
            if !send(WM_MOUSEMOVE,0){return None;}
            let down=send(WM_LBUTTONDOWN,1);
            let up=send(WM_LBUTTONUP,0);
            if !(down&&up){return None;}
            delivered=true;
            let deadline=Instant::now()+Duration::from_millis(400);
            let mut current=None;
            while Instant::now()<deadline{
                std::thread::sleep(Duration::from_millis(80));
                if let Some(value)=qq_position(&group,&ui){current=Some(value);if (value-aim).abs()<=2.0{break;}}
            }
            let Some(current)=current else{break};
            if (current-position).abs()<=2.0{return Some(());}
            aim=(aim+(position-current)).clamp(0.0,total);
        }
        delivered.then_some(())
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
}
