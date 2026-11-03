//! The client side of compute programs on a hub node (node side: `crate::program_serve`, frames:
//! `makepad_compute_program::wire`): a [`RemoteSession`] is a `makepad_compute_program::Session`
//! whose program runs on a node, over the node's kernel stream.
//!
//! It has the in-process session's surface: `send` / `interrupt` steer the program, `poll` /
//! `wait` give the same [`Updates`] (the newest output per name), `cancel` and `wait_end` end it.
//! Outputs are decoded on the thread that takes them, on its cores, and acked then: a node never
//! has more than `wire::IN_FLIGHT` versions of one output on the way, so an app that looks less
//! often gets fewer versions, never a backlog. A stream that breaks ends the session with
//! [`ProgramError::Lost`].

use crate::error::AssetAiError;
use crate::kernel_client::KernelStream;
use makepad_compute_program::codec::Codec;
use makepad_compute_program::wire;
use makepad_compute_program::{Class, Input, Limits, Program, ProgramError, Updates, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The most elements one output may have (1 GiB of words).
const MAX_OUTPUT_ELEMENTS: usize = 1 << 28;

/// What a remote session runs with (the node caps the limits at its own).
#[derive(Clone)]
pub struct RemoteConfig {
    pub params: Value,
    pub inputs: Vec<Input>,
    pub limits: Limits,
    pub seed: u32,
    /// How the node codes outputs (and the app its inputs).
    pub codec: Codec,
    /// Who waits on it: the node runs the program at this class.
    pub class: Class,
}

impl Default for RemoteConfig {
    fn default() -> RemoteConfig {
        RemoteConfig { params: Value::Nil, inputs: Vec::new(), limits: Limits::default(), seed: 0, codec: Codec::DeltaLz4, class: Class::Visible }
    }
}

pub struct RemoteSession {
    stream: Arc<KernelStream>,
    id: u64,
    device: Value,
    rev: AtomicU64,
    /// The end, once it came (a later `wait_end` answers with it).
    end: Mutex<Option<Result<Value, ProgramError>>>,
    threads: usize,
}

impl RemoteSession {
    /// Opens `program` on the stream's node: Err when the node refuses it (busy, over its
    /// limits) or the stream breaks.
    pub fn open(stream: &Arc<KernelStream>, program: &Program, config: RemoteConfig) -> Result<RemoteSession, AssetAiError> {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let open = wire::Open { program: program.clone(), params: config.params, limits: config.limits, seed: config.seed, codec: config.codec, class: config.class, inputs: config.inputs };
        let (id, device) = stream.program_open(&wire::encode_open(&open, threads))?;
        Ok(RemoteSession { stream: stream.clone(), id, device, rev: AtomicU64::new(0), end: Mutex::new(None), threads })
    }

    /// What the program runs on: `{kind, backend, threads}`.
    pub fn device(&self) -> &Value {
        &self.device
    }

    /// Delivers a message (the program's inbox keeps the newest per key). Returns its rev.
    pub fn send(&self, key: &str, value: Value) -> u64 {
        self.post(key, value, false)
    }

    /// [`Self::send`], and the device work in flight stops (see `Session::interrupt`).
    pub fn interrupt(&self, key: &str, value: Value) -> u64 {
        self.post(key, value, true)
    }

    fn post(&self, key: &str, value: Value, interrupt: bool) -> u64 {
        // The node counts the messages in the order they come, as this does.
        let rev = self.rev.fetch_add(1, Ordering::Relaxed) + 1;
        self.stream.program_send(wire::MSG, self.id, wire::encode_msg(key, &value, interrupt));
        rev
    }

    /// Ends the program: its end comes as [`ProgramError::Cancelled`].
    pub fn cancel(&self) {
        self.stream.program_send(wire::CANCEL, self.id, Vec::new());
    }

    /// What came since the last look (never blocks on the network).
    pub fn poll(&self) -> Updates {
        self.take(false).0
    }

    /// What came since the last look, waiting for something or a [`Self::wake`] (then it may
    /// be empty); once the program ended, never waits.
    pub fn wait(&self) -> Updates {
        loop {
            let ended = self.ended();
            let (u, woken) = self.take(!ended);
            if !u.is_empty() || woken || ended {
                return u;
            }
        }
    }

    /// Makes a [`Self::wait`] (now or the next) return: for a thread that waits on the program
    /// and on its own work too.
    pub fn wake(&self) {
        self.stream.program_wake(self.id);
    }

    /// Waits for the end: the program's result.
    pub fn wait_end(&self) -> Result<Value, ProgramError> {
        loop {
            if let Some(end) = self.end.lock().unwrap().clone() {
                return end;
            }
            self.take(true);
        }
    }

    pub fn ended(&self) -> bool {
        self.end.lock().unwrap().is_some()
    }

    fn take(&self, block: bool) -> (Updates, bool) {
        let (frames, woken, closed) = self.stream.program_take(self.id, block);
        let mut out = Updates::default();
        for frame in frames {
            match wire::decode_update(&frame, MAX_OUTPUT_ELEMENTS, self.threads) {
                Ok(u) => out.merge(u),
                Err(e) => {
                    out.end = Some(Err(ProgramError::Lost(format!("a bad update from the node: {e}"))));
                    self.cancel();
                }
            }
        }
        for o in &out.outputs {
            self.stream.program_send(wire::ACK, self.id, wire::encode_ack(&o.name, o.state, o.version));
        }
        let mut end = self.end.lock().unwrap();
        if out.end.is_none() && end.is_none() {
            if let Some(why) = closed {
                out.end = Some(Err(ProgramError::Lost(why)));
            }
        }
        if end.is_none() {
            end.clone_from(&out.end);
        }
        (out, woken)
    }
}

impl Drop for RemoteSession {
    fn drop(&mut self) {
        if !self.ended() {
            self.cancel();
        }
        self.stream.program_forget(self.id);
    }
}
