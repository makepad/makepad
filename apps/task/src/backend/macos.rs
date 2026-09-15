//! macOS backend — real system calls, no `ps`/`top` scraping.
//!
//! | data | mechanism |
//! |---|---|
//! | process list, pid/ppid/uid/state/comm | `sysctl(CTL_KERN, KERN_PROC, KERN_PROC_ALL)` → `struct kinfo_proc[]` |
//! | per-process cpu time, rss, threads | libproc `proc_pidinfo(pid, PROC_PIDTASKINFO)` |
//! | executable path / argv | `proc_pidpath`, `sysctl(KERN_PROCARGS2)` (cached per pid) |
//! | per-core cpu ticks | mach `host_processor_info(PROCESSOR_CPU_LOAD_INFO)` |
//! | memory | mach `host_statistics64(HOST_VM_INFO64)` + `sysctl hw.memsize` |
//! | swap | `sysctl vm.swapusage` → `struct xsw_usage` |
//! | network bytes | `sysctl(CTL_NET, AF_ROUTE, 0, 0, NET_RT_IFLIST2)` → `struct if_msghdr2[]` |
//! | load / uptime | `getloadavg`, `sysctl kern.boottime` |
//!
//! FFI is hand-written in the house style of `libs/terminal_core/src/pty.rs`:
//! small `extern "C"` blocks, no libc crate. The one delicate part is the
//! `kinfo_proc` layout; it is a frozen public ABI and a `const` assert on
//! `size_of` (648 bytes) guards every field offset below.

use super::{cpu_pct_from_ticks, MemInfo, NetInfo, ProcInfo, ProcState, Snapshot, SystemBackend, CPU_STATES};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ---- FFI ----

type MachPort = u32;
type KernReturn = c_int;

#[link(name = "System", kind = "dylib")]
extern "C" {
    fn sysctl(
        name: *const c_int,
        namelen: c_uint,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *const c_void,
        newlen: usize,
    ) -> c_int;
    fn mach_host_self() -> MachPort;
    fn host_processor_info(
        host: MachPort,
        flavor: c_int,
        out_processor_count: *mut u32,
        out_processor_info: *mut *mut c_int,
        out_processor_info_count: *mut u32,
    ) -> KernReturn;
    fn host_statistics64(
        host: MachPort,
        flavor: c_int,
        host_info_out: *mut c_void,
        host_info_out_count: *mut u32,
    ) -> KernReturn;
    fn vm_deallocate(target_task: MachPort, address: usize, size: usize) -> KernReturn;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> KernReturn;
    fn proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    fn getpwuid(uid: u32) -> *const Passwd;
    fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
    static mach_task_self_: MachPort;
}

const CTL_KERN: c_int = 1;
const CTL_VM: c_int = 2;
const CTL_HW: c_int = 6;
const CTL_NET: c_int = 4;
const KERN_PROC: c_int = 14;
const KERN_PROC_ALL: c_int = 0;
const KERN_PROCARGS2: c_int = 49;
const KERN_BOOTTIME: c_int = 21;
const VM_SWAPUSAGE: c_int = 5;
const HW_PAGESIZE: c_int = 7;
const HW_MEMSIZE: c_int = 24;
const AF_ROUTE: c_int = 17;
const NET_RT_IFLIST2: c_int = 6;
const RTM_IFINFO2: u8 = 0x12;
const IFT_LOOP: u8 = 0x18;

const PROCESSOR_CPU_LOAD_INFO: c_int = 2;
const HOST_VM_INFO64: c_int = 4;
const HOST_VM_INFO64_COUNT: u32 = (std::mem::size_of::<VmStatistics64>() / 4) as u32;
const PROC_PIDTASKINFO: c_int = 4;
const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

#[repr(C)]
struct Passwd {
    pw_name: *const c_char,
    // The rest of struct passwd is not read here.
}

/// `struct vm_statistics64` (mach/vm_statistics.h). 152 bytes.
#[repr(C)]
#[derive(Default)]
struct VmStatistics64 {
    free_count: u32,
    active_count: u32,
    inactive_count: u32,
    wire_count: u32,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: u32,
    speculative_count: u32,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: u32,
    throttled_count: u32,
    external_page_count: u32,
    internal_page_count: u32,
    total_uncompressed_pages_in_compressor: u64,
}

/// `struct proc_taskinfo` (libproc.h). 96 bytes.
#[repr(C)]
#[derive(Default)]
struct ProcTaskInfo {
    pti_virtual_size: u64,
    pti_resident_size: u64,
    /// Mach absolute-time units, not nanoseconds — scaled by the timebase.
    pti_total_user: u64,
    pti_total_system: u64,
    pti_threads_user: u64,
    pti_threads_system: u64,
    pti_policy: i32,
    pti_faults: i32,
    pti_pageins: i32,
    pti_cow_faults: i32,
    pti_messages_sent: i32,
    pti_messages_received: i32,
    pti_syscalls_mach: i32,
    pti_syscalls_unix: i32,
    pti_csw: i32,
    pti_threadnum: i32,
    pti_numrunning: i32,
    pti_priority: i32,
}

/// `struct xsw_usage` (sys/sysctl.h).
#[repr(C)]
#[derive(Default)]
struct XswUsage {
    xsu_total: u64,
    xsu_avail: u64,
    xsu_used: u64,
    xsu_pagesize: u32,
    xsu_encrypted: u32,
}

/// `struct timeval` on 64-bit darwin.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Timeval {
    tv_sec: i64,
    tv_usec: i32,
    _pad: i32,
}

/// `struct _ucred` inside `struct eproc`. 76 bytes.
#[repr(C)]
struct Ucred {
    cr_ref: i32,
    cr_uid: u32,
    cr_ngroups: i16,
    _pad: [u8; 2],
    cr_groups: [u32; 16],
}

/// `struct extern_proc` (sys/proc.h). 296 bytes on 64-bit darwin.
///
/// Opaque members (pointers, timers, credentials) are kept as sized blanks —
/// only the fields a task manager reads are typed.
#[repr(C)]
struct ExternProc {
    p_un: [u8; 16],
    p_vmspace: *mut c_void,
    p_sigacts: *mut c_void,
    p_flag: i32,
    p_stat: i8,
    _pad0: [u8; 3],
    p_pid: i32,
    p_oppid: i32,
    p_dupfd: i32,
    _pad1: [u8; 4],
    user_stack: *mut c_void,
    exit_thread: *mut c_void,
    p_debugger: i32,
    p_sigwait: i32,
    p_estcpu: u32,
    p_cpticks: i32,
    /// `fixpt_t`, scaled by FSCALE (2048): the kernel's own %cpu estimate.
    p_pctcpu: u32,
    _pad2: [u8; 4],
    p_wchan: *mut c_void,
    p_wmesg: *mut c_void,
    p_swtime: u32,
    p_slptime: u32,
    p_realtimer: [u8; 32],
    p_rtime: [u8; 16],
    p_uticks: u64,
    p_sticks: u64,
    p_iticks: u64,
    p_traceflag: i32,
    _pad3: [u8; 4],
    p_tracep: *mut c_void,
    p_siglist: i32,
    _pad4: [u8; 4],
    p_textvp: *mut c_void,
    p_holdcnt: i32,
    p_sigmask: u32,
    p_sigignore: u32,
    p_sigcatch: u32,
    p_priority: u8,
    p_usrpri: u8,
    p_nice: i8,
    /// MAXCOMLEN + 1 — the kernel truncates to 16 characters.
    p_comm: [c_char; 17],
    _pad5: [u8; 4],
    p_pgrp: *mut c_void,
    p_addr: *mut c_void,
    p_xstat: u16,
    p_acflag: u16,
    _pad6: [u8; 4],
    p_ru: *mut c_void,
}

/// `struct eproc` (sys/sysctl.h). 352 bytes on 64-bit darwin.
#[repr(C)]
struct Eproc {
    e_paddr: *mut c_void,
    e_sess: *mut c_void,
    /// `struct _pcred` — 104 opaque bytes.
    e_pcred: [u8; 104],
    e_ucred: Ucred,
    _pad0: [u8; 4],
    /// `struct vmspace` — 64 opaque bytes.
    e_vm: [u8; 64],
    e_ppid: i32,
    e_pgid: i32,
    e_jobc: i16,
    _pad1: [u8; 2],
    e_tdev: i32,
    e_tpgid: i32,
    _pad2: [u8; 4],
    e_tsess: *mut c_void,
    e_wmesg: [c_char; 8],
    e_xsize: i32,
    e_xrssize: i16,
    e_xccount: i16,
    e_xswrss: i16,
    _pad3: [u8; 2],
    e_flag: i32,
    e_login: [c_char; 12],
    e_spare: [i32; 4],
}

/// `struct kinfo_proc` — what `KERN_PROC_ALL` hands back, one per process.
#[repr(C)]
struct KinfoProc {
    kp_proc: ExternProc,
    kp_eproc: Eproc,
}

// The kinfo_proc ABI has been frozen at 648 bytes since 64-bit darwin shipped.
// If a future SDK ever moves it, this stops the build instead of letting the
// process table read garbage offsets.
const _: () = assert!(std::mem::size_of::<ExternProc>() == 296);
const _: () = assert!(std::mem::size_of::<Eproc>() == 352);
const _: () = assert!(std::mem::size_of::<KinfoProc>() == 648);
// vm_statistics64 is 152 bytes (HOST_VM_INFO64_COUNT = 38 integer_t words).
const _: () = assert!(std::mem::size_of::<VmStatistics64>() == 152);
const _: () = assert!(HOST_VM_INFO64_COUNT == 38);
const _: () = assert!(std::mem::size_of::<ProcTaskInfo>() == 96);
const _: () = assert!(std::mem::size_of::<IfMsghdr2>() == 160);

/// `struct if_data64` (net/if_var.h). 128 bytes.
#[repr(C)]
struct IfData64 {
    ifi_type: u8,
    ifi_typelen: u8,
    ifi_physical: u8,
    ifi_addrlen: u8,
    ifi_hdrlen: u8,
    ifi_recvquota: u8,
    ifi_xmitquota: u8,
    ifi_unused1: u8,
    ifi_mtu: u32,
    ifi_metric: u32,
    ifi_baudrate: u64,
    ifi_ipackets: u64,
    ifi_ierrors: u64,
    ifi_opackets: u64,
    ifi_oerrors: u64,
    ifi_collisions: u64,
    ifi_ibytes: u64,
    ifi_obytes: u64,
    ifi_imcasts: u64,
    ifi_omcasts: u64,
    ifi_iqdrops: u64,
    ifi_noproto: u64,
    ifi_recvtiming: u32,
    ifi_xmittiming: u32,
    ifi_lastchange: [u8; 8],
}

/// `struct if_msghdr2` (net/if.h). 160 bytes.
#[repr(C)]
struct IfMsghdr2 {
    ifm_msglen: u16,
    ifm_version: u8,
    ifm_type: u8,
    ifm_addrs: i32,
    ifm_flags: i32,
    ifm_index: u16,
    _pad: [u8; 2],
    ifm_snd_len: i32,
    ifm_snd_maxlen: i32,
    ifm_snd_drops: i32,
    ifm_timer: i32,
    ifm_data: IfData64,
}

// ---- sysctl helpers ----

/// `sysctl(mib)` into a freshly sized `Vec<u8>`, retrying once if the kernel's
/// answer grew between the sizing call and the fetch.
fn sysctl_bytes(mib: &[c_int]) -> Option<Vec<u8>> {
    for _ in 0..4 {
        let mut size = 0usize;
        // SAFETY: sizing call — a null oldp asks the kernel for the length only.
        let rc = unsafe {
            sysctl(mib.as_ptr(), mib.len() as c_uint, std::ptr::null_mut(), &mut size, std::ptr::null(), 0)
        };
        if rc != 0 || size == 0 {
            return None;
        }
        // Slack: processes can appear between the two calls.
        let mut buffer = vec![0u8; size + size / 8 + 4096];
        let mut have = buffer.len();
        // SAFETY: buffer is `have` bytes long and mib is a valid oid of len().
        let rc = unsafe {
            sysctl(mib.as_ptr(), mib.len() as c_uint, buffer.as_mut_ptr().cast(), &mut have, std::ptr::null(), 0)
        };
        if rc == 0 {
            buffer.truncate(have);
            return Some(buffer);
        }
        // ENOMEM: the table grew again — size it once more.
        if std::io::Error::last_os_error().raw_os_error() != Some(12) {
            return None;
        }
    }
    None
}

/// A fixed-size sysctl value (`hw.memsize`, `vm.swapusage`, …).
fn sysctl_value<T: Default>(mib: &[c_int]) -> Option<T> {
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>();
    // SAFETY: oldp points at exactly `size` bytes of a T we own.
    let rc = unsafe {
        sysctl(
            mib.as_ptr(),
            mib.len() as c_uint,
            (&mut value as *mut T).cast(),
            &mut size,
            std::ptr::null(),
            0,
        )
    };
    (rc == 0 && size == std::mem::size_of::<T>()).then_some(value)
}

fn c_string(bytes: &[c_char]) -> String {
    // SAFETY: kernel-provided fixed buffers are NUL terminated within bounds;
    // from_ptr stops at the first NUL and the slice outlives the borrow.
    let text = unsafe { CStr::from_ptr(bytes.as_ptr()) };
    text.to_string_lossy().into_owned()
}

// ---- the backend ----

pub struct MacosBackend {
    /// Per-core `[user, system, idle, nice]` ticks from the previous tick.
    previous_cores: Vec<[u64; CPU_STATES]>,
    /// pid → cumulative cpu nanoseconds at the previous tick.
    previous_cpu_ns: HashMap<u32, u64>,
    previous_net: Option<(u64, u64)>,
    last_sample: Option<Instant>,
    /// uid → login name (getpwuid is not cheap; the map is tiny).
    user_names: HashMap<u32, String>,
    /// pid → (program name, command line). Neither changes over a process's
    /// life and both calls are permission-gated, so they are fetched once.
    identities: HashMap<u32, (String, String)>,
    /// `mach_absolute_time` units → nanoseconds (125/3 on Apple silicon).
    timebase: (u64, u64),
    page_size: u64,
    memory_total: u64,
}

impl MacosBackend {
    pub fn new() -> Self {
        let mut info = MachTimebaseInfo { numer: 1, denom: 1 };
        // SAFETY: fills a struct we own; cannot fail on a live host.
        unsafe { mach_timebase_info(&mut info) };
        Self {
            previous_cores: Vec::new(),
            previous_cpu_ns: HashMap::new(),
            previous_net: None,
            last_sample: None,
            user_names: HashMap::new(),
            identities: HashMap::new(),
            timebase: (info.numer.max(1) as u64, info.denom.max(1) as u64),
            page_size: sysctl_value::<u64>(&[CTL_HW, HW_PAGESIZE])
                .filter(|size| *size > 0)
                .unwrap_or_else(|| {
                    sysctl_value::<u32>(&[CTL_HW, HW_PAGESIZE]).unwrap_or(4096) as u64
                }),
            memory_total: sysctl_value::<u64>(&[CTL_HW, HW_MEMSIZE]).unwrap_or(0),
        }
    }

    fn absolute_to_nanos(&self, ticks: u64) -> u64 {
        ticks.saturating_mul(self.timebase.0) / self.timebase.1
    }

    fn user_name(&mut self, uid: u32) -> String {
        if let Some(name) = self.user_names.get(&uid) {
            return name.clone();
        }
        // SAFETY: getpwuid returns a pointer into libc's static storage, valid
        // until the next call on this thread; we copy the name straight out.
        // Only the sampler thread ever calls it.
        let name = unsafe {
            let entry = getpwuid(uid);
            if entry.is_null() || (*entry).pw_name.is_null() {
                uid.to_string()
            } else {
                CStr::from_ptr((*entry).pw_name).to_string_lossy().into_owned()
            }
        };
        self.user_names.insert(uid, name.clone());
        name
    }

    /// (name, cmdline) for a pid. The name is the executable's basename, not
    /// `argv[0]` — a login shell calls itself `-zsh` and a launcher can put
    /// anything there — and falls back to the kernel's 16-char `p_comm`.
    fn identity(&mut self, pid: u32, comm: &str) -> (String, String) {
        if let Some(cached) = self.identities.get(&pid) {
            return cached.clone();
        }
        let path = proc_path(pid);
        let args = proc_args(pid);
        let name = path
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| comm.to_string());
        let cmdline = args.or(path).unwrap_or_else(|| comm.to_string());
        let identity = (name, cmdline);
        self.identities.insert(pid, identity.clone());
        identity
    }

    fn sample_cores(&mut self) -> Vec<f64> {
        let mut processor_count: u32 = 0;
        let mut info: *mut c_int = std::ptr::null_mut();
        let mut info_count: u32 = 0;
        // SAFETY: mach allocates the array and hands back its element count; we
        // free it with vm_deallocate below.
        let rc = unsafe {
            host_processor_info(
                mach_host_self(),
                PROCESSOR_CPU_LOAD_INFO,
                &mut processor_count,
                &mut info,
                &mut info_count,
            )
        };
        if rc != 0 || info.is_null() {
            return vec![0.0; self.previous_cores.len()];
        }
        // SAFETY: mach guarantees info_count valid c_int elements at info.
        let raw = unsafe { std::slice::from_raw_parts(info, info_count as usize) };
        let current: Vec<[u64; CPU_STATES]> = raw
            .chunks_exact(CPU_STATES)
            .take(processor_count as usize)
            // The counters are unsigned 32-bit; c_int is signed, so mask back.
            .map(|core| [core[0] as u32 as u64, core[1] as u32 as u64, core[2] as u32 as u64, core[3] as u32 as u64])
            .collect();
        // SAFETY: frees exactly the region mach handed us.
        unsafe {
            vm_deallocate(mach_task_self_, info as usize, info_count as usize * std::mem::size_of::<c_int>());
        }
        let first = self.previous_cores.is_empty();
        let values = current
            .iter()
            .enumerate()
            .map(|(index, ticks)| {
                if first {
                    return 0.0;
                }
                cpu_pct_from_ticks(*ticks, self.previous_cores.get(index).copied().unwrap_or([0; CPU_STATES]))
            })
            .collect();
        self.previous_cores = current;
        values
    }

    fn sample_memory(&self) -> MemInfo {
        let mut stats = VmStatistics64::default();
        let mut count = HOST_VM_INFO64_COUNT;
        // SAFETY: host_info_out points at a VmStatistics64 sized exactly count
        // 32-bit words, which is what HOST_VM_INFO64 writes.
        let rc = unsafe {
            host_statistics64(
                mach_host_self(),
                HOST_VM_INFO64,
                (&mut stats as *mut VmStatistics64).cast(),
                &mut count,
            )
        };
        let mut memory = MemInfo { total: self.memory_total, ..MemInfo::default() };
        if let Some(swap) = sysctl_value::<XswUsage>(&[CTL_VM, VM_SWAPUSAGE]) {
            memory.swap_total = swap.xsu_total;
            memory.swap_used = swap.xsu_used;
        }
        if rc != 0 {
            return memory;
        }
        let page = self.page_size;
        let pages = |count: u32| (count as u64).saturating_mul(page);
        let wired = pages(stats.wire_count);
        let compressed = pages(stats.compressor_page_count);
        let purgeable = pages(stats.purgeable_count);
        let internal = pages(stats.internal_page_count);
        // Activity Monitor's "Memory Used": app memory + wired + compressed.
        let used = internal.saturating_sub(purgeable).saturating_add(wired).saturating_add(compressed);
        memory.free = pages(stats.free_count);
        // File-backed pages are the reclaimable cache.
        memory.cache = pages(stats.external_page_count);
        memory.used = used.min(memory.total.max(used));
        memory.available = memory.total.saturating_sub(memory.used);
        memory
    }

    fn sample_processes(&mut self, elapsed_ns: u64) -> Vec<ProcInfo> {
        let Some(buffer) = sysctl_bytes(&[CTL_KERN, KERN_PROC, KERN_PROC_ALL, 0]) else {
            return Vec::new();
        };
        let stride = std::mem::size_of::<KinfoProc>();
        let count = buffer.len() / stride;
        let mut processes = Vec::with_capacity(count);
        let mut live_cpu = HashMap::with_capacity(count);
        for index in 0..count {
            // SAFETY: the kernel wrote `count` packed kinfo_proc records; the
            // buffer is at least stride*count bytes and read_unaligned copies
            // out without requiring the Vec's alignment to match.
            let entry: KinfoProc = unsafe {
                std::ptr::read_unaligned(buffer.as_ptr().add(index * stride).cast::<KinfoProc>())
            };
            let pid = entry.kp_proc.p_pid;
            if pid < 0 {
                continue;
            }
            let pid = pid as u32;
            let comm = c_string(&entry.kp_proc.p_comm);
            let task = task_info(pid);
            let cpu_ns = task
                .as_ref()
                .map(|info| self.absolute_to_nanos(info.pti_total_user.saturating_add(info.pti_total_system)));
            let cpu_pct = match (cpu_ns, self.previous_cpu_ns.get(&pid)) {
                (Some(now), Some(&before)) if elapsed_ns > 0 => {
                    now.saturating_sub(before) as f64 / elapsed_ns as f64 * 100.0
                }
                // No previous reading (first tick, or a process we may not
                // inspect): fall back to the kernel's own fixpt_t estimate.
                _ => entry.kp_proc.p_pctcpu as f64 / 2048.0 * 100.0,
            };
            if let Some(now) = cpu_ns {
                live_cpu.insert(pid, now);
            }
            let (name, cmdline) = self.identity(pid, &comm);
            processes.push(ProcInfo {
                pid,
                ppid: entry.kp_eproc.e_ppid.max(0) as u32,
                user: self.user_name(entry.kp_eproc.e_ucred.cr_uid),
                name,
                cmdline,
                cpu_pct: cpu_pct.max(0.0),
                mem_rss: task.as_ref().map(|info| info.pti_resident_size).unwrap_or(0),
                state: proc_state(entry.kp_proc.p_stat, task.as_ref()),
                threads: task.as_ref().map(|info| info.pti_threadnum.max(0) as u32).unwrap_or(0),
            });
        }
        self.previous_cpu_ns = live_cpu;
        // Drop cached identities for pids that went away, so the map cannot
        // grow without bound on a machine that churns processes.
        if self.identities.len() > processes.len() * 2 + 64 {
            let live: std::collections::HashSet<u32> = processes.iter().map(|p| p.pid).collect();
            self.identities.retain(|pid, _| live.contains(pid));
        }
        processes
    }
}

impl SystemBackend for MacosBackend {
    fn name(&self) -> &'static str {
        "macos/sysctl+mach"
    }

    fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let elapsed_ns = self
            .last_sample
            .map(|then| now.duration_since(then).as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        self.last_sample = Some(now);

        let cpu_cores = self.sample_cores();
        let cpu_total = if cpu_cores.is_empty() {
            0.0
        } else {
            cpu_cores.iter().sum::<f64>() / cpu_cores.len() as f64
        };

        let (rx_total, tx_total) = network_totals();
        let seconds = elapsed_ns as f64 / 1e9;
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

        let mut load_avg = [0.0f64; 3];
        // SAFETY: writes at most 3 f64 into an array of 3.
        unsafe { getloadavg(load_avg.as_mut_ptr(), 3) };

        Snapshot {
            cpu_total,
            cpu_cores,
            mem: self.sample_memory(),
            net,
            processes: self.sample_processes(elapsed_ns),
            load_avg,
            uptime_seconds: uptime_seconds(),
            backend: "macos/sysctl+mach",
        }
    }
}

/// Darwin keeps `p_stat` at SRUN for a process's whole life — sleeping is a
/// *thread* state there, so reading `p_stat` alone (what htop's darwin build
/// does) marks every process "R". Zombie/stopped/idle still come from
/// `p_stat`; running vs sleeping comes from the task's running-thread count.
fn proc_state(stat: i8, task: Option<&ProcTaskInfo>) -> ProcState {
    match stat {
        1 => ProcState::Idle,     // SIDL — forked, not yet exec'd
        4 => ProcState::Stopped,  // SSTOP
        5 => ProcState::Zombie,   // SZOMB
        2 | 3 => match task {
            Some(info) if info.pti_numrunning > 0 => ProcState::Running,
            Some(_) => ProcState::Sleeping,
            // Not inspectable (another user's process): say so rather than guess.
            None => ProcState::Unknown,
        },
        _ => ProcState::Unknown,
    }
}

fn task_info(pid: u32) -> Option<ProcTaskInfo> {
    let mut info = ProcTaskInfo::default();
    let size = std::mem::size_of::<ProcTaskInfo>() as c_int;
    // SAFETY: buffer is exactly `size` bytes of a ProcTaskInfo we own. libproc
    // returns the bytes written, or <=0 when we may not inspect the process.
    let written = unsafe {
        proc_pidinfo(pid as c_int, PROC_PIDTASKINFO, 0, (&mut info as *mut ProcTaskInfo).cast(), size)
    };
    (written == size).then_some(info)
}

fn proc_path(pid: u32) -> Option<String> {
    let mut buffer = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    // SAFETY: buffer owns PROC_PIDPATHINFO_MAXSIZE bytes; proc_pidpath writes
    // at most that many and returns the length (0 on failure).
    let written = unsafe { proc_pidpath(pid as c_int, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if written <= 0 {
        return None;
    }
    buffer.truncate(written as usize);
    String::from_utf8(buffer).ok().filter(|path| !path.is_empty())
}

/// `KERN_PROCARGS2`: `[argc:i32][exec path\0][padding\0…][argv…\0]`.
fn proc_args(pid: u32) -> Option<String> {
    let buffer = sysctl_bytes(&[CTL_KERN, KERN_PROCARGS2, pid as c_int])?;
    Some(parse_procargs2(&buffer)).filter(|line| !line.is_empty())
}

/// Pure parser for a `KERN_PROCARGS2` blob, split out so it is testable.
fn parse_procargs2(buffer: &[u8]) -> String {
    if buffer.len() < 8 {
        return String::new();
    }
    let argc = i32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]).max(0) as usize;
    let rest = &buffer[4..];
    // The exec path comes first, then NUL padding up to the argv block.
    let Some(path_end) = rest.iter().position(|byte| *byte == 0) else {
        return String::new();
    };
    let mut cursor = path_end;
    while cursor < rest.len() && rest[cursor] == 0 {
        cursor += 1;
    }
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        if cursor >= rest.len() {
            break;
        }
        let end = rest[cursor..].iter().position(|byte| *byte == 0).map(|at| cursor + at).unwrap_or(rest.len());
        args.push(String::from_utf8_lossy(&rest[cursor..end]).into_owned());
        cursor = end + 1;
    }
    if args.is_empty() {
        return String::from_utf8_lossy(&rest[..path_end]).into_owned();
    }
    args.join(" ")
}

/// Walk the `NET_RT_IFLIST2` message list and sum non-loopback byte counters.
fn network_totals() -> (u64, u64) {
    let Some(buffer) = sysctl_bytes(&[CTL_NET, AF_ROUTE, 0, 0, NET_RT_IFLIST2, 0]) else {
        return (0, 0);
    };
    let mut received = 0u64;
    let mut sent = 0u64;
    let mut offset = 0usize;
    while offset + 4 <= buffer.len() {
        let length = u16::from_ne_bytes([buffer[offset], buffer[offset + 1]]) as usize;
        if length < 4 || offset + length > buffer.len() {
            break;
        }
        let kind = buffer[offset + 3];
        if kind == RTM_IFINFO2 && length >= std::mem::size_of::<IfMsghdr2>() {
            // SAFETY: `length` bytes starting at offset belong to this message
            // and are at least one whole if_msghdr2; read_unaligned copies out.
            let message: IfMsghdr2 =
                unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(offset).cast::<IfMsghdr2>()) };
            if message.ifm_data.ifi_type != IFT_LOOP {
                received = received.saturating_add(message.ifm_data.ifi_ibytes);
                sent = sent.saturating_add(message.ifm_data.ifi_obytes);
            }
        }
        offset += length;
    }
    (received, sent)
}

fn uptime_seconds() -> u64 {
    let Some(boot) = sysctl_value::<Timeval>(&[CTL_KERN, KERN_BOOTTIME]) else {
        return 0;
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    now.saturating_sub(boot.tv_sec.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinfo_proc_field_offsets_match_the_darwin_abi() {
        // Spot-check the fields the process table actually reads. If any of
        // these move, the const size asserts above would already have failed.
        let base = std::mem::align_of::<KinfoProc>();
        assert_eq!(base, 8);
        assert_eq!(std::mem::offset_of!(ExternProc, p_pid), 40);
        assert_eq!(std::mem::offset_of!(ExternProc, p_stat), 36);
        assert_eq!(std::mem::offset_of!(ExternProc, p_comm), 243);
        assert_eq!(std::mem::offset_of!(ExternProc, p_pctcpu), 88);
        assert_eq!(std::mem::offset_of!(Eproc, e_ppid), 264);
        assert_eq!(std::mem::offset_of!(Eproc, e_ucred), 120);
        assert_eq!(std::mem::offset_of!(KinfoProc, kp_eproc), 296);
        assert_eq!(std::mem::offset_of!(IfMsghdr2, ifm_data), 32);
        assert_eq!(std::mem::offset_of!(IfData64, ifi_ibytes), 64);
    }

    #[test]
    fn procargs2_joins_argv_after_the_exec_path() {
        let mut blob = 2i32.to_ne_bytes().to_vec();
        blob.extend_from_slice(b"/usr/bin/demo\0\0\0");
        blob.extend_from_slice(b"demo\0--flag\0IGNORED_ENV=1\0");
        assert_eq!(parse_procargs2(&blob), "demo --flag");
    }

    #[test]
    fn procargs2_falls_back_to_the_exec_path_when_argc_is_zero() {
        let mut blob = 0i32.to_ne_bytes().to_vec();
        blob.extend_from_slice(b"/sbin/launchd\0");
        assert_eq!(parse_procargs2(&blob), "/sbin/launchd");
    }

    #[test]
    fn procargs2_ignores_a_truncated_blob() {
        assert_eq!(parse_procargs2(&[1, 0, 0]), "");
    }

    #[test]
    fn live_process_list_contains_this_test_binary() {
        let mut backend = MacosBackend::new();
        let snapshot = backend.sample();
        let me = std::process::id();
        let mine = snapshot
            .processes
            .iter()
            .find(|process| process.pid == me)
            .expect("our own pid must appear in KERN_PROC_ALL");
        assert!(mine.ppid > 0, "ppid must be real, got {}", mine.ppid);
        assert!(mine.mem_rss > 0, "rss must be real, got {}", mine.mem_rss);
        assert!(mine.threads > 0, "thread count must be real");
        assert!(!mine.user.is_empty());
        // launchd is pid 1 and parents the tree.
        assert!(snapshot.processes.iter().any(|process| process.pid == 1));
        assert!(snapshot.mem.total > 0);
        assert!(!snapshot.cpu_cores.is_empty());
    }
}

