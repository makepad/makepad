use makepad_platform::thread::{ui_hang, ui_phase, UiPhase};
use std::{process::Command, time::Duration};

fn main() {
    if std::env::args().any(|arg| arg == "--watchdog-child") {
        // the log sink a Cx starts: the watchdog's report reaches the log ring (read below) and
        // stderr (the parent's) through it
        makepad_platform::Cx::init_log();
        ui_hang::initialize();
        let (mut cursor, _) = makepad_platform::log_ring::read_since(0, 0);
        {
            let _event = ui_phase(UiPhase::CodePageInstall);
            { let _nested = ui_phase(UiPhase::FontAtlas); }
            std::thread::sleep(Duration::from_millis(400));
            // A stall of at least 400 ms that lasts until the watchdog sampled it: on a loaded
            // box the watchdog runs late (it ticked once, as a 414 ms stall ended, 11-24), and a
            // stall over by then has no sample to report.
            while !ui_hang::stall_sampled() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        // Its report, however late the watchdog writes it; production event dispatch never
        // waits for it.
        'reported: loop {
            let (next, lines) = makepad_platform::log_ring::read_since(cursor, 64);
            cursor = next;
            if lines.iter().any(|l| l.text.contains("[ui-hang]")) {
                break 'reported;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Idle must produce no report.
        std::thread::sleep(Duration::from_millis(400));
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--watchdog-child")
        // The sampler is opt-in, and this test is the opt-in.
        .env("MAKEPAD_UI_HANG_MS", "250")
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
