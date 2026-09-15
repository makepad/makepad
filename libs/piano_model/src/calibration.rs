//! Construction-time numerical corrections for the modelled string modes.
//!
//! Tables contain no audio. Keys must be strictly increasing within A0..=C8
//! (MIDI 21..=108), gains finite within -36..=24 dB, and decay scales finite
//! within 0.1..=4. Invalid fits are rejected, never silently clamped.
//! Pitch interpolation is linear in MIDI key: gains in dB, decay scales in
//! log space, with endpoint clamping. Velocity interpolation is linear in
//! dB between the three knots, also clamped at the endpoints.
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
            self.gain_db[2][m]
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
