//! A cover rendered as the song loop renders a song (`music_iter`): the
//! Score through the engine with its mix and master chain, its master
//! limiter's gain fitted so the cover sits at a loudness target, then the
//! scorecard. The Score is the mechanical import or the arranger's.

use makepad_score_live::RenderOpts;
use makepad_audio_dsp::loudness;
use makepad_music_iter::{card, Card, Song};
use makepad_music_iter::render::{render_mix_checked, render_song_checked};
pub use makepad_music_iter::render::RenderError;

pub struct Rendered {
    /// The Score as rendered (its limiter's gain fitted).
    pub source: String,
    pub song: Song,
    pub card: Card,
    pub lufs: f64,
    pub true_peak: f64,
}

/// Render `source`, refit its master limiter's gain to `target_lufs` (the
/// last Limiter of the chain; once, then render again), and score it.
pub fn render(source: &str, target_lufs: f64, control: &makepad_score_live::PrepareControl) -> Result<Rendered, String> {
    let doc = makepad_score_live::model::read_document_controlled(source, control).map_err(|error| error.to_string())?.doc;
    render_document(source, &doc, target_lufs, control).map_err(RenderError::into_message)
}

/// Render a cover, retaining the load outcome for batch callers.
pub fn render_checked(source: &str, target_lufs: f64, control: &makepad_score_live::PrepareControl) -> Result<Rendered, RenderError> {
    let doc = makepad_score_live::model::read_document_controlled(source, control)?.doc;
    render_document(source, &doc, target_lufs, control)
}

fn render_document(source: &str, doc: &makepad_score_live::model::AuthoredDoc, target_lufs: f64, control: &makepad_score_live::PrepareControl) -> Result<Rendered, RenderError> {
    let mut source = source.to_string();
    if let Some(limiter) = doc.fx.iter().rev().find(|f| f.kind == makepad_score_player::FxKind::Limiter) {
        let mix = render_mix_checked(&source, &RenderOpts::default(), control)?;
        let heard = loudness::measure(&mix.left, &mix.right, mix.rate as f64);
        drop(mix);
        // The gain moves the loudness about dB for dB until it limits hard.
        let gain = limiter.params[1] as f64;
        if heard.lufs.is_finite() {
            let fitted = (gain * 10f64.powf((target_lufs - heard.lufs) / 20.0)).clamp(0.2, 4.0);
            if let Ok(splice) = makepad_score_live::source::set_field(&source, &limiter.name, "gain", &format!("{:.3}", fitted)) {
                source = makepad_edit_values::apply_splices(&source, 0, &[splice])?;
            }
        }
    }
    let song = render_song_checked(&source, &RenderOpts::default(), false, control)?;
    let measured = loudness::measure(&song.mix.left, &song.mix.right, song.mix.rate as f64);
    let card = card::score(&song, &card::Opts { target_lufs: target_lufs as f32 });
    Ok(Rendered { source, song, card, lufs: measured.lufs, true_peak: measured.true_peak })
}

/// The WAV bytes of a render.
pub fn wav(song: &Song) -> Vec<u8> {
    makepad_music_iter::render::wav(song.mix.rate, &song.mix.left, &song.mix.right)
}
