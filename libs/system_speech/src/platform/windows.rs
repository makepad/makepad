//! Windows: `Windows.Media.SpeechSynthesis` for TTS and
//! `Windows.Media.SpeechRecognition` for STT.
//!
//! The synthesizer renders into a WinRT WAV stream that [`crate::wav`] turns
//! into PCM. The recognizer has no PCM-input API at all — it owns the
//! microphone itself — so [`stt_transcribe`] is unsupported here and
//! [`stt_listen`] carries the whole STT story.
//!
//! Everything runs on ordinary worker threads. No `RoInitialize` call is
//! needed: `windows-core`'s factory cache falls back to `CoIncrementMTAUsage`
//! when a class is activated on a thread that has not initialised COM
//! (`libs/windows/windows-core/src/imp/factory_cache.rs`), so activation is
//! apartment-agnostic.

use crate::{
    bcp47, ListenHandle, SpeechAudio, SpeechError, SttCapabilities, SttEvent, SttOptions,
    Transcript, TtsOptions, Voice, VoiceGender,
};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Globalization::Language;
use windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs,
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionConfidence, SpeechRecognitionHypothesisGeneratedEventArgs,
    SpeechRecognitionResultStatus, SpeechRecognizer,
};
use windows::Media::SpeechSynthesis::{
    SpeechSynthesisStream, SpeechSynthesizer, VoiceGender as WinVoiceGender, VoiceInformation,
};
use windows::Storage::Streams::DataReader;
use windows_core::{RuntimeType, HSTRING};
use windows_future::{AsyncStatus, IAsyncAction, IAsyncOperation};

pub(crate) const STT_ENGINE: &str = "windows-speechrecognition";
pub(crate) const TTS_ENGINE: &str = "windows-speechsynthesis";

/// One WinRT tick is 100 ns.
const TICKS_PER_SEC: i64 = 10_000_000;

/// `SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED`. The machine has "online speech
/// recognition" turned off in Settings → Privacy, so the recognizer refuses to
/// start. That is a permission problem, not a broken engine.
const SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED: i32 = 0x8004_5509_u32 as i32;
/// `HRESULT_FROM_WIN32(ERROR_TIMEOUT)`, for a wait that outlived its budget.
const E_TIMEOUT: windows_core::HRESULT = windows_core::HRESULT(0x8007_05B4_u32 as i32);

const SYNTHESIZE_TIMEOUT: Duration = Duration::from_secs(60);
const STREAM_TIMEOUT: Duration = Duration::from_secs(30);
const COMPILE_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long we still wait for `Completed` after asking the session to stop.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

fn backend(err: windows_core::Error) -> SpeechError {
    SpeechError::Backend(format!("{err}"))
}

// ------------------------------------------------------------------ waiting

/// Poll a WinRT async object to completion. `windows-future` only exposes its
/// blocking join behind a spin loop, and its `IntoFuture` needs an executor;
/// this crate's contract is "block the worker thread", so it sleeps instead.
fn wait_ready(
    status: impl Fn() -> windows_core::Result<AsyncStatus>,
    timeout: Duration,
) -> windows_core::Result<()> {
    let deadline = Instant::now() + timeout;
    while status()? == AsyncStatus::Started {
        if Instant::now() >= deadline {
            return Err(windows_core::Error::from_hresult(E_TIMEOUT));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

/// `GetResults` after the wait reports the engine's own failure HRESULT, so an
/// errored or cancelled operation comes back as `Err` without a second look.
fn wait_operation<T: RuntimeType>(
    operation: &IAsyncOperation<T>,
    timeout: Duration,
) -> windows_core::Result<T> {
    wait_ready(|| operation.Status(), timeout)?;
    operation.GetResults()
}

fn wait_action(action: &IAsyncAction, timeout: Duration) -> windows_core::Result<()> {
    wait_ready(|| action.Status(), timeout)?;
    action.GetResults()
}

// --------------------------------------------------------------------- TTS

pub(crate) fn tts_available() -> bool {
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| SpeechSynthesizer::new().is_ok())
}

pub(crate) fn tts_voices() -> Vec<Voice> {
    installed_voices()
        .iter()
        .filter_map(|voice| {
            Some(Voice {
                id: voice.Id().ok()?.to_string_lossy(),
                name: voice.DisplayName().ok()?.to_string_lossy(),
                language: voice.Language().ok()?.to_string_lossy(),
                gender: match voice.Gender() {
                    Ok(WinVoiceGender::Male) => VoiceGender::Male,
                    Ok(WinVoiceGender::Female) => VoiceGender::Female,
                    _ => VoiceGender::Unknown,
                },
                // Every installed SAPI voice renders locally.
                offline: true,
            })
        })
        .collect()
}

fn installed_voices() -> Vec<VoiceInformation> {
    match SpeechSynthesizer::AllVoices() {
        Ok(voices) => voices.into_iter().collect(),
        Err(_) => Vec::new(),
    }
}

pub(crate) fn tts_synthesize(text: &str, options: &TtsOptions) -> Result<SpeechAudio, SpeechError> {
    let synth = SpeechSynthesizer::new().map_err(backend)?;

    if let Some(voice) = pick_voice(&installed_voices(), options) {
        synth.SetVoice(&voice).map_err(backend)?;
    }

    // `SpeechSynthesizerOptions`' rate and pitch arrived in Windows 10 1703; on
    // anything older the QI fails and the utterance plays at normal speed.
    if let Ok(synth_options) = synth.Options() {
        let _ = synth_options.SetSpeakingRate(options.rate.clamp(0.5, 6.0) as f64);
        let _ = synth_options.SetAudioPitch(options.pitch.clamp(0.5, 2.0) as f64);
    }

    let operation = synth
        .SynthesizeTextToStreamAsync(&HSTRING::from(text))
        .map_err(backend)?;
    let stream = wait_operation(&operation, SYNTHESIZE_TIMEOUT).map_err(backend)?;

    let bytes = read_stream(&stream).map_err(backend)?;
    let audio = crate::wav::decode(&bytes).map_err(SpeechError::Backend)?;
    if audio.is_empty() {
        return Err(SpeechError::Empty);
    }
    Ok(audio)
}

/// The requested voice by id, else the first voice whose language matches —
/// exactly first, then on the language prefix, so `"en"` finds `en-GB`.
fn pick_voice(voices: &[VoiceInformation], options: &TtsOptions) -> Option<VoiceInformation> {
    if let Some(wanted) = options.voice.as_deref().filter(|id| !id.is_empty()) {
        return voices
            .iter()
            .find(|voice| voice.Id().map(|id| id.to_string_lossy() == wanted).unwrap_or(false))
            .cloned();
    }
    let wanted = bcp47(&options.language).to_ascii_lowercase();
    let prefix = wanted.split('-').next().unwrap_or(&wanted).to_string();
    let language_of = |voice: &VoiceInformation| {
        voice.Language().map(|l| l.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
    };
    voices
        .iter()
        .find(|voice| language_of(voice) == wanted)
        .or_else(|| {
            voices
                .iter()
                .find(|voice| language_of(voice).split('-').next() == Some(prefix.as_str()))
        })
        .cloned()
}

/// Drain a `SpeechSynthesisStream` into the RIFF/WAVE bytes it holds.
fn read_stream(stream: &SpeechSynthesisStream) -> windows_core::Result<Vec<u8>> {
    let size = stream.Size()?;
    if size == 0 {
        return Ok(Vec::new());
    }
    let reader = DataReader::CreateDataReader(stream)?;
    let load = reader.LoadAsync(size.min(u32::MAX as u64) as u32)?;
    // `LoadAsync` reports how much it actually buffered, which is what
    // `ReadBytes` will hand over.
    let loaded = wait_operation(&load, STREAM_TIMEOUT)?;
    let mut bytes = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

// --------------------------------------------------------------------- STT

pub(crate) fn stt_capabilities() -> SttCapabilities {
    SttCapabilities {
        pcm_input: false,
        engine_mic: true,
        partial_results: true,
        offline: false,
    }
}

pub(crate) fn stt_available() -> bool {
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| SpeechRecognizer::new().is_ok())
}

pub(crate) fn stt_prepare(language: &str) -> Result<(), SpeechError> {
    let recognizer = recognizer_for(language)?;
    compile_constraints(&recognizer)
}

pub(crate) fn stt_transcribe(
    _samples_16k: &[f32],
    _options: &SttOptions,
) -> Result<Transcript, SpeechError> {
    Err(SpeechError::Unsupported(
        "the Windows recognizer only listens on the microphone; use listen",
    ))
}

pub(crate) fn stt_listen(
    options: &SttOptions,
    sink: Sender<SttEvent>,
) -> Result<ListenHandle, SpeechError> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), SpeechError>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let language = options.language.clone();
    let partial_results = options.partial_results;

    std::thread::Builder::new()
        .name("system-speech-listen".to_string())
        .spawn(move || listen_worker(language, partial_results, sink, ready_tx, stop_rx))
        .map_err(|err| SpeechError::Backend(format!("cannot spawn listen thread: {err}")))?;

    // Block until the session is actually running so a missing microphone or a
    // refused privacy policy comes back as an error rather than as an event.
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => return Err(err),
        Err(_) => return Err(SpeechError::Backend("listen thread stopped early".to_string())),
    }

    // The closure runs on whichever thread drops the handle, and WinRT objects
    // made on the worker must only be touched there — so it just signals.
    Ok(ListenHandle::new(move || {
        let _ = stop_tx.send(());
    }))
}

fn recognizer_for(language: &str) -> Result<SpeechRecognizer, SpeechError> {
    // Settle "is there an engine at all?" first, so the mapping below can read
    // a failed `Create` as a missing language pack rather than a missing engine.
    if !stt_available() {
        return Err(SpeechError::Unavailable(
            "no Windows speech recognizer on this machine".to_string(),
        ));
    }
    let tag = HSTRING::from(bcp47(language));
    let language = Language::CreateLanguage(&tag).map_err(backend)?;
    SpeechRecognizer::Create(&language).map_err(|err| {
        if err.code().0 == SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED {
            SpeechError::PermissionDenied
        } else {
            // Construction only fails for a language with no recognizer pack
            // installed; anything else would already have failed the probe.
            SpeechError::Unsupported("language not supported by the Windows recognizer")
        }
    })
}

/// Compile the recognizer's grammar. With no constraints added that is the
/// built-in dictation grammar, which is what a free-form transcript wants.
fn compile_constraints(recognizer: &SpeechRecognizer) -> Result<(), SpeechError> {
    let operation = recognizer.CompileConstraintsAsync().map_err(compile_error)?;
    let result = wait_operation(&operation, COMPILE_TIMEOUT).map_err(compile_error)?;
    match result.Status().map_err(backend)? {
        SpeechRecognitionResultStatus::Success => Ok(()),
        SpeechRecognitionResultStatus::TopicLanguageNotSupported
        | SpeechRecognitionResultStatus::GrammarLanguageMismatch => Err(SpeechError::Unsupported(
            "language not supported by the Windows recognizer",
        )),
        SpeechRecognitionResultStatus::UserCanceled => Err(SpeechError::Cancelled),
        status => Err(SpeechError::Backend(format!(
            "constraint compilation failed with status {}",
            status.0
        ))),
    }
}

fn compile_error(err: windows_core::Error) -> SpeechError {
    if err.code().0 == SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED {
        SpeechError::PermissionDenied
    } else {
        backend(err)
    }
}

/// Give the engine room to hear a first word, but cut the utterance shortly
/// after the speaker stops. A rejected value leaves the platform default.
fn apply_timeouts(recognizer: &SpeechRecognizer) {
    let Ok(timeouts) = recognizer.Timeouts() else {
        return;
    };
    let _ = timeouts.SetInitialSilenceTimeout(TimeSpan { Duration: 5 * TICKS_PER_SEC });
    let _ = timeouts.SetEndSilenceTimeout(TimeSpan { Duration: 12 * TICKS_PER_SEC / 10 });
    let _ = timeouts.SetBabbleTimeout(TimeSpan { Duration: 10 * TICKS_PER_SEC });
}

fn listen_worker(
    language: String,
    partial_results: bool,
    sink: Sender<SttEvent>,
    ready: Sender<Result<(), SpeechError>>,
    stop_rx: mpsc::Receiver<()>,
) {
    let recognizer = match recognizer_for(&language) {
        Ok(recognizer) => recognizer,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    if let Err(err) = compile_constraints(&recognizer) {
        let _ = ready.send(Err(err));
        return;
    }
    apply_timeouts(&recognizer);

    let session = match recognizer.ContinuousRecognitionSession() {
        Ok(session) => session,
        Err(err) => {
            let _ = ready.send(Err(backend(err)));
            return;
        }
    };

    let (done_tx, done_rx) = mpsc::channel::<SpeechRecognitionResultStatus>();

    let result_sink = sink.clone();
    let on_result = TypedEventHandler::<
        SpeechContinuousRecognitionSession,
        SpeechContinuousRecognitionResultGeneratedEventArgs,
    >::new(move |_session, args| {
        if let Some(args) = args.as_ref() {
            if let Ok(result) = args.Result() {
                // `Rejected` is the engine saying "that was noise".
                let confidence = result.Confidence().unwrap_or(SpeechRecognitionConfidence::Rejected);
                if confidence != SpeechRecognitionConfidence::Rejected {
                    if let Ok(text) = result.Text() {
                        let transcript = Transcript::from_text(text.to_string_lossy());
                        if !transcript.is_empty() {
                            let _ = result_sink.send(SttEvent::Final(transcript));
                        }
                    }
                }
            }
        }
        Ok(())
    });

    let on_completed = TypedEventHandler::<
        SpeechContinuousRecognitionSession,
        SpeechContinuousRecognitionCompletedEventArgs,
    >::new(move |_session, args| {
        let status = args
            .as_ref()
            .and_then(|args| args.Status().ok())
            .unwrap_or(SpeechRecognitionResultStatus::Unknown);
        let _ = done_tx.send(status);
        Ok(())
    });

    let result_token = match session.ResultGenerated(&on_result) {
        Ok(token) => token,
        Err(err) => {
            let _ = ready.send(Err(backend(err)));
            return;
        }
    };
    let completed_token = match session.Completed(&on_completed) {
        Ok(token) => token,
        Err(err) => {
            let _ = session.RemoveResultGenerated(result_token);
            let _ = ready.send(Err(backend(err)));
            return;
        }
    };

    let mut hypothesis_token = 0i64;
    if partial_results {
        let partial_sink = sink.clone();
        let on_hypothesis = TypedEventHandler::<
            SpeechRecognizer,
            SpeechRecognitionHypothesisGeneratedEventArgs,
        >::new(move |_recognizer, args| {
            if let Some(args) = args.as_ref() {
                if let Ok(hypothesis) = args.Hypothesis() {
                    if let Ok(text) = hypothesis.Text() {
                        let _ = partial_sink.send(SttEvent::Partial(text.to_string_lossy()));
                    }
                }
            }
            Ok(())
        });
        hypothesis_token = recognizer.HypothesisGenerated(&on_hypothesis).unwrap_or(0);
    }

    let start = session
        .StartAsync()
        .and_then(|action| wait_action(&action, START_TIMEOUT));
    if let Err(err) = start {
        remove_handlers(&recognizer, &session, result_token, completed_token, hypothesis_token);
        let _ = ready.send(Err(compile_error(err)));
        return;
    }
    let _ = ready.send(Ok(()));

    // From here the session exists, so exactly one `Ended` must reach the sink.
    let mut stopped_by_caller = false;
    let mut drain_deadline: Option<Instant> = None;
    let status = loop {
        match done_rx.try_recv() {
            Ok(status) => break Some(status),
            Err(TryRecvError::Disconnected) => break None,
            Err(TryRecvError::Empty) => {}
        }
        if drain_deadline.is_none() && !matches!(stop_rx.try_recv(), Err(TryRecvError::Empty)) {
            stopped_by_caller = true;
            // `StopAsync` lets the engine emit whatever it already has; the
            // `Completed` event still fires, so keep waiting for it.
            stop_session(&session);
            drain_deadline = Some(Instant::now() + DRAIN_TIMEOUT);
        }
        if drain_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    remove_handlers(&recognizer, &session, result_token, completed_token, hypothesis_token);

    if let Some(err) = status.and_then(|status| status_error(status, stopped_by_caller)) {
        let _ = sink.send(SttEvent::Error(err));
    }
    let _ = sink.send(SttEvent::Ended);
}

fn stop_session(session: &SpeechContinuousRecognitionSession) {
    let stopped = session
        .StopAsync()
        .and_then(|action| wait_action(&action, STOP_TIMEOUT));
    if stopped.is_err() {
        let _ = session
            .CancelAsync()
            .and_then(|action| wait_action(&action, STOP_TIMEOUT));
    }
}

fn remove_handlers(
    recognizer: &SpeechRecognizer,
    session: &SpeechContinuousRecognitionSession,
    result_token: i64,
    completed_token: i64,
    hypothesis_token: i64,
) {
    let _ = session.RemoveResultGenerated(result_token);
    let _ = session.RemoveCompleted(completed_token);
    if hypothesis_token != 0 {
        let _ = recognizer.RemoveHypothesisGenerated(hypothesis_token);
    }
}

/// A finished session's status, as an event — or `None` when the ending was
/// the ordinary one (success, our own stop, or the silence timeout firing).
fn status_error(
    status: SpeechRecognitionResultStatus,
    stopped_by_caller: bool,
) -> Option<SpeechError> {
    match status {
        SpeechRecognitionResultStatus::Success
        | SpeechRecognitionResultStatus::TimeoutExceeded => None,
        SpeechRecognitionResultStatus::UserCanceled if stopped_by_caller => None,
        SpeechRecognitionResultStatus::UserCanceled => Some(SpeechError::Cancelled),
        SpeechRecognitionResultStatus::MicrophoneUnavailable => {
            Some(SpeechError::Unavailable("microphone unavailable".to_string()))
        }
        SpeechRecognitionResultStatus::NetworkFailure => Some(SpeechError::Backend(
            "the Windows recognizer lost its network connection".to_string(),
        )),
        SpeechRecognitionResultStatus::TopicLanguageNotSupported
        | SpeechRecognitionResultStatus::GrammarLanguageMismatch => Some(SpeechError::Unsupported(
            "language not supported by the Windows recognizer",
        )),
        status => Some(SpeechError::Backend(format!(
            "recognition ended with status {}",
            status.0
        ))),
    }
}
