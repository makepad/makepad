//! Canonical pure-Rust/CUDA SkinTokens rig backend.
//!
//! The official Lightning checkpoint is downloaded and converted through the
//! shared artifact lifecycle, then a dedicated worker owns all thread-local
//! CUDA caches. The old Torch/bpy implementation is exposed only as the
//! separately named `rig-oracle` backend and is never a fallback.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::download::ensure_converted_file;
use crate::error::AssetAiError;
use crate::rig_backend::check_rig_output;
use makepad_diffusion::skin_tokens_convert::convert_skin_tokens_checkpoint;
use makepad_diffusion::skin_tokens_pipeline::{
    unload_skin_tokens_runtime_weights, SkinTokensPipeline, SkinTokensPipelineParams,
};
use makepad_diffusion::DiffusionError;
use makepad_gltf::parse_glb_bytes;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

pub const SKIN_TOKENS_NATIVE_BACKEND: &str = "rig-native";
pub const SKIN_TOKENS_NATIVE_MODEL: &str = "skintokens";
pub const ROLE_TOKENRIG_CHECKPOINT: &str = "tokenrig-checkpoint";

fn map_diffusion(error: DiffusionError) -> WorkerError {
    match error {
        DiffusionError::Cancelled => WorkerError::Cancelled,
        other => WorkerError::Other(other.to_string()),
    }
}

fn map_conversion(error: DiffusionError) -> AssetAiError {
    match error {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Download(format!("convert SkinTokens checkpoint: {other}")),
    }
}

fn validate_input_glb(bytes: &[u8]) -> Result<(), AssetAiError> {
    let parsed = parse_glb_bytes(bytes)
        .map_err(|error| AssetAiError::Params(format!("input_b64 is not a valid GLB: {error}")))?;
    if parsed.document.meshes_slice().is_empty() {
        return Err(AssetAiError::Params(
            "input GLB contains no mesh to rig".to_string(),
        ));
    }
    Ok(())
}

pub struct RigNativeBackend {
    model_id: String,
    worker: Option<RigWorker>,
    checkpoint: Option<PathBuf>,
}

impl RigNativeBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            worker: None,
            checkpoint: None,
        }
    }
}

impl ContentBackend for RigNativeBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn prepare_artifacts(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        let file = ctx.file_by_role(ROLE_TOKENRIG_CHECKPOINT)?.clone();
        let cache_dir = ctx.cache_dir.to_path_buf();
        let cancel = ctx.cancel.clone();
        let progress = &mut *ctx.progress;
        ensure_converted_file(&file, &cache_dir, &cancel, |source, output| {
            let mut conversion_progress = |stage: &str, fraction: f64| {
                if cancel.is_cancelled() {
                    return Err(DiffusionError::Cancelled);
                }
                progress(stage, fraction);
                Ok(())
            };
            convert_skin_tokens_checkpoint(source, output, Some(&mut conversion_progress))
                .map(|_| ())
                .map_err(map_conversion)
        })?;
        Ok(())
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.cancel.check()?;
        let checkpoint = ctx.path_by_role(ROLE_TOKENRIG_CHECKPOINT)?;
        if self.checkpoint.as_ref() == Some(&checkpoint)
            && self.worker.as_ref().is_some_and(RigWorker::is_alive)
        {
            return Ok(());
        }
        if self.worker.is_some() {
            self.unload()?;
        }
        (ctx.progress)("rig load: checkpoint", 0.05);
        let worker = RigWorker::spawn(&checkpoint, ctx.cancel.clone(), ctx.progress)?;
        self.worker = Some(worker);
        self.checkpoint = Some(checkpoint);
        (ctx.progress)("rig load: resident", 1.0);
        Ok(())
    }

    fn is_resident(&self) -> bool {
        self.worker.as_ref().is_some_and(RigWorker::is_alive)
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        if let Some(worker) = self.worker.as_mut() {
            worker.shutdown()?;
        }
        self.worker = None;
        self.checkpoint = None;
        Ok(())
    }

    fn resident_is_healthy_after_error(&self, error: &AssetAiError) -> bool {
        // Input validation happens before submission. Native cancellation
        // unwinds only at declared seams and keeps immutable resident weights.
        matches!(error, AssetAiError::Cancelled | AssetAiError::Params(_))
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(format!(
                "{} needs an input mesh (input_b64 GLB)",
                self.model_id,
            )));
        }
        validate_input_glb(&params.input_bytes)?;
        cancel.check()?;
        let worker = self.worker.as_ref().ok_or_else(|| {
            AssetAiError::Backend("native SkinTokens used before ensure_loaded".to_string())
        })?;
        let output = match worker.generate(
            params.input_bytes.clone(),
            params.seed,
            cancel.clone(),
            progress,
        ) {
            Ok(output) => output,
            Err(WorkerError::Cancelled) => return Err(AssetAiError::Cancelled),
            Err(WorkerError::Other(message)) => {
                return Err(AssetAiError::Backend(format!(
                    "native SkinTokens: {message}"
                )))
            }
            Err(WorkerError::WorkerGone(message)) => {
                self.worker = None;
                self.checkpoint = None;
                return Err(AssetAiError::Backend(format!(
                    "native SkinTokens: {message}"
                )));
            }
        };
        cancel.check()?;
        check_rig_output(&output)?;
        Ok(vec![ArtifactData {
            content_type: "model/gltf-binary",
            ext: "glb",
            bytes: output,
        }])
    }
}

struct RigWorker {
    tx: Option<mpsc::Sender<WorkerMessage>>,
    join: Option<JoinHandle<()>>,
}

enum WorkerCommand {
    Generate { input: Vec<u8>, seed: u64 },
    Ping,
    Shutdown,
}

struct WorkerMessage {
    command: WorkerCommand,
    cancel: CancelToken,
    events: mpsc::Sender<WorkerEvent>,
}

enum WorkerEvent {
    Progress(String, f64),
    Ready(Result<(), WorkerError>),
    Done(Result<Vec<u8>, WorkerError>),
}

#[derive(Debug)]
enum WorkerError {
    Cancelled,
    Other(String),
    WorkerGone(String),
}

impl RigWorker {
    fn spawn(
        checkpoint: &Path,
        cancel: CancelToken,
        progress: ProgressSink,
    ) -> Result<Self, AssetAiError> {
        let (tx, rx) = mpsc::channel::<WorkerMessage>();
        let (ready_tx, ready_rx) = mpsc::channel::<WorkerEvent>();
        let checkpoint = checkpoint.to_path_buf();
        let join = std::thread::Builder::new()
            .name("skin-tokens-native".to_string())
            .spawn(move || {
                if cancel.is_cancelled() {
                    let _ = ready_tx.send(WorkerEvent::Ready(Err(WorkerError::Cancelled)));
                    return;
                }
                let pipeline = SkinTokensPipeline::load(&checkpoint).map_err(map_diffusion);
                let pipeline = match pipeline {
                    Ok(pipeline) => {
                        let _ = ready_tx.send(WorkerEvent::Ready(Ok(())));
                        pipeline
                    }
                    Err(error) => {
                        let _ = ready_tx.send(WorkerEvent::Ready(Err(error)));
                        return;
                    }
                };
                drop(ready_tx);
                let mut shutdown_reply = None;
                while let Ok(message) = rx.recv() {
                    match message.command {
                        WorkerCommand::Ping => {
                            let _ = message.events.send(WorkerEvent::Done(Ok(Vec::new())));
                        }
                        WorkerCommand::Generate { input, seed } => {
                            let cancelled = || message.cancel.is_cancelled();
                            let mut on_progress = |stage: &str, fraction: f64| {
                                if message.cancel.is_cancelled() {
                                    return Err(DiffusionError::Cancelled);
                                }
                                let _ = message
                                    .events
                                    .send(WorkerEvent::Progress(stage.to_string(), fraction));
                                Ok(())
                            };
                            let result = pipeline
                                .rig_glb(
                                    &input,
                                    &SkinTokensPipelineParams {
                                        seed,
                                        ..Default::default()
                                    },
                                    Some(&cancelled),
                                    Some(&mut on_progress),
                                )
                                .map(|output| output.glb)
                                .map_err(map_diffusion);
                            let _ = message.events.send(WorkerEvent::Done(result));
                        }
                        WorkerCommand::Shutdown => {
                            shutdown_reply = Some(message.events);
                            break;
                        }
                    }
                }
                drop(pipeline);
                let unload = unload_skin_tokens_runtime_weights().map_err(map_diffusion);
                makepad_diffusion::backend::gpu_pool_clear();
                if let Some(events) = shutdown_reply {
                    let _ = events.send(WorkerEvent::Done(unload.map(|_| Vec::new())));
                }
            })
            .map_err(|error| AssetAiError::Backend(format!("spawn SkinTokens worker: {error}")))?;
        loop {
            match ready_rx.recv() {
                Ok(WorkerEvent::Progress(stage, fraction)) => progress(&stage, fraction),
                Ok(WorkerEvent::Ready(Ok(()))) => {
                    return Ok(Self {
                        tx: Some(tx),
                        join: Some(join),
                    })
                }
                Ok(WorkerEvent::Ready(Err(WorkerError::Cancelled))) => {
                    return Err(AssetAiError::Cancelled)
                }
                Ok(WorkerEvent::Ready(Err(error))) => {
                    return Err(AssetAiError::Backend(format!(
                        "load native SkinTokens: {error:?}"
                    )))
                }
                Ok(WorkerEvent::Done(_)) => continue,
                Err(_) => {
                    return Err(AssetAiError::Backend(
                        "SkinTokens worker exited during load".to_string(),
                    ))
                }
            }
        }
    }

    fn is_alive(&self) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        let (events, replies) = mpsc::channel();
        tx.send(WorkerMessage {
            command: WorkerCommand::Ping,
            cancel: CancelToken::new(),
            events,
        })
        .is_ok()
            && replies.recv_timeout(Duration::from_secs(2)).is_ok()
    }

    fn generate(
        &self,
        input: Vec<u8>,
        seed: u64,
        cancel: CancelToken,
        progress: ProgressSink,
    ) -> Result<Vec<u8>, WorkerError> {
        let (events, replies) = mpsc::channel();
        self.tx
            .as_ref()
            .ok_or_else(|| WorkerError::WorkerGone("worker is shut down".to_string()))?
            .send(WorkerMessage {
                command: WorkerCommand::Generate { input, seed },
                cancel,
                events,
            })
            .map_err(|_| WorkerError::WorkerGone("worker channel is gone".to_string()))?;
        loop {
            match replies.recv() {
                Ok(WorkerEvent::Progress(stage, fraction)) => progress(&stage, fraction),
                Ok(WorkerEvent::Done(result)) => return result,
                Ok(WorkerEvent::Ready(_)) => continue,
                Err(_) => {
                    return Err(WorkerError::WorkerGone(
                        "worker dropped its generation reply".to_string(),
                    ))
                }
            }
        }
    }

    fn shutdown(&mut self) -> Result<(), AssetAiError> {
        let Some(tx) = self.tx.take() else {
            return Ok(());
        };
        let (events, replies) = mpsc::channel();
        let sent = tx
            .send(WorkerMessage {
                command: WorkerCommand::Shutdown,
                cancel: CancelToken::new(),
                events,
            })
            .is_ok();
        drop(tx);
        if sent {
            match replies.recv_timeout(Duration::from_secs(120)) {
                Ok(WorkerEvent::Done(Ok(_))) => {}
                Ok(WorkerEvent::Done(Err(error))) => {
                    return Err(AssetAiError::Backend(format!(
                        "SkinTokens unload failed: {error:?}"
                    )))
                }
                Ok(_) => {
                    return Err(AssetAiError::Backend(
                        "SkinTokens unload received an invalid acknowledgement".to_string(),
                    ))
                }
                Err(error) => {
                    return Err(AssetAiError::Backend(format!(
                        "SkinTokens unload acknowledgement timed out: {error}"
                    )))
                }
            }
        }
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| {
                AssetAiError::Backend("SkinTokens worker panicked during unload".to_string())
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_identity_and_role_are_stable() {
        assert_eq!(SKIN_TOKENS_NATIVE_BACKEND, "rig-native");
        assert_eq!(SKIN_TOKENS_NATIVE_MODEL, "skintokens");
        assert_eq!(ROLE_TOKENRIG_CHECKPOINT, "tokenrig-checkpoint");
    }
}
