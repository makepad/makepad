// The run-time module fixtures (libs/relax/compiler/tests/modules): run.sh runs each fixture
// program under the runner and checks its output, its tier-up and its reload trace; every case
// runs, and the failing ones print their output. One cargo test, so the test buckets run it.
use std::process::Command;

#[test]
fn the_module_fixtures_pass_under_the_runner() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = root.join("libs/relax/compiler/tests/modules/run.sh");
    // Under `makepad test` the runner is the one running this test (its loader); under cargo
    // it is the binary cargo just built for this package. (`CARGO_BIN_EXE_makepad` is, under the
    // runner, a wrapper that would run the runner's own main as a program.)
    let runner = std::env::var("MAKEPAD_RUN_LOADER").unwrap_or_else(|_| env!("CARGO_BIN_EXE_makepad").to_string());
    let output = Command::new("bash").arg(&script).arg(&runner).current_dir(&root).output().unwrap();
    let all = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "libs/relax/compiler/tests/modules/run.sh failed:\n{all}");
}

/// What rustc refuses in the app's own crates, a run refuses too and says why, validation-only
/// checks included: E0436 used to compile and run, and the run said nothing.
#[test]
fn a_run_refuses_what_rustc_refuses_in_the_apps_crates() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runner = std::env::var("MAKEPAD_RUN_LOADER").unwrap_or_else(|_| env!("CARGO_BIN_EXE_makepad").to_string());
    let scratch = std::env::temp_dir().join(format!("run-refuses-{}", std::process::id()));
    let output = Command::new(&runner)
        .args(["run", "run-refuses"])
        .current_dir(root.join("libs/relax/compiler/tests/run_refuses"))
        .env("MAKEPAD_RUN_STD", root.join("libs/relax/compiler/std"))
        .env("MAKEPAD_RUN_CACHE_DIR", scratch.join("cache"))
        .env("MAKEPAD_RUN_TARGET_DIR", scratch.join("target"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.status.code(), Some(101), "{stdout}\n{stderr}");
    assert!(!stdout.contains("started"), "it ran: {stdout}\n{stderr}");
    assert!(stderr.contains("E0436: functional record update syntax requires a struct"), "{stderr}");
}

/// `makepad run --quiet <package>` in the fixture workspace `dir` (under libs/relax/compiler/tests),
/// with a cache and target directory of its own: its exit, stdout and stderr.
fn run_fixture(dir: &str, package: &str) -> (std::process::ExitStatus, String, String) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    run_in(&root.join("libs/relax/compiler/tests").join(dir), &["run", "--quiet", package])
}

/// `makepad <args>` in the workspace at `dir`, with a cache and target directory of its own: its
/// exit, stdout and stderr.
fn run_in(dir: &std::path::Path, args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runner = std::env::var("MAKEPAD_RUN_LOADER").unwrap_or_else(|_| env!("CARGO_BIN_EXE_makepad").to_string());
    let name = dir.file_name().unwrap().to_string_lossy();
    let scratch = std::env::temp_dir().join(format!("{name}-run-{}", std::process::id()));
    let output = Command::new(&runner)
        .args(args)
        .current_dir(dir)
        .env("MAKEPAD_RUN_STD", root.join("libs/relax/compiler/std"))
        .env("MAKEPAD_RUN_CACHE_DIR", scratch.join("cache"))
        .env("MAKEPAD_RUN_TARGET_DIR", scratch.join("target"))
        // as from a shell: CARGO is cargo's, which a run sets itself for the proc macros
        .env_remove("CARGO")
        .env_remove("MAKEPAD_RUN_SCRIPT_DEPTH")
        .env_remove("MAKEPAD_RUN_SCRIPT_CHAIN")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    (output.status, String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
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
