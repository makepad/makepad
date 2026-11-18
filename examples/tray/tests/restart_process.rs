//! The app's menu-bar status item works again after the app restarts as a new process (macOS). An
//! exec in place kept a pid the window server had marked as exiting, and the item came back blank
//! while AppKit reported it healthy (relax's restart_parent.rs). So beside the selftest's own lines
//! the system's log may not mark a living pid as exiting: neither the restarted app's nor the run's
//! (the first app's is, as it ends: that is every app's end).
//!
//! The app runs under the JIT with its tray selftest (`MAKEPAD_TRAY_SELFTEST=stay`: the item's
//! button frame and its first menu item, then it goes on), restarts by `session relaunch
//! process=1`, and comes back as a new process behind the same run pid. A logged-in session's
//! window server is needed: on the Metal backend asked for by name a hidden window keeps its real
//! AppKit objects (hal types.rs `OpenReq::from_env`), so the window stays off the display while
//! the status item is real.

#[cfg(all(relax, target_os = "macos"))]
#[test]
fn the_status_item_works_again_after_a_restart_as_a_new_process() {
    use std::os::unix::process::ExitStatusExt;
    use std::time::{Duration, Instant};
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let loader = std::env::var_os("RELAX_LOADER").expect("Relax names its test loader");
    let dir = std::env::temp_dir().join(format!("tray-restart-process-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("app.log");
    let started = Instant::now();
    // a run that can restart (a live one, as `run` is by default): behind its parent
    let out = std::fs::File::create(&log).unwrap();
    let mut run = std::process::Command::new(&loader)
        .args(["run", "makepad-example-tray"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("MAKEPAD_TRAY_SELFTEST", "stay")
        .env("MAKEPAD_HAL_BACKEND", "metal")
        .env("MAKEPAD_HIDE_WINDOWS", "1")
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    let text = || std::fs::read_to_string(&log).unwrap_or_default();
    // the first line from `from` on that `found` picks out, waited for while the run lives
    let wait = |run: &mut std::process::Child, from: usize, what: &str, found: &dyn Fn(&str) -> bool, secs: u64| -> (usize, String) {
        let until = Instant::now() + Duration::from_secs(secs);
        loop {
            let ended = run.try_wait().unwrap();
            let all = text();
            let mut at = from.min(all.len());
            for line in all[at..].split_inclusive('\n') {
                at += line.len();
                if line.ends_with('\n') && found(line) {
                    return (at, line.trim_end().to_string());
                }
            }
            assert!(ended.is_none(), "the run ended ({ended:?}) with no {what}:\n{all}");
            assert!(Instant::now() < until, "no {what} in {secs} s:\n{all}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // `makepad-remote: pid <app pid> app <package> run <run pid> socket ..`
    let announce = |line: &str| -> Option<(i32, i32)> {
        let w: Vec<&str> = line.split("-remote: pid ").nth(1)?.split_whitespace().collect();
        if w.get(1) != Some(&"app") || w.get(3) != Some(&"run") {
            return None;
        }
        Some((w[0].parse().ok()?, w[4].parse().ok()?))
    };
    let (at, line) = wait(&mut run, 0, "announce", &|l| announce(l).is_some(), 600);
    let (first, run_pid) = announce(&line).unwrap();
    assert_eq!(run_pid, run.id() as i32, "the run pid is the process the launcher started: {line}");
    assert_ne!(first, run_pid, "the app is the run's child");
    let (at, _) = wait(&mut run, at, "status item", &|l| l.contains("tray selftest: status item button frame"), 120);
    let (at, _) = wait(&mut run, at, "menu action", &|l| l.contains("tray selftest: TrayMenu") && l.contains("staying"), 120);
    let asked = std::process::Command::new(&loader).args(["inspect", &first.to_string(), "session", "relaunch", "process=1"]).output().unwrap();
    assert!(asked.status.success(), "relaunch: {}{}", String::from_utf8_lossy(&asked.stdout), String::from_utf8_lossy(&asked.stderr));
    let (at, line) = wait(&mut run, at, "the restarted app's announce", &|l| announce(l).is_some_and(|(p, _)| p != first), 300);
    let (second, again) = announce(&line).unwrap();
    assert_eq!(again, run_pid, "the run pid stays: {line}");
    let (at, _) = wait(&mut run, at, "the restarted app's status item", &|l| l.contains("tray selftest: status item button frame"), 120);
    wait(&mut run, at, "the restarted app's menu action", &|l| l.contains("tray selftest: TrayMenu") && l.contains("staying"), 120);
    assert_eq!(unsafe { kill(first, 0) }, -1, "the first app process is gone");
    // the system's view: the window server and the menu bar's agent on either pid
    let minutes = started.elapsed().as_secs() / 60 + 1;
    let scan = std::process::Command::new("/usr/bin/log")
        .args(["show", "--last", &format!("{minutes}m"), "--style", "compact", "--predicate", "eventMessage CONTAINS \"pending exit\" OR eventMessage CONTAINS \"Enqueueing a cancel\""])
        .output()
        .unwrap();
    let scan = String::from_utf8_lossy(&scan.stdout);
    let living = [second.to_string(), run_pid.to_string()];
    let named: Vec<&str> = scan.lines().filter(|l| l.split(|c: char| !c.is_ascii_digit()).any(|w| living.iter().any(|p| p == w))).collect();
    // the run ends: its parent passes the SIGTERM on, and ends as the app does
    unsafe { kill(run_pid, 15) };
    let ended = run.wait().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(named.is_empty(), "the system marked a living process of the run as exiting:\n{}", named.join("\n"));
    assert!(ended.code().is_some() || ended.signal() == Some(15), "the run ended as its app did: {ended:?}");
}
