//! The STT session worker: choose an engine once, then serve utterances.

use super::remote::RemotePipe;
use super::{SpeechReach, SttConfig, SttEngine, SttEngineInfo, SttEvent, SttMsg, Transcript};
use crate::pipe::PipeId;
use crate::registry::Domain;
use makepad_system_speech as sys;
use makepad_system_speech::{ListenHandle, SttCapabilities};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) fn run(
    config: SttConfig,
    msg_rx: Receiver<SttMsg>,
    event_tx: Sender<SttEvent>,
    generation: Arc<AtomicU64>,
) {
    let wake = config.wake.clone();
    let send = move |event: SttEvent| {
        let _ = event_tx.send(event);
        if let Some(wake) = &wake {
            wake();
        }
    };

    let mut engine = match choose(&config, &send) {
        Ok(engine) => engine,
        Err(why) => return send(SttEvent::Failed(why)),
    };
    send(SttEvent::Ready(engine.info()));

    let mut listening: Option<Listening> = None;
    loop {
        if let Some(live) = listening.as_mut() {
            let mut ended = false;
            while let Ok(event) = live.events.try_recv() {
                match event {
                    sys::SttEvent::Level(level) => send(SttEvent::Level(level)),
                    sys::SttEvent::Partial(text) => send(SttEvent::Partial(text)),
                    sys::SttEvent::Final(transcript) => {
                        send(SttEvent::Final { utterance: 0, transcript, secs: 0.0 })
                    }
                    sys::SttEvent::Error(error) => {
                        send(SttEvent::Error { utterance: None, message: error.to_string() })
                    }
                    sys::SttEvent::Ended => ended = true,
                }
            }
            if ended {
                listening = None;
                send(SttEvent::ListenEnded);
            }
        }

        match msg_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(SttMsg::Transcribe { utterance, generation: mine, samples }) => {
                if mine != generation.load(Ordering::Relaxed) {
                    continue;
                }
                let started = Instant::now();
                match engine.transcribe(&samples, &config) {
                    Ok(transcript) => {
                        // A cancel that landed mid-recognition means this
                        // result belongs to a turn nobody wants any more.
                        if mine == generation.load(Ordering::Relaxed) {
                            send(SttEvent::Final {
                                utterance,
                                transcript,
                                secs: started.elapsed().as_secs_f64(),
                            });
                        }
                    }
                    Err(message) => send(SttEvent::Error { utterance: Some(utterance), message }),
                }
            }
            Ok(SttMsg::Listen) => {
                if listening.is_some() {
                    continue;
                }
                match engine.listen(&config) {
                    Ok(live) => listening = Some(live),
                    Err(message) => send(SttEvent::Error { utterance: None, message }),
                }
            }
            Ok(SttMsg::StopListening) => {
                // Keep draining events: the engine still delivers its final
                // result and `Ended` after being told to stop.
                if let Some(live) = listening.as_mut() {
                    if let Some(handle) = live.handle.take() {
                        handle.stop();
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// An engine-owned microphone session in progress.
struct Listening {
    handle: Option<ListenHandle>,
    events: Receiver<sys::SttEvent>,
}

trait Engine {
    fn info(&self) -> SttEngineInfo;
    fn transcribe(&mut self, samples_16k: &[f32], config: &SttConfig) -> Result<Transcript, String>;
    fn listen(&mut self, config: &SttConfig) -> Result<Listening, String>;
}

// ----------------------------------------------------------------- choosing

fn choose(config: &SttConfig, send: &dyn Fn(SttEvent)) -> Result<Box<dyn Engine>, String> {
    let want_whisper = matches!(config.engine, SttEngine::Auto | SttEngine::Whisper);
    let want_system = matches!(config.engine, SttEngine::Auto | SttEngine::System);
    let mut reasons: Vec<String> = Vec::new();

    if want_whisper {
        #[cfg(feature = "stt")]
        {
            if !super::in_process_allowed("whisper") {
                reasons.push("in-process whisper is off on this platform (MAKEPAD=whisper enables it)".into());
            } else if let Some(path) = super::weights::whisper_model_path() {
                match whisper::elect_and_load(&path, config, send) {
                    Ok(engine) => return Ok(engine),
                    Err(why) => reasons.push(why),
                }
            } else {
                reasons.push(format!("no whisper weights ({}) on this machine", super::weights::WHISPER_MODEL_FILE));
            }
        }
        #[cfg(not(feature = "stt"))]
        reasons.push("whisper is not compiled into this build".into());

        if config.reach >= SpeechReach::Machine {
            send(SttEvent::Loading { phase: "looking for a whisper node".into(), fraction: 0.0 });
            match RemotePipe::find(config.reach, Domain::Stt, &["whisper"]) {
                Some(pipe) => return Ok(Box::new(RemoteStt { pipe })),
                None => reasons.push(format!("no node in reach ({:?}) serves stt.whisper", config.reach)),
            }
        }
    }

    if want_system {
        if sys::stt::available() {
            let _ = sys::stt::prepare(&config.language);
            return Ok(Box::new(SystemStt));
        }
        reasons.push(format!("no system recognizer here ({})", sys::stt::engine_name()));
    }

    Err(reasons.join("; "))
}

fn stt_options(config: &SttConfig) -> sys::SttOptions {
    sys::SttOptions {
        language: config.language.clone(),
        partial_results: config.partial_results,
        prefer_offline: config.prefer_offline,
        timestamps: config.timestamps,
    }
}

// ------------------------------------------------------------- the OS engine

struct SystemStt;

impl Engine for SystemStt {
    fn info(&self) -> SttEngineInfo {
        SttEngineInfo {
            pipe: PipeId::new("stt.system"),
            engine: sys::stt::engine_name().to_string(),
            remote: None,
            capabilities: sys::stt::capabilities(),
        }
    }

    fn transcribe(&mut self, samples_16k: &[f32], config: &SttConfig) -> Result<Transcript, String> {
        sys::stt::transcribe(samples_16k, &stt_options(config)).map_err(|e| e.to_string())
    }

    fn listen(&mut self, config: &SttConfig) -> Result<Listening, String> {
        let (tx, events) = channel();
        let handle = sys::stt::listen(&stt_options(config), tx).map_err(|e| e.to_string())?;
        Ok(Listening { handle: Some(handle), events })
    }
}

// ---------------------------------------------------------------- remote pipe

struct RemoteStt {
    pipe: RemotePipe,
}

impl Engine for RemoteStt {
    fn info(&self) -> SttEngineInfo {
        SttEngineInfo {
            pipe: PipeId::new("stt.whisper"),
            engine: format!("whisper ({})", self.pipe.model),
            remote: Some(self.pipe.base_url.clone()),
            capabilities: SttCapabilities { pcm_input: true, engine_mic: false, partial_results: false, offline: true },
        }
    }

    fn transcribe(&mut self, samples_16k: &[f32], config: &SttConfig) -> Result<Transcript, String> {
        self.pipe.transcribe(samples_16k, &config.language, config.timestamps)
    }

    fn listen(&mut self, _config: &SttConfig) -> Result<Listening, String> {
        Err("a remote whisper takes PCM; record and call transcribe".to_string())
    }
}

// ------------------------------------------------------- in-process whisper

#[cfg(feature = "stt")]
mod whisper {
    use super::super::weights;
    use super::*;
    use crate::machine::{self, Claim, ResidencyState};
    use makepad_ai_speech::whisper::{WhisperModel, WhisperParams, WhisperState};
    use std::path::Path;

    /// How long to wait on another process's `Loading` before loading our
    /// own copy. Whisper turbo streams 1.6 GB; a minute covers a cold disk.
    const HOLDER_PATIENCE: Duration = Duration::from_secs(60);
    const POLL: Duration = Duration::from_millis(150);

    pub(super) struct WhisperLocal {
        model: WhisperModel,
        state: WhisperState,
        name: String,
        /// The won machine election, held for the life of the engine.
        _residency: Option<machine::ResidencyGuard>,
    }

    impl Engine for WhisperLocal {
        fn info(&self) -> SttEngineInfo {
            SttEngineInfo {
                pipe: PipeId::new("stt.whisper"),
                engine: format!("whisper ({}, {})", self.name, makepad_ai_speech::whisper::accel_backend_name()),
                remote: None,
                capabilities: SttCapabilities { pcm_input: true, engine_mic: false, partial_results: false, offline: true },
            }
        }

        fn transcribe(&mut self, samples_16k: &[f32], config: &SttConfig) -> Result<Transcript, String> {
            let params = whisper_params(config);
            let segments = self.state.transcribe(&self.model, samples_16k, &params);
            Ok(Transcript {
                segments: segments
                    .into_iter()
                    .map(|s| super::super::Segment { start_ms: s.start_ms, end_ms: s.end_ms, text: s.text })
                    .collect(),
            })
        }

        fn listen(&mut self, _config: &SttConfig) -> Result<Listening, String> {
            Err("whisper takes PCM; record and call transcribe".to_string())
        }
    }

    fn whisper_params(config: &SttConfig) -> WhisperParams {
        let mut params = WhisperParams::default();
        // Whisper wants the bare language code; a BCP-47 tag loses its region.
        params.language = config
            .language
            .split(['-', '_'])
            .next()
            .unwrap_or("en")
            .to_ascii_lowercase();
        params.no_timestamps = !config.timestamps;
        params.single_segment = config.single_segment;
        if config.max_tokens > 0 {
            params.max_tokens = config.max_tokens;
        }
        if let Some(threshold) = config.silence_threshold {
            params.no_speech_thold = threshold;
        }
        params
    }

    /// The machine election around the load (aicore §3): route to a serving
    /// holder, wait on a loading one, else claim and load here.
    pub(super) fn elect_and_load(
        path: &Path,
        config: &SttConfig,
        send: &dyn Fn(SttEvent),
    ) -> Result<Box<dyn Engine>, String> {
        let key = weights::election_key(path);
        let deadline = Instant::now() + HOLDER_PATIENCE;
        loop {
            match machine::read_holder(&key) {
                Ok(Some(record)) => match record.state {
                    ResidencyState::Ready { port } if port > 0 && config.reach >= SpeechReach::Machine => {
                        let url = format!("http://127.0.0.1:{port}");
                        if let Some((_, pipe)) = RemotePipe::at(&url, Domain::Stt, &["whisper"]) {
                            return Ok(Box::new(RemoteStt { pipe }));
                        }
                        // The holder serves something, but not this pipe.
                        break;
                    }
                    ResidencyState::Ready { .. } => {
                        eprintln!("[hub-stt] {key}: held by pid {} without a usable route — loading a duplicate copy", record.pid);
                        break;
                    }
                    ResidencyState::Loading { fraction } => {
                        send(SttEvent::Loading { phase: format!("waiting on pid {}", record.pid), fraction });
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

        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        send(SttEvent::Loading { phase: format!("loading {name}"), fraction: 0.0 });
        let started = Instant::now();
        let model = match WhisperModel::load_file(&path.to_string_lossy()) {
            Ok(model) => model,
            Err(error) => {
                let reason = format!("could not load {}: {error:?}", path.display());
                if let Some(guard) = guard.as_mut() {
                    let retry_after_ms = unix_ms() + 30_000;
                    let _ = guard.publish(ResidencyState::Failed { reason: reason.clone(), retry_after_ms });
                }
                return Err(reason);
            }
        };
        let state = WhisperState::new(&model);
        if let Some(guard) = guard.as_mut() {
            // Resident but not serving a port: co-located claimants see the
            // election held and fall back per the documented soft failure.
            let _ = guard.publish(ResidencyState::Ready { port: 0 });
        }
        send(SttEvent::Loading {
            phase: format!("loaded {name} in {:.1}s", started.elapsed().as_secs_f64()),
            fraction: 1.0,
        });
        Ok(Box::new(WhisperLocal { model, state, name, _residency: guard }))
    }

    fn unix_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
