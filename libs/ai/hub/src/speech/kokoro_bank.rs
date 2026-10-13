//! The Kokoro models one process keeps, and which voice speaks through which.
//!
//! Two Kokoro releases are in use: v1.0 (English, and the 8 older Mandarin
//! voices) and v1.1-zh (the 100 Mandarin voices of the LongMaoData set, plus
//! three English ones). Their symbol tables differ, so a voice names its model
//! and text front end ([`plan_for_voice`]); the bank loads each model once, on
//! the first voice that needs it, and keeps it. Voice packs are read once and
//! swapped under the loaded weights.

use super::weights::{self, KokoroModel};
use makepad_ai_speech::g2p::Frontend;
use makepad_ai_speech::kokoro::KokoroSpeaker;
use makepad_ai_speech::SpeechAudio;
use std::collections::HashMap;
use std::path::PathBuf;

struct Slot {
    speaker: KokoroSpeaker,
    /// The voice whose style table the speaker holds now.
    voice: String,
}

pub struct KokoroBank {
    models: [Option<PathBuf>; 2],
    slots: [Option<Slot>; 2],
    packs: HashMap<String, Vec<f32>>,
}

fn index(model: KokoroModel) -> usize {
    match model {
        KokoroModel::V10 => 0,
        KokoroModel::V11Zh => 1,
    }
}

impl KokoroBank {
    /// `v10` is the English model; `v11_zh` is optional (Mandarin voices of
    /// the v1.1-zh set fail with a clear message without it).
    pub fn new(v10: PathBuf, v11_zh: Option<PathBuf>) -> Self {
        Self { models: [Some(v10), v11_zh], slots: [None, None], packs: HashMap::new() }
    }

    /// The v1.1-zh model next to a v1.0 one, if it is there.
    pub fn sibling_zh_model(v10: &std::path::Path) -> Option<PathBuf> {
        let path = v10.parent()?.join(weights::KOKORO_ZH_MODEL_FILE);
        path.is_file().then_some(path)
    }

    /// Speak `text` with `voice`. `voice_path` finds a voice pack by name.
    pub fn synthesize(
        &mut self,
        voice: &str,
        voice_path: &dyn Fn(&str) -> Option<PathBuf>,
        text: &str,
        speed: f32,
        on_chunk: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<SpeechAudio, String> {
        let speaker = self.speaker_for(voice, voice_path)?;
        speaker.synthesize_with_speed_observed(text, speed, on_chunk).map_err(|e| format!("kokoro: {e:?}"))
    }

    /// [`Self::synthesize`] that hands each clause's samples to `on_audio` as soon as
    /// it is rendered (see `KokoroSpeaker::synthesize_streaming`).
    pub fn synthesize_streaming(
        &mut self,
        voice: &str,
        voice_path: &dyn Fn(&str) -> Option<PathBuf>,
        text: &str,
        speed: f32,
        on_chunk: &mut dyn FnMut(usize, usize) -> bool,
        on_audio: &mut dyn FnMut(&[f32]),
    ) -> Result<SpeechAudio, String> {
        let speaker = self.speaker_for(voice, voice_path)?;
        speaker.synthesize_streaming(text, speed, on_chunk, on_audio).map_err(|e| format!("kokoro: {e:?}"))
    }

    /// The loaded speaker for `voice`'s model, its style table and front end set.
    fn speaker_for(&mut self, voice: &str, voice_path: &dyn Fn(&str) -> Option<PathBuf>) -> Result<&mut KokoroSpeaker, String> {
        let (model, front) = weights::kokoro_plan(voice);
        let at = index(model);
        let frontend = match front {
            weights::KokoroFront::English => Frontend::English,
            weights::KokoroFront::EnglishV11 => Frontend::EnglishV11,
            weights::KokoroFront::MandarinV10 => Frontend::MandarinV10,
            weights::KokoroFront::MandarinV11 => Frontend::MandarinV11,
        };
        if !self.packs.contains_key(voice) {
            let path = voice_path(voice).ok_or_else(|| format!("kokoro voice pack {voice}.mkvoice not found"))?;
            let pack = KokoroSpeaker::read_voice(&path.to_string_lossy())
                .map_err(|e| format!("kokoro load voice {voice}: {e:?}"))?;
            self.packs.insert(voice.to_string(), pack);
        }
        if self.slots[at].is_none() {
            let model_path = self.models[at].clone().ok_or_else(|| {
                format!("voice {voice} needs the Kokoro v1.1-zh model ({}), which is not installed", weights::KOKORO_ZH_MODEL_FILE)
            })?;
            let speaker = KokoroSpeaker::load_with_voice(&model_path.to_string_lossy(), &self.pack_path(voice, voice_path)?)
                .map_err(|e| format!("kokoro load {}: {e:?}", model_path.display()))?;
            self.slots[at] = Some(Slot { speaker, voice: voice.to_string() });
        }
        let slot = self.slots[at].as_mut().unwrap();
        if slot.voice != voice {
            slot.speaker.set_voice(self.packs[voice].clone());
            slot.voice = voice.to_string();
        }
        slot.speaker.set_frontend(frontend);
        Ok(&mut slot.speaker)
    }

    fn pack_path(&self, voice: &str, voice_path: &dyn Fn(&str) -> Option<PathBuf>) -> Result<String, String> {
        voice_path(voice)
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or_else(|| format!("kokoro voice pack {voice}.mkvoice not found"))
    }
}
