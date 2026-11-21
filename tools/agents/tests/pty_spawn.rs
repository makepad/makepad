#![cfg(target_os = "macos")]
mod tests {
    use makepad_agents::pty_spawn::spawn;
    use std::{
        ffi::{c_char, c_void},
        io,
        process::Command,
        ptr,
    };
    fn checked(result: i32) -> io::Result<()> {
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(result))
        }
    }
    use std::{
        fs::File,
        os::fd::{AsRawFd, FromRawFd},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Barrier,
        },
        time::{Duration, Instant},
    };

    static FORKS: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn before_fork() {
        FORKS.fetch_add(1, Ordering::Relaxed);
    }
    extern "C" {
        fn pthread_atfork(
            prepare: Option<extern "C" fn()>,
            parent: Option<extern "C" fn()>,
            child: Option<extern "C" fn()>,
        ) -> i32;
        fn openpty(
            master: *mut i32,
            slave: *mut i32,
            name: *mut c_char,
            termios: *const c_void,
            size: *const c_void,
        ) -> i32;
        fn getsid(pid: i32) -> i32;
        fn tcgetpgrp(fd: i32) -> i32;
    }

    #[test]
    fn twenty_pty_spawns_do_not_fork_or_stall_allocator() {
        checked(unsafe { pthread_atfork(Some(before_fork), None, None) }).unwrap();
        let fork_count = FORKS.load(Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(Barrier::new(2));
        let allocator = {
            let stop = stop.clone();
            let ready = ready.clone();
            std::thread::spawn(move || {
                let mut maximum = Duration::ZERO;
                let mut samples = 0u64;
                ready.wait();
                while !stop.load(Ordering::Relaxed) {
                    let start = Instant::now();
                    let mut bytes = Vec::<u8>::with_capacity(4096);
                    bytes.resize(4096, 0x5a);
                    std::hint::black_box(&mut bytes);
                    drop(bytes);
                    maximum = maximum.max(start.elapsed());
                    samples += 1;
                }
                (maximum, samples)
            })
        };
        ready.wait();
        // Ensure the allocator thread is always joined even if a spawn fails.
        let result = (|| -> io::Result<()> {
            for _ in 0..20 {
                let (mut master, mut slave) = (-1, -1);
                if unsafe {
                    openpty(
                        &mut master,
                        &mut slave,
                        ptr::null_mut(),
                        ptr::null(),
                        ptr::null(),
                    )
                } != 0
                {
                    return Err(io::Error::last_os_error());
                }
                let master = unsafe { File::from_raw_fd(master) };
                let slave = unsafe { File::from_raw_fd(slave) };
                // A changed PATH + bare program is deliberately covered: std
                // Command would fall back to fork for this configuration.
                let mut command = Command::new("cat");
                command
                    .env("PATH", "/bin:/usr/bin")
                    .current_dir("/private/tmp");
                let mut child = spawn(
                    &command,
                    slave.as_raw_fd(),
                    std::path::Path::new(env!("CARGO_BIN_EXE_agents")),
                )?;
                let sid = unsafe { getsid(child.id() as i32) };
                let foreground = unsafe { tcgetpgrp(master.as_raw_fd()) };
                let killed = child.kill();
                // the child ends at its SIGKILL: waited for, it ends the spawns (and the sampler
                // with them) as an error, never a wait without end (under the JIT on a Mac a
                // child outlived it, and the test sampled until its count overflowed, 11-21)
                let killed_at = Instant::now();
                let waited = loop {
                    match child.try_wait() {
                        Ok(None) if killed_at.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(1)),
                        Ok(None) => break Err(io::Error::other(format!("child {} still runs 10 s after its SIGKILL (kill said {killed:?})", child.id()))),
                        other => break other.map(|_| ()),
                    }
                };
                waited?;
                killed?;
                if sid != child.id() as i32 || foreground != child.id() as i32 {
                    return Err(io::Error::other(format!(
                        "PTY ownership: session={sid}, foreground={foreground}, child={}",
                        child.id()
                    )));
                }
            }
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        let (maximum, samples) = allocator.join().unwrap();
        result.unwrap();
        let forks = FORKS.load(Ordering::Relaxed) - fork_count;
        println!("record=pty_spawn children=20 forks={forks} allocator_samples={samples} allocator_max_ms={:.6}", maximum.as_secs_f64() * 1000.0);
        assert_eq!(forks, 0, "the parent must never call fork");
        assert!(samples > 0);
        assert!(
            maximum <= Duration::from_millis(2),
            "allocator stalled for {maximum:?}"
        );
    }
}
