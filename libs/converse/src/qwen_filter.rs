//! The local filtering LLM: a Qwen3.5 instruct model on `makepad-llama`
//! judging which utterances are actually directed at the assistant.
//!
//! An open microphone hears everything. Sending it all to the cloud agent is
//! wasteful and noisy, so this filter runs a small local model (Qwen3.5-9B
//! Q4 fits in ~6 GB) that either rewrites an utterance into one clean
//! instruction (`SEND`) or drops it as local chatter (`SKIP`). Greedy
//! decoding, non-thinking chat template: deterministic and as fast as the
//! model allows.
//!
//! The model file loads lazily on the first judgement — construction is
//! cheap, so apps can create the filter on the UI thread and let the
//! [`crate::filter::FilterWorker`] thread pay for the load.

use crate::filter::{FilterDecision, TranscriptFilter};
use makepad_llama::{LlamaSession, LlamaSessionConfig};
use makepad_widgets::log;

/// Cap on generated tokens per judgement: one SEND/SKIP line, never an essay.
const MAX_JUDGE_TOKENS: usize = 96;
/// Judgement prompts are a few hundred tokens; capping the context keeps the
/// KV cache tiny instead of sized for the model's native 256k window.
const MAX_FILTER_CONTEXT: u32 = 2048;
/// Prefill batching. Batched prefill needed a ggml metal dispatch fix (the
/// non-flat unary kernel's grid dropped its ne0-chunk factor, silently
/// truncating rows past 1024 elements — fixed 2026-07-28); with that in,
/// batch 64 measures ~350-600 tok/s vs ~28 tok/s at batch 1.
const FILTER_PREFILL_BATCH: usize = 64;

pub struct QwenFilter {
    model_path: String,
    /// One line of app context for the system prompt, e.g. "The assistant
    /// builds and edits a 3D game while the user watches."
    app_context: String,
    session: Option<LlamaSession>,
    load_failed: bool,
}

impl QwenFilter {
    /// Cheap: the model loads on the first [`TranscriptFilter::judge`] call.
    pub fn new(model_path: &str, app_context: &str) -> Self {
        Self {
            model_path: model_path.to_string(),
            app_context: app_context.to_string(),
            session: None,
            load_failed: false,
        }
    }

    fn ensure_loaded(&mut self) -> Option<&mut LlamaSession> {
        if self.session.is_none() && !self.load_failed {
            let started = std::time::Instant::now();
            let config = LlamaSessionConfig {
                max_context: Some(MAX_FILTER_CONTEXT),
                prefill_batch_size: FILTER_PREFILL_BATCH,
                ..LlamaSessionConfig::default()
            };
            match LlamaSession::load(&self.model_path, config) {
                Ok(session) => {
                    log!(
                        "filter: loaded {} in {:.1}s",
                        self.model_path,
                        started.elapsed().as_secs_f64()
                    );
                    self.session = Some(session);
                }
                Err(err) => {
                    log!("filter: cannot load {}: {err:?} — forwarding everything", self.model_path);
                    self.load_failed = true;
                }
            }
        }
        self.session.as_mut()
    }

    /// The full ChatML prompt for one judgement, ending in the non-thinking
    /// assistant prefill so the answer starts immediately.
    fn prompt(&self, utterance: &str, recent_dialog: &[String]) -> String {
        let mut out = String::with_capacity(2048);
        out.push_str("<|im_start|>system\n");
        out.push_str(
            "You are the attention gate between a room microphone and the AI assistant \
             inside this application. ",
        );
        if !self.app_context.is_empty() {
            out.push_str(&self.app_context);
            out.push(' ');
        }
        out.push_str(
            "Every transcribed utterance the microphone hears is shown to you, with recent \
             dialog for context. Much of it is NOT for the assistant: people talking to each \
             other or to themselves, background speech, fragments, or the assistant's own \
             spoken replies leaking back into the microphone.\n\
             \n\
             Answer with EXACTLY one line:\n\
             SEND: <instruction> — the utterance is meant for the assistant. Rewrite it as one \
             clear, self-contained instruction: fix obvious speech-recognition errors, drop \
             filler words, resolve references like \"it\" from the dialog, keep the user's \
             language and intent.\n\
             SKIP: <short reason> — not meant for the assistant.\n\
             \n\
             Rules:\n\
             - Hesitations and unfinished fragments: SKIP (the rest is still coming).\n\
             - Speech addressed to another person in the room: SKIP.\n\
             - The assistant's own reply echoed back by the microphone: SKIP.\n\
             - Follow-ups that continue the current task are for the assistant even without \
             an address word (\"now make it red\").\n\
             \n\
             Examples:\n\
             Heard: \"uh can you um can you make the castle way bigger\" -> SEND: Make the castle much bigger.\n\
             Heard: \"mom says dinner is in five minutes\" -> SKIP: talking to someone else\n\
             Heard: \"okay I added a red dragon next to the tower\" -> SKIP: assistant's own reply echoed\n\
             Heard: \"and put a uh\" -> SKIP: unfinished fragment\n",
        );
        out.push_str("<|im_end|>\n<|im_start|>user\n");
        if recent_dialog.is_empty() {
            out.push_str("Recent dialog: (none)\n");
        } else {
            out.push_str("Recent dialog:\n");
            for line in recent_dialog {
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str("\nHeard: \"");
        out.push_str(utterance.trim());
        out.push_str("\"<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n");
        out
    }
}

impl TranscriptFilter for QwenFilter {
    fn judge(&mut self, utterance: &str, recent_dialog: &[String]) -> FilterDecision {
        let prompt = self.prompt(utterance, recent_dialog);
        let Some(session) = self.ensure_loaded() else {
            // No local model: fail open, the backend still gets the raw text.
            return FilterDecision::Forward {
                instruction: utterance.to_string(),
            };
        };
        let started = std::time::Instant::now();
        let reply = (|| -> Result<String, String> {
            let tokens = session
                .vocab()
                .tokenize(&prompt, true, true)
                .map_err(|err| format!("tokenize: {err:?}"))?;
            session.reset().map_err(|err| format!("reset: {err:?}"))?;
            session
                .append_tokens(&tokens)
                .map_err(|err| format!("prefill: {err:?}"))?;
            let generation = session
                .continue_greedy(MAX_JUDGE_TOKENS)
                .map_err(|err| format!("decode: {err:?}"))?;
            Ok(generation.text)
        })();
        match reply {
            Ok(text) => {
                let decision = parse_verdict(&text, utterance);
                log!(
                    "filter: {:?} in {:.2}s (heard {utterance:?})",
                    decision,
                    started.elapsed().as_secs_f64()
                );
                decision
            }
            Err(err) => {
                log!("filter: judgement failed ({err}) — forwarding raw");
                FilterDecision::Forward {
                    instruction: utterance.to_string(),
                }
            }
        }
    }
}

/// Parse the model's `SEND:`/`SKIP:` line. Anything unparseable fails open:
/// forwarding chatter costs a little, dropping a real command costs trust.
fn parse_verdict(reply: &str, raw_utterance: &str) -> FilterDecision {
    // Tolerate a thinking block even though the prompt disables it.
    let reply = match reply.split("</think>").last() {
        Some(tail) => tail,
        None => reply,
    };
    for line in reply.lines() {
        let line = line.trim();
        let upper = line.to_ascii_uppercase();
        if let Some(rest) = upper
            .starts_with("SEND:")
            .then(|| line[5..].trim())
            .filter(|rest| !rest.is_empty())
        {
            return FilterDecision::Forward {
                instruction: rest.to_string(),
            };
        }
        if upper.starts_with("SKIP:") {
            return FilterDecision::Drop {
                reason: line[5..].trim().to_string(),
            };
        }
    }
    FilterDecision::Forward {
        instruction: raw_utterance.to_string(),
    }
}
