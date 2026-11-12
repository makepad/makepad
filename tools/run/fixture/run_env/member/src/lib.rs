//! cargo's run-time environment of the process of a target it runs (cargo Compilation::fill_env),
//! as `cargo test` and `cargo run` give it from the root of the virtual workspace: the package's
//! variables with the values its crates were compiled with, its build script's OUT_DIR and rustc-env and the
//! `[env]` configuration (a variable the run was started with wins unless the entry is forced),
//! and none of what cargo hands rustc alone. A test runs in its package's folder, a program in the
//! folder it was started in (RUN_ENV_START). `check` lists what differs.

pub fn check(test: bool) -> Vec<String> {
    let mut bad = Vec::new();
    let mut same = |k: &str, want: Option<&str>| {
        let got = std::env::var(k).ok();
        if got.as_deref() != want {
            bad.push(format!("{k}: {got:?}, want {want:?}"));
        }
    };
    same("CARGO_MANIFEST_DIR", Some(env!("CARGO_MANIFEST_DIR")));
    same("CARGO_MANIFEST_PATH", Some(env!("CARGO_MANIFEST_PATH")));
    same("CARGO_PKG_NAME", Some(env!("CARGO_PKG_NAME")));
    same("CARGO_PKG_VERSION", Some(env!("CARGO_PKG_VERSION")));
    same("CARGO_PKG_VERSION_MAJOR", Some(env!("CARGO_PKG_VERSION_MAJOR")));
    same("CARGO_PKG_VERSION_MINOR", Some(env!("CARGO_PKG_VERSION_MINOR")));
    same("CARGO_PKG_VERSION_PATCH", Some(env!("CARGO_PKG_VERSION_PATCH")));
    same("CARGO_PKG_VERSION_PRE", Some(env!("CARGO_PKG_VERSION_PRE")));
    same("CARGO_PKG_AUTHORS", Some(env!("CARGO_PKG_AUTHORS")));
    same("CARGO_PKG_DESCRIPTION", Some(env!("CARGO_PKG_DESCRIPTION")));
    same("CARGO_PKG_HOMEPAGE", Some(env!("CARGO_PKG_HOMEPAGE")));
    same("CARGO_PKG_REPOSITORY", Some(env!("CARGO_PKG_REPOSITORY")));
    same("CARGO_PKG_LICENSE", Some(env!("CARGO_PKG_LICENSE")));
    same("CARGO_PKG_LICENSE_FILE", Some(env!("CARGO_PKG_LICENSE_FILE")));
    same("CARGO_PKG_RUST_VERSION", Some(env!("CARGO_PKG_RUST_VERSION")));
    same("CARGO_PKG_README", Some(env!("CARGO_PKG_README")));
    same("OUT_DIR", Some(env!("OUT_DIR")));
    same("RUN_ENV_FROM_SCRIPT", Some(env!("RUN_ENV_FROM_SCRIPT")));
    same("RUN_ENV_CONFIG", Some("from-config"));
    same("RUN_ENV_CONFIG_KEPT", Some("outer"));
    same("RUN_ENV_CONFIG_FORCED", Some("forced"));
    for k in ["CARGO_CRATE_NAME", "CARGO_BIN_NAME", "CARGO_PRIMARY_PACKAGE", "CARGO_TARGET_TMPDIR", "CARGO_MANIFEST_LINKS"] {
        same(k, None);
    }
    // nor did its crates get the package's `links` at compile time: cargo gives it to the build script alone
    if let Some(v) = option_env!("CARGO_MANIFEST_LINKS") {
        bad.push(format!("CARGO_MANIFEST_LINKS at compile time: {v:?}, want none"));
    }
    // the build script saw the package's variables as its crates do
    let script = [
        (env!("RUN_ENV_SCRIPT_CARGO_MANIFEST_DIR"), env!("CARGO_MANIFEST_DIR")),
        (env!("RUN_ENV_SCRIPT_CARGO_MANIFEST_PATH"), env!("CARGO_MANIFEST_PATH")),
        (env!("RUN_ENV_SCRIPT_CARGO_PKG_NAME"), env!("CARGO_PKG_NAME")),
        (env!("RUN_ENV_SCRIPT_CARGO_PKG_VERSION_MAJOR"), env!("CARGO_PKG_VERSION_MAJOR")),
        (env!("RUN_ENV_SCRIPT_CARGO_PKG_VERSION_PRE"), env!("CARGO_PKG_VERSION_PRE")),
        (env!("RUN_ENV_SCRIPT_CARGO_PKG_AUTHORS"), env!("CARGO_PKG_AUTHORS")),
        (env!("RUN_ENV_SCRIPT_CARGO_MANIFEST_LINKS"), "relax_run_env"),
    ];
    for (seen, want) in script {
        if seen != want {
            bad.push(format!("build script: {seen:?}, want {want:?}"));
        }
    }
    let folder = if test { env!("CARGO_MANIFEST_DIR").to_string() } else { std::env::var("RUN_ENV_START").unwrap_or_default() };
    let cwd = std::env::current_dir().ok().and_then(|d| d.canonicalize().ok());
    if cwd.is_none() || cwd != std::path::Path::new(&folder).canonicalize().ok() {
        bad.push(format!("working folder: {cwd:?}, want {folder:?}"));
    }
    bad
}

#[test]
fn a_lib_test_has_cargos_run_time_environment() {
    let mut bad = check(true);
    // the package's binaries are an integration test's alone
    if let Ok(v) = std::env::var("CARGO_BIN_EXE_relax-run-env") {
        bad.push(format!("CARGO_BIN_EXE_relax-run-env: {v:?} in a lib test"));
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
