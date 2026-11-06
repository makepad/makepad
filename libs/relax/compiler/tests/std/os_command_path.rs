// A Command's own PATH finds a bare program name (real std sys/process/unix/unix.rs: no
// posix_spawn when PATH was set, removed or cleared and the program has no '/'; fork, the
// child's environment becomes `environ`, execvp). A stub named like a system program, in a
// folder only the Command's PATH holds, is what runs: spawned, with a pre_exec closure, and
// exec'd. A failed exec leaves this process's PATH as it was. "| " lines equal real std's.
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
}

fn stub_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("relax-command-path-{}-{}", tag, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("ls");
    std::fs::write(&p, "#!/bin/sh\necho stub ls \"$@\"\nexit 7\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn clean(dir: &std::path::Path) {
    std::fs::remove_file(dir.join("ls")).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

fn said(c: &mut Command) -> String {
    match c.stdout(Stdio::piped()).output() {
        Ok(o) => format!("{:?} {:?}", String::from_utf8_lossy(&o.stdout).trim(), o.status.code()),
        Err(e) => format!("error {:?}", e.kind()),
    }
}

#[test]
fn a01_spawn_searches_the_commands_path() {
    let dir = stub_dir("spawn");
    let ahead = std::env::join_paths([dir.clone()].into_iter().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
    println!("| own PATH: {}", said(Command::new("ls").arg("x").env("PATH", &dir)));
    println!("| stub first in PATH: {}", said(Command::new("ls").arg("y").env("PATH", &ahead)));
    println!("| cleared, then PATH: {}", said(Command::new("ls").env_clear().env("PATH", &dir)));
    println!("| PATH holds no such program: {}", said(Command::new("ls").env("PATH", "/nonexistent-relax-dir")));
    println!("| a path is no search: {}", said(Command::new(dir.join("ls")).arg("z").env("PATH", "/nonexistent-relax-dir")));
    clean(&dir);
}

#[test]
fn a02_pre_exec_searches_the_commands_path() {
    let dir = stub_dir("pre-exec");
    let mut c = Command::new("ls");
    c.arg("p").env("PATH", &dir);
    unsafe { c.pre_exec(|| Ok(())) };
    println!("| pre_exec: {}", said(&mut c));
    clean(&dir);
}

#[test]
fn a03_exec_searches_the_commands_path() {
    let dir = stub_dir("exec");
    let pid = unsafe { fork() };
    if pid == 0 {
        let _ = Command::new("ls").env("PATH", &dir).exec();
        unsafe { _exit(99) };
    }
    let mut st = 0;
    unsafe { waitpid(pid, &mut st, 0) };
    println!("| exec exit {}", (st >> 8) & 0xff);
    let path = std::env::var_os("PATH");
    let e = Command::new("ls").env("PATH", "/nonexistent-relax-dir").exec();
    println!("| exec of no such program: {:?}, this PATH kept {}", e.kind(), std::env::var_os("PATH") == path);
    clean(&dir);
}
