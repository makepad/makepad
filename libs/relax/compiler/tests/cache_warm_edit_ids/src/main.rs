// See run.sh: cold, warm, then edits of wf-edit and wf-host interleaved, each a fresh run on one
// disk cache. Every run must print the same result.
//
// Like an app, the program tells the loader its first frame is up (`relax_first_present`, the
// hook makepad-platform calls), runs hot code after it, and quits: the quit's write records the
// startup manifest and its image (quit_mark closes the manifest's window), which the next run
// prelinks. (The mid-run write of a run past the window, killed before its exit, is exit_persist's.)
use std::ffi::{c_char, c_void};

extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

fn first_present() {
    #[cfg(target_os = "macos")]
    let default = -2isize as *mut c_void;
    #[cfg(not(target_os = "macos"))]
    let default = std::ptr::null_mut();
    let hook = unsafe { dlsym(default, c"relax_first_present".as_ptr()) };
    if !hook.is_null() {
        let hook = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn()>(hook) };
        hook();
    }
}

fn round() -> u64 {
    let mut h = wf_host::Host::new();
    for cb in wf_edit::callbacks() {
        h.add(cb);
    }
    let e = wf_edit::eval(5);
    let p = wf_host::apply(|x| wf_edit::eval(x) + 1, 3);
    h.run(10) + e + p + wf_host::host_open(3) + (wf_host::bump(4) & 0) + wf_host::tick(6) + wf_edit::Item(2).later()
}

fn main() {
    let r = round();
    first_present();
    // hot after the present: a fixed amount of work, no clock
    for _ in 0..20_000 {
        std::hint::black_box(wf_host::bump(1));
        assert_eq!(round(), r);
    }
    println!("cache_warm_edit_ids result {}", r);
}
