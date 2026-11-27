//! Windows USB 接收器的 HID 报告传输。
use std::{ffi::c_void, time::Instant};
use windows::{
    core::{Owned, HRESULT, PCWSTR},
    Win32::{
        Devices::{DeviceAndDriverInstallation::*, HumanInterfaceDevice::*, Properties::*},
        Foundation::{ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE},
        Storage::FileSystem::{
            CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            Threading::{CreateEventW, ResetEvent},
            IO::{
                CancelIoEx, DeviceIoControl, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
            },
        },
    },
};

// Windows SDK hidclass.h：FILE_DEVICE_KEYBOARD，功能号 100，分别为 IN/OUT_DIRECT。
const IOCTL_HID_SET_FEATURE: u32 = 0x000b_0191;
const IOCTL_HID_GET_FEATURE: u32 = 0x000b_0192;

pub(super) struct Collection {
    pub(super) usage_page: u16,
    pub(super) usage: u16,
    pub(super) interface: Option<u8>,
    pub(super) input_len: usize,
    pub(super) output_len: usize,
    pub(super) feature_len: usize,
    path: Vec<u16>,
    preparsed: Preparsed,
}

struct Preparsed(PHIDP_PREPARSED_DATA);

impl Drop for Preparsed {
    fn drop(&mut self) {
        unsafe {
            let _ = HidD_FreePreparsedData(self.0);
        }
    }
}

pub(super) fn collections(instance: &str) -> Vec<Collection> {
    let instance = instance.strip_prefix("usb:").unwrap_or(instance);
    let instance = instance.to_ascii_uppercase();
    if !instance.starts_with("USB\\VID_") {
        return Vec::new();
    }
    let instance: Vec<_> = instance.encode_utf16().chain([0]).collect();
    let mut root = 0;
    if unsafe {
        CM_Locate_DevNodeW(
            &mut root,
            PCWSTR(instance.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    } != CR_SUCCESS
    {
        return Vec::new();
    }
    let mut collections = Vec::new();
    let mut pending = vec![(root, None)];
    while let Some((node, parent_interface)) = pending.pop() {
        let mut id = [0u16; MAX_DEVICE_ID_LEN as usize];
        if unsafe { CM_Get_Device_IDW(node, &mut id, 0) } != CR_SUCCESS {
            continue;
        }
        let Some(end) = id.iter().position(|&unit| unit == 0) else {
            continue;
        };
        let name = String::from_utf16_lossy(&id[..end]).to_ascii_uppercase();
        let mut identity = name.split('\\');
        let (Some(bus), Some(hardware)) = (identity.next(), identity.next()) else {
            continue;
        };
        let interface = hardware.split('&').find_map(|part| {
            let number = part.strip_prefix("MI_")?;
            if number.len() != 2 {
                return None;
            }
            u8::from_str_radix(number, 16).ok()
        });
        // 设备树中的另一枚完整 USB 设备属于另一个物理实例。
        if node != root && bus == "USB" && interface.is_none() {
            continue;
        }
        let interface = interface.or(parent_interface);
        if bus == "HID" && supported_collection(node, interface) {
            if let Some(paths) = present_paths(PCWSTR(id.as_ptr())) {
                collections.extend(
                    paths
                        .split_inclusive(|&unit| unit == 0)
                        .filter(|path| path.len() > 1 && path.last() == Some(&0))
                        .filter_map(|path| collection_for(path, interface)),
                );
            }
        }
        let mut child = 0;
        if unsafe { CM_Get_Child(&mut child, node, 0) } == CR_SUCCESS {
            loop {
                pending.push((child, interface));
                let mut sibling = 0;
                if unsafe { CM_Get_Sibling(&mut sibling, child, 0) } != CR_SUCCESS {
                    break;
                }
                child = sibling;
            }
        }
    }
    collections
}

fn present_paths(instance: PCWSTR) -> Option<Vec<u16>> {
    for _ in 0..3 {
        let mut size = 0;
        if unsafe {
            CM_Get_Device_Interface_List_SizeW(
                &mut size,
                &GUID_DEVINTERFACE_HID,
                instance,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        } != CR_SUCCESS
        {
            return None;
        }
        let mut paths = vec![0; size as usize];
        match unsafe {
            CM_Get_Device_Interface_ListW(
                &GUID_DEVINTERFACE_HID,
                instance,
                &mut paths,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        } {
            CR_SUCCESS => return Some(paths),
            CR_BUFFER_SMALL => continue,
            _ => return None,
        }
    }
    None
}

fn collection_for(path: &[u16], interface: Option<u8>) -> Option<Collection> {
    // 零访问权限仅取报告描述长度。发送请求前由适配器选定集合并打开读写句柄。
    let metadata = open_path(path, 0)?;
    let mut prepared = PHIDP_PREPARSED_DATA::default();
    if !unsafe { HidD_GetPreparsedData(*metadata, &mut prepared) }.as_bool() {
        return None;
    }
    let preparsed = Preparsed(prepared);
    let mut caps = HIDP_CAPS::default();
    let status = unsafe { HidP_GetCaps(preparsed.0, &mut caps) };
    if status != HIDP_STATUS_SUCCESS || !supported_usage(caps.UsagePage, caps.Usage, interface) {
        return None;
    }
    let writable = caps.UsagePage >= 0xff00;
    Some(Collection {
        usage_page: caps.UsagePage,
        usage: caps.Usage,
        interface,
        input_len: usize::from(caps.InputReportByteLength),
        output_len: if writable {
            usize::from(caps.OutputReportByteLength)
        } else {
            0
        },
        feature_len: if writable {
            usize::from(caps.FeatureReportByteLength)
        } else {
            0
        },
        path: path.to_vec(),
        preparsed,
    })
}

fn supported_usage(page: u16, usage: u16, interface: Option<u8>) -> bool {
    // Cloud III S 的实测输入报告位于接口 3 的这一个集合；英菲克 IN10 的电量广播位于接口 2。两者均只允许读取。
    page >= 0xff00
        || (page, usage, interface) == (0x01c0, 0x0001, Some(3))
        || (page, usage, interface) == (0x000a, 0x0000, Some(2))
}

fn supported_collection(node: u32, interface: Option<u8>) -> bool {
    let mut kind = DEVPROPTYPE::default();
    let mut ids = [0u16; 512];
    let mut size = std::mem::size_of_val(&ids) as u32;
    if unsafe {
        CM_Get_DevNode_PropertyW(
            node,
            &DEVPKEY_Device_HardwareIds,
            &mut kind,
            Some(ids.as_mut_ptr().cast()),
            &mut size,
            0,
        )
    } != CR_SUCCESS
        || kind != DEVPROP_TYPE_STRING_LIST
    {
        return false;
    }
    String::from_utf16_lossy(&ids[..size as usize / 2])
        .to_ascii_uppercase()
        .split('\0')
        .filter_map(|id| id.strip_prefix("HID_DEVICE_UP:")?.split_once("_U:"))
        .any(|(page, usage)| {
            let (Ok(page), Ok(usage)) = (
                u16::from_str_radix(page, 16),
                u16::from_str_radix(usage, 16),
            ) else {
                return false;
            };
            supported_usage(page, usage, interface)
        })
}

fn open_path(path: &[u16], access: u32) -> Option<Owned<HANDLE>> {
    let flags = if access != 0 {
        FILE_FLAG_OVERLAPPED
    } else {
        Default::default()
    };
    unsafe {
        let file = CreateFileW(
            PCWSTR(path.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            flags,
            HANDLE::default(),
        )
        .ok()?;
        Some(Owned::new(file))
    }
}

pub(super) struct Handle {
    file: Owned<HANDLE>,
    event: Owned<HANDLE>,
    input_len: usize,
    output_len: usize,
    feature_len: usize,
}

impl Collection {
    pub(super) fn supports_report(&self, id: u8, output: bool) -> bool {
        if output {
            self.has_report(HidP_Output, id, self.output_len)
        } else {
            self.has_report(HidP_Input, id, self.input_len)
        }
    }

    pub(super) fn supports_feature(&self, id: u8) -> bool {
        self.has_report(HidP_Feature, id, self.feature_len)
    }

    fn has_report(&self, kind: HIDP_REPORT_TYPE, id: u8, size: usize) -> bool {
        if size == 0 {
            return false;
        }
        let mut report = vec![0; size];
        let status = unsafe { HidP_InitializeReportForID(kind, id, self.preparsed.0, &mut report) };
        status == HIDP_STATUS_SUCCESS
    }

    pub(super) fn open(&self) -> Option<Handle> {
        let access = if self.usage_page >= 0xff00 {
            (GENERIC_READ | GENERIC_WRITE).0
        } else {
            GENERIC_READ.0
        };
        let file = open_path(&self.path, access)?;
        let event = unsafe { Owned::new(CreateEventW(None, true, false, PCWSTR::null()).ok()?) };
        Some(Handle {
            file,
            event,
            input_len: self.input_len,
            output_len: self.output_len,
            feature_len: self.feature_len,
        })
    }
}

enum Operation {
    Read,
    Write,
    SetFeature,
    GetFeature,
}

impl Handle {
    pub(super) fn write(&mut self, bytes: &[u8], deadline: Instant) -> bool {
        if bytes.is_empty() || bytes.len() > self.output_len {
            return false;
        }
        let mut report = vec![0; self.output_len];
        report[..bytes.len()].copy_from_slice(bytes);
        self.transfer(&mut report, deadline, Operation::Write) == Some(self.output_len)
    }

    pub(super) fn read(&mut self, deadline: Instant) -> Option<Vec<u8>> {
        if self.input_len == 0 {
            return None;
        }
        let mut report = vec![0; self.input_len];
        let received = self.transfer(&mut report, deadline, Operation::Read)?;
        if received == 0 || received > report.len() {
            return None;
        }
        report.truncate(received);
        Some(report)
    }

    pub(super) fn set_feature(&mut self, bytes: &[u8], deadline: Instant) -> bool {
        if bytes.is_empty() || bytes.len() > self.feature_len {
            return false;
        }
        let mut report = vec![0; self.feature_len];
        report[..bytes.len()].copy_from_slice(bytes);
        self.transfer(&mut report, deadline, Operation::SetFeature)
            .is_some()
    }

    pub(super) fn get_feature(&mut self, report_id: u8, deadline: Instant) -> Option<Vec<u8>> {
        if self.feature_len == 0 {
            return None;
        }
        let mut report = vec![0; self.feature_len];
        report[0] = report_id;
        let received = self.transfer(&mut report, deadline, Operation::GetFeature)?;
        if received == 0 || received > report.len() || report[0] != report_id {
            return None;
        }
        // 无编号的报告仍有首字节 0，Windows 返回的长度可能不含此字节。
        report.truncate(received + usize::from(report_id == 0 && received < report.len()));
        Some(report)
    }

    fn transfer(&mut self, report: &mut [u8], deadline: Instant, kind: Operation) -> Option<usize> {
        if Instant::now() >= deadline {
            return None;
        }
        unsafe { ResetEvent(*self.event) }.ok()?;
        let mut operation = OVERLAPPED {
            hEvent: *self.event,
            ..Default::default()
        };
        let file = *self.file;
        let buffer: *mut c_void = report.as_mut_ptr().cast();
        let len = report.len() as u32;
        let submitted = unsafe {
            match kind {
                Operation::Read => ReadFile(file, Some(report), None, Some(&mut operation)),
                Operation::Write => WriteFile(file, Some(report), None, Some(&mut operation)),
                Operation::SetFeature => DeviceIoControl(
                    file,
                    IOCTL_HID_SET_FEATURE,
                    Some(buffer.cast_const()),
                    len,
                    None,
                    0,
                    None,
                    Some(&mut operation),
                ),
                Operation::GetFeature => DeviceIoControl(
                    file,
                    IOCTL_HID_GET_FEATURE,
                    Some(buffer.cast_const()),
                    len,
                    Some(buffer),
                    len,
                    None,
                    Some(&mut operation),
                ),
            }
        };
        if submitted.is_err_and(|err| err.code() != HRESULT::from_win32(ERROR_IO_PENDING.0)) {
            return None;
        }
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u128::from(u32::MAX - 1)) as u32;
        let mut bytes = 0;
        if unsafe { GetOverlappedResultEx(file, &operation, &mut bytes, timeout, false) }.is_ok() {
            return (Instant::now() <= deadline).then_some(bytes as usize);
        }
        // 取消只提交请求。确认 I/O 已退出后，才能释放栈上的操作结构和报告缓冲区。
        unsafe {
            let _ = CancelIoEx(file, Some(&operation));
            let _ = GetOverlappedResult(file, &operation, &mut bytes, true);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_collection_keeps_the_observed_nonvendor_input_exception_narrow() {
        assert!(supported_usage(0xff01, 0, Some(1)));
        assert!(supported_usage(0x01c0, 1, Some(3)));
        assert!(!supported_usage(0x01c0, 1, Some(2)));
        assert!(!supported_usage(0x01c0, 1, None));
        assert!(!supported_usage(0x01c0, 2, Some(3)));
        assert!(supported_usage(0x000a, 0, Some(2)));
        assert!(!supported_usage(0x000a, 0, Some(1)));
        assert!(!supported_usage(0x000a, 0, None));
        assert!(!supported_usage(0x000a, 1, Some(2)));
        assert!(!supported_usage(1, 2, Some(3)));
        assert!(!supported_usage(1, 6, Some(3)));
    }
}
