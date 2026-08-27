//! Windows backend — real Win32 calls, no `tasklist` scraping.
//!
//! | data | mechanism |
//! |---|---|
//! | process list, pid/ppid/name/threads | `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` + `Process32FirstW`/`Process32NextW` |
//! | per-process cpu | `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetProcessTimes` (100 ns FILETIME deltas) |
//! | per-process rss | `K32GetProcessMemoryInfo` → `WorkingSetSize` |
//! | owner | `OpenProcessToken` + `GetTokenInformation(TokenUser)` + `LookupAccountSidW` |
//! | total cpu | `GetSystemTimes` |
//! | per-core cpu | `NtQuerySystemInformation(SystemProcessorPerformanceInformation)` |
//! | memory | `GlobalMemoryStatusEx` + `K32GetPerformanceInfo` (system cache) |
//! | network bytes | `GetIfTable2` → `MIB_IF_TABLE2` |
//! | uptime | `GetTickCount64` |
//! | terminate | `OpenProcess(PROCESS_TERMINATE)` + `TerminateProcess` |
//!
//! **Why hand-written FFI and not the vendored `libs/windows/windows-rs`:**
//! that crate's `src/Windows/mod.rs` is a *pruned* binding dump. It has no
//! `Win32::System::Diagnostics` (so no `ToolHelp`), no
//! `Win32::System::ProcessStatus`, no `Win32::System::SystemInformation` and
//! no `Win32::NetworkManagement::IpHelper` module at all — and the bindings
//! are not `cfg(feature)`-gated, so turning the Cargo features on adds
//! nothing. `Win32::System::Threading` there exposes ten functions and
//! `OpenProcess`/`GetProcessTimes` are not among them. Regenerating that
//! vendored crate is a platform-wide change; this file keeps the work inside
//! `apps/mptask` in the same small-`extern`-block house style as
//! `backend/macos.rs` and `libs/terminal_core/src/pty.rs`.
//!
//! Compile-checked for `x86_64-pc-windows-msvc` from macOS; the pure helpers
//! below are unit-tested on every host. Never run on Windows hardware yet.

// Off Windows only the pure helpers and their tests are reachable.
#![allow(dead_code)]

use super::CPU_STATES;

// ---- pure helpers, compiled and tested on every host ----

/// A `FILETIME` is a split 64-bit count of 100 ns intervals.
pub fn filetime_to_100ns(low: u32, high: u32) -> u64 {
    (high as u64) << 32 | low as u64
}

/// Percent of one core from a 100 ns process-time delta over a wall-clock
/// nanosecond delta. A 4-thread process pinned flat reads 400.
pub fn cpu_pct_from_100ns(process_delta: u64, wall_nanos: u64) -> f64 {
    if wall_nanos == 0 {
        return 0.0;
    }
    (process_delta as f64 * 100.0) / (wall_nanos as f64 / 100.0)
}

/// `GetSystemTimes`/`SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION` report a kernel
/// figure that *includes* idle. Split it into the shared four-bucket shape so
/// [`super::cpu_pct_from_ticks`] does the arithmetic for every OS.
pub fn ticks_from_system_times(idle: u64, kernel: u64, user: u64) -> [u64; CPU_STATES] {
    [user, kernel.saturating_sub(idle), idle, 0]
}

/// A NUL-terminated UTF-16 buffer (`szExeFile`, `LookupAccountSidW` output).
pub fn wide_to_string(units: &[u16]) -> String {
    let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

#[cfg(windows)]
pub use platform::{terminate, WindowsBackend};

#[cfg(not(windows))]
pub fn terminate(_pid: u32) -> Result<(), String> {
    Err("the Windows backend is not compiled for this target".to_string())
}

#[cfg(windows)]
mod platform {
    use super::super::{cpu_pct_from_ticks, MemInfo, NetInfo, ProcInfo, ProcState, Snapshot, SystemBackend};
    use super::*;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::time::Instant;

    type Handle = *mut c_void;
    type Bool = i32;
    type NtStatus = i32;

    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const TOKEN_QUERY: u32 = 0x0008;
    /// `TokenUser` in `TOKEN_INFORMATION_CLASS`.
    const TOKEN_USER: i32 = 1;
    /// `SystemProcessorPerformanceInformation` in `SYSTEM_INFORMATION_CLASS`.
    const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: i32 = 8;
    const MAX_PATH: usize = 260;
    /// `IF_TYPE_SOFTWARE_LOOPBACK`.
    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
    /// `IfOperStatusUp`.
    const IF_OPER_STATUS_UP: i32 = 1;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn value(self) -> u64 {
            filetime_to_100ns(self.low, self.high)
        }
    }

    /// `PROCESSENTRY32W` (tlhelp32.h).
    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; MAX_PATH],
    }

    /// `PROCESS_MEMORY_COUNTERS` (psapi.h).
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    /// `MEMORYSTATUSEX` (sysinfoapi.h).
    #[repr(C)]
    #[derive(Default)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    /// `PERFORMANCE_INFORMATION` (psapi.h). Counts are in pages.
    #[repr(C)]
    #[derive(Default)]
    struct PerformanceInformation {
        cb: u32,
        commit_total: usize,
        commit_limit: usize,
        commit_peak: usize,
        physical_total: usize,
        physical_available: usize,
        system_cache: usize,
        kernel_total: usize,
        kernel_paged: usize,
        kernel_non_paged: usize,
        page_size: usize,
        handle_count: u32,
        process_count: u32,
        thread_count: u32,
    }

    /// `SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION` (ntddk / undocumented but
    /// stable since NT 4; this is where per-core ticks come from — the
    /// documented alternative is PDH, which needs a whole counter engine).
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct ProcessorPerformanceInformation {
        idle_time: i64,
        kernel_time: i64,
        user_time: i64,
        dpc_time: i64,
        interrupt_time: i64,
        interrupt_count: u32,
        _pad: u32,
    }

    /// `SID_AND_ATTRIBUTES` inside `TOKEN_USER`.
    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut c_void,
        attributes: u32,
    }

    /// `MIB_IF_ROW2` (netioapi.h) — 1352 bytes; only the byte counters and the
    /// two discriminators are typed, the rest is sized filler.
    #[repr(C)]
    struct MibIfRow2 {
        interface_luid: u64,
        interface_index: u32,
        interface_guid: [u8; 16],
        alias: [u16; 257],
        description: [u16; 257],
        physical_address_length: u32,
        physical_address: [u8; 32],
        permanent_physical_address: [u8; 32],
        mtu: u32,
        if_type: u32,
        tunnel_type: i32,
        media_type: i32,
        physical_medium_type: i32,
        access_type: i32,
        direction_type: i32,
        interface_and_oper_status_flags: u8,
        oper_status: i32,
        admin_status: i32,
        media_connect_state: i32,
        network_guid: [u8; 16],
        connection_type: i32,
        transmit_link_speed: u64,
        receive_link_speed: u64,
        in_octets: u64,
        in_ucast_pkts: u64,
        in_n_ucast_pkts: u64,
        in_discards: u64,
        in_errors: u64,
        in_unknown_protos: u64,
        in_ucast_octets: u64,
        in_multicast_octets: u64,
        in_broadcast_octets: u64,
        out_octets: u64,
        out_ucast_pkts: u64,
        out_n_ucast_pkts: u64,
        out_discards: u64,
        out_errors: u64,
        out_ucast_octets: u64,
        out_multicast_octets: u64,
        out_broadcast_octets: u64,
        out_qlen: u64,
    }

    /// `MIB_IF_TABLE2` — a count followed by a flexible array of rows.
    #[repr(C)]
    struct MibIfTable2 {
        num_entries: u32,
        _pad: u32,
        table: [MibIfRow2; 1],
    }

    // These are frozen Win32 ABIs. Getting a size wrong would silently read the
    // wrong offsets on a machine we cannot test on, so the build refuses first.
    const _: () = assert!(std::mem::size_of::<ProcessEntry32W>() == 568);
    const _: () = assert!(std::mem::size_of::<ProcessMemoryCounters>() == 72);
    const _: () = assert!(std::mem::size_of::<MemoryStatusEx>() == 64);
    const _: () = assert!(std::mem::size_of::<PerformanceInformation>() == 104);
    const _: () = assert!(std::mem::size_of::<ProcessorPerformanceInformation>() == 48);
    const _: () = assert!(std::mem::size_of::<MibIfRow2>() == 1352);
    const _: () = assert!(std::mem::size_of::<MibIfTable2>() == 1360);

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
        fn CloseHandle(object: Handle) -> Bool;
        fn OpenProcess(desired_access: u32, inherit_handle: Bool, process_id: u32) -> Handle;
        fn TerminateProcess(process: Handle, exit_code: u32) -> Bool;
        fn GetProcessTimes(
            process: Handle,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> Bool;
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> Bool;
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> Bool;
        fn GetTickCount64() -> u64;
        fn GetLastError() -> u32;
        // psapi entry points, re-exported from kernel32 since Windows 7 under
        // their K32 names so no psapi.lib import is needed.
        fn K32GetProcessMemoryInfo(
            process: Handle,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> Bool;
        fn K32GetPerformanceInfo(info: *mut PerformanceInformation, size: u32) -> Bool;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, desired_access: u32, token: *mut Handle) -> Bool;
        fn GetTokenInformation(
            token: Handle,
            information_class: i32,
            information: *mut c_void,
            length: u32,
            return_length: *mut u32,
        ) -> Bool;
        fn LookupAccountSidW(
            system_name: *const u16,
            sid: *mut c_void,
            name: *mut u16,
            name_len: *mut u32,
            domain: *mut u16,
            domain_len: *mut u32,
            sid_use: *mut i32,
        ) -> Bool;
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQuerySystemInformation(
            information_class: i32,
            information: *mut c_void,
            length: u32,
            return_length: *mut u32,
        ) -> NtStatus;
    }

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetIfTable2Ex(level: i32, table: *mut *mut MibIfTable2) -> u32;
        fn FreeMibTable(memory: *mut c_void);
    }

    /// `MibIfTableNormal` — skip the statistics-only rows.
    const MIB_IF_TABLE_NORMAL: i32 = 0;

    /// A process handle that closes itself.
    struct OwnedHandle(Handle);

    impl OwnedHandle {
        fn open(pid: u32, access: u32) -> Option<Self> {
            // SAFETY: OpenProcess validates the pid and returns null on refusal.
            let handle = unsafe { OpenProcess(access, 0, pid) };
            (!handle.is_null()).then_some(Self(handle))
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: self.0 came from OpenProcess/OpenProcessToken and is
            // closed exactly once, here.
            unsafe { CloseHandle(self.0) };
        }
    }

    pub struct WindowsBackend {
        previous_cores: Vec<[u64; CPU_STATES]>,
        previous_total: Option<[u64; CPU_STATES]>,
        /// pid → cumulative kernel+user 100 ns ticks at the previous sample.
        previous_cpu: HashMap<u32, u64>,
        previous_net: Option<(u64, u64)>,
        last_sample: Option<Instant>,
        /// pid → owner. The token never changes over a process's life.
        owners: HashMap<u32, String>,
    }

    impl WindowsBackend {
        pub fn new() -> Self {
            Self {
                previous_cores: Vec::new(),
                previous_total: None,
                previous_cpu: HashMap::new(),
                previous_net: None,
                last_sample: None,
                owners: HashMap::new(),
            }
        }

        fn owner(&mut self, pid: u32, process: &OwnedHandle) -> String {
            if let Some(name) = self.owners.get(&pid) {
                return name.clone();
            }
            let name = process_owner(process).unwrap_or_default();
            self.owners.insert(pid, name.clone());
            name
        }

        fn sample_cores(&mut self) -> Vec<f64> {
            let mut buffer = vec![ProcessorPerformanceInformation::default(); 1024];
            let bytes = std::mem::size_of_val(buffer.as_slice()) as u32;
            let mut returned = 0u32;
            // SAFETY: the buffer owns `bytes` bytes of correctly aligned
            // ProcessorPerformanceInformation; the call writes at most that and
            // reports how much through `returned`.
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
                    buffer.as_mut_ptr().cast(),
                    bytes,
                    &mut returned,
                )
            };
            if status < 0 {
                return vec![0.0; self.previous_cores.len()];
            }
            let count = returned as usize / std::mem::size_of::<ProcessorPerformanceInformation>();
            let current: Vec<[u64; CPU_STATES]> = buffer[..count.min(buffer.len())]
                .iter()
                .map(|core| {
                    ticks_from_system_times(
                        core.idle_time.max(0) as u64,
                        core.kernel_time.max(0) as u64,
                        core.user_time.max(0) as u64,
                    )
                })
                .collect();
            let first = self.previous_cores.is_empty();
            let values = current
                .iter()
                .enumerate()
                .map(|(index, ticks)| {
                    if first {
                        return 0.0;
                    }
                    cpu_pct_from_ticks(
                        *ticks,
                        self.previous_cores.get(index).copied().unwrap_or([0; CPU_STATES]),
                    )
                })
                .collect();
            self.previous_cores = current;
            values
        }

        fn sample_total(&mut self) -> Option<f64> {
            let (mut idle, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default());
            // SAFETY: three FILETIMEs we own.
            if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
                return None;
            }
            let current = ticks_from_system_times(idle.value(), kernel.value(), user.value());
            let percent = self
                .previous_total
                .map(|previous| cpu_pct_from_ticks(current, previous));
            self.previous_total = Some(current);
            percent
        }

        fn sample_processes(&mut self, wall_nanos: u64) -> Vec<ProcInfo> {
            // SAFETY: a process snapshot needs no target pid.
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
            if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
                return Vec::new();
            }
            let snapshot = OwnedHandle(snapshot);
            let mut entry: ProcessEntry32W = unsafe { std::mem::zeroed() };
            entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
            // SAFETY: dw_size is set as the API requires before the first walk.
            let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;

            let mut processes = Vec::new();
            let mut live_cpu = HashMap::new();
            while more {
                let pid = entry.th32_process_id;
                let name = wide_to_string(&entry.sz_exe_file);
                let mut process = ProcInfo {
                    pid,
                    ppid: entry.th32_parent_process_id,
                    name: name.clone(),
                    cmdline: name,
                    threads: entry.cnt_threads,
                    // Windows keeps no process-level scheduler state; a listed
                    // process is a live one. Suspension lives per thread.
                    state: ProcState::Running,
                    ..ProcInfo::default()
                };
                if let Some(handle) = OwnedHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
                    if let Some(ticks) = process_cpu_ticks(&handle) {
                        if let Some(&before) = self.previous_cpu.get(&pid) {
                            process.cpu_pct =
                                cpu_pct_from_100ns(ticks.saturating_sub(before), wall_nanos);
                        }
                        live_cpu.insert(pid, ticks);
                    }
                    process.mem_rss = process_working_set(&handle);
                    process.user = self.owner(pid, &handle);
                }
                processes.push(process);
                // SAFETY: same snapshot and entry, walked until it reports done.
                more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
            }
            self.previous_cpu = live_cpu;
            if self.owners.len() > processes.len() * 2 + 64 {
                let live: std::collections::HashSet<u32> = processes.iter().map(|p| p.pid).collect();
                self.owners.retain(|pid, _| live.contains(pid));
            }
            processes
        }
    }

    impl SystemBackend for WindowsBackend {
        fn name(&self) -> &'static str {
            "windows/win32"
        }

        fn sample(&mut self) -> Snapshot {
            let now = Instant::now();
            let wall_nanos = self
                .last_sample
                .map(|then| now.duration_since(then).as_nanos().min(u64::MAX as u128) as u64)
                .unwrap_or(0);
            self.last_sample = Some(now);

            let cpu_cores = self.sample_cores();
            let cpu_total = self.sample_total().unwrap_or_else(|| {
                if cpu_cores.is_empty() {
                    0.0
                } else {
                    cpu_cores.iter().sum::<f64>() / cpu_cores.len() as f64
                }
            });

            let (rx_total, tx_total) = network_totals();
            let seconds = wall_nanos as f64 / 1e9;
            let net = match self.previous_net {
                Some((rx, tx)) if seconds > 0.0 => NetInfo {
                    rx_total,
                    tx_total,
                    rx_per_second: rx_total.saturating_sub(rx) as f64 / seconds,
                    tx_per_second: tx_total.saturating_sub(tx) as f64 / seconds,
                },
                _ => NetInfo { rx_total, tx_total, ..NetInfo::default() },
            };
            self.previous_net = Some((rx_total, tx_total));

            Snapshot {
                cpu_total,
                cpu_cores,
                mem: memory(),
                net,
                processes: self.sample_processes(wall_nanos),
                // Windows has no getloadavg equivalent.
                load_avg: [0.0; 3],
                // SAFETY: no arguments, no output buffer.
                uptime_seconds: unsafe { GetTickCount64() } / 1000,
                backend: "windows/win32",
            }
        }
    }

    fn process_cpu_ticks(process: &OwnedHandle) -> Option<u64> {
        let (mut creation, mut exit) = (FileTime::default(), FileTime::default());
        let (mut kernel, mut user) = (FileTime::default(), FileTime::default());
        // SAFETY: four FILETIMEs we own, against a handle opened for query.
        let ok = unsafe {
            GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user)
        };
        (ok != 0).then(|| kernel.value().saturating_add(user.value()))
    }

    fn process_working_set(process: &OwnedHandle) -> u64 {
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            ..ProcessMemoryCounters::default()
        };
        let size = counters.cb;
        // SAFETY: counters is exactly `size` bytes and cb is set as required.
        let ok = unsafe { K32GetProcessMemoryInfo(process.0, &mut counters, size) };
        if ok == 0 {
            0
        } else {
            counters.working_set_size as u64
        }
    }

    /// `DOMAIN\user` for the process's token, if we may read it.
    fn process_owner(process: &OwnedHandle) -> Option<String> {
        let mut token: Handle = std::ptr::null_mut();
        // SAFETY: writes one handle we own; guarded by the return value.
        if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) } == 0 || token.is_null() {
            return None;
        }
        let token = OwnedHandle(token);

        let mut needed = 0u32;
        // SAFETY: sizing call — a null buffer asks only for the length.
        unsafe { GetTokenInformation(token.0, TOKEN_USER, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return None;
        }
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: buffer holds `needed` bytes, which is what the sizing call asked for.
        let ok = unsafe {
            GetTokenInformation(token.0, TOKEN_USER, buffer.as_mut_ptr().cast(), needed, &mut needed)
        };
        if ok == 0 || buffer.len() < std::mem::size_of::<SidAndAttributes>() {
            return None;
        }
        // TOKEN_USER is a single SID_AND_ATTRIBUTES whose Sid points into the
        // same buffer, so the pointer stays valid while `buffer` lives.
        // SAFETY: the kernel wrote a TOKEN_USER at the start of the buffer.
        let sid = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<SidAndAttributes>()) }.sid;
        if sid.is_null() {
            return None;
        }

        let mut name = [0u16; 256];
        let mut domain = [0u16; 256];
        let mut name_len = name.len() as u32;
        let mut domain_len = domain.len() as u32;
        let mut sid_use = 0i32;
        // SAFETY: both buffers are as long as the lengths we pass in.
        let ok = unsafe {
            LookupAccountSidW(
                std::ptr::null(),
                sid,
                name.as_mut_ptr(),
                &mut name_len,
                domain.as_mut_ptr(),
                &mut domain_len,
                &mut sid_use,
            )
        };
        if ok == 0 {
            return None;
        }
        let user = wide_to_string(&name);
        let domain = wide_to_string(&domain);
        Some(if domain.is_empty() { user } else { format!("{domain}\\{user}") })
    }

    fn memory() -> MemInfo {
        let mut status = MemoryStatusEx {
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            ..MemoryStatusEx::default()
        };
        // SAFETY: one MEMORYSTATUSEX we own with dwLength set as required.
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return MemInfo::default();
        }
        let mut memory = MemInfo {
            total: status.ull_total_phys,
            available: status.ull_avail_phys,
            used: status.ull_total_phys.saturating_sub(status.ull_avail_phys),
            free: status.ull_avail_phys,
            // Filled from K32GetPerformanceInfo below, if it answers.
            cache: 0,
            // The page file total/available pair is Windows' commit charge.
            swap_total: status.ull_total_page_file.saturating_sub(status.ull_total_phys),
            swap_used: status
                .ull_total_page_file
                .saturating_sub(status.ull_avail_page_file)
                .saturating_sub(status.ull_total_phys.saturating_sub(status.ull_avail_phys)),
        };
        let mut info = PerformanceInformation {
            cb: std::mem::size_of::<PerformanceInformation>() as u32,
            ..PerformanceInformation::default()
        };
        let size = info.cb;
        // SAFETY: info is exactly `size` bytes and cb is set as required.
        if unsafe { K32GetPerformanceInfo(&mut info, size) } != 0 {
            memory.cache = (info.system_cache as u64).saturating_mul(info.page_size as u64);
        }
        memory
    }

    fn network_totals() -> (u64, u64) {
        let mut table: *mut MibIfTable2 = std::ptr::null_mut();
        // SAFETY: the API allocates the table and hands back the pointer; it is
        // released with FreeMibTable below.
        if unsafe { GetIfTable2Ex(MIB_IF_TABLE_NORMAL, &mut table) } != 0 || table.is_null() {
            return (0, 0);
        }
        // SAFETY: a successful call guarantees num_entries rows laid out
        // contiguously from the flexible array member.
        let (count, rows) = unsafe { ((*table).num_entries as usize, (*table).table.as_ptr()) };
        let mut received = 0u64;
        let mut sent = 0u64;
        for index in 0..count {
            // SAFETY: index < num_entries, so this row is inside the table.
            let row = unsafe { &*rows.add(index) };
            if row.if_type == IF_TYPE_SOFTWARE_LOOPBACK || row.oper_status != IF_OPER_STATUS_UP {
                continue;
            }
            received = received.saturating_add(row.in_octets);
            sent = sent.saturating_add(row.out_octets);
        }
        // SAFETY: frees exactly the table GetIfTable2Ex allocated.
        unsafe { FreeMibTable(table.cast()) };
        (received, sent)
    }

    pub fn terminate(pid: u32) -> Result<(), String> {
        if pid == 0 {
            return Err("refusing to terminate the system idle process".to_string());
        }
        let Some(process) = OwnedHandle::open(pid, PROCESS_TERMINATE) else {
            // SAFETY: reads the calling thread's own last-error slot.
            return Err(format!("OpenProcess failed: error {}", unsafe { GetLastError() }));
        };
        // SAFETY: a handle opened for PROCESS_TERMINATE; 1 is the exit code a
        // killed process reports.
        if unsafe { TerminateProcess(process.0, 1) } == 0 {
            // SAFETY: as above.
            return Err(format!("TerminateProcess failed: error {}", unsafe { GetLastError() }));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_halves_join_into_one_counter() {
        assert_eq!(filetime_to_100ns(0, 1), 1u64 << 32);
        assert_eq!(filetime_to_100ns(u32::MAX, 0), u32::MAX as u64);
        assert_eq!(filetime_to_100ns(0, 0), 0);
    }

    #[test]
    fn process_cpu_percent_is_per_core() {
        // One second of wall clock, one second of process time = 100%.
        let one_second_100ns = 10_000_000u64;
        let percent = cpu_pct_from_100ns(one_second_100ns, 1_000_000_000);
        assert!((percent - 100.0).abs() < 1e-9, "{percent}");
        // Four threads pinned for that second read 400%.
        let percent = cpu_pct_from_100ns(4 * one_second_100ns, 1_000_000_000);
        assert!((percent - 400.0).abs() < 1e-9, "{percent}");
        assert_eq!(cpu_pct_from_100ns(1234, 0), 0.0);
    }

    #[test]
    fn system_times_split_idle_out_of_the_kernel_figure() {
        // GetSystemTimes' kernel figure includes idle: 80 kernel, 60 idle.
        let ticks = ticks_from_system_times(60, 80, 40);
        assert_eq!(ticks, [40, 20, 60, 0]);
        // 60 busy of 120 total = 50%.
        let percent = super::super::cpu_pct_from_ticks(ticks, [0, 0, 0, 0]);
        assert!((percent - 50.0).abs() < 1e-9, "{percent}");
    }

    #[test]
    fn wide_strings_stop_at_the_nul() {
        let mut buffer = [0u16; 8];
        for (slot, unit) in buffer.iter_mut().zip("mptask".encode_utf16()) {
            *slot = unit;
        }
        assert_eq!(wide_to_string(&buffer), "mptask");
        // No NUL at all: take the whole buffer.
        assert_eq!(wide_to_string(&[0x41, 0x42]), "AB");
        assert_eq!(wide_to_string(&[]), "");
    }

    #[cfg(not(windows))]
    #[test]
    fn terminate_says_so_when_the_backend_is_not_compiled_in() {
        assert!(terminate(1).is_err());
    }
}
