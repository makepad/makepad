use crate::{
    build_initial_prompt, build_repair_prompt, extract_musicxml_candidates, record_provenance,
    BrokerError, CandidateSource, GenerationProvenance, GenerationRequest, ModelPrompt,
    MusicalProblem, ProvenanceError, ScoreChatBroker,
};
use makepad_asset_client::{
    ChatCreateRequest, ChatEventBodyDto, ChatProviderDto, ChatProviderKind, ChatProviderLocality,
    ChatProviderStateDto, ChatSendRequest, ChatSessionId,
};
use makepad_musicxml::MusicXmlDocument;
use makepad_score::model::{AnnotationId, Score};
use std::fmt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Narrow integration point for the semantic MusicXML importer.
///
/// TODO(score-import): when `makepad-score-import` lands in the workspace,
/// provide its adapter here. Keeping this trait document-based ensures the AI
/// crate neither duplicates nor weakens that importer's conversion rules.
pub trait ScoreImporter {
    fn import(&self, document: &MusicXmlDocument) -> Result<Score, ImportError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportError {
    pub message: String,
}

impl ImportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalityPolicy {
    AllowCloud,
    LocalOnly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    pub max_attempts: u8,
    pub event_wait_ms: u64,
    pub event_page_limit: u32,
    /// Bounds a broker that returns empty pages immediately instead of
    /// honoring the requested long poll.
    pub max_empty_event_pages: u8,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            event_wait_ms: 15_000,
            event_page_limit: 128,
            max_empty_event_pages: 8,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProgressEvent {
    ProvidersEnumerated(Vec<ChatProviderDto>),
    SessionCreated(ChatSessionId),
    AttemptStarted { attempt: u8, prompt: ModelPrompt },
    ModelDelta { attempt: u8, text: String },
    ToolProgress { attempt: u8, permille: u16, note: String },
    CandidateEvaluated {
        attempt: u8,
        candidate: usize,
        problems: Option<usize>,
        error: Option<String>,
    },
    AttemptFinished {
        attempt: u8,
        selected_candidate: Option<usize>,
        remaining_problems: Option<usize>,
        improved: bool,
    },
    RepairStoppedNoImprovement { attempt: u8 },
    CancelRequested,
    SessionRetireFailed(String),
    Finished { valid: bool, attempts: u8 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateReport {
    pub candidate: usize,
    pub source: CandidateSource,
    pub complete: bool,
    pub parse_error: Option<String>,
    pub import_error: Option<String>,
    pub problems: Vec<MusicalProblem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptReport {
    pub attempt: u8,
    pub prompt: ModelPrompt,
    pub response: String,
    pub stream_error: Option<String>,
    pub candidates: Vec<CandidateReport>,
    pub selected_candidate: Option<usize>,
    pub improved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationOutcome {
    /// A normal semantic score. All notes remain editable by ordinary tools.
    pub score: Score,
    pub musicxml: String,
    pub remaining_problems: Vec<MusicalProblem>,
    pub attempts: Vec<AttemptReport>,
    pub provenance: GenerationProvenance,
    pub provenance_annotation: AnnotationId,
    pub retire_error: Option<BrokerError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GenerationError {
    InvalidConfiguration(String),
    Broker(BrokerError),
    ProviderNotAdvertised(ChatProviderKind),
    ProviderUnavailable { provider: ChatProviderKind, reason: String },
    LocalityRefused { provider: ChatProviderKind },
    Cancelled,
    RemoteCancelled,
    ModelError { code: String, message: String },
    UnexpectedToolCall { name: String },
    EmptyEventStream,
    NoUsableCandidate,
    Provenance(ProvenanceError),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => f.write_str(message),
            Self::Broker(error) => write!(f, "chat broker failed: {error}"),
            Self::ProviderNotAdvertised(provider) => {
                write!(f, "provider {} is not advertised by the broker", provider.as_str())
            }
            Self::ProviderUnavailable { provider, reason } => write!(
                f,
                "provider {} is unavailable: {reason}",
                provider.as_str()
            ),
            Self::LocalityRefused { provider } => write!(
                f,
                "local-only policy refuses cloud provider {}",
                provider.as_str()
            ),
            Self::Cancelled => f.write_str("score generation cancelled"),
            Self::RemoteCancelled => f.write_str("provider cancelled score generation"),
            Self::ModelError { code, message } => {
                write!(f, "provider error {code}: {message}")
            }
            Self::UnexpectedToolCall { name } => {
                write!(f, "score generation refuses unexpected tool call {name}")
            }
            Self::EmptyEventStream => f.write_str("chat event stream remained empty"),
            Self::NoUsableCandidate => {
                f.write_str("no reply contained importable MusicXML")
            }
            Self::Provenance(error) => write!(f, "could not record provenance: {error}"),
        }
    }
}

impl std::error::Error for GenerationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationFailure {
    pub error: GenerationError,
    pub attempts: Vec<AttemptReport>,
}

struct BestCandidate {
    score: Score,
    musicxml: String,
    problems: Vec<MusicalProblem>,
    prompt: ModelPrompt,
    attempt: u8,
}

struct EvaluatedCandidate {
    index: usize,
    score: Score,
    musicxml: String,
    problems: Vec<MusicalProblem>,
}

struct StreamFailure {
    error: GenerationError,
    partial_response: String,
}

pub struct ScoreAiEngine<'a, B, I> {
    broker: &'a B,
    importer: &'a I,
    config: EngineConfig,
}

impl<'a, B, I> ScoreAiEngine<'a, B, I>
where
    B: ScoreChatBroker,
    I: ScoreImporter,
{
    pub fn new(broker: &'a B, importer: &'a I) -> Self {
        Self {
            broker,
            importer,
            config: EngineConfig::default(),
        }
    }

    pub fn with_config(mut self, config: EngineConfig) -> Self {
        self.config = config;
        self
    }

    /// Returns broker rows unchanged, including explicit Unavailable states.
    pub fn providers(&self) -> Result<Vec<ChatProviderDto>, BrokerError> {
        self.broker.providers()
    }

    pub fn generate<F>(
        &self,
        request: &GenerationRequest,
        provider: ChatProviderKind,
        locality: LocalityPolicy,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<GenerationOutcome, GenerationFailure>
    where
        F: FnMut(ProgressEvent),
    {
        if self.config.max_attempts == 0
            || self.config.event_page_limit == 0
            || self.config.max_empty_event_pages == 0
        {
            return Err(failure(
                GenerationError::InvalidConfiguration(
                    "attempt count, page limit, and empty-page limit must be positive".to_string(),
                ),
                Vec::new(),
            ));
        }
        if cancellation.is_cancelled() {
            return Err(failure(GenerationError::Cancelled, Vec::new()));
        }

        let providers = self
            .broker
            .providers()
            .map_err(|error| failure(GenerationError::Broker(error), Vec::new()))?;
        progress(ProgressEvent::ProvidersEnumerated(providers.clone()));
        let selected = providers
            .iter()
            .find(|row| row.kind == provider)
            .ok_or_else(|| {
                failure(
                    GenerationError::ProviderNotAdvertised(provider),
                    Vec::new(),
                )
            })?;
        enforce_provider(selected, locality).map_err(|error| failure(error, Vec::new()))?;
        if cancellation.is_cancelled() {
            return Err(failure(GenerationError::Cancelled, Vec::new()));
        }

        let create = ChatCreateRequest::new("score-ai", provider).with_client("score-ai");
        let session = self
            .broker
            .create(&create)
            .map_err(|error| failure(GenerationError::Broker(error), Vec::new()))?;
        progress(ProgressEvent::SessionCreated(session.clone()));

        let initial_prompt = build_initial_prompt(request);
        let mut next_prompt = initial_prompt.clone();
        let mut cursor = 0u64;
        let mut attempts = Vec::new();
        let mut best: Option<BestCandidate> = None;

        for attempt in 1..=self.config.max_attempts {
            if cancellation.is_cancelled() {
                self.cancel_and_retire(&session, &mut progress);
                return Err(failure(GenerationError::Cancelled, attempts));
            }
            progress(ProgressEvent::AttemptStarted {
                attempt,
                prompt: next_prompt.clone(),
            });
            let response = match self.send_and_stream(
                &session,
                &next_prompt,
                attempt,
                &mut cursor,
                cancellation,
                &mut progress,
            ) {
                Ok(response) => response,
                Err(stream_failure) => {
                    let error_text = stream_failure.error.to_string();
                    attempts.push(AttemptReport {
                        attempt,
                        prompt: next_prompt.clone(),
                        response: stream_failure.partial_response,
                        stream_error: Some(error_text),
                        candidates: Vec::new(),
                        selected_candidate: None,
                        improved: false,
                    });
                    progress(ProgressEvent::AttemptFinished {
                        attempt,
                        selected_candidate: None,
                        remaining_problems: None,
                        improved: false,
                    });
                    if matches!(stream_failure.error, GenerationError::Cancelled) {
                        self.cancel_and_retire(&session, &mut progress);
                    } else {
                        self.retire(&session, &mut progress);
                    }
                    return Err(failure(stream_failure.error, attempts));
                }
            };
            let (reports, selected_attempt) =
                self.evaluate_reply(&response, request, attempt, &mut progress);
            let selected_candidate = selected_attempt.as_ref().map(|candidate| candidate.index);
            let selected_problem_count = selected_attempt
                .as_ref()
                .map(|candidate| candidate.problems.len());
            let improved = selected_attempt.as_ref().is_some_and(|candidate| {
                best.as_ref()
                    .is_none_or(|current| candidate.problems.len() < current.problems.len())
            });
            if improved {
                let candidate = selected_attempt.expect("improved candidate exists");
                best = Some(BestCandidate {
                    score: candidate.score,
                    musicxml: candidate.musicxml,
                    problems: candidate.problems,
                    prompt: next_prompt.clone(),
                    attempt,
                });
            }
            attempts.push(AttemptReport {
                attempt,
                prompt: next_prompt.clone(),
                response,
                stream_error: None,
                candidates: reports,
                selected_candidate,
                improved,
            });
            progress(ProgressEvent::AttemptFinished {
                attempt,
                selected_candidate,
                remaining_problems: selected_problem_count,
                improved,
            });

            if best.as_ref().is_some_and(|candidate| candidate.problems.is_empty()) {
                break;
            }
            if attempt > 1 && !improved {
                progress(ProgressEvent::RepairStoppedNoImprovement { attempt });
                break;
            }
            if attempt == self.config.max_attempts {
                break;
            }
            let failures = repair_failures(best.as_ref(), attempts.last().expect("attempt exists"));
            next_prompt = build_repair_prompt(
                &initial_prompt,
                &failures,
                best.as_ref().map(|candidate| candidate.musicxml.as_str()),
            );
        }

        let Some(mut best) = best else {
            self.retire(&session, &mut progress);
            return Err(failure(GenerationError::NoUsableCandidate, attempts));
        };
        let provenance = GenerationProvenance {
            provider,
            prompt: best.prompt.provenance_text(),
            attempt: best.attempt,
        };
        let provenance_annotation = record_provenance(&mut best.score, &provenance).map_err(|error| {
            self.retire(&session, &mut progress);
            failure(GenerationError::Provenance(error), attempts.clone())
        })?;
        let retire_error = self.retire(&session, &mut progress);
        progress(ProgressEvent::Finished {
            valid: best.problems.is_empty(),
            attempts: attempts.len() as u8,
        });
        Ok(GenerationOutcome {
            score: best.score,
            musicxml: best.musicxml,
            remaining_problems: best.problems,
            attempts,
            provenance,
            provenance_annotation,
            retire_error,
        })
    }

    fn evaluate_reply<F>(
        &self,
        response: &str,
        request: &GenerationRequest,
        attempt: u8,
        progress: &mut F,
    ) -> (Vec<CandidateReport>, Option<EvaluatedCandidate>)
    where
        F: FnMut(ProgressEvent),
    {
        let candidates = extract_musicxml_candidates(response);
        let mut reports = Vec::with_capacity(candidates.len());
        let mut selected: Option<EvaluatedCandidate> = None;
        for (index, candidate) in candidates.into_iter().enumerate() {
            let mut report = CandidateReport {
                candidate: index,
                source: candidate.source.clone(),
                complete: candidate.complete,
                parse_error: None,
                import_error: None,
                problems: Vec::new(),
            };
            let document = match candidate.parse() {
                Ok(document) => document,
                Err(error) => {
                    report.parse_error = Some(error.clone());
                    progress(ProgressEvent::CandidateEvaluated {
                        attempt,
                        candidate: index,
                        problems: None,
                        error: Some(error),
                    });
                    reports.push(report);
                    continue;
                }
            };
            let score = match self.importer.import(&document) {
                Ok(score) => score,
                Err(error) => {
                    report.import_error = Some(error.to_string());
                    progress(ProgressEvent::CandidateEvaluated {
                        attempt,
                        candidate: index,
                        problems: None,
                        error: Some(error.to_string()),
                    });
                    reports.push(report);
                    continue;
                }
            };
            let problems = crate::validate_score(&score, &request.specification);
            report.problems = problems.clone();
            progress(ProgressEvent::CandidateEvaluated {
                attempt,
                candidate: index,
                problems: Some(problems.len()),
                error: None,
            });
            if selected
                .as_ref()
                .is_none_or(|current| problems.len() <= current.problems.len())
            {
                selected = Some(EvaluatedCandidate {
                    index,
                    score,
                    musicxml: candidate.xml,
                    problems,
                });
            }
            reports.push(report);
        }
        (reports, selected)
    }

    fn send_and_stream<F>(
        &self,
        session: &ChatSessionId,
        prompt: &ModelPrompt,
        attempt: u8,
        cursor: &mut u64,
        cancellation: &CancellationToken,
        progress: &mut F,
    ) -> Result<String, StreamFailure>
    where
        F: FnMut(ProgressEvent),
    {
        let request = ChatSendRequest::text(prompt.user.clone())
            .with_dynamic_context(prompt.system.clone());
        self.broker
            .send(session, &request)
            .map_err(|error| StreamFailure {
                error: GenerationError::Broker(error),
                partial_response: String::new(),
            })?;
        let mut response = String::new();
        let mut empty_pages = 0u8;
        loop {
            if cancellation.is_cancelled() {
                progress(ProgressEvent::CancelRequested);
                return Err(StreamFailure {
                    error: GenerationError::Cancelled,
                    partial_response: response,
                });
            }
            let page = self.broker.events(
                session,
                *cursor,
                self.config.event_wait_ms,
                self.config.event_page_limit,
            );
            let page = match page {
                Ok(page) => page,
                Err(error) => {
                    return Err(StreamFailure {
                        error: GenerationError::Broker(error),
                        partial_response: response,
                    })
                }
            };
            if cancellation.is_cancelled() {
                progress(ProgressEvent::CancelRequested);
                return Err(StreamFailure {
                    error: GenerationError::Cancelled,
                    partial_response: response,
                });
            }
            *cursor = page.cursor;
            if page.events.is_empty() {
                empty_pages = empty_pages.saturating_add(1);
                if empty_pages >= self.config.max_empty_event_pages {
                    return Err(StreamFailure {
                        error: GenerationError::EmptyEventStream,
                        partial_response: response,
                    });
                }
                continue;
            }
            empty_pages = 0;
            for event in page.events {
                match event.body {
                    ChatEventBodyDto::Delta { text, .. } => {
                        response.push_str(&text);
                        progress(ProgressEvent::ModelDelta { attempt, text });
                    }
                    ChatEventBodyDto::ToolProgress { permille, note, .. } => {
                        progress(ProgressEvent::ToolProgress {
                            attempt,
                            permille,
                            note,
                        });
                    }
                    ChatEventBodyDto::ToolCall { name, .. } => {
                        return Err(StreamFailure {
                            error: GenerationError::UnexpectedToolCall { name },
                            partial_response: response,
                        });
                    }
                    ChatEventBodyDto::ToolResult { .. } => {}
                    ChatEventBodyDto::Done => return Ok(response),
                    ChatEventBodyDto::Cancelled => {
                        return Err(StreamFailure {
                            error: GenerationError::RemoteCancelled,
                            partial_response: response,
                        })
                    }
                    ChatEventBodyDto::Error { code, message } => {
                        return Err(StreamFailure {
                            error: GenerationError::ModelError { code, message },
                            partial_response: response,
                        })
                    }
                }
            }
        }
    }

    fn cancel_and_retire<F>(&self, session: &ChatSessionId, progress: &mut F)
    where
        F: FnMut(ProgressEvent),
    {
        progress(ProgressEvent::CancelRequested);
        let _ = self.broker.cancel(session);
        self.retire(session, progress);
    }

    fn retire<F>(&self, session: &ChatSessionId, progress: &mut F) -> Option<BrokerError>
    where
        F: FnMut(ProgressEvent),
    {
        match self.broker.retire(session) {
            Ok(()) => None,
            Err(error) => {
                progress(ProgressEvent::SessionRetireFailed(error.to_string()));
                Some(error)
            }
        }
    }
}

fn enforce_provider(
    provider: &ChatProviderDto,
    locality: LocalityPolicy,
) -> Result<(), GenerationError> {
    if locality == LocalityPolicy::LocalOnly
        && provider.locality != ChatProviderLocality::Local
    {
        return Err(GenerationError::LocalityRefused {
            provider: provider.kind,
        });
    }
    match &provider.state {
        ChatProviderStateDto::Available { .. } => Ok(()),
        ChatProviderStateDto::Unavailable { reason } => {
            Err(GenerationError::ProviderUnavailable {
                provider: provider.kind,
                reason: reason.clone(),
            })
        }
    }
}

fn repair_failures(best: Option<&BestCandidate>, report: &AttemptReport) -> Vec<String> {
    if let Some(best) = best {
        return best.problems.iter().map(ToString::to_string).collect();
    }
    let mut failures = Vec::new();
    if report.candidates.is_empty() {
        failures.push("reply contained no score-partwise or score-timewise root".to_string());
    }
    for candidate in &report.candidates {
        if let Some(error) = &candidate.parse_error {
            failures.push(format!("candidate {} parse failure: {error}", candidate.candidate));
        }
        if let Some(error) = &candidate.import_error {
            failures.push(format!("candidate {} import failure: {error}", candidate.candidate));
        }
    }
    if failures.is_empty() {
        failures.push("reply did not yield a usable semantic score".to_string());
    }
    failures
}

fn failure(error: GenerationError, attempts: Vec<AttemptReport>) -> GenerationFailure {
    GenerationFailure { error, attempts }
}
