use makepad_platform::thread::{ui_hang, ui_phase, UiPhase};
use std::{process::Command, time::Duration};

fn main() {
    if std::env::args().any(|arg| arg == "--watchdog-child") {
        ui_hang::initialize();
        {
            let _event = ui_phase(UiPhase::CodePageInstall);
            { let _nested = ui_phase(UiPhase::FontAtlas); }
            std::thread::sleep(Duration::from_millis(400));
        }
        // Idle must produce no report. Also gives the watchdog time to flush
        // the completed frame; production event dispatch never waits for it.
        std::thread::sleep(Duration::from_millis(400));
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--watchdog-child")
        .output()
        .expect("run isolated main-thread watchdog test");
    let log = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{log}");
    assert!(output.status.success(), "{log}");
    let reports: Vec<_> = log.lines().filter(|line| line.contains("[ui-hang]")).collect();
    assert_eq!(reports.len(), 1, "one completed stall, none at idle: {log}");
    let line = reports[0];
    assert!(line.contains("phase=code-page-install"), "nested phase must restore: {line}");
    assert!(!line.contains("samples=0"), "{line}");
    assert!(!line.contains("<stack unavailable>"), "{line}");
    assert!(line.contains("top frames:"), "{line}");
    let elapsed: f64 = line.split("[ui-hang] ").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    assert!(elapsed >= 400.0, "{line}");
}
