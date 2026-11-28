//! The in-process engine itself: the `LocalLlmSession` and the worker
//! thread that owns the `!Send` llama session (feature `llm`). The prompt
//! format, the event and config types and the tool-call parser live in the
//! parent module so a featureless build (the hub election over the LAN or a
//! machine node) shares them.

use super::*;
use crate::hub_chat::TurnStops;
use makepad_ai_llm::{
    LlamaModel, LlamaSamplerState, LlamaSamplingParams, LlamaSession, LlamaSessionConfig,
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
    UserTurn(String, Arc<AtomicBool>),
    Reset(u64),
    /// One (text, is_error) per tool the last turn asked for, in call order.
    ToolResults(Vec<(String, bool)>),
}

/// A running local-model chat: channels to a dedicated worker thread.
pub struct LocalLlmSession {
    to_worker: Sender<WorkerMsg>,
    from_worker: Receiver<TaggedChatEvent>,
    cancel: TurnStops,
}

impl LocalLlmSession {
    /// Start loading the model. Nothing blocks: the load happens on the worker
    /// and reports itself through [`LocalLlmSession::poll`].
    pub fn start(config: LocalLlmConfig, prefix: String, wake: Option<WakeHook>) -> Self {
        let (event_tx, from_worker) = channel();
        let (to_worker, msg_rx) = channel();
        thread::Builder::new()
            .name("ai-hub-local-llm".into())
            .spawn(move || worker_main(config, prefix, None, msg_rx, 0, event_tx, wake, None))
            .expect("spawn local llm worker");
        Self {
            to_worker,
            from_worker,
            cancel: TurnStops::default(),
        }
    }

    pub fn send_user_turn(&self, text: String) {
        let _ = self.to_worker.send(WorkerMsg::UserTurn(text, self.cancel.for_line()));
    }

    pub fn send_tool_results(&self, results: Vec<(String, bool)>) {
        let _ = self.to_worker.send(WorkerMsg::ToolResults(results));
    }

    /// Stop generation. The worker restores the pre-turn tokens before the next line.
    pub fn cancel(&self) {
        self.cancel.stop();
    }

    pub fn poll(&self) -> Vec<ChatEvent> {
        self.from_worker.try_iter().map(|e| e.event).collect()
    }
}

impl Drop for LocalLlmSession {
    fn drop(&mut self) {
        self.cancel.stop();
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
) -> CompletionDecision {
    if has_visible || has_tool_call {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InProcessLimit {
    Continue,
    CallerMaxTokens,
    ContextFull,
}

fn in_process_decode_limit(
    generated: usize, max_new_tokens: usize, remaining_context: usize, min_remaining_context: usize,
) -> InProcessLimit {
    if generated >= max_new_tokens {
        InProcessLimit::CallerMaxTokens
    } else if remaining_context < min_remaining_context {
        InProcessLimit::ContextFull
    } else {
        InProcessLimit::Continue
    }
}

pub(crate) fn worker_main(
    config: LocalLlmConfig,
    prefix: String,
    // A turn that arrived before this worker existed — the election handed
    // the conversation over after a fleet node died mid-conversation, and
    // the person's next line is what re-elected. Served first.
    first: Option<WorkerMsg>,
    msg_rx: Receiver<WorkerMsg>,
    initial_gen: u64,
    event_tx: Sender<TaggedChatEvent>,
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
    let generation = std::cell::Cell::new(initial_gen);
    let send = |event: ChatEvent| {
        let _ = event_tx.send(TaggedChatEvent { generation: generation.get(), event });
        wake_consumer();
    };

    let started = std::time::Instant::now();
    let base_cfg = LlamaSessionConfig::default();
    let planned = LlamaModel::load(&config.model).and_then(|model| {
        let plan = model.plan_session_memory(base_cfg)?;
        Ok((model, plan))
    });
    let (model, plan) = match planned {
        Ok(planned) => planned,
        Err(error) => return send(ChatEvent::Failed(format!("could not plan {}: {error}", config.model.display()))),
    };
    let name = config.model.file_name().unwrap_or(config.model.as_os_str()).to_string_lossy();
    let memory = crate::gpu_yield::LlmMemory::from_plan(name.into_owned(), "f16".into(), &plan);
    let gpu = crate::gpu::query_gpu();
    let decision = match crate::gpu_yield::fit_context_detailed(
        &memory, gpu.vram_free_mb, crate::gpu_yield::effective_reserve_mb(gpu.unified),
        gpu.unified, config.max_context, crate::llm_context::MIN_CONTEXT,
    ) {
        Ok(decision) => decision,
        Err(why) => return send(ChatEvent::Failed(why)),
    };
    crate::gpu_yield::emit_log(&decision.log_line());
    let load = {
        let event_tx = event_tx.clone();
        let wake_consumer = wake_consumer.clone();
        let residency = &mut residency;
        let mut progress = move |phase: &str, fraction: f64| {
            if let Some(guard) = residency.as_mut() {
                let _ = guard.publish(crate::machine::ResidencyState::Loading { fraction });
            }
            let _ = event_tx.send(TaggedChatEvent {
                generation: initial_gen,
                event: ChatEvent::Loading { phase: phase.to_string(), fraction },
            });
            wake_consumer();
        };
        LlamaSession::from_owned_model_with_progress(
            model,
            LlamaSessionConfig {
                max_context: Some(decision.chosen),
                ..base_cfg
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

    let prefix_tokens = match session.vocab().tokenize(&prefix, true, true) {
        Ok(tokens) => tokens,
        Err(error) => return send(ChatEvent::Failed(format!("tokenize: {error:?}"))),
    };
    let prefill_tokens = prefix_tokens.len();
    if let Err(error) = session.append_tokens(&prefix_tokens) {
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
    let mut turn: Option<(Arc<AtomicBool>, Vec<i32>)> = None;
    let mut rewind = None;
    'worker: loop {
        let msg = match first.take() {
            Some(msg) => msg,
            None => match msg_rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            },
        };
        let turn_text = match msg {
            WorkerMsg::Reset(generation_id) => {
                generation.set(generation_id);
                turn = None;
                if rewind.is_some() || session.token_ids() != prefix_tokens.as_slice() {
                    rewind = Some((prefix_tokens.clone(), "the reset"));
                }
                continue;
            }
            WorkerMsg::UserTurn(text, stop) => {
                if stop.load(Ordering::Relaxed) { continue; }
                if let Some((_, kept)) = turn.take() {
                    rewind = Some((kept, "the stop"));
                }
                if let Some((kept, reason)) = &rewind {
                    // Linear attention cannot be truncated by position. Leave the
                    // original checkpoint in place if Stop interrupts this replay.
                    let progress = |fraction| send(ChatEvent::Loading {
                        phase: format!("re-reading the conversation after {reason}"), fraction,
                    });
                    progress(0.0);
                    if let Err(error) = session.reset() {
                        send(ChatEvent::Failed(format!("could not re-read the conversation after {reason}: {error:?}")));
                        continue;
                    }
                    let mut restored = 0;
                    for chunk in kept.chunks(makepad_ai_llm::lanes::DEFAULT_PREFILL_CHUNK) {
                        if stop.load(Ordering::Relaxed) { continue 'worker; }
                        if let Err(error) = session.append_tokens(chunk) {
                            send(ChatEvent::Failed(format!("could not re-read the conversation after {reason}: {error:?}")));
                            continue 'worker;
                        }
                        restored += chunk.len();
                        progress(restored as f64 / kept.len() as f64);
                    }
                }
                turn = Some((stop, rewind.take().map(|(tokens, _)| tokens).unwrap_or_else(|| session.token_ids().to_vec())));
                user_turn(&text)
            }
            WorkerMsg::ToolResults(results) => {
                let Some((stop, _)) = &turn else { continue };
                if stop.load(Ordering::Relaxed) { continue; }
                tool_response_turn(&results)
            }
        };
        let mut fail = |turn: &mut Option<(Arc<AtomicBool>, Vec<i32>)>, event| {
            let (_, kept) = turn.take().expect("a user or tool input has a turn checkpoint");
            rewind = Some((kept, "the failed turn"));
            send(event);
        };
        let turn_tokens = match session.vocab().tokenize(&turn_text, true, true) {
            Ok(tokens) => tokens,
            Err(error) => {
                fail(&mut turn, ChatEvent::Failed(format!("tokenize: {error:?}")));
                continue;
            }
        };
        if session.remaining_context() < turn_tokens.len() + config.min_remaining_context {
            fail(&mut turn, ChatEvent::ContextFull);
            continue;
        }
        if let Err(error) = session.append_tokens(&turn_tokens) {
            fail(&mut turn, ChatEvent::Failed(format!("could not append the turn: {error:?}")));
            continue;
        }

        // The boundary is the whole prompt, before any generated token. An
        // empty first attempt is rewound here and sampled once with a fresh
        // seed; replaying the unchanged greedy decode would reproduce it.
        let prompt_boundary = session.token_ids().to_vec();
        let stop = turn.as_ref().unwrap().0.clone();
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
            let mut hidden = HiddenWork::default();
            let sampling = LlamaSamplingParams {
                seed: fresh_sample_seed(),
                ..Default::default()
            };
            let mut sampler = LlamaSamplerState::new(sampling.seed);
            loop {
                if stop.load(Ordering::Relaxed) { continue 'worker; }
                match in_process_decode_limit(
                    generated, config.max_new_tokens, session.remaining_context(), config.min_remaining_context,
                ) {
                    InProcessLimit::Continue => {}
                    InProcessLimit::ContextFull => {
                        fail(&mut turn, ChatEvent::ContextFull);
                        continue 'worker;
                    }
                    InProcessLimit::CallerMaxTokens => {
                        if let Some(im_end) = im_end {
                            if let Err(error) = session.append_token(im_end) {
                                fail(&mut turn, ChatEvent::Failed(format!("could not close the reply: {error:?}")));
                                continue 'worker;
                            }
                        }
                        break;
                    }
                }
                let token = if attempt == 0 {
                    match session.next_greedy_token() {
                        Ok(Some(token)) => token,
                        Ok(None) => break,
                        Err(error) => {
                            fail(&mut turn, ChatEvent::Failed(format!("generation failed: {error:?}")));
                            continue 'worker;
                        }
                    }
                } else {
                    match session.continue_sampled_with(1, sampling, &mut sampler) {
                        Ok(result) => match result.token_ids.into_iter().next() {
                            Some(token) => token,
                            None => break,
                        },
                        Err(error) => {
                            fail(&mut turn, ChatEvent::Failed(format!("generation failed: {error:?}")));
                            continue 'worker;
                        }
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
                    if in_tool_call || in_think {
                        if let Some(note) = hidden.wrote(in_tool_call, text.len()) {
                            send(ChatEvent::Working(note));
                        }
                    }
                    if in_tool_call {
                        tool_body.push_str(&text);
                    } else if !in_think {
                        visible.push(text, &send);
                    }
                }
            }
            if stop.load(Ordering::Relaxed) { continue 'worker; }
            match completion_decision(
                attempt,
                visible.emitted,
                !tool_calls.is_empty(),
            ) {
                CompletionDecision::Complete => break 'attempt (tool_calls, generated),
                CompletionDecision::Retry => {
                    if let Err(error) = session.reset().and_then(|_| session.append_tokens(&prompt_boundary)) {
                        fail(&mut turn, ChatEvent::Failed(format!("could not restore prompt for empty-completion retry: {error:?}")));
                        continue 'worker;
                    }
                    attempt += 1;
                }
                CompletionDecision::Fail => {
                    fail(&mut turn, ChatEvent::Failed("empty completion".into()));
                    continue 'worker;
                }
            }
        };
        let secs = generating.elapsed().as_secs_f64();
        let tool_call_count = tool_calls.len();
        if tool_call_count == 0 { turn = None; }
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
    fn in_process_mid_reply_context_exhaustion_reports_context_full_not_turn_done() {
        use InProcessLimit::*;
        assert_eq!(in_process_decode_limit(100, 640, 511, 512), ContextFull);
        assert_eq!(in_process_decode_limit(640, 640, 2000, 512), CallerMaxTokens);
        assert_eq!(in_process_decode_limit(640, 640, 511, 512), CallerMaxTokens);
        assert_eq!(in_process_decode_limit(100, 640, 512, 512), Continue);
    }

    #[test]
    fn direct_in_process_empty_completion_retries_once_then_fails() {
        let events = std::cell::RefCell::new(Vec::new());
        let send = |event| events.borrow_mut().push(event);
        let mut first = LeadingVisible::default();
        first.push(" \n\t".to_string(), &send);
        assert_eq!(
            completion_decision(0, first.emitted, false),
            CompletionDecision::Retry
        );

        let mut second = LeadingVisible::default();
        second.push("   ".to_string(), &send);
        assert_eq!(
            completion_decision(1, second.emitted, false),
            CompletionDecision::Fail
        );
        assert!(events.borrow().is_empty(), "whitespace must never escape");
        assert_eq!(
            completion_decision(0, false, true),
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
