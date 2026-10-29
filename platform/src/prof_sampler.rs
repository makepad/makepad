//! `prof` and `why-slow` of a native (rustc-built) app on macOS: the same sampler, buckets and
//! answer lines as `makepad run`'s inspect (libs/relax/compiler/src/inspect/prof.rs), so a JIT
//! profile and a native one diff bucket by bucket and fn by fn (`makepad inspect <pid> prof ...`
//! renders both the same way).
//!
//! In process, on the remote command thread, with nothing on the UI thread: each round suspends
//! the picked threads that are on a CPU, one at a time, walks the frame records into a fixed array
//! (every read through `mach_vm_read_overwrite`, which fails instead of faulting) and resumes it
//! before anything is allocated or locked. Names come from the main image's own symbol table (local
//! symbols included), system libraries' through dladdr.
//!
//! Buckets (the JIT's names): `program` = the app's own code, std included (under the JIT it is JIT
//! code too); `makepad run runtime / compiled-in crates` = the crates `makepad run` provides
//! rustc-built (tools/run/build.rs PROVIDED: the HAL, the script VM, the shader compiler), whose
//! code is the same machine code both ways; `system (called by the program)`.
//!
//! `raw=<file>` also writes the samples as recorded (`save_raw`), for the JIT side's `vsnative file=`
//! (JIT-vs-native per source fn on macOS, where no process may sample another one).
//!
//! `wall=1` also samples the picked threads while they wait (a wall-clock profile): those samples
//! count in the bucket `off CPU (waiting)`, and their stacks say what they wait for.

use makepad_remote::{json_str, Out, Req};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};

#[path = "thread/ui_hang/demangle.rs"]
mod demangle;

/// tools/run/build.rs PROVIDED, as crate names
const PROVIDED: &[&str] = &[
    "relax_provided_fixture",
    "makepad_live_id",
    "makepad_script",
    "makepad_script_std",
    "makepad_script_census",
    "makepad_script_compute",
    "makepad_shader",
    "makepad_tween_script",
    "makepad_script_audio_aot",
    "makepad_hal",
];

const OFF_CPU: &str = "off CPU (waiting)";

mod sys {
    use super::*;
    #[repr(C)]
    pub struct DlInfo {
        pub filename: *const c_char,
        pub base: *mut c_void,
        pub sname: *const c_char,
        pub saddr: *mut c_void,
    }
    extern "C" {
        pub static mach_task_self_: u32;
        pub fn mach_thread_self() -> u32;
        pub fn task_threads(task: u32, list: *mut *mut u32, count: *mut u32) -> i32;
        pub fn thread_info(t: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
        pub fn thread_suspend(t: u32) -> i32;
        pub fn thread_resume(t: u32) -> i32;
        pub fn thread_get_state(t: u32, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
        pub fn mach_vm_read_overwrite(task: u32, addr: u64, size: u64, data: u64, out: *mut u64) -> i32;
        pub fn mach_vm_deallocate(task: u32, addr: u64, size: u64) -> i32;
        pub fn mach_port_deallocate(task: u32, name: u32) -> i32;
        pub fn pthread_from_mach_thread_np(port: u32) -> *mut c_void;
        pub fn pthread_getname_np(t: *mut c_void, buf: *mut c_char, len: usize) -> i32;
        pub fn pthread_get_stackaddr_np(t: *mut c_void) -> *mut c_void;
        pub fn pthread_get_stacksize_np(t: *mut c_void) -> usize;
        pub fn dladdr(a: *const c_void, info: *mut DlInfo) -> i32;
        pub fn _dyld_get_image_header(i: u32) -> *const u8;
        pub fn _dyld_get_image_vmaddr_slide(i: u32) -> isize;
    }
}

/// The thread that ran the app's `Cx::new` (the UI thread), by its mach thread id.
static MAIN_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Called on the UI thread when the remote endpoint starts.
pub fn note_main_thread() {
    let port = unsafe { sys::mach_thread_self() };
    if let Some(id) = thread_id(port) {
        MAIN_ID.store(id, std::sync::atomic::Ordering::Relaxed);
    }
    unsafe { sys::mach_port_deallocate(sys::mach_task_self_, port) };
}

fn thread_id(port: u32) -> Option<u64> {
    let mut info = [0i32; 6];
    let mut n = 6u32;
    (unsafe { sys::thread_info(port, 4, info.as_mut_ptr(), &mut n) } == 0).then(|| (info[0] as u32 as u64) | ((info[1] as u32 as u64) << 32))
}

/// (CPU ns, on a CPU now)
fn thread_cpu(port: u32) -> Option<(u64, bool)> {
    let mut i = [0i32; 10];
    let mut n = 10u32;
    if unsafe { sys::thread_info(port, 3, i.as_mut_ptr(), &mut n) } != 0 {
        return None;
    }
    let ns = (i[0] as u64 + i[2] as u64) * 1_000_000_000 + (i[1] as u64 + i[3] as u64) * 1000;
    Some((ns, i[6] == 1))
}

struct Th {
    port: u32,
    /// the UI thread (`threads=main`)
    ui: bool,
    label: String,
    name: String,
    lo: u64,
    hi: u64,
    cpu0: u64,
}

/// The process's threads (the caller's and the remote server's left out); ports released by
/// `release`.
fn threads() -> Vec<Th> {
    let me = unsafe { sys::mach_thread_self() };
    let mut list: *mut u32 = std::ptr::null_mut();
    let mut n = 0u32;
    let mut out = Vec::new();
    if unsafe { sys::task_threads(sys::mach_task_self_, &mut list, &mut n) } != 0 {
        return out;
    }
    let ports: Vec<u32> = (0..n as usize).map(|k| unsafe { *list.add(k) }).collect();
    unsafe { sys::mach_vm_deallocate(sys::mach_task_self_, list as u64, n as u64 * 4) };
    let main = MAIN_ID.load(std::sync::atomic::Ordering::Relaxed);
    for (k, &port) in ports.iter().enumerate() {
        let pt = unsafe { sys::pthread_from_mach_thread_np(port) };
        let mut buf = [0 as c_char; 64];
        let mut name = String::new();
        if !pt.is_null() && unsafe { sys::pthread_getname_np(pt, buf.as_mut_ptr(), buf.len()) } == 0 {
            name = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
        }
        let id = thread_id(port).unwrap_or(port as u64);
        // the UI thread: the one that ran Cx::new (a native macOS app's `makepad-app` thread: the process's
        // first thread only runs the AppKit loop), else the first thread
        let ui = if main != 0 { id == main } else { k == 0 };
        if name.is_empty() {
            name = if k == 0 { "process-main".into() } else { String::new() };
        }
        if port == me || name.starts_with("makepad-remote") {
            unsafe { sys::mach_port_deallocate(sys::mach_task_self_, port) };
            continue;
        }
        let (lo, hi) = if pt.is_null() {
            (0, u64::MAX)
        } else {
            let top = unsafe { sys::pthread_get_stackaddr_np(pt) } as u64;
            (top.saturating_sub(unsafe { sys::pthread_get_stacksize_np(pt) } as u64), top)
        };
        let cpu0 = thread_cpu(port).map_or(0, |c| c.0);
        out.push(Th { port, label: String::new(), name, ui, lo, hi, cpu0 });
    }
    unsafe { sys::mach_port_deallocate(sys::mach_task_self_, me) };
    // the UI thread first, then by OS order: T1 is the UI thread
    out.sort_by_key(|t| !t.ui);
    for (k, t) in out.iter_mut().enumerate() {
        t.label = format!("T{}", k + 1);
    }
    out
}

fn release(ths: &[Th]) {
    for t in ths {
        unsafe { sys::mach_port_deallocate(sys::mach_task_self_, t.port) };
    }
}

#[inline]
fn strip(p: u64) -> u64 {
    if cfg!(target_arch = "aarch64") {
        p & 0x0000_ffff_ffff_ffff
    } else {
        p
    }
}

/// One stack of `t` (suspended for the walk): pcs innermost first, into `buf`.
fn walk(t: &Th, buf: &mut [u64; 128]) -> usize {
    let mut state = [0u64; 34];
    #[cfg(target_arch = "aarch64")]
    let (flavor, mut count, pc_i, fp_i, lr_i) = (6, 68u32, 32usize, 29usize, Some(30usize));
    #[cfg(target_arch = "x86_64")]
    let (flavor, mut count, pc_i, fp_i, lr_i) = (4, 42u32, 16usize, 6usize, None::<usize>);
    let read16 = |a: u64, r: &mut [u64; 2]| -> bool {
        let mut got = 0u64;
        unsafe { sys::mach_vm_read_overwrite(sys::mach_task_self_, a, 16, r.as_mut_ptr() as u64, &mut got) == 0 && got == 16 }
    };
    let mut n = 0;
    unsafe {
        if sys::thread_suspend(t.port) != 0 {
            return 0;
        }
        if sys::thread_get_state(t.port, flavor, state.as_mut_ptr().cast(), &mut count) == 0 {
            buf[0] = strip(state[pc_i]);
            n = (buf[0] != 0) as usize;
            let mut fp = strip(state[fp_i]);
            let mut first = true;
            while n < buf.len() && fp >= t.lo && fp.saturating_add(16) <= t.hi && fp & 7 == 0 {
                let mut rec = [0u64; 2];
                if !read16(fp, &mut rec) {
                    break;
                }
                let ret = strip(rec[1]);
                // arm64: a leaf that has not pushed its frame record yet returns through lr, not
                // through the record fp points at (its caller's)
                if first {
                    first = false;
                    if let Some(li) = lr_i {
                        let lr = strip(state[li]);
                        if lr != 0 && lr != ret && n < buf.len() {
                            buf[n] = lr;
                            n += 1;
                        }
                    }
                }
                if ret == 0 || n >= buf.len() {
                    break;
                }
                buf[n] = ret;
                n += 1;
                let next = strip(rec[0]);
                if next <= fp {
                    break;
                }
                fp = next;
            }
        }
        sys::thread_resume(t.port);
    }
    n
}

struct Sample {
    th: usize,
    pcs: Vec<u64>,
    on_cpu: bool,
}

struct Run {
    samples: Vec<Sample>,
    ths: Vec<Th>,
    cpu1: Vec<u64>,
    secs: f64,
    hz: u64,
    wall: bool,
}

fn pick(sel: &str, t: &Th) -> bool {
    match sel {
        "all" | "" => true,
        "main" | "ui" => t.ui,
        s => s.split(',').any(|s| s == t.label || (!s.is_empty() && t.name.starts_with(s))),
    }
}

/// Samples the threads `sel` picks for `secs` at `hz` (on a CPU only, unless `wall`).
fn collect(secs: f64, hz: u64, sel: &str, wall: bool, gone: &dyn Fn() -> bool) -> Run {
    let ths = threads();
    let want: Vec<usize> = (0..ths.len()).filter(|&k| pick(sel, &ths[k])).collect();
    let period = std::time::Duration::from_micros(1_000_000 / hz.max(1));
    let mut samples = Vec::with_capacity(((secs * hz as f64) as usize).min(1 << 17));
    let mut buf = [0u64; 128];
    let mut last: Vec<u64> = ths.iter().map(|t| t.cpu0).collect();
    let mut ran = vec![true; ths.len()];
    let t0 = std::time::Instant::now();
    let mut next_look = t0;
    while t0.elapsed().as_secs_f64() < secs && !gone() {
        let tick = std::time::Instant::now();
        // which threads ran in the last 50 ms (only those are checked each round)
        if tick >= next_look {
            next_look = tick + std::time::Duration::from_millis(50);
            for &k in &want {
                ran[k] = thread_cpu(ths[k].port).is_some_and(|(c, on)| {
                    let moved = c != last[k] || on;
                    last[k] = c;
                    moved
                });
            }
        }
        for &k in &want {
            let on = thread_cpu(ths[k].port).is_some_and(|c| c.1);
            if !wall && !(ran[k] && on) {
                continue;
            }
            let n = walk(&ths[k], &mut buf);
            if n > 0 {
                samples.push(Sample { th: k, pcs: buf[..n].to_vec(), on_cpu: on });
            }
        }
        let spent = tick.elapsed();
        if spent < period {
            std::thread::sleep(period - spent);
        }
    }
    let secs = t0.elapsed().as_secs_f64().min(secs);
    let cpu1 = ths.iter().map(|t| thread_cpu(t.port).map_or(t.cpu0, |c| c.0)).collect();
    Run { samples, ths, cpu1, secs, hz, wall }
}

// ------------------------------------------------------------------ names

#[derive(Clone)]
struct Frame {
    name: String,
    module: String,
    /// in the main image (the app, rustc-built)
    app: bool,
}

/// The main image's defined symbols (address, name), sorted, from its own LC_SYMTAB (local symbols
/// included: dladdr may give only the exported ones).
struct Image {
    lo: u64,
    hi: u64,
    syms: Vec<(u64, String)>,
    name: String,
}

fn main_image() -> Option<Image> {
    unsafe {
        let h = sys::_dyld_get_image_header(0);
        if h.is_null() || *(h as *const u32) != 0xfeed_facf {
            return None;
        }
        let slide = sys::_dyld_get_image_vmaddr_slide(0) as i64 as u64;
        let ncmds = *(h.add(16) as *const u32);
        let mut p = h.add(32);
        let (mut symoff, mut nsyms, mut stroff) = (0u64, 0u64, 0u64);
        let (mut link_vm, mut link_off) = (0u64, 0u64);
        let (mut lo, mut hi) = (u64::MAX, 0u64);
        for _ in 0..ncmds {
            let cmd = *(p as *const u32);
            let size = *(p.add(4) as *const u32) as usize;
            if cmd == 0x19 {
                let seg = CStr::from_ptr(p.add(8) as *const c_char).to_string_lossy();
                let vm = *(p.add(24) as *const u64);
                let vmsize = *(p.add(32) as *const u64);
                let fileoff = *(p.add(40) as *const u64);
                if seg == "__LINKEDIT" {
                    link_vm = vm;
                    link_off = fileoff;
                }
                if seg == "__TEXT" {
                    lo = vm.wrapping_add(slide);
                    hi = lo + vmsize;
                }
            } else if cmd == 0x2 {
                symoff = *(p.add(8) as *const u32) as u64;
                nsyms = *(p.add(12) as *const u32) as u64;
                stroff = *(p.add(16) as *const u32) as u64;
            }
            p = p.add(size);
        }
        if nsyms == 0 || link_vm == 0 {
            return None;
        }
        let at = |off: u64| (link_vm.wrapping_add(slide) + off - link_off) as *const u8;
        let tab = at(symoff);
        let strs = at(stroff);
        let mut syms = Vec::with_capacity(nsyms as usize / 2);
        for k in 0..nsyms as usize {
            let e = tab.add(k * 16);
            let strx = *(e as *const u32) as usize;
            let ty = *e.add(4);
            let value = *(e.add(8) as *const u64);
            // defined in a section, not a debugger entry
            if ty & 0xe0 != 0 || ty & 0x0e != 0x0e || value == 0 {
                continue;
            }
            let raw = CStr::from_ptr(strs.add(strx) as *const c_char).to_string_lossy();
            syms.push((value.wrapping_add(slide), raw.into_owned()));
        }
        syms.sort_by_key(|s| s.0);
        let name = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "app".into());
        Some(Image { lo, hi, syms, name })
    }
}

fn symbolise(img: &Option<Image>, pc: u64) -> Frame {
    if let Some(img) = img {
        if pc >= img.lo && pc < img.hi {
            let i = img.syms.partition_point(|s| s.0 <= pc);
            if let Some((_, raw)) = i.checked_sub(1).map(|i| &img.syms[i]) {
                let raw = raw.strip_prefix('_').unwrap_or(raw);
                return Frame { name: demangle::demangle(raw), module: "native".into(), app: true };
            }
            return Frame { name: format!("{}+{:#x}", img.name, pc - img.lo), module: "native".into(), app: true };
        }
    }
    let mut info: sys::DlInfo = unsafe { std::mem::zeroed() };
    if unsafe { sys::dladdr(pc as *const c_void, &mut info) } == 0 {
        return Frame { name: format!("{:#x}", pc), module: "?".into(), app: false };
    }
    let module = if info.filename.is_null() { "?".to_string() } else { unsafe { CStr::from_ptr(info.filename) }.to_string_lossy().rsplit('/').next().unwrap_or("?").to_string() };
    let name = if info.sname.is_null() { format!("{}+{:#x}", module, pc.wrapping_sub(info.base as u64)) } else { demangle::demangle(&unsafe { CStr::from_ptr(info.sname) }.to_string_lossy()) };
    Frame { name, module, app: false }
}

/// The frames of every sample, named (return addresses looked up one byte back, inside the call).
fn names(run: &Run) -> Vec<Vec<Frame>> {
    let img = main_image();
    let mut cache: HashMap<u64, Frame> = HashMap::new();
    run.samples
        .iter()
        .map(|s| {
            s.pcs
                .iter()
                .enumerate()
                .map(|(i, &p)| {
                    let p = if i == 0 { p } else { p.wrapping_sub(1) };
                    cache.entry(p).or_insert_with(|| symbolise(&img, p)).clone()
                })
                .collect()
        })
        .collect()
}

/// The crate a demangled name's code belongs to: its first path segment (a method's: its type's).
fn crate_of(name: &str) -> &str {
    let n = name.trim_start_matches('<').trim_start_matches('&').trim_start_matches("mut ");
    n.split("::").next().unwrap_or(n)
}

fn activity(s: &Sample, f: &[Frame]) -> &'static str {
    if !s.on_cpu {
        return OFF_CPU;
    }
    match f.first() {
        Some(x) if x.app => {
            if PROVIDED.contains(&crate_of(&x.name)) {
                "makepad run runtime / compiled-in crates"
            } else {
                "program"
            }
        }
        _ => {
            if f.iter().any(|x| x.app) {
                "system (called by the program)"
            } else {
                "system"
            }
        }
    }
}

fn short(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut k = n;
    while !s.is_char_boundary(k) {
        k -= 1;
    }
    format!("{}..", &s[..k])
}

// ------------------------------------------------------------------ answers

/// `prof [s=3] [hz=200] [threads=all|main|<name>|T<n>] [n=15] [wall=1] [save=1]`
pub fn cmd_prof(req: &Req, out: &mut Out) {
    let secs = (req.num("s", 3) as f64).clamp(0.2, 30.0);
    let hz = req.num("hz", 200).clamp(10, 2000);
    let n = req.num("n", 15) as usize;
    let wall = req.get("wall") == Some("1");
    if req.get("ev").is_some_and(|e| e != "cpu") {
        return out.err("a native app samples CPU time only (macOS gives no per-thread hardware sampling without root)");
    }
    out.limit = usize::MAX;
    let gone = out.gone();
    let run = collect(secs, hz, req.get("threads").unwrap_or("all"), wall, &*gone);
    if gone() {
        return release(&run.ths);
    }
    if let Some(path) = req.get("raw") {
        match save_raw(&run, path) {
            Ok(k) => out.line(&format!("{{\"raw\":{},\"samples\":{}}}", json_str(path), k)),
            Err(e) => out.err(&format!("raw={}: {}", path, e)),
        }
    }
    let syms = names(&run);
    report(&run, &syms, n, req.get("save") == Some("1"), out);
    release(&run.ths);
}

/// `prof raw=<file>`: the samples as recorded, for `makepad inspect <jit pid> vsnative file=<file>`
/// (the JIT side names them by this executable's DWARF inline tree). Lines, tab separated:
/// `exe <path>`, `secs <s>`, `hz <n>`, then `s <weight> <frame>...` innermost first, a frame being
/// `@<hex>` (a file virtual address in this executable) or `!<name>` (another image's symbol).
fn save_raw(run: &Run, path: &str) -> Result<usize, String> {
    use std::fmt::Write as _;
    let img = main_image();
    let slide = unsafe { sys::_dyld_get_image_vmaddr_slide(0) } as i64 as u64;
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mut t = format!("exe\t{}\nsecs\t{:.3}\nhz\t{}\n", exe, run.secs, run.hz);
    let mut k = 0;
    for smp in run.samples.iter().filter(|s| s.on_cpu) {
        t.push_str("s\t1");
        for (i, &pc) in smp.pcs.iter().enumerate() {
            match &img {
                Some(m) if pc >= m.lo && pc < m.hi => {
                    let _ = write!(t, "\t@{:x}", pc.wrapping_sub(slide));
                }
                _ => {
                    let f = symbolise(&img, if i == 0 { pc } else { pc.wrapping_sub(1) });
                    let _ = write!(t, "\t!{}", f.name.replace('\t', " "));
                }
            }
        }
        t.push('\n');
        k += 1;
    }
    std::fs::write(path, t).map_err(|e| e.to_string())?;
    Ok(k)
}

fn cpu_ms(run: &Run, k: usize) -> f64 {
    run.cpu1[k].saturating_sub(run.ths[k].cpu0) as f64 / 1e6
}

fn report(run: &Run, syms: &[Vec<Frame>], n: usize, save: bool, out: &mut Out) {
    let total = run.samples.len().max(1);
    let mut partial = Vec::new();
    if run.wall {
        partial.push("wall=1: waiting threads sampled too (bucket `off CPU (waiting)`)".to_string());
    }
    out.measured(&format!("CPU samples: {} over {:.1} s at {} Hz of CPU time per thread ({:.1} ms each)", run.samples.len(), run.secs, run.hz, 1000.0 / run.hz as f64), "thread suspend (native, in process)", &partial);
    out.line(&format!("{{\"window_s\":{:.2},\"hz\":{},\"samples\":{}}}", run.secs, run.hz, run.samples.len()));
    let mut per = vec![0usize; run.ths.len()];
    for s in &run.samples {
        per[s.th] += 1;
    }
    let mut by_cpu: Vec<(usize, f64, usize)> = (0..run.ths.len()).map(|k| (k, cpu_ms(run, k), per[k])).filter(|x| x.1 >= 1.0 || x.2 > 0).collect();
    by_cpu.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(b.2.cmp(&a.2)));
    if let Some(&(k, ms, _)) = by_cpu.first() {
        let wall = run.secs * 1000.0;
        if ms < wall * 0.8 && ms > 0.0 {
            let (seen, of) = off_cpu_stacks(&run.ths[k]);
            if let Some((st, c)) = seen.first() {
                out.line(&format!("{{\"off_cpu\":{{\"tid\":\"{}\",\"pct\":{:.0},\"stack\":{},\"seen\":{},\"of\":{}}}}}", run.ths[k].label, (wall - ms) * 100.0 / wall.max(1.0), json_str(&short(st, 240)), c, of));
            }
        }
    }
    let ts: Vec<String> = by_cpu.iter().take(12).map(|&(k, ms, c)| format!("{{\"thread\":{},\"id\":\"{}\",\"cpu_ms\":{:.0},\"pct_of_samples\":{:.1}}}", json_str(&run.ths[k].name), run.ths[k].label, ms, c as f64 * 100.0 / total as f64)).collect();
    let rest: f64 = by_cpu.iter().skip(12).map(|x| x.1).sum();
    out.line(&format!("{{\"threads\":[{}],\"other_threads\":{},\"other_cpu_ms\":{:.0}}}", ts.join(","), by_cpu.len().saturating_sub(12), rest));
    let mut act: Vec<(&str, usize)> = Vec::new();
    for (s, f) in run.samples.iter().zip(syms) {
        let a = activity(s, f);
        match act.iter_mut().find(|x| x.0 == a) {
            Some(x) => x.1 += 1,
            None => act.push((a, 1)),
        }
    }
    act.sort_by(|a, b| b.1.cmp(&a.1));
    let a: Vec<String> = act.iter().map(|(k, c)| format!("{}:{:.1}", json_str(k), *c as f64 * 100.0 / total as f64)).collect();
    out.line(&format!("{{\"activity_pct\":{{{}}}}}", a.join(",")));
    // self and total by fn
    let key = |f: &Frame| format!("{} [{}]", f.name, f.module);
    let mut selfc: HashMap<String, usize> = HashMap::new();
    let mut totc: HashMap<String, usize> = HashMap::new();
    let mut rep: HashMap<String, &Frame> = HashMap::new();
    let mut by_th: HashMap<String, HashMap<usize, usize>> = HashMap::new();
    for (s, f) in run.samples.iter().zip(syms) {
        if let Some(x) = f.first() {
            rep.entry(key(x)).or_insert(x);
            *selfc.entry(key(x)).or_default() += 1;
            *by_th.entry(key(x)).or_default().entry(s.th).or_default() += 1;
        }
        let mut seen: Vec<String> = Vec::new();
        for x in f {
            let k = key(x);
            if !seen.contains(&k) {
                *totc.entry(k.clone()).or_default() += 1;
                seen.push(k);
            }
        }
    }
    let mut top: Vec<(&String, &usize)> = selfc.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (k, c) in top.iter().take(n) {
        let f = rep[*k];
        let mut fields = format!(",\"in\":{}", json_str(&f.module));
        if let Some((t, _)) = by_th.get(*k).and_then(|m| m.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))) {
            fields.push_str(&format!(",\"thread\":{},\"tid\":\"{}\"", json_str(&run.ths[*t].name), run.ths[*t].label));
        }
        out.line(&format!("{{\"self_pct\":{:.1},\"total_pct\":{:.1},\"fn\":{}{}}}", **c as f64 * 100.0 / total as f64, *totc.get(*k).unwrap_or(c) as f64 * 100.0 / total as f64, json_str(&short(&f.name, 160)), fields));
    }
    out.line(&format!("{{\"fns_not_shown\":{},\"not_shown_self_pct\":{:.1}}}", top.len().saturating_sub(n), top.iter().skip(n).map(|x| *x.1).sum::<usize>() as f64 * 100.0 / total as f64));
    let mut tops: Vec<(&String, &usize)> = totc.iter().filter(|x| *x.1 * 100 >= total * 10).collect();
    tops.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let t: Vec<String> = tops.iter().take(n).map(|(k, c)| format!("[{:.1},{}]", **c as f64 * 100.0 / total as f64, json_str(&short(k, 120)))).collect();
    out.line(&format!("{{\"inclusive_pct_over_10\":[{}]}}", t.join(",")));
    if save {
        // folded stacks (flame graph input): thread;outer;...;inner count
        let mut folded: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for (s, f) in run.samples.iter().zip(syms) {
            let mut parts: Vec<String> = vec![run.ths[s.th].name.clone()];
            if !s.on_cpu {
                parts.push(OFF_CPU.into());
            }
            parts.extend(f.iter().rev().map(|x| x.name.replace(';', ":")));
            *folded.entry(parts.join(";")).or_default() += 1;
        }
        let path = std::env::temp_dir().join(format!("makepad-native-prof-{}-{}.folded", std::process::id(), makepad_remote::now_ms()));
        let text: String = folded.iter().map(|(k, v)| format!("{} {}\n", k, v)).collect();
        match std::fs::write(&path, text) {
            Ok(()) => out.line(&format!("{{\"saved\":{},\"format\":\"folded stacks (flamegraph.pl / speedscope)\"}}", json_str(&path.display().to_string()))),
            Err(e) => out.err(&format!("cannot write {}: {}", path.display(), e)),
        }
    }
}

/// Where `t` is when it is not on a CPU: its stack a few times (15 ms apart), the distinct
/// "<top> from <first app fn>" most seen first.
fn off_cpu_stacks(t: &Th) -> (Vec<(String, usize)>, usize) {
    let img = main_image();
    let mut seen: Vec<(String, usize)> = Vec::new();
    let mut of = 0;
    let mut buf = [0u64; 128];
    for _ in 0..5 {
        if !thread_cpu(t.port).is_some_and(|c| c.1) {
            let n = walk(t, &mut buf);
            if n > 0 {
                let fr: Vec<Frame> = buf[..n.min(32)].iter().enumerate().map(|(i, &p)| symbolise(&img, if i == 0 { p } else { p.wrapping_sub(1) })).collect();
                let top = fr.first().map(|f| f.name.clone()).unwrap_or_default();
                let app = fr.iter().find(|f| f.app).map(|f| format!(" from {}", f.name)).unwrap_or_default();
                let k = format!("{}{}", top, app);
                of += 1;
                match seen.iter_mut().find(|x| x.0 == k) {
                    Some(x) => x.1 += 1,
                    None => seen.push((k, 1)),
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    seen.sort_by(|a, b| b.1.cmp(&a.1));
    (seen, of)
}

/// `why-slow [s=3] [n=12]`: the UI thread's time, ranked: busy share, its buckets, its hottest app
/// fns, where it waits, other busy threads (the JIT's why-slow without its owner lines).
pub fn cmd_why_slow(req: &Req, out: &mut Out) {
    let secs = (req.num("s", 3) as f64).clamp(0.5, 30.0);
    out.limit = usize::MAX;
    let gone = out.gone();
    let run = collect(secs, 250, "all", false, &*gone);
    if gone() {
        return release(&run.ths);
    }
    let syms = names(&run);
    out.measured(&format!("{:.1} s: CPU samples of every thread at 250 Hz, thread CPU times", run.secs), "thread suspend (native, in process)", &[]);
    let Some(main) = run.ths.iter().position(|t| t.ui) else {
        release(&run.ths);
        return out.err("no main thread found");
    };
    let wall_ms = run.secs * 1000.0;
    let main_ms = cpu_ms(&run, main);
    let ms_samples: Vec<(&Sample, &Vec<Frame>)> = run.samples.iter().zip(&syms).filter(|(s, _)| s.th == main).collect();
    let ms_per = main_ms / ms_samples.len().max(1) as f64;
    let mut findings: Vec<(f64, String)> = vec![(main_ms, format!("main thread busy {:.0}% of the time ({:.0} ms CPU in {:.0} ms)", main_ms * 100.0 / wall_ms.max(1.0), main_ms, wall_ms))];
    let mut act: Vec<(&str, usize)> = Vec::new();
    for (s, f) in &ms_samples {
        let a = activity(s, f);
        match act.iter_mut().find(|x| x.0 == a) {
            Some(x) => x.1 += 1,
            None => act.push((a, 1)),
        }
    }
    for (a, c) in &act {
        let ms = *c as f64 * ms_per;
        findings.push((ms * 0.99 - 0.01, format!("main: {:.0} ms in {}", ms, a)));
    }
    let mut hot: HashMap<String, usize> = HashMap::new();
    for (_, f) in &ms_samples {
        if let Some(x) = f.iter().find(|x| x.app) {
            *hot.entry(x.name.clone()).or_default() += 1;
        }
    }
    let mut hv: Vec<(String, usize)> = hot.into_iter().collect();
    hv.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (k, c) in hv.iter().take(5) {
        findings.push((*c as f64 * ms_per, format!("main hot: {} ({:.0} ms)", short(k, 200), *c as f64 * ms_per)));
    }
    for k in 0..run.ths.len() {
        let ms = cpu_ms(&run, k);
        if k != main && ms > wall_ms * 0.3 {
            findings.push((ms * 0.3, format!("thread {} ({}) used {:.0} ms CPU", run.ths[k].name, run.ths[k].label, ms)));
        }
    }
    let off = (wall_ms - main_ms).max(0.0);
    if off > wall_ms * 0.2 {
        let (seen, of) = off_cpu_stacks(&run.ths[main]);
        if let Some((k, c)) = seen.first() {
            findings.push((main_ms - 0.001, format!("main off CPU {:.0}% of the time ({:.0} ms); its stack then ({} of {} captures): {}", off * 100.0 / wall_ms.max(1.0), off, c, of, short(k, 240))));
        }
    }
    findings.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    for (i, (_, f)) in findings.iter().take(req.num("n", 12) as usize).enumerate() {
        out.line(&format!("{{\"rank\":{},\"finding\":{}}}", i + 1, json_str(f)));
    }
    out.line("{\"more\":[\"prof threads=main s=5\",\"prof threads=main wall=1 save=1\"]}");
    release(&run.ths);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_a_busy_thread_by_name() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        // the sampler lists the threads once, when it starts: the spinner must be running (and
        // so named) by then, or the run finds no thread of that name (0 samples)
        let (started, ready) = std::sync::mpsc::channel();
        let h = std::thread::Builder::new()
            .name("prof-sampler-spin".into())
            .spawn(move || {
                let _ = started.send(());
                let mut x = 1u64;
                while !s2.load(std::sync::atomic::Ordering::Relaxed) {
                    x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
                }
                x
            })
            .unwrap();
        ready.recv().unwrap();
        let run = collect(0.5, 200, "prof-sampler-spin", false, &|| false);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        h.join().unwrap();
        assert!(run.samples.len() > 20, "{} samples", run.samples.len());
        let syms = names(&run);
        // the spinning closure is this test's code, named from the binary's own symbol table
        assert!(syms.iter().any(|f| f.iter().any(|x| x.app && x.name.contains("samples_a_busy_thread_by_name"))), "{:?}", syms.first().map(|f| f.iter().map(|x| x.name.clone()).collect::<Vec<_>>()));
        assert!(syms.iter().all(|f| f.first().is_some_and(|x| x.app)), "the spinning thread runs app code only");
        release(&run.ths);
    }

    #[test]
    fn buckets() {
        assert_eq!(crate_of("<makepad_hal::backend::apple::metal::Metal as makepad_hal::Backend>::encode_pass"), "makepad_hal");
        assert_eq!(crate_of("makepad_game_sim::step::step_world"), "makepad_game_sim");
    }
}
