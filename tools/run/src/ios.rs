//! The iOS app's entry (`makepad` built for aarch64-apple-ios / -sim, the executable of the
//! bundle `cargo makepad apple ios run-host` makes): the app's source is the packed store in the
//! bundle (`makepad-run-src.pack`, `makepad pack-src --target aarch64-apple-ios`), what to run is
//! `makepad-run-host.conf` beside it (the Android host's format: `app=`, `flag=`, `env=K=V`,
//! `privileged=1`), and the run is `makepad run --src <pack> <package> [flags]` with the HAL's
//! UIKit shell on the main thread. iOS maps no executable memory for a signed app, so the run is
//! in no-codegen mode (relax_session::options::no_codegen_env): every function of the app, its
//! crates and std runs in the threaded interpreter, and everything native -- this binary -- is
//! signed with the bundle.

/// The packed source store in the bundle.
const PACK: &str = "makepad-run-src.pack";
/// What the bundle runs (cargo_makepad apple `run-host` writes it).
const CONF: &str = "makepad-run-host.conf";

struct Conf {
    package: String,
    flags: Vec<String>,
    env: Vec<(String, String)>,
    privileged: bool,
}

fn parse_conf(text: &str) -> Result<Conf, String> {
    let mut apps: Vec<(String, Vec<String>)> = Vec::new();
    let mut env = Vec::new();
    let mut privileged = false;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (k, v) = line.split_once('=').ok_or_else(|| format!("{CONF}: bad line {line:?}"))?;
        match k {
            "app" => apps.push((v.to_string(), Vec::new())),
            "flag" => apps.last_mut().ok_or_else(|| format!("{CONF}: flag before app"))?.1.push(v.to_string()),
            "env" => {
                let (k, val) = v.split_once('=').ok_or_else(|| format!("{CONF}: env wants KEY=VALUE, got {v:?}"))?;
                env.push((k.to_string(), val.to_string()));
            }
            "privileged" => privileged = v == "1",
            _ => return Err(format!("{CONF}: unknown key {k:?}")),
        }
    }
    // the first app is the one the bundle boots (MAKEPAD_RUN_APP picks another it carries)
    let want = std::env::var("MAKEPAD_RUN_APP").ok().filter(|s| !s.is_empty());
    let (package, flags) = match &want {
        Some(w) => apps.into_iter().find(|a| &a.0 == w).ok_or_else(|| format!("{CONF}: the bundle does not carry {w}"))?,
        None => apps.into_iter().next().ok_or_else(|| format!("{CONF}: no app"))?,
    };
    Ok(Conf { package, flags, env, privileged })
}

/// The process's entry on iOS.
pub fn main() {
    let code = match boot() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("makepad: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn boot() -> Result<i32, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let bundle = exe.parent().ok_or("the executable has no folder")?.to_path_buf();
    let conf = std::fs::read_to_string(bundle.join(CONF)).map_err(|e| format!("{}: {e}", bundle.join(CONF).display()))?;
    let conf = parse_conf(&conf)?;
    // the app's container: HOME is it; only Library/ and tmp/ are the app's to write
    let home = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("no HOME")?);
    let caches = home.join("Library/Caches");
    let relax_home = caches.join("makepad-run");
    std::fs::create_dir_all(&relax_home).map_err(|e| format!("{}: {e}", relax_home.display()))?;
    if std::env::var_os("MAKEPAD_RUN_HOME").is_none() {
        std::env::set_var("MAKEPAD_RUN_HOME", &relax_home);
    }
    if std::env::var_os("MAKEPAD_HOME").is_none() {
        std::env::set_var("MAKEPAD_HOME", home.join("Library/Application Support/makepad"));
    }
    for (k, v) in &conf.env {
        std::env::set_var(k, v);
    }
    let mut args: Vec<Vec<u8>> = vec![exe.as_os_str().as_encoded_bytes().to_vec(), b"run".to_vec()];
    if conf.privileged {
        args.push(b"--privileged".to_vec());
    }
    args.push(b"--src".to_vec());
    args.push(bundle.join(PACK).as_os_str().as_encoded_bytes().to_vec());
    args.push(conf.package.into_bytes());
    args.extend(conf.flags.into_iter().map(String::into_bytes));
    relax_compiler::memstat::tune_malloc();
    relax_runner::set_host(&crate::HOST);
    relax_runner::run_with_args(args);
    Ok(0)
}
