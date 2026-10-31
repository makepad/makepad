// std::io::pipe / PipeReader / PipeWriter (real std 1.87): read/write, EOF after the writer drops,
// close-on-exec, try_clone, fd conversions, and a child process's stdout through Stdio::from(writer).
use std::io::{pipe, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::process::{Command, Stdio};

#[test]
fn a01_basic() {
    let (mut r, mut w) = pipe().unwrap();
    w.write_all(b"hello pipe").unwrap();
    let w2 = w.try_clone().unwrap();
    drop(w);
    (&w2).write_all(b"!").unwrap();
    drop(w2);
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    println!("| read {:?}", s);
    let mut buf = [0u8; 4];
    println!("| eof {}", r.read(&mut buf).unwrap());
    let flags = unsafe { fcntl(r.as_raw_fd(), 1) };
    println!("| cloexec {}", flags & 1 == 1);
    println!("| debug {}", format!("{:?}", r).starts_with("PipeReader(FileDesc(OwnedFd { fd: "));
}

#[test]
fn a02_conversions() {
    let (r, w) = pipe().unwrap();
    let wfd: OwnedFd = w.into();
    let mut w = std::io::PipeWriter::from(wfd);
    w.write_all(b"xy").unwrap();
    let raw = w.into_raw_fd();
    let mut w = unsafe { std::io::PipeWriter::from_raw_fd(raw) };
    w.flush().unwrap();
    drop(w);
    let mut r = std::io::PipeReader::from(OwnedFd::from(r));
    let mut v = Vec::new();
    r.read_to_end(&mut v).unwrap();
    println!("| conv {:?}", v);
}

#[test]
fn a03_child_stdout() {
    let (mut r, w) = pipe().unwrap();
    let mut child = Command::new("echo").arg("from child").stdout(Stdio::from(w)).spawn().unwrap();
    child.wait().unwrap();
    drop(child);
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    println!("| child {:?}", s);
}

extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}
