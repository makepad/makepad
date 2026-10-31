//! A SONG ANALYSED AHEAD: the live analysis run once over a whole track,
//! every hop's picture rows and signals kept, so a frame at ANY time is a
//! pure function of that time. What a render needs: a locked-time export
//! reads the picture at the frame's exact moment (never a live bus that
//! depends on how the frames were reached), and a seek lands on the same
//! picture as playing through.
//!
//! [`AudioTrack::analyse`] runs [`AudioReactive`] hop by hop over the mono
//! samples (the same FFT, bins, onsets, envelopes and auto-gain as live);
//! [`TrackPicture::frame`] lays the rows that were newest at `t` into the
//! same texture layout the live bus uploads and returns the same
//! [`AudioFrame`], so every look, kit and helper reads it unchanged.
//! Before the track and after it the picture is silence.
//!
//! Memory: one byte a spectrum bin and two a waveform point, 768 bytes a
//! hop (~21 ms): about 7 MB for a three-minute song at 48 kHz.

use crate::analysis::{fold_bands, AudioFrame, AudioReactive, Levels, AUDIO_BINS, AUDIO_SPEC_ROWS, AUDIO_TEX_H, AUDIO_TEX_W, AUDIO_WAVE_ROWS, HOP, SPECTRUM_BANDS};
use makepad_draw::*;

/// A track's analysis, hop by hop, on some clock (`start` is that clock's
/// time of the first analysed sample).
#[derive(Clone, Debug, Default)]
pub struct AudioTrack {
    /// Clock time of the first sample analysed.
    pub start: f64,
    /// Seconds a hop.
    pub hop_secs: f64,
    /// `hops * AUDIO_BINS` spectrum values, 0..255.
    spec: Vec<u8>,
    /// `hops * AUDIO_BINS` waveform points, signed.
    wave: Vec<i16>,
    levels: Vec<Levels>,
}

impl AudioTrack {
    /// Analyse `mono` samples at `rate`; the first one plays at `start`.
    pub fn analyse(mono: &[f32], rate: f32, start: f64) -> AudioTrack {
        let mut bus = AudioReactive::new();
        let hops = mono.len() / HOP;
        let mut track = AudioTrack {
            start,
            hop_secs: HOP as f64 / rate.max(8_000.0) as f64,
            spec: Vec::with_capacity(hops * AUDIO_BINS),
            wave: Vec::with_capacity(hops * AUDIO_BINS),
            levels: Vec::with_capacity(hops),
        };
        for hop in mono.chunks_exact(HOP) {
            bus.push_samples(hop, rate);
            if !bus.has_audio() {
                // A rate the bus refuses: nothing to analyse.
                return AudioTrack { start, ..Default::default() };
            }
            track.spec.extend(bus.spectrum().iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
            track.wave.extend(bus.wave_row().iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16));
            track.levels.push(bus.levels());
        }
        track
    }

    /// The same analysis on a clock `by` seconds later.
    pub fn shifted(&self, by: f64) -> AudioTrack {
        AudioTrack { start: self.start + by, ..self.clone() }
    }

    pub fn hops(&self) -> usize {
        self.levels.len()
    }

    /// Whether the track plays at `t`.
    pub fn covers(&self, t: f64) -> bool {
        self.valid(self.newest(t)).is_some()
    }

    /// The hop that was newest at `t` (its samples all played), as a signed
    /// index: negative before the first one.
    fn newest(&self, t: f64) -> i64 {
        if !(self.hop_secs > 0.0) || !t.is_finite() {
            return -1;
        }
        ((t - self.start) / self.hop_secs).floor() as i64 - 1
    }

    fn valid(&self, hop: i64) -> Option<usize> {
        (hop >= 0 && (hop as usize) < self.hops()).then_some(hop as usize)
    }

    /// The signals at `t` (silence outside the track).
    pub fn levels_at(&self, t: f64) -> Levels {
        self.valid(self.newest(t)).map_or(Levels::default(), |h| self.levels[h])
    }

    /// The spectrum at `t` folded into [`SPECTRUM_BANDS`] bands.
    pub fn bands_at(&self, t: f64) -> [f32; SPECTRUM_BANDS] {
        let mut out = [0.0; SPECTRUM_BANDS];
        if let Some(h) = self.valid(self.newest(t)) {
            let row: Vec<f32> = self.spec[h * AUDIO_BINS..(h + 1) * AUDIO_BINS].iter().map(|v| *v as f32 / 255.0).collect();
            fold_bands(&row, &mut out);
        }
        out
    }
}

/// The picture of an [`AudioTrack`] at a moment: owns the textures a host
/// binds; refilled only when the newest hop changes. A new picture goes to
/// the next of three textures, so one a frame still in flight reads is
/// never overwritten under it (a render must read exactly its own moment).
#[derive(Default)]
pub struct TrackPicture {
    texes: Vec<Texture>,
    next: usize,
    data: Vec<f32>,
    /// (track identity, newest hop) the texture shows.
    shown: Option<(usize, i64)>,
}

impl TrackPicture {
    /// The frame of `track` at `t`: the rows newest at `t` in the live
    /// layout, and the signals of that hop.
    pub fn frame(&mut self, cx: &mut Cx, track: &AudioTrack, t: f64) -> AudioFrame {
        let newest = track.newest(t);
        let key = (track as *const AudioTrack as usize ^ track.spec.as_ptr() as usize, newest);
        let spec_cursor = newest.rem_euclid(AUDIO_SPEC_ROWS as i64) as f32;
        let wave_cursor = newest.rem_euclid(AUDIO_WAVE_ROWS as i64) as f32;
        if self.shown != Some(key) || self.texes.is_empty() {
            self.fill(track, newest);
            self.upload(cx);
            self.shown = Some(key);
        }
        let tex = self.texes[(self.next + self.texes.len() - 1) % self.texes.len()].clone();
        track.levels_at(t).frame(tex, spec_cursor, wave_cursor, track.hop_secs as f32, false, track.bands_at(t))
    }

    fn fill(&mut self, track: &AudioTrack, newest: i64) {
        self.data.clear();
        self.data.resize(AUDIO_TEX_W * AUDIO_TEX_H, 0.0);
        for age in 0..AUDIO_SPEC_ROWS as i64 {
            let hop = newest - age;
            let Some(h) = track.valid(hop) else { continue };
            let row = hop.rem_euclid(AUDIO_SPEC_ROWS as i64) as usize * AUDIO_TEX_W;
            for (out, v) in self.data[row..row + AUDIO_BINS].iter_mut().zip(&track.spec[h * AUDIO_BINS..(h + 1) * AUDIO_BINS]) {
                *out = *v as f32 / 255.0;
            }
        }
        for age in 0..AUDIO_WAVE_ROWS as i64 {
            let hop = newest - age;
            let Some(h) = track.valid(hop) else { continue };
            let row = (AUDIO_SPEC_ROWS + hop.rem_euclid(AUDIO_WAVE_ROWS as i64) as usize) * AUDIO_TEX_W;
            for (out, v) in self.data[row..row + AUDIO_BINS].iter_mut().zip(&track.wave[h * AUDIO_BINS..(h + 1) * AUDIO_BINS]) {
                *out = *v as f32 / 32767.0;
            }
        }
    }

    fn upload(&mut self, cx: &mut Cx) {
        const RING: usize = 3;
        if self.texes.len() < RING {
            self.texes.push(Texture::new_with_format(
                cx,
                TextureFormat::VecRf32 { width: AUDIO_TEX_W, height: AUDIO_TEX_H, data: Some(self.data.clone()), updated: TextureUpdated::Full },
            ));
            self.next = self.texes.len() % RING;
            return;
        }
        let tex = &self.texes[self.next];
        let mut held = tex.take_vec_f32(cx);
        held.clear();
        held.extend_from_slice(&self.data);
        tex.put_back_vec_f32(cx, held, None);
        self.next = (self.next + 1) % RING;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks(rate: usize, secs: usize) -> Vec<f32> {
        (0..rate * secs)
            .map(|i| {
                let into = i % (rate / 2);
                if into < 480 { (1.0 - into as f32 / 480.0) * (i as f32 * 0.37).sin() } else { 0.0 }
            })
            .collect()
    }

    /// The analysis is a pure function of the time asked: the same moment
    /// reads the same signals however it is reached, the rows are the live
    /// bus's, and outside the track it is silence.
    #[test]
    fn a_track_reads_the_same_at_the_same_moment() {
        let mono = clicks(48_000, 6);
        let track = AudioTrack::analyse(&mono, 48_000.0, 10.0);
        assert_eq!(track.hops(), mono.len() / HOP);
        assert_eq!(track.levels_at(9.0), Levels::default(), "before the track");
        assert_eq!(track.levels_at(30.0), Levels::default(), "after the track");
        let a = track.levels_at(12.34);
        let b = track.levels_at(12.34);
        assert_eq!(a, b);
        // A click on every half second: the kick fires in the track.
        let kicks = (0..200).map(|k| track.levels_at(10.5 + k as f64 * 0.025).hit[0]).fold(0.0f32, f32::max);
        assert!(kicks > 0.3, "{kicks}");
        // The spectrum folds like the live bus's.
        let bands = track.bands_at(12.03);
        assert!(bands.iter().any(|v| *v > 0.0), "{bands:?}");
        // Shifted, the same reading lands `by` later.
        let later = track.shifted(2.0);
        assert_eq!(later.levels_at(14.34), a);
    }
}
