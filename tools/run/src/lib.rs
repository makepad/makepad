//! The `makepad` binary's host: the bare runner (tools/run/runner, `relax_runner`) plus the crates
//! rustc built into this binary that Relax binds instead of compiling (provided.rs), and the
//! main-thread run host: the HAL's permanent window (hal_run.rs, the boot document) with the
//! compilation and the app domains as its guests (hal_host.rs). The Android host library
//! (tools/run/android_host) is the same host as a shared object.
// Diagnostics never panic: std's print macros panic when a write fails (ENOSPC on the session
// log in a full tmpfs /tmp aborted an app inside a lazy compile, 2026-10-23), so in this
// crate every print is a write whose error is dropped. Test builds keep std's captured macros.
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprintln {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_all(b"\n"); }};
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_fmt(format_args!("{}\n", format_args!($($t)+))); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprint {
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_fmt(format_args!($($t)+)); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! println {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_all(b"\n"); }};
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_fmt(format_args!("{}\n", format_args!($($t)+))); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! print {
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_fmt(format_args!($($t)+)); }};
}

mod hal_host;
#[cfg(target_os = "ios")]
pub mod ios;
mod hal_run;
mod quit_signal;
mod boot_progress;
mod boot_editor;
mod boot_input;
mod boot_probe;
mod build_flags;
mod change_report;
mod devhub_cmd;
mod script_answers;

pub use relax_boot::Environment as BootEnvironment;

/// The `makepad` binary's additions to the runner.
/// Before anything else runs: the kernel store, and the host providers `makepad inspect
/// file=<x.mkcap>` needs in this process too (frame captures replay here).
fn init() {
    install_kernel_store();
    #[cfg(not(target_arch = "wasm32"))]
    {
        relax_compiler::inspect::register_host_inspect("cap", makepad_hal::inspect::dispatch);
        relax_compiler::inspect::register_host_commands(makepad_hal::framecap::COMMANDS);
        relax_compiler::inspect::register_host_commands(makepad_hal::a11y::COMMANDS);
        // a font or automation script the run names under a virtual folder (an overlay's
        // sources) has no disk file; a relative path is the virtual workspace's, as in JIT code
        makepad_hal::set_virtual_reader(|path| {
            let path = if std::path::Path::new(path).is_relative() { format!("{}/{path}", relax_compiler::run::workspace_dir()) } else { path.to_string() };
            relax_compiler::vfs::is_virtual(&path).then(|| relax_compiler::vfs::read(&path))
        });
    }
}

/// NVIDIA's shader disk cache (GL and Vulkan) for every program this binary runs. The driver keys
/// its files by the executable's path, and every JIT build is a new executable: each binary's first
/// runs compiled every pipeline cold (a large app held its first frame 9 s for 109 pipelines), so they
/// all take one name. And a folder of their own, `$XDG_CACHE_HOME/makepad/nvidia`: the driver's
/// shared folder takes no entry once it holds its 1 GB, and every other program's per-path keys
/// fill it (a pool box at 1025 MB: each cold start of a large app compiled its pipelines again, the first
/// frame held 20.5 s for 177 pipelines; in a folder of its own 0.3 s). The driver trims neither: past
/// 3/4 of its cap this one is emptied, and the next runs fill it again. It is renamed away before
/// it is removed, so a start beside this one finds the full folder or a new empty one, never one
/// half removed, and a running program's open files go with the old one (its writes are lost, not
/// mixed into the new folder).
#[cfg(target_os = "linux")]
fn nvidia_shader_cache() {
    if std::env::var_os("__GL_SHADER_DISK_CACHE_APP_NAME").is_none() {
        std::env::set_var("__GL_SHADER_DISK_CACHE_APP_NAME", "makepad");
    }
    if std::env::var_os("__GL_SHADER_DISK_CACHE_PATH").is_some() {
        return;
    }
    let abs = |v: Option<std::ffi::OsString>| v.map(std::path::PathBuf::from).filter(|p| p.is_absolute());
    let Some(base) = abs(std::env::var_os("XDG_CACHE_HOME")).or_else(|| abs(std::env::var_os("HOME")).map(|h| h.join(".cache"))) else {
        return;
    };
    let dir = base.join("makepad").join("nvidia");
    fn bytes(dir: &std::path::Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
        entries.flatten().map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => bytes(&e.path()),
            _ => e.metadata().map_or(0, |m| m.len()),
        }).sum()
    }
    if bytes(&dir) > 768 << 20 {
        // (one name: what a killed removal left goes with the next one)
        let old = dir.with_extension("old");
        let _ = std::fs::remove_dir_all(&old);
        if std::fs::rename(&dir, &old).is_ok() {
            let _ = std::fs::remove_dir_all(&old);
        }
    }
    if std::fs::create_dir_all(&dir).is_ok() {
        std::env::set_var("__GL_SHADER_DISK_CACHE_PATH", &dir);
    }
}

pub static HOST: relax_runner::Host = relax_runner::Host {
    provenance: Some(env!("RELAX_PROVENANCE")),
    init: init,
    provide: provided,
    run_host: run_host,
    // interactive visual iteration: a bounded compile-ahead of what the UI is about to need
    ahead: relax_compiler::options::AheadLevel::Balanced,
    // apps for users: third-party proc macros run confined (what that covers, and what it does
    // not -- build scripts -- is in pmhost.rs's module doc)
    proc_macros: relax_compiler::pmhost::Policy::Sandboxed,
    default_launch: Some(checkout_runner),
    package: env!("CARGO_PKG_NAME"),
    manifest: concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
    commands: &[change_report::COMMAND, devhub_cmd::DEVHUB, devhub_cmd::OBSERVE, devhub_cmd::CI],
    // Makepad's script and widget derives run natively (tools/run/native_macros)
    native_sets: &[&makepad_run_native_macros::SET],
    // Splash bodies: lexed by the Splash tokenizer, never as Rust
    raw_body_macros: &["script_mod"],
    // makepad-platform's build script, answered from the code it shares (no process)
    script_answers: &[&script_answers::PLATFORM],
    // makepad's shader derives: makepad_shader_derive, its derive_impl and micro_proc_macro read no
    // filesystem, environment, clock or process state (audited 2026-10-09)
    pure_macro_crates: &["makepad_shader_derive"],
};

/// `makepad` with no arguments and no `runner` launch record (an installation's entry script
/// writes one): the default app of the checkout this binary was built from, when it has one,
/// run from that checkout.
fn checkout_runner() -> Option<relax_runner::launch::Launch> {
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().ok()?;
    if !checkout.join("apps/commercial/runner/Cargo.toml").is_file() {
        return None;
    }
    Some(relax_runner::launch::Launch {
        id: "runner".to_string(),
        title: "Makepad".to_string(),
        cwd: Some(checkout),
        env: Vec::new(),
        args: vec!["run".to_string(), "--quiet".to_string(), "makepad-runner".to_string()],
    })
}

/// The Android shell forwards its HAL safe area (logical dp), appearance and
/// accessibility snapshot here, also before session 0 opens. Desktop uses zero
/// insets and the HAL's host_dark(). No platform client is linked into Relax.
pub fn set_boot_environment(environment: BootEnvironment) {
    hal_run::set_environment(environment);
}

/// The compute crate's compiled kernels go to the Relax disk store (relax_compiler::kernel_store).
fn install_kernel_store() {
    relax_compiler::kernel_store::set_kernel_compiler(src("makepad_script_compute"));
    makepad_script_compute::kcache::install(makepad_script_compute::kcache::Hook {
        get: relax_compiler::kernel_store::get,
        put: relax_compiler::kernel_store::put,
    });
}

/// Per-file identity data emitted by build.rs, parsed only on a mismatch.
fn source_manifest(name: &str) -> &'static [u8] {
    match name {
        "relax_provided_fixture" => include_bytes!(concat!(env!("OUT_DIR"), "/relax-provided-fixture.src.bin")),
        "makepad_live_id" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-live-id.src.bin")),
        "makepad_script" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-script.src.bin")),
        "makepad_script_std" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-script-std.src.bin")),
        "makepad_script_census" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-script-census.src.bin")),
        "makepad_script_compute" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-script-compute.src.bin")),
        "makepad_shader" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-shader.src.bin")),
        "makepad_cpu_budget" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-cpu-budget.src.bin")),
        "makepad_tween_script" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-tween-script.src.bin")),
        "makepad_script_audio_aot" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-script-audio-aot.src.bin")),
        "makepad_hal" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-hal.src.bin")),
        _ => panic!("unregistered provided crate {name}"),
    }
}

/// The source identity build.rs computed for a provided crate (srchash.rs).
fn src(name: &str) -> u64 {
    let v = match name {
        "relax_provided_fixture" => env!("RELAX_SRC_relax_provided_fixture"),
        "makepad_live_id" => env!("RELAX_SRC_makepad_live_id"),
        "makepad_script" => env!("RELAX_SRC_makepad_script"),
        "makepad_script_std" => env!("RELAX_SRC_makepad_script_std"),
        "makepad_script_census" => env!("RELAX_SRC_makepad_script_census"),
        "makepad_script_compute" => env!("RELAX_SRC_makepad_script_compute"),
        "makepad_shader" => env!("RELAX_SRC_makepad_shader"),
        "makepad_cpu_budget" => env!("RELAX_SRC_makepad_cpu_budget"),
        "makepad_tween_script" => env!("RELAX_SRC_makepad_tween_script"),
        "makepad_script_audio_aot" => env!("RELAX_SRC_makepad_script_audio_aot"),
        "makepad_hal" => env!("RELAX_SRC_makepad_hal"),
        _ => panic!("unknown provided crate: {name}"),
    };
    u64::from_str_radix(v, 16).unwrap()
}

/// The crates built into this binary that Relax binds instead of compiling (provided.rs): their
/// `#[relax_api]` entries, read from this binary's `relax_api` link section.
/// Each binds only while the program's source of it is the one built here (check_sources).
// The provided crates: nothing here calls into them, but they must be in the link for their
// `#[relax_api]` entries (rustc keeps a linked crate's #[used] statics; an unnamed dependency is
// not linked at all)
extern crate makepad_hal;
extern crate makepad_live_id;
extern crate makepad_script;
extern crate makepad_script_audio_aot;
extern crate makepad_script_census;
extern crate makepad_script_compute;
extern crate makepad_script_std;
extern crate makepad_shader;
extern crate makepad_tween_script;
extern crate makepad_cpu_budget;
extern crate relax_provided_fixture;

fn provided() {
    use std::collections::HashMap;
    run_host();
    // the registry, its keys and the lazily built `type_name` strings of the entries (statics of
    // the provided crates) live for the process: the program's heap, not the compiler's
    let _kept = relax_compiler::zone_alloc::kept();
    const CRATES: [&str; 11] = [
        "relax_provided_fixture",
        "makepad_live_id",
        "makepad_script",
        "makepad_script_std",
        "makepad_script_census",
        "makepad_script_compute",
        "makepad_shader",
        "makepad_tween_script",
        "makepad_cpu_budget",
        "makepad_script_audio_aot",
        "makepad_hal",
    ];
    for krate in CRATES {
        let mut shims = HashMap::new();
        let mut types = HashMap::new();
        let mut layouts = HashMap::new();
        let mut per_side = std::collections::HashSet::new();
        for e in relax_api::entries() {
            if e.module.split("::").next() != Some(krate) {
                continue;
            }
            if e.kind == 0 || e.kind > 4 {
                continue;
            }
            match e.kind {
                relax_api::OP => {
                    if let Some(f) = e.shim {
                        shims.insert(relax_compiler::provided::norm((e.key)()), relax_compiler::provided::Shim { sig: (e.sig)(), slots: e.slots, f });
                    }
                }
                relax_api::HANDLE => {
                    types.insert(relax_compiler::provided::norm((e.key)()), (e.size, e.align, e.drop, e.offsets));
                }
                relax_api::PLAIN => {
                    layouts.insert(relax_compiler::provided::norm((e.key)()), (e.size, e.align, e.offsets));
                }
                relax_api::PER_SIDE => {
                    per_side.insert(relax_compiler::provided::norm((e.key)()));
                }
                _ => {}
            }
        }
        relax_compiler::provided::register(krate, shims, types, layouts, per_side, relax_api::last_panic, src(krate), source_manifest(krate));
    }

    // Splash VM registry -> makepad inspect (vm list, script error events)
    relax_compiler::inspect::set_vm_provider(makepad_script::inspect_registry::summaries);
    relax_compiler::inspect::set_script_stack_provider(makepad_script::inspect_registry::script_stack);
    makepad_script::inspect_registry::set_error_hook(relax_compiler::inspect::script_error_event);
    // Splash VM queries (src, vm, verbs, script errors): answered on the VM's own thread
    for (name, _, _) in makepad_script::inspect_query::COMMANDS {
        relax_compiler::inspect::register_host_inspect(name, script_query);
    }
    relax_compiler::inspect::register_host_commands(makepad_script::inspect_query::COMMANDS);
    // an exec restart (an edit's, or into a rebuilt makepad) opens the window where it was and
    // keeps the WM connection: the app keeps its window
    #[cfg(any(unix, windows))]
    relax_compiler::reload::set_before_exec(hal_run::prepare_exec, hal_run::undo_exec);
    relax_compiler::reload::set_restart_begins(hal_run::restart_begins);
}

/// The HAL's host, the main-thread run host (the window, the boot screen) and the run's native
/// end, once: with the provided crates, or before them for an app's start, whose window opens
/// before its source is had (`relax_runner::Host::run_host`).
fn run_host() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // before a program of this process loads the GPU driver (never for a build script's
        // cargo / rustc call, which answers before this)
        #[cfg(target_os = "linux")]
        nvidia_shader_cache();
        // the HAL starts and stops apps as domains through this host; the boot app's privilege is the host's grant
        hal_host::register();
        relax_compiler::run::set_main_host(relax_compiler::run::MainHost {
            domain_zero: cfg!(target_os = "android"),
            run: hal_run::run,
            open: hal_run::open,
            start: hal_run::start,
            batch: hal_run::batch,
            status: hal_run::status,
            end_app: hal_run::end_app,
            next_run: hal_run::next_run,
        });
        relax_compiler::run_end::set_native_end(
            || {
                makepad_hal::end_run();
                makepad_script::end_run();
                makepad_script_census::end_run();
                makepad_script_compute::end_run();
            },
            makepad_script::end_domain,
        );
    });
}

/// makepad inspect's Splash VM commands (makepad_script::inspect_query).
fn script_query(req: &relax_compiler::inspect::Req, out: &mut relax_compiler::inspect::Out) -> bool {
    let mut words = vec![req.cmd.clone()];
    words.extend(req.pos.iter().cloned());
    let Some(lines) = makepad_script::inspect_query::query(&words, &req.kv) else { return false };
    for l in lines {
        if let Some(e) = l.strip_prefix("{\"err\":") {
            let msg = e.strip_suffix('}').unwrap_or(e);
            let msg = msg.strip_prefix('"').and_then(|m| m.strip_suffix('"')).unwrap_or(msg);
            out.err(&msg.replace("\\\"", "\"").replace("\\\\", "\\"));
        } else {
            out.line(&l);
        }
    }
    true
}
