//! The DREAM run's transport: one DECLARED pipeline, then its record.
//!
//! What used to live here (in `expand.rs`) was a chat-broker turn the VJ ran
//! itself, before any job existed, so it could paste the answer into the
//! body of the stage it was about to enqueue — and then sit alive through
//! the whole run re-posting bodies as each stage finished. That whole
//! machine is gone. The store now takes a declared graph:
//!
//!   `POST /v1/pipelines` — expand → image → video, with `$from_stage`
//!   references spliced in AT CLAIM, the first moment every dependency has
//!   provably succeeded.
//!
//! So the app's job shrinks to three requests it can make from anywhere:
//! declare the run, read its record, stop it. Nothing here advances a
//! stage; if the VJ quits mid-run the fleet finishes it anyway and the clip
//! is on the grid when the app comes back.
//!
//! WHY ITS OWN THREAD and not the catalog runtime: the runtime speaks a
//! fixed `ClientRequest` vocabulary that has no pipeline calls, and adding
//! ones belongs to the client crate, not to this app. The pattern is the
//! one the broker expander already used — a worker thread holding an `Api`
//! built from the session's own verified endpoints and token.

use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    Api, ApiEndpoints, HttpLimits, PipelineCancelDto, PipelineCreatedDto, PipelineDetailDto,
    PipelineId, PipelineStageSpec,
};
use std::sync::mpsc::{channel, Receiver, Sender};

/// The declaration exactly as it goes on the wire, for the log.
///
/// A run is inspectable from the instant it is spawned, and this is the
/// client's half of that: the graph, the splices and the weights this app
/// actually sent, rather than a description of what it meant to send. A
/// stage the client itself refuses (a splice naming a stage it does not
/// depend on, say) prints its refusal in place.
pub fn declaration_json(
    namespace: &str,
    title: &str,
    prompt: &str,
    stages: &[PipelineStageSpec],
) -> String {
    let declared: Vec<Value> = stages
        .iter()
        .map(|stage| match stage.to_value() {
            Ok(value) => value,
            Err(error) => obj(vec![("refused", s(error.to_string()))]),
        })
        .collect();
    obj(vec![
        ("namespace", s(namespace)),
        ("title", s(title)),
        ("prompt", s(prompt)),
        ("stages", Value::Arr(declared)),
    ])
    .to_json()
}

/// Requests allowed to queue on the worker before DETAIL polls start being
/// dropped. A poll that is dropped costs nothing — the model re-issues it
/// on the next cadence — whereas a queue that grows without bound turns a
/// slow server into an ever-later record. Declares and cancels are never
/// dropped: they are the operator's own presses.
const MAX_QUEUED: usize = 12;

/// One transport request.
pub enum PipeReq {
    /// Declare the whole run. `tag` is the drawer row that asked.
    Create {
        tag: u64,
        namespace: String,
        title: String,
        prompt: String,
        stages: Vec<PipelineStageSpec>,
    },
    /// One read of the record — everything a row draws, in one request.
    Detail { pipeline: PipelineId },
    /// Stop every non-terminal stage of the run.
    Cancel { pipeline: PipelineId },
}

/// One transport answer. Errors are strings because the row shows them.
pub enum PipeDone {
    Created { tag: u64, result: Result<PipelineCreatedDto, String> },
    Detail { pipeline: PipelineId, result: Result<PipelineDetailDto, String> },
    Cancelled { pipeline: PipelineId, result: Result<PipelineCancelDto, String> },
}

/// Owns the worker thread and the completion channel; the host pumps it
/// each tick.
pub struct Pipelines {
    /// `None` until a session comes up (and again after a reconnect drops
    /// the old worker).
    tx: Option<Sender<PipeReq>>,
    done_tx: Sender<PipeDone>,
    done_rx: Receiver<PipeDone>,
    /// Requests handed to the worker that have not answered yet.
    queued: usize,
}

impl Default for Pipelines {
    fn default() -> Self {
        let (done_tx, done_rx) = channel();
        Pipelines { tx: None, done_tx, done_rx, queued: 0 }
    }
}

impl Pipelines {
    /// (Re)point the transport at a verified session. The previous worker
    /// exits when its current request returns — its answers still land on
    /// the same channel and still match by pipeline id, so a reconnect
    /// mid-run loses nothing.
    pub fn connect(&mut self, endpoints: ApiEndpoints, token: Option<String>) {
        let (tx, rx) = channel::<PipeReq>();
        let done = self.done_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("vj-pipelines".to_string())
            .spawn(move || worker(endpoints, token, rx, done));
        self.tx = spawned.is_ok().then_some(tx);
        self.queued = 0;
    }

    pub fn connected(&self) -> bool {
        self.tx.is_some()
    }

    /// Queue one request. Returns false when it could not be handed over —
    /// the caller decides what that means (a declare says so on the row; a
    /// poll simply happens next tick).
    pub fn submit(&mut self, req: PipeReq) -> bool {
        let droppable = matches!(req, PipeReq::Detail { .. });
        if droppable && self.queued >= MAX_QUEUED {
            return false;
        }
        let Some(tx) = self.tx.as_ref() else { return false };
        if tx.send(req).is_err() {
            self.tx = None;
            return false;
        }
        self.queued += 1;
        true
    }

    /// Everything that answered since the last call.
    pub fn drain(&mut self) -> Vec<PipeDone> {
        let mut out = Vec::new();
        while let Ok(done) = self.done_rx.try_recv() {
            self.queued = self.queued.saturating_sub(1);
            out.push(done);
        }
        out
    }
}

/// The worker loop. Ends when the sender is dropped (app exit, or a
/// reconnect replacing it).
fn worker(
    endpoints: ApiEndpoints,
    token: Option<String>,
    rx: Receiver<PipeReq>,
    done: Sender<PipeDone>,
) {
    // A client that cannot be built still ANSWERS: a row waiting for a
    // declare that never comes back is the one failure mode this transport
    // must not have.
    let api = Api::new(endpoints, HttpLimits::default_v1(), token).map_err(|e| e.to_string());
    for req in rx {
        let answer = match (&api, req) {
            (Ok(api), PipeReq::Create { tag, namespace, title, prompt, stages }) => {
                let result = api
                    .create_pipeline(&namespace, &title, &prompt, &stages)
                    .map_err(|e| e.to_string());
                PipeDone::Created { tag, result }
            }
            (Ok(api), PipeReq::Detail { pipeline }) => PipeDone::Detail {
                pipeline,
                result: api.pipeline_detail(&pipeline).map_err(|e| e.to_string()),
            },
            (Ok(api), PipeReq::Cancel { pipeline }) => PipeDone::Cancelled {
                pipeline,
                result: api.cancel_pipeline(&pipeline).map_err(|e| e.to_string()),
            },
            (Err(error), PipeReq::Create { tag, .. }) => {
                PipeDone::Created { tag, result: Err(error.clone()) }
            }
            (Err(error), PipeReq::Detail { pipeline }) => {
                PipeDone::Detail { pipeline, result: Err(error.clone()) }
            }
            (Err(error), PipeReq::Cancel { pipeline }) => {
                PipeDone::Cancelled { pipeline, result: Err(error.clone()) }
            }
        };
        if done.send(answer).is_err() {
            return; // the app is gone
        }
    }
}
