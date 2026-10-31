// A thread spawned without Builder::stack_size gets RUST_MIN_STACK bytes when the variable is set (else 2 MiB),
// as real std (library/std/src/thread/lifecycle.rs): the sizes are checked as lower bounds. "| " lines must equal real std's (core_diff.sh); run once
// as is and once with RUST_MIN_STACK=8388608 in the environment.
#[cfg(target_os = "macos")]
fn stack_bytes() -> usize {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_get_stacksize_np(t: usize) -> usize;
    }
    unsafe { pthread_get_stacksize_np(pthread_self()) }
}

#[cfg(target_os = "linux")]
fn stack_bytes() -> usize {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(t: usize, attr: *mut u64) -> i32;
        fn pthread_attr_getstacksize(attr: *const u64, size: *mut usize) -> i32;
        fn pthread_attr_destroy(attr: *mut u64) -> i32;
    }
    // pthread_attr_t is 56 bytes on x86_64 / aarch64 glibc
    let mut attr = [0u64; 8];
    let mut size = 0usize;
    unsafe {
        pthread_getattr_np(pthread_self(), attr.as_mut_ptr());
        pthread_attr_getstacksize(attr.as_ptr(), &mut size);
        pthread_attr_destroy(attr.as_mut_ptr());
    }
    size
}

fn mib_of_this_thread() -> usize {
    (stack_bytes() + (1 << 19)) >> 20
}

#[test]
fn a01_default_and_explicit() {
    let env = std::env::var("RUST_MIN_STACK").ok();
    let want = env.as_deref().and_then(|v| v.parse::<usize>().ok()).map_or(2, |b| b >> 20);
    let explicit = std::thread::Builder::new().stack_size(3 << 20).spawn(mib_of_this_thread).unwrap().join().unwrap();
    let default = std::thread::spawn(mib_of_this_thread).join().unwrap();
    let scoped = std::thread::scope(|s| s.spawn(mib_of_this_thread).join().unwrap());
    // at least: glibc may hand a new thread a cached larger stack of an exited one
    println!("| RUST_MIN_STACK {} -> default >= {} MiB: {}, scoped: {}, explicit >= 3 MiB: {}",
        env.as_deref().unwrap_or("unset"), want, default >= want, scoped >= want, explicit >= 3);
}
