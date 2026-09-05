//! Voice attention gate (route.md M3): endpointed transcripts from the
//! VoiceWave mic (pure-Rust whisper + Silero VAD, no external processes)
//! are judged by a QwenFilter on the 4B — SEND when directed at the
//! assistant (wake word "computer" or clear in-context commands), SKIP for
//! ambient speech. The filter session is `!Send`, so it lives on its own
//! worker thread (same pattern as LocalAgent / converse FilterWorker).

use makepad_converse::filter::{FilterDecision, TranscriptFilter};
use makepad_converse::qwen_filter::QwenFilter;
use makepad_widgets::*;
use std::sync::mpsc::{channel, Receiver, Sender};

pub const FILTER_MODEL: &str = "local/models/Qwen3.5-4B-Q5_K_M.gguf";
/// Recent user/assistant lines given to the filter for in-context
/// directedness (matches converse's RECENT_DIALOG_LINES).
pub const RECENT_DIALOG_LINES: usize = 12;

const APP_CONTEXT: &str = "\
The assistant is 'computer', a voice-driven car navigation copilot on a live map \
(routes, destinations, charge stops, weather, sights, map layers). The cabin may \
contain other conversations. FORWARD when the speaker addresses 'computer' by name, \
or when the utterance is clearly a command or question for the navigator in context \
(e.g. a follow-up to the assistant's last answer, or an obvious nav request like \
'take the next exit' / 'find a charger'). Strip the wake word from the forwarded \
instruction. SKIP greetings between people, phone calls, music, mumbling and talk \
not meant for the assistant.";

struct GateJob {
    utterance: String,
    recent: Vec<String>,
}

pub enum GateResult {
    /// Filter model loaded and warmed (startup eager-load).
    Ready { secs: f64 },
    Send { raw: String, instruction: String },
    Skip { raw: String, reason: String },
}

pub struct VoiceGate {
    jobs: Sender<GateJob>,
    results: Receiver<GateResult>,
    signal: SignalToUI,
}

impl VoiceGate {
    pub fn new(spawner: ThreadSpawner) -> Self {
        let (jobs, job_rx) = channel::<GateJob>();
        let (result_tx, results) = channel();
        let signal = SignalToUI::new();
        let worker_signal = signal.clone();
        let spawned = spawner.spawn_worker(
            ThreadOptions {
                name: Some("route-voice-gate".into()),
                ..Default::default()
            },
            move || {
                // QwenFilter is !Send — construct on this thread. Eager
                // warm-up: the model otherwise loads on the FIRST spoken
                // utterance, which is the worst moment for a 3s stall.
                let t0 = Cx::monotonic_now();
                let mut filter = QwenFilter::new(FILTER_MODEL, APP_CONTEXT);
                let _ = filter.judge("warmup", &[]);
                let _ = result_tx.send(GateResult::Ready {
                    secs: Cx::monotonic_now() - t0,
                });
                worker_signal.set();
                while let Ok(job) = job_rx.recv() {
                    let decision = filter.judge(&job.utterance, &job.recent);
                    let result = match decision {
                        FilterDecision::Forward { instruction } => GateResult::Send {
                            raw: job.utterance,
                            instruction,
                        },
                        FilterDecision::Drop { reason } => GateResult::Skip {
                            raw: job.utterance,
                            reason,
                        },
                    };
                    if result_tx.send(result).is_err() {
                        break;
                    }
                    worker_signal.set();
                }
            },
        );
        match spawned {
            Ok(handle) => handle.detach(),
            Err(error) => log!("voice gate worker unavailable: {error}"),
        }
        Self {
            jobs,
            results,
            signal,
        }
    }

    pub fn submit(&self, utterance: String, recent: Vec<String>) {
        let _ = self.jobs.send(GateJob { utterance, recent });
    }

    pub fn poll(&mut self) -> Vec<GateResult> {
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
