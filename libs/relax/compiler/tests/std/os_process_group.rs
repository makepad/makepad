// CommandExt::process_group (std os/unix/process.rs): the child leads a new process group
// (pgroup 0) or joins the given one; without it the child stays in the parent's group.
// Lines starting with "| " must equal real std's (os_diff.sh / core_diff.sh).
use std::os::unix::process::CommandExt;
use std::process::Command;

fn child_pid_pgid(group: Option<i32>) -> (u32, u32) {
    let mut c = Command::new("sh");
    c.args(["-c", "ps -o pgid= -p $$"]);
    if let Some(g) = group {
        c.process_group(g);
    }
    let child = c.stdout(std::process::Stdio::piped()).spawn().unwrap();
    let pid = child.id();
    let out = child.wait_with_output().unwrap();
    let pgid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    (pid, pgid)
}

#[test]
fn a01_process_group() {
    let (pid, pgid) = child_pid_pgid(Some(0));
    println!("| new group: leader {}", pid == pgid);
    let (pid, pgid) = child_pid_pgid(None);
    let ours = Command::new("sh").args(["-c", "ps -o pgid= -p $PPID"]).output().unwrap();
    let ours: u32 = String::from_utf8_lossy(&ours.stdout).trim().parse().unwrap();
    println!("| inherited: leader {} same as parent {}", pid == pgid, pgid == ours);
    let st = Command::new("sh").args(["-c", "exit 4"]).process_group(0).status().unwrap();
    println!("| status {:?}", st.code());
}
