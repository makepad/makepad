//! Per-thread CPU accounting for measurement runs: with `MAKEPAD_THREADCPU=<file>` set, every
//! `profile::set` (the remote's `trace` command turning the frame profile on or off) appends one
//! line `unix_ms \t name#tid=cumulative_cpu_ns ...` (user + system, all live threads of the
//! process, from the kernel's per-thread accounting: nothing is suspended or sampled). Taken
//! synchronously on the calling thread (no helper thread: a thread spawned from JIT code lives
//! in the compiler zone and dies with its swap). macOS only; elsewhere the variable is ignored.
#[cfg(target_os = "macos")]
pub fn snapshot() {
    use std::io::Write;
    let Some(path) = std::env::var("MAKEPAD_THREADCPU").ok().filter(|s| !s.is_empty()) else { return };
    #[repr(C)]
    struct Ident { thread_id: u64, thread_handle: u64, dispatch_qaddr: u64 }
    #[repr(C)]
    struct Ext { user: u64, system: u64, cpu_usage: i32, policy: i32, run_state: i32, flags: i32, sleep_time: i32, curpri: i32, pri: i32, maxpri: i32, name: [std::ffi::c_char; 64] }
    extern "C" {
        static mach_task_self_: u32;
        fn task_threads(task: u32, list: *mut *mut u32, count: *mut u32) -> i32;
        fn thread_info(thread: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
        fn mach_port_deallocate(task: u32, name: u32) -> i32;
        fn mach_vm_deallocate(task: u32, address: u64, size: u64) -> i32;
    }
    let unix_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let mut line = format!("{unix_ms}");
    unsafe {
        let task = mach_task_self_;
        let mut list: *mut u32 = std::ptr::null_mut();
        let mut count = 0u32;
        if task_threads(task, &mut list, &mut count) == 0 {
            for &t in std::slice::from_raw_parts(list, count as usize) {
                let mut id: Ident = std::mem::zeroed();
                let mut ni = (std::mem::size_of::<Ident>() / 4) as u32;
                let mut ext: Ext = std::mem::zeroed();
                let mut ne = (std::mem::size_of::<Ext>() / 4) as u32;
                if thread_info(t, 4, &mut id as *mut _ as *mut i32, &mut ni) == 0
                    && thread_info(t, 5, &mut ext as *mut _ as *mut i32, &mut ne) == 0
                {
                    let name = std::ffi::CStr::from_ptr(ext.name.as_ptr()).to_string_lossy().replace(['\t', '=', ' '], "_");
                    let name = if name.is_empty() { "thread".to_string() } else { name };
                    line.push_str(&format!("\t{}#{}={}", name, id.thread_id, ext.user + ext.system));
                }
                mach_port_deallocate(task, t);
            }
            mach_vm_deallocate(task, list as u64, count as u64 * 4);
        }
    }
    line.push('\n');
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(not(target_os = "macos"))]
pub fn snapshot() {}
