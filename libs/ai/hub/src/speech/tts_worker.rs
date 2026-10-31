//! The TTS session worker: choose a voice engine once, then render utterances
//! in order. Audio goes back as PCM; the app plays it.

use super::remote::RemotePipe;
use super::weights;
use makepad_system_speech::Voice;
use super::{SpeechAudio, SpeechReach, TtsConfig, TtsEngine, TtsEngineInfo, TtsEvent, TtsMsg, WordTiming};
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

    while let Ok(TtsMsg::Say { utterance, generation: mine, text, options }) = msg_rx.recv() {
        if mine != generation.load(Ordering::Relaxed) {
            continue;
        }
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let started = Instant::now();
        let say_config = options.applied_to(&config);
        let mut chunk_index = 0u32;
        let mut emit = |audio: SpeechAudio| {
            if mine == generation.load(Ordering::Relaxed) {
                send(TtsEvent::AudioChunk { utterance, index: chunk_index, audio });
            }
            chunk_index += 1;
        };
        match engine.synthesize(text, &say_config, &mut emit) {
            Ok((audio, timings)) if !audio.is_empty() => {
                // Synthesis is slow enough that a cancel can land while it
                // runs; a stale utterance must never be heard.
                if mine == generation.load(Ordering::Relaxed) {
                    send(TtsEvent::Audio { utterance, audio, timings, secs: started.elapsed().as_secs_f64() });
                }
            }
            Ok(_) => send(TtsEvent::Error { utterance, message: "engine produced no audio".into() }),
            Err(message) => send(TtsEvent::Error { utterance, message }),
        }
    }
}

/// What an engine renders for one text: the PCM, and the words' times when
/// it knows them.
type Spoken = (SpeechAudio, Option<Vec<WordTiming>>);

trait Engine {
    fn info(&self) -> TtsEngineInfo;
    /// `on_chunk` gets each clause as it is ready when `config.stream` is on and the
    /// engine can; the return value is always the whole take.
    fn synthesize(&mut self, text: &str, config: &TtsConfig, on_chunk: &mut dyn FnMut(SpeechAudio)) -> Result<Spoken, String>;
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
                Some(pipe) => return Ok(Box::new(RemoteTts::new(pipe))),
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

    fn synthesize(&mut self, text: &str, config: &TtsConfig, _on_chunk: &mut dyn FnMut(SpeechAudio)) -> Result<Spoken, String> {
        let options = sys::TtsOptions {
            voice: config.voice.clone(),
            language: config.language.clone(),
            rate: config.rate,
            pitch: config.pitch,
        };
        let audio = sys::tts::synthesize(text, &options).map_err(|e| e.to_string())?;
        Ok((audio, None))
    }
}

// ---------------------------------------------------------------- remote pipe

struct RemoteTts {
    pipe: RemotePipe,
    /// The node's own list of installed voices (older nodes: the English catalogue).
    voices: Vec<Voice>,
}

impl RemoteTts {
    fn new(pipe: RemotePipe) -> Self {
        let voices = pipe.voices();
        RemoteTts { pipe, voices }
    }
}

impl Engine for RemoteTts {
    fn info(&self) -> TtsEngineInfo {
        TtsEngineInfo {
            pipe: PipeId::new("tts.kokoro"),
            engine: format!("kokoro ({})", self.pipe.model),
            remote: Some(self.pipe.base_url.clone()),
            sample_rate: 24_000,
            voices: self.voices.clone(),
        }
    }

    fn synthesize(&mut self, text: &str, config: &TtsConfig, on_chunk: &mut dyn FnMut(SpeechAudio)) -> Result<Spoken, String> {
        if config.stream {
            return self.pipe.synthesize_streaming(text, &kokoro_voice_name(config), config.rate, on_chunk);
        }
        self.pipe.synthesize(text, &kokoro_voice_name(config), config.rate)
    }
}

// -------------------------------------------------------- in-process kokoro

#[cfg(feature = "tts")]
mod kokoro {
    use super::super::Voice;
    use super::*;
    use crate::machine::{self, Claim, ResidencyState};
    use super::super::kokoro_bank::KokoroBank;
    use std::path::Path;
    use std::time::Duration;

    const HOLDER_PATIENCE: Duration = Duration::from_secs(30);
    const POLL: Duration = Duration::from_millis(150);

    pub(super) struct KokoroLocal {
        bank: KokoroBank,
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

        fn synthesize(&mut self, text: &str, config: &TtsConfig, on_chunk: &mut dyn FnMut(SpeechAudio)) -> Result<Spoken, String> {
            let voice = kokoro_voice_name(config);
            let audio = if config.stream {
                self.bank.synthesize_streaming(&voice, &weights::kokoro_voice_path, text, config.rate, &mut |_, _| true, &mut |samples: &[f32]| {
                    on_chunk(SpeechAudio { samples: samples.to_vec(), sample_rate: makepad_ai_speech::kokoro::SAMPLE_RATE });
                })?
            } else {
                self.bank.synthesize(&voice, &weights::kokoro_voice_path, text, config.rate, &mut |_, _| true)?
            };
            let timings = audio.timings.map(|words| {
                words
                    .into_iter()
                    .map(|w| WordTiming { word: w.word, start: w.start, end: w.end })
                    .collect()
            });
            Ok((SpeechAudio { samples: audio.samples, sample_rate: audio.sample_rate }, timings))
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
                            return Ok(Box::new(RemoteTts::new(pipe)));
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
        send(TtsEvent::Loading { phase: format!("loading {}", weights::KOKORO_MODEL_FILE), fraction: 0.0 });
        let started = Instant::now();
        let mut bank = KokoroBank::new(path.to_path_buf(), weights::kokoro_zh_model_path());
        // Warm-up, discarded: loads the model the default voice needs (and the
        // first synthesis initializes the Metal context on this thread: better
        // now than on the first sentence).
        if let Err(reason) = bank.synthesize(&voice, &weights::kokoro_voice_path, "Hi.", 1.0, &mut |_, _| true) {
            let reason = format!("could not load {}: {reason}", path.display());
            if let Some(guard) = guard.as_mut() {
                let _ = guard.publish(ResidencyState::Failed { reason: reason.clone(), retry_after_ms: unix_ms() + 30_000 });
            }
            return Err(reason);
        }
        if let Some(guard) = guard.as_mut() {
            let _ = guard.publish(ResidencyState::Ready { port: 0 });
        }
        send(TtsEvent::Loading {
            phase: format!("loaded kokoro in {:.1}s", started.elapsed().as_secs_f64()),
            fraction: 1.0,
        });
        Ok(Box::new(KokoroLocal { bank, voices: weights::kokoro_voices(), _residency: guard }))
    }

    fn unix_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
