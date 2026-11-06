// CommandExt::exec replaces the process (here a forked child): its exit status is the new
// program's; a failed exec returns the error. The exec sets the process up as a spawn sets a
// child up (real std do_exec): stdio redirected to a null device or a file, its folder, after
// a pre_exec closure. "| " lines must equal real std's.
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
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

/// The exit code of `exec` run in a forked child; 100 + errno when the exec failed.
fn in_child(exec: impl FnOnce() -> std::io::Error) -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        let e = exec();
        unsafe { _exit(100 + e.raw_os_error().unwrap_or(99)) };
    }
    let mut st = 0;
    unsafe { waitpid(pid, &mut st, 0) };
    (st >> 8) & 0xff
}

#[test]
fn a02_exec_sets_the_process_up() {
    let dir = std::env::temp_dir().join(format!("relax-exec-stdio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (out, input) = (dir.join("out"), dir.join("in"));
    let said = || std::fs::read_to_string(&out).unwrap().trim().to_string();
    let code = in_child(|| Command::new("sh").args(["-c", "echo out; exit 7"]).stdout(Stdio::null()).exec());
    println!("| stdout null: {code}");
    let code = in_child(|| Command::new("sh").args(["-c", "echo out; echo err >&2; exit 7"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).exec());
    println!("| all null: {code}");
    let code = in_child(|| Command::new("sh").args(["-c", "echo out; echo err >&2; exit 7"]).stdout(File::create(&out).unwrap()).stderr(Stdio::null()).exec());
    println!("| stdout to a file: {code} {:?}", said());
    std::fs::write(&input, "in\n").unwrap();
    let code = in_child(|| Command::new("sh").args(["-c", "read x; echo \"got $x\"; exit 7"]).stdin(File::open(&input).unwrap()).stdout(File::create(&out).unwrap()).exec());
    println!("| stdin from a file: {code} {:?}", said());
    let code = in_child(|| Command::new("sh").args(["-c", "pwd -P; exit 7"]).current_dir("/").stdout(File::create(&out).unwrap()).exec());
    println!("| folder: {code} {:?}", said());
    let code = in_child(|| {
        let mut c = Command::new("sh");
        c.args(["-c", "echo out; exit 7"]).stdout(Stdio::null());
        unsafe { c.pre_exec(|| Ok(())) };
        c.exec()
    });
    println!("| pre_exec, stdout null: {code}");
    std::fs::remove_file(&out).unwrap();
    std::fs::remove_file(&input).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}
