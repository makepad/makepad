//! The whole lyrics bake as one callable: vocals stem in, [`TrackLyrics`]
//! out. The shared implementation behind the asset-ui "analyse" action and
//! (via the same align core) the VJ's live bake.
//!
//! Whisper transcribes with cross-attention capture, then [`crate::align`]
//! refines: teacher-forced windows, onset snapping, the sanity layer.
//! Confidence lands in the document exactly as the audit defends it — the
//! producer declares, readers decide how to display.

use crate::align::{self, OnsetPreset, PipelineConfig};
use crate::schema::{LyricLine, OnsetStats, TrackLyrics};
use std::path::Path;

/// A loaded whisper model plus its decode state, reusable across tracks.
pub struct LyricsBaker {
    model: Box<makepad_ai_speech::whisper::WhisperModel>,
    state: makepad_ai_speech::whisper::WhisperState,
    model_name: String,
}

impl LyricsBaker {
    /// Load the checkpoint at `path` (a ggml whisper file).
    pub fn open(path: &Path) -> Result<LyricsBaker, String> {
        let text = path.to_string_lossy().to_string();
        let model = makepad_ai_speech::whisper::WhisperModel::load_file(&text)
            .map_err(|error| format!("whisper model: {error}"))?;
        let state = makepad_ai_speech::whisper::WhisperState::new(&model);
        let model_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or(text);
        Ok(LyricsBaker { model: Box::new(model), state, model_name })
    }

    /// Bake lyrics from a mono vocals stem at `rate` Hz. `duration_secs` is
    /// the full track length (lines are clamped to it). Returns `None` when
    /// nothing was sung.
    pub fn bake(
        &mut self,
        vocals_mono: &[f32],
        rate: f64,
        duration_secs: f64,
        language: &str,
    ) -> Option<TrackLyrics> {
        let samples_16k = align::resample(vocals_mono, rate, align::WHISPER_RATE);
        let analysis = align::analyze_vocals(vocals_mono, rate, OnsetPreset::Snapping);
        let mut params = makepad_ai_speech::whisper::WhisperParams::default();
        params.language = language.to_string();
        params.no_timestamps = false;
        params.single_segment = false;
        params.temperature = 0.0;
        params.suppress_blank = true;
        let aligned = self.state.transcribe_aligned(&self.model, &samples_16k, &params);
        let config =
            PipelineConfig { language: language.to_string(), force: true, snap: true };
        let (segments, timed_lines) = align::refine(
            &mut self.state,
            &self.model,
            &samples_16k,
            aligned,
            &analysis,
            duration_secs,
            &config,
        );
        if timed_lines.is_empty() {
            return None;
        }
        // Audit stats: what the onset snap actually moved.
        let mut snapped = 0usize;
        let mut total_ms = 0.0f64;
        let mut max_ms = 0.0f64;
        for segment in &segments {
            for word in &segment.words {
                if let Some(delta) = word.snap {
                    snapped += 1;
                    total_ms += delta.abs() * 1000.0;
                    max_ms = max_ms.max(delta.abs() * 1000.0);
                }
            }
        }
        let lines: Vec<LyricLine> = timed_lines
            .into_iter()
            .map(|line| LyricLine {
                start_secs: line.start,
                end_secs: line.end,
                text: line.text,
                words: line.words,
                // The producer's confidence, undiluted: display policy (hop
                // vs sweep) belongs to readers.
                confident: line.confident,
            })
            .collect();
        Some(TrackLyrics {
            backend: "whisper".into(),
            model: self.model_name.clone(),
            language: language.to_string(),
            duration_secs,
            onset: OnsetStats {
                snapped,
                mean_ms: if snapped > 0 { total_ms / snapped as f64 } else { 0.0 },
                max_ms,
            },
            lines,
        })
    }
}
