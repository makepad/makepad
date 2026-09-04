//! Windows activity sampling. No hooks, input contents, titles, or GPU contexts.
//! All handles belong to the single monitor thread. FFI layouts are tested below.
#![allow(dead_code)]
use std::collections::{BTreeMap, BTreeSet};

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct WtsInfoLevel1 {
    session_id: u32,
    session_state: i32,
    session_flags: i32,
    station_name: [u16; 33],
    user_name: [u16; 21],
    domain_name: [u16; 18],
    logon_time: i64,
    connect_time: i64,
    disconnect_time: i64,
    last_input_time: i64,
    current_time: i64,
    incoming_bytes: u32,
    outgoing_bytes: u32,
    incoming_frames: u32,
    outgoing_frames: u32,
    incoming_compressed_bytes: u32,
    outgoing_compressed_bytes: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
union WtsInfoData {
    level1: WtsInfoLevel1,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct WtsInfoEx {
    level: u32,
    data: WtsInfoData,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Gamepad {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    left_x: i16,
    left_y: i16,
    right_x: i16,
    right_y: i16,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct XInputState {
    packet: u32,
    gamepad: Gamepad,
}
#[repr(C)]
#[derive(Clone, Copy)]
union PdhValueData {
    double_value: f64,
    long_value: i32,
    large_value: i64,
    string_value: *const u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct PdhValue {
    status: u32,
    value: PdhValueData,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct PdhItem {
    name: *const u16,
    value: PdhValue,
}

fn held_controls(pad: &Gamepad) -> bool {
    pad.buttons != 0
        || pad.left_trigger > 30
        || pad.right_trigger > 30
        || (pad.left_x as i32).abs() > 7849
        || (pad.left_y as i32).abs() > 7849
        || (pad.right_x as i32).abs() > 8689
        || (pad.right_y as i32).abs() > 8689
}

fn session_idle_seconds(current: i64, last: i64) -> Result<u64, &'static str> {
    if current <= 0 || last <= 0 || last > current {
        return Err("session_input_time_unknown");
    }
    Ok(((current - last) / 10_000_000) as u64)
}

fn engine_pid(name: &str) -> Option<u32> {
    let tail = name.strip_prefix("pid_")?;
    let (pid, rest) = tail.split_once('_')?;
    if !rest.starts_with("luid_") || !rest.contains("_eng_") || !rest.contains("_engtype_") {
        return None;
    }
    pid.parse().ok()
}

/// Sum foreign engine activity instead of averaging away a busy game among idle
/// engines. Cap the aggregate at 100; do not discard unfamiliar app processes.
fn foreign_load(
    values: &[(String, u32, f64)],
    excluded: &BTreeSet<u32>,
    previous: &mut Option<BTreeSet<String>>,
) -> Result<f64, &'static str> {
    let mut names = BTreeSet::new();
    let mut load = 0.0;
    let mut invalid = false;
    for (name, status, value) in values {
        let pid = engine_pid(name).ok_or("gpu_counter_instance_unknown")?;
        if excluded.contains(&pid) {
            continue;
        }
        names.insert(name.clone());
        if (*status != 0 && *status != 1) || !value.is_finite() || *value < 0.0 {
            invalid = true;
        } else {
            load += value.min(100.0);
        }
    }
    let changed = previous.as_ref() != Some(&names);
    *previous = Some(names);
    if invalid {
        Err("gpu_counter_sample_unknown")
    } else if changed {
        Err("gpu_counter_warmup_or_churn")
    } else {
        Ok(load.min(100.0))
    }
}

/// Creation times prevent PID reuse from making a foreign process look owned.
fn descendant_pids(root: u32, processes: &BTreeMap<u32, (u32, u64)>) -> BTreeSet<u32> {
    let mut owned = BTreeSet::from([root]);
    loop {
        let before = owned.len();
        for (&pid, &(parent, created)) in processes {
            if pid == root || created == 0 || !owned.contains(&parent) {
                continue;
            }
            if let Some(&(_, parent_created)) = processes.get(&parent) {
                if parent_created != 0 && created >= parent_created {
                    owned.insert(pid);
                }
            }
        }
        if owned.len() == before {
            return owned;
        }
    }
}

#[cfg(windows)]
pub(super) use native::Probe;

#[cfg(windows)]
mod native {
    use super::*;
    use std::{ffi::c_void, mem, ptr};
    type Handle = *mut c_void;
    type XInputGetState = unsafe extern "system" fn(u32, *mut XInputState) -> u32;
    const PDH_MORE_DATA: u32 = 0x8000_07d2;
    const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
    const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
    #[repr(C)]
    struct Session {
        id: u32,
        station: *mut u16,
        state: i32,
    }
    #[repr(C)]
    struct LastInput {
        size: u32,
        tick: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    struct MonitorInfo {
        size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        exe: [u16; 260],
    }
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "wtsapi32")]
    unsafe extern "system" {
        fn WTSEnumerateSessionsW(
            server: Handle,
            reserved: u32,
            version: u32,
            sessions: *mut *mut Session,
            count: *mut u32,
        ) -> i32;
        fn WTSQuerySessionInformationW(
            server: Handle,
            session: u32,
            class: i32,
            buffer: *mut *mut u16,
            bytes: *mut u32,
        ) -> i32;
        fn WTSFreeMemory(memory: *mut c_void);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcessId() -> u32;
        fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
        fn GetTickCount() -> u32;
        fn GetLastError() -> u32;
        fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle;
        fn FreeLibrary(module: Handle) -> i32;
        fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn GetProcessTimes(
            process: Handle,
            created: *mut FileTime,
            exited: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn GetSystemDirectoryW(path: *mut u16, size: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetLastInputInfo(info: *mut LastInput) -> i32;
        fn GetForegroundWindow() -> Handle;
        fn GetDesktopWindow() -> Handle;
        fn GetShellWindow() -> Handle;
        fn GetClassNameW(window: Handle, name: *mut u16, count: i32) -> i32;
        fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
        fn GetWindowRect(window: Handle, rect: *mut Rect) -> i32;
        fn MonitorFromWindow(window: Handle, flags: u32) -> Handle;
        fn GetMonitorInfoW(monitor: Handle, info: *mut MonitorInfo) -> i32;
        fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> Handle;
        fn CloseDesktop(desktop: Handle) -> i32;
        fn SetThreadDpiAwarenessContext(context: Handle) -> Handle;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHQueryUserNotificationState(state: *mut i32) -> i32;
    }
    #[link(name = "pdh")]
    unsafe extern "system" {
        fn PdhOpenQueryW(source: *const u16, user_data: usize, query: *mut Handle) -> u32;
        fn PdhAddEnglishCounterW(
            query: Handle,
            path: *const u16,
            user_data: usize,
            counter: *mut Handle,
        ) -> u32;
        fn PdhCollectQueryData(query: Handle) -> u32;
        fn PdhGetFormattedCounterArrayW(
            counter: Handle,
            format: u32,
            bytes: *mut u32,
            count: *mut u32,
            items: *mut PdhItem,
        ) -> u32;
        fn PdhCloseQuery(query: Handle) -> u32;
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    struct WtsMemory(*mut c_void);
    impl Drop for WtsMemory {
        fn drop(&mut self) {
            unsafe {
                WTSFreeMemory(self.0);
            }
        }
    }

    pub(in super::super) struct Probe {
        query: Handle,
        counter: Handle,
        collected: bool,
        previous: Option<BTreeSet<String>>,
        xinput_module: Handle,
        xinput: Option<XInputGetState>,
        controller_packets: [Option<u32>; 4],
    }
    impl Probe {
        pub(in super::super) fn new() -> Self {
            let mut result = Self {
                query: ptr::null_mut(),
                counter: ptr::null_mut(),
                collected: false,
                previous: None,
                xinput_module: ptr::null_mut(),
                xinput: None,
                controller_packets: [None; 4],
            };
            result.load_xinput();
            result
        }
        fn load_xinput(&mut self) {
            // Only system DLL search; never load a DLL from a writable CWD.
            for dll in ["xinput1_4.dll", "xinput1_3.dll", "xinput9_1_0.dll"] {
                let module = unsafe { LoadLibraryExW(wide(dll).as_ptr(), ptr::null_mut(), 0x800) };
                if module.is_null() {
                    continue;
                }
                let function = unsafe { GetProcAddress(module, b"XInputGetState\0".as_ptr()) };
                if function.is_null() {
                    unsafe {
                        FreeLibrary(module);
                    }
                    continue;
                }
                self.xinput_module = module;
                self.xinput =
                    Some(unsafe { mem::transmute::<*mut c_void, XInputGetState>(function) });
                return;
            }
        }
        pub(in super::super) fn sample(&mut self) -> super::super::Observation {
            let sessions = sample_sessions();
            let (idle_seconds, session_error, visible) = match sessions {
                Ok((idle, visible)) => (Some(idle), None, visible),
                Err(error) => (None, Some(error), false),
            };
            super::super::Observation {
                idle_seconds,
                session_error,
                fullscreen: if visible {
                    fullscreen()
                } else if session_error.is_none() {
                    Ok(false)
                } else {
                    Err("interactive_session_not_visible")
                },
                controller_active: if visible {
                    self.controllers()
                } else if session_error.is_none() {
                    Ok(false)
                } else {
                    Err("controller_session_not_visible")
                },
                foreign_gpu_percent: self.gpu_load(),
            }
        }
        fn controllers(&mut self) -> Result<bool, &'static str> {
            if self.xinput.is_none() {
                self.load_xinput();
            }
            let get_state = self.xinput.ok_or("controller_api_unavailable")?;
            let mut active = false;
            let mut failed = false;
            for slot in 0..4 {
                let mut state = XInputState::default();
                match unsafe { get_state(slot as u32, &mut state) } {
                    0 => {
                        // A held stick/button stays busy even when the packet is unchanged.
                        active |= held_controls(&state.gamepad)
                            || self.controller_packets[slot]
                                .map_or(true, |old| old != state.packet);
                        self.controller_packets[slot] = Some(state.packet);
                    }
                    1167 => {
                        self.controller_packets[slot] = None;
                    } // ERROR_DEVICE_NOT_CONNECTED
                    _ => failed = true,
                }
            }
            if failed {
                Err("controller_sample_unknown")
            } else {
                Ok(active)
            }
        }
        fn reset_pdh(&mut self) {
            if !self.query.is_null() {
                unsafe {
                    PdhCloseQuery(self.query);
                }
            }
            self.query = ptr::null_mut();
            self.counter = ptr::null_mut();
            self.collected = false;
            self.previous = None;
        }
        fn gpu_load(&mut self) -> Result<f64, &'static str> {
            if self.query.is_null() {
                if unsafe { PdhOpenQueryW(ptr::null(), 0, &mut self.query) } != 0 {
                    self.reset_pdh();
                    return Err("gpu_counter_unavailable");
                }
                let path = wide("\\GPU Engine(*)\\Utilization Percentage");
                if unsafe { PdhAddEnglishCounterW(self.query, path.as_ptr(), 0, &mut self.counter) }
                    != 0
                {
                    self.reset_pdh();
                    return Err("gpu_counter_unavailable");
                }
            }
            if unsafe { PdhCollectQueryData(self.query) } != 0 {
                self.reset_pdh();
                return Err("gpu_counter_collect_failed");
            }
            if !self.collected {
                self.collected = true;
                return Err("gpu_counter_warmup");
            }
            let values = match self.counter_values() {
                Ok(values) => values,
                Err(reason) => {
                    self.reset_pdh();
                    return Err(reason);
                }
            };
            let excluded = process_exclusions()?;
            foreign_load(&values, &excluded, &mut self.previous)
        }
        fn counter_values(&self) -> Result<Vec<(String, u32, f64)>, &'static str> {
            let mut bytes = 0;
            let mut count = 0;
            let format = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
            let status = unsafe {
                PdhGetFormattedCounterArrayW(
                    self.counter,
                    format,
                    &mut bytes,
                    &mut count,
                    ptr::null_mut(),
                )
            };
            if status != PDH_MORE_DATA || bytes == 0 || bytes > 64 * 1024 * 1024 {
                return Err("gpu_counter_instances_missing");
            }
            // u64 storage supplies the alignment required by PDH_FMT_COUNTERVALUE.
            // Retry boundedly if the counter list grows between size/fill calls.
            for _ in 0..3 {
                let mut storage = vec![0u64; (bytes as usize + 7) / 8];
                let capacity = storage.len() * 8;
                bytes = capacity as u32;
                let status = unsafe {
                    PdhGetFormattedCounterArrayW(
                        self.counter,
                        format,
                        &mut bytes,
                        &mut count,
                        storage.as_mut_ptr().cast(),
                    )
                };
                if status == PDH_MORE_DATA {
                    if bytes > 64 * 1024 * 1024 {
                        break;
                    }
                    continue;
                }
                if status != 0
                    || count == 0
                    || count as usize > capacity / mem::size_of::<PdhItem>()
                {
                    return Err("gpu_counter_array_unknown");
                }
                let start = storage.as_ptr() as usize;
                let end = start + capacity;
                let items = unsafe {
                    std::slice::from_raw_parts(storage.as_ptr().cast::<PdhItem>(), count as usize)
                };
                let mut result = Vec::with_capacity(items.len());
                for item in items {
                    let address = item.name as usize;
                    if address < start || address >= end || address % 2 != 0 {
                        return Err("gpu_counter_name_unknown");
                    }
                    let name =
                        unsafe { std::slice::from_raw_parts(item.name, (end - address) / 2) };
                    let length = name
                        .iter()
                        .position(|v| *v == 0)
                        .ok_or("gpu_counter_name_unknown")?;
                    let name = String::from_utf16(&name[..length])
                        .map_err(|_| "gpu_counter_name_unknown")?;
                    result.push((name, item.value.status, unsafe {
                        item.value.value.double_value
                    }));
                }
                return Ok(result);
            }
            Err("gpu_counter_array_churn")
        }
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            self.reset_pdh();
            if !self.xinput_module.is_null() {
                unsafe {
                    FreeLibrary(self.xinput_module);
                }
            }
        }
    }

    fn sample_sessions() -> Result<(u64, bool), &'static str> {
        let mut own_session = 0;
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut own_session) } == 0 {
            return Err("process_session_unknown");
        }
        let mut sessions = ptr::null_mut();
        let mut count = 0;
        if unsafe { WTSEnumerateSessionsW(ptr::null_mut(), 0, 1, &mut sessions, &mut count) } == 0 {
            return Err("session_enumeration_failed");
        }
        let _memory = WtsMemory(sessions.cast());
        if count > 4096 || (count != 0 && sessions.is_null()) {
            return Err("session_enumeration_invalid");
        }
        let entries = if count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(sessions, count as usize) }
        };
        let mut idle = u64::MAX;
        let mut same_active = false;
        for session in entries {
            // Listening/reset/down/init entries are not interactive users.
            if matches!(session.state, 6..=9) {
                continue;
            }
            if !matches!(session.state, 0..=5) {
                return Err("session_state_unknown");
            }
            let mut buffer = ptr::null_mut();
            let mut bytes = 0;
            if unsafe {
                WTSQuerySessionInformationW(
                    ptr::null_mut(),
                    session.id,
                    25,
                    &mut buffer,
                    &mut bytes,
                )
            } == 0
            {
                return Err("session_information_unavailable");
            }
            let _memory = WtsMemory(buffer.cast());
            if buffer.is_null() || (bytes as usize) < mem::size_of::<WtsInfoEx>() {
                return Err("session_information_invalid");
            }
            let info = unsafe { ptr::read_unaligned(buffer.cast::<WtsInfoEx>()) };
            if info.level != 1 {
                return Err("session_information_level_unknown");
            }
            let info = unsafe { info.data.level1 };
            if info.session_id != session.id {
                return Err("session_information_mismatch");
            }
            if info.session_state != session.state {
                return Err("session_changed_during_sample");
            }
            if info.user_name[0] == 0 {
                continue;
            }
            idle = idle.min(session_idle_seconds(
                info.current_time,
                info.last_input_time,
            )?);
            // WTS can inspect cross-session timestamps but foreground windows and
            // XInput cannot. Never assert that an invisible active user is idle.
            if matches!(session.state, 0..=3) {
                if session.id != own_session || own_session == 0 {
                    return Err("other_interactive_session_not_visible");
                }
                same_active = true;
            }
        }
        if same_active {
            let mut last = LastInput {
                size: mem::size_of::<LastInput>() as u32,
                tick: 0,
            };
            if unsafe { GetLastInputInfo(&mut last) } == 0 {
                return Err("last_input_unavailable");
            }
            let now = unsafe { GetTickCount() };
            // Modulo subtraction handles the 49-day tick wrap. A future input
            // tick or age beyond half the cycle is ambiguous, so fail closed
            // rather than converting it into an apparently very idle session.
            if now.wrapping_sub(last.tick) > i32::MAX as u32 {
                return Err("last_input_tick_ambiguous");
            }
            idle = idle.min(super::super::tick_idle_seconds(now, last.tick));
        }
        // No active user requires no session-specific foreground/controller probe.
        // A disconnected logged-in user is still included in the WTS idle minimum.
        Ok((idle, same_active))
    }

    fn fullscreen() -> Result<bool, &'static str> {
        // Secure/other input desktops are not visible from this process.
        let desktop = unsafe { OpenInputDesktop(0, 0, 0x0001) };
        if desktop.is_null() {
            return Err("input_desktop_unavailable");
        }
        unsafe {
            CloseDesktop(desktop);
        }
        let mut state = 0;
        if unsafe { SHQueryUserNotificationState(&mut state) } < 0 {
            return Err("notification_state_unknown");
        }
        match state {
            2 | 3 | 4 | 6 | 7 => return Ok(true),
            1 | 5 => {}
            _ => return Err("notification_state_unknown"),
        }
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return Err("foreground_window_unavailable");
        }
        let shell = unsafe { GetShellWindow() };
        if foreground == unsafe { GetDesktopWindow() } || (!shell.is_null() && foreground == shell)
        {
            return Ok(false);
        }
        let mut class = [0u16; 128];
        let length = unsafe { GetClassNameW(foreground, class.as_mut_ptr(), class.len() as i32) };
        if length <= 0 {
            return Err("foreground_class_unknown");
        }
        let class = String::from_utf16_lossy(&class[..length as usize]);
        if matches!(class.as_str(), "WorkerW" | "Progman") && !shell.is_null() {
            let mut foreground_pid = 0;
            let mut shell_pid = 0;
            unsafe {
                GetWindowThreadProcessId(foreground, &mut foreground_pid);
                GetWindowThreadProcessId(shell, &mut shell_pid);
            }
            if foreground_pid != 0 && foreground_pid == shell_pid {
                return Ok(false);
            }
        }
        // Compare monitor and window bounds in the same physical coordinate
        // space, including headless services on monitors above 100% scaling.
        let old_dpi = unsafe { SetThreadDpiAwarenessContext((-4isize) as Handle) };
        if old_dpi.is_null() {
            return Err("foreground_dpi_context_unknown");
        }
        struct RestoreDpi(Handle);
        impl Drop for RestoreDpi {
            fn drop(&mut self) {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
        let _dpi = RestoreDpi(old_dpi);
        let mut rect = Rect::default();
        if unsafe { GetWindowRect(foreground, &mut rect) } == 0 {
            return Err("foreground_rect_unknown");
        }
        let monitor = unsafe { MonitorFromWindow(foreground, 2) };
        let mut info = MonitorInfo {
            size: mem::size_of::<MonitorInfo>() as u32,
            monitor: Rect::default(),
            work: Rect::default(),
            flags: 0,
        };
        if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return Err("foreground_monitor_unknown");
        }
        // Covers borderless and oversized fullscreen windows. No window title,
        // process allowlist, or exclusive-mode requirement can hide a real game.
        Ok(rect.left <= info.monitor.left
            && rect.top <= info.monitor.top
            && rect.right >= info.monitor.right
            && rect.bottom >= info.monitor.bottom)
    }

    fn process_exclusions() -> Result<BTreeSet<u32>, &'static str> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(2, 0) };
        if snapshot.is_null() || snapshot as isize == -1 {
            return Err("gpu_process_snapshot_unavailable");
        }
        let snapshot = OwnedHandle(snapshot);
        let mut entry: ProcessEntry = unsafe { mem::zeroed() };
        entry.size = mem::size_of::<ProcessEntry>() as u32;
        if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err("gpu_process_snapshot_empty");
        }
        let mut processes = BTreeMap::new();
        let mut compositor = BTreeSet::new();
        let mut directory = [0u16; 32768];
        let size =
            unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) } as usize;
        let dwm_path = if size > 0 && size < directory.len() {
            Some(
                format!("{}\\dwm.exe", String::from_utf16_lossy(&directory[..size]))
                    .to_ascii_lowercase(),
            )
        } else {
            None
        };
        loop {
            let process = unsafe { OpenProcess(0x1000, 0, entry.pid) }; // query-limited, never admin
            let mut created = 0;
            if !process.is_null() {
                let process = OwnedHandle(process);
                let mut creation = FileTime::default();
                let mut exit = FileTime::default();
                let mut kernel = FileTime::default();
                let mut user = FileTime::default();
                if unsafe {
                    GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user)
                } != 0
                {
                    created = ((creation.high as u64) << 32) | creation.low as u64;
                }
                let len = entry
                    .exe
                    .iter()
                    .position(|v| *v == 0)
                    .unwrap_or(entry.exe.len());
                if String::from_utf16_lossy(&entry.exe[..len]).eq_ignore_ascii_case("dwm.exe") {
                    let mut path = [0u16; 32768];
                    let mut length = path.len() as u32;
                    if unsafe {
                        QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length)
                    } != 0
                        && (length as usize) < path.len()
                        && dwm_path.as_deref()
                            == Some(
                                String::from_utf16_lossy(&path[..length as usize])
                                    .to_ascii_lowercase()
                                    .as_str(),
                            )
                    {
                        compositor.insert(entry.pid);
                    }
                }
            }
            processes.insert(entry.pid, (entry.parent, created));
            if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                if unsafe { GetLastError() } != 18 {
                    return Err("gpu_process_snapshot_incomplete");
                }
                break;
            }
        }
        let mut excluded = descendant_pids(unsafe { GetCurrentProcessId() }, &processes);
        excluded.extend(compositor);
        Ok(excluded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_ffi_layout_matches_sdk() {
        use std::mem::{align_of, offset_of, size_of};
        assert_eq!(size_of::<WtsInfoLevel1>(), 224);
        assert_eq!(align_of::<WtsInfoLevel1>(), 8);
        assert_eq!(offset_of!(WtsInfoLevel1, station_name), 12);
        assert_eq!(offset_of!(WtsInfoLevel1, user_name), 78);
        assert_eq!(offset_of!(WtsInfoLevel1, domain_name), 120);
        assert_eq!(offset_of!(WtsInfoLevel1, last_input_time), 184);
        assert_eq!(offset_of!(WtsInfoLevel1, current_time), 192);
        assert_eq!(offset_of!(WtsInfoEx, data), 8);
        assert_eq!(size_of::<WtsInfoEx>(), 232);
        assert_eq!(size_of::<Gamepad>(), 12);
        assert_eq!(size_of::<XInputState>(), 16);
        assert_eq!(offset_of!(XInputState, gamepad), 4);
        assert_eq!(offset_of!(PdhValue, value), 8);
        assert_eq!(size_of::<PdhValue>(), 16);
        if size_of::<usize>() == 8 {
            assert_eq!(size_of::<PdhItem>(), 24);
        }
    }
    #[test]
    fn held_controller_controls_are_activity() {
        assert!(!held_controls(&Gamepad::default()));
        assert!(held_controls(&Gamepad {
            buttons: 1,
            ..Default::default()
        }));
        assert!(held_controls(&Gamepad {
            right_trigger: 31,
            ..Default::default()
        }));
        assert!(held_controls(&Gamepad {
            left_x: i16::MIN,
            ..Default::default()
        }));
        assert!(!held_controls(&Gamepad {
            left_x: 200,
            right_trigger: 2,
            ..Default::default()
        }));
    }
    #[test]
    fn session_times_reject_unknown_and_clock_anomalies() {
        assert_eq!(session_idle_seconds(4_000_000_000, 1_000_000_000), Ok(300));
        assert!(session_idle_seconds(10, 0).is_err());
        assert!(session_idle_seconds(10, 11).is_err());
    }
    fn engine(pid: u32, engine: u32, percent: f64) -> (String, u32, f64) {
        (
            format!("pid_{pid}_luid_0x00000000_0x0000ffff_phys_0_eng_{engine}_engtype_3D"),
            0,
            percent,
        )
    }
    #[test]
    fn foreign_gpu_is_summed_self_excluded_and_churn_recovers() {
        let excluded = BTreeSet::from([42]);
        let mut previous = None;
        let values = vec![engine(42, 0, 100.0), engine(77, 0, 3.0), engine(88, 1, 3.0)];
        assert!(foreign_load(&values, &excluded, &mut previous).is_err());
        assert_eq!(foreign_load(&values, &excluded, &mut previous), Ok(6.0));
        let mut values = values;
        values.push(engine(42, 2, 100.0));
        assert_eq!(foreign_load(&values, &excluded, &mut previous), Ok(6.0));
        values.push(engine(99, 0, 2.0));
        assert!(foreign_load(&values, &excluded, &mut previous).is_err());
        assert_eq!(foreign_load(&values, &excluded, &mut previous), Ok(8.0));
        values[1].2 = f64::NAN;
        assert!(foreign_load(&values, &excluded, &mut previous).is_err());
        values[1].2 = 3.0;
        assert_eq!(foreign_load(&values, &excluded, &mut previous), Ok(8.0));
    }
    #[test]
    fn unknown_counter_instances_fail_closed() {
        assert_eq!(
            engine_pid("pid_123_luid_0x1_0x2_phys_0_eng_1_engtype_Compute"),
            Some(123)
        );
        assert_eq!(engine_pid("pid_123"), None);
        assert_eq!(engine_pid("pid_abc_luid_x_eng_1_engtype_3D"), None);
        assert!(foreign_load(
            &[("unexpected".into(), 0, 0.0)],
            &BTreeSet::new(),
            &mut None
        )
        .is_err());
    }
    #[test]
    fn only_verified_descendants_excluded_not_pid_reuse_or_names() {
        let processes = BTreeMap::from([
            (10, (1, 100)),
            (20, (10, 101)),
            (30, (20, 102)),
            (40, (10, 99)),
            (50, (10, 0)),
            (60, (99, 200)),
        ]);
        assert_eq!(
            descendant_pids(10, &processes),
            BTreeSet::from([10, 20, 30])
        );
    }
}
