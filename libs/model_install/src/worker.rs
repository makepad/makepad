//! The install worker: one thread, files downloaded in order through the
//! hub's resumable, hash-checked downloader.

use crate::{dest_path, provenance_path, provenance_text, root_of, weights_path, Installable};
use makepad_ai_hub::backend::CancelToken;
use makepad_ai_hub::download::{DownloadProgress, Downloader};
use makepad_ai_hub::registry::FileSpec;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

pub enum InstallMsg {
    Progress { short: &'static str, done: u64, total: u64 },
    Done { short: &'static str },
    Failed { short: &'static str, error: String },
    /// The operator pulled the plug; `.part` files stay for a later resume.
    Cancelled,
    /// Every requested file has been tried. The receiver decides what the
    /// row says from what is now actually on disk.
    Finished,
}

/// The drain side of one install worker, plus the cord to pull.
pub struct InstallHandle {
    pub rx: Receiver<InstallMsg>,
    cancel: CancelToken,
}

impl InstallHandle {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

fn spec(model: &Installable) -> FileSpec {
    FileSpec {
        role: None,
        repo: model.repo.to_string(),
        path: model.path.to_string(),
        revision: (!model.revision.is_empty()).then(|| model.revision.to_string()),
        cache_as: model.cache_as.to_string(),
        size: Some(model.bytes),
        sha256: Some(model.sha256.to_string()),
        local: false,
        optional: false,
        converts_to: None,
        conversion: None,
    }
}

/// Write `body` so the file is either its old contents or its new ones.
fn write_file(path: &std::path::Path, body: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, body)?;
    std::fs::rename(&temporary, path)
}

/// One worker thread, files downloaded in order. A failure moves on to the
/// next; a cancel stops the run — either way the `.part` stays for a resumed
/// retry. `acknowledged` is when the operator confirmed the licences the
/// dialog named, and goes into every provenance record this run writes.
pub fn start_install(models: Vec<&'static Installable>, acknowledged: Option<u64>) -> InstallHandle {
    let (out, rx) = channel();
    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let _ = std::thread::Builder::new()
        .name("model-install".into())
        .spawn(move || {
            let downloader = match Downloader::from_env() {
                Ok(downloader) => downloader,
                Err(error) => {
                    if let Some(model) = models.first() {
                        let _ = out.send(InstallMsg::Failed {
                            short: model.short,
                            error: error.to_string(),
                        });
                    }
                    let _ = out.send(InstallMsg::Finished);
                    return;
                }
            };
            for model in models {
                let file = spec(model);
                let root = root_of(model);
                let mut last_percent = u64::MAX;
                let result = downloader.ensure_file(
                    &file,
                    &root,
                    &mut |progress: DownloadProgress| {
                        let total = progress.total.unwrap_or(model.bytes).max(1);
                        let percent = progress.done * 100 / total;
                        if percent != last_percent {
                            last_percent = percent;
                            let _ = out.send(InstallMsg::Progress {
                                short: model.short,
                                done: progress.done,
                                total,
                            });
                        }
                    },
                    &worker_cancel,
                );
                match result {
                    Ok(path) => {
                        // Provenance on the record: what was fetched, where
                        // it lives, under which licence, for which use.
                        println!(
                            "model-install: installed {} at {} — {} ({})",
                            model.id,
                            path.display(),
                            model.license,
                            model.use_.as_str()
                        );
                        if let Err(error) = write_file(&provenance_path(&path), provenance_text(model, &path, acknowledged)) {
                            println!("model-install: provenance record not written: {error}");
                        }
                        let _ = out.send(InstallMsg::Done { short: model.short });
                    }
                    Err(makepad_ai_hub::error::AssetAiError::Cancelled) => {
                        let _ = out.send(InstallMsg::Cancelled);
                        break;
                    }
                    Err(error) => {
                        let _ = out.send(InstallMsg::Failed {
                            short: model.short,
                            error: error.to_string(),
                        });
                    }
                }
            }
            let _ = out.send(InstallMsg::Finished);
        });
    InstallHandle { rx, cancel }
}

/// Hash the file on disk against the pinned sha256, reading in chunks (the
/// files are gigabytes); `progress(done, total)` as it goes. Blocking: run
/// it on a worker.
pub fn verify(model: &Installable, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    use std::io::Read;
    let path = weights_path(model);
    let mut file = std::fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let total = file.metadata().map(|meta| meta.len()).unwrap_or(model.bytes);
    if total != model.bytes {
        return Err(format!("{} bytes on disk, {} pinned", total, model.bytes));
    }
    let mut hasher = makepad_ai_hub::sha256::Sha256::new();
    let mut buffer = vec![0u8; 8 << 20];
    let mut done = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        done += read as u64;
        progress(done, total);
    }
    let hex = makepad_ai_hub::sha256::to_hex(&hasher.finalize());
    if hex == model.sha256 {
        Ok(())
    } else {
        Err(format!("sha256 {hex} does not match the pinned {}", model.sha256))
    }
}

/// Remove a file the installer put under its root: the file, its provenance
/// record, the downloader's receipt and any `.part` left behind. Files found
/// elsewhere (an env override, a probed path outside the managed root) are
/// the operator's own and are never deleted.
pub fn uninstall(model: &Installable) -> Result<(), String> {
    let dest = dest_path(model);
    let found = weights_path(model);
    if found != dest && found.is_file() {
        return Err(format!("{} is not managed here; remove it by hand", found.display()));
    }
    let mut part = dest.as_os_str().to_os_string();
    part.push(".part");
    for path in [
        dest.clone(),
        provenance_path(&dest),
        makepad_ai_hub::download::verification_path(&dest),
        PathBuf::from(part),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    println!("model-install: removed {} from {}", model.id, dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_pins_size_hash_and_revision() {
        let file = Installable { repo: "org/name", revision: "abc", ..Installable::resource("w", "MIT", "", "", 5, "ff", "w.bin", "m/w.bin") };
        let spec = spec(&file);
        assert_eq!((spec.size, spec.sha256.as_deref(), spec.revision.as_deref()), (Some(5), Some("ff"), Some("abc")));
        assert_eq!(spec.dest_path(std::path::Path::new("/r")), file.dest_under(std::path::Path::new("/r")));
    }
}
