//! Native Beat This! backend: audio bytes in, beat/downbeat JSON out.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use makepad_ai_beats::{BeatAnalysis, BeatsModel, SAMPLE_RATE};
use makepad_ai_common::DiffusionError;
use makepad_audio_decode::{decode_audio_limited, sniff as sniff_audio, Limits};
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

const MAX_DECODE_FRAMES: usize = 192_000 * 60 * 120;

pub struct BeatsBackend {
    model_id: String,
    model_path: Option<PathBuf>,
    worker: Option<BeatsWorker>,
}

impl BeatsBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            model_path: None,
            worker: None,
        }
    }
}

enum WorkerCommand {
    Analyze {
        mono: Vec<f32>,
        cancel: CancelToken,
        events: mpsc::Sender<WorkerEvent>,
    },
    Shutdown,
}

enum WorkerEvent {
    Progress(usize, usize),
    Done(Result<BeatAnalysis, WorkerError>),
}

enum WorkerError {
    Cancelled,
    Failed(String),
}

struct BeatsWorker {
    commands: mpsc::Sender<WorkerCommand>,
    join: Option<thread::JoinHandle<()>>,
}

impl BeatsWorker {
    fn spawn(path: PathBuf) -> Result<Self, AssetAiError> {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("makepad-beats".to_string())
            .spawn(move || {
                let mut model = match BeatsModel::load(&path) {
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
                        WorkerCommand::Analyze {
                            mono,
                            cancel,
                            events,
                        } => {
                            let mut hook = |done: usize, total: usize| {
                                if cancel.is_cancelled() {
                                    return Err(DiffusionError::Cancelled);
                                }
                                events.send(WorkerEvent::Progress(done, total)).map_err(|_| {
                                    DiffusionError::model("beats progress receiver disconnected")
                                })?;
                                Ok(())
                            };
                            let result = model.analyze_with_progress(&mono, &mut hook).map_err(
                                |error| match error {
                                    DiffusionError::Cancelled => WorkerError::Cancelled,
                                    other => WorkerError::Failed(other.to_string()),
                                },
                            );
                            let _ = events.send(WorkerEvent::Done(result));
                        }
                        WorkerCommand::Shutdown => break,
                    }
                }
            })
            .map_err(|error| AssetAiError::Backend(format!("beats worker spawn: {error}")))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                commands: commands_tx,
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(AssetAiError::Backend(format!("beats load: {error}")))
            }
            Err(_) => {
                let _ = join.join();
                Err(AssetAiError::Backend(
                    "beats worker exited during model load".to_string(),
                ))
            }
        }
    }

    fn analyze(&self, mono: Vec<f32>, cancel: &CancelToken) -> Result<mpsc::Receiver<WorkerEvent>, AssetAiError> {
        let (events_tx, events_rx) = mpsc::channel();
        self.commands
            .send(WorkerCommand::Analyze {
                mono,
                cancel: cancel.clone(),
                events: events_tx,
            })
            .map_err(|_| AssetAiError::Backend("beats worker is not running".to_string()))?;
        Ok(events_rx)
    }

    fn shutdown(&mut self) -> Result<(), AssetAiError> {
        let _ = self.commands.send(WorkerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| AssetAiError::Backend("beats worker panicked".to_string()))?;
        }
        Ok(())
    }
}

impl Drop for BeatsWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(WorkerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl ContentBackend for BeatsBackend {
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
        (ctx.progress)("load beat-this checkpoint", 0.0);
        if let Some(mut worker) = self.worker.take() {
            worker.shutdown()?;
        }
        let worker = BeatsWorker::spawn(path.clone())?;
        ctx.cancel.check()?;
        self.model_path = Some(path);
        self.worker = Some(worker);
        (ctx.progress)("load beat-this checkpoint", 1.0);
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
                "beats requires an audio file in input_b64".to_string(),
            ));
        }
        progress("decode audio", 0.01);
        let (mono, input_rate) = decode_audio(&params.input_bytes, &params.input_content_type)?;
        cancel.check()?;
        let mono = if input_rate == SAMPLE_RATE {
            mono
        } else {
            progress("resample audio to 22050 Hz", 0.04);
            crate::resample::resample_channel(&mono, input_rate, SAMPLE_RATE)
        };
        cancel.check()?;
        progress("log-mel frontend", 0.07);
        let worker = self.worker.as_ref().ok_or_else(|| {
            AssetAiError::Backend("beats generate called before ensure_loaded".to_string())
        })?;
        let events = worker.analyze(mono, cancel)?;
        let analysis = loop {
            match events.recv() {
                Ok(WorkerEvent::Progress(done, total)) => {
                    let ratio = if total == 0 {
                        1.0
                    } else {
                        done as f64 / total as f64
                    };
                    progress(
                        &format!("beat inference {done}/{total}"),
                        0.10 + 0.88 * ratio,
                    );
                }
                Ok(WorkerEvent::Done(Ok(analysis))) => break analysis,
                Ok(WorkerEvent::Done(Err(WorkerError::Cancelled))) => {
                    return Err(AssetAiError::Cancelled)
                }
                Ok(WorkerEvent::Done(Err(WorkerError::Failed(error)))) => {
                    return Err(AssetAiError::Backend(format!("beats: {error}")))
                }
                Err(_) => {
                    return Err(AssetAiError::Backend(
                        "beats worker disconnected during inference".to_string(),
                    ))
                }
            }
        };
        cancel.check()?;
        progress("serialize beat analysis", 0.99);
        let bytes = analysis_json(&analysis).into_bytes();
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "application/json",
            ext: "json",
            bytes,
        }])
    }
}

fn decode_audio(bytes: &[u8], declared: &str) -> Result<(Vec<f32>, u32), AssetAiError> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return crate::wav::decode_wav_to_mono_f32(bytes)
            .map_err(|error| AssetAiError::Params(format!("beats wav: {error}")));
    }
    let format = sniff_audio(bytes).ok_or_else(|| {
        AssetAiError::Params(format!(
            "beats input is not a supported audio file (declared {declared:?}); send WAV, MP3, FLAC or Ogg Vorbis"
        ))
    })?;
    let audio = decode_audio_limited(bytes, format, Limits::with_max_frames(MAX_DECODE_FRAMES))
        .map_err(|error| AssetAiError::Params(format!("beats audio decode: {error}")))?;
    if audio.rate == 0 || audio.channels == 0 || audio.frames() == 0 {
        return Err(AssetAiError::Params("beats audio is empty".to_string()));
    }
    let channels = audio.channels as usize;
    let mut mono = Vec::with_capacity(audio.frames());
    for frame in audio.pcm_interleaved_f32.chunks_exact(channels) {
        mono.push(frame.iter().copied().sum::<f32>() / channels as f32);
    }
    Ok((mono, audio.rate))
}

fn analysis_json(analysis: &BeatAnalysis) -> String {
    let mut output = String::with_capacity(
        128 + (analysis.beats_secs.len() + analysis.downbeats_secs.len()) * 12,
    );
    write!(
        output,
        "{{\"bpm\":{},\"confidence\":{},\"beats\":[",
        analysis.bpm, analysis.confidence
    )
    .unwrap();
    write_numbers(&mut output, &analysis.beats_secs);
    output.push_str("],\"downbeats\":[");
    write_numbers(&mut output, &analysis.downbeats_secs);
    write!(output, "],\"frame_rate\":{}}}", analysis.frame_rate).unwrap();
    output
}

fn write_numbers(output: &mut String, values: &[f64]) {
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        write!(output, "{value}").unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_contract_has_only_public_summary_fields() {
        let analysis = BeatAnalysis {
            beats_secs: vec![0.5, 1.0],
            downbeats_secs: vec![0.5],
            bpm: 120.0,
            confidence: 0.875,
            frame_rate: 50.0,
            beat_prob: vec![0.1],
            downbeat_prob: vec![0.2],
        };
        assert_eq!(
            analysis_json(&analysis),
            "{\"bpm\":120,\"confidence\":0.875,\"beats\":[0.5,1],\"downbeats\":[0.5],\"frame_rate\":50}"
        );
    }

    #[test]
    fn wav_decode_downmixes_channels() {
        let wav = crate::wav::encode_wav_pcm16_stereo(&[0.5, -0.5], &[-0.5, -0.5], 44_100);
        let (mono, rate) = decode_audio(&wav, "audio/wav").unwrap();
        assert_eq!(rate, 44_100);
        assert!(mono[0].abs() < 1e-3);
        assert!((mono[1] + 0.5).abs() < 1e-3);
    }
}
