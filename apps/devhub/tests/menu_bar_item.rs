//! devhub's one menu-bar item and its panel (macOS). The item is the helper's (`makepad-app-devhub
//! --menu-bar-item`, menu_item.rs): the master writes it states on its stdin and hears its answers
//! on its stdout. The helper shows the fleet panel's window too, so the panel is devhub's own and
//! never a second app with a second item (11-24).
//!
//! - A choice reaches the master as the item its action runs on. The master updates the menu every
//!   few seconds, so a person's click often lands in a menu an update replaced while it was open:
//!   those clicks went nowhere (11-17, "open panel / open devhub nothing does anything").
//! - Open panel opens the panel's window in the helper.
//! - A self-update to other code starts the next helper beside the old one: it opens the panel
//!   where the old one showed it and says it is ready, while the old one still shows the item. Only
//!   once the old one ended does the new one show the item: one item, and the panel stays.
//!
//! The helpers run under the JIT with the tray selftest (`MAKEPAD_TRAY_SELFTEST=stale`): the shell
//! chooses the first enabled item of a new menu, and after the next state, the first enabled item
//! of the menu that state replaced. A logged-in session's window server is needed: on the Metal
//! backend asked for by name a hidden window keeps its real AppKit objects (hal types.rs
//! `OpenReq::from_env`), so the status item is real while no window shows. Their own home: the
//! user's helper holds the user's lock. The panel shows its demo fleet (no master here).

#[cfg(all(relax, target_os = "macos"))]
#[test]
fn one_item_opens_the_panel_and_a_successor_takes_both_over() {
    use makepad_widgets::*;
    use std::io::{BufRead, Write};
    use std::process::{Child, ChildStdin, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;
    let loader = std::env::var_os("RELAX_LOADER").expect("Relax names its test loader");
    let dir = std::env::temp_dir().join(format!("devhub-menu-bar-item-{}", std::process::id()));
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".makepad/devhub")).unwrap();
    struct Helper {
        run: Child,
        to: Option<ChildStdin>,
        answers: mpsc::Receiver<String>,
        log: std::path::PathBuf,
    }
    let start = |name: &str| -> Helper {
        let log = dir.join(format!("{name}.log"));
        let mut run = std::process::Command::new(&loader)
            .args(["run", "makepad-app-devhub", "--", "--menu-bar-item"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("HOME", &home)
            .env("MAKEPAD_HOME", home.join(".makepad"))
            .env("MAKEPAD_TRAY_SELFTEST", "stale")
            .env("MAKEPAD_HAL_BACKEND", "metal")
            .env("MAKEPAD_HIDE_WINDOWS", "1")
            .env("DEVHUB_PANEL_DEMO", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let (tx, answers) = mpsc::channel::<String>();
        let out = run.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if line.starts_with("{\"") && tx.send(line).is_err() {
                    break;
                }
            }
        });
        Helper { to: run.stdin.take(), run, answers, log }
    };
    // menu_item.rs's state line: a glyph, a status line, then the master's items
    let state = |line: &str| {
        let alpha = "ff".repeat(16 * 16);
        let item = |id: LiveId, label: &str, enabled: bool| format!("{{\"id\":\"{}\",\"label\":\"{label}\",\"enabled\":{enabled},\"checked\":null}}", id.0);
        format!("{{\"w\":16,\"h\":16,\"alpha\":\"{alpha}\",\"tooltip\":\"\",\"dropdown\":false,\"menu\":[{},{{\"line\":true}},{},{}]}}\n",
            item(LiveId(11), line, false), item(live_id!(open_panel), "Open panel", true), item(live_id!(open_window), "Open devhub", true))
    };
    let send = |h: &mut Helper, line: &str| h.to.as_mut().unwrap().write_all(state(line).as_bytes()).unwrap();
    let answer = |h: &mut Helper, what: &str, secs: u64| -> String {
        h.answers.recv_timeout(Duration::from_secs(secs)).unwrap_or_else(|e| {
            let _ = h.run.kill();
            let _ = h.run.wait();
            panic!("no answer to {what} ({e}):\n{}", std::fs::read_to_string(&h.log).unwrap_or_default())
        })
    };
    let (panel, ready, opened) = (format!("{{\"item\":\"{}\"}}", live_id!(open_panel).0), "{\"ready\":true}", "{\"panel\":true}");

    let mut old = start("old");
    // (its first answer waits for its build and start) nothing to show first: ready at once
    assert_eq!(answer(&mut old, "its start", 600), ready);
    send(&mut old, "Land: empty");
    assert_eq!(answer(&mut old, "the choice in the new menu", 60), panel);
    assert_eq!(answer(&mut old, "the panel's open", 60), opened, "Open panel opens the panel in the helper");
    send(&mut old, "Land: 1 waiting");
    assert_eq!(answer(&mut old, "the choice in the replaced menu", 60), panel);
    let kept = std::fs::read_to_string(home.join(".makepad/devhub/panel-window.json")).unwrap_or_default();
    assert!(kept.contains("\"shown\":true"), "the panel's window is kept as shown: {kept}");

    // the self-update's next helper, beside the old one, which holds the item
    let mut new = start("new");
    send(&mut new, "Land: 2 waiting");
    assert_eq!(answer(&mut new, "the successor's start", 300), ready, "the successor shows the kept panel, then says it is ready");
    assert!(old.run.try_wait().unwrap().is_none(), "the old helper still runs while its successor gets ready");
    let said = std::fs::read_to_string(&new.log).unwrap_or_default();
    assert!(said.contains("another helper holds"), "the successor waits for the item:\n{said}");
    assert!(said.contains("the panel shows again"), "the successor shows the panel the old one showed:\n{said}");
    // the master ends the old one: only now the successor shows the item (its menu's choice)
    drop(old.to.take());
    let status = old.run.wait().unwrap();
    let said = std::fs::read_to_string(&old.log).unwrap_or_default();
    assert!(said.contains("of the replaced menu"), "the second choice came from the replaced menu:\n{said}");
    assert!(status.success(), "a helper ends with its input ({status}):\n{said}");
    assert_eq!(answer(&mut new, "the choice in the successor's menu", 60), panel);
    drop(new.to.take());
    let status = new.run.wait().unwrap();
    let all = std::fs::read_to_string(&new.log).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(all.contains("let go of the item"), "the successor took the item once the old one ended:\n{all}");
    assert!(status.success(), "the successor ends with its input ({status}):\n{all}");
}
