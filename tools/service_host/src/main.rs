//! makepad-service-host: runs one program as a Windows service.
//!
//! The service's command line names a config file:
//!     makepad-service-host.exe C:\ai\services\MakepadAiNode.cfg
//! and `--console` after it runs the same supervisor in the foreground.
//!
//! Config, one `key=value` per line (`#` starts a comment line):
//!     name=MakepadAiNode        service name
//!     session=user              user: in the logged-on console user's session
//!                               as that user when there is one, else as the
//!                               service account in session 0; system: always
//!                               as the service account
//!     cwd=C:\ai\services        working directory
//!     log=C:\ai\services\logs\MakepadAiNode.log   stdout+stderr, appended
//!     path=C:\some\bin          prepended to PATH (repeatable)
//!     env=KEY=VALUE             set for the program (repeatable)
//!     exe=C:\ai\services\makepad-ai-hub.exe
//!     arg=--port                one argument per line (repeatable)
//!
//! The program is restarted when it exits (backing off when it keeps
//! exiting) and when the console session's user changes, so a program that
//! watches the user's own session (foreground window, controllers) runs
//! there. Everything it starts is in one job object: stopping the service
//! stops all of it, except processes started with breakaway.

#[cfg(not(windows))]
fn main() {
    eprintln!("makepad-service-host runs on Windows only");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    win::main();
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    type Handle = *mut c_void;

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut c_void,
        inherit: i32,
    }
    #[repr(C)]
    struct StartupInfo {
        cb: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_chars: u32,
        y_chars: u32,
        fill: u32,
        flags: u32,
        show_window: u16,
        reserved2_len: u16,
        reserved2: *mut u8,
        std_input: Handle,
        std_output: Handle,
        std_error: Handle,
    }
    #[repr(C)]
    struct ProcessInformation {
        process: Handle,
        thread: Handle,
        process_id: u32,
        thread_id: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct JobBasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct JobExtendedLimits {
        basic: JobBasicLimits,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }
    #[repr(C)]
    struct ServiceStatus {
        service_type: u32,
        current_state: u32,
        controls_accepted: u32,
        win32_exit_code: u32,
        service_exit_code: u32,
        check_point: u32,
        wait_hint: u32,
    }
    #[repr(C)]
    struct ServiceTableEntry {
        name: *mut u16,
        main: Option<unsafe extern "system" fn(u32, *mut *mut u16)>,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateEventW(sa: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
        fn SetEvent(event: Handle) -> i32;
        fn WaitForMultipleObjects(count: u32, handles: *const Handle, all: i32, ms: u32) -> u32;
        fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
        fn CreateProcessW(
            app: *const u16,
            cmd: *mut u16,
            psa: *const c_void,
            tsa: *const c_void,
            inherit: i32,
            flags: u32,
            env: *const c_void,
            cwd: *const u16,
            si: *const StartupInfo,
            pi: *mut ProcessInformation,
        ) -> i32;
        fn CreateJobObjectW(sa: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, code: u32) -> i32;
        fn ResumeThread(thread: Handle) -> u32;
        fn TerminateProcess(process: Handle, code: u32) -> i32;
        fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
        fn WTSGetActiveConsoleSessionId() -> u32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            sa: *const SecurityAttributes,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn GetLastError() -> u32;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn CreateProcessAsUserW(
            token: Handle,
            app: *const u16,
            cmd: *mut u16,
            psa: *const c_void,
            tsa: *const c_void,
            inherit: i32,
            flags: u32,
            env: *const c_void,
            cwd: *const u16,
            si: *const StartupInfo,
            pi: *mut ProcessInformation,
        ) -> i32;
        fn StartServiceCtrlDispatcherW(table: *const ServiceTableEntry) -> i32;
        fn RegisterServiceCtrlHandlerExW(
            name: *const u16,
            handler: unsafe extern "system" fn(u32, u32, *mut c_void, *mut c_void) -> u32,
            context: *mut c_void,
        ) -> Handle;
        fn SetServiceStatus(handle: Handle, status: *const ServiceStatus) -> i32;
    }
    #[link(name = "wtsapi32")]
    extern "system" {
        fn WTSQueryUserToken(session: u32, token: *mut Handle) -> i32;
    }
    #[link(name = "userenv")]
    extern "system" {
        fn CreateEnvironmentBlock(env: *mut *mut c_void, token: Handle, inherit: i32) -> i32;
        fn DestroyEnvironmentBlock(env: *mut c_void) -> i32;
    }

    const INVALID_HANDLE: Handle = -1isize as Handle;
    const NO_SESSION: u32 = 0xFFFF_FFFF;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x102;
    const CREATE_SUSPENDED: u32 = 0x4;
    const CREATE_UNICODE_ENVIRONMENT: u32 = 0x400;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const STARTF_USESHOWWINDOW: u32 = 0x1;
    const STARTF_USESTDHANDLES: u32 = 0x100;
    const JOB_LIMIT_BREAKAWAY_OK: u32 = 0x800;
    const JOB_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_EXTENDED_LIMIT_CLASS: i32 = 9;
    const SERVICE_WIN32_OWN_PROCESS: u32 = 0x10;
    const SERVICE_STOPPED: u32 = 1;
    const SERVICE_START_PENDING: u32 = 2;
    const SERVICE_STOP_PENDING: u32 = 3;
    const SERVICE_RUNNING: u32 = 4;
    const ACCEPT_STOP_SHUTDOWN_SESSION: u32 = 0x1 | 0x4 | 0x80;
    const CONTROL_STOP: u32 = 1;
    const CONTROL_INTERROGATE: u32 = 4;
    const CONTROL_SHUTDOWN: u32 = 5;
    const CONTROL_SESSIONCHANGE: u32 = 0xE;
    const LOG_ROTATE_BYTES: u64 = 64 << 20;

    struct Config {
        path: String,
        name: String,
        user_session: bool,
        cwd: String,
        log: String,
        path_prepend: Vec<String>,
        env: Vec<(String, String)>,
        exe: String,
        args: Vec<String>,
    }

    static CONFIG: OnceLock<Config> = OnceLock::new();
    static WAKE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    static STATUS: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    static STOP: AtomicBool = AtomicBool::new(false);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn load_config(path: &str) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let mut c = Config {
            path: path.to_string(),
            name: String::new(),
            user_session: false,
            cwd: String::new(),
            log: String::new(),
            path_prepend: Vec::new(),
            env: Vec::new(),
            exe: String::new(),
            args: Vec::new(),
        };
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_start_matches('\u{feff}').trim_end_matches('\r');
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or(format!("{path}:{}: expected key=value", i + 1))?;
            let value = value.to_string();
            match key.trim() {
                "name" => c.name = value,
                "session" => match value.as_str() {
                    "user" => c.user_session = true,
                    "system" => c.user_session = false,
                    _ => return Err(format!("{path}:{}: session is user or system", i + 1)),
                },
                "cwd" => c.cwd = value,
                "log" => c.log = value,
                "path" => c.path_prepend.push(value),
                "env" => {
                    let (k, v) = value
                        .split_once('=')
                        .ok_or(format!("{path}:{}: env=KEY=VALUE", i + 1))?;
                    c.env.push((k.to_string(), v.to_string()));
                }
                "exe" => c.exe = value,
                "arg" => c.args.push(value),
                other => return Err(format!("{path}:{}: unknown key {other}", i + 1)),
            }
        }
        for (key, value) in [("name", &c.name), ("cwd", &c.cwd), ("log", &c.log), ("exe", &c.exe)] {
            if value.is_empty() {
                return Err(format!("{path}: {key}= is required"));
            }
        }
        Ok(c)
    }

    fn timestamp() -> String {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
        let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
        // Civil date from days since 1970-01-01 (proleptic Gregorian).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
            rem / 3600,
            rem / 60 % 60,
            rem % 60
        )
    }

    fn host_log(cfg: &Config, message: &str) {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&cfg.log) {
            let _ = writeln!(file, "[service-host {}] {message}", timestamp());
        }
        if STATUS.load(Ordering::SeqCst).is_null() {
            eprintln!("[service-host] {message}");
        }
    }

    /// Windows argument quoting (CommandLineToArgvW rules).
    fn quote(arg: &str, out: &mut String) {
        if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
            out.push_str(arg);
            return;
        }
        out.push('"');
        let mut slashes = 0;
        for ch in arg.chars() {
            match ch {
                '\\' => slashes += 1,
                '"' => {
                    out.extend(std::iter::repeat('\\').take(slashes * 2 + 1));
                    out.push('"');
                    slashes = 0;
                }
                _ => {
                    out.extend(std::iter::repeat('\\').take(slashes));
                    out.push(ch);
                    slashes = 0;
                }
            }
        }
        out.extend(std::iter::repeat('\\').take(slashes * 2));
        out.push('"');
    }

    fn command_line(cfg: &Config) -> Vec<u16> {
        let mut line = String::new();
        quote(&cfg.exe, &mut line);
        for arg in &cfg.args {
            line.push(' ');
            quote(arg, &mut line);
        }
        wide(&line)
    }

    /// The program's environment: the base block with the config applied,
    /// as a sorted, double-NUL-terminated UTF-16 block.
    fn environment(cfg: &Config, mut vars: Vec<(String, String)>) -> Vec<u16> {
        fn set(vars: &mut Vec<(String, String)>, key: &str, value: String) {
            match vars.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
                Some(entry) => entry.1 = value,
                None => vars.push((key.to_string(), value)),
            }
        }
        for (key, value) in &cfg.env {
            set(&mut vars, key, value.clone());
        }
        if !cfg.path_prepend.is_empty() {
            let old = vars
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let mut path = cfg.path_prepend.join(";");
            if !old.is_empty() {
                path.push(';');
                path.push_str(&old);
            }
            set(&mut vars, "Path", path);
        }
        vars.sort_by_key(|(k, _)| k.to_uppercase());
        let mut block = Vec::new();
        for (key, value) in vars {
            block.extend(format!("{key}={value}").encode_utf16());
            block.push(0);
        }
        block.push(0);
        block
    }

    fn user_environment(token: Handle) -> Option<Vec<(String, String)>> {
        let mut env = null_mut();
        if unsafe { CreateEnvironmentBlock(&mut env, token, 0) } == 0 || env.is_null() {
            return None;
        }
        let mut vars = Vec::new();
        let mut p = env as *const u16;
        unsafe {
            loop {
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                if len == 0 {
                    break;
                }
                let entry = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                // Per-drive cwd entries ("=C:=C:\") have an empty name; skip them.
                if let Some((k, v)) = entry.split_once('=').filter(|(k, _)| !k.is_empty()) {
                    vars.push((k.to_string(), v.to_string()));
                }
                p = p.add(len + 1);
            }
            DestroyEnvironmentBlock(env);
        }
        Some(vars)
    }

    /// The session the program should run in: the console user's, or
    /// None for the service account in session 0.
    fn desired_session(cfg: &Config) -> Option<u32> {
        if !cfg.user_session {
            return None;
        }
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if session == NO_SESSION || session == 0 {
            return None;
        }
        let mut token = null_mut();
        if unsafe { WTSQueryUserToken(session, &mut token) } == 0 {
            return None;
        }
        unsafe { CloseHandle(token) };
        Some(session)
    }

    fn open_log(cfg: &Config) -> Handle {
        if let Ok(meta) = std::fs::metadata(&cfg.log) {
            if meta.len() > LOG_ROTATE_BYTES {
                let _ = std::fs::rename(&cfg.log, format!("{}.prev", cfg.log));
            }
        }
        let sa = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            descriptor: null_mut(),
            inherit: 1,
        };
        // FILE_APPEND_DATA | SYNCHRONIZE; share read/write/delete; OPEN_ALWAYS.
        unsafe { CreateFileW(wide(&cfg.log).as_ptr(), 0x4 | 0x0010_0000, 0x7, &sa, 4, 0x80, null_mut()) }
    }

    fn open_nul() -> Handle {
        let sa = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            descriptor: null_mut(),
            inherit: 1,
        };
        // GENERIC_READ, OPEN_EXISTING.
        unsafe { CreateFileW(wide("NUL").as_ptr(), 0x8000_0000, 0x3, &sa, 3, 0, null_mut()) }
    }

    fn new_job() -> Handle {
        let job = unsafe { CreateJobObjectW(null(), null()) };
        let mut limits = JobExtendedLimits::default();
        limits.basic.flags = JOB_LIMIT_KILL_ON_JOB_CLOSE | JOB_LIMIT_BREAKAWAY_OK;
        unsafe {
            SetInformationJobObject(
                job,
                JOB_EXTENDED_LIMIT_CLASS,
                &limits as *const _ as *const c_void,
                std::mem::size_of::<JobExtendedLimits>() as u32,
            );
        }
        job
    }

    /// Starts the program suspended, puts it in the job, then resumes it.
    fn launch(cfg: &Config, job: Handle, session: Option<u32>) -> Result<Handle, String> {
        let log = open_log(cfg);
        if log == INVALID_HANDLE {
            return Err(format!("cannot open log {} (error {})", cfg.log, unsafe { GetLastError() }));
        }
        let nul = open_nul();
        let mut desktop = wide("winsta0\\default");
        let si = StartupInfo {
            cb: std::mem::size_of::<StartupInfo>() as u32,
            reserved: null_mut(),
            desktop: desktop.as_mut_ptr(),
            title: null_mut(),
            x: 0,
            y: 0,
            x_size: 0,
            y_size: 0,
            x_chars: 0,
            y_chars: 0,
            fill: 0,
            flags: STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW,
            show_window: 0,
            reserved2_len: 0,
            reserved2: null_mut(),
            std_input: if nul == INVALID_HANDLE { null_mut() } else { nul },
            std_output: log,
            std_error: log,
        };
        let mut pi = ProcessInformation { process: null_mut(), thread: null_mut(), process_id: 0, thread_id: 0 };
        let mut cmd = command_line(cfg);
        let cwd = wide(&cfg.cwd);
        let flags = CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW;
        let mut token = null_mut();
        let user = session.is_some_and(|s| unsafe { WTSQueryUserToken(s, &mut token) } != 0);
        let ok = if user {
            let base = user_environment(token).unwrap_or_else(|| std::env::vars().collect());
            let env = environment(cfg, base);
            let ok = unsafe {
                CreateProcessAsUserW(
                    token,
                    null(),
                    cmd.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    flags,
                    env.as_ptr() as *const c_void,
                    cwd.as_ptr(),
                    &si,
                    &mut pi,
                )
            };
            unsafe { CloseHandle(token) };
            ok
        } else {
            let env = environment(cfg, std::env::vars().collect());
            unsafe {
                CreateProcessW(
                    null(),
                    cmd.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    flags,
                    env.as_ptr() as *const c_void,
                    cwd.as_ptr(),
                    &si,
                    &mut pi,
                )
            }
        };
        let error = unsafe { GetLastError() };
        unsafe {
            CloseHandle(log);
            if nul != INVALID_HANDLE {
                CloseHandle(nul);
            }
        }
        if ok == 0 {
            return Err(format!("cannot start {} (error {error})", cfg.exe));
        }
        unsafe {
            if AssignProcessToJobObject(job, pi.process) == 0 {
                let error = GetLastError();
                TerminateProcess(pi.process, 1);
                CloseHandle(pi.thread);
                CloseHandle(pi.process);
                return Err(format!("cannot add the program to the job (error {error})"));
            }
            ResumeThread(pi.thread);
            CloseHandle(pi.thread);
        }
        let place = match (user, session) {
            (true, Some(s)) => format!("as the console user in session {s}"),
            _ => "with the host's own account (session 0 when run as a service)".to_string(),
        };
        host_log(cfg, &format!("started pid {} {place}", pi.process_id));
        Ok(pi.process)
    }

    fn stop_all(job: Handle, process: Handle) {
        unsafe {
            TerminateJobObject(job, 1);
            WaitForSingleObject(process, 15_000);
        }
    }

    /// Waits up to `ms` for a stop request. True when stopping.
    fn sleep_unless_stopped(ms: u32) -> bool {
        let wake = WAKE.load(Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_millis(u64::from(ms));
        while !STOP.load(Ordering::SeqCst) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            unsafe { WaitForSingleObject(wake, left.as_millis() as u32) };
        }
        STOP.load(Ordering::SeqCst)
    }

    fn supervise(cfg: &Config) {
        let job = new_job();
        let wake = WAKE.load(Ordering::SeqCst);
        let mut backoff_ms = 2_000;
        host_log(cfg, &format!("{} supervising {} ({})", cfg.name, cfg.exe, cfg.path));
        while !STOP.load(Ordering::SeqCst) {
            let session = desired_session(cfg);
            let process = match launch(cfg, job, session) {
                Ok(process) => process,
                Err(error) => {
                    host_log(cfg, &format!("{error}; retrying in {} s", backoff_ms / 1000));
                    if sleep_unless_stopped(backoff_ms) {
                        break;
                    }
                    backoff_ms = (backoff_ms * 2).min(60_000);
                    continue;
                }
            };
            let started = Instant::now();
            let handles = [process, wake];
            let mut restart_now = false;
            loop {
                let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 30_000) };
                if wait == WAIT_OBJECT_0 {
                    let mut code = 0;
                    unsafe { GetExitCodeProcess(process, &mut code) };
                    host_log(cfg, &format!("program exited with code {code:#x} after {} s", started.elapsed().as_secs()));
                    break;
                }
                if STOP.load(Ordering::SeqCst) {
                    host_log(cfg, "stopping");
                    stop_all(job, process);
                    break;
                }
                if wait == WAIT_OBJECT_0 + 1 || wait == WAIT_TIMEOUT {
                    let now = desired_session(cfg);
                    if now != session {
                        host_log(cfg, &format!("console user changed ({session:?} -> {now:?}); restarting there"));
                        stop_all(job, process);
                        restart_now = true;
                        break;
                    }
                    continue;
                }
                // WAIT_FAILED: never spin; treat the program as gone.
                host_log(cfg, &format!("wait failed (error {})", unsafe { GetLastError() }));
                stop_all(job, process);
                break;
            }
            unsafe { CloseHandle(process) };
            if STOP.load(Ordering::SeqCst) {
                break;
            }
            if restart_now {
                continue;
            }
            if started.elapsed() > Duration::from_secs(120) {
                backoff_ms = 2_000;
            }
            if sleep_unless_stopped(backoff_ms) {
                break;
            }
            backoff_ms = (backoff_ms * 2).min(60_000);
        }
        unsafe { CloseHandle(job) };
        host_log(cfg, "stopped");
    }

    fn report(state: u32, wait_hint: u32) {
        let status = ServiceStatus {
            service_type: SERVICE_WIN32_OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == SERVICE_RUNNING { ACCEPT_STOP_SHUTDOWN_SESSION } else { 0 },
            win32_exit_code: 0,
            service_exit_code: 0,
            check_point: 0,
            wait_hint,
        };
        unsafe { SetServiceStatus(STATUS.load(Ordering::SeqCst), &status) };
    }

    unsafe extern "system" fn control(code: u32, _event: u32, _data: *mut c_void, _ctx: *mut c_void) -> u32 {
        match code {
            CONTROL_STOP | CONTROL_SHUTDOWN => {
                STOP.store(true, Ordering::SeqCst);
                report(SERVICE_STOP_PENDING, 20_000);
                SetEvent(WAKE.load(Ordering::SeqCst));
                0
            }
            CONTROL_SESSIONCHANGE => {
                SetEvent(WAKE.load(Ordering::SeqCst));
                0
            }
            CONTROL_INTERROGATE => 0,
            _ => 120, // ERROR_CALL_NOT_IMPLEMENTED
        }
    }

    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
        let cfg = CONFIG.get().expect("config loaded before dispatch");
        let name = wide(&cfg.name);
        let status = RegisterServiceCtrlHandlerExW(name.as_ptr(), control, null_mut());
        if status.is_null() {
            host_log(cfg, &format!("cannot register the control handler (error {})", GetLastError()));
            return;
        }
        STATUS.store(status, Ordering::SeqCst);
        report(SERVICE_START_PENDING, 5_000);
        report(SERVICE_RUNNING, 0);
        supervise(cfg);
        report(SERVICE_STOPPED, 0);
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        let Some(path) = args.get(1) else {
            eprintln!("usage: makepad-service-host <config.cfg> [--console]");
            std::process::exit(2);
        };
        let cfg = match load_config(path) {
            Ok(cfg) => cfg,
            Err(error) => {
                eprintln!("makepad-service-host: {error}");
                std::process::exit(2);
            }
        };
        let console = args.iter().skip(2).any(|a| a == "--console");
        let _ = CONFIG.set(cfg);
        let cfg = CONFIG.get().unwrap();
        WAKE.store(unsafe { CreateEventW(null(), 0, 0, null()) }, Ordering::SeqCst);
        if console {
            supervise(cfg);
            return;
        }
        let mut name = wide(&cfg.name);
        let table = [
            ServiceTableEntry { name: name.as_mut_ptr(), main: Some(service_main) },
            ServiceTableEntry { name: null_mut(), main: None },
        ];
        if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
            let error = unsafe { GetLastError() };
            eprintln!("makepad-service-host: not started by the service manager (error {error}); use --console to run in the foreground");
            std::process::exit(1);
        }
    }
}
