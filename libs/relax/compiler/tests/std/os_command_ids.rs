// CommandExt::uid and gid (std os/unix/process.rs): the child sets its group, then its user,
// after fork and before exec. Set to the parent's own ids they run unprivileged; a user id that
// is not ours fails the spawn with the child's setuid error (EPERM) unless we are root.
// Lines starting with "| " must equal real std's (agree.sh).
use std::io::ErrorKind;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

fn ids_of(c: &mut Command) -> (u32, u32) {
    let out = c.args(["-c", "id -u; id -g"]).stdout(Stdio::piped()).output().unwrap();
    assert!(out.status.success(), "{:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut it = text.split_whitespace().map(|x| x.parse::<u32>().unwrap());
    (it.next().unwrap(), it.next().unwrap())
}

#[test]
fn a01_own_ids() {
    let (uid, gid) = ids_of(&mut Command::new("sh"));
    let both = ids_of(Command::new("sh").uid(uid).gid(gid));
    println!("| uid and gid: same {}", both == (uid, gid));
    let user = ids_of(Command::new("sh").uid(uid));
    println!("| uid alone: same {}", user == (uid, gid));
    let group = ids_of(Command::new("sh").gid(gid));
    println!("| gid alone: same {}", group == (uid, gid));
    let st = Command::new("sh").args(["-c", "exit 3"]).uid(uid).gid(gid).status().unwrap();
    println!("| status {:?}", st.code());
}

#[test]
fn a02_foreign_uid() {
    let (uid, _) = ids_of(&mut Command::new("sh"));
    if uid == 0 {
        println!("| foreign uid: root may take any");
        return;
    }
    let other = if uid == 1 { 2 } else { 1 };
    match Command::new("sh").args(["-c", "exit 0"]).uid(other).spawn() {
        Ok(mut child) => println!("| foreign uid: spawned {:?}", child.wait().map(|s| s.code())),
        Err(e) => println!("| foreign uid: {:?} os error {}", e.kind(), e.raw_os_error() == Some(1)),
    }
    let e = Command::new("sh").uid(other).output().unwrap_err();
    println!("| foreign uid output: {}", e.kind() == ErrorKind::PermissionDenied);
}
