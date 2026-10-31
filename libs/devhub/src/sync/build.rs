//! The prebuild after a sync: what the WM and its launches would compile, built now with the
//! environment they use, so a WM start or an app launch compiles nothing (WMSTUCK, CUDAFEAT).
//!
//! - `build = wm` is fleetmp/build-next.sh: the WM the way its session runs it (the session's env:
//!   /etc/makepad/wm.env read as data, `MAKEPAD=linux_direct`, `CARGO_INCREMENTAL=0`, offline),
//!   then `target/release/makepad-wm-build <wm flags> -- <prebuild>`, which builds each app (and
//!   the AI Hub, with its features and CUDA env) with the WM's own launch command.
//! - `build = apps` builds each `prebuild` app with the launch's own command
//!   ([`AppLaunch::build_command`]), for a checkout no WM runs from.
//!
//! Every step runs at nice 15, logs to `~/.makepad/devhub/sync/logs`, and reports the crates it
//! compiled (a no-op prebuild compiles none).

use super::conf::{Build, SyncConf};
use makepad_app_registry::{AppLaunch, Runner};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// The result of the prebuild.
#[derive(Debug, Default)]
pub struct Built {
    pub ok: bool,
    pub compiled: Vec<String>,
    pub secs: f64,
    pub log: PathBuf,
    pub err: String,
}

/// `/etc/makepad/wm.env` as `KEY=VALUE` data (what the session script sources).
pub fn wm_env() -> Vec<(String, String)> {
    let text = std::fs::read_to_string("/etc/makepad/wm.env").unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let l = l.strip_prefix("export ").unwrap_or(l);
            if l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            let k = k.trim();
            if k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return None;
            }
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))).unwrap_or(v);
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Variables of the hub's own environment that would change what cargo builds.
fn clear_build_env(cmd: &mut Command) {
    for (k, _) in std::env::vars_os() {
        let Some(k) = k.to_str() else { continue };
        if k.starts_with("CARGO_PROFILE_") || k.starts_with("CARGO_BUILD_") || matches!(k, "RUSTFLAGS" | "CARGO_ENCODED_RUSTFLAGS" | "CARGO_TARGET_DIR" | "CARGO_INCREMENTAL" | "RUSTC_WRAPPER" | "MAKEPAD") {
            cmd.env_remove(k);
        }
    }
}

fn niced(program: &Path, args: &[String]) -> Command {
    let mut cmd = crate::os::niced(program.as_os_str());
    cmd.args(args);
    cmd
}

/// The `--jit` apps of the WM's flags.
pub fn jit_apps(wm_flags: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = wm_flags.iter();
    while let Some(f) = it.next() {
        if f == "--jit" {
            if let Some(list) = it.next() {
                out.extend(list.split(',').filter(|s| !s.is_empty()).map(str::to_string));
            }
        } else if let Some(list) = f.strip_prefix("--jit=") {
            out.extend(list.split(',').filter(|s| !s.is_empty()).map(str::to_string));
        }
    }
    out
}

/// The crate a cargo `Compiling` line names (the launchers' env draws a progress bar in front).
fn compiling(line: &str) -> Option<String> {
    let at = line.rfind("Compiling ")?;
    line[at + "Compiling ".len()..].split_whitespace().next().map(str::to_string)
}

/// Runs one step, appending its output to `log`; the crates it compiled go to `compiled`.
fn step(mut cmd: Command, name: &str, log: &Path, compiled: &mut Vec<String>) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let _ = writeln!(file, "== {name}");
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("{name}: {e}"))?;
    let out = child.stdout.take().unwrap();
    let file2 = file.try_clone().map_err(|e| e.to_string())?;
    let t = std::thread::spawn(move || {
        let mut f = file2;
        let mut got = Vec::new();
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = writeln!(f, "{l}");
            got.push(l);
        }
        got
    });
    let mut last_err = String::new();
    for l in BufReader::new(child.stderr.take().unwrap()).lines().map_while(Result::ok) {
        let _ = writeln!(file, "{l}");
        let t = l.trim_start();
        if let Some(c) = compiling(&l) {
            compiled.push(c);
        } else if t.starts_with("error") {
            last_err = crate::proto::clean(t, 300);
        }
    }
    let lines = t.join().unwrap_or_default();
    compiled.extend(lines.iter().filter_map(|l| compiling(l)));
    let status = child.wait().map_err(|e| format!("{name}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} failed ({status}){}", if last_err.is_empty() { String::new() } else { format!(": {last_err}") }))
    }
}

pub fn logs_dir() -> PathBuf {
    crate::proto::home().join("sync").join("logs")
}

/// Builds what the conf asks for in the checkout.
pub fn prebuild(conf: &SyncConf) -> Built {
    let t = Instant::now();
    let dir = logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join(format!("build-{}.log", crate::proto::wall_ms()));
    let mut b = Built { log: log.clone(), ..Default::default() };
    let root = &conf.checkout;
    let cargo = makepad_app_registry::cargo_bin();
    let r = match conf.build {
        Build::Off => Ok(()),
        Build::Wm => (|| {
            // 1. the WM, with its session's environment
            let jobs = conf.jobs.to_string();
            let wm_args: Vec<String> = ["build", "--offline", "--release", "-j", jobs.as_str(), "-p", "makepad-app-wm"].iter().map(|s| s.to_string()).collect();
            let mut wm = niced(&cargo, &wm_args);
            clear_build_env(&mut wm);
            wm.current_dir(root).envs(wm_env()).env("MAKEPAD", "linux_direct").env("MAKEPAD_WM_ROOT", root).env("CARGO_NET_OFFLINE", "true").env("CARGO_INCREMENTAL", "0");
            step(wm, "makepad-app-wm", &log, &mut b.compiled)?;
            // 2. what the WM starts, with the WM's own launch commands
            let mut args: Vec<String> = conf.wm_flags.clone();
            args.push("--".into());
            args.extend(conf.prebuild.iter().cloned());
            let mut apps = niced(&root.join("target/release/makepad-wm-build"), &args);
            clear_build_env(&mut apps);
            apps.current_dir(root).envs(wm_env()).env("MAKEPAD", "linux_direct").env("MAKEPAD_WM_ROOT", root).env("CARGO_NET_OFFLINE", "true").env("CARGO_BUILD_JOBS", conf.jobs.to_string());
            step(apps, "makepad-wm-build", &log, &mut b.compiled)
        })(),
        Build::Apps => (|| {
            let jit = jit_apps(&conf.wm_flags);
            for id in &conf.prebuild {
                let Some(app) = makepad_app_registry::find(id) else { return Err(format!("prebuild: no app `{id}` in the registry")) };
                let runner = if jit.iter().any(|j| j == id) { Runner::Jit } else { Runner::Native };
                let cmd = AppLaunch::build_command(app, runner, root.clone())?;
                // the launch's own command and env, niced, offline, with the job count
                let mut n = crate::os::niced(cmd.get_program());
                n.args(cmd.get_args());
                for (k, v) in cmd.get_envs() {
                    match v {
                        Some(v) => n.env(k, v),
                        None => n.env_remove(k),
                    };
                }
                n.current_dir(cmd.get_current_dir().unwrap_or(root)).env("CARGO_NET_OFFLINE", "true").env("CARGO_BUILD_JOBS", conf.jobs.to_string());
                step(n, id, &log, &mut b.compiled)?;
            }
            Ok(())
        })(),
    };
    b.secs = t.elapsed().as_secs_f64();
    match r {
        Ok(()) => b.ok = true,
        Err(e) => b.err = e,
    }
    b
}

/// The modification time of a built binary (to tell whether a prebuild replaced it).
pub fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn compiled_crates_behind_progress_bars() {
        assert_eq!(super::compiling("    Building [==>  ] 84/97: relax…   Compiling makepad-script-std v1.0.0 (/x)").as_deref(), Some("makepad-script-std"));
        assert_eq!(super::compiling("   Compiling relax-front v0.1.0").as_deref(), Some("relax-front"));
        assert_eq!(super::compiling("    Finished `release`"), None);
    }

    #[test]
    fn jit_flags() {
        let f: Vec<String> = ["--jit", "sandbox,code", "--module", "x", "--jit=files"].iter().map(|s| s.to_string()).collect();
        assert_eq!(super::jit_apps(&f), vec!["sandbox", "code", "files"]);
    }
}
