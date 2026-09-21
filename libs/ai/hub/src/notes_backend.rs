//! `notes` domain: PCM WAV -> Basic Pitch note-event JSON + MIDI.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use makepad_ai_notes::{to_midi_bytes, NoteTranscription, NotesModel};

pub struct NotesBackend {
    model_id: String,
    model: Option<NotesModel>,
}

impl NotesBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            model: None,
        }
    }
}

impl ContentBackend for NotesBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        if self.model.is_some() {
            return Ok(());
        }
        ctx.ensure_files()?;
        ctx.cancel.check()?;
        (ctx.progress)("notes: parse ONNX", 0.5);
        let path = ctx.path_by_role("model")?;
        self.model = Some(
            NotesModel::load(&path)
                .map_err(|error| AssetAiError::Backend(format!("notes load: {error}")))?,
        );
        (ctx.progress)("notes: ready", 1.0);
        Ok(())
    }

    fn is_resident(&self) -> bool {
        self.model.is_some()
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        self.model = None;
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(
                "notes: input_b64 is required (PCM WAV, any rate/channels)".to_string(),
            ));
        }
        cancel.check()?;
        progress("notes: decode WAV", 0.02);
        let (mono, source_rate) = crate::wav::decode_wav_to_mono_f32(&params.input_bytes)
            .map_err(|error| AssetAiError::Params(format!("notes: invalid WAV: {error}")))?;
        if source_rate == 0 {
            return Err(AssetAiError::Params(
                "notes: WAV sample rate must be non-zero".to_string(),
            ));
        }
        cancel.check()?;
        progress("notes: resample 22050 Hz", 0.05);
        let mono = crate::resample::resample_channel(
            &mono,
            source_rate,
            makepad_ai_notes::SAMPLE_RATE as u32,
        );
        let model = self.model.as_mut().ok_or_else(|| {
            AssetAiError::Backend("notes backend used before ensure_loaded".to_string())
        })?;
        let transcription = model
            .transcribe_with_progress(&mono, |done, total| {
                let fraction = if total == 0 {
                    1.0
                } else {
                    done as f64 / total as f64
                };
                progress(
                    &format!("notes: window {done}/{total}"),
                    0.08 + 0.84 * fraction,
                );
                !cancel.is_cancelled()
            })
            .map_err(|error| {
                if cancel.is_cancelled() {
                    AssetAiError::Cancelled
                } else {
                    AssetAiError::Backend(format!("notes inference: {error}"))
                }
            })?;
        cancel.check()?;
        progress("notes: encode JSON + MIDI", 0.96);
        let json = transcription_json(&transcription).into_bytes();
        let midi = to_midi_bytes(&transcription, None);
        progress("done", 1.0);
        Ok(vec![
            ArtifactData {
                content_type: "application/json",
                ext: "json",
                bytes: json,
            },
            ArtifactData {
                content_type: "audio/midi",
                ext: "mid",
                bytes: midi,
            },
        ])
    }
}

fn transcription_json(transcription: &NoteTranscription) -> String {
    let mut json = format!(
        "{{\"frame_rate\":{},\"notes\":[",
        transcription.frame_rate
    );
    for (index, note) in transcription.notes.iter().enumerate() {
        if index != 0 {
            json.push(',');
        }
        json.push_str(&format!(
            "{{\"start_secs\":{},\"end_secs\":{},\"midi\":{},\"amplitude\":{},\"bends\":[",
            note.start_secs, note.end_secs, note.midi, note.amplitude
        ));
        for (bend_index, bend) in note.bends.iter().enumerate() {
            if bend_index != 0 {
                json.push(',');
            }
            json.push_str(&bend.to_string());
        }
        json.push_str("]}");
    }
    json.push_str("]}");
    json
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Domain, Registry};
    use makepad_ai_notes::{NoteEvent, FRAME_RATE};

    #[test]
    fn registry_contract_is_complete() {
        let registry = Registry::embedded().unwrap();
        let spec = registry.find("basic-pitch").expect("basic-pitch entry");
        assert_eq!(spec.domain, Domain::Notes);
        assert_eq!(spec.backend, "notes");
        assert_eq!(spec.vram_gb, Some(0.1));
        let model = spec.file_by_role("model").unwrap();
        assert_eq!(model.cache_as, "notes/basic_pitch_nmp.onnx");
        assert_eq!(model.size, Some(230_444));
        assert_eq!(
            model.sha256.as_deref(),
            Some("2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec")
        );
    }

    #[test]
    fn domain_round_trips() {
        assert_eq!(Domain::parse("notes"), Some(Domain::Notes));
        assert_eq!(Domain::Notes.as_str(), "notes");
    }

    #[test]
    fn json_contract_contains_bends() {
        let text = transcription_json(&NoteTranscription {
            notes: vec![NoteEvent {
                start_secs: 0.0,
                end_secs: 0.5,
                midi: 60,
                amplitude: 0.75,
                bends: vec![0.0, 1.0 / 3.0],
            }],
            frame_rate: FRAME_RATE,
            onsets: Vec::new(),
        });
        let value = makepad_strict_json::parse(text.as_bytes()).unwrap();
        assert_eq!(value.get("notes").and_then(|notes| notes.as_arr()).unwrap().len(), 1);
    }
}
