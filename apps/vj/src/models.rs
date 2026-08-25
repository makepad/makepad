//! First-use install of the deck models: the BS-RoFormer stem splitter and
//! the whisper karaoke transcriber.
//!
//! The downloader is the asset-ai service's own (`makepad-asset-ai` with
//! default features off — the same featureless slice the asset UI links):
//! resumable `.part` files, pinned size + sha256, atomic commit, a receipt
//! beside the file and a cross-process lock per artifact. Weights land in
//! the checkout's `local/` tree exactly where the existing probes already
//! look — `local/stems_ref/ckpt/` for the splitter, `local/models/` for
//! whisper — so `VJ_STEMS_CKPT` / `MAKEPAD_VOICE_MODEL` overrides keep
//! working and an already-provisioned machine simply shows as installed.
//!
//! Nothing downloads until the operator presses INSTALL MODELS and confirms
//! the dialog naming both weight sets, their sizes and their licenses. Both
//! are MIT, so no acknowledgement is recorded — the dialog is information,
//! not a gate ceremony.

use makepad_asset_ai::backend::CancelToken;
use makepad_asset_ai::download::{DownloadProgress, Downloader};
use makepad_asset_ai::registry::FileSpec;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

/// One installable model file: identity for the ack record, provenance for
/// the log, and the pinned download the shared downloader verifies against.
pub struct VjModel {
    /// Stable id the ack file records.
    pub id: &'static str,
    /// Short name for the one-line progress display.
    pub short: &'static str,
    pub license: &'static str,
    pub bytes: u64,
    sha256: &'static str,
    /// HuggingFace repo, empty when `path` is an absolute URL.
    repo: &'static str,
    /// Repo-relative file, or an absolute URL fetched verbatim.
    path: &'static str,
    /// Destination under [`cache_root`], '/'-separated.
    cache_as: &'static str,
}

pub const STEMS: VjModel = VjModel {
    id: makepad_ai_stems::MODEL_ID,
    short: "stems",
    license: makepad_ai_stems::MODEL_LICENSE,
    bytes: 527_385_512,
    sha256: "3e9daecd70aaed5b5a0d1f861cc4d77eaa45afb3fc6301b1cf32c1be0f5868fb",
    repo: "",
    path: "https://github.com/ZFTurbo/Music-Source-Separation-Training/releases/download/v1.0.12/model_bs_roformer_ep_17_sdr_9.6568.ckpt",
    cache_as: "stems_ref/ckpt/model_bs_roformer_ep_17_sdr_9.6568.ckpt",
};

pub const WHISPER: VjModel = VjModel {
    id: "whisper-large-v3-turbo",
    short: "whisper",
    license: "MIT (OpenAI Whisper; ggml conversion by ggerganov/whisper.cpp)",
    bytes: 1_624_555_275,
    sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    repo: "ggerganov/whisper.cpp",
    path: "ggml-large-v3-turbo.bin",
    cache_as: "models/ggml-large-v3-turbo.bin",
};

impl VjModel {
    fn spec(&self) -> FileSpec {
        FileSpec {
            role: None,
            repo: self.repo.to_string(),
            path: self.path.to_string(),
            revision: None,
            cache_as: self.cache_as.to_string(),
            size: Some(self.bytes),
            sha256: Some(self.sha256.to_string()),
            local: false,
            converts_to: None,
            conversion: None,
        }
    }
}

/// The checkout's `local/` tree — where the stems and lyrics probes look.
fn cache_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local")
}

/// A managed install only counts when the bytes on disk are exactly the
/// pinned artifact — `is_file()` alone let a truncated or hand-copied file
/// read as provisioned, and then load broken. One stat, no hashing (the
/// downloader's fsync-then-rename commit means a file of the right length
/// at the dest went through the sha check).
pub fn dest_is_installed(model: &VjModel) -> bool {
    file_is_pinned_size(&model.spec().dest_path(&cache_root()), model.bytes)
}

pub fn file_is_pinned_size(path: &std::path::Path, bytes: u64) -> bool {
    std::fs::metadata(path).map(|meta| meta.len() == bytes).unwrap_or(false)
}

/// The models this machine still lacks, judged by the same resolution the
/// consumers use — env overrides and every alternate probe path count as
/// installed (the operator's own hands are trusted); the managed dest
/// itself must match its pinned size.
pub fn missing() -> Vec<&'static VjModel> {
    let mut out = Vec::new();
    let stems_installed = if std::env::var("VJ_STEMS_CKPT").is_ok() {
        // Same rule as the managed dest: a checkpoint that is not exactly
        // the pinned size is an interrupted download, not an install.
        file_is_pinned_size(&crate::stems::checkpoint_path(), STEMS.bytes)
    } else {
        dest_is_installed(&STEMS)
    };
    if !stems_installed {
        out.push(&STEMS);
    }
    if crate::lyrics::whisper_model_path().is_none() {
        out.push(&WHISPER);
    }
    out
}

// ---------------------------------------------------------------------------
// the install worker
// ---------------------------------------------------------------------------

pub enum InstallMsg {
    Progress { short: &'static str, done: u64, total: u64 },
    Done { short: &'static str },
    Failed { short: &'static str, error: String },
    /// The operator pulled the plug; `.part` files stay for a later resume.
    Cancelled,
    /// Every requested model has been tried. The receiver decides what the
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

/// One worker thread, models downloaded in order. A failure moves on to the
/// next model; a cancel stops the run — either way the `.part` stays for a
/// resumed retry.
pub fn start_install(models: Vec<&'static VjModel>) -> InstallHandle {
    let (out, rx) = channel();
    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let _ = std::thread::Builder::new()
        .name("vj-model-install".into())
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
            let root = cache_root();
            for model in models {
                let spec = model.spec();
                let mut last_percent = u64::MAX;
                let result = downloader.ensure_file(
                    &spec,
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
                        // it lives, and under which license.
                        println!(
                            "models: installed {} at {} — {}",
                            model.id,
                            path.display(),
                            model.license
                        );
                        let _ = out.send(InstallMsg::Done { short: model.short });
                    }
                    Err(makepad_asset_ai::error::AssetAiError::Cancelled) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_install_lands_where_the_worker_looks() {
        let dest = STEMS.spec().dest_path(&cache_root());
        assert_eq!(
            dest.file_name().unwrap().to_str().unwrap(),
            makepad_ai_stems::MODEL_CHECKPOINT
        );
        if std::env::var("VJ_STEMS_CKPT").is_err() {
            assert_eq!(dest, crate::stems::checkpoint_path());
        }
    }

    #[test]
    fn a_partial_file_never_reads_as_installed() {
        let dir = std::env::temp_dir().join(format!("vj-models-gate-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("model.bin");
        assert!(!file_is_pinned_size(&path, 8), "absent file counted as installed");
        std::fs::write(&path, b"1234").unwrap();
        assert!(!file_is_pinned_size(&path, 8), "truncated file counted as installed");
        std::fs::write(&path, b"12345678").unwrap();
        assert!(file_is_pinned_size(&path, 8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whisper_install_lands_on_a_lyrics_probe_path() {
        let dest = WHISPER.spec().dest_path(&cache_root());
        let parts: Vec<_> = dest
            .components()
            .rev()
            .take(3)
            .map(|part| part.as_os_str().to_string_lossy().to_string())
            .collect();
        assert_eq!(parts, ["ggml-large-v3-turbo.bin", "models", "local"]);
    }

}
