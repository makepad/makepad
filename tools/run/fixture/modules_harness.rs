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
/// resolves where a declaration crate imports it, and a proc macro runs with CARGO set also in a
/// run that did not come from cargo (libs/relax/compiler/tests/proc_macro_hack_entry).
#[test]
fn a_proc_macro_hack_entry_resolves_and_proc_macros_see_cargo() {
    let (status, stdout, stderr) = run_fixture("proc_macro_hack_entry", "pmh-app");
    assert!(status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "hack entry ok, CARGO set: true", "{stderr}");
}
