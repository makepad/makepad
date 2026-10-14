//! Native BS-RoFormer backend: a song in, four stems out.
//!
//! Input (`input_b64`): any audio file the in-repo decoders read (WAV, MP3,
//! Ogg Vorbis, FLAC; sniffed from the bytes), so a library sends the track
//! it already stores instead of a decoded WAV several times its size. Mono
//! plays to both ears; another rate is resampled to the model's 44.1 kHz.
//!
//! Output, by `codec`:
//! - absent: one [`STEMS_ARTIFACT_CONTENT_TYPE`] bundle, planar stereo f32
//!   (exact samples, for a pipeline that keeps working on them);
//! - `"ogg"`: four `audio/ogg` Vorbis artifacts in model order (drums, bass,
//!   other, vocals) at [`OGG_STEM_QUALITY`], the form a library stores
//!   (about 4% of the f32 bundle's bytes).

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
/// The decode bound before resampling: twenty minutes at 48 kHz.
const MAX_DECODED_FRAMES: usize = 48_000 * 60 * 20;

/// Vorbis quality of an Ogg stem: transparent on separated material (a
/// stem is a sum of artifacts of its own, and a lower setting stacks more),
/// the setting a library's stem side channels are stored at.
pub const OGG_STEM_QUALITY: f32 = 0.85;

/// What a stems job hands back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StemsOutput {
    Bundle,
    Ogg,
}

impl StemsOutput {
    fn of(codec: &str) -> Result<StemsOutput, AssetAiError> {
        match codec.trim().to_ascii_lowercase().as_str() {
            "" => Ok(StemsOutput::Bundle),
            "ogg" | "vorbis" => Ok(StemsOutput::Ogg),
            other => Err(AssetAiError::Params(format!("stems codec {other:?}: the stems are the f32 bundle (no codec) or \"ogg\""))),
        }
    }
}

/// The request's song as the model's 44.1 kHz stereo.
fn decode_track(bytes: &[u8]) -> Result<StereoBuf, AssetAiError> {
    use makepad_audio_decode::{decode_audio_limited, sniff, Limits};
    let format = sniff(bytes)
        .ok_or_else(|| AssetAiError::Params("stems input is not audio the hub reads (WAV, MP3, Ogg Vorbis or FLAC)".to_string()))?;
    let audio = decode_audio_limited(bytes, format, Limits::with_max_frames(MAX_DECODED_FRAMES))
        .map_err(|error| AssetAiError::Params(format!("stems input: {error}")))?;
    if audio.channels == 0 || audio.rate == 0 || audio.frames() == 0 {
        return Err(AssetAiError::Params("stems input holds no audio".to_string()));
    }
    let left = audio.channel(0);
    let right = if audio.channels > 1 { audio.channel(1) } else { left.clone() };
    drop(audio.pcm_interleaved_f32);
    let (left, right) = if audio.rate == SAMPLE_RATE {
        (left, right)
    } else {
        (
            crate::resample::resample_channel(&left, audio.rate, SAMPLE_RATE),
            crate::resample::resample_channel(&right, audio.rate, SAMPLE_RATE),
        )
    };
    if left.len() != right.len() || left.is_empty() || left.len() > MAX_TRACK_FRAMES {
        return Err(AssetAiError::Params(format!(
            "stems input frame count {} is outside 1..={MAX_TRACK_FRAMES}",
            left.len()
        )));
    }
    Ok(StereoBuf { left, right })
}

/// Four Ogg Vorbis stems in model order, one encoder thread each.
fn encode_ogg_stems(stems: &StemSet) -> Result<Vec<ArtifactData>, AssetAiError> {
    let options = makepad_audio_encode::EncodeOptions { quality: OGG_STEM_QUALITY, threads: 1, tags: Vec::new() };
    let encoded: Vec<Result<Vec<u8>, String>> = thread::scope(|scope| {
        let handles: Vec<_> = stems
            .iter()
            .map(|stem| {
                let options = options.clone();
                scope.spawn(move || {
                    let mut pcm = Vec::with_capacity(stem.left.len() * 2);
                    for (left, right) in stem.left.iter().zip(&stem.right) {
                        pcm.push(*left);
                        pcm.push(*right);
                    }
                    makepad_audio_encode::encode_vorbis(SAMPLE_RATE, 2, &pcm, &options).map_err(|error| format!("{error:?}"))
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap_or_else(|_| Err("encoder panicked".to_string()))).collect()
    });
    encoded
        .into_iter()
        .map(|bytes| {
            bytes
                .map(|bytes| ArtifactData { content_type: "audio/ogg", ext: "ogg", bytes })
                .map_err(|error| AssetAiError::Backend(format!("stem encode: {error}")))
        })
        .collect()
}

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
        let output = StemsOutput::of(&params.codec)?;
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(
                "stems requires the song as audio (WAV, MP3, Ogg Vorbis or FLAC) in input_b64".to_string(),
            ));
        }
        progress("decode the song", 0.01);
        let track = decode_track(&params.input_bytes)?;
        cancel.check()?;
        let worker = self.worker.as_ref().ok_or_else(|| {
            AssetAiError::Backend("stems generate called before ensure_loaded".to_string())
        })?;
        let events = worker.separate(track, cancel)?;
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
        if output == StemsOutput::Ogg {
            progress("encode Ogg stems", 0.98);
            let artifacts = encode_ogg_stems(&stems)?;
            progress("done", 1.0);
            return Ok(artifacts);
        }
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codec_names_the_answer() {
        assert_eq!(StemsOutput::of("").unwrap(), StemsOutput::Bundle);
        assert_eq!(StemsOutput::of("OGG").unwrap(), StemsOutput::Ogg);
        assert_eq!(StemsOutput::of("vorbis").unwrap(), StemsOutput::Ogg);
        assert!(StemsOutput::of("h265").is_err());
    }

    #[test]
    fn a_mono_48k_song_arrives_as_44k_stereo() {
        let mono: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let track = decode_track(&crate::wav::encode_wav_pcm16_mono(&mono, 48_000)).unwrap();
        assert_eq!(track.left.len(), 44_100);
        assert_eq!(track.left, track.right);
        assert!(decode_track(b"not audio at all").is_err());
    }

    #[test]
    fn ogg_stems_come_back_in_model_order() {
        let tone = |level: f32| (0..44_100).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * level).collect::<Vec<f32>>();
        let stem = |level: f32| StereoBuf { left: tone(level), right: tone(level) };
        let stems: StemSet = [stem(0.1), stem(0.2), stem(0.3), stem(0.4)];
        let artifacts = encode_ogg_stems(&stems).unwrap();
        assert_eq!(artifacts.len(), 4);
        for (artifact, level) in artifacts.iter().zip([0.1f32, 0.2, 0.3, 0.4]) {
            assert_eq!(artifact.content_type, "audio/ogg");
            let audio = makepad_audio_decode::decode_any(&artifact.bytes).unwrap();
            assert_eq!((audio.rate, audio.channels), (SAMPLE_RATE, 2));
            let samples = &audio.pcm_interleaved_f32;
            let rms = (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt();
            let want = level / std::f32::consts::SQRT_2;
            assert!((rms - want).abs() < want * 0.2, "rms {rms} for a tone of {level}");
        }
    }
}
