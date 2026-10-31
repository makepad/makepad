// CommandExt::exec replaces the process (here a forked child): its exit status is the new
// program's; a failed exec returns the error. "| " lines must equal real std's.
use std::os::unix::process::CommandExt;
use std::process::Command;
extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
}

#[test]
fn a01_exec() {
    let pid = unsafe { fork() };
    if pid == 0 {
        let _ = Command::new("sh").args(["-c", "exit $X"]).env("X", "7").exec();
        unsafe { _exit(99) };
    }
    let mut st = 0;
    unsafe { waitpid(pid, &mut st, 0) };
    println!("| exec exit {}", (st >> 8) & 0xff);
    let e = Command::new("/nonexistent/prog").exec();
    println!("| missing {:?}", e.kind());
}
