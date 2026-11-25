// A thread's name at the OS, as std::thread::Builder::name sets it: rustc's std cuts it to what the
// OS keeps (Linux and Android: 15 bytes, TASK_COMM_LEN less its NUL; macOS: 63, MAXTHREADNAMESIZE
// less its NUL) and sets a Windows thread description whole. Relax's std passed a longer name uncut,
// the OS refused it (glibc: ERANGE), and the thread kept its creator's name: under the JIT on Linux
// the compiler's abort reporter ("run-abort-report") and libtest's threads went unnamed. Each name is
// read back at the OS from inside its thread, on both sides of the cut: the longest name kept whole,
// and one byte more.

#[cfg(unix)]
extern "C" {
    fn pthread_self() -> usize;
    fn pthread_getname_np(thread: usize, name: *mut std::ffi::c_char, len: usize) -> i32;
}

#[cfg(unix)]
fn os_name() -> String {
    let mut b = [0u8; 128];
    let r = unsafe { pthread_getname_np(pthread_self(), b.as_mut_ptr().cast(), b.len()) };
    assert_eq!(r, 0, "pthread_getname_np failed");
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).into_owned()
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThread() -> isize;
    fn GetThreadDescription(thread: isize, description: *mut *mut u16) -> i32;
    fn LocalFree(mem: *mut u16) -> *mut u16;
}

#[cfg(windows)]
fn os_name() -> String {
    let mut p: *mut u16 = std::ptr::null_mut();
    let hr = unsafe { GetThreadDescription(GetCurrentThread(), &mut p) };
    assert!(hr >= 0, "GetThreadDescription failed: {hr:#x}");
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) });
    unsafe { LocalFree(p) };
    s
}

/// The name its own thread reads back at the OS.
fn named(name: &str) -> String {
    std::thread::Builder::new().name(name.to_string()).spawn(os_name).unwrap().join().unwrap()
}

fn name_of(len: usize) -> String {
    (0..len).map(|i| (b'a' + (i % 26) as u8) as char).collect()
}

#[test]
fn a_thread_name_is_cut_where_the_os_cuts_it() {
    #[cfg(unix)]
    {
        let kept = if cfg!(target_vendor = "apple") { 63 } else { 15 };
        assert_eq!(named(&name_of(kept)), name_of(kept), "a name of {kept} bytes, the longest the OS keeps, is kept whole");
        assert_eq!(named(&name_of(kept + 1)), name_of(kept), "a name of {} bytes is cut to its first {kept}", kept + 1);
    }
    #[cfg(windows)]
    for len in [16, 64, 200] {
        assert_eq!(named(&name_of(len)), name_of(len), "a thread description of {len} characters is kept whole");
    }
}
