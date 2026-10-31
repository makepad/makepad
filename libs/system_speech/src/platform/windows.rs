//! Windows: `Windows.Media.SpeechSynthesis` for TTS and
//! `Windows.Media.SpeechRecognition` for STT.
//!
//! The synthesizer renders into a WinRT WAV stream that [`crate::wav`] turns
//! into PCM. The recognizer has no PCM-input API at all — it owns the
//! microphone itself — so [`stt_transcribe`] is unsupported here and
//! [`stt_listen`] carries the whole STT story.
//!
//! Calls and waits run on speech workers. Concrete WinRT completion delegates
//! wake those workers, and event delegates publish transcripts to the sink.

use crate::{
    bcp47, ListenHandle, SpeechAudio, SpeechError, SttCapabilities, SttEvent, SttOptions,
    Transcript, TtsOptions, Voice, VoiceGender,
};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::{
    core::{HRESULT, HSTRING},
    WinRT::*,
};

pub(crate) const STT_ENGINE: &str = "windows-speechrecognition";
pub(crate) const TTS_ENGINE: &str = "windows-speechsynthesis";

/// One WinRT tick is 100 ns.
const TICKS_PER_SEC: i64 = 10_000_000;

/// `SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED`. The machine has "online speech
/// recognition" turned off in Settings → Privacy, so the recognizer refuses to
/// start. That is a permission problem, not a broken engine.
const SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED: i32 = 0x8004_5509_u32 as i32;
/// `HRESULT_FROM_WIN32(ERROR_TIMEOUT)`, for a wait that outlived its budget.
const E_TIMEOUT: HRESULT = HRESULT(0x8007_05B4_u32 as i32);

const SYNTHESIZE_TIMEOUT: Duration = Duration::from_secs(60);
const STREAM_TIMEOUT: Duration = Duration::from_secs(30);
const COMPILE_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long we still wait for `Completed` after asking the session to stop.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

fn backend(err: HRESULT) -> SpeechError {
    SpeechError::Backend(format!("{err}"))
}

// ------------------------------------------------------------------ waiting

struct Completed(Sender<()>);
impl ActionCompletedImpl for Completed {
    fn Invoke(&self, _: Option<&AsyncAction>, _: AsyncStatus) -> Result<(), HRESULT> {
        let _ = self.0.send(());
        Ok(())
    }
}
impl SynthesizeCompletedImpl for Completed {
    fn Invoke(&self, _: Option<&SynthesizeOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        let _ = self.0.send(());
        Ok(())
    }
}
impl CompileCompletedImpl for Completed {
    fn Invoke(&self, _: Option<&CompileOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        let _ = self.0.send(());
        Ok(())
    }
}
impl UInt32CompletedImpl for Completed {
    fn Invoke(&self, _: Option<&UInt32Operation>, _: AsyncStatus) -> Result<(), HRESULT> {
        let _ = self.0.send(());
        Ok(())
    }
}
unsafe fn wait_ready(
    receiver: &mpsc::Receiver<()>,
    raw: *mut std::ffi::c_void,
    timeout: Duration,
) -> Result<(), HRESULT> {
    if receiver.recv_timeout(timeout).is_err() {
        if let Ok(info) = AsyncInfo::query(raw) {
            let _ = info.Cancel();
        }
        return Err(E_TIMEOUT);
    }
    Ok(())
}
unsafe fn wait_synthesize(
    operation: &SynthesizeOperation,
) -> Result<SpeechSynthesisStream, HRESULT> {
    let (tx, rx) = mpsc::channel();
    operation.put_Completed(&SynthesizeCompleted::implement(Box::new(Completed(tx))))?;
    wait_ready(&rx, operation.as_raw(), SYNTHESIZE_TIMEOUT)?;
    operation.GetResults()
}
unsafe fn wait_compile(operation: &CompileOperation) -> Result<SpeechCompilationResult, HRESULT> {
    let (tx, rx) = mpsc::channel();
    operation.put_Completed(&CompileCompleted::implement(Box::new(Completed(tx))))?;
    wait_ready(&rx, operation.as_raw(), COMPILE_TIMEOUT)?;
    operation.GetResults()
}
unsafe fn wait_load(operation: &UInt32Operation) -> Result<u32, HRESULT> {
    let (tx, rx) = mpsc::channel();
    operation.put_Completed(&UInt32Completed::implement(Box::new(Completed(tx))))?;
    wait_ready(&rx, operation.as_raw(), STREAM_TIMEOUT)?;
    operation.GetResults()
}
unsafe fn wait_action(action: &AsyncAction, timeout: Duration) -> Result<(), HRESULT> {
    let (tx, rx) = mpsc::channel();
    action.put_Completed(&ActionCompleted::implement(Box::new(Completed(tx))))?;
    wait_ready(&rx, action.as_raw(), timeout)?;
    action.GetResults()
}

// --------------------------------------------------------------------- TTS

pub(crate) fn tts_available() -> bool {
    unsafe {
        static PROBE: OnceLock<bool> = OnceLock::new();
        *PROBE.get_or_init(|| {
            SpeechSynthesizer::activate("Windows.Media.SpeechSynthesis.SpeechSynthesizer").is_ok()
        })
    }
}

pub(crate) fn tts_voices() -> Vec<Voice> {
    unsafe {
        installed_voices()
            .iter()
            .filter_map(|voice| {
                Some(Voice {
                    id: voice.get_Id().ok()?.to_string(),
                    name: voice.get_DisplayName().ok()?.to_string(),
                    language: voice.get_Language().ok()?.to_string(),
                    gender: match voice.get_Gender() {
                        Ok(VOICE_GENDER_MALE) => VoiceGender::Male,
                        Ok(VOICE_GENDER_FEMALE) => VoiceGender::Female,
                        _ => VoiceGender::Unknown,
                    },
                    // Every installed SAPI voice renders locally.
                    offline: true,
                })
            })
            .collect()
    }
}

unsafe fn installed_voices() -> Vec<VoiceInformation> {
    match InstalledVoices::factory("Windows.Media.SpeechSynthesis.SpeechSynthesizer")
        .and_then(|factory| factory.get_AllVoices())
    {
        Ok(voices) => {
            let mut result = Vec::new();
            for index in 0..voices.get_Size().unwrap_or(0) {
                if let Ok(voice) = voices.GetAt(index) {
                    result.push(voice);
                }
            }
            result
        }
        Err(_) => Vec::new(),
    }
}

pub(crate) fn tts_synthesize(text: &str, options: &TtsOptions) -> Result<SpeechAudio, SpeechError> {
    unsafe {
        let synth = SpeechSynthesizer::activate("Windows.Media.SpeechSynthesis.SpeechSynthesizer")
            .map_err(backend)?;

        if let Some(voice) = pick_voice(&installed_voices(), options) {
            synth.put_Voice(&voice).map_err(backend)?;
        }

        // `SpeechSynthesizerOptions`' rate and pitch arrived in Windows 10 1703; on
        // anything older the QI fails and the utterance plays at normal speed.
        if let Ok(synth_options) = SpeechSynthesizer2::query(synth.as_raw())
            .and_then(|s| s.get_Options())
            .and_then(|o| SpeechSynthesizerOptions2::query(o.as_raw()))
        {
            let _ = synth_options.put_SpeakingRate(options.rate.clamp(0.5, 6.0) as f64);
            let _ = synth_options.put_AudioPitch(options.pitch.clamp(0.5, 2.0) as f64);
        }

        let operation = synth
            .SynthesizeTextToStreamAsync(&HSTRING::from_str(text).map_err(backend)?)
            .map_err(backend)?;
        let stream = wait_synthesize(&operation).map_err(backend)?;

        let bytes = read_stream(&stream).map_err(backend)?;
        let audio = crate::wav::decode(&bytes).map_err(SpeechError::Backend)?;
        if audio.is_empty() {
            return Err(SpeechError::Empty);
        }
        Ok(audio)
    }
}

/// The requested voice by id, else the first voice whose language matches —
/// exactly first, then on the language prefix, so `"en"` finds `en-GB`.
unsafe fn pick_voice(
    voices: &[VoiceInformation],
    options: &TtsOptions,
) -> Option<VoiceInformation> {
    if let Some(wanted) = options.voice.as_deref().filter(|id| !id.is_empty()) {
        return voices
            .iter()
            .find(|voice| {
                voice
                    .get_Id()
                    .map(|id| id.to_string() == wanted)
                    .unwrap_or(false)
            })
            .cloned();
    }
    let wanted = bcp47(&options.language).to_ascii_lowercase();
    let prefix = wanted.split('-').next().unwrap_or(&wanted).to_string();
    let language_of = |voice: &VoiceInformation| {
        voice
            .get_Language()
            .map(|l| l.to_string().to_ascii_lowercase())
            .unwrap_or_default()
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
unsafe fn read_stream(stream: &SpeechSynthesisStream) -> Result<Vec<u8>, HRESULT> {
    let size = RandomAccessStream::query(stream.as_raw())?.get_Size()?;
    if size == 0 {
        return Ok(Vec::new());
    }
    let input = InputStream::query(stream.as_raw())?;
    let reader = DataReaderFactory::factory("Windows.Storage.Streams.DataReader")?
        .CreateDataReader(&input)?;
    let load = reader.LoadAsync(size.min(u32::MAX as u64) as u32)?;
    // `LoadAsync` reports how much it actually buffered, which is what
    // `ReadBytes` will hand over.
    let loaded = wait_load(&load)?;
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
    unsafe {
        static PROBE: OnceLock<bool> = OnceLock::new();
        *PROBE.get_or_init(|| {
            SpeechRecognizer::activate("Windows.Media.SpeechRecognition.SpeechRecognizer").is_ok()
        })
    }
}

pub(crate) fn stt_prepare(language: &str) -> Result<(), SpeechError> {
    unsafe {
        let recognizer = recognizer_for(language)?;
        compile_constraints(&recognizer)
    }
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
        .spawn(move || unsafe { listen_worker(language, partial_results, sink, ready_tx, stop_rx) })
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

unsafe fn recognizer_for(language: &str) -> Result<SpeechRecognizer, SpeechError> {
    // Settle "is there an engine at all?" first, so the mapping below can read
    // a failed `Create` as a missing language pack rather than a missing engine.
    if !stt_available() {
        return Err(SpeechError::Unavailable(
            "no Windows speech recognizer on this machine".to_string(),
        ));
    }
    let tag = HSTRING::from_str(&bcp47(language)).map_err(backend)?;
    let language = LanguageFactory::factory("Windows.Globalization.Language")
        .and_then(|factory| factory.CreateLanguage(&tag))
        .map_err(backend)?;
    SpeechRecognizerFactory::factory("Windows.Media.SpeechRecognition.SpeechRecognizer")
        .and_then(|factory| factory.Create(&language))
        .map_err(|err| {
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
unsafe fn compile_constraints(recognizer: &SpeechRecognizer) -> Result<(), SpeechError> {
    let operation = recognizer
        .CompileConstraintsAsync()
        .map_err(compile_error)?;
    let result = wait_compile(&operation).map_err(compile_error)?;
    match result.get_Status().map_err(backend)? {
        SPEECH_RECOGNITION_RESULT_STATUS_SUCCESS => Ok(()),
        SPEECH_RECOGNITION_RESULT_STATUS_TOPIC_LANGUAGE_NOT_SUPPORTED
        | SPEECH_RECOGNITION_RESULT_STATUS_GRAMMAR_LANGUAGE_MISMATCH => Err(
            SpeechError::Unsupported("language not supported by the Windows recognizer"),
        ),
        SPEECH_RECOGNITION_RESULT_STATUS_USER_CANCELED => Err(SpeechError::Cancelled),
        status => Err(SpeechError::Backend(format!(
            "constraint compilation failed with status {}",
            status
        ))),
    }
}

fn compile_error(err: HRESULT) -> SpeechError {
    if err.code().0 == SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED {
        SpeechError::PermissionDenied
    } else {
        backend(err)
    }
}

/// Give the engine room to hear a first word, but cut the utterance shortly
/// after the speaker stops. A rejected value leaves the platform default.
unsafe fn apply_timeouts(recognizer: &SpeechRecognizer) {
    let Ok(timeouts) = recognizer.get_Timeouts() else {
        return;
    };
    let _ = timeouts.put_InitialSilenceTimeout(TimeSpan {
        Duration: 5 * TICKS_PER_SEC,
    });
    let _ = timeouts.put_EndSilenceTimeout(TimeSpan {
        Duration: 12 * TICKS_PER_SEC / 10,
    });
    let _ = timeouts.put_BabbleTimeout(TimeSpan {
        Duration: 10 * TICKS_PER_SEC,
    });
}

unsafe fn listen_worker(
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

    let session = match SpeechRecognizer2::query(recognizer.as_raw())
        .and_then(|r| r.get_ContinuousRecognitionSession())
    {
        Ok(session) => session,
        Err(err) => {
            let _ = ready.send(Err(backend(err)));
            return;
        }
    };

    let (done_tx, done_rx) = mpsc::channel::<SpeechRecognitionResultStatus>();

    let on_result = SpeechResultHandler::implement(Box::new(ResultHandler(sink.clone())));
    let on_completed = SpeechCompletedHandler::implement(Box::new(EndedHandler(done_tx)));
    let recognizer2 = match SpeechRecognizer2::query(recognizer.as_raw()) {
        Ok(value) => value,
        Err(err) => {
            let _ = ready.send(Err(backend(err)));
            return;
        }
    };

    let result_token = match session.add_ResultGenerated(&on_result) {
        Ok(token) => token,
        Err(err) => {
            let _ = ready.send(Err(backend(err)));
            return;
        }
    };
    let completed_token = match session.add_Completed(&on_completed) {
        Ok(token) => token,
        Err(err) => {
            let _ = session.remove_ResultGenerated(result_token);
            let _ = ready.send(Err(backend(err)));
            return;
        }
    };

    let mut hypothesis_token = 0i64;
    if partial_results {
        let on_hypothesis =
            SpeechHypothesisHandler::implement(Box::new(HypothesisHandler(sink.clone())));
        hypothesis_token = recognizer2
            .add_HypothesisGenerated(&on_hypothesis)
            .map(|token| token.Value)
            .unwrap_or(0);
    }

    let start = session
        .StartAsync()
        .and_then(|action| wait_action(&action, START_TIMEOUT));
    if let Err(err) = start {
        remove_handlers(
            &recognizer,
            &session,
            result_token,
            completed_token,
            hypothesis_token,
        );
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

unsafe fn stop_session(session: &SpeechSession) {
    let stopped = session
        .StopAsync()
        .and_then(|action| wait_action(&action, STOP_TIMEOUT));
    if stopped.is_err() {
        let _ = session
            .CancelAsync()
            .and_then(|action| wait_action(&action, STOP_TIMEOUT));
    }
}

unsafe fn remove_handlers(
    recognizer: &SpeechRecognizer,
    session: &SpeechSession,
    result_token: EventRegistrationToken,
    completed_token: EventRegistrationToken,
    hypothesis_token: i64,
) {
    let _ = session.remove_ResultGenerated(result_token);
    let _ = session.remove_Completed(completed_token);
    if hypothesis_token != 0 {
        let _ = SpeechRecognizer2::query(recognizer.as_raw()).and_then(|r| {
            r.remove_HypothesisGenerated(EventRegistrationToken {
                Value: hypothesis_token,
            })
        });
    }
}

/// A finished session's status, as an event — or `None` when the ending was
/// the ordinary one (success, our own stop, or the silence timeout firing).
fn status_error(
    status: SpeechRecognitionResultStatus,
    stopped_by_caller: bool,
) -> Option<SpeechError> {
    match status {
        SPEECH_RECOGNITION_RESULT_STATUS_SUCCESS
        | SPEECH_RECOGNITION_RESULT_STATUS_TIMEOUT_EXCEEDED => None,
        SPEECH_RECOGNITION_RESULT_STATUS_USER_CANCELED if stopped_by_caller => None,
        SPEECH_RECOGNITION_RESULT_STATUS_USER_CANCELED => Some(SpeechError::Cancelled),
        SPEECH_RECOGNITION_RESULT_STATUS_MICROPHONE_UNAVAILABLE => Some(SpeechError::Unavailable(
            "microphone unavailable".to_string(),
        )),
        SPEECH_RECOGNITION_RESULT_STATUS_NETWORK_FAILURE => Some(SpeechError::Backend(
            "the Windows recognizer lost its network connection".to_string(),
        )),
        SPEECH_RECOGNITION_RESULT_STATUS_TOPIC_LANGUAGE_NOT_SUPPORTED
        | SPEECH_RECOGNITION_RESULT_STATUS_GRAMMAR_LANGUAGE_MISMATCH => Some(
            SpeechError::Unsupported("language not supported by the Windows recognizer"),
        ),
        status => Some(SpeechError::Backend(format!(
            "recognition ended with status {}",
            status
        ))),
    }
}

struct ResultHandler(Sender<SttEvent>);
impl SpeechResultHandlerImpl for ResultHandler {
    fn Invoke(
        &self,
        _: Option<&SpeechSession>,
        args: Option<&SpeechResultArgs>,
    ) -> Result<(), HRESULT> {
        unsafe {
            if let Some(args) = args {
                if let Ok(result) = args.get_Result() {
                    if result
                        .get_Confidence()
                        .unwrap_or(SPEECH_RECOGNITION_CONFIDENCE_REJECTED)
                        != SPEECH_RECOGNITION_CONFIDENCE_REJECTED
                    {
                        if let Ok(text) = result.get_Text() {
                            let transcript = Transcript::from_text(text.to_string());
                            if !transcript.is_empty() {
                                let _ = self.0.send(SttEvent::Final(transcript));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
struct EndedHandler(Sender<SpeechRecognitionResultStatus>);
impl SpeechCompletedHandlerImpl for EndedHandler {
    fn Invoke(
        &self,
        _: Option<&SpeechSession>,
        args: Option<&SpeechCompletedArgs>,
    ) -> Result<(), HRESULT> {
        let status = unsafe { args.and_then(|a| a.get_Status().ok()) }
            .unwrap_or(SPEECH_RECOGNITION_RESULT_STATUS_UNKNOWN);
        let _ = self.0.send(status);
        Ok(())
    }
}
struct HypothesisHandler(Sender<SttEvent>);
impl SpeechHypothesisHandlerImpl for HypothesisHandler {
    fn Invoke(
        &self,
        _: Option<&SpeechRecognizer>,
        args: Option<&SpeechHypothesisArgs>,
    ) -> Result<(), HRESULT> {
        unsafe {
            if let Some(args) = args {
                if let Ok(hypothesis) = args.get_Hypothesis() {
                    if let Ok(text) = hypothesis.get_Text() {
                        let _ = self.0.send(SttEvent::Partial(text.to_string()));
                    }
                }
            }
        }
        Ok(())
    }
}
