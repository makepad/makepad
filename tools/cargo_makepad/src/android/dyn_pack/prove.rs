//! The phone's tile build, on the Mac — apps/wm/src/dylib_host.rs
//! `compile_app` verbatim: `cargo rustc --crate-type dylib` against a packaged
//! tree and its target/, the engine forced into the app's crate graph at the
//! rustc level (`--extern force:`, nightly-gated -> RUSTC_BOOTSTRAP=1 in the
//! env, with RUSTFLAGS, the linker string and the remapping rustc wrapper:
//! the device's env.txt). The verdict passes only on the device's success
//! criteria: cargo succeeded, nothing but the app itself Dirty, no build script run, exactly one
//! Compiling (the app), the engine .so untouched, the app .so produced and
//! NEEDING the engine.

use super::Dyn;
use crate::android::compile;
use std::{fs, path::Path, time::SystemTime};

/// Build `app` as the device would and judge the log; Ok(true) is the
/// device's success. `own_build_script_ok` (the pack proof against a fresh
/// cross-build): the app's OWN build script may compile and run here — that
/// run is what the pack ships, so on the phone it is Fresh (apps/files
/// generates PNGs). `extra_env` goes on top of the controlled env (the
/// rehearsal's env.txt values, CARGO_HOME and target dir).
pub fn prove(
    d: &Dyn,
    tree: &Path,
    target: &Path,
    app: &str,
    log: &Path,
    own_build_script_ok: bool,
    extra_env: &[(String, String)],
) -> Result<bool, String> {
    let engine = d.engine_dylib(target);
    if !engine.is_file() {
        return Err(format!("   engine dylib missing: {}", engine.display()));
    }
    let before = stamp(&engine);
    let mut cmd = d.command("rustup", target)?;
    cmd.args(["run", "stable", "cargo", "rustc", "--release", "--lib", "--crate-type", "dylib", "--offline", "--frozen"])
        .arg("--target")
        .arg(d.triple())
        .arg("--manifest-path")
        .arg(tree.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .args(["-p", app, "--no-default-features", "--features", "dynamic-module", "-v", "--", "-Zunstable-options", "--extern"])
        .arg(format!("force:{}={}", d.engine_lib(), engine.display()))
        .current_dir(tree);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // cargo's status is part of the verdict; the log still says what
    // compiled when it failed (a stale .so must never pass as a build).
    let cargo_ok = super::run_logged(&mut cmd, log)?;
    let after = stamp(&engine);
    let text = fs::read_to_string(log).map_err(|e| format!("{}: {e}", log.display()))?;

    let widgets_fresh = text.lines().filter(|l| second_word_after(l, "Fresh") == Some("makepad-widgets")).count();
    // A Dirty line for any package but the app is a unit mismatch. The app
    // itself may be Dirty here: a previous proof left its dylib in target/
    // and the engine was rebuilt since. The pack ships no app lib units, so
    // on the phone the app is always compiled fresh, never Dirty.
    let dirty = text
        .lines()
        .filter(|l| l.trim_start().starts_with("Dirty ") && second_word_after(l, "Dirty") != Some(app))
        .count();
    let own_marker = format!("/build/{app}-");
    let mut scripts = 0;
    for l in text.lines().filter(|l| l.contains("Running `") && l.contains("build-script-build`")) {
        let own = own_build_script_ok && own_script_line(l, &own_marker);
        if !own {
            scripts += 1;
        }
    }
    // Exactly the app compiles (0 when a previous proof left it Fresh: the
    // .so check below still holds); any OTHER package compiling is a unit
    // mismatch.
    let compiling = text.lines().filter(|l| second_word_after(l, "Compiling") == Some(app)).count();
    let others = text
        .lines()
        .filter(|l| l.trim_start().starts_with("Compiling ") && second_word_after(l, "Compiling") != Some(app))
        .count();
    // apps/route builds crate makepad_app_route; every other package is lib<package>.so
    let so = target.join(d.triple()).join("release").join(format!("{}.so", Dyn::app_stem(app)));
    let dylib = fs::metadata(&so).ok().map(|m| (m.len(), so.clone()));
    let needs_engine = if dylib.is_some() {
        compile::read_needed_shared_libs(&d.sdk_dir, d.host_os, &d.urls, &so)?
            .iter()
            .filter(|n| n.as_str() == format!("lib{}.so", d.engine_lib()))
            .count()
    } else {
        0
    };
    let engine_untouched = before == after;
    let pass = cargo_ok
        && widgets_fresh == 1
        && dirty == 0
        && scripts == 0
        && others == 0
        && engine_untouched
        && dylib.is_some()
        && needs_engine == 1;
    println!(
        "   cargo: {}  widgets Fresh: {widgets_fresh}  Dirty: {dirty}  build scripts run: {scripts}  Compiling: {compiling} (other packages: {others})  engine untouched: {}  dylib: {}  NEEDED engine: {needs_engine}",
        if cargo_ok { "ok" } else { "FAILED" },
        if engine_untouched { "yes" } else { "NO" },
        dylib.as_ref().map(|(n, p)| format!("{n} {}", p.display())).unwrap_or_else(|| "MISSING".to_string())
    );
    for l in text
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("Dirty") || t.starts_with("Compiling") || l.starts_with("error")
        })
        .take(12)
    {
        println!("{}", l.split(" (").next().unwrap_or(l));
    }
    if pass {
        println!("   PASS");
    } else {
        println!("   FAIL ({})", log.display());
    }
    Ok(pass)
}

fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let m = fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// `   Compiling <name> v…` / `   Fresh <name> v…`: the word after `key`.
fn second_word_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut words = line.split_whitespace();
    (words.next() == Some(key)).then(|| words.next()).flatten()
}

/// `Running \`…/build/<app>-<hex>/build-script-build\``: the app's own build script.
fn own_script_line(line: &str, marker: &str) -> bool {
    let Some(at) = line.find(marker) else { return false };
    let rest = &line[at + marker.len()..];
    let hex_len = rest.bytes().take_while(|b| b.is_ascii_hexdigit()).count();
    hex_len > 0 && rest[hex_len..].starts_with("/build-script-build`")
}
