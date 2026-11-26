//! 迈从 MCHOSE 2.4G 接收器直连电量（实测 A7 V3 Pro 接收器 PID `0x1018`）。
//!
//! 接收器在系统里是普通 USB HID 复合设备，不走蓝牙栈，所以 `bt::windows` 看不到它；
//! 打开它的厂商集合（usage page `0xFF01`）发一帧「读设备信息」，回帧 `[20]` 就是电量。
//! 命令与响应帧来自 M HUB 驱动（报表 `0x4D`）的实测抓包，真机多次一致。
//!
//! # 为什么这里没有定时轮询
//!
//! 这条查询会经 2.4G 转发给鼠标，鼠标固件收到报文会重置空闲计时器——也就是说
//! **查询本身就会把鼠标叫醒**，和有没有常驻句柄无关。2026-11 的对照实验：
//! 常驻句柄 + 每 10 秒轮询，鼠标整夜不休眠；改成只在用户主动看时查一次，
//! 休眠立刻恢复正常。所以 `Service::refresh()` 是唯一入口，线程平时阻塞在
//! `recv()` 上，对设备零访问；界面只在「打开无线设备页」「悬停常驻徽标」时调用。
//!
//! 读数缓存在进程内，`receivers` 每轮枚举（2 秒，只读 PnP 设备树，不碰设备）
//! 时取一次，因此界面最多晚 2 秒看到新读数。

use std::{
    collections::BTreeMap,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, OnceLock,
    },
    time::Instant,
};

const VENDOR: u16 = 0x3837;
/// 已实测的接收器型号。新型号要等抓包验证后再加，避免给出错误电量。
const PRODUCTS: &[u16] = &[0x1018];
/// 查询帧：报表 `0x4D` + `01 01 00 00 09 00 00 08`（读设备信息）。
const QUERY: [u8; 9] = [0x4D, 0x01, 0x01, 0x00, 0x00, 0x09, 0x00, 0x00, 0x08];
/// 报表 `0x4D` 的报文长度（含报表 ID 字节）。
const REPORT_LEN: usize = 64;
/// 响应帧布局：`4D 01 01 10 00 …`，`[18]` 连接方式、`[19]` 充电、`[20]` 电量百分比。
const REPLY_CMD: u8 = 0x10;
const CONNECT_AT: usize = 18;
const CHARGE_AT: usize = 19;
const BATTERY_AT: usize = 20;

/// 一次成功读取的结果，供 `receivers` 组装成 `Device`。
#[derive(Clone, Debug)]
pub(crate) struct Link {
    pub percent: u8,
    pub charging: Option<bool>,
    /// 接收器的产品名（HID product string），比兼容表里的通用名更具体。
    pub product: String,
    pub observed: Instant,
    /// 同一读数连续保持的起始时间，用于「读数已保持 …」这类文案。
    pub value_since: Instant,
}

fn links() -> &'static Mutex<BTreeMap<u16, Link>> {
    static LINKS: OnceLock<Mutex<BTreeMap<u16, Link>>> = OnceLock::new();
    LINKS.get_or_init(Default::default)
}

/// 取某个接收器（按 VID/PID）最近一次读到的电量。没有读数返回 `None`。
pub(crate) fn link(vid: u16, pid: u16) -> Option<Link> {
    if vid != VENDOR || !PRODUCTS.contains(&pid) {
        return None;
    }
    links().lock().ok()?.get(&pid).cloned()
}

/// 只在用户主动要看时才请求一次查询；`Drop` 时关闭线程。
pub struct Service {
    request: Sender<()>,
}

impl Service {
    /// 请求读一次。同一个请求连发多次会被后台线程按顺序处理，不会并发。
    pub fn refresh(&self) {
        let _ = self.request.send(());
    }
}

pub fn spawn() -> Service {
    let (request, requested) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("daozai-2.4g".into())
        .spawn(move || run(requested));
    Service { request }
}

fn run(requested: Receiver<()>) {
    #[cfg(windows)]
    unsafe {
        // 句柄只在第一次被请求时打开，之后常驻复用（和 M HUB 一样）。
        let mut open: Vec<Open> = Vec::new();
        // 主循环唯一的等待点：没有请求就完全不动设备。
        while requested.recv().is_ok() {
            if open.is_empty() {
                open = connect();
            }
            if open.is_empty() {
                continue;
            }
            let mut alive = Vec::with_capacity(open.len());
            for entry in open.drain(..) {
                match query(entry.handle) {
                    Some((percent, charging)) => {
                        remember(entry.pid, percent, charging, &entry.product);
                        alive.push(entry);
                    }
                    // 问不出来（鼠标睡着了）：保留上一次读数，关掉句柄下次重开。
                    None => drop(entry),
                }
            }
            open = alive;
        }
        for entry in open {
            drop(entry);
        }
    }
    #[cfg(not(windows))]
    let _ = requested;
}

/// 更新缓存；电量没变时保留原来的 `value_since`。
#[cfg(windows)]
fn remember(pid: u16, percent: u8, charging: Option<bool>, product: &str) {
    let Ok(mut links) = links().lock() else { return };
    let now = Instant::now();
    let since = links
        .get(&pid)
        .filter(|old| old.percent == percent)
        .map_or(now, |old| old.value_since);
    links.insert(
        pid,
        Link {
            percent,
            charging,
            product: product.to_string(),
            observed: now,
            value_since: since,
        },
    );
}

#[cfg(windows)]
struct Open {
    pid: u16,
    product: String,
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: 句柄由 `CreateFileW` 打开，且只在这里关闭一次。
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct Dongle {
    path: String,
    pid: u16,
    product: String,
}

/// 枚举所有已实测型号的接收器并打开可读写句柄。
#[cfg(windows)]
unsafe fn connect() -> Vec<Open> {
    let mut open = Vec::new();
    for dongle in find_dongles() {
        if let Some(handle) = open_rw(&dongle.path) {
            open.push(Open {
                pid: dongle.pid,
                product: dongle.product,
                handle,
            });
        }
    }
    open
}

/// 从回帧解出电量与充电状态；非「设备信息」响应、或电量越界时返回 `None`。
#[cfg(windows)]
fn parse(frame: &[u8]) -> Option<(u8, Option<bool>)> {
    if frame.len() <= BATTERY_AT || frame[0] != 0x4D {
        return None;
    }
    // 响应以 `4D 01 01 10 00` 开头；其它命令的响应不带电量。
    if frame[3] != REPLY_CMD || frame[4] != 0x00 {
        return None;
    }
    // 连接方式非 0 才说明接收器真的回话了（发错命令时接收器会原样回显并清零数据区）。
    if frame[CONNECT_AT] == 0 {
        return None;
    }
    let percent = frame[BATTERY_AT];
    if !(1..=100).contains(&percent) {
        return None;
    }
    // `[19]` 实测 0 表示没在充电、1 表示充电中；必要时也可当作未知。
    Some((percent, Some(frame[CHARGE_AT] != 0)))
}

#[cfg(windows)]
unsafe fn find_dongles() -> Vec<Dongle> {
    use windows::core::GUID;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;

    const HID_INTERFACE: GUID = GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);
    let mut found = Vec::new();
    let Ok(set) = SetupDiGetClassDevsW(
        Some(&HID_INTERFACE),
        None,
        windows::Win32::Foundation::HWND::default(),
        DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
    ) else {
        return found;
    };
    for index in 0..u32::MAX {
        let mut iface = SP_DEVICE_INTERFACE_DATA {
            cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        if SetupDiEnumDeviceInterfaces(set, None, &HID_INTERFACE, index, &mut iface).is_err() {
            break;
        }
        let Some(path) = interface_path(set, &iface) else {
            continue;
        };
        if let Some(dongle) = describe(&path) {
            found.push(dongle);
        }
    }
    let _ = SetupDiDestroyDeviceInfoList(set);
    found
}

#[cfg(windows)]
unsafe fn interface_path(
    set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    iface: &windows::Win32::Devices::DeviceAndDriverInstallation::SP_DEVICE_INTERFACE_DATA,
) -> Option<String> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;

    let mut need = 0u32;
    // 第一次调用必然「失败」，只用来拿所需长度。
    let _ = SetupDiGetDeviceInterfaceDetailW(set, iface, None, 0, Some(&mut need), None);
    if need as usize
        <= std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>()
    {
        return None;
    }
    let mut buf = vec![0u8; need as usize];
    let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
    detail.write(SP_DEVICE_INTERFACE_DETAIL_DATA_W {
        cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32,
        DevicePath: [0],
    });
    if SetupDiGetDeviceInterfaceDetailW(set, iface, Some(detail), need, None, None).is_err() {
        return None;
    }
    // 路径紧跟 cbSize，按 UTF-16 读到 NUL 为止。
    let path = buf[std::mem::size_of::<u32>()..]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&c| c != 0)
        .collect::<Vec<u16>>();
    Some(String::from_utf16_lossy(&path))
}

#[cfg(windows)]
fn open_rw(path: &str) -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    unsafe {
        CreateFileW(
            &HSTRING::from(path),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
        .ok()
    }
}

/// 打开一次读 VID/PID 与产品名，随即关闭（枚举阶段，不参与查询）。
#[cfg(windows)]
unsafe fn describe(path: &str) -> Option<Dongle> {
    use windows::Win32::Devices::HumanInterfaceDevice::*;
    use windows::Win32::Foundation::CloseHandle;

    let handle = open_rw(path)?;
    let checked = (|| {
        let mut attr = HIDD_ATTRIBUTES {
            Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
            ..Default::default()
        };
        if !HidD_GetAttributes(handle, &mut attr).as_bool() {
            return None;
        }
        if attr.VendorID != VENDOR || !PRODUCTS.contains(&attr.ProductID) {
            return None;
        }
        let mut name = [0u16; 128];
        let mut product = String::new();
        if HidD_GetProductString(handle, name.as_mut_ptr() as *mut std::ffi::c_void, name.len() as u32)
            .as_bool()
        {
            let len = name.iter().position(|&c| c == 0).unwrap_or(0);
            product = String::from_utf16_lossy(&name[..len]).trim().to_string();
        }
        Some(Dongle {
            path: path.into(),
            pid: attr.ProductID,
            product,
        })
    })();
    let _ = CloseHandle(handle);
    checked
}

/// 在一个句柄上问一次电量：写查询帧，再读回帧（最多四帧，跳过事件帧）。
#[cfg(windows)]
unsafe fn query(handle: windows::Win32::Foundation::HANDLE) -> Option<(u8, Option<bool>)> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows::Win32::System::IO::{CancelIo, OVERLAPPED};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    let event = CreateEventW(None, false, false, None).ok()?;
    let result = (|| {
        let mut report = [0u8; REPORT_LEN];
        report[..QUERY.len()].copy_from_slice(&QUERY);
        let mut ov = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        // 同步失败时事件不会触发，下面的超时兜底。
        let _ = WriteFile(handle, Some(&report), None, Some(&mut ov));
        if WaitForSingleObject(event, 300) != WAIT_OBJECT_0 {
            let _ = CancelIo(handle);
            return None;
        }
        for _ in 0..4 {
            let mut buf = [0u8; REPORT_LEN];
            let mut ov = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            let _ = ReadFile(handle, Some(&mut buf[..]), None, Some(&mut ov));
            if WaitForSingleObject(event, 400) != WAIT_OBJECT_0 {
                let _ = CancelIo(handle);
                return None;
            }
            if let Some(found) = parse(&buf) {
                return Some(found);
            }
        }
        None
    })();
    let _ = CloseHandle(event);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-13 从 M HUB 侧实测抓到的完整设备信息响应（当时电量 83%）。
    fn real_reply() -> Vec<u8> {
        vec![
            0x4D, 0x01, 0x01, 0x10, 0x00, 0x09, 0x00, 0x00, 0x37, 0x38, 0x26, 0x40, 0x04, 0x00,
            0x00, 0x00, 0x00, 0x10, 0x02, 0x00, 0x53, 0x00, 0x08, 0xE4, 0xD8, 0x00,
        ]
    }

    #[cfg(windows)]
    #[test]
    fn decodes_real_a7_v3_pro_frame() {
        assert_eq!(parse(&real_reply()), Some((83, Some(false))));
        assert_eq!(real_reply()[CONNECT_AT], 2, "2.4G");
    }

    #[cfg(windows)]
    #[test]
    fn tracks_battery_and_charging_fields() {
        for level in [1u8, 47, 84, 100] {
            let mut frame = real_reply();
            frame[BATTERY_AT] = level;
            assert_eq!(parse(&frame).map(|(level, _)| level), Some(level));
        }
        let mut charging = real_reply();
        charging[CHARGE_AT] = 1;
        assert_eq!(parse(&charging), Some((83, Some(true))));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_echo_and_foreign_frames() {
        // 发错命令时接收器原样回显并清零数据区——这正是过去“读出 0”的原因。
        let mut echo = vec![0x4D, 0x01, 0x01, 0x00, 0x00];
        echo.resize(64, 0);
        assert_eq!(parse(&echo), None);

        // 其它命令的响应（如键位配置 0x13）不带电量。
        let mut other = real_reply();
        other[3] = 0x13;
        assert_eq!(parse(&other), None);

        // 数据区全零的响应视为无效。
        let mut zeroed = real_reply();
        zeroed[BATTERY_AT] = 0;
        assert_eq!(parse(&zeroed), None);

        // 越界电量（回声里常见 0xFF）不采信。
        let mut over = real_reply();
        over[BATTERY_AT] = 0xFF;
        assert_eq!(parse(&over), None);

        // 事件帧（实测 `4D 01 01 0D`）也不带电量。
        let mut event = vec![0x4D, 0x01, 0x01, 0x0D, 0x00, 0x06];
        event.resize(64, 0);
        assert_eq!(parse(&event), None);

        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0x4D, 0x01, 0x01]), None);
    }

    #[test]
    fn query_is_full_length_report() {
        assert_eq!(QUERY[0], 0x4D);
        assert!(REPORT_LEN > QUERY.len());
    }
}