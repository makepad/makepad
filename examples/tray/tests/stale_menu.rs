//! A status item's menu that an update replaced while it was open still answers a click (macOS).
//! An app that updates its menu while the person reads it (devhub's master: its fleet at a glance,
//! every few seconds) released the open menu's item targets with it, and the click went nowhere.
//!
//! The app runs under the JIT with its tray selftest (`MAKEPAD_TRAY_SELFTEST=stale`): the shell
//! clicks the first menu item, the example answers with a changed menu, and the shell clicks the
//! first item of the menu that was replaced; the example quits on that second `TrayMenu`. A
//! logged-in session's window server is needed: on the Metal backend asked for by name a hidden
//! window keeps its real AppKit objects (hal types.rs `OpenReq::from_env`), so the status item is
//! real while no window shows.

#[cfg(all(relax, target_os = "macos"))]
#[test]
fn a_click_in_a_menu_an_update_replaced_still_arrives() {
    use std::time::{Duration, Instant};
    let loader = std::env::var_os("RELAX_LOADER").expect("Relax names its test loader");
    let dir = std::env::temp_dir().join(format!("tray-stale-menu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("app.log");
    let out = std::fs::File::create(&log).unwrap();
    let mut run = std::process::Command::new(&loader)
        .args(["run", "makepad-example-tray"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("MAKEPAD_TRAY_SELFTEST", "stale")
        .env("MAKEPAD_HAL_BACKEND", "metal")
        .env("MAKEPAD_HIDE_WINDOWS", "1")
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    let text = || std::fs::read_to_string(&log).unwrap_or_default();
    // the run ends once the replaced menu's click arrives; a click that goes nowhere leaves the
    // example waiting for it, and the run is ended a while after the click was made
    let mut clicked: Option<Instant> = None;
    let ended = loop {
        if let Some(status) = run.try_wait().unwrap() {
            break Some(status);
        }
        if clicked.is_none() && text().contains("of the replaced menu") {
            clicked = Some(Instant::now());
        }
        if clicked.is_some_and(|at| at.elapsed() > Duration::from_secs(30)) {
            let _ = run.kill();
            let _ = run.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let all = text();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(all.contains("tray selftest: performing menu item 0"), "the first click:\n{all}");
    assert!(all.contains("received, the menu changes"), "the first click's TrayMenu, and the menu replaced:\n{all}");
    assert!(all.contains("tray selftest: performing menu item 0 of the replaced menu"), "the click in the replaced menu:\n{all}");
    assert!(ended.is_some() && all.contains("from the replaced menu received"), "the replaced menu's click arrives as a TrayMenu ({ended:?}):\n{all}");
}
