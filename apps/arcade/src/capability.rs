//! What this device can actually run (game.md §"Per-platform capability
//! fallback").
//!
//! The local models (Silero/Whisper/Qwen) need a compute backend that does not
//! exist everywhere. ONE code path, two configurations: where the chain
//! exists, the mic is a talk-gate; where it does not, the text box IS the
//! gate and typing goes straight to the cloud agent.

use std::path::{Path, PathBuf};

/// Which local models are present and runnable on this device.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// A GPU/CPU backend makepad-ggml can actually run local models on.
    pub local_compute: bool,
    /// Silero VAD weights — else the mic falls back to an RMS gate.
    pub vad_model: Option<PathBuf>,
    /// Whisper weights — without these there is no transcription, so no voice.
    pub whisper_model: Option<PathBuf>,
    /// Local judge/librarian LLM — without it, typing is the gate and the
    /// librarian degrades to fuzzy name matching.
    pub qwen_model: Option<PathBuf>,
    /// Kokoro voice pack — else replies are text only.
    pub tts_voice: Option<PathBuf>,
}

/// The tier the app actually runs in. Ordered: each tier is a superset of the
/// one below for user-visible capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// mic → VAD → Whisper → local judge → agent, spoken replies.
    Voice,
    /// mic → (RMS or VAD) → Whisper → agent, no local judge: every utterance
    /// that clears the gate costs a cloud call, so the mic is push-to-talk.
    VoiceUnfiltered,
    /// No local models: the text box is the talk-gate.
    Chatbox,
}

impl Tier {
    pub fn shows_mic(self) -> bool {
        !matches!(self, Tier::Chatbox)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tier::Voice => "voice (VAD + local judge)",
            Tier::VoiceUnfiltered => "voice (push-to-talk, no local judge)",
            Tier::Chatbox => "chatbox (no local model compute)",
        }
    }
}

fn find(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.exists()).cloned()
}

fn repo_root() -> PathBuf {
    // Models live untracked at the repo root (see game.md §assets rule).
    std::env::var("MAKEPAD_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

impl Capabilities {
    /// Probe the device. Env overrides win so a test can force any tier.
    pub fn detect() -> Self {
        let root = repo_root();
        Self {
            local_compute: local_compute_available(),
            vad_model: env_or(
                "MAKEPAD_VAD_MODEL",
                &[root.join("silero_vad.onnx")],
            ),
            whisper_model: env_or(
                "MAKEPAD_WHISPER_MODEL",
                &[root.join("ggml-large-v3-turbo.bin")],
            ),
            qwen_model: env_or(
                "ARCADE_QWEN_MODEL",
                &[
                    root.join("local/models/Qwen3.5-4B-Q5_K_M.gguf"),
                    root.join("local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf"),
                ],
            ),
            tts_voice: env_or("ARCADE_TTS_VOICE", &[root.join("bm_fable.mkvoice")]),
        }
    }

    pub fn tier(&self) -> Tier {
        if !self.local_compute || self.whisper_model.is_none() {
            return Tier::Chatbox;
        }
        if self.qwen_model.is_some() {
            Tier::Voice
        } else {
            Tier::VoiceUnfiltered
        }
    }

    /// One startup line naming the tier and exactly what is missing — the
    /// difference between "voice is broken" and "you did not download X".
    pub fn report(&self) -> String {
        let mut missing = Vec::new();
        if !self.local_compute {
            missing.push("local model compute");
        }
        if self.whisper_model.is_none() {
            missing.push("whisper weights");
        }
        if self.vad_model.is_none() {
            missing.push("silero VAD (RMS gate instead)");
        }
        if self.qwen_model.is_none() {
            missing.push("local judge/librarian LLM");
        }
        if self.tts_voice.is_none() {
            missing.push("kokoro voice (text replies only)");
        }
        if missing.is_empty() {
            format!("arcade: tier {} — all local models present", self.tier().label())
        } else {
            format!(
                "arcade: tier {} — missing: {}",
                self.tier().label(),
                missing.join(", ")
            )
        }
    }
}

fn env_or(var: &str, candidates: &[PathBuf]) -> Option<PathBuf> {
    if let Ok(v) = std::env::var(var) {
        if v.is_empty() {
            return None; // explicit "pretend it is absent", for tests
        }
        let p = PathBuf::from(v);
        return p.exists().then_some(p);
    }
    find(candidates)
}

/// Local model compute: a metal/CUDA-class backend the ggml stack can use.
/// `ARCADE_NO_LOCAL=1` forces the chatbox tier (the Quest/mobile shape) on a
/// machine that does have it.
pub fn local_compute_available() -> bool {
    if std::env::var("ARCADE_NO_LOCAL").is_ok() {
        return false;
    }
    // The local-llm feature is what actually links the compute backend; on
    // platforms where it is off, there is nothing to run models on.
    cfg!(feature = "local-llm") && cfg!(any(target_os = "macos", target_os = "linux", target_os = "windows"))
}

/// Where games live: one directory per game.
pub fn arcade_home() -> PathBuf {
    std::env::var("ARCADE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            Path::new(&home).join("arcade-games")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(compute: bool, whisper: bool, qwen: bool) -> Capabilities {
        Capabilities {
            local_compute: compute,
            vad_model: None,
            whisper_model: whisper.then(|| PathBuf::from("w")),
            qwen_model: qwen.then(|| PathBuf::from("q")),
            tts_voice: None,
        }
    }

    #[test]
    fn tier_falls_back_piece_by_piece() {
        // Everything present -> full voice chain.
        assert_eq!(caps(true, true, true).tier(), Tier::Voice);
        // No judge -> voice still works, but every utterance costs a call.
        assert_eq!(caps(true, true, false).tier(), Tier::VoiceUnfiltered);
        // No whisper -> nothing to transcribe, so typing is the gate.
        assert_eq!(caps(true, false, true).tier(), Tier::Chatbox);
        // No compute (Quest/mobile/web) -> chatbox regardless of files.
        assert_eq!(caps(false, true, true).tier(), Tier::Chatbox);
    }

    #[test]
    fn the_text_box_exists_in_every_tier_but_the_mic_does_not() {
        assert!(caps(true, true, true).tier().shows_mic());
        assert!(caps(true, true, false).tier().shows_mic());
        assert!(!caps(false, true, true).tier().shows_mic());
    }

    #[test]
    fn report_names_what_is_missing() {
        let r = caps(true, true, false).report();
        assert!(r.contains("local judge"), "{r}");
        let full = Capabilities {
            local_compute: true,
            vad_model: Some(PathBuf::from("v")),
            whisper_model: Some(PathBuf::from("w")),
            qwen_model: Some(PathBuf::from("q")),
            tts_voice: Some(PathBuf::from("t")),
        };
        assert!(full.report().contains("all local models present"), "{}", full.report());
    }
}
