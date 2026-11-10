// std::os::unix::process::CommandExt::uid and gid: the child calls setgid, then setuid, before it
// execs, and a failure of either comes back from spawn(). tokio's `process::Command::uid` /
// `gid` (src/process/mod.rs) forward to them and did not compile ("no method named `uid`").
// A test cannot switch to another user without being root, so it asks for the ids it already
// has (always allowed) and, when it is not root, for root's (refused: EPERM).
use std::io::ErrorKind;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
}

fn id_of(flag: &str) -> u32 {
    let out = Command::new("id").arg(flag).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

/// What `sh -c <script>` prints as a child set up by `ids`.
fn child_says(script: &str, ids: impl FnOnce(&mut Command)) -> String {
    let mut c = Command::new("sh");
    c.args(["-c", script]).stdout(Stdio::piped());
    ids(&mut c);
    let out = c.output().unwrap();
    assert!(out.status.success(), "{:?}", out);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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
fn a01_child_runs_with_the_ids_asked_for() {
    let (uid, gid) = (id_of("-u"), id_of("-g"));
    assert_eq!(child_says("id -u", |c| { c.uid(uid); }), uid.to_string());
    assert_eq!(child_says("id -g", |c| { c.gid(gid); }), gid.to_string());
    assert_eq!(child_says("echo $(id -u):$(id -g)", |c| { c.uid(uid).gid(gid); }), format!("{uid}:{gid}"));
    // the builder chains like the other setters, in either order, with the rest of the set-up
    let dir = std::env::temp_dir().canonicalize().unwrap();
    let said = child_says("echo $(id -u) $X $(pwd -P)", |c| { c.gid(gid).env("X", "x").uid(uid).current_dir(&dir).process_group(0); });
    assert_eq!(said, format!("{uid} x {}", dir.display()));
    // status() and spawn() take the same path as output()
    assert_eq!(Command::new("sh").args(["-c", "exit 5"]).uid(uid).gid(gid).status().unwrap().code(), Some(5));
    let mut child = Command::new("true").uid(uid).spawn().unwrap();
    assert!(child.wait().unwrap().success());
    // the parent keeps its own ids
    assert_eq!((id_of("-u"), id_of("-g")), (uid, gid));
}

#[test]
fn a02_refused_ids_fail_the_spawn() {
    let (uid, gid) = (id_of("-u"), id_of("-g"));
    if uid == 0 {
        println!("| running as root: every id is allowed, nothing to refuse");
        return;
    }
    let e = Command::new("true").uid(0).spawn().map(|_| ()).unwrap_err();
    assert_eq!((e.kind(), e.raw_os_error()), (ErrorKind::PermissionDenied, Some(1)));
    let e = Command::new("true").uid(0).gid(gid).output().map(|_| ()).unwrap_err();
    assert_eq!((e.kind(), e.raw_os_error()), (ErrorKind::PermissionDenied, Some(1)));
    if gid != 0 {
        let e = Command::new("true").gid(0).status().map(|_| ()).unwrap_err();
        assert_eq!((e.kind(), e.raw_os_error()), (ErrorKind::PermissionDenied, Some(1)));
        // the group is set first: with both refused the program still never runs
        let e = Command::new("true").uid(0).gid(0).spawn().map(|_| ()).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(1));
    }
    // a refused child leaves nothing behind and the next spawn is unaffected
    assert_eq!(child_says("id -u", |c| { c.uid(uid); }), uid.to_string());
}

#[test]
fn a03_exec_sets_the_ids_in_this_process() {
    let (uid, gid) = (id_of("-u"), id_of("-g"));
    let code = in_child(|| Command::new("sh").args(["-c", &format!("test $(id -u) = {uid} && test $(id -g) = {gid} && exit 7")]).uid(uid).gid(gid).exec());
    assert_eq!(code, 7);
    if uid != 0 {
        // refused: exec returns EPERM and the program does not run
        assert_eq!(in_child(|| Command::new("sh").args(["-c", "exit 7"]).uid(0).exec()), 101);
    }
}
