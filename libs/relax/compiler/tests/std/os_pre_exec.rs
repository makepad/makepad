// std::os::unix::process::CommandExt::pre_exec: the closure runs in the child before exec; its
// error aborts the spawn and comes back from spawn() (EINVAL when it has no OS code).
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

#[test]
fn a01_pre_exec_runs_in_child() {
    let dir = std::env::temp_dir().join(format!("relax-pre-exec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let canon = std::fs::canonicalize(&dir).unwrap();
    let d2 = canon.clone();
    let out = unsafe {
        Command::new("pwd").stdout(Stdio::piped()).pre_exec(move || std::env::set_current_dir(&d2)).output().unwrap()
    };
    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
    println!("| child cwd matches {} status {}", got == canon.to_str().unwrap(), out.status.success());
    println!("| parent cwd unchanged {}", std::env::current_dir().unwrap() != canon);
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn a02_pre_exec_errors() {
    let r = unsafe { Command::new("true").pre_exec(|| Err(std::io::Error::from_raw_os_error(2))).spawn() };
    println!("| os error {:?}", r.map(|_| ()).map_err(|e| (e.kind(), e.raw_os_error())));
    let r = unsafe { Command::new("true").pre_exec(|| Err(std::io::Error::new(std::io::ErrorKind::Other, "custom"))).spawn() };
    println!("| custom error {:?}", r.map(|_| ()).map_err(|e| (e.kind(), e.raw_os_error())));
    let mut n = 0;
    let st = unsafe {
        Command::new("sh").arg("-c").arg("exit 3").pre_exec(move || { n += 1; Ok(()) }).pre_exec(|| Ok(())).status().unwrap()
    };
    println!("| two closures status {:?}", st.code());
    let r = unsafe { Command::new("/nonexistent/program").pre_exec(|| Ok(())).spawn() };
    println!("| exec failure {:?}", r.map(|_| ()).map_err(|e| e.kind()));
}
