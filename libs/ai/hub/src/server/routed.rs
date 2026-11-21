//! Routed generation: a `/generate` with `"route": "fleet"` from an app on this machine. This
//! hub routes it the way `/talk` routes its stages ([`HubRouter`]) and keeps a job of its own
//! that a forwarder mirrors from the routed one, so the client needs only its own hub.
//!
//! The record takes no queue place and no class slot ([`crate::jobs::JobStore::insert_routed`]).
//! Each routed job in flight has a forwarder of its own, from a set of long-lived workers that
//! only grows: one slow node or artifact never holds up another job's lease renewals.

use super::*;
use crate::job::{process_origin, GenJob, GenRouter, HubRouter, JobPoll, Want};
use crate::registry::Domain;
use makepad_network::fleet::front::Caller;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

/// Between status reads of a routed job that has nothing new.
const POLL: Duration = Duration::from_millis(100);

/// The checks that answer 400 before any job exists, then the record, its lease and its
/// forwarder. The rest of the request is the picked node's to judge; its refusals end the job.
pub(super) fn submit(shared: &Arc<ServiceShared>, caller: &Caller, mut request: GenerateRequestJson) -> HttpServerResponse {
    if request.route.as_deref() != Some("fleet") {
        return error_json(400, format!("unknown route {:?} (expected \"fleet\")", request.route.as_deref().unwrap_or_default()));
    }
    // The job runs on other nodes under this hub's fleet credential: this machine's apps only.
    if !caller.this_machine {
        return error_json(403, "routed generation is for this machine's apps".to_string());
    }
    let Some(home) = shared.routing_home.get().cloned() else {
        return error_json(503, "this hub routes no jobs before it serves".to_string());
    };
    if let Err(e) = QueuePolicy::parse(request.queue_policy.as_deref()) {
        return error_json(400, e.to_string());
    }
    if let Some(other) = request.queue_class.as_deref().filter(|class| !matches!(*class, "interactive" | "batch")) {
        return error_json(400, format!("unknown queue_class {other:?} (expected \"interactive\" or \"batch\")"));
    }
    let known = shared.registry.find(&request.model).map(|spec| spec.domain);
    let domain = match (known, request.route_domain.as_deref()) {
        (Some(domain), None) => domain,
        (Some(domain), Some(named)) if named == domain.as_str() => domain,
        (Some(domain), Some(named)) => {
            return error_json(400, format!("route_domain {named:?} is not model {}'s domain ({})", request.model, domain.as_str()));
        }
        (None, Some(named)) => match Domain::parse(named) {
            Some(domain) => domain,
            None => return error_json(400, format!("unknown route_domain {named:?}")),
        },
        (None, None) => return error_json(400, "a routed request names its route_domain when this hub does not know its model".to_string()),
    };
    if let Some(problem) = crate::registry::request_problem(domain, &request) {
        return error_json(400, problem);
    }
    // The client's lease is on this hub's record; the forward leg holds a lease of its own.
    let origin = request.origin_key.take().map(|node_key| crate::lease::Origin {
        node_key: scoped_origin(caller, &node_key),
        epoch: request.origin_epoch.take().unwrap_or(0),
    });
    let (job_id, cancel) = shared.jobs.with(|store| {
        let job_id = store.insert_routed(&request.model);
        let cancel = store.cancel_token(&job_id).unwrap_or_default();
        (job_id, cancel)
    });
    crate::control::note_job(&job_id, &caller.client_id, caller.app.as_deref(), crate::control::model_kind(domain.as_str()));
    shared.job_owners.lock().unwrap().insert(job_id.clone(), caller.client_id.clone());
    if let Some(origin) = origin {
        let _ = shared.leases.lock().unwrap().register(&job_id, origin, crate::jobs::now_ms());
    }
    // The router's probes stop at a cancel (the record's token) or the hub's stop.
    let stop: Arc<dyn Fn() -> bool + Send + Sync> = {
        let shared = shared.clone();
        Arc::new(move || cancel.is_cancelled() || shared.stopped.load(Ordering::SeqCst))
    };
    let router = Arc::new(HubRouter { home, stop: stop.clone() });
    forwarders().run(Routed { shared: shared.clone(), job_id: job_id.clone(), domain, request, router, stop });
    ok_json(GenerateResponseJson { job_id: Some(job_id), error: None, think_open: None }.serialize_json())
}

struct Routed {
    shared: Arc<ServiceShared>,
    job_id: String,
    domain: Domain,
    request: GenerateRequestJson,
    router: Arc<dyn GenRouter>,
    /// The record was cancelled or the hub is stopping.
    stop: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Runs one routed job to its end and mirrors it into its record.
fn forward(routed: Routed) {
    let Routed { shared, job_id, domain, mut request, router, stop } = routed;
    // One hop: the node gets a plain request.
    request.route = None;
    request.route_domain = None;
    let memory_only = request.memory_only == Some(true);
    let mut job = GenJob::new(router, process_origin("routed"));
    if let Err(error) = job.start(domain, request, Want::All) {
        // A cancel's error is the cancel (`fail` turns into `cancelled` then).
        shared.jobs.with(|store| store.fail(&job_id, error));
        return;
    }
    let mut text = String::new();
    let mut pieces = 0;
    let mut shown: Option<(String, u16)> = None;
    loop {
        if stop() {
            if shared.jobs.with(|store| store.frees_on_cancel(&job_id)) { job.cancel_freeing() } else { job.cancel() }
            shared.jobs.with(|store| store.cancelled(&job_id));
            return;
        }
        match job.poll() {
            JobPoll::Pending => std::thread::sleep(POLL),
            JobPoll::Progress { permille, stage } => {
                if shown.as_ref().is_none_or(|(was, at)| *was != stage || *at != permille) {
                    shared.jobs.with(|store| store.set_progress(&job_id, &stage, permille as f64 / 1000.0));
                    shown = Some((stage, permille));
                }
                std::thread::sleep(POLL);
            }
            JobPoll::Delta(delta) => {
                text.push_str(&delta);
                shared.jobs.with(|store| store.set_partial_text(&job_id, text.clone()));
            }
            JobPoll::Chunk(piece) => {
                let name = format!("p{pieces}");
                match store_artifact(&shared, &job_id, &name, &piece.content_type, extension(&piece.content_type), piece.bytes, memory_only) {
                    Ok(stored) => {
                        pieces += 1;
                        shared.jobs.with(|store| store.push_partial_artifact(&job_id, stored));
                    }
                    Err(error) => {
                        job.cancel();
                        shared.jobs.with(|store| store.fail(&job_id, error.to_string()));
                        return;
                    }
                }
            }
            JobPoll::Done(output) => {
                let stored: Result<Vec<_>, _> = output.artifacts.into_iter().enumerate()
                    .map(|(index, artifact)| store_artifact(&shared, &job_id, &index.to_string(), &artifact.content_type, extension(&artifact.content_type), artifact.bytes, memory_only))
                    .collect();
                // The node's copy goes once this hub has its own.
                let _ = job.discard();
                let unwanted = shared.jobs.with(|store| match stored {
                    Ok(artifacts) if stop() => {
                        store.cancelled(&job_id);
                        artifacts
                    }
                    Ok(artifacts) => {
                        if let Some(text) = output.text {
                            store.set_partial_text(&job_id, text.clone());
                            store.set_text(&job_id, text);
                        }
                        store.set_model(&job_id, &output.model);
                        store.finish(&job_id, artifacts);
                        Vec::new()
                    }
                    Err(error) => {
                        store.fail(&job_id, error.to_string());
                        Vec::new()
                    }
                });
                // A cancel that came while the answer was copied wins, and its copy goes.
                let mut map = shared.artifacts.lock().unwrap();
                for artifact in unwanted {
                    if let Some(meta) = map.remove(&artifact.id).filter(|meta| meta.bytes.is_none()) {
                        let _ = fs::remove_file(&meta.path);
                    }
                }
                return;
            }
            JobPoll::Failed(error) => {
                job.cancel();
                shared.jobs.with(|store| store.fail(&job_id, error));
                return;
            }
        }
    }
}

/// The file name extension of a copied artifact (what is served is its content type).
fn extension(content_type: &str) -> &'static str {
    match content_type.split(';').next().unwrap_or_default().trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "audio/wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/midi" => "mid",
        "video/mp4" => "mp4",
        "model/gltf-binary" => "glb",
        "application/x-ply" => "ply",
        "application/json" => "json",
        "text/plain" => "txt",
        "text/vnd.abc" => "abc",
        _ => "bin",
    }
}

/// The forwarders: long-lived workers fed by one channel, one per routed job in flight. A job
/// takes an idle worker, or starts one more when none is idle; none ever ends, so the set only
/// grows to the most routed jobs that were ever in flight at once, and is reused.
struct Forwarders {
    idle: Mutex<usize>,
    jobs: mpsc::Sender<Routed>,
    queue: Arc<Mutex<mpsc::Receiver<Routed>>>,
}

fn forwarders() -> &'static Forwarders {
    static FORWARDERS: OnceLock<Forwarders> = OnceLock::new();
    FORWARDERS.get_or_init(|| {
        let (jobs, queue) = mpsc::channel();
        Forwarders { idle: Mutex::new(0), jobs, queue: Arc::new(Mutex::new(queue)) }
    })
}

impl Forwarders {
    fn run(&'static self, job: Routed) {
        {
            let mut idle = self.idle.lock().unwrap();
            if *idle > 0 {
                *idle -= 1;
            } else {
                let queue = self.queue.clone();
                let started = std::thread::Builder::new().name("ai-hub-routed".into()).spawn(move || loop {
                    let next = queue.lock().unwrap().recv();
                    let Ok(job) = next else { return };
                    let (shared, job_id) = (job.shared.clone(), job.job_id.clone());
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| forward(job))).is_err() {
                        shared.jobs.with(|store| store.fail(&job_id, "the routed job's forwarder failed".to_string()));
                    }
                    drop(shared);
                    *forwarders().idle.lock().unwrap() += 1;
                });
                if let Err(error) = started {
                    drop(idle);
                    job.shared.jobs.with(|store| store.fail(&job.job_id, format!("no forwarder for the routed job: {error}")));
                    return;
                }
            }
        }
        let _ = self.jobs.send(job);
    }
}
