//! 【新增】Windows HID 平台层：描述符解析、标准读取、已知私有协议 I/O。
//! 设备路径仅用于本进程打开接口；错误统一枚举化，不把驱动错误中的路径带进反馈。
use super::protocols::{self, Io, Protocol};
use super::{
    Battery, Descriptor, Device, Failure, Field, ReportType, Snapshot, MAX_FIELDS, MAX_INTERFACES,
};
use ::windows::Win32::{Devices::HumanInterfaceDevice::*, Foundation::HANDLE};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::{
    ffi::CStr,
    fs::{File, OpenOptions},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

struct Native {
    file: File,
    preparsed: PHIDP_PREPARSED_DATA,
    caps: HIDP_CAPS,
}
impl Native {
    fn open(path: &CStr) -> Result<Self, Failure> {
        // 元数据和标准 GetReport 使用零访问句柄，不抢占键鼠的输入通道。
        let file = OpenOptions::new()
            .access_mode(0)
            .share_mode(3)
            .open(path.to_string_lossy().as_ref())
            .map_err(|_| Failure::Access)?;
        let mut preparsed = PHIDP_PREPARSED_DATA::default();
        if !unsafe { HidD_GetPreparsedData(HANDLE(file.as_raw_handle()), &mut preparsed) }.as_bool()
        {
            return Err(Failure::Io);
        }
        let mut this = Self {
            file,
            preparsed,
            caps: HIDP_CAPS::default(),
        };
        if unsafe { HidP_GetCaps(this.preparsed, &mut this.caps) } != HIDP_STATUS_SUCCESS {
            return Err(Failure::InvalidReply);
        }
        Ok(this)
    }
    fn fields(&self) -> Result<(Vec<Field>, bool), Failure> {
        let mut result = Vec::new();
        let mut truncated = false;
        for (kind, native, count) in [
            (
                ReportType::Feature,
                HidP_Feature,
                self.caps.NumberFeatureValueCaps,
            ),
            (
                ReportType::Input,
                HidP_Input,
                self.caps.NumberInputValueCaps,
            ),
            (
                ReportType::Output,
                HidP_Output,
                self.caps.NumberOutputValueCaps,
            ),
        ] {
            if count == 0 {
                continue;
            }
            // 超大描述符不分配无限内存；保留明确截断标志，供维护者另行调查。
            if usize::from(count) > MAX_FIELDS {
                truncated = true;
                continue;
            }
            let mut values = vec![HIDP_VALUE_CAPS::default(); usize::from(count)];
            let mut actual = count;
            if unsafe {
                HidP_GetValueCaps(native, values.as_mut_ptr(), &mut actual, self.preparsed)
            } != HIDP_STATUS_SUCCESS
            {
                return Err(Failure::InvalidReply);
            }
            for v in values.iter().take(usize::from(actual)) {
                if result.len() == MAX_FIELDS {
                    truncated = true;
                    break;
                }
                // SAFETY: 根据 IsRange 选择 Windows 联合体中的有效成员。
                let (min, max) = unsafe {
                    if v.IsRange.as_bool() {
                        (v.Anonymous.Range.UsageMin, v.Anonymous.Range.UsageMax)
                    } else {
                        (v.Anonymous.NotRange.Usage, v.Anonymous.NotRange.Usage)
                    }
                };
                result.push(Field {
                    report_type: kind,
                    report_id: v.ReportID,
                    usage_page: v.UsagePage,
                    usage_min: min,
                    usage_max: max,
                    link_collection: v.LinkCollection,
                    bit_size: v.BitSize,
                    report_count: v.ReportCount,
                    logical_min: v.LogicalMin,
                    logical_max: v.LogicalMax,
                    absolute: v.IsAbsolute.as_bool(),
                });
            }
        }
        Ok((result, truncated))
    }
    /// 【新增】厂商字节数组常被 Windows 归入 button caps，不能只查看 value caps。
    /// 这里仅提取 report ID，不读取按钮值，也不把按钮数组解释为电池字段。
    fn button_report_ids(&self) -> Result<([Vec<u8>; 3], bool), Failure> {
        let mut ids: [Vec<u8>; 3] = Default::default();
        let mut truncated = false;
        for (index, kind, count) in [
            (0, HidP_Input, self.caps.NumberInputButtonCaps),
            (1, HidP_Output, self.caps.NumberOutputButtonCaps),
            (2, HidP_Feature, self.caps.NumberFeatureButtonCaps),
        ] {
            if count == 0 {
                continue;
            }
            if usize::from(count) > MAX_FIELDS {
                truncated = true;
                continue;
            }
            let mut caps = vec![HIDP_BUTTON_CAPS::default(); usize::from(count)];
            let mut actual = count;
            if unsafe { HidP_GetButtonCaps(kind, caps.as_mut_ptr(), &mut actual, self.preparsed) }
                != HIDP_STATUS_SUCCESS
            {
                return Err(Failure::InvalidReply);
            }
            ids[index].extend(
                caps.iter()
                    .take(usize::from(actual))
                    .map(|cap| cap.ReportID),
            );
        }
        Ok((ids, truncated))
    }
    fn battery(&self, d: &Descriptor, budget: &Budget<'_>) -> Result<Battery, Failure> {
        let mut last = Failure::Unsupported;
        for field in d
            .fields
            .iter()
            .filter(|f| f.battery_usage().is_some())
            .take(8)
        {
            budget.check()?;
            let (kind, len) = match field.report_type {
                ReportType::Feature => (HidP_Feature, d.feature_len),
                ReportType::Input => (HidP_Input, d.input_len),
                ReportType::Output => continue,
            };
            if len == 0 || len > 4096 {
                last = Failure::InvalidReply;
                continue;
            }
            let mut buffer = vec![0u8; usize::from(len)];
            buffer[0] = field.report_id;
            // 【新增】仅获取描述符确实声明的电池报告，绝不遍历所有 report ID。
            let ok = unsafe {
                if field.report_type == ReportType::Feature {
                    HidD_GetFeature(
                        HANDLE(self.file.as_raw_handle()),
                        buffer.as_mut_ptr().cast(),
                        len.into(),
                    )
                } else {
                    HidD_GetInputReport(
                        HANDLE(self.file.as_raw_handle()),
                        buffer.as_mut_ptr().cast(),
                        len.into(),
                    )
                }
            };
            budget.check()?;
            if !ok.as_bool() {
                last = Failure::Io;
                continue;
            }
            let mut value = 0;
            let status = unsafe {
                HidP_GetUsageValue(
                    kind,
                    field.usage_page,
                    field.link_collection,
                    field.battery_usage().unwrap(),
                    &mut value,
                    self.preparsed,
                    &buffer,
                )
            };
            // 原始报告在此丢弃，只保留电量数值，不保存同一报告中的按键或坐标。
            if status == HIDP_STATUS_SUCCESS {
                if let Some(percent) = field.percent(value) {
                    return Ok(Battery {
                        percent,
                        charging: None,
                        millivolts: None,
                    });
                }
            }
            last = Failure::InvalidReply;
        }
        Err(last)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = unsafe { HidD_FreePreparsedData(self.preparsed) };
    }
}

struct Budget<'a> {
    active: &'a AtomicBool,
    stopped: &'a AtomicBool,
    deadline: Instant,
}
impl Budget<'_> {
    fn check(&self) -> Result<(), Failure> {
        if self.stopped.load(Ordering::Relaxed) || !self.active.load(Ordering::Relaxed) {
            Err(Failure::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Failure::Budget)
        } else {
            Ok(())
        }
    }
}
struct Session<'a> {
    device: HidDevice,
    path: &'a CStr,
    budget: Budget<'a>,
}
impl Io for Session<'_> {
    fn check(&self) -> Result<(), Failure> {
        self.budget.check()
    }
    fn write(&mut self, request: &[u8], control: bool) -> Result<(), Failure> {
        self.check()?;
        if control {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(3)
                .open(self.path.to_string_lossy().as_ref())
                .map_err(|_| Failure::Access)?;
            if !unsafe {
                HidD_SetOutputReport(
                    HANDLE(file.as_raw_handle()),
                    request.as_ptr().cast(),
                    request.len() as u32,
                )
            }
            .as_bool()
            {
                return Err(Failure::Io);
            }
        } else {
            self.device.write(request).map_err(|_| Failure::Io)?;
        }
        self.check()
    }
    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, Failure> {
        self.check()?;
        let mut buffer = [0u8; 64];
        let len = self
            .device
            .read_timeout(&mut buffer, timeout.as_millis().min(100) as i32)
            .map_err(|_| Failure::Io)?;
        self.check()?;
        Ok(buffer[..len].to_vec())
    }
}

fn describe(info: &DeviceInfo) -> Descriptor {
    let mut d = Descriptor {
        vid: info.vendor_id(),
        pid: info.product_id(),
        release: info.release_number(),
        usage_page: info.usage_page(),
        usage: info.usage(),
        interface: info.interface_number(),
        bus: format!("{:?}", info.bus_type()),
        ..Default::default()
    };
    match Native::open(info.path()) {
        Ok(native) => {
            d.input_len = native.caps.InputReportByteLength;
            d.output_len = native.caps.OutputReportByteLength;
            d.feature_len = native.caps.FeatureReportByteLength;
            d.value_caps = [
                native.caps.NumberInputValueCaps,
                native.caps.NumberOutputValueCaps,
                native.caps.NumberFeatureValueCaps,
            ];
            d.button_caps = [
                native.caps.NumberInputButtonCaps,
                native.caps.NumberOutputButtonCaps,
                native.caps.NumberFeatureButtonCaps,
            ];
            match native.fields() {
                Ok((fields, truncated)) => {
                    d.fields = fields;
                    d.fields_truncated = truncated;
                }
                Err(e) => d.error = Some(e),
            }
            match native.button_report_ids() {
                Ok((ids, truncated)) => {
                    d.report_ids = ids;
                    d.fields_truncated |= truncated;
                }
                Err(e) => d.error = Some(e),
            }
            for field in &d.fields {
                let index = match field.report_type {
                    ReportType::Input => 0,
                    ReportType::Output => 1,
                    ReportType::Feature => 2,
                };
                d.report_ids[index].push(field.report_id);
            }
            for ids in &mut d.report_ids {
                ids.sort_unstable();
                ids.dedup();
            }
        }
        Err(e) => d.error = Some(e),
    }
    d
}
/// 【修改】固定 FNV-1a 算法关联本地偏好，避免 DefaultHasher 随工具链升级改变设备 ID。
fn local_id(path: &CStr) -> String {
    let hash = path
        .to_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte.to_ascii_lowercase())).wrapping_mul(0x100000001b3)
        });
    format!("hid-{hash:016x}")
}
fn base_device(info: &DeviceInfo, descriptor: Descriptor) -> Device {
    let failure = descriptor.error.or(Some(Failure::Unsupported));
    Device {
        id: local_id(info.path()),
        name: info
            .product_string()
            .unwrap_or("HID 设备")
            .chars()
            .take(96)
            .collect(),
        descriptor,
        protocol: "unknown",
        transport: "HID",
        slot: None,
        battery: None,
        failure,
    }
}

#[cfg(test)]
pub(super) fn collect(
    enabled: bool,
    active: &AtomicBool,
    stopped: &AtomicBool,
    generation: u64,
) -> Snapshot {
    collect_scheduled(
        enabled,
        active,
        stopped,
        generation,
        &mut super::polling::Polling::default(),
        true,
    )
}

/// 【新增】每轮只查询到期接口；其余设备复用原始采样及时间戳，拔出后清除缓存。
pub(super) fn collect_scheduled(
    enabled: bool,
    active: &AtomicBool,
    stopped: &AtomicBool,
    generation: u64,
    polling: &mut super::polling::Polling,
    force: bool,
) -> Snapshot {
    let mut snapshot = Snapshot::default();
    if !enabled {
        polling.clear();
    }
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(_) => {
            polling.clear();
            snapshot.error = Some(Failure::Io);
            return snapshot;
        }
    };
    let all: Vec<_> = api.device_list().collect();
    snapshot.truncated = all.len() > MAX_INTERFACES;
    let mut catalog: Vec<_> = all
        .into_iter()
        .take(MAX_INTERFACES)
        .map(|info| (info, describe(info)))
        .collect();
    // 【新增】优先已适配的设备，轮换采集起点，防止后面的接口长期得不到预算。
    catalog.sort_by_key(|(_, d)| protocols::candidates(d).is_empty());
    let count = catalog
        .iter()
        .take_while(|(_, d)| !protocols::candidates(d).is_empty())
        .count();
    if count > 0 {
        catalog[..count].rotate_left(generation as usize % count);
    }
    let round_deadline = Instant::now() + Duration::from_secs(12);
    let mut present = Vec::new();
    for (info, descriptor) in catalog {
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        let candidates = protocols::candidates(&descriptor);
        let mut device = base_device(info, descriptor);
        present.push(device.id.clone());
        if candidates.is_empty() {
            if !enabled {
                device.failure = Some(Failure::Disabled);
            }
            snapshot.devices.push(device);
            continue;
        }
        // 【修改】先标识已匹配协议，访问失败时仍可调整该接收器的采样间隔。
        device.protocol = candidates[0].label();
        device.transport = if candidates
            .iter()
            .any(|p| matches!(p, Protocol::Compx { .. }))
        {
            if device.descriptor.pid == 0xfb3d {
                "USB"
            } else {
                "2.4G"
            }
        } else if candidates.contains(&Protocol::Hidpp) {
            "HID++"
        } else {
            "HID"
        };
        if !enabled {
            device.failure = Some(Failure::Disabled);
            snapshot.devices.push(device);
            continue;
        }
        let wireless = device.transport == "2.4G";
        if let Some((cached, sample)) = polling.cached(&device.id, wireless, Instant::now(), force)
        {
            for d in &cached {
                snapshot.samples.insert(d.id.clone(), sample);
            }
            snapshot.devices.extend(cached);
            continue;
        }
        let deadline = (Instant::now() + Duration::from_secs(4)).min(round_deadline);
        let budget = Budget {
            active,
            stopped,
            deadline,
        };
        if let Err(error) = budget.check() {
            device.failure = Some(error);
            let sample = polling.store(
                device.id.clone(),
                wireless,
                vec![device.clone()],
                Instant::now(),
                unix_now(),
            );
            snapshot.samples.insert(device.id.clone(), sample);
            snapshot.devices.push(device);
            continue;
        }
        let mut slots = Vec::new();
        for protocol in candidates {
            device.protocol = protocol.label();
            if protocol == Protocol::Standard {
                let result = Native::open(info.path())
                    .and_then(|native| native.battery(&device.descriptor, &budget));
                match result {
                    Ok(b) => {
                        device.battery = Some(b);
                        device.failure = None;
                        break;
                    }
                    Err(e) => device.failure = Some(e),
                }
                continue;
            }
            // 只有注册成功的厂商集合才申请可读写句柄，普通键鼠输入集合不会走这里。
            let handle = match info.open_device(&api) {
                Ok(handle) => handle,
                Err(_) => {
                    device.failure = Some(Failure::Access);
                    continue;
                }
            };
            let mut session = Session {
                device: handle,
                path: info.path(),
                budget: Budget {
                    active,
                    stopped,
                    deadline,
                },
            };
            match protocol {
                Protocol::Compx { online } => {
                    device.transport = if device.descriptor.pid == 0xfb3d {
                        "USB"
                    } else {
                        "2.4G"
                    };
                    match protocols::read_compx(&mut session, online) {
                        Ok(b) => {
                            device.battery = Some(b);
                            device.failure = None;
                        }
                        Err(e) => device.failure = Some(e),
                    }
                }
                Protocol::Hidpp => {
                    device.transport = "HID++";
                    let mut reader = protocols::Hidpp::new();
                    // 同一个接收器可以同时配对多个设备；每个槽位单独生成身份，不只取第一只鼠标。
                    for slot in [1, 2, 3, 4, 5, 6, 0xff] {
                        if let Err(error) = session.check() {
                            device.failure = Some(error);
                            break;
                        }
                        match reader.read(&mut session, slot) {
                            Ok(b) => {
                                let mut found = device.clone();
                                found.id = format!("{}-{slot}", found.id);
                                found.slot = Some(slot);
                                found.battery = Some(b);
                                found.failure = None;
                                slots.push(found);
                            }
                            Err(e) => device.failure = Some(e),
                        }
                    }
                }
                Protocol::Standard => {}
            }
        }
        let id = device.id.clone();
        let devices = if slots.is_empty() {
            vec![device]
        } else {
            slots
        };
        let sample = polling.store(id, wireless, devices.clone(), Instant::now(), unix_now());
        for d in &devices {
            snapshot.samples.insert(d.id.clone(), sample);
        }
        snapshot.devices.extend(devices);
    }
    if !enabled {
        snapshot.error = Some(Failure::Disabled);
    }
    polling.retain(&present);
    snapshot
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_identity_survives_path_case_changes_and_separates_same_model_devices() {
        let a = std::ffi::CString::new("HID#VID_1234&PID_5678#receiver-a").unwrap();
        let same = std::ffi::CString::new("hid#vid_1234&pid_5678#receiver-a").unwrap();
        let b = std::ffi::CString::new("hid#vid_1234&pid_5678#receiver-b").unwrap();
        assert_eq!(local_id(&a), local_id(&same));
        assert_ne!(local_id(&a), local_id(&b));
        assert!(!local_id(&a).contains("receiver"));
    }
    /// 【新增】按需实机验证；常规测试和 CI 不访问测试机上的真实外设。
    #[test]
    #[ignore = "需指定 DAOZAI_HID_CAPTURE，主动采集当前机器的脱敏 HID 报告"]
    fn capture_native_hid_feedback() {
        let destination = std::env::var_os("DAOZAI_HID_CAPTURE").expect("请指定报告输出文件");
        let mut snapshot = collect(true, &AtomicBool::new(true), &AtomicBool::new(false), 1);
        snapshot.generation = 1;
        snapshot.collected_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let report = super::super::feedback(&snapshot);
        std::fs::write(destination, report.as_bytes()).unwrap();
        println!(
            "HID 接口/设备：{}；有效电量：{}；顶层错误：{:?}",
            snapshot.devices.len(),
            snapshot
                .devices
                .iter()
                .filter(|d| d.battery.is_some())
                .count(),
            snapshot.error
        );
        assert!(snapshot.error.is_none(), "HID 枚举应完成");
    }
}
