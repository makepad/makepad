//! The compiler's and the program's stdout/stderr go nowhere in an Android app: a pipe on both
//! descriptors, read by a thread that writes each line to logcat as it comes (tag "makepad-host").

use std::ffi::{c_char, c_int, CString};

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

extern "C" {
    fn pipe(fds: *mut c_int) -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut u8, n: usize) -> isize;
    fn close(fd: c_int) -> c_int;
}

const ANDROID_LOG_INFO: c_int = 4;

pub fn log(text: &str) {
    let tag = c"makepad-host";
    if let Ok(t) = CString::new(text.replace('\0', " ")) {
        unsafe { __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), t.as_ptr()) };
    }
}

/// Once per process.
pub fn redirect_stdio() {
    unsafe {
        let mut fds = [0 as c_int; 2];
        if pipe(fds.as_mut_ptr()) != 0 {
            return;
        }
        dup2(fds[1], 1);
        dup2(fds[1], 2);
        close(fds[1]);
        let rd = fds[0];
        // The reader allocates nothing and takes no lock: it writes each line from a fixed buffer
        // straight to liblog. (It used to build a String and a CString per line, and its lines
        // reached logcat in bursts up to a minute late while the JIT compiled.)
        std::thread::Builder::new()
            .name("run-logcat".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                // one line, NUL-terminated for liblog; longer lines are split
                let mut line = [0u8; 1024];
                let mut len = 0;
                let tag = c"makepad-host";
                let flush = |line: &mut [u8; 1024], len: &mut usize| {
                    line[*len] = 0;
                    for b in &mut line[..*len] {
                        if *b == 0 { *b = b' '; }
                    }
                    __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), line.as_ptr() as *const c_char);
                    *len = 0;
                };
                loop {
                    let n = read(rd, buf.as_mut_ptr(), buf.len());
                    if n <= 0 {
                        break;
                    }
                    for &b in &buf[..n as usize] {
                        if b == b'\n' {
                            flush(&mut line, &mut len);
                        } else {
                            line[len] = b;
                            len += 1;
                            if len == line.len() - 1 {
                                flush(&mut line, &mut len);
                            }
                        }
                    }
                }
            })
            .ok();
    }
}
