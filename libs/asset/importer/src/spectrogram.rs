//! The importer's side of the audio picture: encode what
//! [`makepad_audio_picture`] renders.
//!
//! The recipe itself — the FFT, the log-frequency band, the colour ramp, the
//! antialiased wave strip — moved to that crate so the preview widgets can
//! draw the SAME picture live from decoded samples instead of only ever
//! showing one somebody had already baked. What stays here is what only a
//! baker needs: PNG/JPEG encoding, and the checks that the published picture
//! fits the content contract.

pub use makepad_audio_picture::composite::{composite_rgba, CompositeRegions, WAVE_STRIP_FRACTION};
pub use makepad_audio_picture::spectrogram::{spectrogram_rgba, HD_H, HD_W};

/// The high-definition spectrogram, PNG-encoded.
pub fn spectrogram_png(samples: &[f32], sample_rate: u32) -> Option<Vec<u8>> {
    let rgba = spectrogram_rgba(samples, sample_rate, HD_W, HD_H)?;
    crate::classic_import::encode_png_rgba(&rgba, HD_W as u32, HD_H as u32).ok()
}

/// The composite an audio asset publishes — spectrogram with a wave strip
/// along its bottom edge — PNG-encoded, with the regions to declare on the
/// thumbnail.
pub fn composite_png(
    samples: &[f32],
    sample_rate: u32,
) -> Option<(Vec<u8>, CompositeRegions)> {
    let (rgba, regions) = composite_rgba(samples, sample_rate, HD_W, HD_H)?;
    let png = crate::classic_import::encode_png_rgba(&rgba, HD_W as u32, HD_H as u32).ok()?;
    Some((png, regions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_data::ThumbnailViewKind;
    use std::f32::consts::PI;

    /// Render a real track to a PNG a human can look at. Skipped unless
    /// `SPECTROGRAM_WAV` names one; writes `SPECTROGRAM_PNG` (or a temp
    /// file) and prints the path. The same instrument the classic importer
    /// uses to put a real map in front of someone.
    #[test]
    fn export_spectrogram_for_capture() {
        let Ok(input) = std::env::var("SPECTROGRAM_WAV") else {
            eprintln!("no SPECTROGRAM_WAV; skipped");
            return;
        };
        let bytes = std::fs::read(&input).expect("wav");
        // Any audio the catalog carries, not just RIFF PCM.
        let media = match std::path::Path::new(&input)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("mp3") => makepad_asset_data::MediaType::Mp3,
            Some("ogg") => makepad_asset_data::MediaType::Ogg,
            _ => makepad_asset_data::MediaType::Wav,
        };
        let pcm = crate::thumbs::decode_audio(&bytes, media).expect("pcm");
        let mono = makepad_audio_picture::mono(&pcm.frames);
        let (png, regions) = composite_png(&mono, pcm.sample_rate).expect("picture");
        let out = std::env::var("SPECTROGRAM_PNG")
            .unwrap_or_else(|_| std::env::temp_dir().join("spectrogram.png").display().to_string());
        std::fs::write(&out, &png).expect("write");
        eprintln!(
            "composite: {} -> {out} ({} bytes, {}x{}, fft {:?}, wave {:?}, {:.1}s of audio)",
            input,
            png.len(),
            HD_W,
            HD_H,
            regions.fft,
            regions.wave,
            pcm.frames.len() as f32 / pcm.sample_rate as f32
        );
    }

    fn busy_track(rate: u32) -> Vec<f32> {
        // A sweep plus a beat, so the picture is not a compressible flat
        // field.
        let mut samples = vec![0.0f32; rate as usize * 20];
        for (i, s) in samples.iter_mut().enumerate() {
            let t = i as f32 / rate as f32;
            let sweep = 80.0 * (1.0 + 40.0 * (t / 20.0));
            *s = (2.0 * PI * sweep * t).sin() * 0.5
                + (2.0 * PI * 7_000.0 * t).sin() * 0.1 * ((t * 8.0).fract() < 0.1) as u8 as f32;
        }
        samples
    }

    /// The picture a thumbnail actually ships as: high definition, inside
    /// the content contract's thumbnail bounds, and small enough that the
    /// VJ's 8 MB thumbnail budget is never in question.
    #[test]
    fn the_hd_thumbnail_encodes_within_every_budget() {
        let rate = 44_100;
        let png = spectrogram_png(&busy_track(rate), rate).expect("png");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((w as usize, h as usize), (HD_W, HD_H));
        assert!(
            w >= makepad_asset_data::limits::THUMBNAIL_MIN_DIM
                && w <= makepad_asset_data::limits::THUMBNAIL_MAX_DIM
                && h >= makepad_asset_data::limits::THUMBNAIL_MIN_DIM
                && h <= makepad_asset_data::limits::THUMBNAIL_MAX_DIM,
            "the content contract accepts 256..=4096 per side"
        );
        assert!(
            png.len() < 8 * 1024 * 1024,
            "a thumbnail must fit the VJ's 8 MB budget: {} bytes",
            png.len()
        );
        eprintln!("hd spectrogram png: {} bytes ({w}x{h})", png.len());
    }

    /// The composite's regions are exactly what the manifest declares, and
    /// they validate against the picture they describe — the whole point of
    /// stamping the layout rather than letting a consumer measure it.
    #[test]
    fn the_composite_regions_become_declared_views() {
        let rate = 44_100;
        let (png, regions) = composite_png(&busy_track(rate), rate).expect("png");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let views = crate::thumbs::audio_views(regions);
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].kind, ThumbnailViewKind::Fft);
        assert_eq!(views[1].kind, ThumbnailViewKind::Wave);
        let meta = makepad_asset_data::ThumbnailMeta {
            blob: makepad_asset_data::BlobId::hash_of(&png),
            media: makepad_asset_data::ThumbnailMedia::Png,
            width: HD_W as u32,
            height: HD_H as u32,
            byte_len: png.len() as u64,
            views,
        };
        meta.validate().expect("the declared regions fit the picture they describe");
        assert_eq!(meta.animation(), None, "a spectrogram is not a sprite sheet");
    }
}
