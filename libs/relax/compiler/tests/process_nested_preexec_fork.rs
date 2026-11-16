#![cfg(unix)]

// A fork inside a pre_exec child hung under the JIT: its compiler pause waited
// for workers the child does not have.
use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(status: i32) -> !;
}

#[test]
fn a_pre_exec_closure_forks_waits_and_execs() {
    let status = unsafe {
        Command::new("/bin/sh").args(["-c", ":"]).pre_exec(|| Ok(())).status()
    }.expect("the plain pre_exec spawn failed");
    assert!(status.success(), "the plain pre_exec spawn exited unsuccessfully: {status}");

    let status = unsafe {
        Command::new("/bin/sh").args(["-c", ":"]).pre_exec(|| {
            let pid = fork();
            if pid < 0 {
                return Err(io::Error::last_os_error());
            }
            if pid == 0 {
                _exit(0);
            }
            let mut status = 0;
            if waitpid(pid, &mut status, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            if status != 0 {
                return Err(io::ErrorKind::Other.into());
            }
            Ok(())
        }).status()
    }.expect("the pre_exec that forks and waits failed");
    assert!(status.success(), "the pre_exec that forks and waits exited unsuccessfully: {status}");
}
