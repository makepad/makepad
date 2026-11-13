// The run-time module fixtures (libs/relax/compiler/tests/modules): run.sh runs each fixture
// program under the runner and checks its output, its tier-up and its reload trace; every case
// runs, and the failing ones print their output. One cargo test, so the test buckets run it.
use std::process::Command;

#[test]
fn the_module_fixtures_pass_under_the_runner() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = root.join("libs/relax/compiler/tests/modules/run.sh");
    let output = Command::new("bash").arg(&script).arg(runner()).current_dir(&root).output().unwrap();
    let all = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "libs/relax/compiler/tests/modules/run.sh failed:\n{all}");
}

/// The fixture workspaces under libs/relax/compiler/tests that come with a script of their own
/// (`run.sh <runner>` prints "<name>: ok"; cases of ported pull requests): all of them at once,
/// each under the runner, the failing ones with their output.
#[test]
fn the_fixture_scripts_pass_under_the_runner() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let names = ["macro_rename_from_expansion", "cache_warm_coroutine_arg", "cache_warm_anon_const_impls", "cache_warm_dep_macro_impls", "derive_declared_by_dep_macro", "pm_as_lib", "script_cargo_is_loader", "non_unicode_launch", "prepared_child", "script_native_dep", "self_native"];
    let failed: Vec<String> = std::thread::scope(|s| {
        let runs: Vec<_> = names
            .iter()
            .map(|name| {
                let root = &root;
                s.spawn(move || {
                    let script = root.join("libs/relax/compiler/tests").join(name).join("run.sh");
                    let output = Command::new("bash").arg(&script).arg(runner()).current_dir(root).env_remove("CARGO").output().unwrap();
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    (!stdout.contains(&format!("{name}: ok"))).then(|| format!("{name}:\n{stdout}\n{}", String::from_utf8_lossy(&output.stderr)))
                })
            })
            .collect();
        runs.into_iter().filter_map(|r| r.join().unwrap()).collect()
    });
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// What rustc refuses in the app's own crates, a run refuses too and says why, validation-only
/// checks included: E0436 used to compile and run, and the run said nothing.
#[test]
fn a_run_refuses_what_rustc_refuses_in_the_apps_crates() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scratch = std::env::temp_dir().join(format!("run-refuses-{}", std::process::id()));
    let output = Command::new(runner())
        .args(["run", "run-refuses"])
        .current_dir(root.join("libs/relax/compiler/tests/run_refuses"))
        .env("RELAX_STD", root.join("libs/relax/compiler/std"))
        .env("RELAX_CACHE_DIR", scratch.join("cache"))
        .env("RELAX_TARGET_DIR", scratch.join("target"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.status.code(), Some(101), "{stdout}\n{stderr}");
    assert!(!stdout.contains("started"), "it ran: {stdout}\n{stderr}");
    assert!(stderr.contains("E0436: functional record update syntax requires a struct"), "{stderr}");
}

/// A bin's wasm export (RELAX_WASM_EXPORT, a run checked for wasm32) exports its `main` as `t0`, the
/// function a host calls (libs/relax/compiler/tests/wasm_export_entry: a static whose initializer
/// compiles before `main`). The export took the run's first compiled function for its entry, here
/// that initializer, which a module does not ship: the module exported no function at all.
#[test]
fn a_bins_wasm_export_exports_its_main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join("libs/relax/compiler/tests/wasm_export_entry");
    let out = std::env::temp_dir().join(format!("wasm-export-entry-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let units = out.join("units.json");
    let unit = format!(
        r#"{{"pkg_id":"path+file://{}#wasm_export_entry@0.0.0","target":{{"kind":["bin"],"name":"wasm_export_entry","src_path":"{}","edition":"2021"}},"mode":"build","features":[],"dependencies":[]}}"#,
        out.display(),
        dir.join("wasm_export_entry.rs").display()
    );
    std::fs::write(&units, format!(r#"{{"version":1,"roots":[0],"units":[{unit}]}}"#)).unwrap();
    let wasm = out.join("x.wasm");
    let (mut command, scratch) = runner_in(&dir);
    let output = command
        .args(["run", "--quiet", "--no-live", "--units", &units.to_string_lossy(), "wasm_export_entry", "--release"])
        .env("RELAX_WASM_EXPORT", &wasm)
        .env("RELAX_CHECK_TARGET", "wasm32-unknown-unknown")
        .envs([("RELAX_SPEC", "0"), ("RELAX_O1", "always"), ("RELAX_O3", "0")])
        .output()
        .unwrap();
    let module = std::fs::read(&wasm);
    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_dir_all(&out);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let module = module.unwrap_or_else(|e| panic!("no module written ({e}):\n{stderr}"));
    assert!(module.starts_with(b"\0asm"), "not a wasm module");
    let exports = wasm_exports(&module);
    assert!(exports.iter().any(|(name, kind)| name == "t0" && *kind == 0), "the module's exports (name, kind; 0 = function): {exports:?}");
}

/// The export section of wasm module `b`: each export's name and kind (0 function, 1 table,
/// 2 memory, 3 global).
fn wasm_exports(b: &[u8]) -> Vec<(String, u8)> {
    fn leb(b: &[u8], i: &mut usize) -> usize {
        let (mut r, mut s) = (0, 0);
        loop {
            let x = b[*i];
            *i += 1;
            r |= ((x & 0x7f) as usize) << s;
            s += 7;
            if x < 0x80 {
                return r;
            }
        }
    }
    let mut out = Vec::new();
    let mut i = 8;
    while i < b.len() {
        let id = b[i];
        i += 1;
        let size = leb(b, &mut i);
        if id == 7 {
            let mut j = i;
            for _ in 0..leb(b, &mut j) {
                let len = leb(b, &mut j);
                out.push((String::from_utf8_lossy(&b[j..j + len]).into_owned(), b[j + len]));
                j += len + 1;
                leb(b, &mut j);
            }
        }
        i += size;
    }
    out
}

/// `makepad run --quiet <package>` in the fixture workspace `dir` (under libs/relax/compiler/tests),
/// with a cache and target directory of its own: its exit, stdout and stderr.
fn run_fixture(dir: &str, package: &str) -> (std::process::ExitStatus, String, String) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // unattended: a compile error or a panic ends the run, never parks it for someone to fix
    run_in(&root.join("libs/relax/compiler/tests").join(dir), &["run", "--quiet", "--unattended", package])
}

/// `makepad <args>` in the workspace at `dir`, with a cache and target directory of its own: its
/// exit, stdout and stderr.
fn run_in(dir: &std::path::Path, args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let (mut command, scratch) = runner_in(dir);
    let output = command.args(args).output().unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    (output.status, String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
}

/// The runner's command in the workspace at `dir`, with a cache and target directory of its own
/// (under the scratch folder it returns).
fn runner_in(dir: &std::path::Path) -> (Command, std::path::PathBuf) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let name = dir.file_name().unwrap().to_string_lossy();
    let scratch = std::env::temp_dir().join(format!("{name}-run-{}", std::process::id()));
    let mut command = Command::new(runner());
    command
        .current_dir(dir)
        .env("RELAX_STD", root.join("libs/relax/compiler/std"))
        .env("RELAX_CACHE_DIR", scratch.join("cache"))
        .env("RELAX_TARGET_DIR", scratch.join("target"))
        // as from a shell: CARGO is cargo's, which a run gives its build scripts and proc macros itself
        .env_remove("CARGO")
        .env_remove("RELAX_SCRIPT_DEPTH")
        .env_remove("RELAX_SCRIPT_CHAIN");
    (command, scratch)
}

/// Under `makepad test` the runner is the one running this test (its loader); under cargo it is
/// the binary cargo just built for this package. (`CARGO_BIN_EXE_makepad` is, under the runner, a
/// wrapper that would run the runner's own main as a program.)
fn runner() -> String {
    std::env::var("RELAX_LOADER").unwrap_or_else(|_| env!("CARGO_BIN_EXE_makepad").to_string())
}

/// `run_in` with `env` set, for at most `deadline`: a run still going then is hung (no exit), and
/// its stderr ends with `makepad inspect <pid> why-hung`'s answer before it is ended.
fn run_until(dir: &std::path::Path, args: &[&str], env: &[(&str, &str)], deadline: std::time::Duration) -> (Option<std::process::ExitStatus>, String, String) {
    use std::io::Read;
    let (mut command, scratch) = runner_in(dir);
    let mut child = command.args(args).envs(env.iter().copied()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    let read = |mut from: Box<dyn Read + Send>| std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = from.read_to_end(&mut text);
        String::from_utf8_lossy(&text).into_owned()
    });
    let (stdout, stderr) = (read(Box::new(child.stdout.take().unwrap())), read(Box::new(child.stderr.take().unwrap())));
    let t0 = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() { break Some(status); }
        if t0.elapsed() >= deadline { break None; }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let why = status.is_none().then(|| {
        let why = Command::new(runner()).args(["inspect", &child.id().to_string(), "why-hung"]).output().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        format!("\n-- hung after {deadline:?}; why-hung:\n{}{}", String::from_utf8_lossy(&why.stdout), String::from_utf8_lossy(&why.stderr))
    });
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap() + &why.unwrap_or_default());
    let _ = std::fs::remove_dir_all(&scratch);
    (status, stdout, stderr)
}

/// A run-time module loads while the proc-macro world lives
/// (libs/relax/compiler/tests/module_beside_world): one module gives the program's unit its
/// sandbox, the next one's derive builds the macro world again, and the world holds more crates
/// than the program then, so a crate index of one unit names a different crate in the other.
/// Each unit keeps its own sandbox state (sandbox.rs `State`).
#[test]
fn a_module_loads_while_the_macro_world_lives() {
    let (status, stdout, stderr) = run_fixture("module_beside_world", "mbw-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "app 42 plain 42 derived 42", "{stderr}");
}

/// A proc macro that calls `Span::call_site().file()` and `.local_file()` (stable since 1.88)
/// compiles and runs (libs/relax/compiler/tests/pm_span_file); `cargo run -p pmsf-app` prints the
/// same line. Before, the macro failed with E0599 `local_file`.
#[test]
fn a_proc_macro_asks_its_call_sites_file() {
    let (status, stdout, stderr) = run_fixture("pm_span_file", "pmsf-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "local_file ok", "{stderr}");
}

/// Impls of a trait with a supertrait bound (`where Self: Sized`) over arrays whose lengths are
/// constants (a type-level number's associated constant, a const fn's value), of a crate upstream
/// (libs/relax/compiler/tests/coherence_const_len).
/// `cargo run -p ccl-app` prints `bytes 44`. A run checked a crate's impls inside the type check
/// of whichever body came first, a length among them, and the checks evaluate the lengths: a
/// false E0391 cycle, and selection found the impls ambiguous.
#[test]
fn impls_over_constant_array_lengths_of_a_trait_with_supertraits_run() {
    let (status, stdout, stderr) = run_fixture("coherence_const_len", "ccl-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "bytes 44", "{stderr}");
    assert!(!stderr.contains("E0391") && !stderr.contains("E0119"), "{stderr}");
}

/// A dependency's impls whose array lengths come from a module the file selection skipped
/// (libs/relax/compiler/tests/coherence_deferred_len, that module deferred): the background
/// coherence check evaluates them while the program runs and lazy-compiles, and the run ends with
/// what `cargo run -p cdl-app` prints. The owner, serving the check's evaluation, waited for the
/// check's front read to load the module, and the check waited for the owner: the program's next
/// lazy compile waited for good (relax issue 33). The deadline only ends that hang.
#[test]
fn a_background_check_that_needs_a_skipped_module_lets_the_program_run() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let env = [
        ("RELAX_LAZY_DEFER", "libs/nums/src/sizes.rs"),
        // no completion loader (it would load the module first), and the check starts at once
        ("RELAX_LAZY_COMPLETE", "0"),
        ("RELAX_STARTUP_PHASE", "0"),
    ];
    let dir = root.join("libs/relax/compiler/tests/coherence_deferred_len");
    let (status, stdout, stderr) = run_until(&dir, &["run", "--quiet", "--unattended", "cdl-app"], &env, std::time::Duration::from_secs(60));
    assert!(status.is_some_and(|s| s.success()), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "steps 8 state 7", "{stderr}");
    // the check's verdict: the impls do not overlap
    assert!(!stderr.contains("E0119"), "{stderr}");
}

/// proc_macro's Literal constructors on text that needs escaping, `Literal::c_string` among them
/// (relax issue 103: proc-macro2 calls it), make the text and the value real proc_macro's do
/// (libs/relax/compiler/tests/pm_literal_ctors; `cargo run -p plc-app` prints the same lines).
#[test]
fn proc_macro_literal_constructors_escape_as_rustc() {
    let (status, stdout, stderr) = run_fixture("pm_literal_ctors", "plc-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    let expected = r#"c"tab\there \"q\" 'a' \\ € \u{7f} \xff"
c"plain"
"a\"b'c\\\n\t\0\u{301}é\u{7f}"
'\''
'"'
'\u{301}'
b'\''
b'"'
b'\0'
b'\xff'
b"q\"'\0\xff\x7f"
("tab\there \"q\" \'a\' \\ € \x7f \xff", "plain", "a\"b'c\\\n\t\0\u{301}é\u{7f}", '\'', '"', '\u{301}', 39, 34, 0, 255, [113, 34, 39, 0, 255, 127])"#;
    assert_eq!(stdout.trim(), expected, "{stderr}");
}

/// A derive's input carries the item's outer attributes, its doc comments as `#[doc = ".."]`
/// among them, also those before the derive and those a macro forwards (relax issue 106)
/// (libs/relax/compiler/tests/derive_item_doc; `cargo run -p did-app` prints the same lines).
#[test]
fn a_derive_sees_the_items_doc_comments() {
    let (status, stdout, stderr) = run_fixture("derive_item_doc", "did-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    let expected = r#"item [doc = " Doc of a struct before its derive."] fields [doc = " Field doc."]
item [doc = " First line," | doc = " second line with \"quotes\"." | doc = " After the derive." | doc = "explicit" | doc = " Block doc. "] fields [doc = " Variant doc."]
item [] fields []
item [doc = r" From a macro."] fields []"#;
    assert_eq!(stdout.trim(), expected, "{stderr}");
}

/// One crate's global_asm! that Relax cannot assemble (a zmm register needs EVEX) is left out
/// on its own (libs/relax/compiler/tests/global_asm_left_out): the app's blocks are placed and
/// run, the zmm crate is reported in one line that names it, its block and the instruction class,
/// a crate whose block jumps to its label is left out with it, and a call to either's symbol
/// panics naming the crate. Under rustc each line ends with its value (`cargo run -p gal-app`).
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn a_global_asm_crate_that_does_not_assemble_is_left_out_alone() {
    // unattended: the panics the app catches unwind at once (attended, the first parks)
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libs/relax/compiler/tests/global_asm_left_out");
    let (status, stdout, stderr) = run_in(&dir, &["run", "--quiet", "--unattended", "gal-app"]);
    assert!(status.success(), "{stdout}\n{stderr}");
    let out: Vec<&str> = stdout.lines().collect();
    let evex = "wide/src/lib.rs:5:1: zmm registers need EVEX, which is not supported (template line 5)";
    let wide = "wide: panicked: `gal_wide_probe` is not defined: crate `gal_wide`'s global_asm! that defines it is left out (";
    let user = "user: panicked: `gal_user_call` is not defined: crate `gal_user`'s global_asm! that defines it is left out (it uses `gal_wide_probe`, which crate `gal_wide`'s left-out global_asm! defines)";
    assert_eq!(out.len(), 3, "app, wide and user lines\n{stdout}\n{stderr}");
    assert_eq!(out[0], "app: 42", "the app's own global_asm! is placed\n{stderr}");
    assert!(out[1].starts_with(wide) && out[1].ends_with(&format!("{evex})")), "a call to the zmm crate's symbol panics naming it\n{stdout}\n{stderr}");
    assert_eq!(out[2], user, "a crate using the zmm crate's label is left out with it\n{stderr}");
    let said: Vec<&str> = stderr.lines().filter(|l| l.contains("global_asm! of crate")).collect();
    assert_eq!(said.len(), 2, "one line per left-out crate\n{stderr}");
    assert!(said.iter().any(|l| l.starts_with("relax run: global_asm! of crate `gal_wide` left out (its symbols stay undefined): ") && l.ends_with(evex)), "the zmm crate's line\n{stderr}");
    assert!(said.iter().any(|l| l.starts_with("relax run: global_asm! of crate `gal_user` left out")), "the using crate's line\n{stderr}");
}

/// Build scripts nest as deep as their build-dependencies go: app's script uses helper_a, whose
/// script uses helper_b (libs/relax/compiler/tests/script_nest). `cargo run -p script-nest-app`
/// prints `scan=b`; the third script's run was refused as a script that runs itself again.
#[test]
fn build_scripts_nest_as_deep_as_their_build_dependencies() {
    let (status, stdout, stderr) = run_fixture("script_nest", "script-nest-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "scan=b", "{stderr}");
}

/// blake3 with `pure` keeps `cc` as a build-dependency and compiles nothing with it: its script
/// runs under Relax without `--allow-c` (libs/relax/compiler/tests/c_free_script, stand-ins for
/// both). `cargo run -p c-free-script-app` prints `impl=portable`; the run was refused as one that
/// compiles C.
#[test]
fn a_pure_blake3_script_runs_without_allow_c() {
    let (status, stdout, stderr) = run_fixture("c_free_script", "c-free-script-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "impl=portable", "{stderr}");
}

/// A build script that compiles C through a build-dependency (`cc`, `cmake`, ..) needs `--allow-c`
/// even when its package also declares `links =`: a check without the flag skips it and says so, a
/// run refuses it. `links =` alone (metadata for dependents) still runs under Relax. Relax issue 68:
/// `links` was the reason found first, so such a script ran its C compile under the JIT. The
/// workspace is written here: lc-native has `links` and the build-dependency `cc` (lc-cc, a
/// stand-in that compiles nothing), lc-meta has `links` only. Under cargo `lc-app` prints
/// `answer 42` and `lc-meta-app` prints `meta ran true`.
#[test]
fn a_links_script_that_compiles_c_needs_allow_c() {
    let ws = std::env::temp_dir().join(format!("links-compiles-c-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let pkg = |name: &str, extra: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n{extra}");
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"cc\", \"native\", \"meta\", \"app\", \"meta-app\"]\nresolver = \"2\"\n".to_string()),
        ("cc/Cargo.toml", pkg("lc-cc", "")),
        ("cc/src/lib.rs", "pub fn compile(_source: &str) {}\n".to_string()),
        ("native/Cargo.toml", pkg("lc-native", "links = \"lc_native\"\nbuild = \"build.rs\"\n\n[build-dependencies]\ncc = { path = \"../cc\", package = \"lc-cc\" }\n")),
        ("native/build.rs", "fn main() {\n    cc::compile(\"csrc/native.c\");\n}\n".to_string()),
        ("native/src/lib.rs", "pub fn answer() -> u32 {\n    42\n}\n".to_string()),
        ("meta/Cargo.toml", pkg("lc-meta", "links = \"lc_meta\"\nbuild = \"build.rs\"\n")),
        ("meta/build.rs", "fn main() {\n    println!(\"cargo:rustc-check-cfg=cfg(lc_meta_ran)\");\n    println!(\"cargo:rustc-cfg=lc_meta_ran\");\n}\n".to_string()),
        ("meta/src/lib.rs", "pub fn ran() -> bool {\n    cfg!(lc_meta_ran)\n}\n".to_string()),
        ("app/Cargo.toml", pkg("lc-app", "\n[dependencies]\nlc-native = { path = \"../native\" }\n")),
        ("app/src/main.rs", "fn main() {\n    println!(\"answer {}\", lc_native::answer());\n}\n".to_string()),
        ("meta-app/Cargo.toml", pkg("lc-meta-app", "\n[dependencies]\nlc-meta = { path = \"../meta\" }\n")),
        ("meta-app/src/main.rs", "fn main() {\n    println!(\"meta ran {}\", lc_meta::ran());\n}\n".to_string()),
    ];
    for (path, text) in &files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let meta = run_in(&ws, &["run", "--quiet", "lc-meta-app"]);
    let check = run_in(&ws, &["check", "lc-app"]);
    let run = run_in(&ws, &["run", "--quiet", "lc-app"]);
    let _ = std::fs::remove_dir_all(&ws);
    let (status, stdout, stderr) = meta;
    assert!(status.success() && stdout.trim() == "meta ran true", "a links-only script runs under Relax: {stdout}\n{stderr}");
    let (status, stdout, stderr) = check;
    assert!(status.success(), "the check of the rest goes on: {stdout}\n{stderr}");
    assert!(stderr.contains("`lc-native`'s build script compiles C (lc-native: build-dependency cc): not run without `--allow-c`"), "the check skips the C script and says so: {stderr}");
    let (status, stdout, stderr) = run;
    assert!(!status.success() && !stdout.contains("answer"), "the run went on: {stdout}\n{stderr}");
    assert!(stderr.contains("`lc-native` compiles C (lc-native: build-dependency cc)"), "the run names the C gate: {stderr}");
}

/// A build script that fails fails `makepad check` too, as `cargo check` does: the check said
/// so and went on, the crate checked without the script's cfgs (relax PR 132). The script here
/// fails when SF_FAIL is set (`rerun-if-env-changed`): a check without it passes, and the next
/// check with it runs the script again and fails, the first run's output standing in for nothing.
#[test]
fn a_build_script_that_fails_fails_the_check() {
    let ws = std::env::temp_dir().join(format!("script-fails-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[package]\nname = \"sf-lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\nbuild = \"build.rs\"\n\n[workspace]\n"),
        ("build.rs", "fn main() {\n    println!(\"cargo:rerun-if-env-changed=SF_FAIL\");\n    if std::env::var_os(\"SF_FAIL\").is_some() {\n        eprintln!(\"sf-lib: told to fail\");\n        std::process::exit(1);\n    }\n}\n"),
        ("src/lib.rs", "pub fn two() -> u32 {\n    2\n}\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    // one cache for both checks: the second finds the first one's script output
    let (mut first, scratch) = runner_in(&ws);
    let passed = first.args(["check", "sf-lib"]).env_remove("SF_FAIL").output().unwrap();
    let (mut second, _) = runner_in(&ws);
    let failed = second.args(["check", "sf-lib"]).env("SF_FAIL", "1").output().unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_dir_all(&ws);
    let text = |o: &std::process::Output| format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(passed.status.success(), "the script runs and the check passes: {}", text(&passed));
    assert!(!failed.status.success(), "the script failed and the check passed: {}", text(&failed));
    assert!(text(&failed).contains("build script failed (exit 1)"), "the check names the script's failure: {}", text(&failed));
}

/// A `macro_rules!` that another macro's expansion defines in a module file, beside a
/// `pub(crate) use` of it, is callable through a `use` of that path from a file the load reads
/// first (relax issue 128: icu_locale_core's `subtag!`, made by `impl_tinystr_subtag!` in
/// `subtags` and imported by modules declared before it), and by its path, `made::inner::seven!()`
/// or `crate::made::inner::seven!()`, from such a file. rustc runs it; the load said the macro
/// was not supported.
#[test]
fn a_macro_an_expansion_defines_is_callable_through_its_import() {
    let ws = std::env::temp_dir().join(format!("expansion-macro-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[package]\nname = \"expansion-macro-import\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n"),
        ("src/main.rs", "#[macro_use]\nmod gen;\nmod early;\nmod made;\n\nuse made::seven;\n\nfn main() {\n    println!(\"{} {} {} {}\", seven!(), early::twice(), made::inner::seven!(), early::nested());\n}\n"),
        ("src/gen.rs", "macro_rules! make_seven {\n    ($v:expr) => {\n        macro_rules! seven_inner { () => { $v } }\n        pub(crate) use seven_inner as seven;\n    };\n}\n"),
        ("src/early.rs", "use crate::made::seven;\n\npub fn twice() -> u32 {\n    seven!() * 2\n}\n\npub fn nested() -> u32 {\n    crate::made::inner::seven!() + 1\n}\n"),
        ("src/made.rs", "pub mod inner {\n    make_seven!(70u32);\n}\n\nmake_seven!(7u32);\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_in(&ws, &["run", "--quiet", "--unattended", "expansion-macro-import"]);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(status.success() && stdout.trim() == "7 14 70 71", "the generated macro through its import and its path: {stdout}\n{stderr}");
}

/// One file declared as two modules (`#[path]` on both) imports a dependency's exported macro: the
/// import is in scope in each module (relax issue 135: the prescan walked the file once, under its
/// first module, and the second module's call was "cannot find macro"), and its `module_path!()`
/// names each module, not the file's first (relax PR 148's case: the file has one parse). rustc
/// prints "value 3 value 3 one_file_two_modules::first one_file_two_modules::second".
#[test]
fn one_file_declared_as_two_modules_imports_a_macro_in_both() {
    let ws = std::env::temp_dir().join(format!("one-file-two-modules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[package]\nname = \"one-file-two-modules\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nmacro-dep = { path = \"dep\" }\n\n[workspace]\n"),
        ("dep/Cargo.toml", "[package]\nname = \"macro-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n"),
        ("dep/src/lib.rs", "pub struct Value(pub u32);\n\nimpl std::fmt::Display for Value {\n    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {\n        write!(f, \"value {}\", self.0)\n    }\n}\n\n#[macro_export]\nmacro_rules! json {\n    ($v:expr) => {\n        $crate::Value($v)\n    };\n}\n"),
        ("src/main.rs", "#[path = \"../shared/value.rs\"]\nmod first;\n#[path = \"../shared/value.rs\"]\nmod second;\n\nfn main() {\n    println!(\"{} {} {} {}\", first::value(), second::value(), first::module(), second::module());\n}\n"),
        ("shared/value.rs", "use macro_dep::{Value, json};\n\npub fn value() -> Value {\n    json!(3)\n}\n\npub fn module() -> &'static str {\n    module_path!()\n}\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_in(&ws, &["run", "--quiet", "--unattended", "one-file-two-modules"]);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(status.success() && stdout.trim() == "value 3 value 3 one_file_two_modules::first one_file_two_modules::second",
        "the macro the file imports, and module_path!, in each module: {stdout}\n{stderr}");
}

/// A module declared in an `include!`d file imports a dependency's exported macro with `use`, as
/// russh's `include!("lib_inner.rs")` declares `mod client;` (relax issue 173: "macro `warn!` is
/// not supported"; the same module declared in lib.rs ran). A `mod` in a file that a non-mod.rs
/// file includes resolves in the included file's folder: rustc 1.99.1 finds src/deep.rs, where
/// Relax looked for src/client/deep.rs. rustc prints the two warnings and 42.
#[test]
fn a_module_declared_in_an_included_file_imports_its_macros() {
    let ws = std::env::temp_dir().join(format!("included-mods-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[package]\nname = \"included-mods\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nlib1 = { path = \"lib1\" }\n\n[workspace]\n"),
        ("dep/Cargo.toml", "[package]\nname = \"macro-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n"),
        ("dep/src/lib.rs", "#[macro_export]\nmacro_rules! warn {\n    ($($t:tt)*) => { eprintln!($($t)*) };\n}\n"),
        ("lib1/Cargo.toml", "[package]\nname = \"lib1\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nmacro-dep = { path = \"../dep\" }\n"),
        ("lib1/src/lib.rs", "include!(\"inner.rs\");\n"),
        ("lib1/src/inner.rs", "pub mod client;\n"),
        ("lib1/src/client.rs", "use macro_dep::warn;\n\ninclude!(\"client_more.rs\");\n\npub async fn connect<R: Send + 'static>(r: R, id: &str) -> Result<R, String> {\n    if id.is_empty() {\n        warn!(\"empty id {:?}\", id);\n    }\n    Ok(r)\n}\n"),
        ("lib1/src/client_more.rs", "pub mod deep;\n"),
        ("lib1/src/deep.rs", "use macro_dep::warn;\n\npub fn one() -> u32 {\n    warn!(\"deep {}\", 1);\n    1\n}\n"),
        ("src/main.rs", "use std::future::Future;\nuse std::task::{Context, Poll, Waker};\n\nfn block_on<F: Future>(f: F) -> F::Output {\n    let mut f = std::pin::pin!(f);\n    let mut cx = Context::from_waker(Waker::noop());\n    loop {\n        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {\n            return v;\n        }\n    }\n}\n\nfn main() {\n    let got = block_on(lib1::client::connect(41u32, \"\")).expect(\"connect\") + lib1::client::deep::one();\n    println!(\"{}\", got);\n}\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_in(&ws, &["run", "--quiet", "--unattended", "included-mods"]);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(status.success() && stdout.trim() == "42" && stderr.contains("empty id \"\"") && stderr.contains("deep 1"),
        "the included file's modules and their imported macro: {stdout}\n{stderr}");
}

/// A module a macro declares (`decl!(leaf)` expanding to `mod leaf;`) imports a macro another
/// module of the crate re-exports and calls it: by its imported name, through `super::`, through
/// an imported module's path, and from a file module of its own (relax issue 183: "macro `h3!` is
/// not supported"; the same `mod leaf;` written in main.rs ran). The prescan reads only the files
/// of the `mod` items it sees, and the file's own imports were left to it. rustc prints 32.
#[test]
fn a_module_a_macro_declares_imports_macros_from_another_module() {
    let ws = std::env::temp_dir().join(format!("macro-declared-mod-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[package]\nname = \"macro-declared-mod\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n"),
        ("src/main.rs", "mod macros {\n    macro_rules! h3 { () => { 8u32 } }\n    pub(crate) use h3;\n}\n\nmacro_rules! decl {\n    ($($n:ident),*) => { $(mod $n;)* };\n}\n\ndecl!(by_name, by_super, by_module, nested);\n\nfn main() {\n    println!(\"{}\", by_name::v() + by_super::v() + by_module::v() + nested::v());\n}\n"),
        ("src/by_name.rs", "use crate::macros::h3;\n\npub fn v() -> u32 {\n    h3!()\n}\n"),
        ("src/by_super.rs", "use super::macros::h3;\n\npub fn v() -> u32 {\n    h3!()\n}\n"),
        ("src/by_module.rs", "use crate::macros as m;\n\npub fn v() -> u32 {\n    m::h3!()\n}\n"),
        ("src/nested.rs", "mod inner;\n\npub fn v() -> u32 {\n    inner::v()\n}\n"),
        ("src/nested/inner.rs", "use crate::macros::h3;\n\npub fn v() -> u32 {\n    h3!()\n}\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_in(&ws, &["run", "--quiet", "--unattended", "macro-declared-mod"]);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(status.success() && stdout.trim() == "32", "the macro each macro-declared module imports: {stdout}\n{stderr}");
}

/// An integration test's `CARGO_BIN_EXE_<bin>` is the package's bin built with the features the
/// test is built with, as cargo builds every target of a package with its one feature set: a bin
/// whose `required-features` the run selects, and a default feature `--no-default-features` turns
/// off. (The runner's bin ran with no features: "target `gated` requires the features: `fancy`".)
#[test]
fn a_tests_bin_is_built_with_the_tests_features() {
    let ws = std::env::temp_dir().join(format!("bin-features-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[package]\nname = \"bin-features\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[features]\ndefault = [\"extra\"]\nextra = []\nfancy = []\n\n[[bin]]\nname = \"gated\"\npath = \"src/bin/gated.rs\"\nrequired-features = [\"fancy\"]\n\n[[test]]\nname = \"run_bin\"\nrequired-features = [\"fancy\"]\n\n[workspace]\n"),
        ("src/lib.rs", "pub fn name() -> &'static str {\n    if cfg!(feature = \"extra\") { \"fancy extra\" } else { \"fancy\" }\n}\n"),
        ("src/bin/gated.rs", "#[cfg(feature = \"fancy\")]\nfn main() {\n    println!(\"{}\", bin_features::name());\n}\n"),
        ("tests/run_bin.rs", "#[test]\nfn the_bin_has_the_tests_features() {\n    let out = std::process::Command::new(env!(\"CARGO_BIN_EXE_gated\")).output().unwrap();\n    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();\n    assert_eq!(got, bin_features::name(), \"stderr: {}\", String::from_utf8_lossy(&out.stderr));\n}\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let with_default = run_in(&ws, &["test", "bin-features", "--features", "fancy", "--test", "run_bin"]);
    let without_default = run_in(&ws, &["test", "bin-features", "--no-default-features", "--features", "fancy", "--test", "run_bin"]);
    let _ = std::fs::remove_dir_all(&ws);
    for (what, (status, stdout, stderr)) in [("--features fancy", with_default), ("--no-default-features --features fancy", without_default)] {
        assert!(status.success() && stdout.contains("the_bin_has_the_tests_features ... ok"), "{what}: the bin runs with the test's features: {stdout}\n{stderr}");
    }
}

/// `makepad test` runs the unit tests of an example that says `test = true`, beside the lib's, as
/// `cargo test` does, and `--examples` those of every example; a discovered example without it
/// stays out of the default run. Relax issue 120: no example's tests ran.
#[test]
fn an_examples_tests_run_when_it_says_test_true() {
    let ws = std::env::temp_dir().join(format!("example-tests-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[package]\nname = \"ex-tests\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[[example]]\nname = \"demo\"\ntest = true\n\n[workspace]\n"),
        ("src/lib.rs", "pub fn two() -> u32 {\n    2\n}\n\n#[test]\nfn lib_two() {\n    assert_eq!(two(), 2);\n}\n"),
        ("examples/demo.rs", "fn main() {\n    println!(\"{}\", ex_tests::two());\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn demo_runs() {\n        assert_eq!(ex_tests::two(), 2);\n    }\n}\n"),
        ("examples/other.rs", "fn main() {}\n\n#[test]\nfn other_runs() {}\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let default = run_in(&ws, &["test", "ex-tests"]);
    let examples = run_in(&ws, &["test", "ex-tests", "--examples"]);
    let _ = std::fs::remove_dir_all(&ws);
    let (status, stdout, stderr) = default;
    assert!(status.success() && stdout.contains("lib_two ... ok"), "the lib's tests: {stdout}\n{stderr}");
    assert!(stdout.contains("demo_runs ... ok"), "the `test = true` example's tests run by default: {stdout}\n{stderr}");
    assert!(!stdout.contains("other_runs"), "an example without `test = true` is not tested by default: {stdout}");
    let (status, stdout, stderr) = examples;
    assert!(status.success() && stdout.contains("demo_runs ... ok") && stdout.contains("other_runs ... ok"), "--examples tests every example: {stdout}\n{stderr}");
    assert!(!stdout.contains("lib_two"), "--examples selects the examples alone: {stdout}");
}

/// Checkouts of one repository share a run dir under RELAX_SHARE (vfs.rs SHARE): both are served
/// under the share's one virtual root (their manifest dir), and a build script's output is kept per
/// content of the inputs it declared, here a path under its current folder. Checkout b, whose data
/// file differs, runs the script into an output of its own and prints its own data; checkout a's
/// next run takes a's output again without running the script. Under cargo each checkout prints its
/// own `data` line. The workspace is written here, twice.
#[test]
fn checkouts_of_a_shared_workspace_keep_their_own_script_outputs() {
    let base = std::env::temp_dir().join(format!("shared-checkouts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let files = [
        ("Cargo.toml", "[package]\nname = \"share-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\nbuild = \"build.rs\"\n\n[workspace]\n"),
        ("build.rs", "fn main() {\n    let data = std::env::current_dir().unwrap().join(\"data.txt\");\n    println!(\"cargo:rerun-if-changed={}\", data.display());\n    let text = std::fs::read_to_string(&data).unwrap();\n    let out = std::path::Path::new(&std::env::var(\"OUT_DIR\").unwrap()).join(\"data.rs\");\n    std::fs::write(out, format!(\"pub const DATA: &str = {:?};\\n\", text.trim())).unwrap();\n}\n"),
        ("src/main.rs", "include!(concat!(env!(\"OUT_DIR\"), \"/data.rs\"));\n\nfn main() {\n    println!(\"data {DATA}\");\n    println!(\"dir {}\", env!(\"CARGO_MANIFEST_DIR\"));\n}\n"),
    ];
    for (checkout, data) in [("a", "one"), ("b", "two")] {
        for (path, text) in files.iter().copied().chain([("data.txt", data)]) {
            let path = base.join(checkout).join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }
    let run = |checkout: &str| {
        let (mut command, _) = runner_in(&base.join(checkout));
        let output = command
            .args(["run", "--quiet", "share-app"])
            .env("RELAX_SHARE", "share-test")
            .env("RELAX_PROFILE", "1")
            .env("RELAX_CACHE_DIR", base.join("run/cache"))
            .env("RELAX_TARGET_DIR", base.join("run/target"))
            .output()
            .unwrap();
        (output.status, String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    let runs = [run("a"), run("b"), run("a")];
    let _ = std::fs::remove_dir_all(&base);
    for ((status, stdout, stderr), (data, script)) in runs.iter().zip([("one", "runs"), ("two", "runs"), ("one", "cached")]) {
        let ran = stderr.lines().find(|l| l.starts_with("relax build-script: share-app ")).unwrap_or("no script line");
        assert!(status.success() && stdout.lines().any(|l| l == format!("data {data}")), "data {data}: {stdout}\n{ran}\n{stderr}");
        assert!(ran.starts_with(&format!("relax build-script: share-app {script}")), "the script {script}: {ran}");
        assert!(stdout.lines().any(|l| l.starts_with("dir ") && l.ends_with("/vfs/share/share-test")), "the share's root, not a checkout: {stdout}");
    }
}

/// A shared workspace's script output that is its checkout's own is not another checkout's (relax
/// review B-M8): one script prints its folder on disk as a `rustc-env`, one writes it into a file in
/// its OUT_DIR, one reads the commit in the repository's git metadata and declares `.git/HEAD`,
/// which reads the same in every clone. A script sees its package in the share's folder; the folder
/// on disk comes from a child, which gets no mount (`pwd -P` here, `git rev-parse --show-toplevel`
/// in the review's case). Every input they declare is the same in both checkouts, so their outputs
/// were shared: checkout b printed a's folders and commit. Under cargo each checkout prints its
/// own; a's next run takes a's outputs again without running the scripts. The workspace is written
/// here, twice.
#[test]
fn a_script_output_that_names_its_checkout_stays_with_it() {
    let base = std::env::temp_dir().join(format!("shared-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let member = |name: &str, script: &str, lib: &str| {
        [
            (format!("{name}/Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\nbuild = \"build.rs\"\n")),
            (format!("{name}/build.rs"), format!("fn main() {{\n    println!(\"cargo:rerun-if-changed=build.rs\");\n{script}}}\n")),
            (format!("{name}/src/lib.rs"), lib.to_string()),
        ]
    };
    let mut files: Vec<(String, String)> = vec![
        ("Cargo.toml".into(), "[package]\nname = \"own-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nnamed = { path = \"named\" }\ngenerated = { path = \"generated\" }\ncommit = { path = \"commit\" }\n\n[workspace]\n".into()),
        ("src/main.rs".into(), "fn main() {\n    println!(\"named {}\", named::DIR);\n    println!(\"generated {}\", generated::DIR);\n    println!(\"commit {}\", commit::COMMIT);\n}\n".into()),
        (".git/HEAD".into(), "ref: refs/heads/main\n".into()),
    ];
    let disk = "    let disk = String::from_utf8(std::process::Command::new(\"pwd\").arg(\"-P\").output().unwrap().stdout).unwrap();\n";
    files.extend(member("named", &format!("{disk}    println!(\"cargo:rustc-env=NAMED_DIR={{}}\", disk.trim());\n"), "pub const DIR: &str = env!(\"NAMED_DIR\");\n"));
    files.extend(member("generated", &format!("{disk}    let out = std::path::Path::new(&std::env::var(\"OUT_DIR\").unwrap()).join(\"dir.rs\");\n    std::fs::write(out, format!(\"pub const DIR: &str = {{:?}};\\n\", disk.trim())).unwrap();\n"), "include!(concat!(env!(\"OUT_DIR\"), \"/dir.rs\"));\n"));
    files.extend(member("commit", "    let here = std::env::current_dir().unwrap();\n    let git = here.parent().unwrap().join(\".git\");\n    println!(\"cargo:rerun-if-changed={}\", git.join(\"HEAD\").display());\n    let head = std::fs::read_to_string(git.join(\"HEAD\")).unwrap();\n    let commit = std::fs::read_to_string(git.join(head.trim().trim_start_matches(\"ref: \"))).unwrap();\n    println!(\"cargo:rustc-env=COMMIT={}\", commit.trim());\n", "pub const COMMIT: &str = env!(\"COMMIT\");\n"));
    for (checkout, commit) in [("a", "1111"), ("b", "2222")] {
        for (path, text) in files.iter().map(|(p, t)| (p.as_str(), t.as_str())).chain([(".git/refs/heads/main", commit)]) {
            let path = base.join(checkout).join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }
    let run = |checkout: &str| {
        let (mut command, _) = runner_in(&base.join(checkout));
        let output = command
            .args(["run", "--quiet", "own-app"])
            .env("RELAX_SHARE", "share-own")
            .env("RELAX_PROFILE", "1")
            .env("RELAX_CACHE_DIR", base.join("run/cache"))
            .env("RELAX_TARGET_DIR", base.join("run/target"))
            .output()
            .unwrap();
        (output.status, String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    let runs = [run("a"), run("b"), run("a")];
    for ((status, stdout, stderr), (checkout, commit, script)) in runs.iter().zip([("a", "1111", "runs"), ("b", "2222", "runs"), ("a", "1111", "cached")]) {
        let dir = std::fs::canonicalize(base.join(checkout)).unwrap();
        let dir = dir.to_string_lossy();
        assert!(status.success(), "checkout {checkout}: {stdout}\n{stderr}");
        for (what, want) in [("named", format!("{dir}/named")), ("generated", format!("{dir}/generated")), ("commit", commit.to_string())] {
            assert!(stdout.lines().any(|l| l == format!("{what} {want}")), "checkout {checkout}'s own {what} ({want}): {stdout}");
            let ran = stderr.lines().find(|l| l.starts_with(&format!("relax build-script: {what} "))).unwrap_or("no script line");
            assert!(ran.starts_with(&format!("relax build-script: {what} {script}")), "checkout {checkout}, the {what} script {script}: {ran}");
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// RELAX_SHARE in a folder of no cargo workspace shares nothing: the loader mounts before it knows
/// the command, and `--version` and `inspect list` failed there with "no Cargo.toml" (a review of
/// relax PR 57). Both answer, as without the variable. They run in the file system's root: a temp
/// folder can lie inside a workspace.
#[test]
fn a_share_outside_any_workspace_still_answers_control_commands() {
    let root = std::env::temp_dir().ancestors().last().unwrap().to_path_buf();
    for words in [&["--version"][..], &["inspect", "list", "--json"][..]] {
        let (mut command, scratch) = runner_in(std::path::Path::new("share-control"));
        let output = command.current_dir(&root).args(words).env("RELAX_SHARE", "share-control").output().unwrap();
        let _ = std::fs::remove_dir_all(&scratch);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{words:?} with RELAX_SHARE outside a workspace: {stderr}");
    }
}

/// A program that starts its own executable runs itself again, whatever the first argument: under
/// the JIT its executable is its own hard link of the loader (relax-compiler selfexe.rs), so `myapp
/// serve` and `myapp help` reach the app, not the loader's `serve` and `help` commands (as they did
/// while the rule went by the first word), and argv[0] names that same file.
#[test]
fn a_program_that_restarts_itself_with_a_loader_word_runs_itself() {
    let ws = std::env::temp_dir().join(format!("self-serve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let main = r#"fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "serve" || a == "help") {
        println!("app {}", args[1..].join(" "));
        return;
    }
    let exe = std::env::current_exe().unwrap();
    for words in [&["serve", "--port", "8080"][..], &["help"][..]] {
        let out = std::process::Command::new(&exe).args(words).output().unwrap();
        print!("{}", String::from_utf8_lossy(&out.stdout));
    }
    let same = std::fs::canonicalize(&args[0]).unwrap() == std::fs::canonicalize(&exe).unwrap();
    println!("argv[0] is current_exe: {same}");
}
"#;
    let files = [("Cargo.toml", "[package]\nname = \"self-serve\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n"), ("src/main.rs", main)];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_until(&ws, &["run", "--quiet", "--unattended", "self-serve"], &[], std::time::Duration::from_secs(120));
    let _ = std::fs::remove_dir_all(&ws);
    assert!(status.is_some_and(|s| s.success()), "the run ends: {stdout}\n{stderr}");
    assert_eq!(stdout, "app serve --port 8080\napp help\nargv[0] is current_exe: true\n", "{stderr}");
}

/// A run's arguments and environment values need not be Unicode: the program gets them as the OS
/// gave them (a byte 0xff on Unix, an unpaired surrogate on Windows). Relax issue 159: the flight
/// recorder read them as Strings when a run started (`std::env::args`, `std::env::vars`, which
/// panic on such a value, whatever its name), so the run stopped before main with exit 101.
#[test]
fn a_programs_arguments_and_environment_need_not_be_unicode() {
    #[cfg(unix)]
    let not_unicode = |bytes: &[u8]| -> std::ffi::OsString { std::os::unix::ffi::OsStringExt::from_vec(bytes.to_vec()) };
    #[cfg(windows)]
    let not_unicode = |bytes: &[u8]| -> std::ffi::OsString {
        let wide: Vec<u16> = bytes.iter().map(|&b| if b < 0x80 { b as u16 } else { 0xd800 }).collect();
        std::os::windows::ffi::OsStringExt::from_wide(&wide)
    };
    let ws = std::env::temp_dir().join(format!("launch-bytes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let main = r#"fn main() {
    let arg = std::env::args_os().nth(1).expect("an argument");
    let value = std::env::var_os("NON_UNICODE_VALUE").expect("a value");
    let recorded = std::env::var_os("MAKEPAD_NON_UNICODE").expect("a MAKEPAD_ value");
    println!("arg {arg:?} unicode {}", arg.to_str().is_some());
    println!("value {value:?} {recorded:?}");
}
"#;
    let files = [("Cargo.toml", "[package]\nname = \"launch-bytes\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n"), ("src/main.rs", main)];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (arg, value, recorded) = (not_unicode(b"\xff"), not_unicode(b"a\xfeb"), not_unicode(b"\xfe"));
    let (mut command, scratch) = runner_in(&ws);
    let output = command
        .args(["run", "--quiet", "--unattended", "launch-bytes", "--"])
        .arg(&arg)
        .env("NON_UNICODE_VALUE", &value)
        .env("MAKEPAD_NON_UNICODE", &recorded)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_dir_all(&ws);
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "the run ends: {:?}\n{stdout}\n{stderr}", output.status);
    assert_eq!(stdout, format!("arg {arg:?} unicode false\nvalue {value:?} {recorded:?}\n"), "{stderr}");
}

/// A package's run-env (`[package.metadata.relax.run-env]`) is its program's: a name that steers
/// Relax or the tools it runs (RELAX_ALLOW_C, LD_PRELOAD) stops the run before anything is built,
/// with an error naming it and the manifest. An accepted value reaches the program and its
/// children, and neither the loader nor a build script it runs; a provided crate's code reads it
/// through relax-api's env (the program's environment), not through std::env (the loader process's).
/// `--allow-c` is the invocation's: a
/// child the program starts (a nested `$RELAX_LOADER run` gets the same environment) does not
/// inherit it. Found by a security review of the runner: a manifest granted itself `--allow-c`
/// (the runner set run-env in its own process before the session read its options).
#[cfg(unix)]
#[test]
fn a_packages_run_env_is_its_programs_and_grants_nothing() {
    let ws = std::env::temp_dir().join(format!("run-env-grants-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let manifest = |name: &str, env: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[package.metadata.relax.run-env]\n{env}\n[workspace]\n");
    // (the provided fixture's ops run the loader's rustc build: `program_var` reads relax-api's
    // env, the program's environment; `host_var` std::env, the loader process's)
    let main = r#"fn main() {
    if std::env::args().nth(1).as_deref() == Some("--again") {
        println!("again bound {}", relax_provided_fixture::program_var("APP_MODE"));
        return;
    }
    println!("mode {}", std::env::var("APP_MODE").unwrap_or_default());
    println!("bound {}", relax_provided_fixture::program_var("APP_MODE"));
    println!("host {}", relax_provided_fixture::host_var("APP_MODE"));
    println!("script {}", env!("SCRIPT_SAW"));
    let out = std::process::Command::new("env").output().unwrap();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.starts_with("APP_MODE=") || line.starts_with("RELAX_ALLOW_C=") {
            println!("child {line}");
        }
    }
    let again = std::process::Command::new(std::env::current_exe().unwrap()).arg("--again").output().unwrap();
    print!("{}", String::from_utf8_lossy(&again.stdout));
}
"#;
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixture").canonicalize().unwrap();
    let provided = format!("\n[dependencies]\nrelax-provided-fixture = {{ path = \"{}\" }}\n\n[workspace]\n", fixture.display());
    let script = "fn main() {\n    println!(\"cargo:rerun-if-env-changed=APP_MODE\");\n    println!(\"cargo:rustc-env=SCRIPT_SAW={}\", std::env::var(\"APP_MODE\").unwrap_or_else(|_| \"none\".into()));\n}\n";
    let files = [
        ("refused/Cargo.toml", manifest("run-env-refused", "RELAX_ALLOW_C = \"1\"\nLD_PRELOAD = \"./inject.so\"\n")),
        ("refused/src/main.rs", "fn main() {\n    println!(\"ran\");\n}\n".to_string()),
        ("granted/Cargo.toml", manifest("run-env-granted", "APP_MODE = \"demo\"\n").replace("publish = false\n", "publish = false\nbuild = \"build.rs\"\n").replace("\n[workspace]\n", &provided)),
        ("granted/build.rs", script.to_string()),
        ("granted/src/main.rs", main.to_string()),
    ];
    for (path, text) in &files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let refused = run_in(&ws.join("refused"), &["run", "--quiet", "run-env-refused"]);
    let granted = run_in(&ws.join("granted"), &["run", "--quiet", "--allow-c", "run-env-granted"]);
    let _ = std::fs::remove_dir_all(&ws);
    let (status, stdout, stderr) = refused;
    assert_eq!(status.code(), Some(2), "a run-env that sets RELAX_ALLOW_C stops the run: {stdout}\n{stderr}");
    assert!(!stdout.contains("ran"), "the refused package's program ran: {stdout}");
    assert!(stderr.contains("refused/Cargo.toml: [package.metadata.relax.run-env] sets RELAX_ALLOW_C,"), "the error names the manifest and the key: {stderr}");
    let (status, stdout, stderr) = granted;
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "mode demo\nbound rustc demo\nhost rustc none\nscript none\nchild APP_MODE=demo\nagain bound rustc demo\n", "the program, a provided crate's read of the program's environment, its child and its run again read the run-env; the host's environment, the build script and the child's --allow-c do not: {stderr}");
    assert!(stderr.contains("run-env-granted's run-env") && stderr.contains("gives the program APP_MODE=demo"), "the run says what its run-env gave: {stderr}");
}

/// A program that relaunches itself through a shell without RELAX_LOADER (a cleared environment)
/// or with RELAX_LOADER naming its own executable runs again, and its spawn of `$RELAX_LOADER`
/// starts the loader, not the program a third time. Found by a security review of the runner: the
/// relaunched run took its own executable (a run's link of the loader) for the loader and exported
/// it, so each spawn of "the loader" ran the program again, without end, or waited for a build
/// script's claim its parent held. The program answers `--version` itself, so the run ends without
/// the fix too. The run is started with RELAX_LOADER naming its loader, as a program under the
/// loader starts one (`$RELAX_LOADER run`, as this test does under `makepad test`): its build
/// environment named the loader, and the cleared relaunch, handed it, hid the loader its start
/// found from the program ("cleared: no loader" in the post-land tests only).
#[cfg(unix)]
#[test]
fn a_relaunched_runs_loader_is_the_loader_not_its_own_executable() {
    let ws = std::env::temp_dir().join(format!("relaunch-loader-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let main = r#"fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--version") => println!("program again"),
        Some("--worker") => {
            let label = &args[2];
            let Some(loader) = std::env::var_os("RELAX_LOADER") else {
                println!("{label}: no loader");
                return;
            };
            let out = std::process::Command::new(loader).arg("--version").output().unwrap();
            let program = String::from_utf8_lossy(&out.stdout).starts_with("program again");
            println!("{label}: the loader answers as the {}", if program { "program" } else { "loader" });
        }
        _ => {
            let exe = std::env::current_exe().unwrap();
            for (label, script) in [("cleared", "unset RELAX_LOADER; exec \"$0\" --worker cleared"), ("named", "RELAX_LOADER=\"$0\"; export RELAX_LOADER; exec \"$0\" --worker named")] {
                let out = std::process::Command::new("/bin/sh").args(["-c", script]).arg(&exe).output().unwrap();
                print!("{}", String::from_utf8_lossy(&out.stdout));
                eprint!("{label} {:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
            }
        }
    }
}
"#;
    let files = [
        ("Cargo.toml", "[package]\nname = \"relaunch-loader\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n"),
        ("src/main.rs", main),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (mut command, scratch) = runner_in(&ws);
    let out = command.args(["run", "--quiet", "relaunch-loader"]).env("RELAX_LOADER", runner()).output().unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_dir_all(&ws);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "cleared: the loader answers as the loader\nnamed: the loader answers as the loader\n", "a relaunched run's RELAX_LOADER is the loader: {stderr}");
}

/// A `links` package's build script hands its metadata on: the scripts of the packages that depend
/// on it directly run after it and read it as `DEP_<LINKS>_<KEY>` (`cargo:KEY=VALUE` with a key
/// that is no instruction, and `cargo::metadata=KEY=VALUE`); a package further down gets none, nor
/// does a package's script from its dev-dependencies (its tests' own, after the script). A
/// check without `--allow-c` does not run a script that reads the metadata of a script it skips
/// (one that compiles C) and says so. Relax issue 79: no script was given DEP_ variables, so one
/// that reads them failed, and the check only said the script failed (aws-lc-rs). The workspace is
/// written here: lm-sys has `links = "lm-native"`, lm-mid depends on it and lm-app on lm-mid, and
/// each of the latter two scripts lists the DEP_ variables it sees; lm-app's unit test asserts its
/// script saw none of lm-devsys's, a `links` dev-dependency. lm-csys has `links` and the
/// build-dependency `cc` (lm-cc, a stand-in that compiles nothing); lm-reader's script reads its
/// include folder. Under cargo `lm-app` prints the line asserted below and its test passes.
#[test]
fn a_links_scripts_metadata_reaches_its_dependents_scripts() {
    let ws = std::env::temp_dir().join(format!("links-metadata-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let pkg = |name: &str, extra: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\nbuild = \"build.rs\"\n{extra}");
    let list_deps = |var: &str| format!("fn main() {{\n    let mut deps: Vec<String> = std::env::vars().filter(|(k, _)| k.starts_with(\"DEP_\")).map(|(k, v)| format!(\"{{k}}={{v}}\")).collect();\n    deps.sort();\n    println!(\"cargo:rustc-env={var}={{}}\", deps.join(\" \"));\n}}\n");
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"cc\", \"sys\", \"mid\", \"app\", \"devsys\", \"csys\", \"reader\"]\nresolver = \"2\"\n".to_string()),
        ("cc/Cargo.toml", "[package]\nname = \"lm-cc\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n".to_string()),
        ("cc/src/lib.rs", "pub fn compile(_source: &str) {}\n".to_string()),
        ("sys/Cargo.toml", pkg("lm-sys", "links = \"lm-native\"\n")),
        (
            "sys/build.rs",
            "fn main() {\n    println!(\"cargo:include=/lm/include\");\n    println!(\"cargo::metadata=root=/lm/root\");\n    println!(\"cargo:version-tag=1.2\");\n    println!(\"cargo:rustc-check-cfg=cfg(lm_sys_built)\");\n    println!(\"cargo:rustc-cfg=lm_sys_built\");\n}\n".to_string(),
        ),
        ("sys/src/lib.rs", "pub fn built() -> bool {\n    cfg!(lm_sys_built)\n}\n".to_string()),
        ("mid/Cargo.toml", pkg("lm-mid", "\n[dependencies]\nlm-sys = { path = \"../sys\" }\n")),
        ("mid/build.rs", list_deps("LM_MID_DEPS")),
        ("mid/src/lib.rs", "pub const DEPS: &str = env!(\"LM_MID_DEPS\");\npub use lm_sys::built;\n".to_string()),
        ("app/Cargo.toml", pkg("lm-app", "\n[dependencies]\nlm-mid = { path = \"../mid\" }\n\n[dev-dependencies]\nlm-devsys = { path = \"../devsys\" }\n")),
        ("app/build.rs", list_deps("LM_APP_DEPS")),
        (
            "app/src/main.rs",
            "fn main() {\n    println!(\"mid [{}] app [{}] built {}\", lm_mid::DEPS, env!(\"LM_APP_DEPS\"), lm_mid::built());\n}\n\n#[test]\nfn no_dev_dependency_metadata() {\n    assert_eq!(env!(\"LM_APP_DEPS\"), \"\", \"lm-devsys is a dev-dependency\");\n}\n".to_string(),
        ),
        ("devsys/Cargo.toml", pkg("lm-devsys", "links = \"lm-dev\"\n")),
        ("devsys/build.rs", "fn main() {\n    println!(\"cargo:include=/lm-dev/include\");\n}\n".to_string()),
        ("devsys/src/lib.rs", "".to_string()),
        ("csys/Cargo.toml", pkg("lm-csys", "links = \"lm-c\"\n\n[build-dependencies]\ncc = { path = \"../cc\", package = \"lm-cc\" }\n")),
        ("csys/build.rs", "fn main() {\n    cc::compile(\"c/lm.c\");\n    println!(\"cargo:include=/lm-c/include\");\n}\n".to_string()),
        ("csys/src/lib.rs", "pub fn seven() -> u32 {\n    7\n}\n".to_string()),
        ("reader/Cargo.toml", pkg("lm-reader", "\n[dependencies]\nlm-csys = { path = \"../csys\" }\n")),
        (
            "reader/build.rs",
            "fn main() {\n    let include = std::env::var(\"DEP_LM_C_INCLUDE\").expect(\"DEP_LM_C_INCLUDE\");\n    println!(\"cargo:rustc-check-cfg=cfg(lm_c_include)\");\n    if !include.is_empty() {\n        println!(\"cargo:rustc-cfg=lm_c_include\");\n    }\n}\n".to_string(),
        ),
        ("reader/src/main.rs", "fn main() {\n    println!(\"{}\", lm_csys::seven());\n}\n".to_string()),
    ];
    for (path, text) in &files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let run = run_in(&ws, &["run", "--quiet", "lm-app"]);
    let test = run_in(&ws, &["test", "lm-app"]);
    let check = run_in(&ws, &["check", "lm-reader"]);
    let _ = std::fs::remove_dir_all(&ws);
    let (status, stdout, stderr) = run;
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "mid [DEP_LM_NATIVE_INCLUDE=/lm/include DEP_LM_NATIVE_ROOT=/lm/root DEP_LM_NATIVE_VERSION_TAG=1.2] app [] built true", "lm-sys's metadata reaches lm-mid's script only: {stderr}");
    let (status, stdout, stderr) = test;
    assert!(status.success() && format!("{stdout}{stderr}").contains("1 passed; 0 failed"), "lm-app's script saw no metadata of its dev-dependency: {stdout}\n{stderr}");
    let (status, stdout, stderr) = check;
    assert!(status.success(), "the check goes on: {stdout}\n{stderr}");
    assert!(
        stderr.contains("the build script of `lm-reader` was not run: it reads the `DEP_` metadata of `lm-csys` (links = \"lm-c\"), and its build script compiles C, which is not run without `--allow-c`"),
        "the check says why lm-reader's script did not run: {stderr}"
    );
    assert!(!stderr.contains("build script failed"), "lm-reader's script ran without the metadata: {stderr}");
}

/// A package reached at the top of a run and inside a build script's run (a dependency of the app
/// and a build-dependency of the app's script) runs its build script once: the output made at one
/// depth stands at the other. Relax issue 69: the script's rerun key held the nesting depth and
/// chain, so the second reach ran the script again. The workspace is written here: sd-leaf's script
/// counts its runs in its OUT_DIR and hands the count on, sd-app's script reads it inside its own
/// run and sd-app's `main` at the top. Under cargo `sd-app` prints `leaf runs 1 script saw 1`.
#[test]
fn a_script_reached_at_two_depths_runs_once() {
    let ws = std::env::temp_dir().join(format!("script-two-depths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let pkg = |name: &str, extra: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\nbuild = \"build.rs\"\n{extra}");
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"leaf\", \"app\"]\nresolver = \"2\"\n".to_string()),
        ("leaf/Cargo.toml", pkg("sd-leaf", "")),
        (
            "leaf/build.rs",
            "fn main() {\n    let runs = format!(\"{}/runs\", std::env::var(\"OUT_DIR\").unwrap());\n    let n = std::fs::read_to_string(&runs).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) + 1;\n    std::fs::write(&runs, n.to_string()).unwrap();\n    println!(\"cargo:rerun-if-changed=build.rs\");\n    println!(\"cargo:rustc-env=SD_LEAF_RUNS={n}\");\n}\n".to_string(),
        ),
        ("leaf/src/lib.rs", "pub const RUNS: &str = env!(\"SD_LEAF_RUNS\");\n".to_string()),
        ("app/Cargo.toml", pkg("sd-app", "\n[dependencies]\nsd-leaf = { path = \"../leaf\" }\n\n[build-dependencies]\nsd-leaf = { path = \"../leaf\" }\n")),
        ("app/build.rs", "fn main() {\n    println!(\"cargo:rustc-env=SD_SCRIPT_SAW={}\", sd_leaf::RUNS);\n}\n".to_string()),
        ("app/src/main.rs", "fn main() {\n    println!(\"leaf runs {} script saw {}\", sd_leaf::RUNS, env!(\"SD_SCRIPT_SAW\"));\n}\n".to_string()),
    ];
    for (path, text) in &files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_in(&ws, &["run", "--quiet", "sd-app"]);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "leaf runs 1 script saw 1", "sd-leaf's script ran once for both depths: {stderr}");
}

/// A crate that does not load stops the run before `main`, and the run says so: the exit read as
/// the program's own. The workspace is written here: an app and a path dependency with an unclosed
/// delimiter, which `main` never calls.
#[test]
fn a_load_error_says_it_stops_the_run_before_main() {
    let ws = std::env::temp_dir().join(format!("load-error-stops-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"app\", \"broken\"]\nresolver = \"2\"\n"),
        ("app/Cargo.toml", "[package]\nname = \"load-error-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbroken = { path = \"../broken\" }\n"),
        ("app/src/main.rs", "fn main() {\n    println!(\"main ran\");\n}\n"),
        ("broken/Cargo.toml", "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("broken/src/lib.rs", "pub fn f( {}\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_in(&ws, &["run", "--quiet", "load-error-app"]);
    let _ = std::fs::remove_dir_all(&ws);
    assert_eq!(status.code(), Some(1), "the exit stays 1: {stdout}\n{stderr}");
    assert!(!stdout.contains("main ran"), "main ran: {stdout}\n{stderr}");
    assert!(stderr.contains("load error broken"), "the load error is named: {stderr}");
    assert!(stderr.contains("1 crate did not load (the load errors above), so the run stops before `main`"), "the run says why it ends: {stderr}");
}

/// A build script that does not compile fails the run with its own error: the cause the run gives
/// was the summary that ends the script's compile (`error: could not compile ..: not started`),
/// which names none (relax PR 59). The workspace is written here: an app whose build.rs assigns a
/// `&str` to a `u8`.
#[test]
fn a_build_script_that_does_not_compile_names_its_error() {
    let ws = std::env::temp_dir().join(format!("script-compile-error-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let files = [
        ("Cargo.toml", "[package]\nname = \"script-error-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n\n[workspace]\n"),
        ("build.rs", "fn main() {\n    let level: u8 = \"high\";\n    println!(\"cargo:rustc-env=LEVEL={level}\");\n}\n"),
        ("src/main.rs", "fn main() {\n    println!(\"main ran {}\", env!(\"LEVEL\"));\n}\n"),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_in(&ws, &["run", "--quiet", "script-error-app"]);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(!status.success() && !stdout.contains("main ran"), "the run went on: {stdout}\n{stderr}");
    let failed = stderr.lines().find(|l| l.contains("build script failed")).unwrap_or_else(|| panic!("no build script failure: {stderr}"));
    assert!(failed.contains("E0308: mismatched types"), "the script's own error is the cause: {stderr}");
}

/// A fixture run is unattended: a call of code that does not compile ends the run with its error,
/// where an attended run parks for someone to fix it, and a test has nobody (a fixture's compile
/// error held the whole modules harness for 25 minutes). The fixture's main runs, then calls a
/// function of its dependency (not one of the app's crates, which a run checks whole before main)
/// that rustc refuses (E0308; libs/relax/compiler/tests/fixture_compile_error).
#[test]
fn a_fixture_whose_code_does_not_compile_fails_its_test() {
    let (status, stdout, stderr) = run_fixture("fixture_compile_error/ce_app", "ce-app");
    assert!(!status.success() && stderr.contains("main ran"), "the run starts main and ends at the error: {stdout}\n{stderr}");
    assert!(stderr.contains("E0308"), "the run names the compile error: {stderr}");
}

/// A crate a proc macro and the program both use, with different features, is two units (cargo's
/// host and target): the macro world's has the proc macro's features, and the derive those turn
/// on runs there (relax issue 53; libs/relax/compiler/tests/proc_macro_host_features).
#[test]
fn a_proc_macros_dependency_has_the_proc_macros_features() {
    let (status, stdout, stderr) = run_fixture("proc_macro_host_features", "hf-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "measured 35 base 5", "{stderr}");
}

/// A strong `dep/feature` on an optional dependency turns the dependency on and, as under cargo,
/// the package's feature named after it (relax issue 63: num's `num-rational/num-bigint-std`;
/// libs/relax/compiler/tests/dep_feature_named_feature).
#[test]
fn a_dependency_feature_turns_on_the_feature_named_after_the_dependency() {
    let (status, stdout, stderr) = run_fixture("dep_feature_named_feature", "df-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "wide 1 / wide 2", "{stderr}");
}

/// A fragment a macro_rules arm forwards reaches a hosted proc macro in an invisible group, as
/// under rustc (sqlx's query_as! reads `"sql" + $tail` and `x as _` through one), and what a macro
/// hands back of the group reads as rustc reads it (libs/relax/compiler/tests/macro_forwarded_expr_group).
#[test]
fn a_forwarded_expression_reaches_a_proc_macro_in_an_invisible_group() {
    let (status, stdout, stderr) = run_fixture("macro_forwarded_expr_group", "fwd-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "forwarded expr group ok head tail true\nhanded back: 4", "{stderr}");
}

/// A proc macro called through a path is the one the path's imports name, a rename among them
/// (`time::serde::format_description!` is time_macros' `serde_format_description`):
/// libs/relax/compiler/tests/macro_path_renamed_proc_macro.
#[test]
fn a_proc_macro_path_follows_its_imports_through_a_rename() {
    let (status, stdout, stderr) = run_fixture("macro_path_renamed_proc_macro", "rename-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "renamed proc macro ok plain tagged", "{stderr}");
}

/// A proc-macro-hack entry point (`_proc_macro_hack_<name>`, made by an attribute's expansion)
/// resolves where a declaration crate imports it, a proc macro runs with CARGO set also in a run
/// that did not come from cargo, and the `macro_rules!` the derive writes inside the declaration
/// crate's macro takes `$crate` from that crate (relax PR 84; libs/relax/compiler/tests/proc_macro_hack_entry).
/// `cargo run -p pmh-app` prints the line asserted below.
#[test]
fn a_proc_macro_hack_entry_resolves_and_proc_macros_see_cargo() {
    let (status, stdout, stderr) = run_fixture("proc_macro_hack_entry", "pmh-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "hack entry ok, CARGO set: true, answer 42", "{stderr}");
}

/// A dependency's `macro_rules!` macro named like the crate and like a private module of it,
/// imported in a use group, expands in the importer's consts (relax issue 40: "macro `zerovec!` is
/// not supported"): libs/relax/compiler/tests/macro_crate_named_xcrate.
#[test]
fn a_macro_named_like_its_crate_expands_in_the_importer() {
    let (status, stdout, stderr) = run_fixture("macro_crate_named_xcrate", "xvec-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "bmp 2 Some(65536) Some(0) all Some(1114112) empty 0", "{stderr}");
}

/// Importing a dependency's exported macro named like the crate does not import its private
/// module of that name, so the crate's path still reaches its attribute macro, and so does a
/// public module alias (relax PR 7): libs/relax/compiler/tests/attribute_macro_private_module.
#[test]
fn a_crate_named_macro_import_keeps_the_crate_for_its_attributes() {
    let (status, stdout, stderr) = run_fixture("attribute_macro_private_module", "apm-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "apm 17", "{stderr}");
}

/// A crate Relax provides (std/substitutes.txt) keeps the features the graph enables: smallvec's
/// `write` and arrayvec's default `std` give them `io::Write` (relax issue 117: the provided
/// crates were loaded without features): libs/relax/compiler/tests/substitute_features.
#[test]
fn a_provided_crate_keeps_the_features_the_graph_enables() {
    let (status, stdout, stderr) = run_fixture("substitute_features", "subfeat-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "[104, 105] 4 [104, 101, 108, 108]", "{stderr}");
}

/// An enum's discriminants cast earlier variants of the same enum, as a flags derive writes them
/// (`B = next_bit(E::A as u128) as u64`), and nothing is reported on the way (relax issue 116: the
/// cast read the tag of an enum whose layout needs those very discriminants, which failed with
/// E0080, or with E0081 when a later one saw a stand-in value):
/// libs/relax/compiler/tests/discr_casts_own_variants.
#[test]
fn an_enum_discriminant_casts_earlier_variants_of_its_enum() {
    let (status, stdout, stderr) = run_fixture("discr_casts_own_variants", "discr-casts");
    assert!(status.success(), "{stdout}\n{stderr}");
    let reported: Vec<&str> = stderr.lines().filter(|l| l.contains("error") || l.contains("warning") || l.contains(": E0")).collect();
    assert!(reported.is_empty(), "nothing is reported: {reported:?}");
    assert_eq!(stdout.trim(), "[1, 2, 4] [1, 2, 4, 8] (-3, -2, 2, -2, true) (-1, 0, 5, true) 4", "{stderr}");
}

/// A constant whose evaluation needs itself is rustc's E0391: its error line reaches stderr under
/// `--quiet` too (a run printed it only without `--quiet`), and the constant has no value, not the
/// one computed across the cycle: its use panics with the constant's own error (the discriminant
/// that needs its own enum's size printed `1` before) (libs/relax/compiler/tests/const_eval_cycle_reported).
#[test]
fn a_const_eval_cycle_prints_its_error_and_its_use_panics() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join("libs/relax/compiler/tests/const_eval_cycle_reported");
    let (status, stdout, stderr) = run_in(&dir, &["run", "--quiet", "--unattended", "cycle-reported"]);
    assert!(
        stderr.contains("E0391: cycle detected when const-evaluating `cycle_reported::E::A::{constant#0}`"),
        "the cycle's error line: {stdout}\n{stderr}"
    );
    assert!(stderr.contains("could not evaluate constant cycle_reported::E::A::{constant#0}"), "the use panics with the constant's error: {stderr}");
    assert!(stdout.trim().is_empty(), "no value is printed: {stdout}");
    assert!(!status.success(), "the panic ends the run: {stderr}");
}

/// A program's process ends as rustc's binary's does. A normal return from main, and a panic in
/// it, still write what stdout holds without a newline (relax issue 130: the tail was lost unless
/// main called process::exit). A test that runs its own binary through `/bin/sh` reaches the test
/// harness, as it does through std's Command (relax issue 139: the shell's child was Relax's
/// command line, "unknown command"), and finds its package's binary where cargo puts it, in the
/// folder above its own executable's (relax issue 139: nothing was there; on Windows, where the
/// CARGO_BIN_EXE_* launcher is a `.cmd`, no `proc-exit.exe` was there, relax PR 153). And a test's CARGO_BIN_EXE_* binary
/// that exits 2 has only its own line on stderr (relax issue 140: Relax added "session log kept
/// at ..."). The test's runs of itself run the package's build script again (it reruns on
/// SELF_CHILD), and the script runs: a run started from a test's own executable spawned its build
/// script by that executable, which ran the test program instead and waited for the build script
/// lock its parent held (the tests bucket hung, 11-13). A test that runs its own executable and
/// then has its package's binary run that executable again (FORWARD_CHILD, which the script does
/// not rerun on) reaches the test harness both times: the run of itself wrote the CARGO_BIN_EXE_*
/// launcher with its own executable as the loader, so the binary's start ran the test program,
/// which refused the loader's options (relax issue 139: "Unrecognized option: 'silent'"). The
/// binary above the test's folder has the path the test started as its argv[0], as cargo's has. A
/// test run that execs its own executable, directly or through `/bin/sh`, goes on as the test in
/// the same process, and that image's spawn of its own executable reaches the test harness again:
/// the exec keeps the run's link (an exec's new image took its pid's folder for a gone process's
/// and removed the file it ran from, so the spawn found nothing, 11-13). Under cargo both runs
/// print `tail` and `cargo test` passes.
#[test]
fn a_programs_exit_and_its_own_executable_are_as_rustcs() {
    let ws = std::env::temp_dir().join(format!("proc-exit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let main = r#"use std::io::Write;
fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("tail") => std::io::stdout().lock().write_all(b"tail").unwrap(),
        Some("argv0") => println!("{}", std::env::args().next().unwrap()),
        Some("forward") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            let status = std::process::Command::new(&args[0]).args(&args[1..]).status().unwrap();
            std::process::exit(status.code().unwrap_or(1));
        }
        Some("panic") => {
            std::io::stdout().lock().write_all(b"tail").unwrap();
            panic!("after the tail");
        }
        _ => {
            eprintln!("expected refusal");
            std::process::exit(2);
        }
    }
}
"#;
    let lib = r#"#[cfg(unix)]
#[test]
fn self_via_shell() {
    if std::env::var("SELF_CHILD").as_deref() == Ok("1") {
        println!("helper ran");
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let direct = std::process::Command::new(&exe).args(["--exact", "self_via_shell", "--nocapture"]).env("SELF_CHILD", "1").output().unwrap();
    assert!(String::from_utf8_lossy(&direct.stdout).contains("helper ran"), "direct: {}", String::from_utf8_lossy(&direct.stderr));
    let shell = std::process::Command::new("/bin/sh")
        .args(["-c", "exec \"$SELF_EXE\" --exact self_via_shell --nocapture"])
        .env("SELF_EXE", &exe)
        .env("SELF_CHILD", "1")
        .output()
        .unwrap();
    assert!(shell.status.success() && String::from_utf8_lossy(&shell.stdout).contains("helper ran"), "shell: {}", String::from_utf8_lossy(&shell.stderr));
}

#[cfg(unix)]
#[test]
fn self_after_exec() {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().unwrap();
    let args = ["--exact", "self_after_exec", "--nocapture"];
    match std::env::var("EXEC_STAGE").as_deref() {
        Ok("child") => println!("helper ran"),
        Ok("replaced") => {
            let child = std::process::Command::new(&exe).args(args).env("EXEC_STAGE", "child").output().unwrap();
            assert!(String::from_utf8_lossy(&child.stdout).contains("helper ran"), "the exec's image runs itself: {}", String::from_utf8_lossy(&child.stderr));
            println!("replaced ran");
        }
        Ok(mode) => {
            let mut command = std::process::Command::new(if mode == "shell" { std::path::Path::new("/bin/sh") } else { exe.as_path() });
            if mode == "shell" {
                command.args(["-c", "exec \"$SELF_EXE\" \"$@\"", "sh"]).env("SELF_EXE", &exe);
            }
            let error = command.args(args).env("EXEC_STAGE", "replaced").exec();
            panic!("exec returned: {error}");
        }
        Err(_) => {
            for mode in ["direct", "shell"] {
                let run = std::process::Command::new(&exe).args(args).env("EXEC_STAGE", mode).output().unwrap();
                assert!(run.status.success() && String::from_utf8_lossy(&run.stdout).contains("replaced ran"), "{mode} exec: {}", String::from_utf8_lossy(&run.stderr));
            }
        }
    }
}
"#;
    let test = r#"#[test]
fn refusal_has_only_the_programs_diagnostic() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_proc-exit")).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "expected refusal\n");
}

#[test]
fn binary_runs_the_test_after_the_test_ran_itself() {
    if std::env::var("FORWARD_CHILD").as_deref() == Ok("1") {
        println!("helper ran");
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let args = ["--exact", "binary_runs_the_test_after_the_test_ran_itself", "--nocapture"];
    let direct = std::process::Command::new(&exe).args(args).env("FORWARD_CHILD", "1").output().unwrap();
    assert!(String::from_utf8_lossy(&direct.stdout).contains("helper ran"), "direct: {}", String::from_utf8_lossy(&direct.stderr));
    let forwarded = std::process::Command::new(env!("CARGO_BIN_EXE_proc-exit")).arg("forward").arg(&exe).args(args).env("FORWARD_CHILD", "1").output().unwrap();
    assert!(forwarded.status.success() && String::from_utf8_lossy(&forwarded.stdout).contains("helper ran"), "through the binary: {}", String::from_utf8_lossy(&forwarded.stderr));
}

#[test]
fn binary_is_above_the_tests_folder() {
    let test = std::env::current_exe().unwrap();
    let binary = test.parent().unwrap().parent().unwrap().join(format!("proc-exit{}", std::env::consts::EXE_SUFFIX));
    let output = std::process::Command::new(&binary).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "expected refusal\n");
    let argv0 = std::process::Command::new(&binary).arg("argv0").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&argv0.stdout).trim_end(), binary.to_str().unwrap(), "the binary's argv[0]");
}
"#;
    let files = [
        ("Cargo.toml", "[package]\nname = \"proc-exit\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n"),
        ("build.rs", "fn main() {\n    println!(\"cargo:rerun-if-env-changed=SELF_CHILD\");\n}\n"),
        ("src/main.rs", main),
        ("src/lib.rs", lib),
        ("tests/exit.rs", test),
    ];
    for (path, text) in files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let deadline = std::time::Duration::from_secs(120);
    let tail = run_until(&ws, &["run", "--quiet", "--unattended", "proc-exit", "--", "tail"], &[], deadline);
    let panic = run_until(&ws, &["run", "--quiet", "--unattended", "proc-exit", "--", "panic"], &[], deadline);
    let test = run_until(&ws, &["test", "proc-exit"], &[], deadline);
    let _ = std::fs::remove_dir_all(&ws);
    let (status, stdout, stderr) = tail;
    assert!(status.is_some_and(|s| s.success()) && stdout == "tail", "main's return writes stdout's tail: {stdout:?}\n{stderr}");
    let (status, stdout, stderr) = panic;
    assert!(status.is_some_and(|s| s.code() == Some(101)) && stdout == "tail", "main's panic writes stdout's tail and exits 101: {status:?} {stdout:?}\n{stderr}");
    let (status, stdout, stderr) = test;
    assert!(status.is_some_and(|s| s.success()), "the shell's child is the test harness, an exec of the test's own executable keeps it, the binary is above the test's folder with its path as argv[0], the binary runs the test after the test ran itself, and its stderr is its own: {stdout}\n{stderr}");
}

/// A native build's cargo run builds the program's other packages as stubs, and runs the stub of
/// each build script it reaches: sn-plain's, a dependency of the native package. On Windows cargo
/// runs `build-script-build.exe`, which it hard-links from what rustc made,
/// `build_script_build-<hash>.exe`: the stub had no `.exe`, so the first stubbed script cargo
/// started was "never executed" (os error 2), whichever it was (relax issue 164). sn-native is
/// native by its `links`; nothing compiles C.
#[test]
fn a_native_builds_stubbed_build_scripts_run() {
    let ws = std::env::temp_dir().join(format!("stub-scripts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let pkg = |name: &str, extra: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n{extra}");
    let script = "fn main() {\n    println!(\"cargo:rerun-if-changed=build.rs\");\n}\n";
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"plain\", \"native\", \"app\"]\nresolver = \"2\"\n".to_string()),
        ("plain/Cargo.toml", pkg("sn-plain", "build = \"build.rs\"\n")),
        ("plain/build.rs", script.to_string()),
        ("plain/src/lib.rs", "pub fn word() -> &'static str {\n    \"stubs ran\"\n}\n".to_string()),
        ("native/Cargo.toml", pkg("sn-native", "links = \"sn_native\"\nbuild = \"build.rs\"\n\n[dependencies]\nsn-plain = { path = \"../plain\" }\n")),
        ("native/build.rs", script.to_string()),
        ("native/src/lib.rs", "pub fn word() -> &'static str {\n    sn_plain::word()\n}\n".to_string()),
        ("app/Cargo.toml", pkg("sn-app", "\n[dependencies]\nsn-native = { path = \"../native\" }\n")),
        ("app/src/main.rs", "fn main() {\n    println!(\"{}\", sn_native::word());\n}\n".to_string()),
    ];
    for (path, text) in &files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (status, stdout, stderr) = run_in(&ws, &["run", "--quiet", "--unattended", "--allow-c", "sn-app"]);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(status.success() && stdout == "stubs ran\n", "the native build's cargo run ran the stub of sn-plain's build script: {stdout}\n{stderr}");
}

/// An integration test that runs its package's binary with another HOME (and no XDG cache), with
/// a PATH that holds no build tools, without RELAX_ALLOW_C or with a cleared environment gets the
/// binary run when the binary has a native dependency (`--allow-c`), with the environment the
/// test gave it. The child's Relax compiles and links the native package with the build the
/// test's run made, so it starts no cargo and no C compiler (relax issue 151: under the test's
/// PATH it failed with "native build directory: No such file or directory"); the test's native
/// permission and that build are the binary's, not its environment's (relax issue 157: without
/// the variable, or with none at all, it failed with "it is built only with --allow-c"). Without
/// that build (the HOME case: the binary started by a shell, outside its test's run, so its run
/// builds with the environment it was given, and named no receipt), the child builds its natives
/// itself:
/// it starts cargo, which under another HOME needs rustup's homes named as rustup's cargo names
/// them for the programs it runs (relax issue 129: "rustup could not choose a version of cargo to
/// run"). The runner starts as from a shell without rustup's variables. hc-native has `links`
/// and the build-dependency `cc` (hc-cc, a stand-in that compiles nothing). Under cargo, `cargo
/// test -p hc-app` passes (the variable is none of cargo's).
#[test]
fn a_tests_binary_with_a_native_dependency_runs_in_the_environment_its_test_gives() {
    let ws = std::env::temp_dir().join(format!("home-child-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let pkg = |name: &str, extra: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n{extra}");
    let child = r#"#[test]
fn child_builds_its_natives_with_a_different_home() {
    let root = std::env::temp_dir().join(format!("home-child-home-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    // started by a shell: a start by this run's std is the test's build (relax issue 168)
    #[cfg(unix)]
    let mut start = {
        let mut sh = std::process::Command::new("/bin/sh");
        sh.args(["-c", "exec \"$0\"", env!("CARGO_BIN_EXE_hc-app")]);
        sh
    };
    #[cfg(not(unix))]
    let mut start = std::process::Command::new(env!("CARGO_BIN_EXE_hc-app"));
    let output = start.env("HOME", &root).env("XDG_CACHE_HOME", "").env("RELAX_BIN_NATIVE_RECEIPT", "").current_dir(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), format!("child ran {} {}\n", root.display(), std::env::var("PATH").unwrap()));
}

#[test]
fn child_runs_without_build_tools_in_its_path() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hc-app")).env("PATH", "/no-tools-for-this-test").output().unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), format!("child ran {} /no-tools-for-this-test\n", std::env::var("HOME").unwrap_or_default()));
}

#[test]
fn child_runs_without_the_tests_native_permission() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hc-app")).env_remove("RELAX_ALLOW_C").output().unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), format!("child ran {} {}\n", std::env::var("HOME").unwrap_or_default(), std::env::var("PATH").unwrap_or_default()));
}

#[test]
fn child_runs_with_a_cleared_environment() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hc-app")).env_clear().output().unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "child ran  \n");
}
"#;
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"cc\", \"native\", \"app\"]\nresolver = \"2\"\n".to_string()),
        ("cc/Cargo.toml", pkg("hc-cc", "")),
        ("cc/src/lib.rs", "pub fn compile(_source: &str) {}\n".to_string()),
        ("native/Cargo.toml", pkg("hc-native", "links = \"hc_native\"\nbuild = \"build.rs\"\n\n[build-dependencies]\ncc = { path = \"../cc\", package = \"hc-cc\" }\n")),
        ("native/build.rs", "fn main() {\n    cc::compile(\"csrc/native.c\");\n}\n".to_string()),
        ("native/src/lib.rs", "pub fn var(name: &str) -> String {\n    std::env::var(name).unwrap_or_default()\n}\n".to_string()),
        ("app/Cargo.toml", pkg("hc-app", "\n[dependencies]\nhc-native = { path = \"../native\" }\n")),
        ("app/src/main.rs", "fn main() {\n    println!(\"child ran {} {}\", hc_native::var(\"HOME\"), hc_native::var(\"PATH\"));\n}\n".to_string()),
        ("app/tests/child.rs", child.to_string()),
    ];
    for (path, text) in &files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (mut command, scratch) = runner_in(&ws);
    for name in ["RUSTUP_HOME", "CARGO_HOME", "RUSTUP_TOOLCHAIN"] {
        command.env_remove(name);
    }
    let output = command.args(["test", "--allow-c", "hc-app"]).output().unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_dir_all(&ws);
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("child_builds_its_natives_with_a_different_home ... ok"), "the child built its native dependency with cargo under the test's HOME (rustup's homes, relax issue 129): {stdout}\n{stderr}");
    assert!(stdout.contains("child_runs_without_build_tools_in_its_path ... ok"), "the child took the test run's native build and ran with the test's PATH, which holds no build tools (relax issue 151): {stdout}\n{stderr}");
    assert!(stdout.contains("child_runs_without_the_tests_native_permission ... ok"), "the child had the test run's native permission without RELAX_ALLOW_C in its environment (relax issue 157): {stdout}\n{stderr}");
    assert!(stdout.contains("child_runs_with_a_cleared_environment ... ok"), "the child had the test run's native permission and build with a cleared environment, and ran with none (relax issue 157): {stdout}\n{stderr}");
    assert!(output.status.success(), "the tests passed: {stdout}\n{stderr}");
}

/// A native build refreshed under the same key (its script ran again since, here under another
/// key's build in the same cargo target) whose library then fails to link keeps the library and
/// receipt of the build before it: a package binary prepared with that build still runs with its
/// library, and a second refresh fails at the link again (a receipt newer than its library would
/// have been taken as the build). Once the library links, the refresh is the build. Before, the
/// refresh removed the library before its link, so the binary found its test run's build gone
/// and built its natives itself, with the C compiler that links no shared library (relax issue
/// 160). The fixture's C library returns its source's value plus RN_SALT, which its build script
/// reads and the native key does not hold. The first test run keeps a copy of its
/// CARGO_BIN_EXE_rn-app with the build it names. CC is one wrapper for every run (CC is part of
/// the key), which links no shared library while the file `refuse` is beside it. rn-cc is a
/// stand-in for `cc` that compiles the C file into a static library with `$CC` and `ar`. The
/// scenario is relax PR 166's (moorbrook).
#[cfg(unix)]
#[test]
fn a_native_refresh_whose_link_fails_keeps_the_build_an_earlier_binary_runs_with() {
    let ws = std::env::temp_dir().join(format!("native-refresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let pkg = |name: &str, extra: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n{extra}");
    let cc = r#"/// Compiles the C file `source` with `define` into the static library `lib<name>.a` in OUT_DIR, and links it.
pub fn compile(source: &str, define: &str, name: &str) {
    let out = std::env::var("OUT_DIR").unwrap();
    let object = format!("{out}/{name}.o");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    assert!(std::process::Command::new(cc).args(["-c", "-fPIC", define, source, "-o", &object]).status().unwrap().success());
    assert!(std::process::Command::new("ar").args(["crs", &format!("{out}/lib{name}.a"), &object]).status().unwrap().success());
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static={name}");
}
"#;
    let build = r#"fn main() {
    println!("cargo:rerun-if-changed=csrc/native.c");
    println!("cargo:rerun-if-env-changed=RN_SALT");
    let salt = std::env::var("RN_SALT").unwrap_or_else(|_| "0".to_string());
    cc::compile("csrc/native.c", &format!("-DSALT={salt}"), "rn_native");
}
"#;
    let keep = r#"#[test]
fn runs_and_keeps_its_binary() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rn-app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), format!("{}\n", std::env::var("RN_WANT").unwrap()), "{}", String::from_utf8_lossy(&output.stderr));
    let kept = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kept");
    if kept.exists() {
        return;
    }
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_rn-app"), kept.join("rn-app")).unwrap();
    std::fs::write(kept.join("receipt"), std::env::var("RELAX_BIN_NATIVE_RECEIPT").unwrap_or_default()).unwrap();
}
"#;
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"cc\", \"native\", \"app\"]\nresolver = \"2\"\n".to_string()),
        ("cc/Cargo.toml", pkg("rn-cc", "")),
        ("cc/src/lib.rs", cc.to_string()),
        ("native/Cargo.toml", pkg("rn-native", "links = \"rn_native\"\nbuild = \"build.rs\"\n\n[build-dependencies]\ncc = { path = \"../cc\", package = \"rn-cc\" }\n")),
        ("native/build.rs", build.to_string()),
        ("native/csrc/native.c", String::new()),
        ("native/src/lib.rs", "extern \"C\" {\n    fn rn_value() -> i32;\n}\n\npub fn value() -> i32 {\n    unsafe { rn_value() }\n}\n".to_string()),
        ("app/Cargo.toml", pkg("rn-app", "\n[dependencies]\nrn-native = { path = \"../native\" }\n")),
        ("app/src/main.rs", "fn main() {\n    println!(\"{}\", rn_native::value());\n}\n".to_string()),
        ("app/tests/keep.rs", keep.to_string()),
        ("cc-wrapper", "#!/bin/sh\nif [ -e \"$(dirname \"$0\")/refuse\" ]; then\n    case \" $* \" in *\" -shared \"*|*\" -dynamiclib \"*) echo \"refusing cc: links no shared library\" >&2; exit 1;; esac\nfi\nexec cc \"$@\"\n".to_string()),
    ];
    for (path, text) in &files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let cc = ws.join("cc-wrapper");
    std::fs::set_permissions(&cc, <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755)).unwrap();
    // `makepad test --allow-c rn-app` with the C library's source returning `value` plus `salt`,
    // the C compiler refusing shared links or not
    let test = |value: u32, salt: u32, refuse: bool| {
        std::fs::write(ws.join("native/csrc/native.c"), format!("int rn_value(void) {{ return {value} + SALT; }}\n")).unwrap();
        if refuse {
            std::fs::write(ws.join("refuse"), "").unwrap();
        } else {
            let _ = std::fs::remove_file(ws.join("refuse"));
        }
        let (mut command, _) = runner_in(&ws);
        command.env("CC", &cc).env("RN_SALT", salt.to_string()).env("RN_WANT", (value + salt).to_string());
        let output = command.args(["test", "--allow-c", "rn-app"]).output().unwrap();
        (output.status, format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
    };
    let first = test(7, 0, false);
    let other = test(9, 0, false);
    let refresh = test(7, 100, true);
    let again = test(7, 100, true);
    // the binary the first run prepared, with the first run's build, under the compiler that links nothing
    let (runner, scratch) = runner_in(&ws);
    let mut earlier = Command::new(ws.join("kept/rn-app"));
    for (name, value) in runner.get_envs() {
        match value {
            Some(value) => earlier.env(name, value),
            None => earlier.env_remove(name),
        };
    }
    let receipt = std::fs::read_to_string(ws.join("kept/receipt")).unwrap_or_default();
    let output = earlier.current_dir(&ws).env("CC", &cc).env("RELAX_BIN_NATIVE_RECEIPT", &receipt).output().unwrap();
    let recovered = test(7, 100, false);
    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_dir_all(&ws);
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let refused = |run: &(std::process::ExitStatus, String)| !run.0.success() && run.1.contains("refusing cc: links no shared library");
    assert!(first.0.success() && !receipt.is_empty(), "the first run linked the native library, its binary returned 7, and the test kept the binary with that build: {}", first.1);
    assert!(other.0.success(), "the second run built the other library (9): {}", other.1);
    assert!(refused(&refresh), "the refresh of the first build failed at its link: {}", refresh.1);
    assert!(refused(&again), "a second refresh failed at its link too, not on a receipt newer than the library: {}", again.1);
    assert!(output.status.success() && stdout == "7\n", "the binary prepared before the refresh ran with the first build's library: {stdout}\n{stderr}");
    assert!(recovered.0.success(), "with the library linking again, the refresh is the build (107): {}", recovered.1);
}

/// `makepad test` builds a package's tests with cargo test's profile, `test`: debug assertions and
/// overflow checks on, unless the workspace's profiles say otherwise for a package (here the
/// dependency's: `debug-assertions` off in `[profile.dev.package.pc-dep]`, which `test` inherits,
/// `overflow-checks` off in `[profile.test.package.pc-dep]`); and selects the tests under
/// `cfg(debug_assertions)` as cargo does. `--release` builds the release profile: no checks, the
/// `cfg(not(debug_assertions))` tests. The host's units, the build script and the proc macro,
/// have the profile's checks under its `build-override` (here `overflow-checks` off). Before,
/// every test built release (relax issue 141: the default run failed a `cfg!(debug_assertions)`
/// test and ran a release-only one), and build scripts and proc macros always (relax PR 150).
/// The build script is told the profile as cargo tells it (PROFILE, OPT_LEVEL, DEBUG; before, always
/// `release,3,false`), and its CARGO, a proc macro's, `env!("CARGO")` and the program's are absolute
/// paths (before, the whole run had CARGO=cargo, which the program and its children looked up on
/// PATH; found by a review of the runner).
/// Under cargo, `cargo test -p pc-app -- --nocapture` prints
/// `app=true,true dep=false,false build=true,false macro=true script=debug,0,true,true
/// cargo=true,true,true` and runs `debug_only`; with `--release`, `app=false,false dep=false,false
/// build=false,false macro=false script=release,3,false,true cargo=true,true,true` and
/// `release_only`.
#[test]
fn a_tests_profile_is_cargo_tests() {
    let ws = std::env::temp_dir().join(format!("profile-checks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let pkg = |name: &str, extra: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n{extra}");
    let overflows = "fn overflows() -> bool {\n    std::panic::catch_unwind(|| std::hint::black_box(255u8) + 1).is_err()\n}\n";
    let app = r#"#[test]
fn profile_checks() {
    let dep = pc_dep::checks();
    println!("app={},{} dep={},{} build={} macro={}", cfg!(debug_assertions), overflows(), dep.0, dep.1, env!("BUILD_CHECKS"), pc_macro::host_debug!());
    println!("script={} cargo={},{},{}", env!("BUILD_PROFILE"), pc_macro::cargo_absolute!(), std::path::Path::new(env!("CARGO")).is_absolute(), std::env::var("CARGO").is_ok_and(|c| std::path::Path::new(&c).is_absolute()));
}

#[cfg(debug_assertions)]
#[test]
fn debug_only() {}

#[cfg(not(debug_assertions))]
#[test]
fn release_only() {}
"#;
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"app\", \"dep\", \"macro\"]\nresolver = \"2\"\n\n[profile.dev.package.pc-dep]\ndebug-assertions = false\n\n[profile.test.package.pc-dep]\noverflow-checks = false\n\n[profile.test.build-override]\noverflow-checks = false\n".to_string()),
        ("macro/Cargo.toml", pkg("pc-macro", "\n[lib]\nproc-macro = true\n")),
        ("macro/src/lib.rs", "extern crate proc_macro;\n\n#[proc_macro]\npub fn host_debug(_: proc_macro::TokenStream) -> proc_macro::TokenStream {\n    cfg!(debug_assertions).to_string().parse().unwrap()\n}\n\n#[proc_macro]\npub fn cargo_absolute(_: proc_macro::TokenStream) -> proc_macro::TokenStream {\n    std::env::var(\"CARGO\").is_ok_and(|c| std::path::Path::new(&c).is_absolute()).to_string().parse().unwrap()\n}\n".to_string()),
        ("app/build.rs", format!("{overflows}\nfn main() {{\n    println!(\"cargo:rustc-env=BUILD_CHECKS={{}},{{}}\", cfg!(debug_assertions), overflows());\n    let var = |k: &str| std::env::var(k).unwrap_or_default();\n    println!(\"cargo:rustc-env=BUILD_PROFILE={{}},{{}},{{}},{{}}\", var(\"PROFILE\"), var(\"OPT_LEVEL\"), var(\"DEBUG\"), std::path::Path::new(&var(\"CARGO\")).is_absolute());\n}}\n")),
        ("dep/Cargo.toml", pkg("pc-dep", "")),
        ("dep/src/lib.rs", format!("{overflows}\npub fn checks() -> (bool, bool) {{\n    (cfg!(debug_assertions), overflows())\n}}\n")),
        ("app/Cargo.toml", pkg("pc-app", "\n[dependencies]\npc-dep = { path = \"../dep\" }\npc-macro = { path = \"../macro\" }\n")),
        ("app/src/lib.rs", format!("{overflows}\n{app}")),
    ];
    for (path, text) in &files {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let deadline = std::time::Duration::from_secs(120);
    let test = run_until(&ws, &["test", "pc-app", "--", "--nocapture"], &[], deadline);
    let release = run_until(&ws, &["test", "pc-app", "--release", "--", "--nocapture"], &[], deadline);
    let _ = std::fs::remove_dir_all(&ws);
    let cases = [
        (test, "test", "app=true,true dep=false,false build=true,false macro=true", "script=debug,0,true,true", "debug_only", "release_only"),
        (release, "release", "app=false,false dep=false,false build=false,false macro=false", "script=release,3,false,true", "release_only", "debug_only"),
    ];
    for ((status, stdout, stderr), profile, line, script, only, not) in cases {
        assert!(status.is_some_and(|s| s.success()), "the {profile} profile's run passes: {stdout}\n{stderr}");
        assert!(stdout.contains(line), "the {profile} profile's checks, the dependency's as its overrides set them, the host's as its build-override does: {stdout}\n{stderr}");
        assert!(stdout.contains(&format!("{script} cargo=true,true,true")), "the build script's PROFILE, OPT_LEVEL and DEBUG are the {profile} profile's, and CARGO is an absolute path for the script, the proc macro, env! and the program: {stdout}");
        assert!(stdout.contains(&format!("test {only} ... ok")) && !stdout.contains(not), "the {profile} profile selects {only}, not {not}: {stdout}");
    }
}
