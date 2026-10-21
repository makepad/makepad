//! The HAL's domain host (HAL.md 5d): what starts, stops and relaunches apps when a privileged
//! domain (the window manager) sends `DomainStart` / `DomainStop` / `DomainRelaunch`. The HAL
//! hands each command to [`Host`]; an app starts as a domain of the running program
//! (`relax_compiler::domain::start_in_run`) with its whole run inside the launch the HAL made
//! (`makepad_hal::with_launch`: its `hal_open` opens the session the parent asked for). The
//! domain's progress, crash and exit go back to the HAL through [`observe`] as
//! `Hal::report_domain`, which tells the parent.
//!
//! The primary app of `makepad run` follows the same path as children of a window manager.
//! Privilege is granted by the host (`--privileged`); JIT code cannot claim it.

use makepad_hal::{domain_state, domain_stop, with_caps, with_launch, Caps, DomainHost, DomainLaunch, Hal};
use relax_compiler::domain::{self, AppSpec, Around, DomainState};
use std::sync::Arc;
use std::time::Duration;

/// The compile phases `domain::start` reports, in order (the HAL's `phase` is the index).
const PHASES: [&str; 8] = ["package graph", "resolve", "build scripts", "loading crates", "load", "collect", "link", "main"];

#[derive(Default)]
pub struct Host;

impl Host {
    fn launch(&self, launch: DomainLaunch) -> Result<(), String> {
        let domain = launch.domain;
        let hook = launch.clone();
        let primary = crate::hal_run::spec(domain);
        let privileged = primary.is_some() && relax_runner::privileged();
        let around: Around = Arc::new(move |run: &mut dyn FnMut()| with_caps(Caps { privileged }, || with_launch(&hook, || run())));
        let mut spec = primary.unwrap_or_else(|| AppSpec {
            package: launch.app.clone(), args: launch.args.split_terminator('\0').map(str::to_string).collect(), ..Default::default()
        });
        spec.parent = (launch.parent != 0).then_some(launch.parent);
        spec.id = Some(domain);
        spec.around = Some(around);
        domain::prepare(&spec).map_err(|e| e.join(" | "))?;
        // The registry exists before returning to HAL, so stop can cancel the loader even
        // before it is scheduled. Never join the compiling thread ahead of cancellation.
        if let Err(e) = std::thread::Builder::new().name(format!("domain-load-{domain}")).spawn(move || {
            if let Err(e) = domain::start_prepared_in_run(spec, domain) {
                if !matches!(domain::state(domain), Some(DomainState::Stopped)) {
                    eprintln!("makepad-run domain: {}: {}", launch.app, e.join(" | "));
                }
            }
        }) {
            domain::loading_failed(domain, &e.to_string());
            return Err(format!("starting the loader thread: {e}"));
        }
        Ok(())
    }
}

impl DomainHost for Host {
    fn start(&self, launch: DomainLaunch) -> Result<(), String> {
        self.launch(launch)
    }

    fn stop(&self, domain: u32, mode: u32) {
        domain::begin_stop(domain);
        relax_compiler::reload::domain_stopping(domain);
        relax_compiler::inspect::stop_domain(domain);
        relax_compiler::inspect::events::release_domain(domain);
        let how = if mode == domain_stop::KILL { domain::Stop::Kill } else { domain::Stop::Quit };
        if let Err(e) = domain::stop(domain, how) { eprintln!("makepad-run domain: {domain}: {e}"); }
        report_stop(domain);
    }

    // The HAL stopped the old run (`stop(QUIT)` returned) and freed its session: start again.
    fn relaunch(&self, launch: DomainLaunch) -> Result<(), String> {
        let id = launch.domain;
        crate::hal_run::restarting(id);
        let result = self.launch(launch);
        if let Err(e) = &result {
            eprintln!("makepad-run domain: {id}: {e}");
            crate::hal_run::observe(id, &DomainState::Crashed(e.clone()));
        }
        result
    }
}

/// `MAKEPAD_RUN_DOMAIN_REPORT=1`: each domain's start is measured on stderr: the process's memory
/// (RSS/PSS kB) when the start began, at the domain's first frame and 5 s later, and the time
/// from the start to the first frame.
fn report_start(id: domain::DomainId) {
    if std::env::var_os("MAKEPAD_RUN_DOMAIN_REPORT").is_none() {
        return;
    }
    let before = domain::process_memory_kb();
    let _ = std::thread::Builder::new().name(format!("domain-report-{id}")).spawn(move || {
        let first = domain::wait_first_frame(id, Duration::from_secs(300));
        let at = domain::process_memory_kb();
        std::thread::sleep(Duration::from_secs(5));
        let settled = domain::process_memory_kb();
        let package = domain::info(id).map(|i| i.package).unwrap_or_default();
        eprintln!(
            "makepad-run domain-report: id={id} pkg={package} first_frame_ms={} rss_kb={}/{}/{} pss_kb={}/{}/{}",
            first.map_or("none".to_string(), |ms| format!("{ms:.0}")),
            before.0,
            at.0,
            settled.0,
            before.1,
            at.1,
            settled.1
        );
        report_gpu(id);
    });
}

/// `MAKEPAD_RUN_DOMAIN_REPORT=1`: the HAL ledger at that moment on stderr: the session's GPU bytes by
/// kind (count) and the device's own total, so the phone's GPU memory can be read from logcat.
fn report_gpu(id: domain::DomainId) {
    let hal = Hal::global();
    let all = hal.report_all(30);
    let kinds = all.by_kind.iter().map(|g| format!("{}={:.1}MB/{}", g.key, g.bytes as f64 / 1048576.0, g.count)).collect::<Vec<_>>().join(" ");
    let sessions = all.by_session.iter().map(|s| format!("{}:{:.1}MB", s.session, s.bytes as f64 / 1048576.0)).collect::<Vec<_>>().join(" ");
    eprintln!(
        "relax-domain-gpu: id={id} ledger={:.1} MB device={} sessions=[{sessions}] kinds=[{kinds}]",
        all.total_bytes as f64 / 1048576.0,
        all.device_allocated.map_or("n/a".to_string(), |b| format!("{:.1} MB", b as f64 / 1048576.0)),
    );
}

/// `MAKEPAD_RUN_DOMAIN_REPORT=1`: what a stop left of the domain on stderr: its state, threads, arena and
/// static-block bytes still held, the bytes teardown has freed in all so far, and the process memory.
fn report_stop(id: domain::DomainId) {
    if std::env::var_os("MAKEPAD_RUN_DOMAIN_REPORT").is_none() {
        return;
    }
    let Some(i) = domain::info(id) else { return };
    let (rss, pss) = domain::process_memory_kb();
    eprintln!(
        "makepad-run domain-stop: id={id} pkg={} state={:?} threads={} arena_in_use={} arena_committed={} static_committed={} freed_by_teardown={} rss_kb={rss} pss_kb={pss}",
        i.package, i.state, i.threads, i.arena_in_use, i.arena_committed, i.static_committed, domain::freed_by_teardown()
    );
    eprintln!("makepad-run domain-stop-heaps: id={id} {}", domain::process_breakdown());
    // the process's memory once the stop's own work and the next frames have settled
    let _ = std::thread::Builder::new().name(format!("domain-report-{id}")).spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        let (rss, pss) = domain::process_memory_kb();
        eprintln!("makepad-run domain-after-stop: id={id} 8s later rss_kb={rss} pss_kb={pss}");
        eprintln!("makepad-run domain-after-stop-heaps: id={id} {}", domain::process_breakdown());
    });
}

/// A domain changed state (`domain::set_observer`): the HAL tells the parent.
fn observe(id: domain::DomainId, state: &DomainState) {
    let _kept = relax_compiler::zone_alloc::kept();
    crate::hal_run::observe(id, state);
    if std::env::var_os("MAKEPAD_RUN_DOMAIN_REPORT").is_some() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() % 1_000_000);
        eprintln!("domain-step: id={id} t={t} ms state={}", match state { DomainState::Compiling(p) => format!("compiling:{p}"), DomainState::Running => "running".into(), other => format!("{other:?}") });
    }
    let hal = Hal::global();
    match state {
        DomainState::Compiling(phase) => {
            if *phase == PHASES[0] {
                report_start(id);
            }
            let at = PHASES.iter().position(|p| *p == phase.as_str()).unwrap_or(0);
            let (done, total) = progress_counts();
            let frac = if total > 0 && *phase == "loading crates" { done as f32 / total as f32 } else { 0.0 };
            let text = phase_text(phase, done, total);
            hal.report_domain(id, domain_state::COMPILING, at as u32, (at as f32 + frac) * 100.0 / PHASES.len() as f32, &text);
            if *phase == PHASES[0] { watch_counts(id); }
        }
        DomainState::Running => {
            hal.report_domain(id, domain_state::RUNNING, 0, 99.0, "starting app");
        }
        DomainState::Crashed(msg) => {
            relax_compiler::reload::domain_stopping(id);
            hal.report_domain(id, domain_state::CRASHED, 0, 0.0, msg);
        }
        DomainState::Exited(code) => {
            relax_compiler::reload::domain_stopping(id);
            hal.report_domain(id, domain_state::EXITED, 0, 100.0, &code.to_string());
        }
        // the HAL freed the session before it asked to stop
        DomainState::Stopped => {}
        DomainState::Leaked { threads } => {
            hal.report_domain(id, domain_state::CRASHED, 0, 0.0, &format!("shutdown timed out: {threads} threads still alive; domain memory retained"));
        }
    }
}

/// Registers the host with the HAL and the observer with the domains.
pub fn register() {
    let _kept = relax_compiler::zone_alloc::kept();
    #[cfg(not(target_arch = "wasm32"))]
    relax_compiler::inspect::register_host_inspect("gpu", makepad_hal::inspect::dispatch);
    #[cfg(not(target_arch = "wasm32"))]
    relax_compiler::inspect::register_host_inspect("hal", makepad_hal::inspect::dispatch);
    Hal::global().set_domain_host(Arc::new(Host::default()));
    domain::set_observer(observe);
}

pub fn start(launch: DomainLaunch) -> Result<(), String> {
    Host.launch(launch)
}

fn progress_counts() -> (u32, u32) {
    let (_, done, total) = relax_compiler::run::progress();
    (done, total)
}

/// What the card says for a phase: the real counts of this compile (crates loaded, functions compiled so far).
fn phase_text(phase: &str, done: u32, total: u32) -> String {
    let fns = relax_compiler::jit::COMPILED_FNS.load(std::sync::atomic::Ordering::Relaxed);
    let _ = (done, total);
    if fns > 0 { format!("{phase} \u{b7} {} fns", group(fns)) } else { phase.to_string() }
}

/// Until a domain's first frame its card is refreshed 10 times a second with the compile/startup
/// loop's own counters (crates loaded, functions compiled so far, including on demand after main).
fn watch_counts(id: domain::DomainId) {
    let _ = std::thread::Builder::new().name(format!("domain-counts-{id}")).spawn(move || loop {
        std::thread::sleep(Duration::from_millis(100));
        let Some(info) = domain::info(id) else { return };
        if info.first_frame_ms.is_some() { return; }
        let (done, total) = progress_counts();
        match info.state {
            DomainState::Compiling(phase) => {
                let at = PHASES.iter().position(|p| *p == phase.as_str()).unwrap_or(0);
                let frac = if total > 0 && phase == "loading crates" { done as f32 / total as f32 } else { 0.0 };
                Hal::global().report_domain(id, domain_state::COMPILING, at as u32, (at as f32 + frac) * 100.0 / PHASES.len() as f32, &phase_text(&phase, done, total));
            }
            DomainState::Running => {
                let fns = relax_compiler::jit::COMPILED_FNS.load(std::sync::atomic::Ordering::Relaxed);
                Hal::global().report_domain(id, domain_state::RUNNING, 0, 99.0, &format!("starting app \u{b7} {} fns", group(fns)));
            }
            _ => return,
        }
    });
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 { out.push(','); }
        out.push(c);
    }
    out
}
