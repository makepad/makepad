//! Windows activity sampling. No hooks, input contents, titles, or GPU contexts.
//! All handles belong to the single monitor thread. FFI layouts are tested below.
#![allow(dead_code)]
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Gamepad {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    left_x: i16,
    left_y: i16,
    right_x: i16,
    right_y: i16,
}

#[cfg(windows)]
use makepad_windows_sys::Win32::{
    System::{
        Performance::PDH_FMT_COUNTERVALUE_ITEM_W as PdhItem, RemoteDesktop::WTSINFOEXW as WtsInfoEx,
    },
    UI::Input::XboxController::XINPUT_STATE as XInputState,
};

#[cfg(all(windows, test))]
use makepad_windows_sys::Win32::{
    System::{
        Performance::PDH_FMT_COUNTERVALUE as PdhValue,
        RemoteDesktop::WTSINFOEX_LEVEL1_W as WtsInfoLevel1,
    },
    UI::Input::XboxController::XINPUT_GAMEPAD,
};

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
) -> Result<f64, &'static str> {
    if values.is_empty() {
        return Err("gpu_counter_instances_missing");
    }
    let mut load = 0.0;
    let mut invalid = false;
    for (name, status, value) in values {
        let pid = engine_pid(name).ok_or("gpu_counter_instance_unknown")?;
        if excluded.contains(&pid) {
            continue;
        }
        // PDH has already calculated the rate from its samples. VALID_DATA
        // and NEW_DATA both mean a usable value, including an engine that
        // appeared since our last read. An instance-set comparison falsely
        // cancelled every queued generation when an idle GPU context changed.
        // Missing/invalid rate samples still fail closed below; the native
        // query retains its initial two-collection warmup and array checks.
        if (*status != 0 && *status != 1) || !value.is_finite() || *value < 0.0 {
            invalid = true;
        } else {
            load += value.min(100.0);
        }
    }
    if invalid {
        Err("gpu_counter_sample_unknown")
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
    use makepad_windows_sys::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W as ProcessEntry;
    use makepad_windows_sys::Win32::UI::Input::KeyboardAndMouse::LASTINPUTINFO as LastInput;
    use makepad_windows_sys::{
        core::PWSTR,
        Win32::{
            Foundation::{
                CloseHandle_raw, FreeLibrary_raw, GetLastError_raw, FILETIME as FileTime,
                RECT as Rect,
            },
            Graphics::Gdi::{
                GetMonitorInfoW_raw, MonitorFromWindow_raw, MONITORINFO as MonitorInfo,
            },
            System::{
                Diagnostics::ToolHelp::*, LibraryLoader::*, Performance::*, RemoteDesktop::*,
                StationsAndDesktops::*, SystemInformation::*, Threading::*,
            },
            UI::{
                HiDpi::*, Input::KeyboardAndMouse::*, Shell::SHQueryUserNotificationState_raw,
                WindowsAndMessaging::*,
            },
        },
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle_raw(makepad_windows_sys::Win32::Foundation::HANDLE(
                    (self.0) as *mut ::core::ffi::c_void,
                ))
                .0;
            }
        }
    }
    struct WtsMemory(*mut c_void);
    impl Drop for WtsMemory {
        fn drop(&mut self) {
            unsafe {
                WTSFreeMemory_raw(self.0);
            }
        }
    }

    pub(in super::super) struct Probe {
        query: Handle,
        counter: Handle,
        collected: bool,
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
                let module = unsafe {
                    LoadLibraryExW_raw(
                        makepad_windows_sys::core::PCWSTR((wide(dll).as_ptr()) as *const u16),
                        makepad_windows_sys::Win32::Foundation::HANDLE(ptr::null_mut()),
                        makepad_windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_FLAGS(
                            0x800,
                        ),
                    )
                    .0
                };
                if module.is_null() {
                    continue;
                }
                let function = unsafe {
                    GetProcAddress_raw(
                        makepad_windows_sys::Win32::Foundation::HMODULE(module),
                        makepad_windows_sys::core::PCSTR(b"XInputGetState\0".as_ptr()),
                    )
                    .map_or(std::ptr::null_mut(), |function| {
                        function as *mut std::ffi::c_void
                    })
                };
                if function.is_null() {
                    unsafe {
                        FreeLibrary_raw(makepad_windows_sys::Win32::Foundation::HMODULE(module)).0;
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
                        active |= held_controls(&Gamepad {
                            buttons: state.Gamepad.wButtons.0,
                            left_trigger: state.Gamepad.bLeftTrigger,
                            right_trigger: state.Gamepad.bRightTrigger,
                            left_x: state.Gamepad.sThumbLX,
                            left_y: state.Gamepad.sThumbLY,
                            right_x: state.Gamepad.sThumbRX,
                            right_y: state.Gamepad.sThumbRY,
                        }) || self.controller_packets[slot]
                            .map_or(true, |old| old != state.dwPacketNumber);
                        self.controller_packets[slot] = Some(state.dwPacketNumber);
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
                    PdhCloseQuery_raw(makepad_windows_sys::Win32::System::Performance::PDH_HQUERY(
                        self.query,
                    ));
                }
            }
            self.query = ptr::null_mut();
            self.counter = ptr::null_mut();
            self.collected = false;
        }
        fn gpu_load(&mut self) -> Result<f64, &'static str> {
            if self.query.is_null() {
                if unsafe {
                    PdhOpenQueryW_raw(
                        makepad_windows_sys::core::PCWSTR(ptr::null()),
                        0,
                        (&mut self.query) as *mut Handle
                            as *mut makepad_windows_sys::Win32::System::Performance::PDH_HQUERY,
                    )
                } != 0
                {
                    self.reset_pdh();
                    return Err("gpu_counter_unavailable");
                }
                let path = wide("\\GPU Engine(*)\\Utilization Percentage");
                if unsafe {
                    PdhAddEnglishCounterW_raw(
                        makepad_windows_sys::Win32::System::Performance::PDH_HQUERY(self.query),
                        makepad_windows_sys::core::PCWSTR(path.as_ptr()),
                        0,
                        (&mut self.counter) as *mut Handle
                            as *mut makepad_windows_sys::Win32::System::Performance::PDH_HCOUNTER,
                    )
                } != 0
                {
                    self.reset_pdh();
                    return Err("gpu_counter_unavailable");
                }
            }
            if unsafe {
                PdhCollectQueryData_raw(
                    makepad_windows_sys::Win32::System::Performance::PDH_HQUERY(self.query),
                )
            } != 0
            {
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
            foreign_load(&values, &excluded)
        }
        fn counter_values(&self) -> Result<Vec<(String, u32, f64)>, &'static str> {
            let mut bytes = 0u32;
            let mut count = 0u32;
            let format = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
            let status = unsafe {
                PdhGetFormattedCounterArrayW_raw(
                    makepad_windows_sys::Win32::System::Performance::PDH_HCOUNTER(self.counter),
                    makepad_windows_sys::Win32::System::Performance::PDH_FMT(format),
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
                    PdhGetFormattedCounterArrayW_raw(makepad_windows_sys::Win32::System::Performance::PDH_HCOUNTER(self.counter), makepad_windows_sys::Win32::System::Performance::PDH_FMT(format), &mut bytes, &mut count, (storage.as_mut_ptr().cast()) as *mut makepad_windows_sys::Win32::System::Performance::PDH_FMT_COUNTERVALUE_ITEM_W)
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
                    let address = item.szName.0 as usize;
                    if address < start || address >= end || address % 2 != 0 {
                        return Err("gpu_counter_name_unknown");
                    }
                    let name =
                        unsafe { std::slice::from_raw_parts(item.szName.0, (end - address) / 2) };
                    let length = name
                        .iter()
                        .position(|v| *v == 0)
                        .ok_or("gpu_counter_name_unknown")?;
                    let name = String::from_utf16(&name[..length])
                        .map_err(|_| "gpu_counter_name_unknown")?;
                    result.push((name, item.FmtValue.CStatus, unsafe {
                        item.FmtValue.Anonymous.doubleValue
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
                    FreeLibrary_raw(makepad_windows_sys::Win32::Foundation::HMODULE(
                        self.xinput_module,
                    ))
                    .0;
                }
            }
        }
    }

    fn sample_sessions() -> Result<(u64, bool), &'static str> {
        let mut own_session = 0u32;
        if unsafe { ProcessIdToSessionId_raw(GetCurrentProcessId_raw(), &mut own_session).0 } == 0 {
            return Err("process_session_unknown");
        }
        let mut sessions: *mut WTS_SESSION_INFOW = ptr::null_mut();
        let mut count = 0u32;
        if unsafe {
            WTSEnumerateSessionsW_raw(makepad_windows_sys::Win32::Foundation::HANDLE(ptr::null_mut()), 0, 1, (&mut sessions) as *mut *mut makepad_windows_sys::Win32::System::RemoteDesktop::WTS_SESSION_INFOW, &mut count).0
        } == 0
        {
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
            if matches!(session.State.0, 6..=9) {
                continue;
            }
            if !matches!(session.State.0, 0..=5) {
                return Err("session_state_unknown");
            }
            let mut buffer = PWSTR::null();
            let mut bytes = 0u32;
            if unsafe {
                WTSQuerySessionInformationW_raw(
                    makepad_windows_sys::Win32::Foundation::HANDLE(ptr::null_mut()),
                    session.SessionId,
                    makepad_windows_sys::Win32::System::RemoteDesktop::WTS_INFO_CLASS(25),
                    (&mut buffer) as *mut makepad_windows_sys::core::PWSTR,
                    &mut bytes,
                )
                .0
            } == 0
            {
                return Err("session_information_unavailable");
            }
            let _memory = WtsMemory(buffer.0.cast());
            if buffer.0.is_null() || (bytes as usize) < mem::size_of::<WtsInfoEx>() {
                return Err("session_information_invalid");
            }
            let info = unsafe { ptr::read_unaligned(buffer.0.cast::<WtsInfoEx>()) };
            if info.Level != 1 {
                return Err("session_information_level_unknown");
            }
            let info = unsafe { info.Data.WTSInfoExLevel1 };
            if info.SessionId != session.SessionId {
                return Err("session_information_mismatch");
            }
            if info.SessionState.0 != session.State.0 {
                return Err("session_changed_during_sample");
            }
            if info.UserName[0] == 0 {
                continue;
            }
            idle = idle.min(session_idle_seconds(info.CurrentTime, info.LastInputTime)?);
            // WTS can inspect cross-session timestamps but foreground windows and
            // XInput cannot. Never assert that an invisible active user is idle.
            if matches!(session.State.0, 0..=3) {
                if session.SessionId != own_session || own_session == 0 {
                    return Err("other_interactive_session_not_visible");
                }
                same_active = true;
            }
        }
        if same_active {
            let mut last = LastInput {
                cbSize: mem::size_of::<LastInput>() as u32,
                dwTime: 0,
            };
            if unsafe {
                GetLastInputInfo_raw((&mut last) as *mut makepad_windows_sys::Win32::UI::Input::KeyboardAndMouse::LASTINPUTINFO).0
            } == 0
            {
                return Err("last_input_unavailable");
            }
            let now = unsafe { GetTickCount_raw() };
            // Modulo subtraction handles the 49-day tick wrap. A future input
            // tick or age beyond half the cycle is ambiguous, so fail closed
            // rather than converting it into an apparently very idle session.
            if now.wrapping_sub(last.dwTime) > i32::MAX as u32 {
                return Err("last_input_tick_ambiguous");
            }
            idle = idle.min(super::super::tick_idle_seconds(now, last.dwTime));
        }
        // No active user requires no session-specific foreground/controller probe.
        // A disconnected logged-in user is still included in the WTS idle minimum.
        Ok((idle, same_active))
    }

    fn fullscreen() -> Result<bool, &'static str> {
        // Secure/other input desktops are not visible from this process.
        let desktop = unsafe {
            OpenInputDesktop_raw(
                makepad_windows_sys::Win32::System::StationsAndDesktops::DESKTOP_CONTROL_FLAGS(0),
                makepad_windows_sys::core::BOOL(0),
                makepad_windows_sys::Win32::System::StationsAndDesktops::DESKTOP_ACCESS_FLAGS(
                    0x0001,
                ),
            )
            .0
        };
        if desktop.is_null() {
            return Err("input_desktop_unavailable");
        }
        unsafe {
            CloseDesktop_raw(
                makepad_windows_sys::Win32::System::StationsAndDesktops::HDESK(desktop),
            )
            .0;
        }
        let mut state = 0;
        if unsafe {
            SHQueryUserNotificationState_raw(
                (&mut state) as *mut i32
                    as *mut makepad_windows_sys::Win32::UI::Shell::QUERY_USER_NOTIFICATION_STATE,
            )
            .0
        } < 0
        {
            return Err("notification_state_unknown");
        }
        match state {
            2 | 3 | 4 | 6 | 7 => return Ok(true),
            1 | 5 => {}
            _ => return Err("notification_state_unknown"),
        }
        let foreground = unsafe { GetForegroundWindow_raw().0 };
        if foreground.is_null() {
            return Err("foreground_window_unavailable");
        }
        let shell = unsafe { GetShellWindow_raw().0 };
        if foreground == unsafe { GetDesktopWindow_raw().0 }
            || (!shell.is_null() && foreground == shell)
        {
            return Ok(false);
        }
        let mut class = [0u16; 128];
        let length = unsafe {
            GetClassNameW_raw(
                makepad_windows_sys::Win32::Foundation::HWND(foreground),
                makepad_windows_sys::core::PWSTR(class.as_mut_ptr()),
                class.len() as i32,
            )
        };
        if length <= 0 {
            return Err("foreground_class_unknown");
        }
        let class = String::from_utf16_lossy(&class[..length as usize]);
        if matches!(class.as_str(), "WorkerW" | "Progman") && !shell.is_null() {
            let mut foreground_pid = 0u32;
            let mut shell_pid = 0u32;
            unsafe {
                GetWindowThreadProcessId_raw(
                    makepad_windows_sys::Win32::Foundation::HWND(foreground),
                    &mut foreground_pid,
                );
                GetWindowThreadProcessId_raw(
                    makepad_windows_sys::Win32::Foundation::HWND(shell),
                    &mut shell_pid,
                );
            }
            if foreground_pid != 0 && foreground_pid == shell_pid {
                return Ok(false);
            }
        }
        // Compare monitor and window bounds in the same physical coordinate
        // space, including headless services on monitors above 100% scaling.
        let old_dpi = unsafe {
            SetThreadDpiAwarenessContext_raw(
                makepad_windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT(
                    ((-4isize) as Handle) as *mut ::core::ffi::c_void,
                ),
            )
            .0
        };
        if old_dpi.is_null() {
            return Err("foreground_dpi_context_unknown");
        }
        struct RestoreDpi(Handle);
        impl Drop for RestoreDpi {
            fn drop(&mut self) {
                unsafe {
                    SetThreadDpiAwarenessContext_raw(
                        makepad_windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT(
                            (self.0) as *mut ::core::ffi::c_void,
                        ),
                    )
                    .0;
                }
            }
        }
        let _dpi = RestoreDpi(old_dpi);
        let mut rect = Rect::default();
        if unsafe {
            GetWindowRect_raw(
                makepad_windows_sys::Win32::Foundation::HWND(foreground),
                (&mut rect) as *mut makepad_windows_sys::Win32::Foundation::RECT,
            )
            .0
        } == 0
        {
            return Err("foreground_rect_unknown");
        }
        let monitor = unsafe {
            MonitorFromWindow_raw(
                makepad_windows_sys::Win32::Foundation::HWND(foreground),
                makepad_windows_sys::Win32::Graphics::Gdi::MONITOR_FROM_FLAGS(2),
            )
            .0
        };
        let mut info = MonitorInfo {
            cbSize: mem::size_of::<MonitorInfo>() as u32,
            rcMonitor: Rect::default(),
            rcWork: Rect::default(),
            dwFlags: 0,
        };
        if monitor.is_null()
            || unsafe {
                GetMonitorInfoW_raw(
                    makepad_windows_sys::Win32::Graphics::Gdi::HMONITOR(monitor),
                    (&mut info) as *mut makepad_windows_sys::Win32::Graphics::Gdi::MONITORINFO,
                )
                .0
            } == 0
        {
            return Err("foreground_monitor_unknown");
        }
        // Covers borderless and oversized fullscreen windows. No window title,
        // process allowlist, or exclusive-mode requirement can hide a real game.
        Ok(rect.left <= info.rcMonitor.left
            && rect.top <= info.rcMonitor.top
            && rect.right >= info.rcMonitor.right
            && rect.bottom >= info.rcMonitor.bottom)
    }

    fn process_exclusions() -> Result<BTreeSet<u32>, &'static str> {
        let snapshot = unsafe {
            CreateToolhelp32Snapshot_raw(makepad_windows_sys::Win32::System::Diagnostics::ToolHelp::CREATE_TOOLHELP_SNAPSHOT_FLAGS(2), 0).0
        };
        if snapshot.is_null() || snapshot as isize == -1 {
            return Err("gpu_process_snapshot_unavailable");
        }
        let snapshot = OwnedHandle(snapshot);
        let mut entry: ProcessEntry = unsafe { mem::zeroed() };
        entry.dwSize = mem::size_of::<ProcessEntry>() as u32;
        if unsafe {
            Process32FirstW_raw(makepad_windows_sys::Win32::Foundation::HANDLE((snapshot.0) as *mut ::core::ffi::c_void), (&mut entry) as *mut makepad_windows_sys::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W).0
        } == 0
        {
            return Err("gpu_process_snapshot_empty");
        }
        let mut processes = BTreeMap::new();
        let mut compositor = BTreeSet::new();
        let mut directory = [0u16; 32768];
        let size = unsafe {
            GetSystemDirectoryW_raw(
                makepad_windows_sys::core::PWSTR(directory.as_mut_ptr()),
                directory.len() as u32,
            )
        } as usize;
        let dwm_path = if size > 0 && size < directory.len() {
            Some(
                format!("{}\\dwm.exe", String::from_utf16_lossy(&directory[..size]))
                    .to_ascii_lowercase(),
            )
        } else {
            None
        };
        loop {
            let process = unsafe {
                OpenProcess_raw(
                    makepad_windows_sys::Win32::System::Threading::PROCESS_ACCESS_RIGHTS(0x1000),
                    makepad_windows_sys::core::BOOL(0),
                    entry.th32ProcessID,
                )
                .0
            }; // query-limited, never admin
            let mut created = 0;
            if !process.is_null() {
                let process = OwnedHandle(process);
                let mut creation = FileTime::default();
                let mut exit = FileTime::default();
                let mut kernel = FileTime::default();
                let mut user = FileTime::default();
                if unsafe {
                    GetProcessTimes_raw(
                        makepad_windows_sys::Win32::Foundation::HANDLE(
                            (process.0) as *mut ::core::ffi::c_void,
                        ),
                        (&mut creation) as *mut makepad_windows_sys::Win32::Foundation::FILETIME,
                        (&mut exit) as *mut makepad_windows_sys::Win32::Foundation::FILETIME,
                        (&mut kernel) as *mut makepad_windows_sys::Win32::Foundation::FILETIME,
                        (&mut user) as *mut makepad_windows_sys::Win32::Foundation::FILETIME,
                    )
                    .0
                } != 0
                {
                    created =
                        ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
                }
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|v| *v == 0)
                    .unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case("dwm.exe")
                {
                    let mut path = [0u16; 32768];
                    let mut length = path.len() as u32;
                    if unsafe {
                        QueryFullProcessImageNameW_raw(
                            makepad_windows_sys::Win32::Foundation::HANDLE(
                                (process.0) as *mut ::core::ffi::c_void,
                            ),
                            makepad_windows_sys::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
                            makepad_windows_sys::core::PWSTR(path.as_mut_ptr()),
                            &mut length,
                        )
                        .0
                    } != 0
                        && (length as usize) < path.len()
                        && dwm_path.as_deref()
                            == Some(
                                String::from_utf16_lossy(&path[..length as usize])
                                    .to_ascii_lowercase()
                                    .as_str(),
                            )
                    {
                        compositor.insert(entry.th32ProcessID);
                    }
                }
            }
            processes.insert(entry.th32ProcessID, (entry.th32ParentProcessID, created));
            if unsafe {
                Process32NextW_raw(makepad_windows_sys::Win32::Foundation::HANDLE((snapshot.0) as *mut ::core::ffi::c_void), (&mut entry) as *mut makepad_windows_sys::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W).0
            } == 0
            {
                if unsafe { GetLastError_raw().0 } != 18 {
                    return Err("gpu_process_snapshot_incomplete");
                }
                break;
            }
        }
        let mut excluded = descendant_pids(unsafe { GetCurrentProcessId_raw() }, &processes);
        excluded.extend(compositor);
        Ok(excluded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn windows_ffi_layout_matches_sdk() {
        use std::mem::{align_of, offset_of, size_of};
        assert_eq!(size_of::<WtsInfoLevel1>(), 224);
        assert_eq!(align_of::<WtsInfoLevel1>(), 8);
        assert_eq!(offset_of!(WtsInfoLevel1, WinStationName), 12);
        assert_eq!(offset_of!(WtsInfoLevel1, UserName), 78);
        assert_eq!(offset_of!(WtsInfoLevel1, DomainName), 120);
        assert_eq!(offset_of!(WtsInfoLevel1, LastInputTime), 184);
        assert_eq!(offset_of!(WtsInfoLevel1, CurrentTime), 192);
        assert_eq!(offset_of!(WtsInfoEx, Data), 8);
        assert_eq!(size_of::<WtsInfoEx>(), 232);
        assert_eq!(size_of::<XINPUT_GAMEPAD>(), 12);
        assert_eq!(size_of::<XInputState>(), 16);
        assert_eq!(offset_of!(XInputState, Gamepad), 4);
        assert_eq!(offset_of!(PdhValue, Anonymous), 8);
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
    fn valid_foreign_gpu_churn_is_counted_without_false_probe_failure() {
        let excluded = BTreeSet::from([42]);
        let values = vec![engine(42, 0, 100.0), engine(77, 0, 3.0), engine(88, 1, 3.0)];
        assert_eq!(foreign_load(&values, &excluded), Ok(6.0));
        let mut values = values;
        values.push(engine(42, 2, 100.0));
        assert_eq!(foreign_load(&values, &excluded), Ok(6.0));
        values.push(engine(99, 0, 2.0));
        values.last_mut().unwrap().1 = 1; // PDH_CSTATUS_NEW_DATA is valid too.
        assert_eq!(foreign_load(&values, &excluded), Ok(8.0));
        values.remove(1);
        assert_eq!(foreign_load(&values, &excluded), Ok(5.0));
        values.push(engine(101, 0, 90.0));
        assert_eq!(foreign_load(&values, &excluded), Ok(95.0));
    }
    #[test]
    fn missing_or_invalid_foreign_gpu_samples_still_fail_closed() {
        let excluded = BTreeSet::from([42]);
        let mut values = vec![engine(42, 0, 100.0), engine(77, 0, 3.0)];
        values[1].2 = f64::NAN;
        assert!(foreign_load(&values, &excluded).is_err());
        values[1].2 = -1.0;
        assert!(foreign_load(&values, &excluded).is_err());
        values[1].2 = 3.0;
        values[1].1 = 0xc0000bba; // PDH_CSTATUS_INVALID_DATA.
        assert!(foreign_load(&values, &excluded).is_err());
        assert!(foreign_load(&[], &excluded).is_err());
        assert_eq!(foreign_load(&values[..1], &excluded), Ok(0.0));
    }
    #[test]
    fn valid_engine_churn_keeps_jobs_admissible_but_real_load_and_unknown_data_interrupt() {
        use super::super::{Config, Observation, Policy, GPU, GPU_ERROR, IDLE};
        let mut policy = Policy::new(Config {enabled:true,supported:true,..Config::default()});
        let mut observation = Observation {idle_seconds:Some(600),fullscreen:Ok(false),controller_active:Ok(false),foreign_gpu_percent:Ok(0.0),session_error:None};
        for second in 0..=20 { policy.observe(second * 1000, &observation); }
        // A succession of new, valid idle contexts must not reset quiet time
        // or interrupt a sibling generation that has just left the queue.
        for second in 21..25 {
            observation.foreign_gpu_percent = foreign_load(&[engine(100 + second, 0, 1.0)], &BTreeSet::new());
            assert_eq!(policy.observe(u64::from(second) * 1000, &observation).0, IDLE);
        }
        for second in 25..29 {
            observation.foreign_gpu_percent = foreign_load(&[engine(100 + second, 0, 90.0)], &BTreeSet::new());
            assert_eq!(policy.observe(u64::from(second) * 1000, &observation).0, if second == 28 {GPU} else {IDLE});
        }
        // An unreadable sample keeps the last reading for the grace period,
        // then fails closed.
        observation.foreign_gpu_percent = foreign_load(&[], &BTreeSet::new());
        assert_eq!(policy.observe(29000, &observation).0, GPU);
        assert_eq!(policy.observe(32000, &observation).0, GPU_ERROR);
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
