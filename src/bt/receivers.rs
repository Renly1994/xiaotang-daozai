//! 已识别的 2.4G USB 接收器。拔插查询独立于蓝牙枚举。
use super::compat::{builtin_receivers, Package, Store};
use super::{mchose, BatteryReading, BatterySource, Device, DeviceKind};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{w, GUID},
    Win32::Devices::DeviceAndDriverInstallation::*,
};

const GUID_DEVINTERFACE_USB_DEVICE: GUID = GUID::from_u128(0xa5dcbf10_6530_11d2_901f_00c04fb951ed);

fn receiver_snapshot(
    pack: Option<&Package>,
    instances: impl IntoIterator<Item = impl AsRef<str>>,
) -> Vec<Device> {
    let builtin = builtin_receivers();
    let mut devices: Vec<_> = instances
        .into_iter()
        .filter_map(|instance| {
            let instance = instance.as_ref().to_ascii_uppercase();
            let (hardware, serial_or_path) = instance.strip_prefix("USB\\")?.split_once('\\')?;
            if serial_or_path.is_empty() || serial_or_path.contains('\\') {
                return None;
            }
            // 只匹配完整 USB 设备的硬件 ID，复合设备的 MI_* 子接口不会单独出现。
            let name = builtin
                .receiver_name(hardware)
                .or_else(|| pack?.receiver_name(hardware))?;
            let mut device = Device {
                id: format!("usb:{instance}"),
                name: name.into(),
                kind: DeviceKind::Receiver24G,
                ..Default::default()
            };
            // 厂商协议能给电量的接收器（迈从）：补上读数和更具体的产品名。
            if let Some((vid, pid)) = usb_pair(hardware) {
                if let Some(link) = mchose::link(vid, pid) {
                    if !link.product.trim().is_empty() {
                        device.name = link.product.trim().to_string();
                    }
                    device.battery = Some(link.percent);
                    device.reading = Some(BatteryReading {
                        percent: link.percent,
                        source: BatterySource::Receiver,
                        observed: link.observed,
                        value_since: link.value_since,
                        charging: link.charging,
                    });
                }
            }
            Some(device)
        })
        .collect();
    devices.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    devices.dedup_by(|a, b| a.id == b.id);
    devices
}

/// 从 `VID_3837&PID_1018` 这样的硬件 ID 里取 VID/PID。
fn usb_pair(hardware: &str) -> Option<(u16, u16)> {
    let hex = |text: &str| u16::from_str_radix(text, 16).ok();
    let mut vid = None;
    let mut pid = None;
    for part in hardware.split('&') {
        if let Some(text) = part.strip_prefix("VID_") {
            vid = hex(text);
        } else if let Some(text) = part.strip_prefix("PID_") {
            pid = hex(text);
        }
    }
    Some((vid?, pid?))
}

fn present_usb_instances() -> Option<Vec<String>> {
    const FLAGS: u32 = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
    // 拔插发生在查询长度和读取之间时，PnP 可能要求扩大缓冲区。
    for _ in 0..3 {
        let mut size = 0;
        if unsafe { CM_Get_Device_ID_List_SizeW(&mut size, w!("USB"), FLAGS) } != CR_SUCCESS {
            return None;
        }
        let mut buffer = vec![0; size as usize];
        match unsafe { CM_Get_Device_ID_ListW(w!("USB"), &mut buffer, FLAGS) } {
            CR_SUCCESS => {
                let ids = buffer
                    .split(|&ch| ch == 0)
                    .take_while(|id| !id.is_empty())
                    .map(String::from_utf16_lossy)
                    .collect();
                return Some(ids);
            }
            CR_BUFFER_SMALL => continue,
            _ => return None,
        }
    }
    None
}

struct Notification {
    handle: HCMNOTIFICATION,
    sender: *mut SyncSender<()>,
}

impl Notification {
    fn register(sender: SyncSender<()>) -> Option<Self> {
        let mut filter = CM_NOTIFY_FILTER {
            cbSize: std::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            ..Default::default()
        };
        filter.u.DeviceInterface = CM_NOTIFY_FILTER_0_2 {
            ClassGuid: GUID_DEVINTERFACE_USB_DEVICE,
        };
        let mut handle = HCMNOTIFICATION::default();
        let sender = Box::into_raw(Box::new(sender));
        let result = unsafe {
            CM_Register_Notification(
                &filter,
                Some(sender.cast()),
                Some(on_usb_change),
                &mut handle,
            )
        };
        if result == CR_SUCCESS {
            return Some(Self { handle, sender });
        }
        unsafe {
            drop(Box::from_raw(sender));
        }
        None
    }
}

impl Drop for Notification {
    fn drop(&mut self) {
        // 注销成功会等回调结束。失败时保留回调上下文，避免系统仍访问已释放的发送端。
        unsafe {
            if CM_Unregister_Notification(self.handle) == CR_SUCCESS {
                drop(Box::from_raw(self.sender));
            }
        }
    }
}

unsafe extern "system" fn on_usb_change(
    _: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    _: *const CM_NOTIFY_EVENT_DATA,
    _: u32,
) -> u32 {
    if matches!(
        action,
        CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL | CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL
    ) {
        // 发送端在注销成功后释放。回调只唤醒工作线程，把 PnP 查询留给工作线程。
        let sender = unsafe { &*context.cast::<SyncSender<()>>() };
        let _ = sender.try_send(());
    }
    0
}

pub(super) fn run(
    tx: SyncSender<Vec<Device>>,
    stop: Arc<AtomicBool>,
    store: Store,
    wake: impl Fn(),
) {
    let (events, changes) = mpsc::sync_channel(1);
    let notification = Notification::register(events.clone());
    // 接口事件可能先于设备树更新。轻量的 USB 枚举为先到的通知和注册失败兜底。
    let scan_interval = Duration::from_secs(if notification.is_some() { 2 } else { 1 });
    run_scans(
        tx,
        stop,
        store,
        wake,
        changes,
        scan_interval,
        present_usb_instances,
    );
}

fn run_scans(
    tx: SyncSender<Vec<Device>>,
    stop: Arc<AtomicBool>,
    store: Store,
    wake: impl Fn(),
    changes: mpsc::Receiver<()>,
    scan_interval: Duration,
    scan: impl Fn() -> Option<Vec<String>>,
) {
    let mut active = store.package();
    let mut next_scan = Instant::now();
    let mut instances = None;
    let mut latest = None;
    let mut published = None;
    while !stop.load(Ordering::Relaxed) {
        let selected = store.package();
        let changed = match (&selected, &active) {
            (Some(selected), Some(active)) => !Arc::ptr_eq(selected, active),
            _ => selected.is_some() != active.is_some(),
        };
        if Instant::now() >= next_scan || changed {
            active = selected;
            if let Some(found) = scan() {
                instances = Some(found);
            }
            // 启停和更新按最新识别表重算，刚好枚举失败时也能移除已停用的扩展项。
            latest = instances
                .as_ref()
                .map(|ids| receiver_snapshot(active.as_deref(), ids));
            next_scan = Instant::now() + scan_interval;
        }
        if latest != published {
            if let Some(devices) = &latest {
                match tx.try_send(devices.clone()) {
                    Ok(()) => {
                        published.clone_from(&latest);
                        wake();
                    }
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
        }
        if changes.recv_timeout(Duration::from_millis(200)).is_ok() {
            next_scan = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bt::compat::builtin_data;
    use serde_json::json;

    #[test]
    fn receiver_snapshot_native_catalog_covers_all_without_plugin() {
        let rows = builtin_data();
        let instances: Vec<_> = rows
            .iter()
            .map(|row| {
                format!(
                    r"USB\VID_{}&PID_{}\DEVICE",
                    row["vid"].as_str().unwrap(),
                    row["pid"].as_str().unwrap()
                )
            })
            .collect();
        let devices = receiver_snapshot(None, &instances);
        assert_eq!(devices.len(), rows.len());
        for (instance, row) in instances.iter().zip(rows) {
            let device = devices
                .iter()
                .find(|device| device.id == format!("usb:{instance}"))
                .unwrap();
            assert_eq!(Some(device.name.as_str()), row["name"].as_str());
            // 没有实测报文前，接收器只报连接、不报电量。
            assert_eq!(device.battery, None);
            assert_eq!(device.kind, DeviceKind::Receiver24G);
        }
    }

    #[test]
    fn usb_pair_reads_vid_pid_from_hardware_id() {
        assert_eq!(usb_pair("VID_3837&PID_1018"), Some((0x3837, 0x1018)));
        assert_eq!(usb_pair("VID_046D&PID_C548"), Some((0x046D, 0xC548)));
        assert_eq!(usb_pair("VID_2717&PID_5055"), Some((0x2717, 0x5055)));
        assert_eq!(usb_pair("VID_2717"), None);
        assert_eq!(usb_pair(""), None);
    }

    #[test]
    fn mchose_battery_needs_a_real_reading() {
        // 缓存里没有读数时，即使硬件 ID 是迈从也不编造电量。
        assert!(mchose::link(0x3837, 0x1018).is_none());
        assert!(mchose::link(0x046D, 0xC548).is_none());
        let devices = receiver_snapshot(None, [r"USB\VID_3837&PID_1018\0123456789"]);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].battery, None);
        assert_eq!(devices[0].reading, None);
    }

    /// 真机验收：插着迈从接收器时手动跑
    /// `cargo test --lib --features sdk-js bt::receivers::tests::live -- --ignored --nocapture`
    /// 只按需读一次（和界面一致），不做任何轮询。
    #[test]
    #[ignore]
    fn live_mchose_receiver_reports_battery() {
        let service = mchose::spawn();
        service.refresh();
        let mut found = None;
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(100));
            if let Some(link) = mchose::link(0x3837, 0x1018) {
                found = Some(link);
                break;
            }
        }
        let Some(link) = found else {
            panic!("等待期间没有读到迈从接收器电量（接收器没插或鼠标在休眠）");
        };
        println!(
            "迈从接收器：{} · {}% · 充电={:?}",
            link.product, link.percent, link.charging
        );
        let instances = present_usb_instances().expect("枚举 USB 设备失败");
        let devices = receiver_snapshot(None, &instances);
        let device = devices
            .iter()
            .find(|device| device.id.contains("VID_3837&PID_1018"))
            .expect("接收器没有出现在快照里");
        assert_eq!(device.battery, Some(link.percent));
        assert_eq!(
            device.reading.as_ref().map(|reading| reading.source),
            Some(BatterySource::Receiver)
        );
        assert_eq!(device.kind, DeviceKind::Receiver24G);
        println!("快照：{} · {:?}%", device.name, device.battery);
    }

    #[test]
    fn receiver_worker_tracks_compat_pack_lifecycle() {
        let store = Store::default();
        let scans = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::sync_channel(1);
            let (_events, changes) = mpsc::sync_channel(1);
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let service = crate::bt::Service { rx, stop };
            let worker_store = store.clone();
            let scans = &scans;
            scope.spawn(move || {
                run_scans(
                    tx,
                    worker_stop,
                    worker_store,
                    || {},
                    changes,
                    Duration::from_secs(60),
                    || {
                        let count = scans.fetch_add(1, Ordering::Relaxed);
                        // 首次枚举成功。之后模拟 PnP 读取失败，验证启停仍能重新匹配实体列表。
                        (count == 0).then(|| {
                            [
                                r"USB\VID_2717&PID_5055\DEVICE_2",
                                r"USB\VID_046D&PID_C542\DEVICE_1",
                                r"usb\vid_046d&pid_c542\device_1",
                                r"USB\VID_046D&PID_C542&MI_00\INTERFACE_1",
                            ]
                            .map(str::to_owned)
                            .to_vec()
                        })
                    },
                );
            });

            let snapshot = || service.rx.recv_timeout(Duration::from_secs(3)).unwrap();
            let baseline = snapshot();
            assert_eq!(baseline.len(), 1);
            assert_eq!(baseline[0].name, "小米无线键鼠套装 2");
            let mut data = json!({
                "id":"daozai.wireless-receivers","api_version":1,"version":"1.1.0",
                "receivers":[
                    {"vid":"2717","pid":"5055","name":"兼容包里的同型号名称"},
                    {"vid":"046D","pid":"C542","name":"罗技 Nano 接收器"}
                ]
            });
            let parse = |data: &serde_json::Value| {
                Package::parse(&serde_json::to_vec(data).unwrap()).unwrap()
            };
            store.set(Some(parse(&data)));

            let updated = snapshot();
            assert_eq!(updated.len(), 2);
            assert_eq!(updated[0].name, "罗技 Nano 接收器");
            assert_eq!(updated[0].id, r"usb:USB\VID_046D&PID_C542\DEVICE_1");
            assert_eq!(updated[1], baseline[0]);
            assert_eq!(store.package().unwrap().version(), "1.1.0");

            data["version"] = json!("1.2.0");
            data["receivers"][1]["name"] = json!("罗技 Nano 无线接收器");
            store.set(Some(parse(&data)));
            let latest = snapshot();
            assert_eq!(latest[0].name, "罗技 Nano 无线接收器");
            assert_eq!(latest[1], baseline[0]);

            store.set(None);
            assert_eq!(snapshot(), baseline);
            store.set(Some(parse(&data)));
            assert_eq!(snapshot(), latest);
            store.set(None);
            assert_eq!(snapshot(), baseline);
            assert!(store.package().is_none());
        });
    }

    #[test]
    #[ignore = "需本机插有小米无线键鼠套装 2 的接收器"]
    fn receiver_live_usb_builtin_survives_plugin_lifecycle() {
        let store = Store::default();
        let assert_xiaomi = || {
            let devices =
                receiver_snapshot(store.package().as_deref(), present_usb_instances().unwrap());
            let xiaomi: Vec<_> = devices
                .iter()
                .filter(|device| device.id.starts_with(r"usb:USB\VID_2717&PID_5055\"))
                .collect();
            assert_eq!(xiaomi.len(), 1, "请插入一枚小米无线键鼠套装 2 的接收器");
            assert_eq!(xiaomi[0].name, "小米无线键鼠套装 2");
            assert_eq!(xiaomi[0].battery, None);
        };
        assert_xiaomi();
        let data = json!({
            "id":"daozai.wireless-receivers","api_version":1,"version":"1.0.1",
            "receivers":[{"vid":"2717","pid":"5055","name":"兼容包里的同型号名称"}]
        });
        let package = Package::parse(&serde_json::to_vec(&data).unwrap()).unwrap();
        store.set(Some(package));
        assert_xiaomi();
        store.set(None);
        assert_xiaomi();
    }

    #[test]
    fn receiver_snapshot_uses_physical_usb_instance() {
        let devices = receiver_snapshot(
            None,
            [
                r"USB\VID_2717&PID_5055\5&AAA&0&1",
                r"usb\vid_2717&pid_5055\5&aaa&0&1",
                r"USB\VID_2717&PID_5055&MI_00\6&BBB&0&0000",
                r"USB\VID_2717&PID_5055&MI_01\6&BBB&0&0001",
                r"HID\VID_2717&PID_5055&MI_01\7&CCC&0&0000",
                r"USB\VID_2717&PID_5055\5&AAA&0&2",
            ],
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, r"usb:USB\VID_2717&PID_5055\5&AAA&0&1");
        assert_eq!(devices[1].id, r"usb:USB\VID_2717&PID_5055\5&AAA&0&2");
        assert!(devices
            .iter()
            .all(|device| device.kind == DeviceKind::Receiver24G
                && device.connected
                && device.battery.is_none()
                && device.reading.is_none()));
    }

    #[test]
    fn receiver_snapshot_excludes_unverified_vid_pid_and_non_usb_nodes() {
        assert!(receiver_snapshot(
            None,
            [
                r"USB\VID_8087&PID_0026\5&AAA&0&1",
                r"USB\VID_046D&PID_C08B\5&AAA&0&2",
                r"USB\VID_046D&PID_C51B\5&AAA&0&3",
                r"USB\VID_046D&PID_C548\",
                r"USB\VID_046D&PID_C548\A\B",
                r"HID\VID_046D&PID_C548\A",
            ]
        )
        .is_empty());
        assert_eq!(
            receiver_snapshot(None, [r"USB\VID_046D&PID_C548\A"])[0].name,
            "罗技 Bolt 接收器"
        );
    }

    #[test]
    fn receiver_usb_change_notifications_wake_the_scanner() {
        let (sender, changes) = mpsc::sync_channel(1);
        let context = std::ptr::from_ref(&sender).cast();
        for action in [
            CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
            CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL,
        ] {
            for _ in 0..2 {
                assert_eq!(
                    unsafe {
                        on_usb_change(
                            HCMNOTIFICATION::default(),
                            context,
                            action,
                            std::ptr::null(),
                            0,
                        )
                    },
                    0
                );
            }
            assert_eq!(changes.try_recv(), Ok(()));
            assert_eq!(changes.try_recv(), Err(mpsc::TryRecvError::Empty));
        }
    }

    #[test]
    #[ignore = "按需验证本机 USB 通知及接收器枚举"]
    fn receiver_live_usb_sources() {
        let (sender, _changes) = mpsc::sync_channel(1);
        let _notification = Notification::register(sender).expect("Windows USB 拔插通知注册应成功");
        let devices = receiver_snapshot(
            None,
            present_usb_instances().expect("Windows 已插入 USB 设备枚举应成功"),
        );
        for device in devices {
            println!(
                "{} | id={} | 电量={:?}",
                device.name, device.id, device.battery
            );
        }
    }
}
