//! Native BS-RoFormer backend: decoded 44.1 kHz stereo PCM in, four stems out.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::protocol::{encode_stems_artifact, StemsArtifact, STEMS_ARTIFACT_CONTENT_TYPE};
use makepad_ai_stems::{Demixer, StemSet, StemsModel, StereoBuf, SAMPLE_RATE};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

const MAX_TRACK_FRAMES: usize = SAMPLE_RATE as usize * 60 * 20;

pub struct StemsBackend {
    model_id: String,
    model_path: Option<PathBuf>,
    worker: Option<StemsWorker>,
}

impl StemsBackend {
    pub fn new(model_id: &str) -> Self {
        Self { model_id: model_id.to_string(), model_path: None, worker: None }
    }
}

enum WorkerCommand {
    Separate {
        track: StereoBuf,
        cancel: CancelToken,
        events: mpsc::Sender<WorkerEvent>,
    },
    Shutdown,
}

enum WorkerEvent {
    Progress(usize, usize),
    Done(Result<StemSet, WorkerError>),
}

enum WorkerError {
    Cancelled,
    Failed(String),
}

struct StemsWorker {
    commands: mpsc::Sender<WorkerCommand>,
    join: Option<thread::JoinHandle<()>>,
}

impl StemsWorker {
    fn spawn(path: PathBuf) -> Result<Self, AssetAiError> {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("makepad-stems".to_string())
            .spawn(move || {
                let mut model = match StemsModel::load(&path) {
                    Ok(model) => {
                        let _ = ready_tx.send(Ok(()));
                        model
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                while let Ok(command) = commands_rx.recv() {
                    match command {
                        WorkerCommand::Separate { track, cancel, events } => {
                            let result = separate(&mut model, &track, &cancel, &events);
                            let _ = events.send(WorkerEvent::Done(result));
                        }
                        WorkerCommand::Shutdown => break,
                    }
                }
            })
            .map_err(|error| AssetAiError::Backend(format!("stems worker spawn: {error}")))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { commands: commands_tx, join: Some(join) }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(AssetAiError::Backend(format!("stems load: {error}")))
            }
            Err(_) => {
                let _ = join.join();
                Err(AssetAiError::Backend(
                    "stems worker exited during model load".to_string(),
                ))
            }
        }
    }

    fn separate(
        &self,
        track: StereoBuf,
        cancel: &CancelToken,
    ) -> Result<mpsc::Receiver<WorkerEvent>, AssetAiError> {
        let (events_tx, events_rx) = mpsc::channel();
        self.commands
            .send(WorkerCommand::Separate {
                track,
                cancel: cancel.clone(),
                events: events_tx,
            })
            .map_err(|_| AssetAiError::Backend("stems worker is not running".to_string()))?;
        Ok(events_rx)
    }

    fn shutdown(&mut self) -> Result<(), AssetAiError> {
        let _ = self.commands.send(WorkerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| AssetAiError::Backend("stems worker panicked".to_string()))?;
        }
        Ok(())
    }
}

impl Drop for StemsWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(WorkerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn separate(
    model: &mut StemsModel,
    track: &StereoBuf,
    cancel: &CancelToken,
    events: &mpsc::Sender<WorkerEvent>,
) -> Result<StemSet, WorkerError> {
    let frames = track.left.len();
    let mut output: StemSet = std::array::from_fn(|_| StereoBuf::silence(frames));
    let mut demixer = Demixer::new(model, track)
        .map_err(|error| WorkerError::Failed(error.to_string()))?;
    let total = demixer.span_count();
    let mut done = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(WorkerError::Cancelled);
        }
        let Some(span) = demixer
            .next_span()
            .map_err(|error| WorkerError::Failed(error.to_string()))?
        else {
            break;
        };
        for stem in 0..output.len() {
            let end = (span.start + span.stems[stem].left.len()).min(frames);
            if span.start < end {
                let len = end - span.start;
                output[stem].left[span.start..end]
                    .copy_from_slice(&span.stems[stem].left[..len]);
                output[stem].right[span.start..end]
                    .copy_from_slice(&span.stems[stem].right[..len]);
            }
        }
        done += 1;
        let _ = events.send(WorkerEvent::Progress(done, total));
    }
    Ok(output)
}

impl ContentBackend for StemsBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        let path = ctx.path_by_role("weights")?;
        if self.worker.is_some() && self.model_path.as_ref() == Some(&path) {
            return Ok(());
        }
        ctx.cancel.check()?;
        (ctx.progress)("load BS-RoFormer checkpoint", 0.0);
        if let Some(mut worker) = self.worker.take() {
            worker.shutdown()?;
        }
        self.worker = Some(StemsWorker::spawn(path.clone())?);
        self.model_path = Some(path);
        ctx.cancel.check()?;
        (ctx.progress)("load BS-RoFormer checkpoint", 1.0);
        Ok(())
    }

    fn is_resident(&self) -> bool {
        self.worker.is_some()
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        if let Some(mut worker) = self.worker.take() {
            worker.shutdown()?;
        }
        self.model_path = None;
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        cancel.check()?;
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(
                "stems requires a 44.1 kHz stereo PCM WAV in input_b64".to_string(),
            ));
        }
        progress("decode 44.1 kHz stereo PCM", 0.01);
        let (left, right, rate) = crate::wav::decode_wav_to_stereo_f32(&params.input_bytes)
            .map_err(|error| AssetAiError::Params(format!("stems WAV: {error}")))?;
        if rate != SAMPLE_RATE {
            return Err(AssetAiError::Params(format!(
                "stems input rate is {rate} Hz; expected {SAMPLE_RATE} Hz"
            )));
        }
        if left.len() != right.len() || left.is_empty() || left.len() > MAX_TRACK_FRAMES {
            return Err(AssetAiError::Params(format!(
                "stems input frame count {} is outside 1..={MAX_TRACK_FRAMES}",
                left.len()
            )));
        }
        let worker = self.worker.as_ref().ok_or_else(|| {
            AssetAiError::Backend("stems generate called before ensure_loaded".to_string())
        })?;
        let events = worker.separate(StereoBuf { left, right }, cancel)?;
        let stems = loop {
            match events.recv() {
                Ok(WorkerEvent::Progress(done, total)) => {
                    let ratio = if total == 0 { 1.0 } else { done as f64 / total as f64 };
                    progress(&format!("separate stems {done}/{total}"), 0.02 + 0.95 * ratio);
                }
                Ok(WorkerEvent::Done(Ok(stems))) => break stems,
                Ok(WorkerEvent::Done(Err(WorkerError::Cancelled))) => {
                    return Err(AssetAiError::Cancelled)
                }
                Ok(WorkerEvent::Done(Err(WorkerError::Failed(error)))) => {
                    return Err(AssetAiError::Backend(format!("stems: {error}")))
                }
                Err(_) => {
                    return Err(AssetAiError::Backend(
                        "stems worker disconnected during separation".to_string(),
                    ))
                }
            }
        };
        cancel.check()?;
        progress("serialize stems", 0.98);
        let [drums, bass, other, vocals] = stems;
        let frames = drums.left.len();
        let artifact = StemsArtifact {
            sample_rate: SAMPLE_RATE,
            frames,
            channels: [
                drums.left,
                drums.right,
                bass.left,
                bass.right,
                other.left,
                other.right,
                vocals.left,
                vocals.right,
            ],
        };
        let bytes = encode_stems_artifact(&artifact).map_err(AssetAiError::Backend)?;
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: STEMS_ARTIFACT_CONTENT_TYPE,
            ext: "mpst",
            bytes,
        }])
    }
}

