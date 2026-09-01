//! The hub chat session: the machine election wrapped around the local
//! engine (aicore §3).
//!
//! `HubChatSession::start` is what apps consume. Its worker thread runs the
//! election before any weights move:
//!
//! - a live co-located holder serving this model (a machine node / the hub
//!   service) → route the conversation there over loopback instead of
//!   loading a second copy — v1 routes only tool-less sessions; a tool pack
//!   keeps the session on the in-process engine, where the tool protocol is
//!   native;
//! - a holder still loading → wait on its published progress (one load, N
//!   clients) up to a bounded patience, then fall through;
//! - the election is open → claim it, load in-process, and publish
//!   loading/ready INTO the lock record so later apps wait instead of
//!   stampeding. The guard lives exactly as long as the worker: process
//!   death reopens the election at the OS level.
//! - the election is held by a non-serving app → log the duplicate and load
//!   anyway: blocking a person's chat on another app's private residency is
//!   worse than the documented soft failure (duplicate RAM).
//!
//! The surface is [`crate::local_llm`]'s: same [`ChatEvent`]s, same four
//! methods, so a consumer cannot tell which side of the election it landed
//! on except by its `Loading` phases.

use crate::chat_wire::{ChatMessage, ChatRole};
use crate::local_llm::{build_prefix, ChatEvent, LocalLlmConfig, ToolSpec, WakeHook};
use crate::local_llm::WorkerMsg;
use crate::machine::{self, Claim, ResidencyState};
use crate::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use crate::providers::qwen::{FleetQwenChatProvider, HttpFleetTransport};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a session waits on another process's published `Loading` before
/// giving up and loading its own copy. Weights stream for tens of seconds;
/// two minutes covers a cold 9GB load without stranding anyone forever.
const HOLDER_PATIENCE: Duration = Duration::from_secs(120);
/// Poll cadence against the lock record / the proxy provider.
const POLL: Duration = Duration::from_millis(150);

/// A running hub chat. Same shape as [`crate::local_llm::LocalLlmSession`].
pub struct HubChatSession {
    to_worker: Sender<WorkerMsg>,
    from_worker: Receiver<ChatEvent>,
    cancel: Arc<AtomicBool>,
}

pub struct HubChatConfig {
    pub llm: LocalLlmConfig,
    pub system_prompt: String,
    pub tools: Vec<ToolSpec>,
    pub wake: Option<WakeHook>,
}

impl HubChatSession {
    pub fn start(config: HubChatConfig) -> Self {
        let (event_tx, from_worker) = channel();
        let (to_worker, msg_rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        std::thread::Builder::new()
            .name("ai-hub-chat".into())
            .spawn(move || elect_and_run(config, msg_rx, event_tx, worker_cancel))
            .expect("spawn hub chat worker");
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

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Vec<ChatEvent> {
        self.from_worker.try_iter().collect()
    }
}

/// The election key for a model file: its lowercase basename — the same key
/// the service claims for a registry model's primary weights file, so an app
/// and a machine node loading one GGUF meet in one election.
fn election_key(config: &LocalLlmConfig) -> String {
    config
        .model
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "unknown-model".to_string())
}

fn elect_and_run(
    config: HubChatConfig,
    msg_rx: Receiver<WorkerMsg>,
    event_tx: Sender<ChatEvent>,
    cancel: Arc<AtomicBool>,
) {
    let wake = config.wake.clone();
    let send = |event: ChatEvent| {
        let _ = event_tx.send(event);
        if let Some(wake) = &wake {
            wake();
        }
    };
    let key = election_key(&config.llm);

    // Patience loop: route to a serving holder, wait out a loading one.
    let deadline = Instant::now() + HOLDER_PATIENCE;
    loop {
        match machine::read_holder(&key) {
            Ok(Some(record)) => match record.state {
                ResidencyState::Ready { port } if port > 0 && config.tools.is_empty() => {
                    return run_proxy(port, config, msg_rx, event_tx, cancel);
                }
                ResidencyState::Ready { .. } => {
                    // A non-serving holder (another app), or a serving one we
                    // cannot use because this session carries tools: the
                    // documented soft failure — load our own copy.
                    eprintln!(
                        "[hub-chat] {key}: held by pid {} without a usable route — \
                         loading a duplicate copy",
                        record.pid
                    );
                    break;
                }
                ResidencyState::Loading { fraction } => {
                    send(ChatEvent::Loading {
                        phase: format!("waiting on pid {}", record.pid),
                        fraction,
                    });
                    if Instant::now() > deadline {
                        eprintln!("[hub-chat] {key}: holder still loading after patience — loading our own");
                        break;
                    }
                    std::thread::sleep(POLL * 4);
                }
                ResidencyState::Failed { reason, .. } => {
                    eprintln!("[hub-chat] {key}: holder reported failure ({reason}); loading our own");
                    break;
                }
            },
            _ => break,
        }
    }

    // Try to become the host; run the local engine either way.
    let guard = match machine::claim(&key) {
        Ok(Claim::Won(mut guard)) => {
            let _ = guard.publish(ResidencyState::Loading { fraction: 0.0 });
            Some(guard)
        }
        _ => None,
    };
    let prefix = build_prefix(&config.system_prompt, &config.tools);
    crate::local_llm::worker_main(
        config.llm,
        prefix,
        msg_rx,
        event_tx,
        cancel,
        config.wake,
        guard,
    );
}

/// The proxy branch: the conversation runs on the co-located node; this
/// worker holds the transcript and speaks the provider protocol. Tool packs
/// never reach here (v1 keeps them on the in-process engine).
fn run_proxy(
    port: u16,
    config: HubChatConfig,
    msg_rx: Receiver<WorkerMsg>,
    event_tx: Sender<ChatEvent>,
    cancel: Arc<AtomicBool>,
) {
    let wake = config.wake.clone();
    let send = |event: ChatEvent| {
        let _ = event_tx.send(event);
        if let Some(wake) = &wake {
            wake();
        }
    };
    let base = format!("http://127.0.0.1:{port}");
    let mut provider = FleetQwenChatProvider::new(HttpFleetTransport, vec![base]);
    send(ChatEvent::Ready {
        prefill_tokens: 0,
        secs: 0.0,
    });
    let mut history: Vec<ChatMessage> = Vec::new();
    loop {
        let msg = match msg_rx.recv() {
            Ok(msg) => msg,
            Err(_) => return,
        };
        let text = match msg {
            WorkerMsg::UserTurn(text) => text,
            // Tool-less by contract; results arriving anyway are dropped.
            WorkerMsg::ToolResults(_) => continue,
        };
        history.push(ChatMessage::new(ChatRole::User, text));
        let input = TurnInput::new(config.system_prompt.clone(), history.clone());
        if let Err(error) = provider.begin_turn(&input) {
            send(ChatEvent::Failed(format!("machine node: {error}")));
            continue;
        }
        let started = Instant::now();
        let mut turn_text = String::new();
        'turn: loop {
            if cancel.load(Ordering::Relaxed) {
                provider.cancel();
                break 'turn;
            }
            // A queued user message means the person moved on mid-answer.
            match msg_rx.try_recv() {
                Err(TryRecvError::Empty) => {}
                _ => {
                    provider.cancel();
                    break 'turn;
                }
            }
            let mut done = false;
            for event in provider.poll() {
                match event {
                    ProviderEvent::Delta(text) => {
                        turn_text.push_str(&text);
                        send(ChatEvent::Delta(text));
                    }
                    ProviderEvent::Done { text } => {
                        let full = if text.trim().is_empty() {
                            turn_text.clone()
                        } else {
                            text
                        };
                        if !full.trim().is_empty() {
                            history.push(ChatMessage::new(ChatRole::Assistant, full));
                        }
                        send(ChatEvent::TurnDone {
                            tool_calls: 0,
                            tokens: 0,
                            secs: started.elapsed().as_secs_f64(),
                            context_used: 0,
                            context_max: 0,
                        });
                        done = true;
                    }
                    ProviderEvent::Error(error) => {
                        send(ChatEvent::Failed(format!("machine node: {error}")));
                        done = true;
                    }
                    ProviderEvent::Status { .. }
                    | ProviderEvent::Serving(_)
                    | ProviderEvent::FunctionCall { .. } => {}
                }
            }
            if done {
                break 'turn;
            }
            std::thread::sleep(POLL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_election_key_is_the_file_basename() {
        let mut config = LocalLlmConfig::new("local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf".into());
        assert_eq!(election_key(&config), "qwen3.5-9b-ud-q4_k_xl.gguf");
        config.model = "".into();
        assert_eq!(election_key(&config), "unknown-model");
    }
}
