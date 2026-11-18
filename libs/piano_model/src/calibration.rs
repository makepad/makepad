//! Construction-time numerical corrections for the modelled string modes.
//!
//! Tables contain no audio. Keys must be strictly increasing within A0..=C8
//! (MIDI 21..=108), gains finite within -36..=24 dB, and decay scales finite
//! within 0.1..=4. Invalid fits are rejected, never silently clamped.
//! Pitch interpolation is linear in MIDI key: gains in dB, decay scales in
//! log space, with endpoint clamping. Velocity interpolation is linear in
//! dB between the three knots, clamped below the lowest; above the top knot
//! (112) the mf->ff slope continues up to velocity 127. The reference's
//! loudest layers keep getting louder there (+3..4 dB from 112 to 127 as
//! its SFZ plays them) where the model's hammer alone adds +1..2.
//!
//! A fitted entry is a correction at ITS key only. It makes that key's
//! partial meet the recording, and so it carries the inverse of whatever
//! the model does at that partial's frequency: where the soundboard's
//! response has a peak or a notch there, where the bridge drains the
//! partial, where the strike comb falls. A key between two measured keys
//! has its partials at other frequencies, so the corrections themselves do
//! not carry over: interpolating them left the keys in between 5-8 dB off
//! their neighbours in level, and single partials 10-18 dB off. What does
//! carry over is the TARGET: the level and the decay rate the fit asked
//! for. `resolve` therefore interpolates, per fitted partial, the model's
//! own linear prediction of the radiated level plus the fitted gain, and
//! the fitted decay rate of each string mode, and gives the key in between
//! the gain and decay that bring ITS prediction onto that target. Entries
//! the fit left at the note's fill value (the last entry of a row) are not
//! targets, and where both neighbours carry only those the plain
//! interpolation above still applies.
//!
//! Array index 0 is the fundamental. All current string modes are directly
//! addressable through `CALIBRATION_PARTIALS`. Beyond that range, the last
//! gain in dB and log decay scale taper linearly to zero over 16 partials:
//! the first extra partial retains 15/16 of the correction, the 16th and
//! above use unity.

use crate::keys::{KeyDesign, FIRST_KEY, LAST_KEY};

pub const CALIBRATION_PARTIALS: usize = 240;
pub const CALIBRATION_VELOCITIES: [u8; 3] = [28, 68, 112];

#[derive(Clone, Debug)]
pub struct CalibrationNote {
    pub key: u8,
    /// Excitation correction in dB, including note loudness, at each
    /// velocity in `CALIBRATION_VELOCITIES`. No output gain is changed.
    pub gain_db: [[f32; CALIBRATION_PARTIALS]; 3],
    /// Pole-radius exponent: r_new = r_old^scale. Values above one decay
    /// faster; below one sustain longer. Additional damper loss is unchanged.
    pub decay_scale: [f32; CALIBRATION_PARTIALS],
}

pub(crate) fn validate(notes: &[CalibrationNote]) {
    for (i, note) in notes.iter().enumerate() {
        assert!((FIRST_KEY..=LAST_KEY).contains(&note.key), "calibration key outside A0..=C8");
        assert!(i == 0 || notes[i - 1].key < note.key, "calibration keys must be strictly increasing");
        assert!(
            note.gain_db.iter().flatten().all(|g| g.is_finite() && (-36.0..=24.0).contains(g)),
            "calibration gains must be finite and within -36..=24 dB"
        );
        assert!(
            note.decay_scale.iter().all(|s| s.is_finite() && (0.1..=4.0).contains(s)),
            "calibration decay scales must be finite and within 0.1..=4"
        );
    }
}

/// Called only after validation, while constructing the instrument.
pub(crate) fn for_key(notes: &[CalibrationNote], key: u8) -> Option<CalibrationNote> {
    if notes.is_empty() {
        return None;
    }
    let hi = notes.partition_point(|note| note.key < key).min(notes.len() - 1);
    let mut result = notes[hi].clone();
    result.key = key;
    if hi > 0 && key < notes[hi].key {
        let lo = &notes[hi - 1];
        let t = (key - lo.key) as f32 / (notes[hi].key - lo.key) as f32;
        for m in 0..CALIBRATION_PARTIALS {
            for v in 0..3 {
                result.gain_db[v][m] = lo.gain_db[v][m] + (result.gain_db[v][m] - lo.gain_db[v][m]) * t;
            }
            let a = lo.decay_scale[m];
            let b = result.decay_scale[m];
            if a != b {
                result.decay_scale[m] = (a.ln() + (b.ln() - a.ln()) * t).exp();
            }
        }
    }
    Some(result)
}

/// Window of the level a fitted gain stands for, seconds after the strike.
/// From the strike itself: a partial the bridge drains in a few tens of
/// milliseconds has spent its energy before a later window opens, and a
/// level read there is only its aftersound.
pub(crate) const LEVEL_WINDOW: (f64, f64) = (0.0, 1.2);

/// Decay rate (1/s) of a mode from its rotation.
fn sigma_of(cr: f32, ci: f32, sample_rate: f64) -> f64 {
    -((cr as f64).hypot(ci as f64)).ln() * sample_rate
}

/// The design's linear prediction of partial `m`'s radiated level (dB, an
/// arbitrary but key-independent reference): the mezzo-forte pulse spectrum,
/// the strike comb, each string mode's residue and how much of it the level
/// window sees at its decay rate, and the radiation at the partial's
/// frequency. None for a mode that does not exist.
///
/// `notch_floor` bounds how deep a notch of the radiation counts, as a
/// power ratio to the mean over the surrounding semitone either way (0 for
/// the response as it is). A notch is two board modes cancelling in the
/// steady state; at the strike they have not built up yet and nothing
/// cancels, so a partial driven hard enough to fill a deep notch speaks as
/// a burst first. The key that receives a target fills at most 6 dB:
/// filling them whole put D2's and G2's fortissimo peaks 1.4 and 1.0 dB
/// higher, and filling none leaves the keys whose fundamental sits in a
/// notch 3-7 dB under their neighbours (key-to-key level roughness 1.5-2.8
/// dB rms against 0.7-0.9).
fn predicted_db(
    key: &KeyDesign,
    idx: usize,
    m: usize,
    sample_rate: f64,
    radiation: &dyn Fn(usize, f64) -> f64,
    notch_floor: f64,
) -> Option<f64> {
    if m >= key.modes_per_osc || key.gin[m] == 0.0 {
        return None;
    }
    let n = (m + 1) as f64;
    let f = n * key.f0 as f64 * (1.0 + key.b_coeff as f64 * n * n).sqrt();
    let (t0, t1) = LEVEL_WINDOW;
    let mut seen = 0.0;
    for osc in 0..key.n_osc {
        let i = osc * key.modes_padded + m;
        let amp = (key.gout[i] as f64).hypot(key.gout_re[i] as f64);
        let sigma = sigma_of(key.cr_sus[i], key.ci_sus[i], sample_rate).max(1e-3);
        seen += amp * amp * ((-2.0 * sigma * t0).exp() - (-2.0 * sigma * t1).exp()) / (2.0 * sigma * (t1 - t0));
    }
    let pulse = 1.0 / (1.0 + (2.0 * f * key.pulse_tau).powi(2));
    let mut radiated = radiation(idx, f);
    if notch_floor > 0.0 {
        let around = [-1.0f64, -0.5, 0.5, 1.0].map(|st| radiation(idx, f * (st / 12.0).exp2()).powi(2));
        let mean = (around.iter().sum::<f64>() + radiated * radiated) / 5.0;
        radiated = radiated.max((notch_floor * mean).sqrt());
    }
    let level = (key.gin[m] as f64).abs() * pulse * radiated * seen.sqrt();
    (level.is_finite() && level > 0.0).then(|| 20.0 * level.log10())
}

/// The table resolved for every key, in key order, with each key's decay
/// applied to its design. See the module notes for what is interpolated.
pub(crate) fn resolve(
    notes: &[CalibrationNote],
    keys: &mut [KeyDesign],
    sample_rate: f64,
    radiation: &dyn Fn(usize, f64) -> f64,
) -> Vec<Option<CalibrationNote>> {
    let mut out: Vec<Option<CalibrationNote>> = (0..keys.len()).map(|i| for_key(notes, FIRST_KEY + i as u8)).collect();
    if notes.is_empty() {
        keys.iter_mut().for_each(|k| crate::keys::lock_unisons(k, sample_rate, LEVEL_WINDOW.1));
        return out;
    }
    let anchor = |i: usize| notes.binary_search_by_key(&(FIRST_KEY + i as u8), |n| n.key).ok();
    // Measured keys, and the keys outside the table's range, take their
    // entries as they are.
    for i in 0..keys.len() {
        let key = FIRST_KEY + i as u8;
        if anchor(i).is_some() || key < notes[0].key || key > notes[notes.len() - 1].key {
            out[i].as_ref().unwrap().apply_decay(&mut keys[i]);
        }
    }
    for i in 0..keys.len() {
        let key = FIRST_KEY + i as u8;
        if anchor(i).is_some() || key < notes[0].key || key > notes[notes.len() - 1].key {
            continue;
        }
        let hi_n = notes.partition_point(|n| n.key < key);
        let (lo, hi) = (&notes[hi_n - 1], &notes[hi_n]);
        let (li, hi_i) = ((lo.key - FIRST_KEY) as usize, (hi.key - FIRST_KEY) as usize);
        let t = (key - lo.key) as f64 / (hi.key - lo.key) as f64;
        let note = out[i].as_mut().unwrap();
        // the plain interpolation first: the sympathetic bank and every
        // partial the fit did not reach
        note.apply_decay(&mut keys[i]);
        let (before, rest) = keys.split_at_mut(i);
        let (this, after) = rest.split_first_mut().unwrap();
        let (klo, khi) = (&before[li], &after[hi_i - i - 1]);
        let modes = this.modes_per_osc.min(klo.modes_per_osc).min(khi.modes_per_osc).min(CALIBRATION_PARTIALS);
        let slots = this.n_osc.min(klo.n_osc).min(khi.n_osc);
        let last = CALIBRATION_PARTIALS - 1;
        let gain_fitted = |m: usize| (0..3).any(|v| lo.gain_db[v][m] != lo.gain_db[v][last] || hi.gain_db[v][m] != hi.gain_db[v][last]);
        for m in 0..modes {
            // A fitted level stands for a level over time, so it carries
            // its neighbours' decay with it even where the decay itself was
            // left as modelled: a partial this key's bridge happens to
            // drain would otherwise be driven up to 24 dB harder to meet
            // the level window, and speak as a burst at the strike.
            let decay_fitted = lo.decay_scale[m] != lo.decay_scale[last] || hi.decay_scale[m] != hi.decay_scale[last];
            if !decay_fitted && !gain_fitted(m) {
                continue;
            }
            for osc in 0..slots {
                let a = sigma_of(klo.cr_sus[osc * klo.modes_padded + m], klo.ci_sus[osc * klo.modes_padded + m], sample_rate);
                let b = sigma_of(khi.cr_sus[osc * khi.modes_padded + m], khi.ci_sus[osc * khi.modes_padded + m], sample_rate);
                let j = osc * this.modes_padded + m;
                let radius = (this.cr_sus[j] as f64).hypot(this.ci_sus[j] as f64);
                if !(a > 0.0 && b > 0.0 && radius > 0.0) {
                    continue;
                }
                let sigma = (a.ln() + (b.ln() - a.ln()) * t).exp();
                // strictly inside the unit circle in f32 at any sample rate
                let factor = (-sigma / sample_rate).exp().min(1.0 - 1.2e-7) / radius;
                this.cr_sus[j] = (this.cr_sus[j] as f64 * factor) as f32;
                this.ci_sus[j] = (this.ci_sus[j] as f64 * factor) as f32;
            }
        }
    }
    // Every key has its decay now, which settles how far each unison locks
    // and so how much aftersound the level predictions below count.
    keys.iter_mut().for_each(|k| crate::keys::lock_unisons(k, sample_rate, LEVEL_WINDOW.1));
    for i in 0..keys.len() {
        let key = FIRST_KEY + i as u8;
        if anchor(i).is_some() || key < notes[0].key || key > notes[notes.len() - 1].key {
            continue;
        }
        let hi_n = notes.partition_point(|n| n.key < key);
        let (lo, hi) = (&notes[hi_n - 1], &notes[hi_n]);
        let (li, hi_i) = ((lo.key - FIRST_KEY) as usize, (hi.key - FIRST_KEY) as usize);
        let t = (key - lo.key) as f64 / (hi.key - lo.key) as f64;
        let note = out[i].as_mut().unwrap();
        let (this, klo, khi) = (&keys[i], &keys[li], &keys[hi_i]);
        let modes = this.modes_per_osc.min(klo.modes_per_osc).min(khi.modes_per_osc).min(CALIBRATION_PARTIALS);
        let last = CALIBRATION_PARTIALS - 1;
        let gain_fitted = |m: usize| (0..3).any(|v| lo.gain_db[v][m] != lo.gain_db[v][last] || hi.gain_db[v][m] != hi.gain_db[v][last]);
        for m in 0..modes {
            if !gain_fitted(m) {
                continue;
            }
            let (Some(pa), Some(pb), Some(px)) = (
                predicted_db(klo, li, m, sample_rate, radiation, 0.0),
                predicted_db(khi, hi_i, m, sample_rate, radiation, 0.0),
                predicted_db(this, i, m, sample_rate, radiation, 0.25),
            ) else {
                continue;
            };
            for v in 0..3 {
                let (a, b) = (lo.gain_db[v][m] as f64 + pa, hi.gain_db[v][m] as f64 + pb);
                note.gain_db[v][m] = (a + (b - a) * t - px).clamp(-36.0, 24.0) as f32;
            }
        }
    }
    out
}

fn partial_weight(partial: usize) -> (usize, f32) {
    if partial < CALIBRATION_PARTIALS {
        (partial, 1.0)
    } else {
        let weight = (CALIBRATION_PARTIALS + 15).saturating_sub(partial) as f32 / 16.0;
        (CALIBRATION_PARTIALS - 1, weight)
    }
}

impl CalibrationNote {
    pub(crate) fn gain_at(&self, partial: usize, velocity: u8) -> f32 {
        let (m, weight) = partial_weight(partial);
        let [lo, mid, hi] = CALIBRATION_VELOCITIES;
        let db = if velocity <= lo {
            self.gain_db[0][m]
        } else if velocity >= hi {
            let over = (velocity.min(127) - hi) as f32 / (hi - mid) as f32;
            self.gain_db[2][m] + (self.gain_db[2][m] - self.gain_db[1][m]) * over
        } else {
            let v = usize::from(velocity >= mid);
            let t = (velocity - CALIBRATION_VELOCITIES[v]) as f32
                / (CALIBRATION_VELOCITIES[v + 1] - CALIBRATION_VELOCITIES[v]) as f32;
            self.gain_db[v][m] + (self.gain_db[v + 1][m] - self.gain_db[v][m]) * t
        };
        db * weight
    }

    fn decay_at(&self, partial: usize) -> f32 {
        let (m, weight) = partial_weight(partial);
        if weight == 1.0 {
            self.decay_scale[m]
        } else if weight == 0.0 {
            1.0
        } else {
            (self.decay_scale[m].ln() * weight).exp()
        }
    }

    pub(crate) fn apply_decay(&self, key: &mut KeyDesign) {
        for m in 0..key.modes_per_osc {
            let scale = self.decay_at(m);
            for osc in 0..key.n_osc {
                let i = osc * key.modes_padded + m;
                scale_radius(&mut key.cr_sus[i], &mut key.ci_sus[i], scale);
            }
        }
        // The sympathetic bank represents these same string partials.
        // Its coupling gains and additional damper losses stay physical.
        for m in 0..key.sym_modes {
            scale_radius(&mut key.sym_cr[m], &mut key.sym_ci[m], self.decay_at(m));
        }
    }
}

fn scale_radius(cr: &mut f32, ci: &mut f32, scale: f32) {
    if scale == 1.0 {
        return;
    }
    let (re, im) = (*cr as f64, *ci as f64);
    let radius = re.hypot(im);
    if radius == 0.0 {
        return; // Padded/inactive mode.
    }
    let factor = radius.powf(scale as f64) / radius;
    *cr = (re * factor) as f32;
    *ci = (im * factor) as f32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_future_partials_taper_to_neutral() {
        let note = CalibrationNote {
            key: FIRST_KEY,
            gain_db: [[-16.0; CALIBRATION_PARTIALS]; 3],
            decay_scale: [4.0; CALIBRATION_PARTIALS],
        };
        for extra in 0..=16 {
            let partial = CALIBRATION_PARTIALS - 1 + extra;
            let weight = (16 - extra) as f32 / 16.0;
            assert_eq!(note.gain_at(partial, 68), -16.0 * weight);
            assert!((note.decay_at(partial) - 4.0f32.powf(weight)).abs() < 3e-7);
        }
        assert_eq!(note.gain_at(usize::MAX, 68), 0.0);
        assert_eq!(note.decay_at(usize::MAX), 1.0);
    }
}
