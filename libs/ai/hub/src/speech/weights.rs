//! Where the speech weights live, and what they are called.
//!
//! Resolution order for every file: an explicit env override, the working
//! directory, next to the executable (what a bundled app sees), then the
//! shared home `~/.makepad/weights/<stt|tts>/…` — the same cache paths the
//! registry's `cache_as` entries use, so a file the machine node downloaded
//! is found by every app on the machine without any coordination.

use crate::home;
use makepad_system_speech::{Voice, VoiceGender};
use std::path::{Path, PathBuf};

pub const WHISPER_MODEL_FILE: &str = "ggml-large-v3-turbo.bin";
pub const WHISPER_MODEL_ENV: &str = "MAKEPAD_VOICE_MODEL";
/// The registry id of the Whisper model the in-process engine loads and the
/// `stt.whisper` pipe serves.
pub const WHISPER_MODEL_ID: &str = "whisper-large-v3-turbo";

pub const KOKORO_MODEL_FILE: &str = "kokoro-v1_0.mktts";
pub const KOKORO_MODEL_ENV: &str = "MAKEPAD_TTS_MODEL";
pub const KOKORO_VOICE_ENV: &str = "MAKEPAD_TTS_VOICE";
pub const KOKORO_DEFAULT_VOICE: &str = "bm_daniel";
/// The registry id of the Kokoro model (`tts.kokoro`).
pub const KOKORO_MODEL_ID: &str = "kokoro";

/// Kokoro v1.0's English voice packs. Fixed for the model version, so a
/// remote `tts.kokoro` pipe can be listed without a voices endpoint.
pub const KOKORO_VOICE_NAMES: &[&str] = &[
    "af_alloy", "af_aoede", "af_bella", "af_heart", "af_jessica", "af_kore", "af_nicole", "af_nova",
    "af_river", "af_sarah", "af_sky", "am_adam", "am_echo", "am_eric", "am_fenrir", "am_liam",
    "am_michael", "am_onyx", "am_puck", "am_santa", "bf_alice", "bf_emma", "bf_isabella", "bf_lily",
    "bm_daniel", "bm_fable", "bm_george", "bm_lewis",
];

fn candidates(env: &str, name: &str, sub: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(path) = std::env::var(env) {
        if !path.trim().is_empty() {
            out.push(PathBuf::from(path));
        }
    }
    out.push(PathBuf::from(name));
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        out.push(dir.join(name));
    }
    out.push(home::weights_dir().join(sub).join(name));
    out
}

fn first_file(paths: Vec<PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|p| p.is_file())
}

/// The Whisper weights, if this machine has them.
pub fn whisper_model_path() -> Option<PathBuf> {
    first_file(candidates(WHISPER_MODEL_ENV, WHISPER_MODEL_FILE, "stt"))
}

/// The Kokoro weights, if this machine has them.
pub fn kokoro_model_path() -> Option<PathBuf> {
    first_file(candidates(KOKORO_MODEL_ENV, KOKORO_MODEL_FILE, "tts"))
}

/// A Kokoro voice pack by name (`"bm_daniel"` or `"bm_daniel.mkvoice"`),
/// searched next to the model and along the usual chain. `MAKEPAD_TTS_VOICE`
/// pointing at a file wins outright, as it always has.
pub fn kokoro_voice_path(name: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var(KOKORO_VOICE_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let file = if name.ends_with(".mkvoice") { name.to_string() } else { format!("{name}.mkvoice") };
    let mut paths = Vec::new();
    if let Some(dir) = kokoro_model_path().and_then(|m| m.parent().map(Path::to_path_buf)) {
        paths.push(dir.join(&file));
    }
    paths.extend(candidates("", &file, "tts").into_iter().skip(0));
    first_file(paths)
}

/// The voice the environment or the default asks for, as a bare pack name.
pub fn kokoro_default_voice() -> String {
    if let Ok(path) = std::env::var(KOKORO_VOICE_ENV) {
        if let Some(stem) = Path::new(&path).file_stem().and_then(|s| s.to_str()) {
            return stem.to_string();
        }
    }
    KOKORO_DEFAULT_VOICE.to_string()
}

/// A [`Voice`] for a Kokoro pack name: `bm_daniel` → "Daniel", en-GB, male.
pub fn kokoro_voice(name: &str) -> Voice {
    let stem = name.strip_suffix(".mkvoice").unwrap_or(name);
    let mut chars = stem.chars();
    let accent = chars.next();
    let gender = chars.next();
    let language = match accent {
        Some('a') => "en-US",
        Some('b') => "en-GB",
        _ => "en",
    };
    let gender = match gender {
        Some('f') => VoiceGender::Female,
        Some('m') => VoiceGender::Male,
        _ => VoiceGender::Unknown,
    };
    let bare = stem.split_once('_').map(|(_, rest)| rest).unwrap_or(stem);
    let mut pretty = String::new();
    for (i, c) in bare.chars().enumerate() {
        pretty.push(if i == 0 { c.to_ascii_uppercase() } else { c });
    }
    Voice { id: stem.to_string(), name: pretty, language: language.to_string(), gender, offline: true }
}

/// The Kokoro voices present on this machine: every `.mkvoice` next to the
/// model and along the resolution chain, deduplicated by name.
pub fn kokoro_voices() -> Vec<Voice> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = kokoro_model_path().and_then(|m| m.parent().map(Path::to_path_buf)) {
        dirs.push(dir);
    }
    dirs.push(PathBuf::from("."));
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        dirs.push(dir);
    }
    dirs.push(home::weights_dir().join("tts"));
    let mut names: Vec<String> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".mkvoice") {
                if !names.iter().any(|n| n == stem) {
                    names.push(stem.to_string());
                }
            }
        }
    }
    names.sort();
    names.iter().map(|n| kokoro_voice(n)).collect()
}

/// The full catalogue, for a remote Kokoro whose files we cannot see.
pub fn kokoro_voice_catalogue() -> Vec<Voice> {
    KOKORO_VOICE_NAMES.iter().map(|n| kokoro_voice(n)).collect()
}

/// The machine-election key for a weights file: its lowercase file name,
/// exactly as `hub_chat` keys the LLM.
pub fn election_key(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "unknown-model".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kokoro_voice_names_decode() {
        let v = kokoro_voice("bm_daniel");
        assert_eq!(v.id, "bm_daniel");
        assert_eq!(v.name, "Daniel");
        assert_eq!(v.language, "en-GB");
        assert_eq!(v.gender, VoiceGender::Male);
        let v = kokoro_voice("af_heart.mkvoice");
        assert_eq!((v.id.as_str(), v.language.as_str(), v.gender), ("af_heart", "en-US", VoiceGender::Female));
    }

    #[test]
    fn catalogue_has_all_28_voices() {
        assert_eq!(kokoro_voice_catalogue().len(), 28);
    }

    #[test]
    fn election_key_is_the_lowercase_file_name() {
        assert_eq!(election_key(Path::new("/x/GGML-Large-v3-turbo.bin")), "ggml-large-v3-turbo.bin");
    }
}
