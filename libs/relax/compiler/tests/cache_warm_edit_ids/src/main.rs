// See run.sh: cold, warm, then edits of wf-edit and wf-host interleaved, each a fresh run on one
// disk cache. Every run must print the same result.
//
// Like an app, the program tells the loader its first frame is up (`relax_first_present`, the
// hook makepad-platform calls) and keeps running past the startup window, so each run's
// mid-run write records the startup manifest and its image -- with the O2 code the hot host fns
// got after the present -- as an app's does.
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
    // hot after the present (O2 code), past the startup window (the mid-run manifest write)
    let t = std::time::Instant::now();
    while t.elapsed() < std::time::Duration::from_millis(7500) {
        for _ in 0..2000 {
            std::hint::black_box(wf_host::bump(1));
            assert_eq!(round(), r);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    println!("cache_warm_edit_ids result {}", r);
}
