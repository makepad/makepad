//! `impl Write for &ChildStdin`, as real std has it: a child's stdin written through a shared
//! reference (`writeln!(&stdin, ..)`, `(&stdin).write_all(..)`), which failed to compile under
//! the JIT with E0277 while rustc took it. Shared and owned writes go to the same pipe, and a
//! write after the child is gone keeps the pipe's error.
#![cfg(unix)]
use std::io::{ErrorKind, Write};
use std::process::{ChildStdin, Command, Stdio};

fn writes<W: Write>(mut w: W, s: &str) -> std::io::Result<()> {
    w.write_all(s.as_bytes())?;
    w.flush()
}

#[test]
fn shared_and_owned_writes_reach_the_child() {
    let mut child = Command::new("cat").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin: ChildStdin = child.stdin.take().unwrap();
    stdin.write_all(b"owned ").unwrap();
    (&stdin).write_all(b"shared ").unwrap();
    writeln!(&stdin, "fmt {}", 42).unwrap();
    writes(&stdin, "generic").unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "owned shared fmt 42\ngeneric", "the writes through &ChildStdin did not reach the pipe in order");
}

#[test]
fn a_shared_write_after_the_child_exits_is_a_broken_pipe() {
    let mut child = Command::new("sh").args(["-c", "exit 0"]).stdin(Stdio::piped()).spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!((&stdin).write(b"gone").unwrap_err().kind(), ErrorKind::BrokenPipe);
}
