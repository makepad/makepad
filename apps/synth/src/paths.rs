//! Where Synth keeps its files: wherever `SYNTH_DATA_DIR` says (a test run
//! keeps its data apart), else `local/synth` in the Makepad checkout Synth
//! runs from, else (an installed Synth) the app's data folder. Inside it:
//! `scores/` (the library, `*.score`), `drafts/` (unsaved changes),
//! `samples/` (audio a score plays by name), `renders/` (WAV exports),
//! `resources/` (the downloads: the GM SoundFont, the MIDI datasets) and
//! `batches/` (songs written at once).

use std::path::PathBuf;

/// The data folder, resolved once.
pub fn data_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(dir) = std::env::var_os("SYNTH_DATA_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
            return dir;
        }
        // The running binary's checkout first; the working directory covers
        // a binary built into a target dir outside it. Not on Android: there
        // the checkout is the APK's read-only source pack.
        #[cfg(not(target_os = "android"))]
        for start in [std::env::current_exe().ok(), std::env::current_dir().ok()].into_iter().flatten() {
            for root in start.ancestors() {
                if root.join("makepad.splash").is_file() && root.join("apps/synth/Cargo.toml").is_file() {
                    return root.join("local/synth");
                }
            }
        }
        makepad_widgets::makepad_platform::home::app_data_dir("synth").unwrap_or_else(|| PathBuf::from("synth-data"))
    }).clone()
}

/// A folder of the data folder, made on first use.
fn sub(name: &str) -> PathBuf {
    let dir = data_dir().join(name);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn scores_dir() -> PathBuf {
    sub("scores")
}

pub fn drafts_dir() -> PathBuf {
    sub("drafts")
}

pub fn samples_dir() -> PathBuf {
    sub("samples")
}

pub fn renders_dir() -> PathBuf {
    sub("renders")
}
