//! Install pinned registry artifacts through the hub's normal verified downloader.
use makepad_ai_hub::local::{InstallMsg, InstallState, LocalModels};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
fn run() -> Result<(), String> {
    let mut model = None;
    let mut accept = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--accept-license" => accept = true,
            "--weights-dir" => {
                let dir = args.next().ok_or("--weights-dir needs DIR")?;
                std::env::set_var("MAKEPAD_ASSET_AI_CACHE", dir);
            }
            "--help" | "-h" => {
                println!("hub-install <model_id> [--accept-license] [--weights-dir DIR]");
                return Ok(());
            }
            s if !s.starts_with('-') && model.is_none() => model = Some(arg),
            _ => return Err(format!("unexpected argument: {arg}")),
        }
    }
    let model =
        model.ok_or("usage: hub-install <model_id> [--accept-license] [--weights-dir DIR]")?;
    let mut models = LocalModels::open().map_err(|e| e.to_string())?;
    let spec = models
        .spec(&model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    println!(
        "{}: {} files, {} bytes",
        spec.id,
        spec.files.len(),
        spec.files.iter().map(|f| f.size.unwrap_or(0)).sum::<u64>()
    );
    for f in &spec.files {
        println!(
            "{}: {} ({} bytes)",
            f.role.as_deref().unwrap_or("file"),
            f.cache_as,
            f.size.unwrap_or(0)
        );
    }
    if (spec.gated || !models.license_acknowledged(&model)) && !accept {
        return Err(
            "license not accepted; review the registry license and pass --accept-license".into(),
        );
    }
    if accept {
        models
            .acknowledge_license(&model)
            .map_err(|e| e.to_string())?;
    }
    let handle = models.start_install(&model).map_err(|e| e.to_string())?;
    let mut files = BTreeMap::new();
    let mut last = Instant::now();
    let mut last_bytes = 0;
    let mut current = String::new();
    loop {
        let mut finished = false;
        for msg in handle.poll() {
            match msg {
                InstallMsg::Progress { file, done, total } => {
                    // Progress names the source path; FileDone names cache_as.
                    // Store both under cache_as so verified files count once.
                    let key = spec.files.iter().find(|f| f.path == file)
                        .map(|f| f.cache_as.clone()).unwrap_or(file);
                    current = key.clone();
                    files.insert(key, (done, total));
                }
                InstallMsg::FileDone { file } => {
                    if let Some(f) = spec.files.iter().find(|f| f.cache_as == file) {
                        let n = f.size.unwrap_or(0);
                        files.insert(file.clone(), (n, n));
                    }
                    println!("verified {file}");
                }
                InstallMsg::Finished => finished = true,
                InstallMsg::Failed(e) => {
                    handle.cancel();
                    return Err(e);
                }
                InstallMsg::Cancelled => return Err("installation cancelled".into()),
            }
        }
        if last.elapsed() >= Duration::from_secs(1) || finished {
            let bytes: u64 = files.values().map(|x| x.0).sum();
            let total = spec.files.iter().map(|f| f.size.unwrap_or(0)).sum::<u64>();
            println!(
                "{current}: {bytes}/{total} bytes, {:.2} MB/s",
                bytes.saturating_sub(last_bytes) as f64
                    / last.elapsed().as_secs_f64()
                    / 1_000_000.0
            );
            last = Instant::now();
            last_bytes = bytes;
        }
        if finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !matches!(models.install_state(&model), InstallState::Installed) {
        return Err("download finished but install_state is not Installed".into());
    }
    for f in &spec.files {
        if let Some(role) = &f.role {
            let path = models
                .installed_path(&model, role)
                .ok_or_else(|| format!("installed role missing: {role}"))?;
            println!("{role}: {}", path.display());
        }
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("hub-install: {e}");
        std::process::exit(1);
    }
}
