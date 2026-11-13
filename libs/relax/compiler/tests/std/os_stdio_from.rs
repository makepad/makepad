// std::process::Stdio from this process's own streams and from a File (relax issue 110): a child's
// stream given `io::stdout()` goes where this process's stdout goes, `io::stderr()` where its
// stderr goes, a File into the file. While the children run, this process's stdout and stderr are
// two pipes (`captured`), so each line is told by where it arrived.
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};

extern "C" {
    fn dup(fd: i32) -> i32;
    fn dup2(fd: i32, to: i32) -> i32;
    fn close(fd: i32) -> i32;
}

/// Runs `f` with this process's stdout and stderr each a pipe: the `child` lines that reached
/// them, and what `f` returned.
fn captured<R>(f: impl FnOnce() -> R) -> (String, String, R) {
    std::io::stdout().flush().unwrap();
    std::io::stderr().flush().unwrap();
    let (mut out_r, out_w) = std::io::pipe().unwrap();
    let (mut err_r, err_w) = std::io::pipe().unwrap();
    let saved = unsafe { (dup(1), dup(2)) };
    unsafe {
        dup2(out_w.as_raw_fd(), 1);
        dup2(err_w.as_raw_fd(), 2);
    }
    drop((out_w, err_w));
    let r = f();
    unsafe {
        dup2(saved.0, 1);
        dup2(saved.1, 2);
        close(saved.0);
        close(saved.1);
    }
    let lines = |r: &mut std::io::PipeReader| {
        let mut s = String::new();
        r.read_to_string(&mut s).unwrap();
        s.lines().filter(|l| l.starts_with("child")).collect::<Vec<_>>().join("/")
    };
    (lines(&mut out_r), lines(&mut err_r), r)
}

fn both() -> Command {
    let mut c = Command::new("sh");
    c.args(["-c", "echo child-out; echo child-err >&2"]);
    c
}

#[test]
fn a01_stdout_and_stderr_into_stdio() {
    let (out, err, ok) = captured(|| both().stdout(std::io::stderr()).stderr(std::io::stdout()).status().unwrap().success());
    println!("| swapped: stdout [{out}] stderr [{err}] {ok}");
    let (out, err, ok) = captured(|| {
        let stderr: Stdio = std::io::stderr().into();
        let stdout: Stdio = Stdio::from(std::io::stdout());
        both().stdout(stdout).stderr(stderr).status().unwrap().success()
    });
    println!("| own: stdout [{out}] stderr [{err}] {ok}");
    let (out, err, output) = captured(|| both().stderr(std::io::stdout()).output().unwrap());
    println!("| output: stdout [{out}] stderr [{err}] piped {:?} {:?}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
fn a02_file_into_stdio() {
    let path = std::env::temp_dir().join(format!("stdio-from-file-{}", std::process::id()));
    let file = std::fs::File::create(&path).unwrap();
    let (out, err, ok) = captured(|| both().stderr(Stdio::from(file.try_clone().unwrap())).status().unwrap().success());
    println!("| file: stdout [{out}] stderr [{err}] {ok}");
    let mut read = std::fs::File::open(&path).unwrap();
    let mut text = String::new();
    read.read_to_string(&mut text).unwrap();
    println!("| file holds {text:?}");
    let mut file = file;
    file.seek(std::io::SeekFrom::End(0)).unwrap();
    let (out, err, ok) = captured(|| both().stdout(file).stderr(Stdio::null()).status().unwrap().success());
    text.clear();
    read.rewind().unwrap();
    read.read_to_string(&mut text).unwrap();
    println!("| file again: stdout [{out}] stderr [{err}] {ok} holds {text:?}");
    std::fs::remove_file(&path).unwrap();
}
