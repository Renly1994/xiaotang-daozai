//! 已识别的 2.4G USB 接收器。拔插查询独立于蓝牙枚举。
use super::compat::{builtin_receivers, Package, Store};
use super::{halo_worker, usb_battery, BatteryReading, BatterySource, Device, DeviceKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{w, GUID, PCWSTR},
    Win32::Devices::{
        DeviceAndDriverInstallation::*,
        Properties::{DEVPKEY_Device_LastArrivalDate, DEVPROPTYPE, DEVPROP_TYPE_FILETIME},
    },
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
            Some(Device {
                id: format!("usb:{instance}"),
                name: name.into(),
                kind: DeviceKind::Receiver24G,
                ..Default::default()
            })
        })
        .collect();
    devices.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    devices.dedup_by(|a, b| a.id == b.id);
    devices
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

fn last_arrival(id: &str) -> Option<u64> {
    let instance: Vec<_> = id.strip_prefix("usb:")?.encode_utf16().chain([0]).collect();
    let mut node = 0;
    if unsafe {
        CM_Locate_DevNodeW(
            &mut node,
            PCWSTR(instance.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    } != CR_SUCCESS
    {
        return None;
    }
    let mut kind = DEVPROPTYPE::default();
    let mut value = 0u64;
    let value_size = std::mem::size_of::<u64>() as u32;
    let mut size = value_size;
    let result = unsafe {
        CM_Get_DevNode_PropertyW(
            node,
            &DEVPKEY_Device_LastArrivalDate,
            &mut kind,
            Some((&mut value as *mut u64).cast()),
            &mut size,
            0,
        )
    };
    (result == CR_SUCCESS && kind == DEVPROP_TYPE_FILETIME && size == value_size && value != 0)
        .then_some(value)
}

fn changed_arrivals<'a>(
    ids: impl Iterator<Item = &'a str>,
    known: &mut BTreeMap<String, u64>,
    read: impl Fn(&str) -> Option<u64>,
) -> Vec<String> {
    let previous = std::mem::take(known);
    let mut changed = Vec::new();
    for id in ids {
        let before = previous.get(id).copied();
        // 属性暂时不可读时保留上次的到达时间，等下一次成功查询再比较。
        let Some(current) = read(id).or(before) else {
            continue;
        };
        if before.is_some_and(|value| value != current) {
            changed.push(id.to_owned());
        }
        known.insert(id.to_owned(), current);
    }
    changed
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
        last_arrival,
    );
}

#[derive(Default)]
struct Enhancement {
    executable: Option<PathBuf>,
    worker: Option<halo_worker::Worker>,
    retiring: Option<(std::thread::JoinHandle<()>, Vec<halo_worker::Receiver>)>,
    targets: Vec<halo_worker::Receiver>,
    generation: u64,
    started: Option<Instant>,
    retry_after: Option<Instant>,
}

impl Enhancement {
    fn sync(
        &mut self,
        executable: Option<PathBuf>,
        targets: Vec<halo_worker::Receiver>,
        reconnected: bool,
        reassigned_to_native: bool,
        now: Instant,
    ) {
        let replaced = self.executable != executable;
        if replaced || targets.is_empty() || reassigned_to_native {
            if let Some(thread) = self.worker.take().and_then(halo_worker::Worker::shutdown) {
                self.retiring = Some((thread, self.targets.clone()));
            }
            self.retry_after = None;
        }
        self.executable = executable;
        if replaced || reconnected || self.targets != targets {
            self.targets = targets;
            self.generation = self.generation.wrapping_add(1);
            self.started = Some(now);
            if let Some(worker) = &self.worker {
                worker.set_targets(self.generation, self.targets.clone());
            }
        }
    }

    fn blocks_native(&self, id: &str) -> bool {
        self.retiring.as_ref().is_some_and(|(thread, targets)| {
            !thread.is_finished()
                && id.strip_prefix("usb:").is_some_and(|instance| {
                    targets.iter().any(|target| target.usb_instance == instance)
                })
        })
    }

    fn snapshot(&mut self, now: Instant) -> Option<Arc<halo_worker::Snapshot>> {
        if let Some((thread, _)) = &self.retiring {
            if !thread.is_finished() {
                return None;
            }
            self.retiring = None;
        }
        if self.targets.is_empty() {
            return None;
        }
        if self.worker.is_none() && self.retry_after.is_none_or(|at| now >= at) {
            let path = self.executable.as_ref()?.clone();
            self.worker = halo_worker::Worker::spawn(path).ok();
            self.retry_after = Some(now + Duration::from_secs(15));
            if let Some(worker) = &self.worker {
                worker.set_targets(self.generation, self.targets.clone());
            }
        }
        self.worker
            .as_ref()?
            .snapshot(now)
            .filter(|snapshot| snapshot.generation == self.generation)
    }
}

fn worker_receiver(id: &str) -> Option<halo_worker::Receiver> {
    let usb_instance = id.strip_prefix("usb:")?;
    let (hardware, _) = usb_instance.strip_prefix("USB\\")?.split_once('\\')?;
    let (vid, pid) = hardware.strip_prefix("VID_")?.split_once("&PID_")?;
    Some(halo_worker::Receiver {
        usb_instance: usb_instance.into(),
        vid: u16::from_str_radix(vid, 16).ok()?,
        pid: u16::from_str_radix(pid, 16).ok()?,
    })
}

fn merge_enhanced(
    devices: &[Device],
    snapshot: &halo_worker::Snapshot,
    started: Instant,
    now: Instant,
) -> Option<Vec<Device>> {
    let mut grouped: BTreeMap<&str, BTreeMap<&str, &halo_worker::Reading>> = BTreeMap::new();
    for observed in &snapshot.devices {
        let Some(instance) = observed.usb_instance.as_deref() else {
            continue;
        };
        if observed.online == Some(false)
            || !observed.sampled_at.is_some_and(|at| {
                at >= started && now.saturating_duration_since(at) <= super::FRESH_FOR
            })
        {
            continue;
        }
        grouped
            .entry(instance)
            .or_default()
            .insert(&observed.key, observed);
    }
    if grouped.is_empty() {
        return None;
    }
    let mut merged = Vec::with_capacity(devices.len());
    for receiver in devices {
        let records = receiver
            .id
            .strip_prefix("usb:")
            .and_then(|id| grouped.get(id));
        let Some(records) = records.filter(|_| receiver.reading.is_none()) else {
            merged.push(receiver.clone());
            continue;
        };
        for observed in records.values() {
            let mut device = receiver.clone();
            // 同一接收器的其他设备暂时离线时，仍按设备自己的标识跟踪电量。
            device.id.push('#');
            device.id.push_str(&observed.key);
            device.name.clone_from(&observed.name);
            device.battery = observed.level;
            device.reading = observed
                .level
                .zip(observed.sampled_at)
                .map(|(percent, sampled)| {
                    let mut reading = BatteryReading::new(
                        percent,
                        BatterySource::UsbReceiver,
                        sampled,
                        observed.charging,
                    );
                    reading.approximate = observed.approximate;
                    reading
                });
            if device.reading.is_none() {
                device.charging_only = observed.charging.zip(observed.sampled_at);
            }
            merged.push(device);
        }
    }
    Some(merged)
}

fn run_scans(
    tx: SyncSender<Vec<Device>>,
    stop: Arc<AtomicBool>,
    store: Store,
    wake: impl Fn(),
    changes: mpsc::Receiver<()>,
    scan_interval: Duration,
    scan: impl Fn() -> Option<Vec<String>>,
    read_arrival: impl Fn(&str) -> Option<u64>,
) {
    let mut active = store.package();
    let mut next_scan = Instant::now();
    let mut instances = None;
    let mut latest: Option<Vec<Device>> = None;
    let mut published = None;
    let mut battery = usb_battery::Poller::new();
    let mut enhancement = Enhancement::default();
    let mut arrivals = BTreeMap::new();
    while !stop.load(Ordering::Relaxed) {
        let selected = store.package();
        let enhanced_executable = selected
            .as_ref()
            .and_then(|_| store.component()?.executable());
        let now = Instant::now();
        let changed = match (&selected, &active) {
            (Some(selected), Some(active)) => !Arc::ptr_eq(selected, active),
            _ => selected.is_some() != active.is_some(),
        };
        if now >= next_scan || changed || enhancement.executable != enhanced_executable {
            active = selected;
            let found = scan();
            let scanned = found.is_some();
            if let Some(found) = found {
                store.set_usb_instances(found.clone(),now);
                instances = Some(found);
            }
            // 启停和更新按最新识别表重算，刚好枚举失败时也能移除已停用的扩展项。
            latest = instances
                .as_ref()
                .map(|ids| receiver_snapshot(active.as_deref(), ids));
            if let Some(devices) = &latest {
                battery.sync(devices, active.as_deref(), now);
                let changed_ids = if scanned {
                    // 两次扫描之间重插时，实例 ID 可能相同，Windows 的到达时间会变化。
                    if enhanced_executable.is_some() {
                        changed_arrivals(
                            devices.iter().map(|device| device.id.as_str()),
                            &mut arrivals,
                            &read_arrival,
                        )
                    } else {
                        changed_arrivals(battery.target_ids(), &mut arrivals, &read_arrival)
                    }
                } else {
                    Vec::new()
                };
                for id in &changed_ids {
                    battery.invalidate(id);
                }
                let (targets, reassigned) = if enhanced_executable.is_some() {
                    let native: BTreeSet<_> = battery
                        .target_ids()
                        .filter_map(|id| id.strip_prefix("usb:"))
                        .collect();
                    let devices = devices
                        .iter()
                        .filter(|device| !usb_battery::is_reading(&device.id))
                        .filter_map(|device| worker_receiver(&device.id))
                        .filter(|target| !native.contains(target.usb_instance.as_str()))
                        .collect();
                    let reassigned = enhancement
                        .targets
                        .iter()
                        .any(|target| native.contains(target.usb_instance.as_str()));
                    (devices, reassigned)
                } else {
                    (Vec::new(), false)
                };
                enhancement.sync(
                    enhanced_executable,
                    targets,
                    !changed_ids.is_empty(),
                    reassigned,
                    now,
                );
            }
            next_scan = now + scan_interval;
        }
        if let Some(devices) = &mut latest {
            battery.poll(devices, now, |id| enhancement.blocks_native(id));
            let enhanced = enhancement
                .snapshot(now)
                .and_then(|snapshot| merge_enhanced(devices, &snapshot, enhancement.started?, now));
            let devices = enhanced.as_ref().unwrap_or(devices);
            if published.as_ref() != Some(devices) {
                match tx.try_send(devices.clone()) {
                    Ok(()) => {
                        published = Some(devices.clone());
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
    store.clear_usb_instances();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bt::compat::builtin_data;
    use serde_json::json;

    #[test]
    fn receiver_enhancement_joins_only_current_physical_receivers() {
        const FIRST: &str = r"USB\VID_0B05&PID_1ACE\FIRST";
        const SECOND: &str = r"USB\VID_0B05&PID_1ACE\SECOND";
        let first = Device {
            id: format!("usb:{FIRST}"),
            name: "ROG OMNI 接收器".into(),
            kind: DeviceKind::Receiver24G,
            ..Default::default()
        };
        let mut second = first.clone();
        second.id = format!("usb:{SECOND}");
        let now = Instant::now();
        let started = now - Duration::from_secs(5);
        let make_reading = |instance: &str, key: &str, level: Option<u8>, time: Instant| {
            let mut reading: halo_worker::Reading = serde_json::from_value(json!({
                "key": key, "name": key, "kind": "mouse",
                "level": level, "charging": true, "online": true,
                "vid": 0x0B05, "pid": 0x1ACE, "usb_instance": instance,
                "fresh": true, "age_ms": 0
            }))
            .unwrap();
            reading.sampled_at = Some(time);
            reading
        };
        let mut snapshot = halo_worker::Snapshot {
            generation: 2,
            observed: now,
            devices: vec![
                make_reading(FIRST, "A", Some(0), now),
                make_reading(FIRST, "B", None, now),
                make_reading(SECOND, "C", Some(100), started - Duration::from_millis(1)),
            ],
        };
        assert_eq!(worker_receiver(&first.id).unwrap().usb_instance, FIRST);
        let merged =
            merge_enhanced(&[first.clone(), second.clone()], &snapshot, started, now).unwrap();
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].id, format!("{}#A", first.id));
        assert_eq!(merged[0].battery, Some(0));
        assert_eq!(merged[1].battery, None);
        assert_eq!(merged[1].charging_only, Some((true, now)));
        assert_eq!(merged[2], second);

        snapshot.devices[1].online = Some(false);
        let only_first =
            merge_enhanced(std::slice::from_ref(&first), &snapshot, started, now).unwrap();
        assert_eq!(only_first, merged[..1]);

        snapshot.devices[0].sampled_at = Some(started - Duration::from_millis(1));
        snapshot.devices[1].online = Some(true);
        let only_second = merge_enhanced(&[first], &snapshot, started, now).unwrap();
        assert_eq!(only_second, merged[1..2]);
    }

    #[test]
    fn receiver_arrival_change_detects_replug_without_missing_snapshot() {
        let first = r"usb:USB\VID_3554&PID_F58A\A";
        let second = r"usb:USB\VID_3554&PID_F58A\B";
        let ids = [first, second];
        let mut known = BTreeMap::new();

        assert!(changed_arrivals(ids.into_iter(), &mut known, |_| Some(10)).is_empty());
        assert!(
            changed_arrivals(ids.into_iter(), &mut known, |id| (id == second)
                .then_some(10))
            .is_empty()
        );
        assert_eq!(known[first], 10);
        assert_eq!(
            changed_arrivals(ids.into_iter(), &mut known, |id| Some(if id == first {
                11
            } else {
                10
            })),
            [first]
        );
        assert_eq!(known[first], 11);

        assert!(changed_arrivals([second].into_iter(), &mut known, |_| Some(10)).is_empty());
        assert!(!known.contains_key(first));
    }

    #[test]
    fn receiver_snapshot_native_catalog_covers_all_23_without_plugin() {
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
        assert_eq!(devices.len(), 23);
        for (instance, row) in instances.iter().zip(rows) {
            let device = devices
                .iter()
                .find(|device| device.id == format!("usb:{instance}"))
                .unwrap();
            assert_eq!(Some(device.name.as_str()), row["name"].as_str());
        }
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
                    |_| None,
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
