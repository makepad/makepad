//! The transcript filter: decides which utterances are actually directed at
//! the assistant before anything is sent to the (paid, remote) backend LLM.
//!
//! An open mic hears everything — self-talk, other people in the room, the
//! assistant's own replies leaking back in. The filter judges each utterance
//! and either forwards it (possibly rewritten into a cleaner instruction) or
//! drops it as local chatter. Implementations range from [`PassthroughFilter`]
//! (forward everything, for push-to-talk setups where intent is explicit) to a
//! small local LLM judging and rewriting on its own worker thread.

use makepad_widgets::makepad_draw::thread::SignalToUI;
use std::sync::mpsc::{self, Receiver, Sender};

/// What to do with one heard utterance.
#[derive(Clone, Debug)]
pub enum FilterDecision {
    /// Send to the backend LLM, as this (possibly rewritten) instruction.
    Forward { instruction: String },
    /// Local chatter — never leaves the machine.
    Drop { reason: String },
}

/// Judges utterances on the filter worker thread. `recent_dialog` holds the
/// last few exchanges (user and assistant) as plain lines, oldest first, so
/// the judge can resolve references like "make it bigger".
///
/// Deliberately not `Send`: implementations are constructed on the worker
/// thread (see [`FilterWorker::new`]), so single-thread resources like a
/// Metal-backed LLM session can live inside.
pub trait TranscriptFilter {
    fn judge(&mut self, utterance: &str, recent_dialog: &[String]) -> FilterDecision;
}

/// Forwards every utterance untouched. The right filter when input arrives
/// through push-to-talk or a text box: the user already expressed intent.
pub struct PassthroughFilter;

impl TranscriptFilter for PassthroughFilter {
    fn judge(&mut self, utterance: &str, _recent_dialog: &[String]) -> FilterDecision {
        FilterDecision::Forward {
            instruction: utterance.to_string(),
        }
    }
}

pub(crate) struct FilterJob {
    pub utterance: String,
    pub recent_dialog: Vec<String>,
}

pub(crate) struct FilterResult {
    pub utterance: String,
    pub decision: FilterDecision,
}

/// Runs a [`TranscriptFilter`] on its own thread: judging may block for as
/// long as a local LLM takes, which must never stall the UI thread.
pub(crate) struct FilterWorker {
    jobs: Sender<FilterJob>,
    results: Receiver<FilterResult>,
    signal: SignalToUI,
}

impl FilterWorker {
    /// `make_filter` runs on the worker thread, so the filter itself never
    /// crosses threads — only the factory must be `Send`.
    pub fn new(
        make_filter: impl FnOnce() -> Box<dyn TranscriptFilter> + Send + 'static,
    ) -> Self {
        let (jobs, job_rx) = mpsc::channel::<FilterJob>();
        let (result_tx, results) = mpsc::channel();
        let signal = SignalToUI::new();
        let worker_signal = signal.clone();
        std::thread::spawn(move || {
            let mut filter = make_filter();
            while let Ok(job) = job_rx.recv() {
                let decision = filter.judge(&job.utterance, &job.recent_dialog);
                if result_tx
                    .send(FilterResult {
                        utterance: job.utterance,
                        decision,
                    })
                    .is_err()
                {
                    break;
                }
                worker_signal.set();
            }
        });
        Self {
            jobs,
            results,
            signal,
        }
    }

    pub fn submit(&self, job: FilterJob) {
        let _ = self.jobs.send(job);
    }

    /// Completed judgements, if the worker signalled since the last poll.
    pub fn poll(&self) -> Vec<FilterResult> {
        if !self.signal.check_and_clear() {
            return Vec::new();
        }
        let mut out = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            out.push(result);
        }
        out
    }
}
