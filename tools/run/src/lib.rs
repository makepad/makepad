//! The `makepad` binary's host: the bare runner (tools/run/runner, `relax_runner`) plus the crates
//! rustc built into this binary that Relax binds instead of compiling (provided.rs), and the
//! main-thread run host: the HAL's permanent window (hal_run.rs, the boot document) with the
//! compilation and the app domains as its guests (hal_host.rs). The Android host library
//! (tools/run/android_host) is the same host as a shared object.
mod hal_host;
mod hal_run;
mod quit_signal;
mod boot_progress;
mod boot_editor;
mod boot_input;
mod boot_probe;

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
    }
}

pub static HOST: relax_runner::Host = relax_runner::Host {
    provenance: env!("MAKEPAD_RUN_PROVENANCE"),
    init: init,
    provide: provided,
    // interactive visual iteration: a bounded compile-ahead of what the UI is about to need
    ahead: relax_compiler::options::AheadLevel::Balanced,
    default_launch: Some(checkout_runner),
};

/// `makepad` with no arguments and no `runner` launch record (an installation's entry script
/// writes one): the runner app of the checkout this binary was built from, when it has one
/// (apps/commercial/runner), run from that checkout like `makepad run makepad-runner` there.
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
        "makepad_tween_script" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-tween-script.src.bin")),
        "makepad_script_audio_aot" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-script-audio-aot.src.bin")),
        "makepad_hal" => include_bytes!(concat!(env!("OUT_DIR"), "/makepad-hal.src.bin")),
        _ => panic!("unregistered provided crate {name}"),
    }
}

/// The source identity build.rs computed for a provided crate (srchash.rs).
fn src(name: &str) -> u64 {
    let v = match name {
        "relax_provided_fixture" => env!("MAKEPAD_RUN_SRC_relax_provided_fixture"),
        "makepad_live_id" => env!("MAKEPAD_RUN_SRC_makepad_live_id"),
        "makepad_script" => env!("MAKEPAD_RUN_SRC_makepad_script"),
        "makepad_script_std" => env!("MAKEPAD_RUN_SRC_makepad_script_std"),
        "makepad_script_census" => env!("MAKEPAD_RUN_SRC_makepad_script_census"),
        "makepad_script_compute" => env!("MAKEPAD_RUN_SRC_makepad_script_compute"),
        "makepad_shader" => env!("MAKEPAD_RUN_SRC_makepad_shader"),
        "makepad_tween_script" => env!("MAKEPAD_RUN_SRC_makepad_tween_script"),
        "makepad_script_audio_aot" => env!("MAKEPAD_RUN_SRC_makepad_script_audio_aot"),
        "makepad_hal" => env!("MAKEPAD_RUN_SRC_makepad_hal"),
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
extern crate relax_provided_fixture;

fn provided() {
    use std::collections::HashMap;
    // the HAL starts and stops apps as domains through this host; the boot app's privilege is the host's grant
    hal_host::register();
    // the registry, its keys and the lazily built `type_name` strings of the entries (statics of
    // the provided crates) live for the process: the program's heap, not the compiler's
    let _kept = relax_compiler::zone_alloc::kept();
    const CRATES: [&str; 10] = [
        "relax_provided_fixture",
        "makepad_live_id",
        "makepad_script",
        "makepad_script_std",
        "makepad_script_census",
        "makepad_script_compute",
        "makepad_shader",
        "makepad_tween_script",
        "makepad_script_audio_aot",
        "makepad_hal",
    ];
    for krate in CRATES {
        let mut shims = HashMap::new();
        let mut types = HashMap::new();
        let mut layouts = HashMap::new();
        for e in makepad_relax_api::entries() {
            if e.module.split("::").next() != Some(krate) {
                continue;
            }
            if e.kind == 0 || e.kind > 4 {
                continue;
            }
            match e.kind {
                makepad_relax_api::OP => {
                    if let Some(f) = e.shim {
                        shims.insert(relax_compiler::provided::norm((e.key)()), relax_compiler::provided::Shim { sig: (e.sig)(), slots: e.slots, f });
                    }
                }
                makepad_relax_api::HANDLE => {
                    types.insert(relax_compiler::provided::norm((e.key)()), (e.size, e.align, e.drop, e.offsets));
                }
                makepad_relax_api::PLAIN => {
                    layouts.insert(relax_compiler::provided::norm((e.key)()), (e.size, e.align, e.offsets));
                }
                _ => {}
            }
        }
        relax_compiler::provided::register(krate, shims, types, layouts, makepad_relax_api::last_panic, src(krate), source_manifest(krate));
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
    relax_compiler::run::set_main_host(relax_compiler::run::MainHost {
        domain_zero: cfg!(target_os = "android"),
        run: hal_run::run,
        open: hal_run::open,
        start: hal_run::start,
        relaunch: hal_run::relaunch,
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
