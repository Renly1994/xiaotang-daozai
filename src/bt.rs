//! 经典蓝牙与低功耗蓝牙设备的连接状态和电量。

use std::time::{Duration,Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub battery: Option<u8>,
    pub kind: DeviceKind,
    /// 是否为 BLE 设备（决定是否尝试 GATT 电量）。
    pub le: bool,
    /// 当前电量来自哪条通路（设置页诊断用）。
    pub route: BatteryRoute,
    /// 读取失败原因；成功时为空。
    pub note: String,
}

impl Default for Device {
    fn default() -> Self {
        Self { id:String::new(), name:String::new(), battery:None, kind:DeviceKind::Mouse, le:false, route:BatteryRoute::Unknown, note:String::new() }
    }
}

/// 电量读取通路（仅用于诊断展示）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BatteryRoute { #[default] Unknown, Pnp, BleGatt, Hid, Unsupported }

impl BatteryRoute {
    pub fn label(self) -> &'static str {
        match self { Self::Unknown=>"未知", Self::Pnp=>"PnP 缓存", Self::BleGatt=>"BLE GATT", Self::Hid=>"2.4G HID", Self::Unsupported=>"未支持" }
    }
}

/// 一次电量扫描的结果（逐设备，含失败原因）。
#[derive(Clone, Debug)]
pub struct BatteryReading {
    pub id: String,
    pub level: Option<u8>,
    pub route: BatteryRoute,
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind { Bone, Earbuds, Headphones, Speaker, Keyboard, Mouse, Gamepad, Phone, Other }

fn device_kind(name: &str, classic: bool, class: Option<u32>) -> DeviceKind {
    use DeviceKind::*;
    let name = name.to_lowercase();
    if ["shokz", "aftershokz", "openmove", "openrun", "骨传导"].iter().any(|s| name.contains(s)) { return Bone; }
    if ["earbuds", "airpods", "buds", "earphones", "入耳"].iter().any(|s| name.contains(s)) { return Earbuds; }
    if ["keyboard", "键盘"].iter().any(|s| name.contains(s)) { return Keyboard; }
    if ["mouse", "鼠标"].iter().any(|s| name.contains(s)) { return Mouse; }
    if ["gamepad", "controller", "手柄"].iter().any(|s| name.contains(s)) { return Gamepad; }
    if ["speaker", "音箱", "音响"].iter().any(|s| name.contains(s)) { return Speaker; }
    if let Some(class) = class {
        if classic {
            match (class >> 8) & 0x1f {
                2 => return Phone,
                4 => return if matches!((class >> 2) & 0x3f, 5 | 7 | 10) { Speaker } else { Headphones },
                5 => return match (class >> 6) & 3 { 1 => Keyboard, 2 => Mouse, 3 => Keyboard, _ => Other },
                _ => {}
            }
        } else {
            match (class >> 6, class & 0x3f) {
                (15, 1) => return Keyboard,
                (15, 2) => return Mouse,
                (15, 3 | 4) => return Gamepad,
                (1, _) => return Phone,
                _ => {}
            }
        }
    }
    if ["headphone", "headset", "耳机"].iter().any(|s| name.contains(s)) { Headphones } else { Other }
}

impl Device {
    /// 通知条正文：`名称 · 电量 80%`，设备没上报电量时只写名称。
    pub fn summary(&self) -> String {
        match self.battery {
            Some(b) => format!("{} · 电量 {b}%", self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Default)]
struct Cache {devices:Vec<Device>,battery_at:Option<Instant>}
impl Cache {
    fn update(&mut self,mut devices:Vec<Device>,now:Instant,read:impl FnOnce(&[Device])->Result<Vec<BatteryReading>,()>)->Vec<Device>{
        let changed=devices.len()!=self.devices.len() || devices.iter().any(|d|!self.devices.iter().any(|old|old.id==d.id));
        for device in &mut devices {
            if let Some(old)=self.devices.iter().find(|old|old.id==device.id){
                device.battery=old.battery;device.route=old.route;device.note=old.note.clone();
            }
        }
        if devices.is_empty(){self.battery_at=None;}
        else if changed || self.battery_at.is_none_or(|at|now.duration_since(at)>=Duration::from_secs(60)) {
            // 失败同样限频，避免设备驱动异常时每两秒重新枚举整机。
            self.battery_at=Some(now);
            if let Ok(readings)=read(&devices){
                for device in &mut devices {
                    if let Some(reading)=readings.iter().find(|reading|reading.id==device.id){
                        device.battery=reading.level;device.route=reading.route;device.note=reading.note.clone();
                    }
                }
            }
        }
        self.devices=devices.clone();devices
    }
}

/// 释放服务时唤醒后台线程，停止后续扫描。
pub struct Service {
    pub rx: std::sync::mpsc::Receiver<Vec<Device>>,
    stop: std::sync::mpsc::Sender<()>,
}
impl Drop for Service {
    fn drop(&mut self) {let _=self.stop.send(());}
}

pub fn spawn() -> Service {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let (stop, stopped)=std::sync::mpsc::channel();
    std::thread::Builder::new().name("daozai-bluetooth".into()).spawn(move || {
        #[cfg(windows)]
        let mut cache=Cache::default();
        loop {
            if stopped.try_recv()!=Err(std::sync::mpsc::TryRecvError::Empty) {return;}
            #[cfg(windows)]
            if let Ok(found)=query(&mut cache) {
                if matches!(tx.try_send(found),Err(std::sync::mpsc::TrySendError::Disconnected(_))) {return;}
            }
            #[cfg(not(windows))]
            let _=&tx;
            if stopped.recv_timeout(Duration::from_secs(2))!=Err(std::sync::mpsc::RecvTimeoutError::Timeout) {return;}
        }
    }).ok();
    Service {rx,stop}
}

#[cfg(windows)]
fn query(cache:&mut Cache) -> Result<Vec<Device>, ()> {
    use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
    use windows::Devices::Enumeration::DeviceInformation;
    let selectors = [
        (true, BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected).map_err(|_| ())?),
        (false, BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected).map_err(|_| ())?),
    ];
    let mut devices = Vec::new();
    for (classic, selector) in selectors {
        let selector = windows::core::HSTRING::from(format!("({selector}) AND System.Devices.Aep.IsConnected:=System.StructuredQueryType.Boolean#True"));
        let found = DeviceInformation::FindAllAsyncAqsFilter(&selector).map_err(|_| ())?.get().map_err(|_| ())?;
        for info in found {
            let Ok(id) = info.Id() else { continue };
            let Ok(name) = info.Name() else { continue };
            let name = name.to_string();
            if name.trim().is_empty() { continue; }
            let address = id.to_string().rsplit('-').next().unwrap_or_default().replace(':', "").to_ascii_uppercase();
            if devices.iter().any(|d: &Device| d.id == address) { continue; }
            let kind=if let Some(old)=cache.devices.iter().find(|old|old.id==address && old.name==name){old.kind}else{
                let class=if classic {
                    BluetoothDevice::FromIdAsync(&id).ok().and_then(|a|a.get().ok()).and_then(|d|{
                        let class=d.ClassOfDevice().ok().and_then(|c|c.RawValue().ok());let _=d.Close();class
                    })
                }else{
                    BluetoothLEDevice::FromIdAsync(&id).ok().and_then(|a|a.get().ok()).and_then(|d|{
                        let class=d.Appearance().ok().and_then(|c|c.RawValue().ok()).map(u32::from);let _=d.Close();class
                    })
                };
                device_kind(&name,classic,class)
            };
            devices.push(Device { id: address, kind, name, battery: None, le: !classic, route: BatteryRoute::Unknown, note: String::new() });
        }
    }
    devices.sort_by(|a,b|a.id.cmp(&b.id));
    Ok(cache.update(devices,Instant::now(),read_batteries))
}

#[cfg(windows)]
fn read_batteries(devices:&[Device])->Result<Vec<BatteryReading>,()> {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Devices::Properties::DEVPROPKEY;
    use windows::Win32::Foundation::HWND;

    // DEVPKEY_Bluetooth_Battery {104EA319-6EE2-4701-BD47-8DDBF425BBE5}, 2
    const FMTID: GUID = GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5);
    const BATTERY: DEVPROPKEY = DEVPROPKEY { fmtid: FMTID, pid: 2 };
    // 同一 GUID 下的 7 号属性是更新时间；停在旧日期的值就是假电量。
    const UPDATED: DEVPROPKEY = DEVPROPKEY { fmtid: FMTID, pid: 7 };

    let mut nodes: Vec<(String, Option<u8>, Option<u64>)> = Vec::new();
    unsafe {
        let set = SetupDiGetClassDevsW(None, PCWSTR::null(), HWND::default(), DIGCF_PRESENT | DIGCF_ALLCLASSES)
            .map_err(|_| ())?;
        for i in 0..u32::MAX {
            let mut data = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(set, i, &mut data).is_err() {
                break;
            }
            let Some(instance) = instance_id(set, &data) else { continue };
            let upper = instance.to_ascii_uppercase();
            if !devices.iter().any(|device|upper.contains(&device.id)){continue;}
            nodes.push((upper, byte_prop(set, &data, &BATTERY), u64_prop(set, &data, &UPDATED)));
        }
        // 每次枚举后释放设备列表句柄。
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    let now = filetime_now();
    Ok(devices.iter().map(|device| {
        let matched: Vec<_> = nodes.iter().filter(|(node,_,_)| node.contains(&device.id)).collect();
        let pnp = matched.iter().find_map(|(_,level,updated)| fresh_pnp_level(*level,*updated,now));
        let pnp_note = if pnp.is_some() {
            String::new()
        } else if matched.iter().any(|(_,level,_)| level.is_some()) {
            "PnP 缓存已过期".to_string()
        } else {
            "PnP 无 Battery 属性".to_string()
        };
        let gatt: Result<u8,&'static str> = if device.le {
            let id = device.id.clone();
            with_timeout(Duration::from_secs(3), move || gatt_battery(&id))
        } else {
            Err("经典蓝牙没有 GATT 电量")
        };
        match gatt {
            Ok(level) => BatteryReading { id:device.id.clone(), level:Some(level), route:BatteryRoute::BleGatt, note:String::new() },
            Err(gatt_note) => BatteryReading {
                id:device.id.clone(), level:pnp, route:BatteryRoute::Pnp,
                note:if pnp.is_some() {format!("GATT：{gatt_note}")} else {format!("GATT：{gatt_note}；{pnp_note}")},
            },
        }
    }).collect())
}

/// PnP 电量只有在「1..=100 且更新时间不超过 24 小时」时才可信。
pub(crate) fn fresh_pnp_level(level:Option<u8>,updated:Option<u64>,now:u64)->Option<u8>{
    const MAX_AGE_100NS:u64=24*60*60*10_000_000;
    let level=level.filter(|value|(1..=100).contains(value))?;
    let updated=updated?;
    (now.saturating_sub(updated)<=MAX_AGE_100NS).then_some(level)
}

fn filetime_now()->u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed|elapsed.as_secs()*10_000_000+(elapsed.subsec_nanos()/100) as u64+116_444_736_000_000_000)
        .unwrap_or(0)
}

/// WinRT 的 `.get()` 没有超时，放到独立线程里等待，避免拖住蓝牙扫描线程。
#[cfg(windows)]
fn with_timeout<T:Send+'static>(limit:Duration, task:impl FnOnce()->Result<T,&'static str>+Send+'static)->Result<T,&'static str>{
    let (tx,rx)=std::sync::mpsc::sync_channel(1);
    if std::thread::Builder::new().name("daozai-gatt".into()).spawn(move||{let _=tx.send(task());}).is_err(){
        return Err("无法启动 GATT 线程");
    }
    match rx.recv_timeout(limit) {Ok(result)=>result,Err(_)=>Err("GATT 超时")}
}

#[cfg(windows)]
fn gatt_battery(id:&str)->Result<u8,&'static str> {
    use windows::core::{GUID, Interface};
    use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};
    use windows::Devices::Bluetooth::GenericAttributeProfile::GattCommunicationStatus;
    use windows::Win32::System::WinRT::IBufferByteAccess;

    const BATTERY_SERVICE: GUID = GUID::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb);
    const BATTERY_LEVEL: GUID = GUID::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb);

    let address = u64::from_str_radix(id,16).map_err(|_|"设备地址无效")?;
    // 后台线程没有现成的 COM 套间，显式初始化为 MTA（重复初始化会返回错误，可忽略）。
    unsafe {let _=windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);}
    let device = BluetoothLEDevice::FromBluetoothAddressAsync(address).map_err(|_|"创建 BLE 设备失败")?
        .get().map_err(|_|"连接 BLE 设备失败")?;
    let services = device.GetGattServicesWithCacheModeAsync(BluetoothCacheMode::Uncached).map_err(|_|"枚举 GATT 服务失败")?
        .get().map_err(|_|"枚举 GATT 服务超时")?;
    let service = services.Services().map_err(|_|"读取服务列表失败")?
        .into_iter().find(|service|service.Uuid().ok()==Some(BATTERY_SERVICE))
        .ok_or("无 180F 电量服务")?;
    let characteristics = service.GetCharacteristicsWithCacheModeAsync(BluetoothCacheMode::Uncached).map_err(|_|"枚举电量特征失败")?
        .get().map_err(|_|"枚举电量特征超时")?;
    let characteristic = characteristics.Characteristics().map_err(|_|"读取特征列表失败")?
        .into_iter().find(|characteristic|characteristic.Uuid().ok()==Some(BATTERY_LEVEL))
        .ok_or("无 2A19 电量特征")?;
    let value = characteristic.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached).map_err(|_|"读取电量失败")?
        .get().map_err(|_|"读取电量超时")?;
    if value.Status().map_err(|_|"读取状态不可用")? != GattCommunicationStatus::Success {
        return Err("读取状态非 Success");
    }
    let buffer = value.Value().map_err(|_|"电量值为空")?;
    let len = buffer.Length().map_err(|_|"电量长度不可用")? as usize;
    let access:IBufferByteAccess = buffer.cast().map_err(|_|"电量缓冲不可读")?;
    let data = unsafe {std::slice::from_raw_parts(access.Buffer().map_err(|_|"电量缓冲不可读")?,len)};
    let level = *data.first().ok_or("电量值为空")?;
    if !(1..=100).contains(&level) {return Err("电量值无效");}
    Ok(level)
}

#[cfg(windows)]
unsafe fn instance_id(
    set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    data: &windows::Win32::Devices::DeviceAndDriverInstallation::SP_DEVINFO_DATA,
) -> Option<String> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::SetupDiGetDeviceInstanceIdW;
    let mut buf = [0u16; 512];
    let mut need = 0u32;
    SetupDiGetDeviceInstanceIdW(set, data, Some(&mut buf), Some(&mut need)).ok()?;
    Some(String::from_utf16_lossy(&buf[..(need as usize).saturating_sub(1)]))
}

#[cfg(windows)]
unsafe fn byte_prop(
    set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    data: &windows::Win32::Devices::DeviceAndDriverInstallation::SP_DEVINFO_DATA,
    key: &windows::Win32::Devices::Properties::DEVPROPKEY,
) -> Option<u8> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::SetupDiGetDevicePropertyW;
    use windows::Win32::Devices::Properties::DEVPROPTYPE;
    // DEVPROP_TYPE_BYTE
    const BYTE: DEVPROPTYPE = DEVPROPTYPE(0x3);
    let mut ty = DEVPROPTYPE::default();
    let mut buf = [0u8; 1];
    let mut size = 0u32;
    SetupDiGetDevicePropertyW(set, data, key, &mut ty, Some(&mut buf), Some(&mut size), 0).ok()?;
    (ty == BYTE && size == 1).then_some(buf[0])
}

/// 读取 8 字节属性（更新时间可以是 FILETIME 或 UINT64）。
#[cfg(windows)]
unsafe fn u64_prop(
    set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    data: &windows::Win32::Devices::DeviceAndDriverInstallation::SP_DEVINFO_DATA,
    key: &windows::Win32::Devices::Properties::DEVPROPKEY,
) -> Option<u64> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::SetupDiGetDevicePropertyW;
    use windows::Win32::Devices::Properties::DEVPROPTYPE;
    const UINT64: DEVPROPTYPE = DEVPROPTYPE(0x9);
    const FILETIME: DEVPROPTYPE = DEVPROPTYPE(0x10);
    let mut ty = DEVPROPTYPE::default();
    let mut buf = [0u8; 8];
    let mut size = 0u32;
    SetupDiGetDevicePropertyW(set, data, key, &mut ty, Some(&mut buf), Some(&mut size), 0).ok()?;
    ((ty == UINT64 || ty == FILETIME) && size == 8).then(|| u64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id:&str)->Device {Device{id:id.into(),name:id.into(),..Device::default()}}
    fn reading(id:&str,level:Option<u8>)->BatteryReading {BatteryReading{id:id.into(),level,route:BatteryRoute::Pnp,note:String::new()}}

    #[test]
    fn idle_bluetooth_does_not_rescan_all_hardware_every_two_seconds(){
        let start=std::time::Instant::now();let mut cache=Cache::default();let mut scans=0;
        for second in (0..360).step_by(2){
            let found=cache.update(vec![device("AA")],start+Duration::from_secs(second),|_|{scans+=1;Ok(vec![reading("AA",Some(80))])});
            assert_eq!(found[0].battery,Some(80));
        }
        assert_eq!(scans,6,"六分钟内同一设备的电量扫描应最多每分钟一次");
    }

    #[test]
    fn bluetooth_connection_change_refreshes_immediately_and_failed_scans_back_off(){
        let now=std::time::Instant::now();let mut cache=Cache::default();
        cache.update(vec![device("AA")],now,|_|Ok(vec![reading("AA",Some(80))]));
        let found=cache.update(vec![device("AA"),device("BB")],now+Duration::from_secs(2),|_|Ok(vec![reading("AA",Some(79)),reading("BB",Some(60))]));
        assert_eq!(found[1].battery,Some(60));
        let found=cache.update(vec![device("AA"),device("BB")],now+Duration::from_secs(62),|_|Err(()));
        assert_eq!(found[0].battery,Some(79));
        cache.update(vec![device("AA"),device("BB")],now+Duration::from_secs(64),|_|panic!("失败后不能立即重复扫描"));
        assert!(cache.update(vec![],now+Duration::from_secs(66),|_|panic!("无设备不能扫描整机")).is_empty());
        cache.update(vec![device("AA")],now+Duration::from_secs(68),|_|Ok(vec![reading("AA",Some(78))]));
        assert_eq!(cache.devices[0].battery,Some(78));
    }

    #[test]
    fn identifies_connected_audio_and_input_devices() {
        assert_eq!(device_kind("OpenMove by Shokz", true, Some(0x240418)), DeviceKind::Bone);
        assert_eq!(device_kind("QingLong 87 Ultra 8K", false, Some(961)), DeviceKind::Keyboard);
        assert_eq!(device_kind("MX Anywhere", false, Some(962)), DeviceKind::Mouse);
        assert_eq!(device_kind("耳机", true, Some(0x0404)), DeviceKind::Headphones);
        assert_eq!(device_kind("AirPods", true, None), DeviceKind::Earbuds);
        assert_eq!(device_kind("未知设备", false, None), DeviceKind::Other);
    }

    #[test]
    fn pnp_battery_needs_a_recent_update_time(){
        const DAY:u64=24*60*60*10_000_000;
        let now=1_700_000_000_000_000_000u64;
        assert_eq!(fresh_pnp_level(Some(2),Some(now-DAY/2),now),Some(2),"近期的值可以采信");
        assert_eq!(fresh_pnp_level(Some(2),Some(now-200*DAY),now),None,"停在旧日期的假电量必须丢弃");
        assert_eq!(fresh_pnp_level(Some(2),None,now),None,"没有更新时间也丢弃");
        assert_eq!(fresh_pnp_level(Some(0),Some(now),now),None,"0 表示未知");
        assert_eq!(fresh_pnp_level(Some(101),Some(now),now),None,"超出范围");
    }

    #[test]
    fn summary_omits_missing_battery() {
        let d = Device { id: "1".into(), name: "Headset".into(), kind: DeviceKind::Headphones, ..Device::default() };
        assert_eq!(d.summary(), "Headset");
        let d = Device { battery: Some(80), ..d };
        assert_eq!(d.summary(), "Headset · 电量 80%");
    }
}

#[cfg(test)]
mod lifecycle_tests {
    #[test]
    fn dropping_service_signals_stop_even_without_successful_scans(){
        let (_tx,rx)=std::sync::mpsc::channel();
        let (stop,stopped)=std::sync::mpsc::channel();
        drop(super::Service {rx,stop});
        assert_eq!(stopped.recv_timeout(std::time::Duration::from_millis(100)),Ok(()));
    }
}
