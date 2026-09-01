//! The TTS session worker: choose a voice engine once, then render utterances
//! in order. Audio goes back as PCM; the app plays it.

use super::remote::RemotePipe;
use super::weights;
use super::{SpeechAudio, SpeechReach, TtsConfig, TtsEngine, TtsEngineInfo, TtsEvent, TtsMsg};
use crate::pipe::PipeId;
use crate::registry::Domain;
use makepad_system_speech as sys;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

pub(crate) fn run(
    config: TtsConfig,
    msg_rx: Receiver<TtsMsg>,
    event_tx: Sender<TtsEvent>,
    generation: Arc<AtomicU64>,
) {
    let wake = config.wake.clone();
    let send = move |event: TtsEvent| {
        let _ = event_tx.send(event);
        if let Some(wake) = &wake {
            wake();
        }
    };

    let mut engine = match choose(&config, &send) {
        Ok(engine) => engine,
        Err(why) => return send(TtsEvent::Failed(why)),
    };
    send(TtsEvent::Ready(engine.info()));

    while let Ok(TtsMsg::Say { utterance, generation: mine, text }) = msg_rx.recv() {
        if mine != generation.load(Ordering::Relaxed) {
            continue;
        }
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let started = Instant::now();
        match engine.synthesize(text, &config) {
            Ok(audio) if !audio.is_empty() => {
                // Synthesis is slow enough that a cancel can land while it
                // runs; a stale utterance must never be heard.
                if mine == generation.load(Ordering::Relaxed) {
                    send(TtsEvent::Audio { utterance, audio, secs: started.elapsed().as_secs_f64() });
                }
            }
            Ok(_) => send(TtsEvent::Error { utterance, message: "engine produced no audio".into() }),
            Err(message) => send(TtsEvent::Error { utterance, message }),
        }
    }
}

trait Engine {
    fn info(&self) -> TtsEngineInfo;
    fn synthesize(&mut self, text: &str, config: &TtsConfig) -> Result<SpeechAudio, String>;
}

// ----------------------------------------------------------------- choosing

fn choose(config: &TtsConfig, send: &dyn Fn(TtsEvent)) -> Result<Box<dyn Engine>, String> {
    let want_kokoro = matches!(config.engine, TtsEngine::Auto | TtsEngine::Kokoro);
    let want_system = matches!(config.engine, TtsEngine::Auto | TtsEngine::System);
    let mut reasons: Vec<String> = Vec::new();

    if want_kokoro {
        #[cfg(feature = "tts")]
        {
            if !super::in_process_allowed("kokoro") {
                reasons.push("in-process kokoro is off on this platform (MAKEPAD=kokoro enables it)".into());
            } else if let Some(path) = weights::kokoro_model_path() {
                match kokoro::elect_and_load(&path, config, send) {
                    Ok(engine) => return Ok(engine),
                    Err(why) => reasons.push(why),
                }
            } else {
                reasons.push(format!("no kokoro weights ({}) on this machine", weights::KOKORO_MODEL_FILE));
            }
        }
        #[cfg(not(feature = "tts"))]
        reasons.push("kokoro is not compiled into this build".into());

        if config.reach >= SpeechReach::Machine {
            send(TtsEvent::Loading { phase: "looking for a kokoro node".into(), fraction: 0.0 });
            match RemotePipe::find(config.reach, Domain::Speech, &["kokoro"]) {
                Some(pipe) => return Ok(Box::new(RemoteTts { pipe })),
                None => reasons.push(format!("no node in reach ({:?}) serves tts.kokoro", config.reach)),
            }
        }
    }

    if want_system {
        if sys::tts::available() {
            return Ok(Box::new(SystemTts));
        }
        reasons.push(format!("no system voice here ({})", sys::tts::engine_name()));
    }

    Err(reasons.join("; "))
}

/// The Kokoro voice a config asks for, as a bare pack name.
fn kokoro_voice_name(config: &TtsConfig) -> String {
    config
        .voice
        .as_deref()
        .map(|v| v.strip_suffix(".mkvoice").unwrap_or(v).to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(weights::kokoro_default_voice)
}

// ------------------------------------------------------------- the OS engine

struct SystemTts;

impl Engine for SystemTts {
    fn info(&self) -> TtsEngineInfo {
        TtsEngineInfo {
            pipe: PipeId::new("tts.system"),
            engine: sys::tts::engine_name().to_string(),
            remote: None,
            // The OS decides per voice (Apple 22.05 kHz, Windows 16/22/24 kHz
            // by voice); every `Audio` event carries its own rate.
            sample_rate: 0,
            voices: sys::tts::voices(),
        }
    }

    fn synthesize(&mut self, text: &str, config: &TtsConfig) -> Result<SpeechAudio, String> {
        let options = sys::TtsOptions {
            voice: config.voice.clone(),
            language: config.language.clone(),
            rate: config.rate,
            pitch: config.pitch,
        };
        sys::tts::synthesize(text, &options).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------- remote pipe

struct RemoteTts {
    pipe: RemotePipe,
}

impl Engine for RemoteTts {
    fn info(&self) -> TtsEngineInfo {
        TtsEngineInfo {
            pipe: PipeId::new("tts.kokoro"),
            engine: format!("kokoro ({})", self.pipe.model),
            remote: Some(self.pipe.base_url.clone()),
            sample_rate: 24_000,
            voices: weights::kokoro_voice_catalogue(),
        }
    }

    fn synthesize(&mut self, text: &str, config: &TtsConfig) -> Result<SpeechAudio, String> {
        self.pipe.synthesize(text, &kokoro_voice_name(config), config.rate)
    }
}

// -------------------------------------------------------- in-process kokoro

#[cfg(feature = "tts")]
mod kokoro {
    use super::super::Voice;
    use super::*;
    use crate::machine::{self, Claim, ResidencyState};
    use makepad_ai_speech::kokoro::KokoroSpeaker;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    const HOLDER_PATIENCE: Duration = Duration::from_secs(30);
    const POLL: Duration = Duration::from_millis(150);

    pub(super) struct KokoroLocal {
        model_path: PathBuf,
        speaker: KokoroSpeaker,
        voice: String,
        voices: Vec<Voice>,
        _residency: Option<machine::ResidencyGuard>,
    }

    impl Engine for KokoroLocal {
        fn info(&self) -> TtsEngineInfo {
            TtsEngineInfo {
                pipe: PipeId::new("tts.kokoro"),
                engine: format!("kokoro ({})", weights::KOKORO_MODEL_FILE),
                remote: None,
                sample_rate: makepad_ai_speech::kokoro::SAMPLE_RATE,
                voices: self.voices.clone(),
            }
        }

        fn synthesize(&mut self, text: &str, config: &TtsConfig) -> Result<SpeechAudio, String> {
            let wanted = kokoro_voice_name(config);
            if wanted != self.voice {
                // A voice is a 510x256 style table; the speaker reloads with
                // it. Rare (the config picks one voice), so the reload cost
                // is acceptable.
                let voice_path = weights::kokoro_voice_path(&wanted)
                    .ok_or_else(|| format!("kokoro voice pack {wanted}.mkvoice not found"))?;
                self.speaker = KokoroSpeaker::load_with_voice(
                    &self.model_path.to_string_lossy(),
                    &voice_path.to_string_lossy(),
                )
                .map_err(|e| format!("kokoro load voice {wanted}: {e:?}"))?;
                self.voice = wanted;
            }
            let audio = self
                .speaker
                .synthesize_with_speed(text, config.rate)
                .map_err(|e| format!("kokoro: {e:?}"))?;
            Ok(SpeechAudio { samples: audio.samples, sample_rate: audio.sample_rate })
        }
    }

    pub(super) fn elect_and_load(
        path: &Path,
        config: &TtsConfig,
        send: &dyn Fn(TtsEvent),
    ) -> Result<Box<dyn Engine>, String> {
        let key = weights::election_key(path);
        let deadline = Instant::now() + HOLDER_PATIENCE;
        loop {
            match machine::read_holder(&key) {
                Ok(Some(record)) => match record.state {
                    ResidencyState::Ready { port } if port > 0 && config.reach >= SpeechReach::Machine => {
                        let url = format!("http://127.0.0.1:{port}");
                        if let Some((_, pipe)) = RemotePipe::at(&url, Domain::Speech, &["kokoro"]) {
                            return Ok(Box::new(RemoteTts { pipe }));
                        }
                        break;
                    }
                    ResidencyState::Ready { .. } => {
                        eprintln!("[hub-tts] {key}: held by pid {} without a usable route — loading a duplicate copy", record.pid);
                        break;
                    }
                    ResidencyState::Loading { fraction } => {
                        send(TtsEvent::Loading { phase: format!("waiting on pid {}", record.pid), fraction });
                        if Instant::now() > deadline {
                            break;
                        }
                        std::thread::sleep(POLL * 4);
                    }
                    ResidencyState::Failed { .. } => break,
                },
                _ => break,
            }
        }

        let mut guard = match machine::claim(&key) {
            Ok(Claim::Won(mut guard)) => {
                let _ = guard.publish(ResidencyState::Loading { fraction: 0.0 });
                Some(guard)
            }
            _ => None,
        };

        let voice = kokoro_voice_name(config);
        let voice_path = match weights::kokoro_voice_path(&voice) {
            Some(path) => path,
            None => {
                let reason = format!("kokoro voice pack {voice}.mkvoice not found");
                if let Some(guard) = guard.as_mut() {
                    let _ = guard.publish(ResidencyState::Failed { reason: reason.clone(), retry_after_ms: unix_ms() + 30_000 });
                }
                return Err(reason);
            }
        };
        send(TtsEvent::Loading { phase: format!("loading {}", weights::KOKORO_MODEL_FILE), fraction: 0.0 });
        let started = Instant::now();
        let mut speaker = match KokoroSpeaker::load_with_voice(&path.to_string_lossy(), &voice_path.to_string_lossy()) {
            Ok(speaker) => speaker,
            Err(error) => {
                let reason = format!("could not load {}: {error:?}", path.display());
                if let Some(guard) = guard.as_mut() {
                    let _ = guard.publish(ResidencyState::Failed { reason: reason.clone(), retry_after_ms: unix_ms() + 30_000 });
                }
                return Err(reason);
            }
        };
        // Discarded warm-up: the first synthesis initializes the Metal
        // context on this thread — better now than on the first sentence.
        let _ = speaker.synthesize("Hi.");
        if let Some(guard) = guard.as_mut() {
            let _ = guard.publish(ResidencyState::Ready { port: 0 });
        }
        send(TtsEvent::Loading {
            phase: format!("loaded kokoro in {:.1}s", started.elapsed().as_secs_f64()),
            fraction: 1.0,
        });
        Ok(Box::new(KokoroLocal {
            model_path: path.to_path_buf(),
            speaker,
            voice,
            voices: weights::kokoro_voices(),
            _residency: guard,
        }))
    }

    fn unix_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
