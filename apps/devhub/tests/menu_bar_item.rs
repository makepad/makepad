//! A choice in the menu-bar item's menu reaches the master as the item its action runs on (macOS).
//! The item is the helper's (`makepad-app-devhub --menu-bar-item`, menu_item.rs): the master
//! writes it states on its stdin and hears the items chosen on its stdout. The master updates the
//! menu every few seconds, so a person's click often lands in a menu an update replaced while it
//! was open: those clicks went nowhere (11-17, "open panel / open devhub nothing does anything").
//!
//! The helper runs under the JIT with the tray selftest (`MAKEPAD_TRAY_SELFTEST=stale`): the
//! shell chooses the first enabled item of the new menu, and after the next state, the first
//! enabled item of the menu that state replaced. Both answers name the master's "Open panel".
//! A logged-in session's window server is needed: on the Metal backend asked for by name a hidden
//! window keeps its real AppKit objects (hal types.rs `OpenReq::from_env`), so the status item is
//! real while no window shows. Its own home: the user's helper holds the user's lock.

#[cfg(all(relax, target_os = "macos"))]
#[test]
fn a_choice_in_the_helpers_menu_reaches_the_master_also_from_a_replaced_menu() {
    use makepad_widgets::*;
    use std::io::{BufRead, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;
    let loader = std::env::var_os("RELAX_LOADER").expect("Relax names its test loader");
    let dir = std::env::temp_dir().join(format!("devhub-menu-bar-item-{}", std::process::id()));
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".makepad/devhub")).unwrap();
    let log = dir.join("helper.log");
    let mut run = std::process::Command::new(&loader)
        .args(["run", "makepad-app-devhub", "--", "--menu-bar-item"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("HOME", &home)
        .env("MAKEPAD_HOME", home.join(".makepad"))
        .env("MAKEPAD_TRAY_SELFTEST", "stale")
        .env("MAKEPAD_HAL_BACKEND", "metal")
        .env("MAKEPAD_HIDE_WINDOWS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let (tx, answers) = mpsc::channel::<String>();
    let out = run.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if line.starts_with("{\"item\"") && tx.send(line).is_err() {
                break;
            }
        }
    });
    // menu_item.rs's state line: a glyph, a status line, then the master's items
    let state = |line: &str| {
        let alpha = "ff".repeat(16 * 16);
        let item = |id: LiveId, label: &str, enabled: bool| format!("{{\"id\":\"{}\",\"label\":\"{label}\",\"enabled\":{enabled},\"checked\":null}}", id.0);
        format!("{{\"w\":16,\"h\":16,\"alpha\":\"{alpha}\",\"tooltip\":\"\",\"dropdown\":false,\"menu\":[{},{{\"line\":true}},{},{}]}}\n",
            item(LiveId(11), line, false), item(live_id!(open_panel), "Open panel", true), item(live_id!(open_window), "Open devhub", true))
    };
    let mut to = run.stdin.take().unwrap();
    let answer = |what: &str, secs: u64, run: &mut std::process::Child| -> String {
        answers.recv_timeout(Duration::from_secs(secs)).unwrap_or_else(|e| {
            let _ = run.kill();
            let _ = run.wait();
            panic!("no answer to {what} ({e}):\n{}", std::fs::read_to_string(&log).unwrap_or_default())
        })
    };
    let panel = format!("{{\"item\":\"{}\"}}", live_id!(open_panel).0);
    to.write_all(state("Land: empty").as_bytes()).unwrap();
    // (the first state waits for the helper's build and start)
    assert_eq!(answer("the choice in the new menu", 600, &mut run), panel);
    to.write_all(state("Land: 1 waiting").as_bytes()).unwrap();
    assert_eq!(answer("the choice in the replaced menu", 60, &mut run), panel);
    // the master's end ends its helper
    drop(to);
    let status = run.wait().unwrap();
    let all = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(all.contains("of the replaced menu"), "the second choice came from the replaced menu:\n{all}");
    assert!(status.success(), "the helper ends with its input ({status}):\n{all}");
}
