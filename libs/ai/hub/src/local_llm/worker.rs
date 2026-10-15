//! The in-process engine itself: the `LocalLlmSession` and the worker
//! thread that owns the `!Send` llama session (feature `llm`). The prompt
//! format, the event and config types and the tool-call parser live in the
//! parent module so a featureless build (the hub election over the LAN or a
//! machine node) shares them.

use super::*;
use makepad_ai_llm::{
    LlamaSamplerState, LlamaSamplingParams, LlamaSession, LlamaSessionConfig,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
};

pub(crate) enum WorkerMsg {
    UserTurn(String),
    /// One (text, is_error) per tool the last turn asked for, in call order.
    ToolResults(Vec<(String, bool)>),
}

/// A running local-model chat: channels to a dedicated worker thread.
pub struct LocalLlmSession {
    to_worker: Sender<WorkerMsg>,
    from_worker: Receiver<ChatEvent>,
    cancel: Arc<AtomicBool>,
}

impl LocalLlmSession {
    /// Start loading the model. Nothing blocks: the load happens on the worker
    /// and reports itself through [`LocalLlmSession::poll`].
    pub fn start(config: LocalLlmConfig, prefix: String, wake: Option<WakeHook>) -> Self {
        let (event_tx, from_worker) = channel();
        let (to_worker, msg_rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        thread::Builder::new()
            .name("ai-hub-local-llm".into())
            .spawn(move || worker_main(config, prefix, None, msg_rx, event_tx, worker_cancel, wake, None))
            .expect("spawn local llm worker");
        Self {
            to_worker,
            from_worker,
            cancel,
        }
    }

    pub fn send_user_turn(&self, text: String) {
        self.cancel.store(false, Ordering::Relaxed);
        let _ = self.to_worker.send(WorkerMsg::UserTurn(text));
    }

    pub fn send_tool_results(&self, results: Vec<(String, bool)>) {
        let _ = self.to_worker.send(WorkerMsg::ToolResults(results));
    }

    /// Stop the turn that is running, per token. The worker closes the
    /// dangling assistant turn so the cache stays valid and drops any tool
    /// calls it had collected — an interrupted question must not keep acting.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Vec<ChatEvent> {
        self.from_worker.try_iter().collect()
    }
}

fn user_turn(text: &str) -> String {
    format!("<|im_start|>user\n{text}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n")
}

fn tool_response_turn(results: &[(String, bool)]) -> String {
    let mut out = String::from("<|im_start|>user\n");
    for (result, is_error) in results {
        out.push_str("<tool_response>\n");
        if *is_error {
            out.push_str("ERROR: ");
        }
        out.push_str(result);
        out.push_str("\n</tool_response>\n");
    }
    out.push_str("<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n");
    out
}

/// Hold leading whitespace until the turn has proved it has visible content.
/// Once the first non-whitespace fragment arrives, streaming is immediate.
#[derive(Default)]
struct LeadingVisible {
    pending: String,
    emitted: bool,
}

impl LeadingVisible {
    fn push(&mut self, text: String, send: &impl Fn(ChatEvent)) {
        if self.emitted {
            send(ChatEvent::Delta(text));
            return;
        }
        self.pending.push_str(&text);
        if !self.pending.trim().is_empty() {
            self.emitted = true;
            send(ChatEvent::Delta(std::mem::take(&mut self.pending)));
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CompletionDecision {
    Complete,
    Retry,
    Fail,
}

fn completion_decision(
    attempt: usize,
    has_visible: bool,
    has_tool_call: bool,
    interrupted: bool,
) -> CompletionDecision {
    if has_visible || has_tool_call || interrupted {
        CompletionDecision::Complete
    } else if attempt == 0 {
        CompletionDecision::Retry
    } else {
        CompletionDecision::Fail
    }
}

fn fresh_sample_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64).rotate_left(17)
}

// --------------------------------------------------------------- the worker

pub(crate) fn worker_main(
    config: LocalLlmConfig,
    prefix: String,
    // A turn that arrived before this worker existed — the election handed
    // the conversation over after a fleet node died mid-conversation, and
    // the person's next line is what re-elected. Served first.
    first: Option<WorkerMsg>,
    msg_rx: Receiver<WorkerMsg>,
    event_tx: Sender<ChatEvent>,
    cancel: Arc<AtomicBool>,
    wake: Option<WakeHook>,
    // The held machine election, when this worker won it (aicore §3). The
    // guard publishes load progress so co-located waiters ride one load,
    // and it drops — reopening the election — exactly when this worker ends.
    mut residency: Option<crate::machine::ResidencyGuard>,
) {
    let wake_consumer = move || {
        if let Some(wake) = &wake {
            wake();
        }
    };
    let send = |event: ChatEvent| {
        let _ = event_tx.send(event);
        wake_consumer();
    };

    let started = std::time::Instant::now();
    let load = {
        let event_tx = event_tx.clone();
        let wake_consumer = wake_consumer.clone();
        let residency = &mut residency;
        let mut progress = move |phase: &str, fraction: f64| {
            if let Some(guard) = residency.as_mut() {
                let _ = guard.publish(crate::machine::ResidencyState::Loading { fraction });
            }
            let _ = event_tx.send(ChatEvent::Loading {
                phase: phase.to_string(),
                fraction,
            });
            wake_consumer();
        };
        LlamaSession::load_with_progress(
            &config.model,
            LlamaSessionConfig {
                max_context: Some(config.max_context),
                ..Default::default()
            },
            &mut progress,
        )
    };
    let mut session = match load {
        Ok(session) => session,
        Err(error) => {
            send(ChatEvent::Failed(format!(
                "could not load {}: {error:?}",
                config.model.display()
            )));
            return;
        }
    };

    let tokens = match session.vocab().tokenize(&prefix, true, true) {
        Ok(tokens) => tokens,
        Err(error) => return send(ChatEvent::Failed(format!("tokenize: {error:?}"))),
    };
    let prefill_tokens = tokens.len();
    if let Err(error) = session.append_tokens(&tokens) {
        return send(ChatEvent::Failed(format!("prefill: {error:?}")));
    }
    if let Some(guard) = residency.as_mut() {
        // Resident but not serving a port: co-located claimants see the
        // election held and fall back per the documented soft failure.
        let _ = guard.publish(crate::machine::ResidencyState::Ready { port: 0 });
    }
    send(ChatEvent::Ready {
        prefill_tokens,
        secs: started.elapsed().as_secs_f64(),
    });

    let im_end = session.vocab().token_id("<|im_end|>");
    let tool_call_open = session.vocab().token_id("<tool_call>");
    let tool_call_close = session.vocab().token_id("</tool_call>");
    let think_open = session.vocab().token_id("<think>");
    let think_close = session.vocab().token_id("</think>");

    let mut first = first;
    'worker: loop {
        let msg = match first.take() {
            Some(msg) => msg,
            None => match msg_rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            },
        };
        let turn_text = match msg {
            WorkerMsg::UserTurn(text) => user_turn(&text),
            WorkerMsg::ToolResults(results) => tool_response_turn(&results),
        };
        let turn_tokens = match session.vocab().tokenize(&turn_text, true, true) {
            Ok(tokens) => tokens,
            Err(_) => continue,
        };
        if session.remaining_context() < turn_tokens.len() + config.min_remaining_context
            || session.append_tokens(&turn_tokens).is_err()
        {
            send(ChatEvent::ContextFull);
            continue;
        }

        // The boundary is the whole prompt, before any generated token. An
        // empty first attempt is rewound here and sampled once with a fresh
        // seed; replaying the unchanged greedy decode would reproduce it.
        let prompt_boundary = session.token_ids().to_vec();
        let generating = std::time::Instant::now();
        let mut attempt = 0usize;
        let (tool_calls, generated) = 'attempt: loop {
            // Stream the answer; capture <tool_call> bodies; swallow <think>.
            let mut decoder = session.vocab().text_decoder();
            let mut generated = 0usize;
            let mut in_tool_call = false;
            let mut in_think = false;
            let mut tool_body = String::new();
            let mut tool_calls: Vec<(String, Vec<(String, String)>)> = Vec::new();
            let mut visible = LeadingVisible::default();
            let mut interrupted = false;
            let sampling = LlamaSamplingParams {
                seed: fresh_sample_seed(),
                ..Default::default()
            };
            let mut sampler = LlamaSamplerState::new(sampling.seed);
            loop {
                if cancel.load(Ordering::Relaxed) {
                    // Close the dangling assistant turn so the cache stays valid,
                    // and drop the tool calls: an interrupted question must not
                    // keep looking at things.
                    if let Some(im_end) = im_end {
                        let _ = session.append_token(im_end);
                    }
                    tool_calls.clear();
                    interrupted = true;
                    break;
                }
                if generated >= config.max_new_tokens
                    || session.remaining_context() < config.min_remaining_context
                {
                    if let Some(im_end) = im_end {
                        let _ = session.append_token(im_end);
                    }
                    break;
                }
                let token = if attempt == 0 {
                    match session.next_greedy_token() {
                        Ok(Some(token)) => token,
                        // End of turn, or nothing left to say.
                        _ => break,
                    }
                } else {
                    match session.continue_sampled_with(1, sampling, &mut sampler) {
                        Ok(result) => match result.token_ids.into_iter().next() {
                            Some(token) => token,
                            None => break,
                        },
                        Err(_) => break,
                    }
                };
                generated += 1;
                if Some(token) == tool_call_open {
                    in_tool_call = true;
                    tool_body.clear();
                    continue;
                }
                if Some(token) == tool_call_close {
                    if in_tool_call {
                        in_tool_call = false;
                        match parse_tool_call(&tool_body) {
                            Ok(call) => tool_calls.push(call),
                            // Malformed: show what it tried rather than hanging.
                            Err(error) => visible.push(
                                format!("[bad tool call: {error}]"),
                                &send,
                            ),
                        }
                    }
                    continue;
                }
                if Some(token) == think_open {
                    in_think = true;
                    continue;
                }
                if Some(token) == think_close {
                    in_think = false;
                    continue;
                }
                if let Some(text) = decoder.push_token(session.vocab(), token) {
                    if in_tool_call {
                        tool_body.push_str(&text);
                    } else if !in_think {
                        visible.push(text, &send);
                    }
                }
            }
            match completion_decision(
                attempt,
                visible.emitted,
                !tool_calls.is_empty(),
                interrupted,
            ) {
                CompletionDecision::Complete => break 'attempt (tool_calls, generated),
                CompletionDecision::Retry => {
                    if session.reset().is_err() || session.append_tokens(&prompt_boundary).is_err() {
                        send(ChatEvent::Failed("could not restore prompt for empty-completion retry".into()));
                        return;
                    }
                    attempt += 1;
                }
                CompletionDecision::Fail => {
                    if session.reset().is_ok() && session.append_tokens(&prompt_boundary).is_ok() {
                        if let Some(im_end) = im_end {
                            let _ = session.append_token(im_end);
                        }
                    }
                    send(ChatEvent::Failed("empty completion".to_string()));
                    continue 'worker;
                }
            }
        };
        let secs = generating.elapsed().as_secs_f64();
        let tool_call_count = tool_calls.len();
        for (name, args) in tool_calls {
            send(ChatEvent::ToolCall { name, args });
        }
        send(ChatEvent::TurnDone {
            tool_calls: tool_call_count,
            tokens: generated,
            secs,
            context_used: session.token_count(),
            context_max: session.max_context(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_in_process_empty_completion_retries_once_then_fails() {
        let events = std::cell::RefCell::new(Vec::new());
        let send = |event| events.borrow_mut().push(event);
        let mut first = LeadingVisible::default();
        first.push(" \n\t".to_string(), &send);
        assert_eq!(
            completion_decision(0, first.emitted, false, false),
            CompletionDecision::Retry
        );

        let mut second = LeadingVisible::default();
        second.push("   ".to_string(), &send);
        assert_eq!(
            completion_decision(1, second.emitted, false, false),
            CompletionDecision::Fail
        );
        assert!(events.borrow().is_empty(), "whitespace must never escape");
        assert_eq!(
            completion_decision(0, false, true, false),
            CompletionDecision::Complete,
            "a valid tool-only turn is not empty"
        );
    }

    #[test]
    fn leading_whitespace_is_buffered_but_visible_text_streams_incrementally() {
        let mut output = LeadingVisible::default();
        let events = std::cell::RefCell::new(Vec::new());
        let send = |event| events.borrow_mut().push(event);
        output.push(" \n".to_string(), &send);
        assert!(events.borrow().is_empty());
        output.push("7".to_string(), &send);
        output.push("✅".to_string(), &send);
        let text: String = events
            .into_inner()
            .into_iter()
            .filter_map(|event| match event {
                ChatEvent::Delta(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(text, " \n7✅");
    }
}
