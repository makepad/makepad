//! One blocking transport operation per pool task. Poll delays belong to the
//! controller, never to a pool worker. An accepted POST is never replayed.
use super::*;
use makepad_ai_hub::client::ContentProvider;
use makepad_ai_hub::protocol::{
    ArtifactRefJson, GenerateRequestJson, JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR,
};
use makepad_ai_hub::registry::Domain;
use makepad_widgets::makepad_platform::thread::{Lane, SubmitError, TaskHandle};

pub(super) type Picker = Arc<dyn Fn(&GenerateRequestJson)
    -> Result<(String, Box<dyn ContentProvider + Send>), String> + Send + Sync>;

pub(super) fn fleet_picker() -> Picker {
    Arc::new(|request| {
        let (service, _) = makepad_asset_creator::runner::pick_node_for_request("stems", request)
            .map_err(|error| error.to_string())?;
        Ok((service.base_url().to_string(), Box::new(service)))
    })
}

struct Accepted {
    service: Box<dyn ContentProvider + Send>,
    node: String,
    id: String,
}

impl Accepted {
    fn cancel(&self) {
        let _ = self.service.cancel(&self.id);
    }

    fn status(&self, output: &JobOutput, stage: &str) {
        output.status(format!("stems: hub {} · {} · {}", self.node, self.id, bounded_reason(stage)), true);
    }
}

enum Step {
    Prepare,
    Submit { request: GenerateRequestJson, node: String, service: Box<dyn ContentProvider + Send> },
    Poll(Accepted),
    Fetch(Accepted, ArtifactRefJson),
    Install(Vec<u8>),
    Finished,
}

impl Step {
    fn run(self, work: &Work, picker: &Picker) -> Result<Self, String> {
        let output = &work.output;
        // Cancellation is deliberately ours, not TaskHandle::cancel(): an
        // accepted job still owes the server a cancel, even if queued locally.
        if work.cancelled() {
            match &self {
                Self::Poll(remote) | Self::Fetch(remote, _) => remote.cancel(),
                _ => {}
            }
            return Ok(Self::Finished);
        }
        match self {
            Self::Prepare => {
                output.status("stems: preparing upload".into(), true);
                let track = to_stereo_buf(&work.job.pcm);
                let wav = makepad_ai_hub::wav::encode_wav_pcm16_stereo(&track.left, &track.right, STEMS_RATE);
                let input_b64 = String::from_utf8(makepad_ai_hub::makepad_base64::base64_encode(
                    &wav, &makepad_ai_hub::makepad_base64::BASE64_STANDARD,
                )).map_err(|error| error.to_string())?;
                let request = GenerateRequestJson {
                    model: makepad_ai_stems::MODEL_ID.into(),
                    input_b64: Some(input_b64),
                    input_content_type: Some("audio/wav".into()),
                    ..Default::default()
                };
                if work.cancelled() { return Ok(Self::Finished); }
                // Selection sees the exact model AND the actual audio payload/cost.
                let (node, service) = picker(&request)?;
                Ok(Self::Submit { request, node, service })
            }
            Self::Submit { request, node, service } => {
                output.status(format!("stems: uploading to {node}"), true);
                if work.cancelled() { return Ok(Self::Finished); }
                // Only proven pre-admission refusals may wait; cancellation stops retries.
                // Accepted jobs and ambiguous POST outcomes are never replayed.
                let id = service.request_pending(Domain::Stems, &request, &|| work.cancelled(), &mut |note| output.status(format!("stems: {}", bounded_reason(note)), true)).map_err(|error| error.to_string())?;
                let remote = Accepted { service, node, id };
                if work.cancelled() {
                    remote.cancel();
                    return Ok(Self::Finished);
                }
                remote.status(output, "accepted / queued");
                Ok(Self::Poll(remote))
            }
            Self::Poll(remote) => {
                let status = match remote.service.poll(&remote.id) {
                    Ok(status) => status,
                    Err(error) => {
                        remote.cancel();
                        return Err(format!("hub {} · {}: {error}", remote.node, remote.id));
                    }
                };
                if work.cancelled() {
                    remote.cancel();
                    return Ok(Self::Finished);
                }
                match status.state.as_str() {
                    JOB_STATE_DONE => {
                        let artifact = status.artifacts.first().cloned()
                            .ok_or_else(|| "hub stem job finished without an artifact".to_string())?;
                        Ok(Self::Fetch(remote, artifact))
                    }
                    JOB_STATE_ERROR => Err(format!("hub {} · {}: {}", remote.node, remote.id,
                        status.error.unwrap_or_else(|| "stem job failed".into()))),
                    JOB_STATE_CANCELLED => Err(format!("hub {} · {}: cancelled", remote.node, remote.id)),
                    _ => {
                        remote.status(output, status.stage.as_deref().unwrap_or(&status.state));
                        Ok(Self::Poll(remote))
                    }
                }
            }
            Self::Fetch(remote, artifact) => {
                remote.status(output, "fetching artifact");
                let fetched = remote.service.fetch_artifact(&artifact.id)
                    .map_err(|error| format!("hub artifact: {error}"))?;
                if work.cancelled() { return Ok(Self::Finished); }
                makepad_ai_hub::client::verify_artifact_bytes(&fetched.bytes, &artifact)
                    .map_err(|error| error.to_string())?;
                Ok(Self::Install(fetched.bytes))
            }
            step => Ok(step),
        }
    }
}

pub(super) struct Remote {
    pub work: Arc<Work>,
    step: Option<Step>,
    task: Option<TaskHandle<Result<Step, String>>>,
    ready_at: f64,
}

impl Remote {
    pub fn new(work: Work) -> Self {
        Self { work: Arc::new(work), step: Some(Step::Prepare), task: None, ready_at: 0.0 }
    }

    /// Returns true once settled. All cache access stays on the controller.
    pub fn pump(&mut self, pool: &TaskPool, picker: &Picker, root: &Path) -> bool {
        if let Some(task) = &mut self.task {
            let Some(result) = task.try_take() else { return false };
            self.task = None;
            match result {
                Ok(Ok(step)) => {
                    self.ready_at = if matches!(step, Step::Poll(_)) { Cx::monotonic_now() + 0.5 } else { 0.0 };
                    self.step = Some(step);
                }
                error => {
                    let reason = match error {
                        Ok(Err(error)) => error,
                        Err(error) => format!("hub worker: {error}"),
                        _ => unreachable!(),
                    };
                    self.work.output.status(format!("stems: {}", bounded_reason(&reason)), false);
                    return true;
                }
            }
        }
        if matches!(self.step, Some(Step::Finished)) { return true; }
        if self.work.cancelled() && matches!(self.step, Some(Step::Prepare | Step::Submit { .. })) {
            return true;
        }
        if let Some(Step::Install(_)) = &self.step {
            if let Some(Step::Install(bytes)) = self.step.take() {
                if !self.work.cancelled() {
                    self.work.output.status("stems: installing artifact".into(), true);
                    if let Err(error) = install_hub_artifact(
                        &self.work.job, root, self.work.digest.as_deref().unwrap(), &bytes,
                        &self.work.output, &|| self.work.cancelled(),
                    ) {
                        self.work.output.status(format!("stems: {}", bounded_reason(&error)), false);
                    }
                }
            }
            return true;
        }
        if !self.work.cancelled() && Cx::monotonic_now() < self.ready_at { return false; }
        // Reserve before taking the step. Refusal leaves even an accepted
        // poll/cancel intact for the next pump; it can never become a new POST.
        let slot = match pool.reserve(if matches!(self.step, Some(Step::Prepare | Step::Fetch(..))) { Lane::Heavy } else { Lane::Light }) {
            Ok(slot) => slot,
            Err(SubmitError::Closed) => {
                // Pool shutdown cannot execute follow-ups. Cancel is best effort;
                // the app controller is itself a worker, so use it for this final
                // cleanup only. Never post, poll or fetch through this fallback.
                if let Some(Step::Poll(remote) | Step::Fetch(remote, _)) = &self.step { remote.cancel(); }
                self.work.output.status("stems: task pool closed".into(), false);
                return true;
            }
            Err(_) => return false,
        };
        let step = self.step.take().expect("remote step owned by controller");
        let work = self.work.clone();
        let picker = picker.clone();
        self.task = Some(slot.submit(move || step.run(&work, &picker)));
        false
    }
}
