//! 外设电量：蓝牙设备沿用 `bt` 的扫描结果，USB 接收器走 SetupAPI。
//!
//! 设备按稳定 key 区分：蓝牙 `bt:<规范化 MAC>`，USB `usb:<vid>:<pid>`。
//! 岛体只显示「在线」设备，离线的一瞬间就从岛上移除；设置页保留三拍，
//! 方便用户看清是掉线还是拔掉了接收器。

use crate::bt::BatteryRoute;
use crate::paint::H;
use crate::theme::W;
use eframe::egui::Color32;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind { Mouse, Headphones, Keyboard, Other }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link { Bluetooth, Usb }

impl Link {
    pub fn label(self) -> &'static str {match self {Self::Bluetooth=>"蓝牙",Self::Usb=>"2.4G"}}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// 稳定标识：`bt:d200388f9d27` / `usb:2717:50fc`。
    pub key: String,
    /// 系统给出的原名。
    pub name: String,
    /// 已知型号的中文名；认不出时为 None。
    pub chinese: Option<&'static str>,
    pub kind: Kind,
    pub link: Link,
    pub battery: Option<u8>,
    /// 电量来自哪条通路（设置页诊断用）。
    pub route: BatteryRoute,
    /// 读取失败原因；成功时为空。
    pub note: String,
    /// 最近一次扫描是否出现；岛体只认这个（掉线立刻移除）。
    pub present: bool,
    /// 连续三次未出现后置为「未连接」；设置页用它决定灰显和开关可用性。
    pub online: bool,
    /// 连续未扫到的次数；用于判断何时标记「未连接」。
    pub(crate) missed: u8,
}

impl Default for Device {
    fn default() -> Self {
        Self{key:String::new(),name:String::new(),chinese:None,kind:Kind::Other,link:Link::Bluetooth,battery:None,route:BatteryRoute::Unknown,note:String::new(),present:false,online:false,missed:0}
    }
}

impl Device {
    /// 岛体/设置页显示名：`原名（中文名）`，认不出时只有原名。
    pub fn display_name(&self) -> String {
        match self.chinese {
            Some(chinese) if !chinese.is_empty() && chinese != self.name => format!("{}（{}）", self.name, chinese),
            _ => self.name.clone(),
        }
    }
    pub fn battery_text(&self) -> String {
        self.battery.map(|level|format!("{level}%")).unwrap_or_else(||"—".into())
    }
    /// 列表右侧的电量位：离线只显示状态；在线但拿不到电量仍显示 `—`。
    pub fn status_text(&self) -> String {
        if !self.online {"离线".into()} else {self.battery_text()}
    }
    /// 设置页说明：`蓝牙 · 电量 80%（BLE GATT）` / `2.4G · Windows 未提供电量（2.4G HID：超时）` / `蓝牙 · 离线`。
    pub fn description(&self) -> String {
        if !self.online {return format!("{} · 离线",self.link.label());}
        match self.battery {
            Some(level)=>format!("{} · 电量 {level}%（{}）",self.link.label(),self.route.label()),
            None if self.route==BatteryRoute::Unsupported=>format!("{} · 未提供电量（厂商协议未支持）",self.link.label()),
            None if self.note.is_empty()=>format!("{} · Windows 未提供电量（读取通道：{}）",self.link.label(),self.route.label()),
            None=>format!("{} · Windows 未提供电量（读取通道：{}；{}）",self.link.label(),self.route.label(),self.note),
        }
    }
}

/// 列表分组：0 在线有电量 → 1 在线无电量 → 2 离线。
fn tier(device:&Device)->u8 {
    if !device.online {2} else if device.battery.is_some() {0} else {1}
}

/// 设置页与岛体共用的列表排序：在线在前、同组电量低的在前、离线灰显排最后。
/// 组内再看类型、名称、稳定 key，保证每次扫描后位置不跳动。
pub(crate) fn sort_devices(list:&mut Vec<Device>) {
    list.sort_by(|a,b| {
        let (ta,tb)=(tier(a),tier(b));
        ta.cmp(&tb)
            .then_with(|| if ta==0 {a.battery.unwrap_or(0).cmp(&b.battery.unwrap_or(0))} else {std::cmp::Ordering::Equal})
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// 已知型号的中文名。USB 接收器优先按 VID/PID 判定，蓝牙按设备名。
pub fn chinese_name(name: &str, key: &str) -> Option<&'static str> {
    if let Some((_,_,_,chinese))=receivers().iter().find(|(vid,pid,_,_)|key.eq_ignore_ascii_case(&format!("usb:{}:{}",vid.to_ascii_lowercase(),pid.to_ascii_lowercase()))) {
        return Some(chinese);
    }
    let lower=name.to_ascii_lowercase();
    if lower.contains("mi mouse4p") || lower.contains("mi mouse 4 pro") || key.eq_ignore_ascii_case("bt:d200388f9d27") {
        return Some("小米无线鼠标 4 Pro");
    }
    None
}

fn receivers() -> &'static [(&'static str,&'static str,&'static str,&'static str)] {
    // VID, PID, Windows 常见原名, 中文名
    &[
        ("2717","50FC","Mi Mouse 4 Pro Receiver","小米无线鼠标 4 Pro 接收器"),
        ("3554","FA09","2.4G Wireless Receiver","RK R98"),
    ]
}

fn kind_of(kind: crate::bt::DeviceKind) -> Option<Kind> {
    use crate::bt::DeviceKind as B;
    match kind {
        B::Mouse => Some(Kind::Mouse),
        B::Keyboard => Some(Kind::Keyboard),
        B::Bone | B::Earbuds | B::Headphones => Some(Kind::Headphones),
        // 手机 / 音箱 / 手柄等不属于「鼠标、耳机、键盘」，不纳入设备电量。
        _ => None,
    }
}

fn from_bluetooth(device: &crate::bt::Device) -> Option<Device> {
    let kind=kind_of(device.kind)?;
    let key=format!("bt:{}",device.id.to_ascii_lowercase());
    let chinese=chinese_name(&device.name,&key);
    Some(Device {
        key,
        name: device.name.clone(),
        chinese,
        kind,
        link: Link::Bluetooth,
        battery: device.battery.filter(|level|*level<=100),
        route: device.route,
        note: device.note.clone(),
        present: true,
        online: true,
        missed: 0,
    })
}

/// USB 接收器（HID）快照。
pub struct Service {
    pub rx: std::sync::mpsc::Receiver<Vec<Device>>,
    stop: std::sync::mpsc::Sender<()>,
}
impl Drop for Service {
    fn drop(&mut self) {let _=self.stop.send(());}
}

/// 每两秒枚举一次 USB 接收器；蓝牙部分由 `crate::bt` 负责，这里不重复扫描。
/// 小米接收器的电量每 60 秒查询一次，其余时间复用缓存，避免频繁打开 HID 句柄。
pub fn spawn() -> Service {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let (stop, stopped)=std::sync::mpsc::channel();
    std::thread::Builder::new().name("daozai-devices".into()).spawn(move || {
        let mut scanner=UsbScanner::default();
        loop {
            if stopped.try_recv()!=Err(std::sync::mpsc::TryRecvError::Empty) {return;}
            let found=scanner.scan();
            if matches!(tx.try_send(found),Err(std::sync::mpsc::TrySendError::Disconnected(_))) {return;}
            if stopped.recv_timeout(Duration::from_secs(2))!=Err(std::sync::mpsc::RecvTimeoutError::Timeout) {return;}
        }
    }).ok();
    Service {rx,stop}
}

/// 把蓝牙快照和 USB 快照合并成带在线状态的设备表。
#[derive(Default)]
pub struct Tracker { devices: Vec<Device> }
impl Tracker {
    pub fn update(&mut self, bluetooth: &[crate::bt::Device], usb: &[Device]) -> Vec<Device> {
        let mut found:Vec<Device>=bluetooth.iter().filter_map(from_bluetooth).collect();
        for device in usb {
            let mut device=device.clone();
            device.present=true;device.online=true;device.missed=0;
            if !found.iter().any(|other|other.key==device.key) {found.push(device);}
        }
        for old in &mut self.devices {
            match found.iter().find(|new|new.key==old.key) {
                Some(new)=>{
                    old.present=true;old.missed=0;old.online=true;
                    old.name.clone_from(&new.name);old.chinese=new.chinese;
                    old.kind=new.kind;old.link=new.link;
                    old.battery=new.battery;
                    old.route=new.route;old.note.clone_from(&new.note);
                }
                None=>{
                    old.present=false;
                    old.missed=old.missed.saturating_add(1);
                    // 连续三拍都没出现才标记未连接；岛体在第一次未出现时已移除。
                    old.online=old.missed<3;
                }
            }
        }
        for new in found {
            if !self.devices.iter().any(|old|old.key==new.key) {self.devices.push(new);}
        }
        // 离线设备保留在表里，设置页继续显示并保留逐设备开关。
        self.devices.clone()
    }
}

/// USB 接收器扫描状态：缓存小米接收器的电量与失败原因。
#[derive(Default)]
struct UsbScanner { battery: std::collections::HashMap<String,(Option<u8>,String,std::time::Instant)> }

impl UsbScanner {
    #[cfg(windows)]
    fn scan(&mut self) -> Vec<Device> {
        use windows::core::PCWSTR;
        use windows::Win32::Devices::DeviceAndDriverInstallation::*;
        use windows::Win32::Foundation::HWND;
        let mut found:Vec<Device>=Vec::new();
        unsafe {
            let Ok(set)=SetupDiGetClassDevsW(None,PCWSTR::null(),HWND::default(),DIGCF_PRESENT|DIGCF_ALLCLASSES) else {return found;};
            for index in 0..u32::MAX {
                let mut data=SP_DEVINFO_DATA {cbSize:std::mem::size_of::<SP_DEVINFO_DATA>() as u32,..Default::default()};
                if SetupDiEnumDeviceInfo(set,index,&mut data).is_err() {break;}
                let Some(instance)=instance_id(set,&data) else {continue;};
                let Some((vid,pid))=usb_vid_pid(&instance) else {continue;};
                let Some((_,_,default_name,chinese))=receivers().iter().find(|(known_vid,known_pid,_,_)|known_vid.eq_ignore_ascii_case(vid)&&known_pid.eq_ignore_ascii_case(pid)) else {continue;};
                let key=format!("usb:{}:{}",vid.to_ascii_lowercase(),pid.to_ascii_lowercase());
                if found.iter().any(|device:&Device|device.key==key) {continue;}
                let name=friendly_name(set,&data).unwrap_or_else(||default_name.to_string());
                let mut device=Device {
                    key:key.clone(),
                    name,
                    chinese:Some(chinese),
                    kind:Kind::Mouse,
                    link:Link::Usb,
                    battery:None,
                    route:BatteryRoute::Unsupported,
                    note:String::new(),
                    present:true,
                    online:true,
                    missed:0,
                };
                if key.eq_ignore_ascii_case("usb:2717:50fc") {
                    // 接收器协议：HID 集合 0xFF03 / usage 0，33/33 字节，每 60 秒查询一次。
                    let stale=self.battery.get(&key).is_none_or(|(_,_,at)|at.elapsed()>=Duration::from_secs(60));
                    if stale {
                        let entry=match query_xiaomi_battery() {
                            Ok(level)=>(Some(level),String::new(),std::time::Instant::now()),
                            Err(note)=>(None,note.to_string(),std::time::Instant::now()),
                        };
                        self.battery.insert(key.clone(),entry);
                    }
                    if let Some((level,note,_))=self.battery.get(&key) {
                        device.battery=*level;device.route=BatteryRoute::Hid;device.note.clone_from(note);
                    }
                }
                found.push(device);
            }
            let _=SetupDiDestroyDeviceInfoList(set);
        }
        found
    }

    #[cfg(not(windows))]
    fn scan(&mut self) -> Vec<Device> {Vec::new()}
}

/// 小米无线鼠标 4 Pro 的 2.4G 电量查询：打开 MI_02 的厂商集合，写 0x16，读回 0x16 报告。
#[cfg(windows)]
fn query_xiaomi_battery() -> Result<u8,&'static str> {
    let paths=xiaomi_hid_paths()?;
    if paths.is_empty() {return Err("未找到 MI_02 接口");}
    let mut last="未找到 33/33 电量集合";
    for path in paths {
        let handle=match open_hid(&path) {Ok(handle)=>handle,Err(note)=>{last=note;continue;}};
        let usable=match hid_caps_match(handle) {Ok(usable)=>usable,Err(note)=>{last=note;false}};
        if !usable {
            unsafe {let _=windows::Win32::Foundation::CloseHandle(handle);}
            continue;
        }
        let result=query_hid_handle(handle);
        unsafe {let _=windows::Win32::Foundation::CloseHandle(handle);}
        return result;
    }
    Err(last)
}

/// 列出路径里带 `MI_02` 的小米 HID 接口，避免打开 MI_00（鼠标输入）/ MI_01。
#[cfg(windows)]
fn xiaomi_hid_paths() -> Result<Vec<String>,&'static str> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Devices::HumanInterfaceDevice::GUID_DEVINTERFACE_HID;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    let mut paths=Vec::new();
    unsafe {
        let set=SetupDiGetClassDevsW(Some(&GUID_DEVINTERFACE_HID),PCWSTR::null(),HWND::default(),DIGCF_PRESENT|DIGCF_DEVICEINTERFACE).map_err(|_|"枚举 HID 接口失败")?;
        for index in 0..u32::MAX {
            let mut iface=SP_DEVICE_INTERFACE_DATA {cbSize:std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,..Default::default()};
            if SetupDiEnumDeviceInterfaces(set,None,&GUID_DEVINTERFACE_HID,index,&mut iface).is_err() {break;}
            let mut need=0u32;
            let _=SetupDiGetDeviceInterfaceDetailW(set,&iface,None,0,Some(&mut need),None);
            if need==0 {continue;}
            let mut raw=vec![0u8;need as usize];
            let detail=raw.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            std::ptr::write(detail,SP_DEVICE_INTERFACE_DETAIL_DATA_W{cbSize:if cfg!(target_pointer_width="64") {8}else{6},DevicePath:[0]});
            if SetupDiGetDeviceInterfaceDetailW(set,&iface,Some(detail),need,None,None).is_err() {continue;}
            // DevicePath 紧跟 cbSize（偏移 4）。
            let units=std::slice::from_raw_parts(raw.as_ptr().add(4) as *const u16,(need as usize-4)/2);
            let path=String::from_utf16_lossy(units);
            let path=path.trim_end_matches('\0');
            let upper=path.to_ascii_uppercase();
            if upper.contains("VID_2717")&&upper.contains("PID_50FC")&&upper.contains("MI_02") {
                paths.push(path.to_string());
            }
        }
        let _=SetupDiDestroyDeviceInfoList(set);
    }
    Ok(paths)
}

#[cfg(windows)]
fn open_hid(path:&str)->Result<windows::Win32::Foundation::HANDLE,&'static str> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GENERIC_READ,GENERIC_WRITE,HANDLE};
    use windows::Win32::Storage::FileSystem::{CreateFileW,FILE_ATTRIBUTE_NORMAL,FILE_FLAG_OVERLAPPED,FILE_SHARE_READ,FILE_SHARE_WRITE,OPEN_EXISTING};
    let wide:Vec<u16>=path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        CreateFileW(PCWSTR(wide.as_ptr()),GENERIC_READ.0|GENERIC_WRITE.0,FILE_SHARE_READ|FILE_SHARE_WRITE,None,OPEN_EXISTING,FILE_ATTRIBUTE_NORMAL|FILE_FLAG_OVERLAPPED,HANDLE::default())
            .map_err(|_|"句柄打开失败")
    }
}

/// 校验这是 usagePage 0xFF03、usage 0 且输入 / 输出各 33 字节的集合。
#[cfg(windows)]
fn hid_caps_match(handle:windows::Win32::Foundation::HANDLE)->Result<bool,&'static str> {
    use windows::Win32::Devices::HumanInterfaceDevice::*;
    unsafe {
        let mut attributes=HIDD_ATTRIBUTES::default();
        if !HidD_GetAttributes(handle,&mut attributes).as_bool() {return Err("读取 HID 属性失败");}
        if attributes.VendorID!=0x2717 || attributes.ProductID!=0x50FC {return Ok(false);}
        let mut preparsed=PHIDP_PREPARSED_DATA(0);
        if !HidD_GetPreparsedData(handle,&mut preparsed).as_bool() {return Err("读取 HID 预解析数据失败");}
        let mut caps=HIDP_CAPS::default();
        let status=HidP_GetCaps(preparsed,&mut caps);
        let _=HidD_FreePreparsedData(preparsed);
        if status!=HIDP_STATUS_SUCCESS {return Err("读取 HID 能力失败");}
        Ok(caps.UsagePage==0xFF03 && caps.Usage==0 && caps.InputReportByteLength==33 && caps.OutputReportByteLength==33)
    }
}

#[cfg(windows)]
fn query_hid_handle(handle:windows::Win32::Foundation::HANDLE)->Result<u8,&'static str> {
    std::thread::sleep(Duration::from_millis(100));
    let mut frame=query_frame();
    unsafe {overlapped_io(handle,&mut frame,true,Duration::from_millis(500))?;}
    let deadline=std::time::Instant::now()+Duration::from_secs(3);
    let mut last="未找到 0x16 电量报告";
    while std::time::Instant::now()<deadline {
        let mut buf=[0u8;33];
        match unsafe {overlapped_io(handle,&mut buf,false,Duration::from_millis(300))} {
            Ok(bytes)=> {
                match level_from_report(&buf[..(bytes as usize).min(buf.len())]) {
                    Ok(level)=>return Ok(level),
                    Err(note)=>last=note,
                }
            }
            Err("超时")=>{last="读取超时";}
            Err(note)=>return Err(note),
        }
    }
    Err(last)
}

/// 33 字节查询帧：reportId 0、固定 0、命令 0x16，其余清零。
fn query_frame()->[u8;33] {let mut frame=[0u8;33];frame[2]=0x16;frame}

/// 兼容原生报告（0x16 在 [2]）与 hidapi 剥掉 reportId（0x16 在 [1]）；电量在命令后第 3 字节。
fn level_from_report(report:&[u8])->Result<u8,&'static str> {
    for command in [1usize,2] {
        if report.get(command)==Some(&0x16) {
            let level=*report.get(command+3).ok_or("报告长度不足")?;
            return match level {
                1..=100=>Ok(level),
                0=>Err("设备休眠返回 0"),
                _=>Err("电量值超出范围"),
            };
        }
    }
    Err("未找到 0x16 电量报告")
}

/// 同步 `ReadFile` / `WriteFile` 会永久阻塞，这里用 OVERLAPPED + 事件做超时。
#[cfg(windows)]
unsafe fn overlapped_io(handle:windows::Win32::Foundation::HANDLE,buf:&mut [u8],write:bool,timeout:Duration)->Result<u32,&'static str> {
    use windows::Win32::Foundation::{CloseHandle,WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::{ReadFile,WriteFile};
    use windows::Win32::System::IO::{CancelIoEx,GetOverlappedResult,OVERLAPPED};
    use windows::Win32::System::Threading::{CreateEventW,WaitForSingleObject};
    let event=CreateEventW(None,false,false,None).map_err(|_|"创建事件失败")?;
    let mut overlapped=OVERLAPPED {hEvent:event,..Default::default()};
    let result=if write {WriteFile(handle,Some(buf),None,Some(&mut overlapped))} else {ReadFile(handle,Some(buf),None,Some(&mut overlapped))};
    if let Err(error)=result {
        // 0x800703E5 = ERROR_IO_PENDING，重叠操作正常排队。
        if error.code()!=windows::core::HRESULT(0x800703E5u32 as i32) {
            let _=CloseHandle(event);
            return Err(if write {"写入失败"} else {"读取失败"});
        }
    }
    let wait=WaitForSingleObject(event,timeout.as_millis().min(u32::MAX as u128) as u32);
    if wait!=WAIT_OBJECT_0 {
        let _=CancelIoEx(handle,Some(&overlapped));
        let _=CloseHandle(event);
        return Err("超时");
    }
    let mut bytes=0u32;
    let outcome=GetOverlappedResult(handle,&overlapped,&mut bytes,false);
    let _=CloseHandle(event);
    outcome.map_err(|_|if write {"写入失败"} else {"读取失败"})?;
    Ok(bytes)
}

/// 从 `USB\VID_2717&PID_50FC\...` 里取出大写的 VID/PID。
fn usb_vid_pid(instance: &str) -> Option<(&str,&str)> {
    let lower=instance.to_ascii_lowercase();
    let vid_start=lower.find("vid_")?+4;
    let pid_start=lower.find("pid_")?+4;
    let vid=instance.get(vid_start..vid_start+4)?;
    let pid=instance.get(pid_start..pid_start+4)?;
    if !vid.bytes().all(|b|b.is_ascii_hexdigit())||!pid.bytes().all(|b|b.is_ascii_hexdigit()) {return None;}
    Some((vid,pid))
}

#[cfg(windows)]
unsafe fn instance_id(
    set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    data: &windows::Win32::Devices::DeviceAndDriverInstallation::SP_DEVINFO_DATA,
) -> Option<String> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::SetupDiGetDeviceInstanceIdW;
    let mut buf=[0u16;512];
    let mut need=0u32;
    SetupDiGetDeviceInstanceIdW(set,data,Some(&mut buf),Some(&mut need)).ok()?;
    Some(String::from_utf16_lossy(&buf[..(need as usize).saturating_sub(1)]))
}

#[cfg(windows)]
unsafe fn friendly_name(
    set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    data: &windows::Win32::Devices::DeviceAndDriverInstallation::SP_DEVINFO_DATA,
) -> Option<String> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::{SetupDiGetDeviceRegistryPropertyW,SPDRP_FRIENDLYNAME};
    let mut buf=[0u16;256];
    let mut kind=0u32;
    let mut need=0u32;
    let bytes=std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8,buf.len()*2);
    SetupDiGetDeviceRegistryPropertyW(set,data,SPDRP_FRIENDLYNAME,Some(&mut kind),Some(bytes),Some(&mut need)).ok()?;
    let text=String::from_utf16_lossy(&buf[..(need as usize/2).saturating_sub(1)]);
    (!text.trim().is_empty()).then_some(text)
}

// ---------- 岛体「设备电量」页 ----------

const ROW_X:f32=40.0;
const ROW_W:f32=339.0;
const ROW_H:f32=32.0;
/// 展开后额外增加的高度：三行诊断信息。
const DETAIL_H:f32=52.0;
/// 悬停展开速率：约 0.18 s 收敛到 95%，天然缓出。
const HOVER_RATE:f32=16.0;
/// 移出后延迟收起，避免边缘抖动。
const HOVER_LEAVE:f64=0.13;

impl crate::app::App {
    pub(crate) fn page_devices(&mut self,c:&crate::paint::Card,env:&crate::app::Env) {
        use crate::{paint::alpha, theme};
        let mut list=self.devices.clone();
        // 在线有电量（电量升序）→ 在线无电量 → 离线灰显排最后；组内按类型、名称稳定排序。
        sort_devices(&mut list);
        self.device_page_anim.retain(|key,_|list.iter().any(|device|&device.key==key));
        if list.is_empty() {
            c.text(ROW_X,74.0,"还没有发现设备",12.5,W::Semibold,theme::GRAY,H::Left);
            c.text(ROW_X,94.0,"连接蓝牙外设，或插入 2.4G 接收器后再回到这一页。",10.5,W::Regular,theme::DIM,H::Left);
            return;
        }
        let online=list.iter().filter(|device|device.online).count();
        c.text(379.0,25.0,&format!("{online}/{} 台在线",list.len()),10.5,W::Regular,theme::DIM,H::Right);
        let first=self.device_page_first.min(list.len().saturating_sub(3));
        let bottom=198.0;
        let mut y=34.0;
        for device in list.iter().skip(first) {
            if y+ROW_H>bottom {break;}
            // 热区用「上一帧高度 ∪ 当前高度」：展开/收起过程中光标始终留在卡内。
            let previous=self.device_page_anim.get(&device.key).map_or(0.0,|(t,_)|*t);
            let previous_h=ROW_H+DETAIL_H*previous;
            let local=env.cursor.map(|p|c.to_local(p));
            let hovered=local.is_some_and(|p|p.x>=ROW_X&&p.x<=ROW_X+ROW_W&&p.y>=y&&p.y<=y+previous_h);
            let t={
                let entry=self.device_page_anim.entry(device.key.clone()).or_insert((0.0,None));
                if hovered {
                    entry.1=None;
                    entry.0=crate::anim::approach(entry.0,1.0,HOVER_RATE,self.dt);
                } else if entry.0>0.0001 {
                    let left=*entry.1.get_or_insert(self.t);
                    if self.t-left>=HOVER_LEAVE {entry.0=crate::anim::approach(entry.0,0.0,HOVER_RATE,self.dt);}
                } else {
                    entry.1=None;
                }
                entry.0
            };
            let h=ROW_H+DETAIL_H*t;
            let resp=c.hit(ROW_X,y,ROW_W,h,("device-page",device.key.as_str()));
            if resp.clicked() {self.settings_tab=12;self.open_settings();}
            c.fill(ROW_X,y,ROW_W,h,10.0,Color32::from_white_alpha((7.0+9.0*t+if hovered {6.0}else{0.0}) as u8));
            if device.online {
                crate::extras::device_badge(c,ROW_X+20.0,y+16.0,self.settings.device_icon_style,device.kind,device.battery);
            } else {
                let dim=c.faded(0.5);
                crate::extras::device_badge(&dim,ROW_X+20.0,y+16.0,self.settings.device_icon_style,device.kind,device.battery);
            }
            let title=device.display_name();
            let title_color=if device.online {theme::WHITE}else{alpha(theme::GRAY,0.55)};
            c.text(ROW_X+44.0,y+21.0,&c.fit(&title,12.0,W::Semibold,236.0),12.0,W::Semibold,title_color,H::Left);
            // 电量位：圆点在数字前；离线只显示状态，且整行与圆点一起降透明度。
            let status=device.status_text();
            let status_color=if device.online {theme::WHITE}else{alpha(theme::WHITE,0.45)};
            let dot=if device.online {theme::GREEN}else{alpha(theme::GRAY,0.4)};
            let right=ROW_X+ROW_W-8.0;
            let text_left=right-c.text_w(&status,11.5,W::Semibold);
            c.circle(text_left-8.5,y+16.5,3.5,dot);
            c.text(right,y+21.0,&status,11.5,W::Semibold,status_color,H::Right);
            if t>0.02 {
                // 只对详情区域做插值裁剪，尺寸与透明度一起过渡。
                let a=crate::anim::smoothstep(0.25,1.0,t);
                let d=c.faded(a).clipped(ROW_X,y+ROW_H,ROW_W,h-ROW_H);
                let route=if device.route==BatteryRoute::Unsupported {"厂商协议未支持"}else{device.route.label()};
                let line1=format!("类型：{} · 连接：{} · 状态：{}",kind_label(device.kind),device.link.label(),if device.online {"在线"}else{"离线"});
                let line2=if device.online {format!("电量：{} · 读取通路：{}",device.battery_text(),route)} else {"电量：离线，设备不在线".to_string()};
                let (line3,color3)=if !device.note.is_empty() {(format!("诊断：{}",device.note),alpha(theme::YELLOW,0.92))}
                    else if !device.online {("恢复连接后自动刷新".to_string(),alpha(theme::GRAY,0.8))}
                    else if device.route==BatteryRoute::Unsupported {("该 2.4G 接收器暂未适配厂商协议".to_string(),alpha(theme::YELLOW,0.8))}
                    else if device.battery.is_none() {("等待采集结果".to_string(),alpha(theme::GRAY,0.8))}
                    else {("读取正常".to_string(),alpha(theme::GRAY,0.8))};
                d.text(ROW_X+44.0,y+46.0,&d.fit(&line1,10.5,W::Regular,ROW_W-52.0),10.5,W::Regular,alpha(theme::GRAY,0.9),H::Left);
                d.text(ROW_X+44.0,y+62.0,&d.fit(&line2,10.5,W::Regular,ROW_W-52.0),10.5,W::Regular,if device.online {alpha(theme::WHITE,0.9)}else{alpha(theme::GRAY,0.8)},H::Left);
                d.text(ROW_X+44.0,y+78.0,&d.fit(&line3,10.0,W::Regular,ROW_W-52.0),10.0,W::Regular,color3,H::Left);
            }
            y+=h+4.0;
        }
    }
}

fn kind_label(kind:Kind)->&'static str {
    match kind {Kind::Mouse=>"鼠标",Kind::Headphones=>"耳机",Kind::Keyboard=>"键盘",Kind::Other=>"其他"}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bluetooth(id:&str,name:&str,battery:Option<u8>)->crate::bt::Device {crate::bt::Device{id:id.into(),name:name.into(),battery,kind:crate::bt::DeviceKind::Mouse,..crate::bt::Device::default()}}

    fn usb(vid:&str,pid:&str,level:Option<u8>)->Device {
        let route=if vid.eq_ignore_ascii_case("2717") {BatteryRoute::Hid}else{BatteryRoute::Unsupported};
        Device{key:format!("usb:{}:{}",vid,pid),name:"2.4G Wireless Receiver".into(),chinese:chinese_name("2.4G Wireless Receiver",&format!("usb:{vid}:{pid}")),kind:Kind::Mouse,link:Link::Usb,battery:level,route,note:String::new(),present:true,online:true,missed:0}
    }

    fn sample(key:&str,name:&str,kind:Kind,battery:Option<u8>,online:bool)->Device {
        Device{key:key.into(),name:name.into(),chinese:None,kind,link:Link::Bluetooth,battery,route:BatteryRoute::BleGatt,note:String::new(),present:online,online,missed:0}
    }

    #[test]
    fn list_order_is_online_by_battery_then_no_battery_then_offline(){
        let mut list=vec![
            sample("bt:c","C 键盘",Kind::Keyboard,None,false),
            sample("bt:b","B 耳机",Kind::Headphones,None,true),
            sample("bt:a","A 鼠标 80",Kind::Mouse,Some(80),true),
            sample("bt:d","D 鼠标 30",Kind::Mouse,Some(30),true),
            sample("bt:e","E 鼠标无电量",Kind::Mouse,None,true),
        ];
        sort_devices(&mut list);
        let keys:Vec<&str>=list.iter().map(|device|device.key.as_str()).collect();
        assert_eq!(keys,["bt:d","bt:a","bt:e","bt:b","bt:c"],"在线有电量按电量升序 → 在线无电量 → 离线");
        assert_eq!(list[0].status_text(),"30%");
        assert_eq!(list[2].status_text(),"—","在线无电量仍显示 —");
        assert_eq!(list[4].status_text(),"离线","离线只显示状态，不显示电量");
    }

    #[test]
    fn parses_usb_vid_pid_from_instance_id(){
        assert_eq!(usb_vid_pid(r"USB\VID_2717&PID_50FC\5&2a3b1c4d&0&1"),Some(("2717","50FC")));
        assert_eq!(usb_vid_pid(r"USB\VID_3554&PID_FA09\6&1"),Some(("3554","FA09")));
        assert_eq!(usb_vid_pid(r"HID\VID_0000&PID_0001"),Some(("0000","0001")));
        assert_eq!(usb_vid_pid(r"ROOT\BLUETOOTH\0000"),None);
    }

    #[test]
    fn known_models_get_chinese_names_and_unknown_keep_original(){
        assert_eq!(chinese_name("Mi Mouse4P","bt:d200388f9d27"),Some("小米无线鼠标 4 Pro"));
        assert_eq!(chinese_name("Mi Mouse4P","bt:000000000000"),Some("小米无线鼠标 4 Pro"));
        assert_eq!(chinese_name("2.4G Wireless Receiver","usb:3554:fa09"),Some("RK R98"));
        assert_eq!(chinese_name("Mi Mouse 4 Pro Receiver","usb:2717:50fc"),Some("小米无线鼠标 4 Pro 接收器"));
        assert_eq!(chinese_name("Tonemay Q6","bt:aa11bb22cc33"),None);
        assert_eq!(chinese_name("未知设备","bt:001122334455"),None);
    }

    #[test]
    fn display_name_uses_full_width_parentheses_only_when_chinese_exists(){
        let mut device=usb("3554","fa09",None);
        assert_eq!(device.display_name(),"2.4G Wireless Receiver（RK R98）");
        assert_eq!(device.battery_text(),"—");
        assert_eq!(device.description(),"2.4G · 未提供电量（厂商协议未支持）");
        device.chinese=None;
        assert_eq!(device.display_name(),"2.4G Wireless Receiver");
        device.battery=Some(66);device.route=BatteryRoute::Hid;
        assert_eq!(device.description(),"2.4G · 电量 66%（2.4G HID）");
    }

    #[test]
    fn tracker_removes_from_island_immediately_and_marks_offline_after_three_misses(){
        let mut tracker=Tracker::default();
        let found=tracker.update(&[bluetooth("AA","Mi Mouse4P",Some(80))],&[]);
        assert_eq!(found[0].key,"bt:aa");
        assert!(found[0].present&&found[0].online);
        // 第一拍未出现：岛体立即移除（present=false），设置页还有宽限。
        let found=tracker.update(&[],&[]);
        assert!(!found[0].present);
        assert!(found[0].online);
        tracker.update(&[],&[]);
        let found=tracker.update(&[],&[]);
        assert_eq!(found.len(),1,"离线设备保留在设备表里");
        assert!(!found[0].online,"连续三次未出现后标记未连接");
    }

    #[test]
    fn tracker_takes_latest_battery_and_merges_usb(){
        let mut tracker=Tracker::default();
        tracker.update(&[bluetooth("AA","Mi Mouse4P",Some(70))],&[usb("3554","fa09",None)]);
        // 蓝牙线程的缓存负责「偶发拿不到时沿用」，这里拿到明确的 None 就必须清掉，
        // 否则 PnP 那个停在旧日期的 2% 假电量会被一直保留。
        let found=tracker.update(&[bluetooth("AA","Mi Mouse4P",None)],&[usb("3554","fa09",None)]);
        assert_eq!(found.len(),2);
        assert!(found.iter().all(|device|device.present), "恢复在线后应重新上岛");
        assert_eq!(found.iter().find(|d|d.key=="bt:aa").unwrap().battery,None,"明确的空电量不得沿用旧值");
    }

    #[test]
    fn hid_report_parsing_accepts_native_and_stripped_frames(){
        let mut native=[0u8;33];
        native[2]=0x16;native[5]=84;
        assert_eq!(level_from_report(&native),Ok(84));
        let mut stripped=[0u8;33];
        stripped[1]=0x16;stripped[4]=84;
        assert_eq!(level_from_report(&stripped),Ok(84));
        let mut sleeping=[0u8;33];
        sleeping[2]=0x16;sleeping[5]=0;
        assert_eq!(level_from_report(&sleeping),Err("设备休眠返回 0"),"0 表示关机/休眠，不能显示成 0%");
        let mut invalid=[0u8;33];
        invalid[2]=0x16;invalid[5]=101;
        assert_eq!(level_from_report(&invalid),Err("电量值超出范围"));
        assert_eq!(level_from_report(&[0u8;33]),Err("未找到 0x16 电量报告"));
        assert_eq!(query_frame()[2],0x16,"查询帧命令固定在第 3 字节");
    }
}
