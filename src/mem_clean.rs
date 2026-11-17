//! 极限清理：把可回收的物理内存一次性还给系统。
//!
//! 和「任务管理器」里只看数字不同，这里真的动手：
//! 1. 提权：SeDebugPrivilege / SeProfileSingleProcessPrivilege / SeIncreaseQuotaPrivilege；
//! 2. 逐个进程 EmptyWorkingSet：把工作集里近期没被访问的页踢回备用列表；
//! 3. 清空备用列表（Standby List）与低优先级备用列表——这一步才是释放量的大头；
//! 4. 清一次系统文件缓存工作集，把缓存白占的那部分也还回去。
//!
//! 全程跑在后台线程，界面线程只负责显示「释放了多少」，不阻塞绘制。

/// 一次清理的结果。字段都是默认值时表示没跑成（非 Windows 或提权失败）。
#[derive(Clone, Copy, Default)]
pub struct Outcome {
    /// 释放的物理内存（字节）：清理后可用量 - 清理前可用量。
    pub freed: u64,
    /// 成功收缩工作集的进程数。
    pub trimmed: u32,
    /// 备用列表是否清空成功。
    pub purged: bool,
    /// 耗时（毫秒）。
    pub ms: u64,
}

pub fn run() -> Outcome {
    #[cfg(windows)]
    {
        win::run()
    }
    #[cfg(not(windows))]
    {
        Outcome::default()
    }
}

/// 只收缩「本进程」的工作集，不动别的进程、不清备用列表、不需要管理员。
///
/// 岛仔折叠静置时调用：把字体映射、驱动代码页、一次性用完的图这些近期不再访问的页
/// 还回系统，任务管理器里的「内存」立刻回落；私有提交不变，下次展开只是补几页缺页。
pub fn trim_self() -> bool {
    #[cfg(windows)]
    {
        win::trim_self()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use super::Outcome;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_SET_QUOTA: u32 = 0x0100;
    const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
    const TOKEN_QUERY: u32 = 0x0008;
    const SE_PRIVILEGE_ENABLED: u32 = 0x0002;
    const ERROR_NOT_ALL_ASSIGNED: u32 = 1300;
    /// NtSetSystemInformation 的 SystemMemoryListInformation 类别。
    const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 0x50;
    /// MEMORY_LIST_COMMAND：3 = 刷脏页列表，4 = 清备用列表，5 = 清低优先级备用列表。
    const FLUSH_MODIFIED_LIST: u32 = 3;
    const PURGE_STANDBY_LIST: u32 = 4;
    const PURGE_LOW_PRIORITY_STANDBY_LIST: u32 = 5;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Luid {
        low: u32,
        high: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct LuidAndAttributes {
        luid: Luid,
        attributes: u32,
    }

    #[repr(C)]
    struct TokenPrivileges {
        count: u32,
        privilege: [LuidAndAttributes; 1],
    }

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetLastError() -> u32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        fn SetSystemFileCacheSize(minimum: usize, maximum: usize, flags: u32) -> i32;
        fn GetCurrentProcessId() -> u32;
        fn GetProcessHeap() -> isize;
        fn HeapCompact(heap: isize, flags: u32) -> usize;
    }

    #[link(name = "psapi")]
    extern "system" {
        fn EnumProcesses(list: *mut u32, bytes: u32, needed: *mut u32) -> i32;
        fn EmptyWorkingSet(process: isize) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
        fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut Luid) -> i32;
        fn AdjustTokenPrivileges(
            token: isize,
            disable_all: i32,
            new_state: *const TokenPrivileges,
            buffer_len: u32,
            previous: *mut c_void,
            return_len: *mut u32,
        ) -> i32;
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtSetSystemInformation(class: i32, info: *mut c_void, len: u32) -> i32;
    }

    /// 只收缩本进程的工作集。EmptyWorkingSet 对自己始终允许，无需任何特权。
    /// 顺手压一遍默认堆：长期驻留攒下的碎片块合并后可以真正还给系统，而不只是挪出工作集。
    pub fn trim_self() -> bool {
        unsafe {
            let ok = EmptyWorkingSet(GetCurrentProcess()) != 0;
            let heap = GetProcessHeap();
            if heap != 0 {
                HeapCompact(heap, 0);
            }
            ok
        }
    }

    /// 打开当前进程令牌里的某个特权。普通权限下失败是正常的，返回 false 即可。
    fn enable(name: &str) -> bool {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mut token: isize = 0;
            if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut enabled = false;
            let mut luid = Luid::default();
            if LookupPrivilegeValueW(std::ptr::null(), wide.as_ptr(), &mut luid) != 0 {
                let state = TokenPrivileges {
                    count: 1,
                    privilege: [LuidAndAttributes { luid, attributes: SE_PRIVILEGE_ENABLED }],
                };
                // PreviousState / ReturnLength 同为 NULL 是 MSDN 要求的用法。
                if AdjustTokenPrivileges(
                    token,
                    0,
                    &state,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ) != 0
                {
                    // 返回真但一个特权都没派下来时会带 ERROR_NOT_ALL_ASSIGNED。
                    enabled = GetLastError() != ERROR_NOT_ALL_ASSIGNED;
                }
            }
            CloseHandle(token);
            enabled
        }
    }

    /// 当前可用物理内存（字节）。
    fn avail() -> u64 {
        let mut status = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return 0;
        }
        status.avail_phys
    }

    /// 取当前全部进程号。缓冲区占满说明进程比预想的多，翻倍重来。
    fn list_pids() -> Vec<u32> {
        let mut capacity = 1024usize;
        loop {
            let mut pids = vec![0u32; capacity];
            let mut needed: u32 = 0;
            let bytes = (capacity * std::mem::size_of::<u32>()) as u32;
            if unsafe { EnumProcesses(pids.as_mut_ptr(), bytes, &mut needed) } == 0 {
                return Vec::new();
            }
            if needed as usize >= capacity * std::mem::size_of::<u32>() && capacity < 65_536 {
                capacity *= 2;
                continue;
            }
            pids.truncate(needed as usize / std::mem::size_of::<u32>());
            // 0 是空闲位，4 是 System，都不动。
            pids.retain(|pid| *pid != 0 && *pid != 4);
            return pids;
        }
    }

    fn trim_one(pid: u32) -> bool {
        let handle =
            unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle == 0 {
            return false;
        }
        let ok = unsafe { EmptyWorkingSet(handle) } != 0;
        unsafe { CloseHandle(handle) };
        ok
    }

    /// 逐进程收工作集，返回成功的进程数。
    /// 受保护进程（反作弊、PPL 系统进程）打不开是常态，直接跳过。
    ///
    /// 单个进程的收缩要等系统把页换出去，串行跑几百个进程要好几秒；
    /// 分四路并发后首轮从 5.5 秒降到 1 秒出头，点下去不用干等。
    fn trim_all() -> u32 {
        const WORKERS: usize = 4;
        // 跳过自己：把自己的工作集踢出去之后界面还得重新换回来，白白抖一下。
        let me = unsafe { GetCurrentProcessId() };
        let pids: Vec<u32> = list_pids().into_iter().filter(|pid| *pid != me).collect();
        if pids.is_empty() {
            return 0;
        }
        let chunk = (pids.len() + WORKERS - 1) / WORKERS;
        let mut handles = Vec::with_capacity(WORKERS);
        for slice in pids.chunks(chunk) {
            let slice = slice.to_vec();
            let handle = std::thread::Builder::new()
                .name("daozai-trim".into())
                .stack_size(96 * 1024)
                .spawn(move || slice.iter().filter(|pid| trim_one(**pid)).count() as u32);
            handles.push(handle);
        }
        handles
            .into_iter()
            .filter_map(|handle| handle.ok())
            .filter_map(|handle| handle.join().ok())
            .sum()
    }

    fn purge(command: u32) -> bool {
        let mut cmd = command;
        let status = unsafe {
            NtSetSystemInformation(
                SYSTEM_MEMORY_LIST_INFORMATION,
                &mut cmd as *mut u32 as *mut c_void,
                std::mem::size_of::<u32>() as u32,
            )
        };
        status >= 0
    }

    pub(super) fn run() -> Outcome {
        let started = std::time::Instant::now();
        enable("SeDebugPrivilege");
        enable("SeProfileSingleProcessPrivilege");
        enable("SeIncreaseQuotaPrivilege");

        let before = avail();
        let trimmed = trim_all();
        // 三条命令按「脏页 → 备用列表 → 低优先级备用列表」的顺序走，缺权限的那条失败不影响其余。
        let flush = purge(FLUSH_MODIFIED_LIST);
        let standby = purge(PURGE_STANDBY_LIST);
        let low = purge(PURGE_LOW_PRIORITY_STANDBY_LIST);
        // 收掉系统文件缓存的工作集后再读一次，数字才准。
        unsafe { SetSystemFileCacheSize(usize::MAX, usize::MAX, 0) };
        std::thread::sleep(std::time::Duration::from_millis(140));
        let after = avail();

        Outcome {
            freed: after.saturating_sub(before),
            trimmed,
            purged: standby || low || flush,
            ms: started.elapsed().as_millis() as u64,
        }
    }
}