use crate::{Segment, WhisperModel, WhisperParams, WhisperState};
use std::path::Path;

const DEFAULT_MODEL_PATH: &str = "ggml-large-v3-turbo.bin";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceBackendKind {
    Whisper,
    NativeApple,
}

impl VoiceBackendKind {
    pub fn default_for_platform() -> Self {
        #[cfg(target_os = "ios")]
        {
            Self::NativeApple
        }
        #[cfg(all(target_os = "macos", not(force_whisper)))]
        {
            Self::NativeApple
        }
        #[cfg(not(any(target_os = "ios", all(target_os = "macos", not(force_whisper)))))]
        {
            Self::Whisper
        }
    }

    pub fn from_makepad_env() -> Self {
        #[cfg(target_os = "ios")]
        {
            return Self::NativeApple;
        }

        if std::env::var("MAKEPAD").ok().is_some_and(|configs| {
            configs
                .split(['+', ','])
                .any(|config| config.eq_ignore_ascii_case("whisper"))
        }) {
            return Self::Whisper;
        }
        Self::default_for_platform()
    }
}

#[derive(Clone, Debug)]
pub struct VoiceTranscribeParams {
    pub language: String,
    pub translate: bool,
    pub include_timestamps: bool,
    pub single_segment: bool,
    pub max_tokens: usize,
    pub silence_threshold: f32,
    pub suppress_blank: bool,
    pub temperature: f32,
}

impl Default for VoiceTranscribeParams {
    fn default() -> Self {
        let whisper = WhisperParams::default();
        Self {
            language: whisper.language,
            translate: whisper.translate,
            include_timestamps: !whisper.no_timestamps,
            single_segment: whisper.single_segment,
            max_tokens: whisper.max_tokens,
            silence_threshold: whisper.no_speech_thold,
            suppress_blank: whisper.suppress_blank,
            temperature: whisper.temperature,
        }
    }
}

impl VoiceTranscribeParams {
    pub fn for_live_dictation() -> Self {
        let mut out = Self::default();
        out.include_timestamps = false;
        out.single_segment = true;
        out.max_tokens = 48;
        out.silence_threshold = 0.65;
        out.suppress_blank = true;
        out.temperature = 0.0;
        out
    }

    fn to_whisper_params(&self) -> WhisperParams {
        let mut out = WhisperParams::default();
        out.language = self.language.clone();
        out.translate = self.translate;
        out.no_timestamps = !self.include_timestamps;
        out.single_segment = self.single_segment;
        out.max_tokens = self.max_tokens;
        out.no_speech_thold = self.silence_threshold;
        out.suppress_blank = self.suppress_blank;
        out.temperature = self.temperature;
        out
    }
}

#[derive(Debug)]
pub enum VoiceTranscribeError {
    BackendUnavailable(&'static str),
    ModelLoadFailed(String),
    BackendFailed(&'static str),
}

pub struct WhisperTranscriber {
    model_path: String,
    model: Option<WhisperModel>,
    state: Option<WhisperState>,
    model_load_failed: bool,
}

impl WhisperTranscriber {
    fn model_path_from_env() -> String {
        std::env::var("MAKEPAD_VOICE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL_PATH.to_string())
    }

    pub fn new_from_env() -> Self {
        Self {
            model_path: Self::model_path_from_env(),
            model: None,
            state: None,
            model_load_failed: false,
        }
    }

    fn ensure_loaded(&mut self) -> Result<(), VoiceTranscribeError> {
        if self.model.is_some() && self.state.is_some() {
            return Ok(());
        }
        if self.model_load_failed {
            return Err(VoiceTranscribeError::ModelLoadFailed(
                self.model_path.clone(),
            ));
        }
        match WhisperModel::load_file(&self.model_path) {
            Ok(model) => {
                self.state = Some(WhisperState::new(&model));
                self.model = Some(model);
                Ok(())
            }
            Err(_) => {
                self.model_load_failed = true;
                Err(VoiceTranscribeError::ModelLoadFailed(
                    self.model_path.clone(),
                ))
            }
        }
    }

    pub fn preload(&mut self, _params: &VoiceTranscribeParams) -> Result<(), VoiceTranscribeError> {
        self.ensure_loaded()
    }

    pub fn transcribe(
        &mut self,
        samples: &[f32],
        params: &VoiceTranscribeParams,
    ) -> Result<Vec<Segment>, VoiceTranscribeError> {
        self.ensure_loaded()?;
        let whisper = params.to_whisper_params();
        match (self.model.as_ref(), self.state.as_mut()) {
            (Some(model), Some(state)) => Ok(state.transcribe(model, samples, &whisper)),
            _ => Err(VoiceTranscribeError::BackendFailed(
                "whisper backend state unavailable",
            )),
        }
    }
}

pub struct NativeAppleTranscriber;

#[cfg(any(target_os = "ios", all(target_os = "macos", not(force_whisper))))]
impl NativeAppleTranscriber {
    pub fn new() -> Self {
        Self
    }

    pub fn preload(&mut self, params: &VoiceTranscribeParams) -> Result<(), VoiceTranscribeError> {
        crate::apple_speech::ensure_model(&params.language)
            .map_err(|_| VoiceTranscribeError::BackendFailed("apple ensure_model failed"))
    }

    pub fn transcribe(
        &mut self,
        samples: &[f32],
        params: &VoiceTranscribeParams,
    ) -> Result<Vec<Segment>, VoiceTranscribeError> {
        Ok(crate::apple_speech::transcribe(
            samples,
            &params.to_whisper_params(),
        ))
    }
}

#[cfg(not(any(target_os = "ios", all(target_os = "macos", not(force_whisper)))))]
impl NativeAppleTranscriber {
    pub fn new() -> Self {
        Self
    }

    pub fn preload(&mut self, _params: &VoiceTranscribeParams) -> Result<(), VoiceTranscribeError> {
        Err(VoiceTranscribeError::BackendUnavailable(
            "native apple backend unavailable",
        ))
    }

    pub fn transcribe(
        &mut self,
        _samples: &[f32],
        _params: &VoiceTranscribeParams,
    ) -> Result<Vec<Segment>, VoiceTranscribeError> {
        Err(VoiceTranscribeError::BackendUnavailable(
            "native apple backend unavailable",
        ))
    }
}

pub enum VoiceTranscriber {
    Whisper(WhisperTranscriber),
    NativeApple(NativeAppleTranscriber),
}

impl VoiceTranscriber {
    pub fn new(kind: VoiceBackendKind) -> Self {
        match kind {
            VoiceBackendKind::Whisper => Self::Whisper(WhisperTranscriber::new_from_env()),
            VoiceBackendKind::NativeApple => Self::NativeApple(NativeAppleTranscriber::new()),
        }
    }

    pub fn from_makepad_env() -> Self {
        #[cfg(target_os = "ios")]
        {
            return Self::NativeApple(NativeAppleTranscriber::new());
        }

        let kind = VoiceBackendKind::from_makepad_env();

        #[cfg(all(target_os = "macos", not(force_whisper)))]
        {
            let model_path = WhisperTranscriber::model_path_from_env();
            let model_exists = Path::new(&model_path).exists();

            if kind == VoiceBackendKind::Whisper {
                if !model_exists {
                    eprintln!(
                        "[voice] whisper model not found at '{}', using native apple backend",
                        model_path
                    );
                    return Self::NativeApple(NativeAppleTranscriber::new());
                }
                return Self::Whisper(WhisperTranscriber::new_from_env());
            }

            // On Apple platforms, auto-prefer Whisper when the model is available.
            if model_exists {
                return Self::Whisper(WhisperTranscriber::new_from_env());
            }
        }

        Self::new(kind)
    }

    pub fn kind(&self) -> VoiceBackendKind {
        match self {
            Self::Whisper(_) => VoiceBackendKind::Whisper,
            Self::NativeApple(_) => VoiceBackendKind::NativeApple,
        }
    }

    pub fn preload(&mut self, params: &VoiceTranscribeParams) -> Result<(), VoiceTranscribeError> {
        match self {
            Self::Whisper(inner) => inner.preload(params),
            Self::NativeApple(inner) => inner.preload(params),
        }
    }

    pub fn transcribe(
        &mut self,
        samples: &[f32],
        params: &VoiceTranscribeParams,
    ) -> Result<Vec<Segment>, VoiceTranscribeError> {
        match self {
            Self::Whisper(inner) => inner.transcribe(samples, params),
            Self::NativeApple(inner) => inner.transcribe(samples, params),
        }
    }
}
