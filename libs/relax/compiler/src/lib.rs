//! Relax: the compiler, JIT runtime and std as a library. Source-matched linked proc macros
//! run natively; all other implementations use the JIT host (pmhost.rs). Embedders (the
//! `makepad` loader) set lexer extension points (lexer::register_raw_body) before `cli_main`.
//! `cli_main` is the command line driver (the `makepad` binary):
//!   makepad parse <units.json> [threads]   parse the crate graph, time it, write the census
//!   makepad file <path> [edition]          parse one file, report errors
//!   makepad clean [-p <package>]           remove the workspace's <target>/relax (or one app's files)
//!   makepad run [opts] <package> [cargo flags] [-- args]   boot a cargo package from source (run.rs)

/// First statement of a test that builds, runs or drops a JIT unit: the test runs alone in a
/// child process of this test binary, and the calling process only checks its verdict. The JIT
/// runtime's state is process-wide by design (one program per process): the unit a lazy compile
/// reaches (`jit::UNIT`), the exit latch a dropped unit sets (`preempt::stop_all`: from then on
/// no cache key is made and preemptible checks unwind), the tier and speculation workers. Tests
/// sharing a process saw each other's units: a lazily compiled slot of one test was built in
/// another's unit (the parallel suite aborted: "failed to initiate panic" / "free(): invalid
/// pointer"), and every test after the first dropped unit found the cache stopped.
#[cfg(test)]
macro_rules! isolated_test {
    ($name:ident) => {
        if !$crate::test_child::run_alone(module_path!(), stringify!($name)) {
            return;
        }
    };
}

#[cfg(test)]
mod test_child {
    const CHILD: &str = "RELAX_TEST_CHILD";

    /// True in the child (run the test body); in the parent, runs the child and returns false
    /// once it passed (a failure panics with the child's output).
    pub fn run_alone(module: &str, name: &str) -> bool {
        let module = module.split_once("::").map_or("", |(_, rest)| rest);
        let test = if module.is_empty() { name.to_string() } else { format!("{module}::{name}") };
        if std::env::var(CHILD).is_ok_and(|t| t == test) {
            return true;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--include-ignored", "--test-threads=1", "--nocapture"])
            .env(CHILD, &test)
            .output()
            .unwrap();
        let all = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success() && all.contains("test result: ok. 1 passed"), "{test} in its own process:\n{all}");
        false
    }
}

pub mod front;
pub mod global_asm;
pub mod live;
pub mod reload;
pub mod modules;
pub mod sandbox;
pub mod domain;
pub(crate) mod domain_threads;
pub mod jit;
pub mod guards;
pub mod web;
pub mod web32;
pub mod coverage;
pub mod owner;
pub mod postframe;
mod nounwind;
pub mod os;
pub mod lower;
pub mod cabi;
pub use relax_types::*;
pub mod run;
mod harness;
mod test_pkg;
pub mod tui;
pub mod check;
mod gandalf_out;
pub mod serve;
mod serve_rpc;
mod serve_query;
mod serve_os;
pub mod boundary;
pub mod provided;
pub mod pmhost;
mod native_macros;
pub mod ctfe;
mod valtrees;
pub mod syms;
pub mod wasm_export;
pub mod pack;
pub mod store;
pub mod kernel_store;
pub mod cache;
pub mod clean;
pub mod mainprof;
pub mod inspect;
pub mod memreport;
pub mod registry_net;

/// Compiler statics held for rollback; forget them before destroying their generations.
pub struct ZoneTaken {
    owner: owner::ZoneTaken,
    native_macros: native_macros::ZoneTaken,
    cache: cache::ZoneTaken,
    jit: jit::ZoneTaken,
    run: run::ZoneTaken,
    tui: tui::ZoneTaken,
}

/// Takes this crate's compiler statics while every compiler thread is quiesced.
/// None, taking nothing, when the owner has requests queued.
pub fn zone_take() -> Option<ZoneTaken> {
    let owner = owner::zone_take()?;
    Some(ZoneTaken {
        owner,
        native_macros: native_macros::zone_take(),
        cache: cache::zone_take(),
        jit: jit::zone_take(),
        run: run::zone_take(),
        tui: tui::zone_take(),
    })
}

/// Restores the statics after an aborted swap, forgetting the new generation's values.
pub fn zone_put_back(taken: ZoneTaken) {
    owner::zone_put_back(taken.owner);
    native_macros::zone_put_back(taken.native_macros);
    cache::zone_put_back(taken.cache);
    jit::zone_put_back(taken.jit);
    run::zone_put_back(taken.run);
    tui::zone_put_back(taken.tui);
}

#[cfg(test)]
mod parse_work_bench;

/// The system allocator (Relax's own malloc zone once a program swaps the default zone:
/// zone_alloc.rs), with per-subsystem accounting when MAKEPAD_RUN_MEM_REPORT is set (memstat.rs).
#[global_allocator]
static GLOBAL_ALLOC: memstat::Counting = memstat::Counting;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// The command line driver: `makepad <command> ..` (also the `makepad` loader's).
/// What this compiler was built from (jit::PROVENANCE).
pub use jit::PROVENANCE;

/// The same, as a marker other binaries find in this one's bytes (provenance_of).
#[used]
static PROVENANCE_MARK: &str = concat!("<<RELAX-PROVENANCE:", env!("MAKEPAD_RUN_PROVENANCE"), ">>");

/// The compiler stamp a binary file carries (its PROVENANCE_MARK), if any.
pub fn provenance_of(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let tag = b"<<RELAX-PROVENANCE:";
    let i = bytes.windows(tag.len()).position(|w| w == tag)? + tag.len();
    let j = bytes[i..].windows(2).position(|w| w == b">>")?;
    Some(String::from_utf8_lossy(&bytes[i..i + j]).into_owned())
}

/// The Rust release Relax's std and language follow (the toolchain the entry scripts pin). A build
/// script's RUSTC is Relax itself; asked `--version`, it answers this (relax_as_rustc).
pub const RUSTC_VERSION: &str = "1.98.0";

/// MAKEPAD_RUN_AS_RUSTC=1: a build script's RUSTC is Relax itself. `--version` / `-vV` / `-V` answer
/// as a rustc of RUSTC_VERSION would (build scripts such as libc's read the minor version);
/// `--print` requests about the target (cargo's target-info probe, which `cargo metadata
/// --filter-platform` runs) answer from Relax's own target model (print_as_rustc). Returns whether
/// it answered.
pub fn relax_as_rustc() -> bool {
    if std::env::var_os("MAKEPAD_RUN_AS_RUSTC").is_none() {
        return false;
    }
    let args: Vec<String> = os::proc::args_bytes().iter().skip(1).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    // the script's own run (and a nested script's) is a Relax command, not a RUSTC question
    if args.first().is_some_and(|a| a == "run") {
        return false;
    }
    let sess = &session_from_env(&args);
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--version"] | ["-V"] => println!("rustc {} (makepad run {})", RUSTC_VERSION, PROVENANCE),
        ["-vV"] | ["--version", "--verbose"] => {
            println!("rustc {} (makepad run {})", RUSTC_VERSION, PROVENANCE);
            println!("binary: rustc");
            println!("commit-hash: unknown");
            println!("commit-date: unknown");
            println!("host: {}", crate::cfg::CfgSet::machine(sess, &[]).triple());
            println!("release: {}", RUSTC_VERSION);
        }
        _ => {
            // probes pipe a program in (cargo's target info, rustix, autocfg): read it all, as rustc
            // would, so the writer never sees a broken pipe
            let mut input = Vec::new();
            if args.iter().any(|a| a == "-") {
                let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut input);
            }
            match print_as_rustc(sess, &args, &input) {
                Ok(out) => {
                    use std::io::Write;
                    let mut o = std::io::stdout().lock();
                    let _ = o.write_all(out.as_bytes());
                    let _ = o.flush();
                }
                Err(e) => {
                    // anything else asked of RUSTC (a compile probe: autocfg, rustc_version's
                    // `--emit`): Relax is not rustc. Fail the probe as a missing rustc would (the
                    // script decides); never run the script's program again.
                    eprintln!("makepad: not rustc: a build script asked RUSTC {:?}: {}", args, e);
                    std::process::exit(1);
                }
            }
        }
    }
    true
}

/// The `--print` requests of a rustc command line, answered for Relax's target model the way
/// rustc_driver_impl print_crate_info answers them (in command-line order, nothing compiled):
/// `cfg` (CfgSet of `--target`, else of this machine, plus `--cfg`), `file-names` and `crate-name`
/// (rustc_session output.rs filename_for_input with the target's affixes; a crate type the target
/// cannot produce is dropped with rustc's warning), `sysroot` (Relax's std sources), `split-debuginfo`
/// (the target's supported kinds), `host-tuple`. Err for anything else: any option outside those and
/// lint levels, an input whose crate attributes would matter, an unknown target.
pub fn print_as_rustc(sess: &std::sync::Arc<crate::Session>, args: &[String], input: &[u8]) -> Result<String, String> {
    let (mut prints, mut crate_types, mut cfgs) = (Vec::new(), Vec::new(), Vec::new());
    let (mut target, mut crate_name, mut inputs): (Option<String>, Option<String>, Vec<String>) = (None, None, Vec::new());
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // `--opt=value`, `--opt value`, and `-Xvalue` / `-X value` for the one-letter lint options
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (a, None),
        };
        let value = |i: &mut usize| -> Result<String, String> {
            if let Some(v) = &inline {
                return Ok(v.clone());
            }
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("{name} needs a value"))
        };
        match name {
            "--print" => prints.push(value(&mut i)?),
            "--target" => target = Some(value(&mut i)?),
            "--crate-type" => crate_types.extend(value(&mut i)?.split(',').map(str::to_string)),
            "--crate-name" => crate_name = Some(value(&mut i)?),
            "--cfg" => cfgs.push(value(&mut i)?),
            // lint levels change no print
            "--cap-lints" | "--check-cfg" => {
                value(&mut i)?;
            }
            "-W" | "-A" | "-D" | "-F" => {
                i += 1;
            }
            _ if a.len() > 2 && (a.starts_with("-W") || a.starts_with("-A") || a.starts_with("-D") || a.starts_with("-F")) => {}
            "-" => inputs.push(a.to_string()),
            _ if !a.starts_with('-') => inputs.push(a.to_string()),
            _ => return Err(format!("`{a}` is not a print request")),
        }
        i += 1;
    }
    if prints.is_empty() {
        return Err("no `--print` request".to_string());
    }
    let cs = match &target {
        None => crate::cfg::CfgSet::machine(sess, &[]),
        Some(t) => {
            use crate::options::CheckTarget as C;
            match C::from_triple(t).ok_or_else(|| format!("could not find specification for target \"{t}\""))? {
                C::MacosArm64 => crate::cfg::CfgSet::macos_arm64(sess, &[]),
                C::LinuxX64 => crate::cfg::CfgSet::linux_x64(sess, &[]),
                C::WindowsX64 => crate::cfg::CfgSet::windows_x64(sess, &[]),
                C::AndroidArm64 => crate::cfg::CfgSet::android_arm64(sess, &[]),
                C::Web(w) => crate::cfg::CfgSet::web(sess, w, &[]),
                C::Host => unreachable!("from_triple names a target"),
            }
        }
    };
    let triple = cs.triple();
    let attrs_matter = prints.iter().any(|p| p == "file-names" || p == "crate-name");
    if attrs_matter && (inputs.len() != 1 || inputs[0] != "-" || !input.iter().all(u8::is_ascii_whitespace)) {
        // rustc reads #![crate_name] / #![crate_type] of the input here
        return Err("file-names / crate-name of a source input".to_string());
    }
    let name = crate_name.unwrap_or_else(|| "rust_out".to_string());
    if crate_types.is_empty() {
        crate_types.push("bin".to_string());
    }
    // rustc_target spec: (dll prefix, dll suffix, exe suffix, staticlib prefix, staticlib suffix,
    // split-debuginfo kinds, only_cdylib)
    let (dll_pre, dll_suf, exe_suf, st_pre, st_suf, split, only_cdylib): (&str, &str, &str, &str, &str, &[&str], bool) = match triple {
        "aarch64-apple-darwin" => ("lib", ".dylib", "", "lib", ".a", &["off", "packed", "unpacked"], false),
        "x86_64-unknown-linux-gnu" | "aarch64-linux-android" => ("lib", ".so", "", "lib", ".a", &["off", "packed", "unpacked"], false),
        "x86_64-pc-windows-gnu" => ("", ".dll", ".exe", "lib", ".a", &["off"], false),
        "wasm32-unknown-unknown" | "wasm64-unknown-unknown" => ("", ".wasm", ".wasm", "lib", ".a", &["off"], true),
        t => return Err(format!("no target model for {t}")),
    };
    let mut out = String::new();
    for p in &prints {
        match p.as_str() {
            "cfg" => {
                let mut v: Vec<String> = cs.set.iter().chain(cfgs.iter()).filter(|e| !e.starts_with("feature=")).map(|e| match e.split_once('=') {
                    Some((k, val)) => format!("{k}=\"{}\"", val.trim_matches('"')),
                    None => e.clone(),
                }).collect();
                v.sort();
                v.dedup();
                for c in v {
                    out.push_str(&c);
                    out.push('\n');
                }
            }
            "file-names" => {
                for t in &crate_types {
                    let f = match t.as_str() {
                        "rlib" | "lib" => format!("lib{name}.rlib"),
                        "dylib" | "proc-macro" if only_cdylib => {
                            eprintln!("warning: dropping unsupported crate type `{t}` for target `{triple}`");
                            continue;
                        }
                        "dylib" | "cdylib" | "proc-macro" => format!("{dll_pre}{name}{dll_suf}"),
                        "staticlib" => format!("{st_pre}{name}{st_suf}"),
                        "bin" => format!("{name}{exe_suf}"),
                        other => return Err(format!("unknown crate type: `{other}`")),
                    };
                    out.push_str(&f);
                    out.push('\n');
                }
            }
            "crate-name" => {
                out.push_str(&name);
                out.push('\n');
            }
            "sysroot" => {
                out.push_str(&std_dir());
                out.push('\n');
            }
            "split-debuginfo" => {
                for k in split {
                    out.push_str(k);
                    out.push('\n');
                }
            }
            "host-tuple" => {
                out.push_str(crate::cfg::CfgSet::machine(sess, &[]).triple());
                out.push('\n');
            }
            other => return Err(format!("`--print {other}`")),
        }
    }
    Ok(out)
}

/// Construct the invocation inputs before parsing; CLI overrides replace environment values.
/// rustc: compiler/rustc_session/src/config.rs:2428, session.rs:90-93.
pub fn session_from_env(args: &[String]) -> std::sync::Arc<crate::Session> {
    let mut options = crate::options::Options::from_env();
    // `--no-asm` / `--allow-c` anywhere before `--`, for every command (run, test, check, the
    // relax runner): the environment carries them to the processes this one starts (build
    // scripts, test harnesses); cli_main drops them from the arguments
    for a in args.iter().take_while(|a| a.as_str() != "--") {
        match a.as_str() {
            "--no-asm" => {
                options.no_asm = true;
                std::env::set_var("MAKEPAD_RUN_NO_ASM", "1");
            }
            "--allow-c" => {
                options.allow_c = true;
                std::env::set_var("MAKEPAD_RUN_ALLOW_C", "1");
            }
            _ => {}
        }
    }
    options.live |= args.get(1).map(String::as_str) == Some("live");
    if args.get(1).map(String::as_str) == Some("fix") {
        options.relaxed = Some("all".to_string());
    }
    options.cpu = relax_backend::x64::detect_cpu(&options);
    options.spec_hot = Some(options.spec_hot.unwrap_or_else(crate::jit::on_battery));
    options.spec_jobs = Some(options.spec_jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get()).saturating_sub(1).clamp(1, 6)));
    options.tier1_profile_data = options.tier1_profile.as_ref().and_then(|p| p.to_str()).and_then(|p| std::fs::read_to_string(p).ok());
    let mut session = crate::Session { options, runtime: crate::options::RuntimeOptions::from_env() };
    session.options.target = match session.options.check_target {
        crate::options::CheckTarget::WindowsX64 => "x86_64-pc-windows-gnu",
        crate::options::CheckTarget::MacosArm64 => "aarch64-apple-darwin",
        crate::options::CheckTarget::LinuxX64 => "x86_64-unknown-linux-gnu",
        crate::options::CheckTarget::AndroidArm64 => "aarch64-linux-android",
        crate::options::CheckTarget::Web(w) => crate::options::CheckTarget::web_triple(w),
        _ if cfg!(target_os = "android") => "aarch64-linux-android",
        _ if cfg!(all(target_os = "macos", target_arch = "aarch64")) => "aarch64-apple-darwin",
        _ if cfg!(windows) => "x86_64-pc-windows-gnu",
        _ => "x86_64-unknown-linux-gnu",
    }.to_string();
    std::sync::Arc::new(session)
}

pub fn cli_main(sess: &std::sync::Arc<crate::Session>) {
    registry_net::install();
    if std::env::var_os("MAKEPAD_RUN_CODE_SIG").is_some() {
        relax_backend::regalloc::SIG_ON.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    // a program's own arguments after `--` may be any bytes (`makepad run p -- $'\xff'`): Relax
    // reads its own options lossily, the program gets the bytes (run.rs)
    let mut args: Vec<String> = os::proc::args_bytes().iter().map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    // the gates were read into the session (session_from_env)
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let mut k = 0;
    args.retain(|a| {
        k += 1;
        !(k <= end && (a == "--no-asm" || a == "--allow-c"))
    });
    if args.get(1).map(String::as_str) == Some("--version") {
        println!("makepad run {}", PROVENANCE);
        std::process::exit(0);
    }
    if args.len() < 3 && !matches!(args.get(1).map(String::as_str), Some("serve" | "clean")) {
        eprintln!("makepad: {} needs an argument; run `makepad help {}`", args.get(1).map_or("this command", String::as_str), args.get(1).map_or("", String::as_str));
        std::process::exit(2);
    }
    match args[1].as_str() {
        "parse" => {
            let threads = if args.len() > 3 { args[3].parse::<usize>().unwrap_or(16) } else { 16 };
            cmd_parse(sess, &args[2], threads);
        }
        "live" => {
            let code = live::cmd_live(sess, &args[2..]);
            os::proc::exit(code);
        }
        "resolve" => {
            cmd_resolve(sess, &args[2]);
        }
        "run" => {
            let code = run::cmd_run(sess, &args[2..]);
            os::proc::exit(code);
        }
        // `makepad check [--json] [--units u.json] <package>`: type/borrow check and compile, run
        // nothing; `--watch`: the warm session (type/borrow check only unless --deep), re-checking
        // what each edit invalidates
        "check" => {
            crate::lexer::set_validation(true);
            os::proc::exit(serve_rpc::check(sess, &args[2..]));
        }
        // `makepad selftest [--root-only] <package>`: a test of Relax itself -- every fn of the
        // package's crates through the whole pipeline to machine code, every failure listed
        // (check.rs); never through the check server
        "selftest" => {
            let mut a = vec!["--check".to_string(), "--selftest".to_string()];
            a.extend(args[2..].iter().filter(|x| !matches!(x.as_str(), "--serve" | "--no-serve")).cloned());
            os::proc::exit(run::cmd_run(sess, &a));
        }
        "serve" => os::proc::exit(serve_rpc::cmd_serve(sess, &args[2..])),
        // `makepad clean [-p <package>]`: remove the workspace's <target>/relax (clean.rs)
        "clean" => std::process::exit(clean::cmd_clean(&args[2..])),
        "schema" | "hover" | "definition" => os::proc::exit(serve_rpc::query(sess, &args[1], &args[2..])),
        // `makepad fix --to-strict [--units u.json] <package>`: check in relaxed mode (all features
        // unless --relaxed=<list> follows) and write every accepted relaxation's rustc form back
        // into the sources (relaxed.rs)
        "fix" if args.get(2).map(String::as_str) == Some("--to-strict") => {
            relaxed::FIX.store(true, std::sync::atomic::Ordering::Relaxed);

            let mut a = vec!["--check".to_string()];
            a.extend_from_slice(&args[3..]);
            let code = run::cmd_run(sess, &a);
            std::process::exit(code);
        }
        // `makepad test <file.rs>`: one file's tests; `makepad test <package>`: the package's
        // lib and integration tests, each a harness program (run.rs `--harness`)
        "test" if !args[2..].iter().any(|a| a.ends_with(".rs")) => {
            let code = test_pkg::cmd_test_package(&args[2..]);
            os::proc::exit(code);
        }
        "test" => {
            let code = cmd_test(sess, &args[2..]);
            os::proc::exit(code);
        }
        "std-surface" => {
            let code = surface::cmd_std_surface(sess, &args[2..]);
            std::process::exit(code);
        }
        "lex" => {
            let code = source_context::cmd_lex(&args[2..]);
            std::process::exit(code);
        }
        "file" => {
            let ed = if args.len() > 3 { args[3].parse::<u16>().unwrap_or(2021) } else { 2021 };
            let src = std::fs::read(&args[2]).expect("read");
            match parser::parse_source(sess, &src, ed) {
                Ok(p) => println!("ok: {} items, {} exprs", p.ast.items.len(), p.ast.exprs.len()),
                Err((pos, msg)) => {
                    let (l, c) = lexer::line_col(&src, pos);
                    println!("{}:{}:{}: {}", args[2], l, c, msg);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("unknown command");
            std::process::exit(2);
        }
    }
}

fn count_lines(s: &[u8]) -> usize {
    let mut n = 0;
    for &c in s {
        if c == b'\n' {
            n += 1;
        }
    }
    n
}

/// Parses all files on `threads` threads; returns (seconds, errors).
fn parse_all(sess: &std::sync::Arc<crate::Session>, files: &[crates::SrcFile], threads: usize) -> (f64, usize) {
    parse_all_mode(sess, files, threads, false)
}

fn parse_all_mode(sess: &std::sync::Arc<crate::Session>, files: &[crates::SrcFile], threads: usize, lazy: bool) -> (f64, usize) {
    let next = AtomicUsize::new(0);
    let errors = AtomicUsize::new(0);
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| { let _z = crate::zone_alloc::compiler_thread(); loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= files.len() {
                    break;
                }
                let f = &files[i];
                if parser::parse_source_mode(sess, &f.src, f.edition, lazy).is_err() {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            } });
        }
    });
    (t0.elapsed().as_secs_f64(), errors.load(Ordering::Relaxed))
}

fn lex_all(files: &[crates::SrcFile]) -> (f64, usize) {
    let t0 = Instant::now();
    let mut ntok = 0;
    let mut toks = Vec::new();
    for f in files {
        toks.clear();
        let _ = lexer::lex(&f.src, f.edition, &mut toks);
        ntok += toks.len();
    }
    (t0.elapsed().as_secs_f64(), ntok)
}

fn cmd_parse(sess: &std::sync::Arc<crate::Session>, units_path: &str, threads: usize) {
    let crates = match crates::load_units(units_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let t0 = Instant::now();
    let mut cfgs = Vec::new();
    for c in &crates {
        cfgs.push(cfg::CfgSet::host(sess, &c.features));
    }
    let (files, missing, errors) = crates::discover(sess, &crates, &cfgs, threads);
    let discover_s = t0.elapsed().as_secs_f64();
    let mut lines = 0usize;
    let mut bytes = 0usize;
    let mut act_files = 0usize;
    let mut act_lines = 0usize;
    for f in &files {
        let n = count_lines(&f.src);
        lines += n;
        bytes += f.src.len();
        if f.active {
            act_files += 1;
            act_lines += n;
        }
    }
    println!("crates: {}  files: {}  lines: {}  bytes: {}", crates.len(), files.len(), lines, bytes);
    println!("active on x86_64 linux (cfg): files: {}  lines: {}", act_files, act_lines);
    println!("discover (read + parse + follow mods, {} threads): {:.3} s", threads, discover_s);
    println!("parse errors: {}", errors.len());
    for e in &errors {
        println!("  ERROR {}", e);
    }
    println!("declared mod files not found: {}", missing.len());
    for m in &missing {
        println!("  missing {}", m);
    }

    // timings (sources in memory)
    let mut best_lex = f64::MAX;
    let mut ntok = 0;
    for _ in 0..3 {
        let (t, n) = lex_all(&files);
        if t < best_lex {
            best_lex = t;
        }
        ntok = n;
    }
    let mut best1 = f64::MAX;
    for _ in 0..3 {
        let (t, _) = parse_all(sess, &files, 1);
        if t < best1 {
            best1 = t;
        }
    }
    let mut bestn = f64::MAX;
    for _ in 0..5 {
        let (t, _) = parse_all(sess, &files, threads);
        if t < bestn {
            bestn = t;
        }
    }
    let mut best1l = f64::MAX;
    for _ in 0..3 {
        let (t, _) = parse_all_mode(sess, &files, 1, true);
        if t < best1l {
            best1l = t;
        }
    }
    let mut bestnl = f64::MAX;
    for _ in 0..5 {
        let (t, _) = parse_all_mode(sess, &files, threads, true);
        if t < bestnl {
            bestnl = t;
        }
    }
    // raw macro bodies (registered raw-body macros, never tokenised as Rust)
    let mut raw_bytes = 0usize;
    let mut raw_blocks = 0usize;
    let mut toks = Vec::new();
    for f in &files {
        toks.clear();
        let _ = lexer::lex(&f.src, f.edition, &mut toks);
        for t in &toks {
            if t.kind == lexer::T::RawBody {
                raw_bytes += (t.hi - t.lo) as usize;
                raw_blocks += 1;
            }
        }
    }
    let l = lines as f64;
    let mb = bytes as f64 / 1e6;
    println!("raw macro bodies: {}, {} bytes ({:.1}% of source)", raw_blocks, raw_bytes, raw_bytes as f64 * 100.0 / bytes as f64);
    println!("lazy bodies, 1 thread:   {:.3} s  {:.2} M lines/s  {:.0} MB/s", best1l, l / best1l / 1e6, mb / best1l);
    println!("lazy bodies, {} threads: {:.3} s  {:.2} M lines/s  {:.0} MB/s", threads, bestnl, l / bestnl / 1e6, mb / bestnl);
    println!("tokens: {}", ntok);
    println!("lex only, 1 thread:    {:.3} s  {:.2} M lines/s  {:.0} MB/s", best_lex, l / best_lex / 1e6, bytes as f64 / best_lex / 1e6);
    println!("full parse, 1 thread:    {:.3} s  {:.2} M lines/s  {:.0} MB/s", best1, l / best1 / 1e6, mb / best1);
    println!("full parse, {} threads:  {:.3} s  {:.2} M lines/s  {:.0} MB/s", threads, bestn, l / bestn / 1e6, mb / bestn);

    // census per crate
    let mut per: Vec<census::Census> = Vec::new();
    for _ in 0..crates.len() {
        per.push(census::Census::new());
    }
    for f in &files {
        if !f.active {
            continue;
        }
        if let Ok(p) = parser::parse_source(sess, &f.src, f.edition) {
            let mut w = census::Walker::new(&p, &cfgs[f.crate_idx as usize]);
            w.file();
            per[f.crate_idx as usize].merge(&w.cen);
        }
    }
    let mut total = census::Census::new();
    for c in &per {
        total.merge(c);
    }
    let out = census_report(&crates, &per, &total);
    let path = "census.md";
    std::fs::write(path, out).expect("write census");
    println!("census written to {}", path);
}

fn census_report(crates: &[crates::CrateUnit], per: &[census::Census], total: &census::Census) -> String {
    let mut s = String::new();
    s.push_str("# Relax subset census (base stack)\n\n");
    s.push_str("| crate | ");
    for n in census::NAMES {
        s.push_str(n);
        s.push_str(" | ");
    }
    s.push_str("build.rs |\n|---|");
    for _ in 0..census::N + 1 {
        s.push_str("---|");
    }
    s.push('\n');
    let mut order: Vec<usize> = Vec::new();
    for i in 0..crates.len() {
        order.push(i);
    }
    order.sort_by(|a, b| per[*b].c[census::C_LINES].cmp(&per[*a].c[census::C_LINES]));
    for &i in &order {
        s.push_str(&format!("| {} | ", crates[i].name));
        for k in 0..census::N {
            s.push_str(&format!("{} | ", per[i].c[k]));
        }
        s.push_str(if crates[i].build_script { "yes |\n" } else { "|\n" });
    }
    s.push_str("| **total** | ");
    for k in 0..census::N {
        s.push_str(&format!("{} | ", total.c[k]));
    }
    s.push_str("|\n\n");
    let kinds: [(u8, &str); 5] = [
        (b'm', "user macro calls (R1/R2)"),
        (b'd', "third-party derives (R3)"),
        (b'o', "our derives"),
        (b'a', "unknown item attributes (attribute macros or derive helpers)"),
        (b'p', "iterator adapters (R5)"),
    ];
    for (k, title) in kinds {
        let mut v: Vec<(String, u64)> = Vec::new();
        for e in &total.names {
            if e.0 == k {
                v.push((e.1.clone(), e.2));
            }
        }
        v.sort_by(|a, b| b.1.cmp(&a.1));
        s.push_str(&format!("## {}\n\n", title));
        let mut n = 0;
        for (name, c) in &v {
            s.push_str(&format!("{} {}, ", name, c));
            n += 1;
            if n >= 80 {
                break;
            }
        }
        s.push_str("\n\n");
    }
    s
}

pub mod std_source;

/// std sources: $MAKEPAD_RUN_STD, else <repo>/libs/relax/compiler/std next to the binary's target dir
pub fn std_dir() -> String {
    match std::env::var("MAKEPAD_RUN_STD") {
        Ok(d) => d,
        Err(_) => {
            let mut d = std::env::current_exe().unwrap_or_default();
            for _ in 0..3 {
                d.pop();
            }
            // <worktree>/<cargo target dir>/release/makepad -> <worktree>/libs/relax/compiler/std; node 165's
            // layout keeps the sources under <dir>/src
            let direct = format!("{}/libs/relax/compiler/std", d.display());
            if std::path::Path::new(&direct).join("core/lib.rs").exists() {
                direct
            } else {
                format!("{}/src/libs/relax/compiler/std", d.display())
            }
        }
    }
}

/// Loads core, alloc (when present) and std; returns core and the dependency list user crates
/// get ([core, alloc, std]).
/// core, alloc (when the std has it) and std, as load_std_crates loads them (for a preload).
/// `simd`: with core::simd / std::simd (see `std_simd_wanted`).
pub fn std_pre_crates(sess: &std::sync::Arc<crate::Session>, simd: bool) -> Vec<preload::PreCrate> {
    let dir = std_dir();
    let mut v = vec![preload::PreCrate {
        
        name: "core".to_string(),
        root: format!("{}/core/lib.rs", dir),
        edition: 2021,
        cfg: std_cfg(sess, simd),
        deps: Vec::new(),
        proc_macro: false,
        image_deps: Vec::new(),
        image_dep_names: Vec::new(),
    }];
    let alloc_root = format!("{}/alloc/lib.rs", dir);
    if crate::vfs::exists(&alloc_root) {
        v.push(preload::PreCrate {
            
            name: "alloc".to_string(),
            root: alloc_root,
            edition: 2021,
            cfg: std_cfg(sess, simd),
            deps: Vec::new(),
            proc_macro: false,
            image_deps: Vec::new(),
            image_dep_names: Vec::new(),
        });
    }
    v.push(preload::PreCrate {
        
        name: "std".to_string(),
        root: format!("{}/std/lib.rs", dir),
        edition: 2021,
        cfg: std_cfg(sess, simd),
        deps: Vec::new(),
        proc_macro: false,
        image_deps: Vec::new(),
        image_dep_names: Vec::new(),
    });
    for i in 0..v.len() {
        v[i].image_dep_names = (0..i).map(|d| (v[d].name.clone(), v[d].name.clone())).collect();
        v[i].image_deps = (0..i).collect();
    }
    v
}

/// The cfg set of the std crates: the host's, plus relax_portable_simd when the program uses
/// core::simd.
pub fn std_cfg(sess: &std::sync::Arc<crate::Session>, simd: bool) -> cfg::CfgSet {
    // a web target's std is read as rustc's std for the triple (cfg.rs `web`), with Relax std's
    // web layer (`relax_web`: core::arch::wasm32, std/sys/web.rs)
    let mut c = match sess.options.check_target {
        crate::options::CheckTarget::Web(w) => {
            let mut c = cfg::CfgSet::web(sess, w, &[]);
            c.set.push("relax_web".to_string());
            c
        }
        _ => cfg::CfgSet::host(sess, &[]),
    };
    if simd {
        c.set.push("relax_portable_simd".to_string());
    }
    c
}

/// Whether a program with these crate roots can use core::simd / std::simd: only a crate whose
/// root enables `#![feature(portable_simd)]` can name them (rustc: E0658 otherwise), so std
/// leaves portable-simd out of every other program. A root that cannot be read counts as
/// wanting it.
pub fn std_simd_wanted<'a>(roots: impl IntoIterator<Item = &'a str>) -> bool {
    roots.into_iter().any(|r| match crate::vfs::read_shared(r) {
        Ok(src) => src.windows(13).any(|w| w == b"portable_simd"),
        Err(_) => true,
    })
}

pub fn load_std_crates(sess: &std::sync::Arc<crate::Session>, prog: &mut program::Program) -> Result<(u32, Vec<(program::Sym, u32)>), String> {
    load_std_crates_with(sess, prog, |_, _| {})
}

/// The boot driver can retire each std crate before loading the next one.
pub(crate) fn load_std_crates_with(sess: &std::sync::Arc<crate::Session>, prog: &mut program::Program, mut loaded: impl FnMut(&mut program::Program, u32)) -> Result<(u32, Vec<(program::Sym, u32)>), String> {
    let r = load_std_crates_inner(sess, prog, &mut loaded);
    // a proc-macro world waiting for a std file this load did not make parses it itself
    if !prog.std_from_shared {
        program::share_std_end();
    }
    r
}

fn load_std_crates_inner(sess: &std::sync::Arc<crate::Session>, prog: &mut program::Program, loaded: &mut impl FnMut(&mut program::Program, u32)) -> Result<(u32, Vec<(program::Sym, u32)>), String> {
    let simd = prog.std_simd;
    let pre = std_pre_crates(sess, simd);
    let core = prog.load_crate_deferred("core", &pre[0].root, 2021, std_cfg(sess, simd), Vec::new())?;
    loaded(prog, core);
    let core_sym = prog.syms.intern("core");
    let mut deps = vec![(core_sym, core)];
    if pre.len() == 3 {
        let alloc = prog.load_crate_deferred("alloc", &pre[1].root, 2021, std_cfg(sess, simd), deps.clone())?;
        loaded(prog, alloc);
        deps.push((prog.syms.intern("alloc"), alloc));
    }
    let std = prog.load_crate_deferred("std", &pre[pre.len() - 1].root, 2021, std_cfg(sess, simd), deps.clone())?;
    loaded(prog, std);
    deps.push((prog.syms.intern("std"), std));
    Ok((core, deps))
}

pub fn load_with_std(sess: &std::sync::Arc<crate::Session>, root: &str, name: &str, test: bool) -> Result<(program::Program, u32, u32), Vec<String>> {
    pmhost::install();
    let mut prog = program::Program::new(sess);
    prog.std_simd = std_simd_wanted([root]);
    let (core, deps) = load_std_crates(sess, &mut prog).map_err(|e| vec![e])?;
    let mut cfgset = cfg::CfgSet::host(sess, &[]);
    if test {
        cfgset.set.push("test".to_string());
    }
    cfgset.set.push("relax".to_string());
    // a single file is edition 2021 unless its header says otherwise (`//@ edition: 2024`,
    // the compiletest directive; rustc takes it as --edition)
    let edition = std::fs::read_to_string(root).ok().and_then(|src| {
        src.lines().take_while(|l| l.starts_with("//")).find_map(|l| l.strip_prefix("//@ edition:").and_then(|e| e.trim().parse::<u16>().ok()))
    });
    let krate = match prog.load_crate_deferred(name, root, edition.unwrap_or(2021), cfgset, deps) {
        Ok(c) => c,
        Err(e) => return Err(vec![e]),
    };
    prog.prelude_crate = Some(core);
    prog.resolve_imports();
    if !prog.errors.is_empty() {
        return Err(prog.errors.clone());
    }
    Ok((prog, core, krate))
}

fn cmd_test(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> i32 {
    let runtime = &sess.runtime;
    let options = &sess.options;
    let root = &args[0];
    let filter = if args.len() > 1 { args[1].clone() } else { String::new() };
    // the test program's argv is its own (the harness binary and its filter), not Relax's
    let mut pa = vec![root.clone().into_bytes()];
    if !filter.is_empty() {
        pa.push(filter.clone().into_bytes());
    }
    jit::set_program_args(pa);
    let t0 = Instant::now();
    let (prog, core, krate) = match load_with_std(sess, root, "test_crate", true) {
        Ok(x) => x,
        Err(e) => {
            for m in e {
                eprintln!("error: {}", m);
            }
            return 1;
        }
    };
    // the crate under test is the program's own crate (run.rs): its #![recursion_limit] and
    // #![type_length_limit] govern monomorphization (TypingEnv::fully_monomorphized)
    crate::layout::set_root_crate(krate);
    let t1 = Instant::now();
    let mut tcx = tcx::Tcx::new();
    tcx.collect(&prog);
    if !tcx.errors.is_empty() {
        for m in &tcx.errors {
            eprintln!("error: {}", m);
        }
        return 1;
    }
    let t2 = Instant::now();
    let tests = jit::find_tests(&prog, krate);
    let mut u = jit::Unit::new(prog, tcx, core);
    let enum_errors = u.tcx.check_enum_discriminants(&u.prog, krate);
    if !enum_errors.is_empty() {
        for error in enum_errors { eprintln!("error: {}", error); }
        return 1;
    }
    jit::enable_pipelining(options);
    println!(
        "boot: load+parse+resolve {:.2} ms, signatures/layouts {:.2} ms; {} tests",
        (t1 - t0).as_secs_f64() * 1e3,
        (t2 - t1).as_secs_f64() * 1e3,
        tests.len()
    );
    let repeat: usize = runtime.repeat;
    let mut passed = 0;
    let mut failed = 0;
    let mut runs = Vec::new();
    for _ in 0..repeat {
        for d in &tests {
            runs.push(*d);
        }
    }
    let total_runs = runs.len();
    for (ri, d) in runs.into_iter().enumerate() {
        let last_round = ri + tests.len() >= total_runs;
        let name = u.prog.def_path(d);
        if !filter.is_empty() && !name.contains(&filter) {
            continue;
        }
        // speculative workers compile concurrently: unit access under the lock
        let entry = crate::owner::call(crate::owner::Class::Program, || {
            let id = u.fn_id(jit::FnKey::Inst(d, Vec::new()));
            crate::wasm_export::root(id, &name);
            u.entry(id)
        });
        let ts = Instant::now();
        jit::watchdog(runtime.watchdog_ms);
        let up: *mut jit::Unit = &mut *u;
        let r = if runtime.test_domains <= 1 {
            unsafe { jit::call_jit(up, entry, 0) }
        } else {
            // the same test at the same time in N domains of this unit (domain 0 + N-1 new
            // blocks), each on its own thread: a static shared between them fails its asserts
            let bases: Vec<u64> = std::iter::once(u.syms.dom0()).chain((1..runtime.test_domains).map(|_| crate::owner::call(crate::owner::Class::Program, || crate::domain::create(&u)))).collect();
            let upa = up as usize;
            let rs: Vec<_> = std::thread::scope(|sc| {
                let hs: Vec<_> = bases.iter().map(|&b| sc.spawn(move || unsafe { jit::call_jit_in(upa as *mut jit::Unit, b, entry, 0) })).collect();
                hs.into_iter().map(|h| h.join().expect("domain test thread")).collect()
            });
            for &b in &bases[1..] {
                crate::domain::destroy(b);
            }
            rs.into_iter().find(|r| r.is_err()).unwrap_or(Ok(0))
        };
        jit::watchdog_done();
        let dt = ts.elapsed().as_secs_f64() * 1e3;
        if !last_round {
            continue;
        }
        crate::owner::call(crate::owner::Class::Program, || {
            match r {
                Ok(_) => {
                    println!("test {} ... ok ({:.2} ms)", name, dt);
                    passed += 1;
                }
                Err(p) => {
                    println!("test {} ... FAILED ({:.2} ms)", name, dt);
                    println!("  report: {}", u.report(&p));
                    failed += 1;
                }
            }
            for e in std::mem::take(&mut u.errors) {
                println!("  {}", e);
            }
        });
    }
    let code = crate::owner::call(crate::owner::Class::Program, || {
        println!(
            "test result: {} passed; {} failed. compiled {} fns ({} bytes, {} RIR insts): typeck {:.2} ms, lower {:.2} ms, opt {:.2} ms, codegen {:.2} ms",
            passed,
            failed,
            u.stats.compiled,
            u.stats.code_bytes,
            u.stats.rir_insts,
            u.stats.typeck_ns as f64 / 1e6,
            u.stats.lower_ns as f64 / 1e6,
            u.stats.opt_ns as f64 / 1e6,
            u.stats.codegen_ns as f64 / 1e6
        );
        let mut rules = String::new();
        for (i, n) in relax_codegen::opt::RULE_NAMES.iter().enumerate() {
            rules.push_str(&format!("{} {}  ", n, u.stats.inline_rules[i]));
        }
        println!("inlined call sites by rule: {}", rules);
        if std::env::var("MAKEPAD_RUN_PASS_TIMES").is_ok() {
            let mut passes = String::new();
            for i in 0..relax_backend::stats::PASS_NAMES.len() {
                passes.push_str(&format!("{} {:.2}  ", relax_backend::stats::PASS_NAMES[i], relax_backend::stats::PASS_NS[i].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6));
            }
            println!("opt passes (ms): {}", passes);
        }
        u.flush_query_errors(true);
        if !u.errors.is_empty() {
            for error in &u.errors { eprintln!("error: {error}"); }
            return 1;
        }
        if failed > 0 {
            1
        } else {
            0
        }
    });
    // the caller ends the process: tearing the unit down would wait on state a contained fault
    // (a stack overflow in the compiler) left locked
    std::mem::forget(u);
    code
}

/// M2: loads every crate of the unit graph with Relax's core/std and resolves all imports.
fn cmd_resolve(sess: &std::sync::Arc<crate::Session>, units_path: &str) {
    let crates = match crates::load_units(units_path).and_then(|mut c| {
        crates::apply_substitutes(sess, &mut c, &std_dir(), false)?;
        Ok(c)
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            return;
        }
    };
    let t0 = Instant::now();
    let mut prog = program::Program::new(sess);
    prog.std_simd = std_simd_wanted(crates.iter().map(|c| c.src_path.as_str()));
    let (core, std_deps) = load_std_crates(sess, &mut prog).unwrap();
    let mut load_errors = 0;
    // dependencies first: a crate's `macro_rules!` must be known when its users parse
    let order = crates::topo_order(&crates);
    let mut crate_of = vec![u32::MAX; crates.len()];
    for &ci in &order {
        let c = &crates[ci];
        let mut deps = std_deps.clone();
        for (i, n) in &c.deps {
            let s = prog.syms.intern(n);
            deps.push((s, crate_of[*i]));
        }
        let mut cs = cfg::CfgSet::host(sess, &c.features);
        cs.set.push("relax".to_string());
        match prog.load_crate_deferred(&c.name, &c.src_path, c.edition, cs, deps) {
            Ok(k) => {
                prog.crates[k as usize].proc_macro = c.proc_macro;
                prog.crates[k as usize].is_bin = c.bin;
                crate_of[ci] = k;
            }
            Err(e) => {
                println!("load error {}: {}", c.name, e);
                load_errors += 1;
            }
        }
    }
    prog.prelude_crate = Some(core);
    let t1 = Instant::now();
    prog.resolve_imports();
    let t2 = Instant::now();
    let mut lines = 0;
    for f in &prog.files {
        lines += count_lines(&f.src);
    }
    let mut std_rooted = 0;
    let mut other = 0;
    let mut std_paths: Vec<(String, u32)> = Vec::new();
    let mut other_samples = Vec::new();
    for e in &prog.errors {
        let path = match e.find('`') {
            Some(a) => {
                let rest = &e[a + 1..];
                match rest.find('`') {
                    Some(b) => rest[..b].to_string(),
                    None => rest.to_string(),
                }
            }
            None => e.clone(),
        };
        if path.starts_with("std::") || path.starts_with("core::") || path.starts_with("alloc::") {
            std_rooted += 1;
            let mut found = false;
            for x in std_paths.iter_mut() {
                if x.0 == path {
                    x.1 += 1;
                    found = true;
                }
            }
            if !found {
                std_paths.push((path, 1));
            }
        } else {
            other += 1;
            if other_samples.len() < 60 {
                other_samples.push(e.clone());
            }
        }
    }
    let mut n_imports = 0;
    for _ in &prog.imports {
        n_imports += 1;
    }
    println!("crates loaded: {} ({} load errors), files {}, lines {}, defs {}, modules {}", crates.len(), load_errors, prog.files.len(), lines, prog.defs.len(), prog.mods.len());
    println!("load (read+parse+collect, 1 thread) {:.1} ms, import resolution {:.1} ms", (t1 - t0).as_secs_f64() * 1e3, (t2 - t1).as_secs_f64() * 1e3);
    let hs = macro_reg::stats();
    println!("derive sites expanded ahead in parallel {} (wall {:.1} ms), on the spot {}; parse of the expansions {:.1} ms",
        hs.pre_hits, hs.pre_wall_ns as f64 / 1e6, hs.pre_misses, hs.parse_ns as f64 / 1e6);
    println!("proc-macro host (in load): macro world {:.1} ms, {} expansions {:.1} ms (JIT compile {} fns {:.1} ms CPU, encode {:.1} ms, decode {:.1} ms, {} output bytes), failed {}; render {:.1} ms, parse {:.1} ms of {} bytes of expanded text",
        hs.world_ns as f64 / 1e6, hs.calls, hs.call_ns as f64 / 1e6, hs.compiled, hs.compile_ns as f64 / 1e6, hs.encode_ns as f64 / 1e6, hs.decode_ns as f64 / 1e6, hs.out_bytes, hs.failed, hs.render_ns as f64 / 1e6, hs.parse_ns as f64 / 1e6, hs.text_bytes);
    println!("imports: {} total, unresolved {} (std/core/alloc-rooted {}, other {})", n_imports, prog.errors.len(), std_rooted, other);
    std_paths.sort_by(|a, b| b.1.cmp(&a.1));
    println!("distinct std paths needed: {}", std_paths.len());
    let mut out = String::new();
    for (p, n) in &std_paths {
        out.push_str(&format!("{} {}\n", n, p));
    }
    let _ = std::fs::write("std_surface.txt", out);
    println!("(std surface written to std_surface.txt)");
    for e in other_samples {
        println!("  {}", e);
    }
}

#[cfg(test)]
mod lazy_reload_tests;

#[cfg(test)]
mod calls_tests;

#[cfg(test)]
mod vfs_bind_tests;

#[cfg(test)]
#[test]
fn round18_rpit_diagnostic() {
    // `makepad test` is process-global (it installs the runtime and exits): run it in a child of
    // this test binary, as lazy_reload_tests does, instead of naming a built makepad binary.
    const CHILD: &str = "MAKEPAD_RUN_ROUND18_RPIT_CHILD";
    let dir = env!("CARGO_MANIFEST_DIR");
    if std::env::var_os(CHILD).is_some() {
        let sess = session_from_env(&[]);
        os::proc::exit(cmd_test(&sess, &[format!("{dir}/tests/rpit.rs")]));
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "round18_rpit_diagnostic", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1").env("MAKEPAD_RUN_STD", format!("{dir}/std")).env("MAKEPAD_RUN_CACHE", "off")
        .env("MAKEPAD_HIDE_WINDOWS", "1").env("SANDBOX_MUTE", "1").output().unwrap();
    let all = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    eprintln!("{all}");
    assert!(output.status.success());
    assert!(all.contains("test result: 10 passed; 0 failed"), "{all}");
}
