//! macOS backend — real system calls, no `ps`/`top` scraping.
//!
//! | data | mechanism |
//! |---|---|
//! | process list, pid/ppid/uid/state/comm/start | `sysctl(CTL_KERN, KERN_PROC, KERN_PROC_ALL)` → `struct kinfo_proc[]` |
//! | precise start identity | libproc `proc_pidinfo(pid, PROC_PIDTBSDINFO)` → `pbi_start_tvsec/usec` |
//! | per-process cpu time, rss, threads | libproc `proc_pidinfo(pid, PROC_PIDTASKINFO)` |
//! | executable path / argv | `proc_pidpath`, `sysctl(KERN_PROCARGS2)` (cached per incarnation) |
//! | per-core cpu ticks | mach `host_processor_info(PROCESSOR_CPU_LOAD_INFO)` |
//! | memory | mach `host_statistics64(HOST_VM_INFO64)` + `sysctl hw.memsize` |
//! | swap | `sysctl vm.swapusage` → `struct xsw_usage` |
//! | network bytes | `sysctl(CTL_NET, AF_ROUTE, 0, 0, NET_RT_IFLIST2)` → `struct if_msghdr2[]` |
//! | disk bytes | IOKit `IOBlockStorageDriver` → `Statistics` → `Bytes (Read)` / `Bytes (Write)` |
//! | GPU busy | IOKit `IOAccelerator` → `PerformanceStatistics` → `Device Utilization %` |
//! | threads | `PROC_PIDLISTTHREADS` (64-bit thread handles) + `PROC_PIDTHREADINFO` per handle |
//! | footprint, disk I/O, idle wake-ups per process | `proc_pid_rusage(pid, RUSAGE_INFO_V4)`, every tick |
//! | network bytes/packets per process | `com.apple.network.statistics` kernel control (see `macos_ntstat`) |
//! | open files, sockets | `PROC_PIDLISTFDS` + `proc_pidfdinfo(PROC_PIDFDVNODEPATHINFO / PROC_PIDFDSOCKETINFO)` |
//! | mapped files ("libraries") | `PROC_PIDREGIONPATHINFO` walk |
//! | load / uptime | `getloadavg`, `sysctl kern.boottime` |
//!
//! FFI is hand-written in the house style of `libs/terminal_core/src/pty.rs`:
//! small `extern "C"` blocks, no libc crate. Struct sizes and the field
//! offsets read below were printed from the installed SDK (MacOSX26) with a
//! scratch C program and are guarded by `const` asserts.
//!
//! Power is not measured: the only sources are `powermetrics` (root) and the
//! private IOReport, so the tile says "unavailable" rather than a guess.

use super::{
    bounded, cpu_pct_from_ticks, cpu_pct_from_time, not_collected, now_ms, Detail, DiskInfo, FileInfo,
    IdentityDetail, LibraryInfo, MemInfo, MemoryDetail, NetInfo, PortInfo, ProcDetail, ProcExtra,
    ProcInfo, ProcKey, ProcMeta, ProcState, Reading, RegionSummary, Snapshot, SystemBackend, ThreadInfo,
    ThreadState, Want, CPU_STATES, MAX_CMDLINE_LEN, MAX_NAME_LEN,
};
use super::macos_ntstat::Ntstat;
use crate::metrics::Measure;
use makepad_widgets::log;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ---- FFI ----

type MachPort = u32;
type KernReturn = c_int;
type CFTypeRef = *const c_void;

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
    fn proc_pidfdinfo(pid: c_int, fd: c_int, flavor: c_int, buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    fn getpwuid(uid: u32) -> *const Passwd;
    fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
    static mach_task_self_: MachPort;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(main_port: MachPort, matching: *const c_void, existing: *mut u32) -> KernReturn;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOObjectRelease(object: u32) -> KernReturn;
    fn IORegistryEntryCreateCFProperty(entry: u32, key: CFTypeRef, allocator: CFTypeRef, options: u32) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: CFTypeRef, cstr: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, number_type: isize, value: *mut c_void) -> u8;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFRelease(cf: CFTypeRef);
}

const CTL_KERN: c_int = 1;
const CTL_VM: c_int = 2;
const CTL_HW: c_int = 6;
const CTL_NET: c_int = 4;
const KERN_PROC: c_int = 14;
const KERN_PROC_ALL: c_int = 0;
const KERN_PROC_PID: c_int = 1;
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
const PROC_PIDLISTFDS: c_int = 1;
const PROC_PIDTBSDINFO: c_int = 3;
const PROC_PIDTASKINFO: c_int = 4;
const PROC_PIDLISTTHREADS: c_int = 6;
const PROC_PIDREGIONPATHINFO: c_int = 8;
/// `PROC_PIDTHREADINFO`: takes a thread HANDLE as `PROC_PIDLISTTHREADS`
/// returns it. Measured on our own process: flavor 5 answers every listed
/// handle (112 bytes, names filled in); `PROC_PIDTHREADID64INFO` (15) wants
/// the unique thread id instead and answers ESRCH for those handles.
const PROC_PIDTHREADINFO: c_int = 5;
/// Stable thread ids and per-id thread info: `PROC_PIDLISTTHREADIDS` is in
/// XNU's `sys/proc_info_private.h` (28), not the public SDK header; the
/// paired `PROC_PIDTHREADID64INFO` (15) is public. Both measured working on
/// this host.
const PROC_PIDLISTTHREADIDS: c_int = 28;
const PROC_PIDTHREADID64INFO: c_int = 15;
const PROC_PIDFDVNODEPATHINFO: c_int = 2;
const PROC_PIDFDSOCKETINFO: c_int = 3;
const PROX_FDTYPE_VNODE: u32 = 1;
const PROX_FDTYPE_SOCKET: u32 = 2;
const PROX_FDTYPE_PSHM: u32 = 3;
const PROX_FDTYPE_PSEM: u32 = 4;
const PROX_FDTYPE_KQUEUE: u32 = 5;
const PROX_FDTYPE_PIPE: u32 = 6;
const RUSAGE_INFO_V4: c_int = 4;
const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
const MAXPATHLEN: usize = 1024;
/// `TH_USAGE_SCALE` (mach/thread_info.h): `pth_cpu_usage` is per mille.
const TH_USAGE_SCALE: f64 = 1000.0;
/// Threads / descriptors read per detail collection.
const MAX_DETAIL_ITEMS: usize = 4096;
const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const KCF_NUMBER_SINT64_TYPE: isize = 4;
const ERRNO_EPERM: i32 = 1;
const ERRNO_ESRCH: i32 = 3;

/// Sizes printed from the SDK headers (`sizeof`/`offsetof`), see the module
/// docs. The byte-offset readers below index into buffers of these sizes.
const SOCKET_FDINFO_SIZE: usize = 792;
const SOCKET_SOI_PROTOCOL: usize = 180;
const SOCKET_SOI_FAMILY: usize = 184;
const SOCKET_SOI_KIND: usize = 256;
const SOCKET_SOI_PROTO: usize = 264;
const INSI_FPORT: usize = 0;
const INSI_LPORT: usize = 4;
const INSI_VFLAG: usize = 24;
const INSI_FADDR: usize = 32;
const INSI_LADDR: usize = 48;
const TCPSI_STATE: usize = 80;
const UNSI_ADDR: usize = 16;
const UNSI_CADDR: usize = 271;
const VNODE_FDINFOWITHPATH_SIZE: usize = 1200;
const VNODE_FDINFO_PATH: usize = 176;
const REGIONWITHPATH_SIZE: usize = 1272;
const REGION_PATH: usize = 248;
const REGION_PRI_FLAGS: usize = 12;
const REGION_PAGES_RESIDENT: usize = 36;
const REGION_PRIVATE_RESIDENT: usize = 64;
const REGION_SHARED_RESIDENT: usize = 68;
const REGION_ADDRESS: usize = 80;
const REGION_SIZE: usize = 88;
const SOCKINFO_IN: i32 = 1;
const SOCKINFO_TCP: i32 = 2;
const SOCKINFO_UN: i32 = 3;
const INI_IPV4: u8 = 0x1;
const INI_IPV6: u8 = 0x2;
/// `PROC_REGION_SUBMAP` in `pri_flags`: a submap, not a mapping itself.
const PROC_REGION_SUBMAP: u32 = 1;

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

/// `struct proc_taskinfo` (sys/proc_info.h). 96 bytes.
#[repr(C)]
#[derive(Default)]
struct ProcTaskInfo {
    pti_virtual_size: u64,
    pti_resident_size: u64,
    /// "total time" in the header. Measured on this SDK/host (see the
    /// task report): mach absolute-time units, scaled by `mach_timebase_info`.
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

/// `struct proc_bsdinfo` (sys/proc_info.h). 136 bytes.
#[repr(C)]
struct ProcBsdInfo {
    pbi_flags: u32,
    pbi_status: u32,
    pbi_xstatus: u32,
    pbi_pid: u32,
    pbi_ppid: u32,
    pbi_uid: u32,
    pbi_gid: u32,
    pbi_ruid: u32,
    pbi_rgid: u32,
    pbi_svuid: u32,
    pbi_svgid: u32,
    rfu_1: u32,
    pbi_comm: [c_char; 16],
    pbi_name: [c_char; 32],
    pbi_nfiles: u32,
    pbi_pgid: u32,
    pbi_pjobc: u32,
    e_tdev: u32,
    e_tpgid: u32,
    pbi_nice: i32,
    pbi_start_tvsec: u64,
    pbi_start_tvusec: u64,
}

impl Default for ProcBsdInfo {
    fn default() -> Self {
        // SAFETY: all-zero is a valid value for every field (ints and char arrays).
        unsafe { std::mem::zeroed() }
    }
}

/// `struct proc_threadinfo` (sys/proc_info.h). 112 bytes.
#[repr(C)]
struct ProcThreadInfo {
    pth_user_time: u64,
    pth_system_time: u64,
    pth_cpu_usage: i32,
    pth_policy: i32,
    pth_run_state: i32,
    pth_flags: i32,
    pth_sleep_time: i32,
    pth_curpri: i32,
    pth_priority: i32,
    pth_maxpriority: i32,
    pth_name: [c_char; 64],
}

impl Default for ProcThreadInfo {
    fn default() -> Self {
        // SAFETY: all-zero is a valid value for every field.
        unsafe { std::mem::zeroed() }
    }
}

/// `struct rusage_info_v4` (sys/resource.h). 296 bytes; read with flavor
/// `RUSAGE_INFO_V4` so the buffer and the flavor agree.
#[repr(C)]
struct RusageInfoV4 {
    ri_uuid: [u8; 16],
    ri_user_time: u64,
    ri_system_time: u64,
    ri_pkg_idle_wkups: u64,
    ri_interrupt_wkups: u64,
    ri_pageins: u64,
    ri_wired_size: u64,
    ri_resident_size: u64,
    ri_phys_footprint: u64,
    ri_proc_start_abstime: u64,
    ri_proc_exit_abstime: u64,
    ri_child_user_time: u64,
    ri_child_system_time: u64,
    ri_child_pkg_idle_wkups: u64,
    ri_child_interrupt_wkups: u64,
    ri_child_pageins: u64,
    ri_child_elapsed_abstime: u64,
    ri_diskio_bytesread: u64,
    ri_diskio_byteswritten: u64,
    ri_cpu_time_qos_default: u64,
    ri_cpu_time_qos_maintenance: u64,
    ri_cpu_time_qos_background: u64,
    ri_cpu_time_qos_utility: u64,
    ri_cpu_time_qos_legacy: u64,
    ri_cpu_time_qos_user_initiated: u64,
    ri_cpu_time_qos_user_interactive: u64,
    ri_billed_system_time: u64,
    ri_serviced_system_time: u64,
    ri_logical_writes: u64,
    ri_lifetime_max_phys_footprint: u64,
    ri_instructions: u64,
    ri_cycles: u64,
    ri_billed_energy: u64,
    ri_serviced_energy: u64,
    ri_interval_max_phys_footprint: u64,
    ri_runnable_time: u64,
}

impl Default for RusageInfoV4 {
    fn default() -> Self {
        // SAFETY: all-zero is a valid value for every field.
        unsafe { std::mem::zeroed() }
    }
}

/// `struct proc_fdinfo` (sys/proc_info.h). 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProcFdInfo {
    proc_fd: i32,
    proc_fdtype: u32,
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

/// `struct timeval` on 64-bit darwin: `long tv_sec`, `int32 tv_usec`, pad.
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
/// only the fields a task manager reads are typed. `p_un` is a union of the
/// run-queue links and `struct timeval __p_starttime`; `KERN_PROC_ALL` fills
/// the start time, which `MacosBackend::new` cross-checks against
/// `proc_bsdinfo` for our own process once at start-up.
#[repr(C)]
struct ExternProc {
    p_starttime: Timeval,
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
const _: () = assert!(std::mem::size_of::<ProcBsdInfo>() == 136);
const _: () = assert!(std::mem::size_of::<ProcThreadInfo>() == 112);
const _: () = assert!(std::mem::size_of::<RusageInfoV4>() == 296);
const _: () = assert!(std::mem::size_of::<ProcFdInfo>() == 8);
const _: () = assert!(std::mem::size_of::<Timeval>() == 16);
const _: () = assert!(std::mem::size_of::<IfMsghdr2>() == 160);
const _: () = assert!(std::mem::offset_of!(ProcBsdInfo, pbi_start_tvsec) == 120);
const _: () = assert!(std::mem::offset_of!(ProcBsdInfo, pbi_name) == 64);
const _: () = assert!(std::mem::offset_of!(ProcThreadInfo, pth_name) == 48);
const _: () = assert!(std::mem::offset_of!(RusageInfoV4, ri_phys_footprint) == 72);
const _: () = assert!(std::mem::offset_of!(RusageInfoV4, ri_diskio_bytesread) == 144);
const _: () = assert!(std::mem::offset_of!(RusageInfoV4, ri_lifetime_max_phys_footprint) == 240);

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
    let raw: Vec<u8> = bytes.iter().map(|byte| *byte as u8).take_while(|byte| *byte != 0).collect();
    String::from_utf8_lossy(&raw).into_owned()
}

fn read_u32(buffer: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([buffer[offset], buffer[offset + 1], buffer[offset + 2], buffer[offset + 3]])
}

fn read_i32(buffer: &[u8], offset: usize) -> i32 {
    read_u32(buffer, offset) as i32
}

fn read_u64(buffer: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buffer[offset..offset + 8]);
    u64::from_ne_bytes(bytes)
}

/// A NUL-terminated string somewhere inside a kernel buffer.
fn cstr_at(buffer: &[u8], offset: usize, max: usize) -> String {
    let end = (offset + max).min(buffer.len());
    let slice = &buffer[offset..end];
    let len = slice.iter().position(|byte| *byte == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..len]).into_owned()
}

/// The precise start stamp: microseconds since the epoch, as `ProcKey::start`.
fn start_micros(tv_sec: u64, tv_usec: u64) -> u64 {
    tv_sec.saturating_mul(1_000_000).saturating_add(tv_usec)
}

// ---- the backend ----

/// What the backend remembers about one process incarnation.
struct Identity {
    meta: Arc<ProcMeta>,
    /// The kinfo start stamp this incarnation was first seen with; compared
    /// exactly every tick.
    kinfo_start: u64,
    /// What the metadata was built from, to notice an exec or a reparent.
    comm: String,
    uid: u32,
    /// Cumulative cpu nanoseconds at the previous tick.
    previous_cpu_ns: Option<u64>,
}

pub struct MacosBackend {
    /// Per-core `[user, system, idle, nice]` ticks from the previous tick.
    previous_cores: Vec<[u64; CPU_STATES]>,
    previous_net: Option<(u64, u64)>,
    /// (read, written, when read) at the previous IOKit disk walk.
    previous_disk: Option<(u64, u64, Instant)>,
    last_sample: Option<Instant>,
    /// uid → login name (getpwuid is not cheap; the map is tiny).
    user_names: HashMap<u32, String>,
    /// pid → identity. Keyed by pid for the lookup, but the entry is replaced
    /// whenever the kernel's start time for that pid changes: a reused pid is
    /// a new incarnation with new metadata and no previous cpu reading.
    identities: HashMap<u32, Identity>,
    /// `mach_absolute_time` units → nanoseconds (125/3 on Apple silicon).
    timebase: (u64, u64),
    page_size: u64,
    memory_total: u64,
    disk_state: DiskCounters,
    gpu_missing_logged: bool,
    /// Per-process network counters; `None` when the kernel control could
    /// not be opened or answered in a layout this build cannot read.
    ntstat: Option<Ntstat>,
}

/// What the IOKit disk walk found last time, so a machine without the
/// statistics says so once instead of every tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DiskCounters {
    #[default]
    Untried,
    Present,
    Absent,
}

impl MacosBackend {
    pub fn new() -> Self {
        let mut info = MachTimebaseInfo { numer: 1, denom: 1 };
        // SAFETY: fills a struct we own; cannot fail on a live host.
        unsafe { mach_timebase_info(&mut info) };
        let timebase = (info.numer.max(1) as u64, info.denom.max(1) as u64);
        let backend = Self {
            previous_cores: Vec::new(),
            previous_net: None,
            previous_disk: None,
            last_sample: None,
            user_names: HashMap::new(),
            identities: HashMap::new(),
            timebase,
            page_size: sysctl_value::<u64>(&[CTL_HW, HW_PAGESIZE])
                .filter(|size| *size > 0)
                .unwrap_or_else(|| {
                    sysctl_value::<u32>(&[CTL_HW, HW_PAGESIZE]).unwrap_or(4096) as u64
                }),
            memory_total: sysctl_value::<u64>(&[CTL_HW, HW_MEMSIZE]).unwrap_or(0),
            disk_state: DiskCounters::Untried,
            gpu_missing_logged: false,
            ntstat: Ntstat::open(),
        };
        backend.check_start_time_sources();
        backend
    }

    /// The `kinfo_proc` start time is what the whole table is keyed on; it
    /// must agree with `proc_bsdinfo`'s for a process we may inspect. Checked
    /// once, on ourselves; a disagreement is logged loudly.
    fn check_start_time_sources(&self) {
        let me = std::process::id();
        let kinfo = kinfo_for_pid(me).map(|entry| entry.kp_proc.p_starttime);
        let bsd = bsd_info(me).ok();
        match (kinfo, bsd) {
            (Some(kinfo), Some(bsd)) => {
                let from_kinfo = start_micros(kinfo.tv_sec.max(0) as u64, kinfo.tv_usec.max(0) as u64);
                let from_bsd = start_micros(bsd.pbi_start_tvsec, bsd.pbi_start_tvusec);
                if from_kinfo != from_bsd {
                    log!("task: WARNING kinfo_proc start {from_kinfo} differs from proc_bsdinfo start {from_bsd}; identities use proc_bsdinfo where readable");
                } else {
                    log!("task: start identity check ok (kinfo_proc == proc_bsdinfo, {from_bsd} µs)");
                }
            }
            _ => log!("task: start identity check skipped (own process not readable)"),
        }
    }

    /// `proc_taskinfo` / `rusage` times are mach absolute-time units
    /// (measured, see the module docs); `proc_threadinfo` times are already
    /// nanoseconds and never pass through here.
    fn to_nanos(&self, ticks: u64) -> u64 {
        ((ticks as u128 * self.timebase.0 as u128) / self.timebase.1 as u128).min(u64::MAX as u128) as u64
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

    /// The metadata for a pid whose kernel start time is `kinfo_start`.
    ///
    /// The same incarnation only when the kinfo start matches EXACTLY (to
    /// the microsecond); anything else is a new process on a reused pid and
    /// gets a fresh identity. Within one incarnation a changed comm (exec),
    /// parent (reparenting) or uid interns a new metadata `Arc` under the
    /// same key, so older samples keep what was true when they were taken.
    fn identity(&mut self, entry: &KinfoProc, kinfo_start: u64) -> (Arc<ProcMeta>, Option<u64>) {
        let pid = entry.kp_proc.p_pid as u32;
        let comm = c_string(&entry.kp_proc.p_comm);
        let uid = entry.kp_eproc.e_ucred.cr_uid;
        let ppid = entry.kp_eproc.e_ppid.max(0) as u32;
        if let Some(identity) = self.identities.get(&pid) {
            if identity.kinfo_start == kinfo_start {
                if identity.comm == comm && identity.uid == uid && identity.meta.ppid == ppid {
                    return (identity.meta.clone(), identity.previous_cpu_ns);
                }
                let key = identity.meta.key;
                let previous_cpu_ns = identity.previous_cpu_ns;
                let meta = self.build_meta(key, ppid, uid, &comm);
                if let Some(identity) = self.identities.get_mut(&pid) {
                    identity.meta = meta.clone();
                    identity.comm = comm;
                    identity.uid = uid;
                }
                return (meta, previous_cpu_ns);
            }
        }
        // A new incarnation. The unambiguous proc_bsdinfo stamp when we may
        // read it; otherwise the kinfo one, which is the same kernel field
        // (checked at start-up); 0 (unverified) if neither is there.
        let start = match bsd_info(pid) {
            Ok(info) => start_micros(info.pbi_start_tvsec, info.pbi_start_tvusec),
            Err(_) => kinfo_start,
        };
        let meta = self.build_meta(ProcKey { pid, start }, ppid, uid, &comm);
        self.identities.insert(pid, Identity { meta: meta.clone(), kinfo_start, comm, uid, previous_cpu_ns: None });
        (meta, None)
    }

    fn build_meta(&mut self, key: ProcKey, ppid: u32, uid: u32, comm: &str) -> Arc<ProcMeta> {
        let path = proc_path(key.pid);
        let args = proc_args(key.pid);
        let name = path
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| comm.to_string());
        let is_app = path.as_deref().is_some_and(|path| path.contains(".app/Contents/MacOS/"));
        let cmdline = args.or(path).unwrap_or_else(|| comm.to_string());
        Arc::new(ProcMeta {
            key,
            ppid,
            user: self.user_name(uid),
            name: bounded(name, MAX_NAME_LEN),
            cmdline: bounded(cmdline, MAX_CMDLINE_LEN),
            started_secs: key.start / 1_000_000,
            is_app,
        })
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
        if self.ntstat.as_mut().is_some_and(|ntstat| !ntstat.poll()) {
            self.ntstat = None;
        }
        let mut seen: HashSet<u32> = HashSet::with_capacity(count);
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
            seen.insert(pid);
            let kinfo_start = start_micros(
                entry.kp_proc.p_starttime.tv_sec.max(0) as u64,
                entry.kp_proc.p_starttime.tv_usec.max(0) as u64,
            );
            let (meta, previous_cpu_ns) = self.identity(&entry, kinfo_start);
            let task = task_info(pid);
            let cpu_ns = task
                .as_ref()
                .map(|info| self.to_nanos(info.pti_total_user.saturating_add(info.pti_total_system)));
            let cpu_pct = match (cpu_ns, previous_cpu_ns) {
                (Some(now), Some(before)) => cpu_pct_from_time(now, before, elapsed_ns),
                // No previous reading (first tick, or a process we may not
                // inspect): fall back to the kernel's own fixpt_t estimate.
                _ => entry.kp_proc.p_pctcpu as f64 / 2048.0 * 100.0,
            };
            if let Some(identity) = self.identities.get_mut(&pid) {
                identity.previous_cpu_ns = cpu_ns;
            }
            let mut extra = task_extra(task.as_ref(), entry.kp_proc.p_nice);
            // Same permission as the task read: skip the call where that
            // was refused.
            if let Some(usage) = task.as_ref().and_then(|_| rusage_v4(pid)) {
                extra.disk_read = Some(usage.ri_diskio_bytesread);
                extra.disk_written = Some(usage.ri_diskio_byteswritten);
                extra.footprint = Some(usage.ri_phys_footprint);
                extra.idle_wakeups = Some(usage.ri_pkg_idle_wkups);
            }
            if let Some(ntstat) = self.ntstat.as_mut() {
                let net = ntstat.counts(pid, meta.key.start);
                extra.net_rx_bytes = Some(net.rx_bytes);
                extra.net_tx_bytes = Some(net.tx_bytes);
                extra.net_rx_packets = Some(net.rx_packets);
                extra.net_tx_packets = Some(net.tx_packets);
            }
            processes.push(ProcInfo {
                meta,
                cpu_pct: cpu_pct.max(0.0),
                mem_rss: task.as_ref().map(|info| info.pti_resident_size).unwrap_or(0),
                cpu_time_ns: cpu_ns,
                state: proc_state(entry.kp_proc.p_stat, task.as_ref()),
                threads: task.as_ref().map(|info| info.pti_threadnum.max(0) as u32).unwrap_or(0),
                extra,
            });
        }
        // Forget incarnations that are gone, every tick: a pid that comes
        // back is a new process and must start with fresh metadata.
        self.identities.retain(|pid, _| seen.contains(pid));
        if let Some(ntstat) = self.ntstat.as_mut() {
            ntstat.retain_pids(&seen);
        }
        processes
    }

    fn sample_disk(&mut self) -> Reading<DiskInfo> {
        let totals = block_storage_totals();
        match totals {
            Some((read_total, write_total)) => {
                if self.disk_state != DiskCounters::Present {
                    self.disk_state = DiskCounters::Present;
                }
                let read_at = Instant::now();
                let (read_per_second, write_per_second) = match self.previous_disk {
                    // The divisor is the gap between the two counter reads.
                    Some((read, write, then)) => {
                        let seconds = read_at.duration_since(then).as_secs_f64();
                        if seconds > 0.0 {
                            (
                                read_total.saturating_sub(read) as f64 / seconds,
                                write_total.saturating_sub(write) as f64 / seconds,
                            )
                        } else {
                            (0.0, 0.0)
                        }
                    }
                    _ => (0.0, 0.0),
                };
                self.previous_disk = Some((read_total, write_total, read_at));
                Reading::Value(DiskInfo { read_total, write_total, read_per_second, write_per_second })
            }
            None => {
                if self.disk_state != DiskCounters::Absent {
                    self.disk_state = DiskCounters::Absent;
                    log!("task: no IOBlockStorageDriver statistics; disk rates unavailable");
                }
                self.previous_disk = None;
                Reading::Unavailable("no IOBlockStorageDriver statistics")
            }
        }
    }

    fn sample_gpu(&mut self) -> Reading<f64> {
        match accelerator_utilisation() {
            Some(percent) => Reading::Value(percent),
            None => {
                if !self.gpu_missing_logged {
                    self.gpu_missing_logged = true;
                    log!("task: no IOAccelerator 'Device Utilization %' statistic; GPU unavailable");
                }
                Reading::Unavailable("no IOAccelerator utilisation statistic")
            }
        }
    }

    // ---- detail ----

    /// The thread list, and whether it is whole (not cut at the buffer's
    /// capacity, every listed thread read, every thread identifiable).
    ///
    /// Threads are listed by their stable 64-bit kernel thread id
    /// (`PROC_PIDLISTTHREADIDS`, 28, read per id with
    /// `PROC_PIDTHREADID64INFO`, 15; both select on `thuniqueid` in XNU's
    /// `fill_taskthreadlist` / `fill_taskthreadinfo`, and the ids match
    /// `pthread_threadid_np` — measured on this host). The older
    /// `PROC_PIDLISTTHREADS` returns each thread's user-space `cthread_self`
    /// handle, which is 0 for threads without one: several different
    /// workers then share 0 and a lookup of 0 finds only the first. It is
    /// used only if the id flavour is refused, and its 0 entries are left
    /// out as unidentifiable (the list is then not whole).
    ///
    /// `cpu_time_ns` is `pth_user_time + pth_system_time`, nanoseconds
    /// (measured). `cpu_pct` is the scheduler's aged `pth_cpu_usage`
    /// estimate, kept for the live fallback table only: recorded history
    /// derives thread CPU from `cpu_time_ns` deltas over real timestamps.
    fn detail_threads(&self, pid: u32, expected: usize) -> (Detail<Vec<ThreadInfo>>, bool) {
        let capacity = (expected.max(1) + 32).min(MAX_DETAIL_ITEMS);
        let size = (capacity * std::mem::size_of::<u64>()) as c_int;
        let mut ids = vec![0u64; capacity];
        // SAFETY: the buffer holds `capacity` u64 thread ids; libproc writes
        // at most `size` bytes and returns the count written.
        let written = unsafe { proc_pidinfo(pid as c_int, PROC_PIDLISTTHREADIDS, 0, ids.as_mut_ptr().cast(), size) };
        let (mut list, flavor, unidentified) = if written > 0 {
            ids.truncate(written as usize / std::mem::size_of::<u64>());
            (ids, PROC_PIDTHREADID64INFO, 0usize)
        } else {
            let mut handles = vec![0u64; capacity];
            // SAFETY: as above, for PROC_PIDLISTTHREADS handles.
            let written = unsafe { proc_pidinfo(pid as c_int, PROC_PIDLISTTHREADS, 0, handles.as_mut_ptr().cast(), size) };
            if written <= 0 {
                return (denied_or_gone(), false);
            }
            handles.truncate(written as usize / std::mem::size_of::<u64>());
            let zeros = handles.iter().filter(|handle| **handle == 0).count();
            (handles, PROC_PIDTHREADINFO, zeros)
        };
        let cut = list.len() >= capacity;
        list.retain(|id| *id != 0);
        let listed = list.len();
        let mut threads = Vec::with_capacity(listed);
        let mut last_error = None;
        for id in list {
            let mut info = ProcThreadInfo::default();
            let size = std::mem::size_of::<ProcThreadInfo>() as c_int;
            // SAFETY: a ProcThreadInfo we own, exactly `size` bytes; the arg
            // is an id (or handle) exactly as the matching list returned it.
            let written = unsafe { proc_pidinfo(pid as c_int, flavor, id, (&mut info as *mut ProcThreadInfo).cast(), size) };
            if written != size {
                // A thread can exit between the list and the read.
                last_error = std::io::Error::last_os_error().raw_os_error();
                continue;
            }
            let state = match info.pth_run_state {
                1 => ThreadState::Running,
                2 => ThreadState::Stopped,
                3 => ThreadState::Waiting,
                4 => ThreadState::Uninterruptible,
                5 => ThreadState::Halted,
                _ => ThreadState::Unknown,
            };
            threads.push(ThreadInfo {
                id,
                name: c_string(&info.pth_name),
                state,
                cpu_pct: Some(info.pth_cpu_usage.max(0) as f64 * 100.0 / TH_USAGE_SCALE),
                // Already nanoseconds (measured): NOT timebase-scaled.
                cpu_time_ns: Some(info.pth_user_time.saturating_add(info.pth_system_time)),
            });
        }
        if threads.is_empty() && listed > 0 {
            // Every read failed: that is not "no threads".
            return (
                Detail::Unavailable(match last_error {
                    Some(ERRNO_EPERM) => "permission denied: thread details were refused".to_string(),
                    Some(code) => format!("{listed} threads listed, none readable (errno {code})"),
                    None => format!("{listed} threads listed, none readable"),
                }),
                false,
            );
        }
        let complete = !cut && unidentified == 0 && threads.len() == listed;
        (Detail::Ready(threads), complete)
    }

    /// Descriptors and sockets, and whether the list is whole (not cut at
    /// the buffer's capacity).
    fn detail_files(&self, pid: u32, expected: usize) -> (Detail<Vec<FileInfo>>, Detail<Vec<PortInfo>>, bool) {
        let capacity = (expected.max(1) + 64).min(MAX_DETAIL_ITEMS);
        let mut fds = vec![ProcFdInfo::default(); capacity];
        let size = (fds.len() * std::mem::size_of::<ProcFdInfo>()) as c_int;
        // SAFETY: `capacity` proc_fdinfo records we own; libproc writes at
        // most `size` bytes and returns the count written.
        let written = unsafe { proc_pidinfo(pid as c_int, PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), size) };
        if written <= 0 {
            return (denied_or_gone(), denied_or_gone(), false);
        }
        fds.truncate(written as usize / std::mem::size_of::<ProcFdInfo>());
        let complete = fds.len() < capacity;
        let mut files = Vec::new();
        let mut ports = Vec::new();
        let mut vnode_buffer = vec![0u8; VNODE_FDINFOWITHPATH_SIZE];
        let mut socket_buffer = vec![0u8; SOCKET_FDINFO_SIZE];
        for fd in fds {
            match fd.proc_fdtype {
                PROX_FDTYPE_VNODE => {
                    // SAFETY: buffer is exactly VNODE_FDINFOWITHPATH_SIZE bytes,
                    // the size of struct vnode_fdinfowithpath this flavor fills.
                    let written = unsafe {
                        proc_pidfdinfo(pid as c_int, fd.proc_fd, PROC_PIDFDVNODEPATHINFO, vnode_buffer.as_mut_ptr().cast(), VNODE_FDINFOWITHPATH_SIZE as c_int)
                    };
                    let path = if written as usize == VNODE_FDINFOWITHPATH_SIZE {
                        cstr_at(&vnode_buffer, VNODE_FDINFO_PATH, MAXPATHLEN)
                    } else {
                        String::new()
                    };
                    files.push(FileInfo { fd: fd.proc_fd, kind: "file", path: if path.is_empty() { "(path not readable)".to_string() } else { path } });
                }
                PROX_FDTYPE_SOCKET => {
                    // SAFETY: buffer is exactly SOCKET_FDINFO_SIZE bytes, the
                    // size of struct socket_fdinfo this flavor fills.
                    let written = unsafe {
                        proc_pidfdinfo(pid as c_int, fd.proc_fd, PROC_PIDFDSOCKETINFO, socket_buffer.as_mut_ptr().cast(), SOCKET_FDINFO_SIZE as c_int)
                    };
                    if written as usize == SOCKET_FDINFO_SIZE {
                        match parse_socket(&socket_buffer) {
                            Some(port) => {
                                files.push(FileInfo { fd: fd.proc_fd, kind: "socket", path: format!("{} {} → {}", port.protocol, port.local, port.remote) });
                                ports.push(port);
                            }
                            None => files.push(FileInfo { fd: fd.proc_fd, kind: "socket", path: "(other socket family)".to_string() }),
                        }
                    } else {
                        files.push(FileInfo { fd: fd.proc_fd, kind: "socket", path: "(not readable)".to_string() });
                    }
                }
                PROX_FDTYPE_PSHM => files.push(FileInfo { fd: fd.proc_fd, kind: "shm", path: String::new() }),
                PROX_FDTYPE_PSEM => files.push(FileInfo { fd: fd.proc_fd, kind: "sem", path: String::new() }),
                PROX_FDTYPE_KQUEUE => files.push(FileInfo { fd: fd.proc_fd, kind: "kqueue", path: String::new() }),
                PROX_FDTYPE_PIPE => files.push(FileInfo { fd: fd.proc_fd, kind: "pipe", path: String::new() }),
                other => files.push(FileInfo { fd: fd.proc_fd, kind: "other", path: format!("(fd type {other})") }),
            }
        }
        (Detail::Ready(files), Detail::Ready(ports), complete)
    }

    /// Walk the address space with `PROC_PIDREGIONPATHINFO`: every mapping,
    /// with the file behind it when there is one. Bounded at 8192 regions.
    fn detail_regions(&self, pid: u32) -> Result<(RegionSummary, Vec<LibraryInfo>), Detail<()>> {
        let mut buffer = vec![0u8; REGIONWITHPATH_SIZE];
        let mut address: u64 = 0;
        let mut summary = RegionSummary::default();
        let mut files: HashMap<String, u64> = HashMap::new();
        let mut any = false;
        let page = self.page_size;
        for _ in 0..8192 {
            // SAFETY: buffer is exactly REGIONWITHPATH_SIZE bytes, the size of
            // struct proc_regionwithpathinfo; `address` asks for the region
            // at or after it.
            let written = unsafe {
                proc_pidinfo(pid as c_int, PROC_PIDREGIONPATHINFO, address, buffer.as_mut_ptr().cast(), REGIONWITHPATH_SIZE as c_int)
            };
            if written as usize != REGIONWITHPATH_SIZE {
                if !any {
                    let errno = std::io::Error::last_os_error().raw_os_error();
                    if errno == Some(ERRNO_EPERM) {
                        return Err(Detail::denied());
                    }
                    if errno == Some(ERRNO_ESRCH) {
                        return Err(Detail::Unavailable("the process is gone".to_string()));
                    }
                }
                break;
            }
            any = true;
            let region_address = read_u64(&buffer, REGION_ADDRESS);
            let region_size = read_u64(&buffer, REGION_SIZE);
            let next = region_address.saturating_add(region_size.max(page));
            if next <= address {
                break;
            }
            address = next;
            if read_u32(&buffer, REGION_PRI_FLAGS) & PROC_REGION_SUBMAP != 0 {
                continue;
            }
            summary.regions += 1;
            summary.resident += read_u32(&buffer, REGION_PAGES_RESIDENT) as u64 * page;
            summary.private_resident += read_u32(&buffer, REGION_PRIVATE_RESIDENT) as u64 * page;
            summary.shared_resident += read_u32(&buffer, REGION_SHARED_RESIDENT) as u64 * page;
            let path = cstr_at(&buffer, REGION_PATH, MAXPATHLEN);
            if !path.is_empty() {
                *files.entry(path).or_insert(0) += region_size;
            }
        }
        if !any {
            return Err(Detail::Unavailable("no regions reported".to_string()));
        }
        let mut libraries: Vec<LibraryInfo> = files.into_iter().map(|(path, mapped)| LibraryInfo { path, mapped }).collect();
        libraries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((summary, libraries))
    }
}

/// `proc_pidinfo` said no: EPERM is "not ours", anything else is "gone".
fn denied_or_gone<T>() -> Detail<T> {
    match std::io::Error::last_os_error().raw_os_error() {
        Some(ERRNO_EPERM) => Detail::denied(),
        Some(ERRNO_ESRCH) => Detail::Unavailable("the process is gone".to_string()),
        Some(code) => Detail::Unavailable(format!("the OS refused (errno {code})")),
        None => Detail::Unavailable("the OS refused".to_string()),
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
        let disk = self.sample_disk();
        let gpu_pct = self.sample_gpu();
        self.last_sample = Some(now);

        let mut load_avg = [0.0f64; 3];
        // SAFETY: writes at most 3 f64 into an array of 3.
        unsafe { getloadavg(load_avg.as_mut_ptr(), 3) };

        Snapshot {
            cpu_total,
            cpu_cores,
            mem: self.sample_memory(),
            net,
            disk,
            gpu_pct,
            power_watts: Reading::Unavailable("not measured: needs powermetrics (root)"),
            processes: self.sample_processes(elapsed_ns),
            load_avg,
            uptime_seconds: uptime_seconds(),
            backend: "macos/sysctl+mach",
        }
    }

    fn detail(&mut self, key: ProcKey, want: Want) -> ProcDetail {
        let pid = key.pid;
        let time_ms = now_ms();
        let bsd = match bsd_info(pid) {
            Ok(info) => info,
            Err(Detail::Unavailable(reason)) => return ProcDetail::unavailable(key, time_ms, &reason),
            Err(Detail::Ready(())) => return ProcDetail::gone(key, time_ms),
        };
        if start_micros(bsd.pbi_start_tvsec, bsd.pbi_start_tvusec) != key.start {
            return ProcDetail::gone(key, time_ms);
        }
        let task = task_info(pid);
        let status = match bsd.pbi_status {
            1 => "idle (forked, not yet exec'd)",
            2 => match task.as_ref() {
                Some(info) if info.pti_numrunning > 0 => "running",
                Some(_) => "sleeping",
                None => "runnable",
            },
            3 => "sleeping",
            4 => "stopped",
            5 => "zombie",
            _ => "unknown",
        };
        let identity = IdentityDetail {
            path: proc_path(pid).unwrap_or_default(),
            status: status.to_string(),
            cpu_time_ns: task.as_ref().map(|info| self.to_nanos(info.pti_total_user.saturating_add(info.pti_total_system))),
            threads: task.as_ref().map(|info| info.pti_threadnum.max(0) as u32).unwrap_or(0),
            running_threads: task.as_ref().map(|info| info.pti_numrunning.max(0) as u32).unwrap_or(0),
        };
        let rusage = rusage_v4(pid);
        let regions = if want.libraries { Some(self.detail_regions(pid)) } else { None };
        let memory = match task.as_ref() {
            Some(_) => Detail::Ready(MemoryDetail {
                regions: match &regions {
                    Some(Ok((summary, _))) => Some(*summary),
                    _ => None,
                },
            }),
            None => denied_or_gone(),
        };
        let (threads, threads_complete) = if want.threads { self.detail_threads(pid, identity.threads as usize) } else { (not_collected(), false) };
        let (files, ports, files_complete) = if want.files { self.detail_files(pid, bsd.pbi_nfiles as usize) } else { (not_collected(), not_collected(), false) };
        let libraries = regions.map(|result| match result {
            Ok((_, libraries)) => Detail::Ready(libraries),
            Err(Detail::Unavailable(reason)) => Detail::Unavailable(reason),
            Err(Detail::Ready(())) => Detail::Ready(Vec::new()),
        });
        // What this read adds to the basic sample's figures. Units measured
        // on this host (task report): the rusage sizes are bytes, as are the
        // disk counters; `pbi_nfiles` is the descriptor table's size.
        let mut measures = vec![(Measure::FdTable, bsd.pbi_nfiles as i64)];
        if let Some(r) = rusage.as_ref() {
            measures.push((Measure::Footprint, r.ri_phys_footprint as i64));
            measures.push((Measure::PeakFootprint, r.ri_lifetime_max_phys_footprint as i64));
            measures.push((Measure::Wired, r.ri_wired_size as i64));
            measures.push((Measure::DiskRead, r.ri_diskio_bytesread as i64));
            measures.push((Measure::DiskWritten, r.ri_diskio_byteswritten as i64));
        }
        if let (Detail::Ready(list), true) = (&files, files_complete) {
            measures.push((Measure::OpenFds, list.len() as i64));
        }
        ProcDetail { key, time_ms, identity: Detail::Ready(identity), memory, threads, files, ports, libraries, measures, threads_complete, files_complete }
    }

    fn is_alive(&mut self, key: ProcKey) -> Option<bool> {
        if !key.verified() {
            return None;
        }
        if let Ok(info) = bsd_info(key.pid) {
            return Some(start_micros(info.pbi_start_tvsec, info.pbi_start_tvusec) == key.start);
        }
        // Not ours to inspect: ask sysctl for this pid's kinfo_proc. The read
        // (not the sizing call, which adds slack) returns no bytes for a pid
        // no process has — measured on this host; otherwise the kinfo start
        // is the same kernel field.
        let mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, key.pid as c_int];
        let mut buffer = vec![0u8; std::mem::size_of::<KinfoProc>()];
        let mut have = buffer.len();
        // SAFETY: buffer is `have` bytes long and mib is a valid oid of len().
        let rc = unsafe { sysctl(mib.as_ptr(), mib.len() as c_uint, buffer.as_mut_ptr().cast(), &mut have, std::ptr::null(), 0) };
        if rc != 0 {
            return None;
        }
        if have < std::mem::size_of::<KinfoProc>() {
            return Some(false);
        }
        // SAFETY: the kernel wrote one whole kinfo_proc.
        let entry: KinfoProc = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<KinfoProc>()) };
        if entry.kp_proc.p_pid != key.pid as i32 {
            return None;
        }
        Some(start_micros(entry.kp_proc.p_starttime.tv_sec.max(0) as u64, entry.kp_proc.p_starttime.tv_usec.max(0) as u64) == key.start)
    }

    fn signal_verified(&mut self, key: ProcKey, force: bool) -> Result<(), String> {
        let current = match bsd_info(key.pid) {
            Ok(info) => Some(start_micros(info.pbi_start_tvsec, info.pbi_start_tvusec)),
            // Not ours to inspect: the kinfo start is the same kernel field.
            Err(_) => kinfo_for_pid(key.pid).map(|entry| {
                start_micros(entry.kp_proc.p_starttime.tv_sec.max(0) as u64, entry.kp_proc.p_starttime.tv_usec.max(0) as u64)
            }),
        };
        match current {
            None => Err(format!("PID {} is gone; not signalled", key.pid)),
            Some(start) if start != key.start => Err(format!("PID {} now belongs to another process; not signalled", key.pid)),
            Some(_) => super::unix_signal::terminate(key.pid, force),
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

/// What `proc_taskinfo` and `kinfo_proc` carry beyond CPU and resident size.
/// The task counters are `int32_t` in the kernel (sys/proc_info.h: "number
/// of page faults", "number of actual pageins", "number of copy-on-write
/// faults", "number of context switches", mach/unix system calls): read as
/// the unsigned count they wrap through.
fn task_extra(task: Option<&ProcTaskInfo>, nice: i8) -> ProcExtra {
    let nice = Some(nice as i32);
    let Some(info) = task else { return ProcExtra { nice, ..ProcExtra::default() } };
    let count = |value: i32| Some(value as u32 as u64);
    ProcExtra {
        virtual_bytes: Some(info.pti_virtual_size),
        faults: count(info.pti_faults),
        pageins: count(info.pti_pageins),
        cow_faults: count(info.pti_cow_faults),
        context_switches: count(info.pti_csw),
        syscalls: Some(info.pti_syscalls_mach as u32 as u64 + info.pti_syscalls_unix as u32 as u64),
        priority: Some(info.pti_priority),
        nice,
        running_threads: Some(info.pti_numrunning.max(0) as u32),
        ..ProcExtra::default()
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

/// `PROC_PIDTBSDINFO`: the unambiguous start stamp, status and nice. Err
/// says why the process could not be read.
fn bsd_info(pid: u32) -> Result<ProcBsdInfo, Detail<()>> {
    let mut info = ProcBsdInfo::default();
    let size = std::mem::size_of::<ProcBsdInfo>() as c_int;
    // SAFETY: buffer is exactly `size` bytes of a ProcBsdInfo we own.
    let written = unsafe {
        proc_pidinfo(pid as c_int, PROC_PIDTBSDINFO, 0, (&mut info as *mut ProcBsdInfo).cast(), size)
    };
    if written == size {
        Ok(info)
    } else {
        Err(denied_or_gone())
    }
}

fn rusage_v4(pid: u32) -> Option<RusageInfoV4> {
    let mut info = RusageInfoV4::default();
    // SAFETY: flavor V4 fills exactly a rusage_info_v4, which is what we own.
    let rc = unsafe { proc_pid_rusage(pid as c_int, RUSAGE_INFO_V4, (&mut info as *mut RusageInfoV4).cast()) };
    (rc == 0).then_some(info)
}

/// One `kinfo_proc` by pid (`KERN_PROC_PID`), for start-time verification of
/// a process we may not `proc_pidinfo`.
fn kinfo_for_pid(pid: u32) -> Option<KinfoProc> {
    let buffer = sysctl_bytes(&[CTL_KERN, KERN_PROC, KERN_PROC_PID, pid as c_int])?;
    if buffer.len() < std::mem::size_of::<KinfoProc>() {
        return None;
    }
    // SAFETY: the kernel wrote at least one whole kinfo_proc; read_unaligned
    // copies it out of the byte buffer.
    let entry: KinfoProc = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<KinfoProc>()) };
    (entry.kp_proc.p_pid == pid as i32).then_some(entry)
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
    let mut args = Vec::with_capacity(argc.min(4096));
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

/// One `struct socket_fdinfo` → a port row for the IN/TCP/UNIX families.
/// Port fields are `int` in network byte order in the kernel's pcb, as lsof
/// reads them (`ntohs`).
fn parse_socket(buffer: &[u8]) -> Option<PortInfo> {
    let kind = read_i32(buffer, SOCKET_SOI_KIND);
    let protocol_number = read_i32(buffer, SOCKET_SOI_PROTOCOL);
    let family = read_i32(buffer, SOCKET_SOI_FAMILY);
    let proto = &buffer[SOCKET_SOI_PROTO..];
    match kind {
        SOCKINFO_IN | SOCKINFO_TCP => {
            let port = |offset: usize| u16::from_be((read_i32(proto, offset) & 0xffff) as u16);
            let vflag = proto[INSI_VFLAG];
            let address = |offset: usize| {
                if vflag & INI_IPV6 != 0 && vflag & INI_IPV4 == 0 {
                    let mut octets = [0u8; 16];
                    octets.copy_from_slice(&proto[offset..offset + 16]);
                    std::net::Ipv6Addr::from(octets).to_string()
                } else {
                    // in4in6_addr: three words of padding, then the IPv4 address.
                    let mut octets = [0u8; 4];
                    octets.copy_from_slice(&proto[offset + 12..offset + 16]);
                    std::net::Ipv4Addr::from(octets).to_string()
                }
            };
            let (protocol, state) = if kind == SOCKINFO_TCP {
                ("tcp", tcp_state(read_i32(proto, TCPSI_STATE)).to_string())
            } else if protocol_number == 17 {
                ("udp", String::new())
            } else {
                ("ip", format!("protocol {protocol_number}"))
            };
            let local = format!("{}:{}", address(INSI_LADDR), port(INSI_LPORT));
            let remote_port = port(INSI_FPORT);
            let remote = if remote_port == 0 { "*".to_string() } else { format!("{}:{}", address(INSI_FADDR), remote_port) };
            Some(PortInfo { protocol, local, remote, state })
        }
        SOCKINFO_UN => {
            // sockaddr_un: sun_len, sun_family, then the path.
            let local = cstr_at(proto, UNSI_ADDR + 2, 104);
            let remote = cstr_at(proto, UNSI_CADDR + 2, 104);
            Some(PortInfo {
                protocol: "unix",
                local: if local.is_empty() { "(unnamed)".to_string() } else { local },
                remote: if remote.is_empty() { "-".to_string() } else { remote },
                state: if family == 1 { String::new() } else { format!("family {family}") },
            })
        }
        _ => None,
    }
}

fn tcp_state(state: i32) -> &'static str {
    match state {
        0 => "CLOSED",
        1 => "LISTEN",
        2 => "SYN_SENT",
        3 => "SYN_RECEIVED",
        4 => "ESTABLISHED",
        5 => "CLOSE_WAIT",
        6 => "FIN_WAIT_1",
        7 => "CLOSING",
        8 => "LAST_ACK",
        9 => "FIN_WAIT_2",
        10 => "TIME_WAIT",
        _ => "?",
    }
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

// ---- IOKit ----

/// A CFString we created and must release.
struct CfString(CFTypeRef);

impl CfString {
    fn new(text: &str) -> Option<Self> {
        let c = std::ffi::CString::new(text).ok()?;
        // SAFETY: a NUL-terminated UTF-8 string; CF copies it. Null allocator
        // is the default allocator.
        let string = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), KCF_STRING_ENCODING_UTF8) };
        (!string.is_null()).then_some(Self(string))
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        // SAFETY: we own exactly one reference from CFStringCreateWithCString.
        unsafe { CFRelease(self.0) };
    }
}

/// A CF property we copied out of the registry and must release.
struct CfProperty(CFTypeRef);

impl CfProperty {
    fn of(entry: u32, key: &CfString) -> Option<Self> {
        // SAFETY: entry is a live io_registry_entry_t from IOIteratorNext;
        // the returned object is a +1 reference we release in Drop.
        let value = unsafe { IORegistryEntryCreateCFProperty(entry, key.0, std::ptr::null(), 0) };
        (!value.is_null()).then_some(Self(value))
    }

    fn is_dictionary(&self) -> bool {
        // SAFETY: a valid CF object.
        unsafe { CFGetTypeID(self.0) == CFDictionaryGetTypeID() }
    }

    /// A 64-bit integer under `key` in this dictionary, if it is one.
    fn number(&self, key: &CfString) -> Option<i64> {
        if !self.is_dictionary() {
            return None;
        }
        // SAFETY: self is a dictionary; the value is borrowed from it and
        // only read while self is alive.
        unsafe {
            let value = CFDictionaryGetValue(self.0, key.0);
            if value.is_null() || CFGetTypeID(value) != CFNumberGetTypeID() {
                return None;
            }
            let mut out: i64 = 0;
            if CFNumberGetValue(value, KCF_NUMBER_SINT64_TYPE, (&mut out as *mut i64).cast()) == 0 {
                return None;
            }
            Some(out)
        }
    }
}

impl Drop for CfProperty {
    fn drop(&mut self) {
        // SAFETY: we own the reference IORegistryEntryCreateCFProperty returned.
        unsafe { CFRelease(self.0) };
    }
}

/// Every registry entry of IOKit class `class_name`, visited by `visit`.
fn for_each_service(class_name: &str, mut visit: impl FnMut(u32)) -> bool {
    let Ok(class) = std::ffi::CString::new(class_name) else { return false };
    // SAFETY: IOServiceMatching builds a dictionary the matching call consumes.
    let matching = unsafe { IOServiceMatching(class.as_ptr()) };
    if matching.is_null() {
        return false;
    }
    let mut iterator: u32 = 0;
    // SAFETY: port 0 is the default main port; the matching dictionary's one
    // reference is consumed here whatever the result.
    let rc = unsafe { IOServiceGetMatchingServices(0, matching, &mut iterator) };
    if rc != 0 || iterator == 0 {
        return false;
    }
    let mut any = false;
    loop {
        // SAFETY: a valid iterator; zero ends it. Each entry is released.
        let entry = unsafe { IOIteratorNext(iterator) };
        if entry == 0 {
            break;
        }
        any = true;
        visit(entry);
        unsafe { IOObjectRelease(entry) };
    }
    // SAFETY: releases the iterator we were handed.
    unsafe { IOObjectRelease(iterator) };
    any
}

/// Cumulative bytes read and written across every `IOBlockStorageDriver`.
fn block_storage_totals() -> Option<(u64, u64)> {
    let statistics = CfString::new("Statistics")?;
    let read_key = CfString::new("Bytes (Read)")?;
    let write_key = CfString::new("Bytes (Write)")?;
    let mut read = 0u64;
    let mut write = 0u64;
    let mut found = false;
    for_each_service("IOBlockStorageDriver", |entry| {
        if let Some(stats) = CfProperty::of(entry, &statistics) {
            if let (Some(r), Some(w)) = (stats.number(&read_key), stats.number(&write_key)) {
                found = true;
                read = read.saturating_add(r.max(0) as u64);
                write = write.saturating_add(w.max(0) as u64);
            }
        }
    });
    found.then_some((read, write))
}

/// The busiest accelerator's "Device Utilization %", as its driver reports it.
fn accelerator_utilisation() -> Option<f64> {
    let statistics = CfString::new("PerformanceStatistics")?;
    let utilisation = CfString::new("Device Utilization %")?;
    let mut best: Option<f64> = None;
    for_each_service("IOAccelerator", |entry| {
        if let Some(stats) = CfProperty::of(entry, &statistics) {
            if let Some(value) = stats.number(&utilisation) {
                let value = (value as f64).clamp(0.0, 100.0);
                best = Some(best.map_or(value, |b: f64| b.max(value)));
            }
        }
    });
    best
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
            .find(|process| process.meta.key.pid == me)
            .expect("our own pid must appear in KERN_PROC_ALL");
        assert!(mine.meta.ppid > 0, "ppid must be real, got {}", mine.meta.ppid);
        assert!(mine.mem_rss > 0, "rss must be real, got {}", mine.mem_rss);
        assert!(mine.threads > 0, "thread count must be real");
        assert!(!mine.meta.user.is_empty());
        // launchd is pid 1 and parents the tree.
        assert!(snapshot.processes.iter().any(|process| process.meta.key.pid == 1));
        assert!(snapshot.mem.total > 0);
        assert!(!snapshot.cpu_cores.is_empty());
    }
}
