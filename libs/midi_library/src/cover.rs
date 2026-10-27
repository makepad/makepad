//! A cover rendered as the song loop renders a song (`music_iter`): the
//! Score through the engine with its mix and master chain, its master
//! limiter's gain fitted so the cover sits at a loudness target, then the
//! scorecard. The Score is the mechanical import or the arranger's.

use makepad_score_live::RenderOpts;
use makepad_audio_dsp::loudness;
use makepad_music_iter::{card, render_song, Card, Song};

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
pub fn render(source: &str, target_lufs: f64) -> Result<Rendered, String> {
    let doc = makepad_score_live::model::load(source).map_err(|d| d.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n"))?.doc;
    let mut source = source.to_string();
    if let Some(limiter) = doc.fx.iter().rev().find(|f| f.kind == makepad_score_player::FxKind::Limiter) {
        let mix = makepad_music_iter::render::render_mix(&source, &RenderOpts::default())?;
        let heard = loudness::measure(&mix.left, &mix.right, mix.rate as f64);
        drop(mix);
        // The gain moves the loudness about dB for dB until it limits hard.
        let gain = limiter.params[1] as f64;
        if heard.lufs.is_finite() {
            let fitted = (gain * 10f64.powf((target_lufs - heard.lufs) / 20.0)).clamp(0.2, 4.0);
            if let Ok(splice) = makepad_score_live::source::set_field(&source, &limiter.name, "gain", &format!("{:.3}", fitted)) {
                source = makepad_score_live::source::apply(&source, &splice);
            }
        }
    }
    let song = render_song(&source, &RenderOpts::default(), false)?;
    let measured = loudness::measure(&song.mix.left, &song.mix.right, song.mix.rate as f64);
    let card = card::score(&song, &card::Opts { target_lufs: target_lufs as f32 });
    Ok(Rendered { source, song, card, lufs: measured.lufs, true_peak: measured.true_peak })
}

/// The WAV bytes of a render.
pub fn wav(song: &Song) -> Vec<u8> {
    makepad_music_iter::render::wav(song.mix.rate, &song.mix.left, &song.mix.right)
}
