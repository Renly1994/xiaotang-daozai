use std::{
    env,
    fs::File,
    io::{self, Read},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use serde_json::Value;
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::{
            JobObjects::{
                CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Pipes::CreatePipe,
            Threading::{
                CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
                InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
                CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, EXTENDED_STARTUPINFO_PRESENT,
                LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
                STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
            },
        },
    },
};

use super::{Failure, Mode, CLI_MISSING, CLI_OLD, SLASH_DISABLED};

const MAX_REPORT: u64 = 1024 * 1024;
const MAX_DIAGNOSTICS: u64 = 64 * 1024;
const UPDATE_KEY: &str = "AGY_CLI_DISABLE_AUTO_UPDATE";
const START_ERROR: &str = "无法在后台启动 Antigravity CLI，请检查安装或系统策略";
const TIMEOUT: &str = "Antigravity 额度查询超时";
const SETTINGS_ERROR: &str = "无法核对 Antigravity CLI 的斜杠命令设置，请检查 CLI 设置";

pub(super) fn discover() -> Option<PathBuf> {
    let local = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let standard = local.iter().flat_map(|root| {
        [
            root.join("agy/bin/agy.exe"),
            root.join("Microsoft/WinGet/Links/agy.exe"),
        ]
    });
    let path = env::var_os("PATH").unwrap_or_default();
    standard
        .chain(env::split_paths(&path).map(|dir| dir.join("agy.exe")))
        .find(|exe| exe.is_absolute() && exe.is_file())
}

pub(super) fn report(exe: &Path, credits: bool) -> Result<Value, Failure> {
    if !exe.is_absolute()
        || !exe
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("agy.exe"))
        || !exe.is_file()
    {
        return Err(Failure::Message(CLI_MISSING.into()));
    }
    let work_dir = env::temp_dir();
    let version = run(exe, "--version", &work_dir, Duration::from_secs(5))?;
    if version.code != 0 || !supports_read_only_report(&version.stdout) {
        return Err(Failure::Message(CLI_OLD.into()));
    }
    check_settings()?;

    let command = if credits { "credits" } else { "usage" };
    let arguments = if credits {
        "--disable-slash-commands=false -p /credits --output-format json"
    } else {
        "--disable-slash-commands=false -p /usage --output-format json"
    };
    let result = run(exe, arguments, &work_dir, Duration::from_secs(20))?;
    let json = serde_json::from_slice::<Value>(without_bom(&result.stdout)).unwrap_or(Value::Null);
    if result.code == 0
        && json["status"].as_str() == Some("SUCCESS")
        && json["num_turns"].as_u64() == Some(0)
        && json["command"]["name"].as_str() == Some(command)
        && json["command"]["data"].is_object()
    {
        return Ok(json);
    }
    Err(classify_failure(&result, &json))
}

fn without_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}

fn supports_read_only_report(stdout: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(without_bom(stdout)) else {
        return false;
    };
    let line = text.trim().lines().next().unwrap_or_default();
    let version = [
        "Antigravity CLI version ",
        "Antigravity CLI ",
        "agy version ",
        "agy ",
    ]
    .iter()
    .find_map(|prefix| line.strip_prefix(prefix))
    .unwrap_or(line)
    .trim();
    let version = version.strip_prefix('v').unwrap_or(version);
    let version = version
        .split_once('+')
        .map_or(version, |(version, _)| version);
    let numbers = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>();
    matches!(numbers.as_deref(), Ok([major, minor, patch]) if (*major, *minor, *patch) >= (1, 1, 11))
}

fn check_settings() -> Result<(), Failure> {
    let home = env::var_os("USERPROFILE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(env::home_dir)
        .ok_or_else(|| Failure::Message(SETTINGS_ERROR.into()))?;
    check_settings_file(&home.join(".gemini/antigravity-cli/settings.json"))?;
    if let Some(root) = env::var_os("ANTIGRAVITY_HOME").filter(|value| !value.is_empty()) {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return Err(Failure::Message(SETTINGS_ERROR.into()));
        }
        check_settings_file(&root.join("settings.json"))?;
        check_settings_file(&root.join("antigravity-cli/settings.json"))?;
    }
    Ok(())
}

fn check_settings_file(path: &Path) -> Result<(), Failure> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(Failure::Message(SETTINGS_ERROR.into())),
    };
    let bad_settings = || Failure::Message(SETTINGS_ERROR.into());
    let bytes = read_bounded(file, MAX_REPORT).map_err(|_| bad_settings())?;
    let value = serde_json::from_slice::<Value>(without_bom(&bytes)).map_err(|_| bad_settings())?;
    let settings = value.as_object().ok_or_else(bad_settings)?;
    match settings.get("disableSlashCommands") {
        Some(Value::Bool(true)) => Err(Failure::Message(SLASH_DISABLED.into())),
        Some(Value::Bool(false)) | None => Ok(()),
        _ => Err(bad_settings()),
    }
}

fn classify_failure(output: &Output, json: &Value) -> Failure {
    // 来自 CLI 的诊断只用于本地分类，界面和日志使用固定提示。
    let text_error = if output.code != 0 && json.is_null() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        "".into()
    };
    let diagnostic = format!(
        "{}\n{}\n{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        json["error"].as_str().unwrap_or_default(),
        json["response"].as_str().unwrap_or_default(),
        text_error,
    )
    .to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| diagnostic.contains(word));
    if has(&["ai studio"]) && has(&["api key", "gemini_api_key"]) {
        Failure::UnsupportedMode(Mode::ApiKey)
    } else if has(&["workspace", "organization", "administrator"])
        && has(&[
            "has disabled access",
            "access is disabled",
            "turned off access",
        ])
    {
        Failure::UnsupportedMode(Mode::WorkspaceDenied)
    } else if has(&["pay-as-you-go", "pay as you go"])
        && has(&["no quota", "not available", "not supported"])
    {
        Failure::UnsupportedMode(Mode::Cloud)
    } else if has(&["enterprise", "business account"])
        && has(&["no quota", "not available", "not supported"])
    {
        Failure::UnsupportedMode(Mode::Enterprise)
    } else if has(&["workspace account"]) && has(&["no quota", "not available", "not supported"]) {
        Failure::UnsupportedMode(Mode::Workspace)
    } else if has(&[
        "authentication required",
        "not logged in",
        "not signed in",
        "login required",
        "unauthenticated",
        "please sign in",
        "please authenticate",
    ]) {
        Failure::Login
    } else if has(&["too many requests", "resource_exhausted", "status code 429"]) {
        Failure::Temporary("Antigravity 额度查询暂时受限".into())
    } else {
        Failure::Temporary("暂未读到 Antigravity 额度，请稍后重试".into())
    }
}

struct Output {
    code: u32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn handle(value: &OwnedHandle) -> HANDLE {
    HANDLE(value.as_raw_handle())
}

fn pipe() -> windows::core::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe {
        CreatePipe(&mut read, &mut write, None, 0)?;
        Ok((
            OwnedHandle::from_raw_handle(read.0),
            OwnedHandle::from_raw_handle(write.0),
        ))
    }
}

struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}

impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

fn start(
    exe: &Path,
    arguments: &str,
    dir: &Path,
) -> windows::core::Result<(OwnedHandle, OwnedHandle, File, File)> {
    let job = unsafe { OwnedHandle::from_raw_handle(CreateJobObjectW(None, PCWSTR::null())?.0) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // 仅允许 CLI 单进程运行，阻止其在 DETACHED_PROCESS 下派生的 MCP 等控制台孙进程弹出终端窗口。
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    unsafe {
        SetInformationJobObject(
            handle(&job),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
    }
    let (stdin, stdin_writer) = pipe()?;
    let (stdout, stdout_writer) = pipe()?;
    let (stderr, stderr_writer) = pipe()?;
    let handles = [
        handle(&stdin),
        handle(&stdout_writer),
        handle(&stderr_writer),
    ];
    let job_handles = [handle(&job)];
    for child in handles {
        unsafe { SetHandleInformation(child, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT)? };
    }
    drop(stdin_writer);

    // 在创建时指定 Job 和允许继承的句柄，子进程启动前即受 Job 管理。
    let mut bytes = 0;
    let _ = unsafe {
        InitializeProcThreadAttributeList(LPPROC_THREAD_ATTRIBUTE_LIST::default(), 2, 0, &mut bytes)
    };
    let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
    unsafe { InitializeProcThreadAttributeList(list, 2, 0, &mut bytes)? };
    let attrs = Attributes {
        list,
        _storage: storage,
    };
    unsafe {
        UpdateProcThreadAttribute(
            attrs.list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(handles.as_ptr().cast()),
            size_of_val(&handles),
            None,
            None,
        )?;
        UpdateProcThreadAttribute(
            attrs.list,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            Some(job_handles.as_ptr().cast()),
            size_of_val(&job_handles),
            None,
            None,
        )?;
    }
    let startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: handles[0],
            hStdOutput: handles[1],
            hStdError: handles[2],
            ..Default::default()
        },
        lpAttributeList: attrs.list,
    };
    let application: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let directory: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    // 应用路径单独交给 Windows，参数固定；argv[0] 仍提供实际安装位置。
    let mut command: Vec<u16> = std::iter::once(b'"' as u16)
        .chain(exe.as_os_str().encode_wide())
        .chain("\" ".encode_utf16())
        .chain(arguments.encode_utf16())
        .chain([0])
        .collect();
    let environment = child_environment();
    let mut process = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            PWSTR(command.as_mut_ptr()),
            None,
            None,
            true,
            // 隐藏的控制台仍可打开 CONIN$，CLI 会据此发起交互登录。
            EXTENDED_STARTUPINFO_PRESENT | DETACHED_PROCESS | CREATE_UNICODE_ENVIRONMENT,
            Some(environment.as_ptr().cast()),
            PCWSTR(directory.as_ptr()),
            &startup.StartupInfo,
            &mut process,
        )?;
        drop(OwnedHandle::from_raw_handle(process.hThread.0));
        Ok((
            job,
            OwnedHandle::from_raw_handle(process.hProcess.0),
            stdout.into(),
            stderr.into(),
        ))
    }
}

fn child_environment() -> Vec<u16> {
    let mut entries: Vec<_> = env::vars_os()
        .filter(|(key, _)| !key.eq_ignore_ascii_case(UPDATE_KEY))
        .collect();
    // 每次查询沿用当前环境，只关闭本次官方 CLI 进程的自动更新。
    entries.push((UPDATE_KEY.into(), "true".into()));
    entries.sort_unstable_by_key(|(key, _)| key.to_string_lossy().to_uppercase());
    entries
        .into_iter()
        .flat_map(|(mut key, value)| {
            key.push("=");
            key.push(value);
            key.encode_wide().chain([0]).collect::<Vec<_>>()
        })
        .chain([0])
        .collect()
}

fn read_bounded(read: impl Read, maximum: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    read.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

fn read_pipe(pipe: File, limit: u64) -> io::Result<Receiver<io::Result<Vec<u8>>>> {
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::Builder::new().spawn(move || {
        let _ = send.send(read_bounded(pipe, limit));
    })?;
    Ok(receive)
}

fn run(exe: &Path, arguments: &str, dir: &Path, timeout: Duration) -> Result<Output, Failure> {
    let deadline = Instant::now() + timeout;
    let start_error = |_| Failure::Message(START_ERROR.into());
    let (job, process, stdout, stderr) = start(exe, arguments, dir).map_err(start_error)?;
    let pipe_error = |_| Failure::Temporary("无法读取 Antigravity CLI 输出".into());
    let stdout = read_pipe(stdout, MAX_REPORT).map_err(pipe_error)?;
    let stderr = read_pipe(stderr, MAX_DIAGNOSTICS).map_err(pipe_error)?;
    let wait = deadline
        .saturating_duration_since(Instant::now())
        .as_millis() as u32;
    match unsafe { WaitForSingleObject(handle(&process), wait) } {
        WAIT_OBJECT_0 => {}
        WAIT_TIMEOUT => return Err(Failure::Temporary(TIMEOUT.into())),
        _ => return Err(Failure::Temporary("无法读取 Antigravity CLI 状态".into())),
    }
    let mut code = 0;
    unsafe { GetExitCodeProcess(handle(&process), &mut code) }
        .map_err(|_| Failure::Temporary("无法读取 Antigravity CLI 状态".into()))?;
    drop(job);
    let receive = |pipe: Receiver<io::Result<Vec<u8>>>| {
        let wait = deadline.saturating_duration_since(Instant::now());
        pipe.recv_timeout(wait)
            .map_err(|_| Failure::Temporary(TIMEOUT.into()))?
            .map_err(pipe_error)
    };
    Ok(Output {
        code,
        stdout: receive(stdout)?,
        stderr: receive(stderr)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_process_has_no_console() {
        let exe = env::current_exe().unwrap();
        let module = module_path!().split_once("::").unwrap().1;
        let args = format!("--exact {module}::quota_no_console_fixture --ignored");
        let output = run(&exe, &args, exe.parent().unwrap(), Duration::from_secs(5))
            .ok()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.code, 0, "{stdout}");
        assert!(stdout.contains("1 passed"), "{stdout}");
    }

    #[test]
    #[ignore = "供额度子进程测试启动"]
    fn quota_no_console_fixture() {
        use windows::{
            core::w,
            Win32::{
                Foundation::{GENERIC_READ, GENERIC_WRITE},
                Storage::FileSystem::{
                    CreateFileW, GetFileType, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ,
                    FILE_SHARE_WRITE, FILE_TYPE_PIPE, OPEN_EXISTING,
                },
            },
        };

        let stdin = HANDLE(io::stdin().as_raw_handle());
        assert_eq!(unsafe { GetFileType(stdin) }, FILE_TYPE_PIPE);
        let console_input = unsafe {
            CreateFileW(
                w!("CONIN$"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        assert!(console_input.is_err(), "后台额度查询不应获得控制台输入");
    }

    #[test]
    fn quota_process_cannot_spawn_child_process() {
        let exe = env::current_exe().unwrap();
        let module = module_path!().split_once("::").unwrap().1;
        let args = format!("--exact {module}::quota_no_child_process_fixture --ignored");
        let output = run(&exe, &args, exe.parent().unwrap(), Duration::from_secs(5))
            .ok()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.code, 0, "{stdout}");
        assert!(stdout.contains("1 passed"), "{stdout}");
    }

    #[test]
    #[ignore = "供额度子进程测试启动"]
    fn quota_no_child_process_fixture() {
        let exe = env::current_exe().unwrap();
        if let Ok(mut child) = std::process::Command::new(&exe).arg("--list").spawn() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("后台额度查询不应派生孙进程");
        }
    }

    #[test]
    fn version_must_support_read_only_slash_commands() {
        assert!(!supports_read_only_report(b"1.1.10"));
        assert!(!supports_read_only_report(
            b"Antigravity CLI v1.1.11-preview.2"
        ));
        assert!(!supports_read_only_report(b"unverified client 1.5.13"));
        assert!(supports_read_only_report(b"agy version 1.1.11\r\n"));
        assert!(supports_read_only_report(b"Antigravity CLI v1.5.13"));
        assert!(supports_read_only_report(b"1.10.0"));
    }

    #[test]
    fn unsupported_login_type_requires_a_matching_diagnostic() {
        let mut output = Output {
            code: 2,
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert!(matches!(
            classify_failure(&output, &Value::Null),
            Failure::Temporary(_)
        ));
        output.stderr = b"No account quota when using a Gemini API key. Check AI Studio.".to_vec();
        assert!(matches!(
            classify_failure(&output, &Value::Null),
            Failure::UnsupportedMode(Mode::ApiKey)
        ));
        output.stderr = b"authentication required".to_vec();
        assert!(matches!(
            classify_failure(&output, &Value::Null),
            Failure::Login
        ));
    }

    #[test]
    fn process_runner_collects_output_and_enforces_timeout() {
        let exe = env::current_exe().unwrap();
        let dir = exe.parent().unwrap();
        let output = run(&exe, "--list", dir, Duration::from_secs(5))
            .ok()
            .unwrap();
        assert_eq!(output.code, 0);
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("process_runner_collects_output_and_enforces_timeout"));
        assert!(read_bounded(&b"123"[..], 2).is_err());

        let module = module_path!().split_once("::").unwrap().1;
        let args = format!("--exact {module}::process_runner_timeout_fixture --ignored");
        assert!(matches!(
            run(&exe, &args, dir, Duration::from_millis(250)),
            Err(Failure::Temporary(message)) if message == TIMEOUT
        ));
    }

    #[test]
    #[ignore = "需要通过 DAOZAI_TEST_OFFICIAL_AGY 指定官方签名的 CLI，仅执行版本查询"]
    fn official_google_cli_passes_the_native_version_probe() {
        let exe = PathBuf::from(env::var_os("DAOZAI_TEST_OFFICIAL_AGY").unwrap())
            .canonicalize()
            .unwrap();
        let _verified = super::super::lock_official_cli(&exe).expect("官方 CLI 应通过签名校验");
        let output = run(
            &exe,
            "--disable-slash-commands=false --version",
            &env::temp_dir(),
            Duration::from_secs(10),
        )
        .ok()
        .expect("官方 CLI 应返回版本报告");
        assert_eq!(output.code, 0);
        assert!(supports_read_only_report(&output.stdout));
    }

    #[test]
    #[ignore = "供额度子进程测试启动"]
    fn process_runner_timeout_fixture() {
        std::thread::sleep(Duration::from_secs(3));
    }
}
