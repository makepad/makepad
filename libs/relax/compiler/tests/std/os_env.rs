// std-os lane: std::env and std::process (differential: os_diff.sh)
use std::env;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn a01_env() {
    env::set_var("HR_OS_TEST_A", "value 1");
    println!("| {:?} {:?}", env::var("HR_OS_TEST_A"), env::var_os("HR_OS_TEST_A"));
    env::remove_var("HR_OS_TEST_A");
    println!("| {:?} {:?}", env::var("HR_OS_TEST_A"), env::var_os("HR_OS_TEST_A"));
    match env::var("HR_OS_TEST_A") {
        Err(e) => println!("| {}", e),
        Ok(_) => {}
    }
    env::set_var("HR_OS_TEST_B", "x=y");
    let found: Vec<(String, String)> = env::vars().filter(|(k, _)| k.starts_with("HR_OS_TEST_")).collect();
    println!("| {:?}", found);
    println!("| {}", env::vars_os().count() > 3);
    let cwd = env::current_dir().unwrap();
    println!("| {}", cwd.is_absolute());
    let tmp = env::temp_dir();
    println!("| {}", tmp.is_absolute());
    println!("| {}", env::current_exe().unwrap().is_absolute());
    println!("| {}", env::args().count() >= 1);
    let paths: Vec<_> = env::split_paths("/a:/b::c").collect();
    println!("| {:?} {:?}", paths, env::join_paths(["/x", "y"].iter()));
    println!("| {:?}", env::join_paths(["a:b"].iter()).map_err(|e| e.to_string()));
    println!("| {} {} {} {}", env::consts::OS, env::consts::ARCH, env::consts::FAMILY, env::consts::DLL_SUFFIX);
    let d = env::temp_dir();
    env::set_current_dir(&d).unwrap();
    println!("| {}", env::current_dir().unwrap() == std::fs::canonicalize(&d).unwrap());
    env::set_current_dir(&cwd).unwrap();
}

#[test]
fn a02_process() {
    println!("| {}", std::process::id() > 0);
    let out = Command::new("/bin/echo").arg("hello").args(["a", "b"]).output().unwrap();
    println!("| {:?} {:?} {:?} {}", out.status.code(), String::from_utf8_lossy(&out.stdout), out.stderr, out.status);
    let out = Command::new("sh").arg("-c").arg("echo out; echo err 1>&2; exit 3").output().unwrap();
    println!("| {:?} {:?} {:?} {} {}", out.status.code(), out.stdout, out.stderr, out.status.success(), out.status);
    let out = Command::new("sh").args(["-c", "echo $HR_X-$HOME_UNSET; pwd"]).env("HR_X", "set").env_remove("HOME_UNSET").current_dir("/").output().unwrap();
    println!("| {:?}", String::from_utf8_lossy(&out.stdout));
    let out = Command::new("sh").args(["-c", "env"]).env_clear().env("ONLY", "1").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    println!("| {}", text.contains("ONLY=1"));
    let mut child = Command::new("cat").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"piped through cat").unwrap();
    let out = child.wait_with_output().unwrap();
    println!("| {:?} {}", String::from_utf8_lossy(&out.stdout), out.status);
    let st = Command::new("sh").args(["-c", "exit 7"]).stdout(Stdio::null()).status().unwrap();
    println!("| {:?} {}", st.code(), st);
    let mut c = Command::new("sleep").arg("5").spawn().unwrap();
    println!("| {:?}", c.try_wait().unwrap().is_none());
    c.kill().unwrap();
    let st = c.wait().unwrap();
    println!("| {:?} {}", st.code(), st);
    let e = Command::new("/nonexistent/binary").output().unwrap_err();
    println!("| {:?} {}", e.kind(), e);
    let big = Command::new("sh").args(["-c", "i=0; while [ $i -lt 2000 ]; do echo line-$i; echo eline-$i 1>&2; i=$((i+1)); done"]).output().unwrap();
    println!("| {} {}", big.stdout.len(), big.stderr.len());
}
