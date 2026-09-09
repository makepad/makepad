#[cfg(not(headless))]
use crate::os::apple::apple_sys::ui_hang_sys as sys;
#[cfg(headless)]
#[path = "../../os/apple/apple_sys/ui_hang_sys.rs"]
mod sys;
use std::ffi::{c_void, CStr};

pub(super) struct Target { thread: u32, bottom: usize, top: usize }
impl Target {
    pub(super) fn current() -> Option<Self> {
        unsafe {
            let pthread = sys::pthread_self();
            let top = sys::pthread_get_stackaddr_np(pthread) as usize;
            let bottom = top.checked_sub(sys::pthread_get_stacksize_np(pthread))?;
            let thread = sys::mach_thread_self();
            // Resolve Mach stubs before any thread is suspended: a first
            // lazy binding could otherwise enter the loader under suspension.
            let mut state = [0u64; 34];
            #[cfg(target_arch = "aarch64")]
            let (flavor, mut count) = (6, 68);
            #[cfg(target_arch = "x86_64")]
            let (flavor, mut count) = (4, 42);
            sys::thread_get_state(thread, flavor, state.as_mut_ptr().cast(), &mut count);
            let mut copied = 0;
            let mut scratch = 0u64;
            sys::mach_vm_read_overwrite(sys::mach_task_self_, &state as *const _ as u64,
                8, &mut scratch as *mut _ as u64, &mut copied);
            // A resume at suspension count zero fails without changing it.
            sys::thread_resume(thread);
            (thread != 0 && top > bottom).then_some(Self { thread, bottom, top })
        }
    }

    pub(super) fn sample(&self) -> Vec<usize> {
        // All scratch is allocated before suspension. No allocator, loader,
        // logger, unwinder or Rust mutex is entered with the UI suspended.
        let mut frames = [0usize; 64];
        let mut len = 0;
        let mut state = [0u64; 34];
        #[cfg(target_arch = "aarch64")]
        let (flavor, mut count, pc_index, fp_index) = (6, 68, 32, 29);
        #[cfg(target_arch = "x86_64")]
        let (flavor, mut count, pc_index, fp_index) = (4, 42, 16, 6);
        unsafe {
            if sys::thread_suspend(self.thread) != 0 { return Vec::new(); }
            if sys::thread_get_state(self.thread, flavor, state.as_mut_ptr().cast(), &mut count) == 0 {
                frames[0] = strip_pointer(state[pc_index] as usize);
                len = usize::from(frames[0] != 0);
                let mut fp = strip_pointer(state[fp_index] as usize);
                while len < frames.len() && fp >= self.bottom && fp <= self.top.saturating_sub(16) && fp & 7 == 0 {
                    let mut record = [0usize; 2];
                    let mut read = 0;
                    if sys::mach_vm_read_overwrite(sys::mach_task_self_, fp as u64, 16,
                        record.as_mut_ptr() as u64, &mut read) != 0 || read != 16 { break; }
                    let pc = strip_pointer(record[1]);
                    if pc == 0 { break; }
                    frames[len] = pc;
                    len += 1;
                    let next = strip_pointer(record[0]);
                    if next <= fp { break; }
                    fp = next;
                }
            }
            // Every successful suspend is paired, including get_state/read failure.
            sys::thread_resume(self.thread);
        }
        frames[..len].to_vec()
    }

    pub(super) fn symbolize(&self, pc: usize) -> String {
        unsafe {
            let mut info: sys::DlInfo = std::mem::zeroed();
            if sys::dladdr(pc.saturating_sub(1) as *const c_void, &mut info) == 0 {
                return format!("0x{pc:x}");
            }
            let image = if info.filename.is_null() { "?".into() } else {
                CStr::from_ptr(info.filename).to_string_lossy()
            };
            let image = image.rsplit('/').next().unwrap_or("?");
            if info.symbol_name.is_null() {
                return format!("{image}+0x{:x} [0x{pc:x}]", pc.saturating_sub(info.image_base as usize));
            }
            let name = CStr::from_ptr(info.symbol_name).to_string_lossy();
            format!("{}+0x{:x} ({image})", demangle_legacy(&name), pc.saturating_sub(info.symbol_address as usize))
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) { unsafe { sys::mach_port_deallocate(sys::mach_task_self_, self.thread); } }
}

#[inline]
fn strip_pointer(pointer: usize) -> usize {
    #[cfg(target_arch = "aarch64")]
    { // Remove PAC/TBI bits on arm64e system-library return addresses.
        pointer & 0x0000_ffff_ffff_ffff
    }
    #[cfg(not(target_arch = "aarch64"))]
    { pointer }
}

fn demangle_legacy(name: &str) -> String {
    let Some(mut rest) = name.strip_prefix("_ZN").or_else(|| name.strip_prefix("__ZN")) else { return name.into(); };
    let mut parts = Vec::new();
    while rest != "E" && !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let Some(len) = rest[..digits].parse::<usize>().ok() else { return name.into(); };
        rest = &rest[digits..];
        let Some(part) = rest.get(..len) else { return name.into(); };
        if !(part.len() == 17 && part.starts_with('h') && part[1..].bytes().all(|b| b.is_ascii_hexdigit())) {
            parts.push(part);
        }
        rest = &rest[len..];
    }
    parts.join("::").replace("$LT$", "<").replace("$GT$", ">").replace("$u20$", " ").replace("$RF$", "&").replace("..", "::")
}
