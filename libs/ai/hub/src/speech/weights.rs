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
/// The Kokoro v1.1-zh model (100 Mandarin voices, 3 English), next to the v1.0 one.
pub const KOKORO_ZH_MODEL_FILE: &str = "kokoro-v1_1-zh.mktts";
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

/// Which Kokoro model a voice belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KokoroModel {
    V10,
    V11Zh,
}

/// The text front end (and symbol table) a voice speaks with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KokoroFront {
    English,
    EnglishV11,
    MandarinV10,
    MandarinV11,
}

/// The model and text front end a voice pack speaks through, by its name:
/// `zf_xiaobei` (v1.0 Mandarin), `zf_001` / `zm_009` (v1.1-zh Mandarin, numbered),
/// `af_maple` / `af_sol` / `bf_vale` (the v1.1-zh English voices) and everything
/// else (v1.0 English).
pub fn kokoro_plan(voice: &str) -> (KokoroModel, KokoroFront) {
    use KokoroFront as Frontend;
    let name = voice.strip_suffix(".mkvoice").unwrap_or(voice);
    let numbered = name.split_once('_').is_some_and(|(_, rest)| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
    if name.starts_with('z') {
        return if numbered { (KokoroModel::V11Zh, Frontend::MandarinV11) } else { (KokoroModel::V10, Frontend::MandarinV10) };
    }
    if matches!(name, "af_maple" | "af_sol" | "bf_vale") {
        return (KokoroModel::V11Zh, Frontend::EnglishV11);
    }
    (KokoroModel::V10, Frontend::English)
}

/// The v1.1-zh model file, if this machine has it.
pub fn kokoro_zh_model_path() -> Option<PathBuf> {
    if let Some(v10) = kokoro_model_path() {
        if let Some(sibling) = v10.parent().map(|d| d.join(KOKORO_ZH_MODEL_FILE)).filter(|p| p.is_file()) {
            return Some(sibling);
        }
    }
    first_file(candidates("MAKEPAD_TTS_ZH_MODEL", KOKORO_ZH_MODEL_FILE, "tts"))
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
        Some('z') => "zh-CN",
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

/// The installed voices with language and model, for `GET /voices`.
pub fn voices_json() -> crate::protocol::VoicesJson {
    let voices = kokoro_voices()
        .into_iter()
        .map(|v| {
            let model = match kokoro_plan(&v.id).0 {
                KokoroModel::V10 => "kokoro-v1.0",
                KokoroModel::V11Zh => "kokoro-v1.1-zh",
            };
            crate::protocol::VoiceJson {
                gender: match v.gender {
                    VoiceGender::Female => "female",
                    VoiceGender::Male => "male",
                    _ => "unknown",
                }
                .to_string(),
                id: v.id,
                name: v.name,
                language: v.language,
                model: model.to_string(),
            }
        })
        .collect();
    crate::protocol::VoicesJson { voices }
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
    fn voices_name_their_model_and_front_end() {
        use KokoroFront as Frontend;
        assert_eq!(kokoro_plan("bm_daniel"), (KokoroModel::V10, Frontend::English));
        assert_eq!(kokoro_plan("zf_xiaobei"), (KokoroModel::V10, Frontend::MandarinV10));
        assert_eq!(kokoro_plan("zm_yunxi.mkvoice"), (KokoroModel::V10, Frontend::MandarinV10));
        assert_eq!(kokoro_plan("zf_001"), (KokoroModel::V11Zh, Frontend::MandarinV11));
        assert_eq!(kokoro_plan("zm_009"), (KokoroModel::V11Zh, Frontend::MandarinV11));
        assert_eq!(kokoro_plan("af_maple"), (KokoroModel::V11Zh, Frontend::EnglishV11));
        assert_eq!(kokoro_voice("zf_001").language, "zh-CN");
    }

    #[test]
    fn the_voice_listing_names_language_and_model() {
        let v = voices_json();
        assert!(v.voices.iter().all(|v| !v.id.is_empty() && !v.language.is_empty()));
        // Whatever is installed, the model a voice reports follows its name.
        for voice in &v.voices {
            let want = if voice.id.starts_with('z') && voice.id.rsplit('_').next().is_some_and(|t| t.chars().all(|c| c.is_ascii_digit())) { "kokoro-v1.1-zh" } else if matches!(voice.id.as_str(), "af_maple" | "af_sol" | "bf_vale") { "kokoro-v1.1-zh" } else { "kokoro-v1.0" };
            assert_eq!(voice.model, want, "{}", voice.id);
        }
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
