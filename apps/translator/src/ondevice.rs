//! The phone's own models: Qwen3.5-4B (translation, and its vision tower for
//! the camera), Whisper (listening) and Kokoro (speaking), all in this
//! process on the GPU (Vulkan). Nothing leaves the phone.
//!
//! The weights live under one Makepad home, as on a desktop hub:
//! `weights/llm/` (the Qwen GGUF and its mmproj), `weights/stt/` (Whisper),
//! `weights/tts/` (Kokoro models and voice packs). On Android that home is
//! the app's files directory, or its external files directory
//! (`/sdcard/Android/data/<package>/files/makepad`), the one a computer can
//! copy the weights into.

use std::path::{Path, PathBuf};

/// The model id that selects the phone's own models in the settings.
pub const MODEL: &str = "on-device";
/// What the settings call the phone's translation model.
pub const LABEL: &str = "Qwen3.5-4B";

/// The translation model's file: our converted Qwen3.5-4B first, the earlier
/// Q5_K_M (sideloaded) as the fallback.
pub const QWEN_FILES: [&str; 2] = ["Qwen3.5-4B-MTP-MIX-b.gguf", "Qwen3.5-4B-Q5_K_M.gguf"];
#[cfg(target_os = "android")]
pub const MMPROJ_FILES: [&str; 2] = ["mmproj-Makepad-q8_0.gguf", "mmproj-F16.gguf"];
pub const WHISPER_FILE: &str = "ggml-large-v3-turbo-q5_k.bin";

fn first_file(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Weights complete enough to translate: a Qwen GGUF under `home`.
fn has_weights(home: &Path) -> bool {
    first_file(&home.join("weights/llm"), &QWEN_FILES).is_some()
}

/// The package this process runs as (on Android the process name).
fn package() -> Option<String> {
    let cmdline = std::fs::read("/proc/self/cmdline").ok()?;
    let name = String::from_utf8_lossy(cmdline.split(|b| *b == 0).next().unwrap_or_default()).into_owned();
    Some(name.split(':').next().unwrap_or_default().to_string()).filter(|p| !p.is_empty())
}

/// Where the phone's models are installed by the app: its own files directory
/// (a computer can also copy them into the external one, which is searched too).
pub fn install_home() -> Option<PathBuf> {
    if !cfg!(target_os = "android") {
        return None;
    }
    // Weights already on the phone (sideloaded) are used where they are.
    if let Ok(home) = activate() {
        return Some(home);
    }
    Some(Path::new("/data/user/0").join(package()?).join("files/makepad"))
}

/// Picks the Makepad home that holds the on-device weights and points the
/// in-process speech engines and the weights lookup at it. Ok once the weights
/// are there; Err (with the places looked at) while they are not, and asked
/// again each time (the store may have finished installing them since).
pub fn activate() -> Result<PathBuf, String> {
    static ACTIVE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(home) = active.as_ref() {
        return Ok(home.clone());
    }
    let home = find_and_set()?;
    *active = Some(home.clone());
    Ok(home)
}

fn find_and_set() -> Result<PathBuf, String> {
    if !cfg!(target_os = "android") {
        return Err("on-device models run on Android only".into());
    }
    let package = package().ok_or("no process name")?;
    let homes = [
        Path::new("/data/user/0").join(&package).join("files/makepad"),
        Path::new("/sdcard/Android/data").join(&package).join("files/makepad"),
    ];
    let Some(home) = homes.iter().find(|home| has_weights(home)).cloned() else {
        let tried: Vec<String> = homes.iter().map(|h| h.join("weights/llm").display().to_string()).collect();
        return Err(format!("the on-device model files are not on this phone: {}", tried.join(", ")));
    };
    // SAFETY: runs from startup or the first on-device job, before the speech
    // engines read the environment, and again only when the weights arrive.
    unsafe {
        std::env::set_var("MAKEPAD_HOME", &home);
        if let Some(whisper) = first_file(&home.join("weights/stt"), &[WHISPER_FILE]) {
            std::env::set_var("MAKEPAD_VOICE_MODEL", whisper);
        }
    }
    Ok(home)
}

/// Where speech runs: on the phone (Whisper and Kokoro load into this
/// process) or, `false`, on the hub (Hub mode: the phone's copies stay out of
/// the way, or the speech sessions would pick them first). Call before a
/// speech session starts; a session keeps the engine it found.
pub fn speech_in_process(on: bool) {
    // SAFETY: set from the UI thread just before a speech session starts; the
    // speech crate reads it when that session picks its engine.
    unsafe {
        if on {
            std::env::set_var("MAKEPAD", "whisper+kokoro");
        } else {
            std::env::remove_var("MAKEPAD");
        }
    }
}

/// The Makepad home holding the on-device weights, if any.
pub fn home() -> Option<PathBuf> {
    activate().ok()
}

pub fn available() -> bool {
    home().is_some()
}

/// The translation model and its vision tower.
#[cfg(target_os = "android")]
pub fn model_paths() -> Result<(PathBuf, Option<PathBuf>), String> {
    let llm = activate()?.join("weights/llm");
    let qwen = first_file(&llm, &QWEN_FILES).ok_or("the translation model is gone")?;
    Ok((qwen, first_file(&llm, &MMPROJ_FILES)))
}
