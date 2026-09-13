//! How loud a record actually is, by the broadcast rule rather than by
//! peak or by plain average.
//!
//! Two records that measure the same peak can be ten decibels apart to
//! the ear, and a plain RMS is fooled by anything with a lot of bass. The
//! measurement here is the one broadcasters settled on: weight the signal
//! the way hearing does, average it over short blocks, and throw away the
//! blocks that are too quiet to count.
//!
//! Written from the published standard (ITU-R BS.1770 and EBU R128), not
//! ported: the weighting is two textbook biquads, the gating is two
//! thresholds, and the whole of it is arithmetic anyone can check. The
//! compliance case at the bottom is the check -- a 1 kHz tone at -23 dBFS
//! on both channels must read -23.0.

/// The offset that puts the weighted mean square on the LUFS scale. From
/// the standard, and the reason a 1 kHz tone reads its own level: the
/// K-weighting is about 0.69 dB up at 1 kHz, and this takes it back off.
const LUFS_OFFSET: f64 = -0.691;

/// Blocks are this long, and start four times per block length.
const BLOCK_SECS: f64 = 0.4;
const BLOCK_OVERLAP: usize = 4;

/// Below this a block is not programme at all -- it is a gap -- and it is
/// left out of the mean entirely.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// And below this much under the ungated mean, a block is a quiet passage
/// rather than the level of the record.
const RELATIVE_GATE_LU: f64 = -10.0;

/// The high shelf of the K-weighting: what a head does to sound arriving
/// at it. Analogue design, so the filter is right at any sample rate
/// rather than only at the one the standard tabulates.
const SHELF_HZ: f64 = 1681.974450955533;
const SHELF_GAIN_DB: f64 = 3.999843853973347;
const SHELF_Q: f64 = 0.7071752369554196;

/// And the high pass under it, which is what stops a subsonic rumble
/// counting as loudness.
const HIGHPASS_HZ: f64 = 38.13547087602444;
const HIGHPASS_Q: f64 = 0.5003270373238773;

/// A direct-form biquad. Its own state, because each channel is filtered
/// separately and a shared one would leak one into the other.
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// The K-weighting's shelf, by the standard's own bilinear design.
    ///
    /// Not the textbook shelf formula: that one, given the same corner,
    /// Q and gain, lands a quarter of a decibel off, and a loudness
    /// meter a quarter of a decibel off is not a loudness meter. This
    /// derivation reproduces the coefficients the standard tabulates for
    /// 48 kHz exactly, and is right at every other rate for the same
    /// reason -- the design is analogue and the rate only enters through
    /// the frequency warp.
    fn high_shelf(rate: f64, hz: f64, q: f64, gain_db: f64) -> Biquad {
        let k = (std::f64::consts::PI * hz / rate).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        // The shelf's mid-band companion gain. Its exponent is the
        // standard's; it is what makes the shelf reach its full lift by
        // the corner rather than short of it.
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        Biquad {
            b0: (vh + vb * k / q + k * k) / a0,
            b1: 2.0 * (k * k - vh) / a0,
            b2: (vh - vb * k / q + k * k) / a0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            ..Default::default()
        }
    }

    /// And the high pass under it, normalised to unity at the top of the
    /// band the way the standard's own numerator is.
    fn high_pass(rate: f64, hz: f64, q: f64) -> Biquad {
        let k = (std::f64::consts::PI * hz / rate).tan();
        let a0 = 1.0 + k / q + k * k;
        Biquad {
            b0: 1.0,
            b1: -2.0,
            b2: 1.0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            ..Default::default()
        }
    }

    #[inline]
    fn step(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// The integrated loudness of a whole record, in LUFS.
///
/// `None` when there is nothing to measure -- too short for one block, or
/// so quiet that every block falls under the absolute gate. That is a
/// different answer from "very quiet" and must not be turned into a
/// number: a record with no measurement is one nothing should be
/// normalised against.
pub fn integrated_lufs(frames: &[[i16; 2]], sample_rate: u32) -> Option<f64> {
    let rate = sample_rate as f64;
    if rate < 8_000.0 || frames.is_empty() {
        return None;
    }
    let block = (rate * BLOCK_SECS).round() as usize;
    let hop = block / BLOCK_OVERLAP;
    if block == 0 || hop == 0 || frames.len() < block {
        return None;
    }
    // Weight both channels once, into one running sum of squares per
    // channel, walked block by block. The filters keep their state across
    // the whole record, exactly as a meter's would.
    let mut shelf = [
        Biquad::high_shelf(rate, SHELF_HZ, SHELF_Q, SHELF_GAIN_DB),
        Biquad::high_shelf(rate, SHELF_HZ, SHELF_Q, SHELF_GAIN_DB),
    ];
    let mut pass = [
        Biquad::high_pass(rate, HIGHPASS_HZ, HIGHPASS_Q),
        Biquad::high_pass(rate, HIGHPASS_HZ, HIGHPASS_Q),
    ];
    let scale = 1.0 / 32768.0;
    let mut weighted = Vec::with_capacity(frames.len() * 2);
    for frame in frames {
        for channel in 0..2 {
            let x = frame[channel] as f64 * scale;
            let y = pass[channel].step(shelf[channel].step(x));
            weighted.push(y * y);
        }
    }
    // Block means, from a prefix sum: every block is the same length, so
    // one pass over the sums answers all of them.
    let mut running = Vec::with_capacity(weighted.len() / 2 + 1);
    running.push(0.0f64);
    for pair in weighted.chunks_exact(2) {
        let last = *running.last().unwrap_or(&0.0);
        running.push(last + pair[0] + pair[1]);
    }
    let mut blocks = Vec::new();
    let mut start = 0;
    while start + block <= frames.len() {
        let sum = running[start + block] - running[start];
        let mean = sum / block as f64;
        if mean > 0.0 {
            blocks.push((mean, LUFS_OFFSET + 10.0 * mean.log10()));
        }
        start += hop;
    }
    gate(&blocks)
}

/// Two thresholds, in the order the standard sets them: drop what is not
/// programme, take the mean of what is left, then drop what is quiet
/// against THAT and take the mean again.
fn gate(blocks: &[(f64, f64)]) -> Option<f64> {
    let above_absolute: Vec<(f64, f64)> = blocks
        .iter()
        .copied()
        .filter(|(_, lufs)| *lufs > ABSOLUTE_GATE_LUFS)
        .collect();
    if above_absolute.is_empty() {
        return None;
    }
    let mean = |set: &[(f64, f64)]| -> f64 {
        let sum: f64 = set.iter().map(|(mean, _)| *mean).sum();
        sum / set.len() as f64
    };
    let ungated = mean(&above_absolute);
    if !(ungated > 0.0) {
        return None;
    }
    let relative = LUFS_OFFSET + 10.0 * ungated.log10() + RELATIVE_GATE_LU;
    let kept: Vec<(f64, f64)> = above_absolute
        .into_iter()
        .filter(|(_, lufs)| *lufs > relative)
        .collect();
    if kept.is_empty() {
        return None;
    }
    let gated = mean(&kept);
    (gated > 0.0).then(|| LUFS_OFFSET + 10.0 * gated.log10())
}

/// The gain that brings a record to `target`, as a multiplier.
///
/// Clamped, because normalisation is a convenience and never a reason to
/// hand the mixer a number that would tear the record apart: a very quiet
/// recording is brought up as far as is sensible and no further.
pub fn gain_for_target(measured_lufs: f64, target_lufs: f64) -> f32 {
    if !measured_lufs.is_finite() || !target_lufs.is_finite() {
        return 1.0;
    }
    let db = (target_lufs - measured_lufs).clamp(-24.0, 12.0);
    (10f64.powf(db / 20.0) as f32).clamp(0.06, 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, secs: f64, dbfs: f64) -> Vec<[i16; 2]> {
        let amplitude = 10f64.powf(dbfs / 20.0) * 32767.0;
        let step = 2.0 * std::f64::consts::PI * hz / rate as f64;
        (0..(rate as f64 * secs) as usize)
            .map(|n| {
                let value = (amplitude * (step * n as f64).sin()).round() as i16;
                [value, value]
            })
            .collect()
    }

    /// The compliance case: a 1 kHz tone at -23 dBFS on both channels
    /// reads -23.0. It exercises the whole chain at once -- both filters,
    /// the block mean, the offset -- and nothing else in this file needs
    /// to be trusted for it to be meaningful.
    #[test]
    fn a_tone_at_minus_twenty_three_reads_minus_twenty_three() {
        let pcm = sine(48_000, 1_000.0, 10.0, -23.0);
        let lufs = integrated_lufs(&pcm, 48_000).expect("a measurement");
        assert!((lufs + 23.0).abs() < 0.15, "{lufs}");
    }

    /// And the same at another rate, because the filters are designed
    /// rather than tabulated.
    #[test]
    fn the_weighting_is_right_at_a_rate_the_standard_does_not_tabulate() {
        let pcm = sine(44_100, 1_000.0, 10.0, -23.0);
        let lufs = integrated_lufs(&pcm, 44_100).expect("a measurement");
        assert!((lufs + 23.0).abs() < 0.2, "{lufs}");
    }

    /// Six decibels up, which is a factor of 1.995 and not of two --
    /// the arithmetic, not the round number people say.
    /// The design against the standard's own table. This is what makes
    /// the weighting right at every rate rather than at one: get the
    /// derivation wrong and the tone tests still pass to within a
    /// quarter of a decibel, while this does not.
    #[test]
    fn the_weighting_reproduces_the_published_coefficients() {
        let shelf = Biquad::high_shelf(48_000.0, SHELF_HZ, SHELF_Q, SHELF_GAIN_DB);
        for (got, want) in [
            (shelf.b0, 1.53512485958697),
            (shelf.b1, -2.69169618940638),
            (shelf.b2, 1.19839281085285),
            (shelf.a1, -1.69065929318241),
            (shelf.a2, 0.73248077421585),
        ] {
            assert!((got - want).abs() < 1e-12, "{got} vs {want}");
        }
        let pass = Biquad::high_pass(48_000.0, HIGHPASS_HZ, HIGHPASS_Q);
        for (got, want) in [
            (pass.b0, 1.0),
            (pass.b1, -2.0),
            (pass.b2, 1.0),
            (pass.a1, -1.99004745483398),
            (pass.a2, 0.99007225036621),
        ] {
            assert!((got - want).abs() < 1e-11, "{got} vs {want}");
        }
    }

    /// The relative gate is what makes this the level of the RECORD
    /// rather than the average of everything on it: a long quiet passage
    /// that is still audible is left out of the reckoning.
    #[test]
    fn a_quiet_passage_is_left_out_of_the_level() {
        let mut pcm = sine(48_000, 1_000.0, 6.0, -20.0);
        // Twenty-five decibels down: audible, far above the absolute
        // gate, and not what this record's level is.
        pcm.extend(sine(48_000, 1_000.0, 30.0, -45.0));
        let lufs = integrated_lufs(&pcm, 48_000).expect("a measurement");
        let loud = integrated_lufs(&sine(48_000, 1_000.0, 6.0, -20.0), 48_000).unwrap();
        assert!((lufs - loud).abs() < 0.6, "{lufs} vs the loud passage's {loud}");
    }

    #[test]
    fn twice_the_amplitude_is_six_decibels() {
        let quiet = integrated_lufs(&sine(48_000, 1_000.0, 8.0, -30.0), 48_000).unwrap();
        let loud = integrated_lufs(&sine(48_000, 1_000.0, 8.0, -24.0), 48_000).unwrap();
        assert!((loud - quiet - 6.0).abs() < 0.05, "{quiet} -> {loud}");
    }

    /// The high pass is the point of the weighting: a rumble is not
    /// loudness, whatever its RMS says.
    #[test]
    fn a_subsonic_rumble_is_not_loudness() {
        let rumble = integrated_lufs(&sine(48_000, 20.0, 8.0, -20.0), 48_000);
        let midband = integrated_lufs(&sine(48_000, 1_000.0, 8.0, -20.0), 48_000).unwrap();
        // It may not even reach the absolute gate; if it does, it is far
        // below the mid-band tone of the same amplitude.
        match rumble {
            None => {}
            Some(rumble) => assert!(rumble < midband - 12.0, "{rumble} vs {midband}"),
        }
    }

    /// The gate is what makes this a measurement of the RECORD rather
    /// than of the silence around it.
    #[test]
    fn silence_between_passages_does_not_drag_the_measurement_down() {
        let mut pcm = sine(48_000, 1_000.0, 6.0, -20.0);
        pcm.extend(std::iter::repeat_n([0i16; 2], 48_000 * 20));
        let lufs = integrated_lufs(&pcm, 48_000).expect("a measurement");
        let alone = integrated_lufs(&sine(48_000, 1_000.0, 6.0, -20.0), 48_000).unwrap();
        assert!((lufs - alone).abs() < 0.3, "{lufs} vs {alone}");
    }

    #[test]
    fn nothing_to_measure_is_not_a_number() {
        assert!(integrated_lufs(&[], 48_000).is_none());
        assert!(integrated_lufs(&[[0; 2]; 48_000], 48_000).is_none(), "silence");
        // Too short for a single block.
        assert!(integrated_lufs(&sine(48_000, 1_000.0, 0.2, -20.0), 48_000).is_none());
    }

    #[test]
    fn the_gain_is_the_difference_and_is_bounded() {
        let gain = gain_for_target(-20.0, -14.0);
        assert!((gain - 10f32.powf(6.0 / 20.0)).abs() < 1e-5, "{gain}");
        assert!(gain_for_target(-14.0, -14.0) == 1.0);
        // A recording made at almost nothing is brought up as far as is
        // sensible and no further.
        assert!(gain_for_target(-70.0, -14.0) <= 4.0);
        assert!(gain_for_target(0.0, -14.0) >= 0.06);
        assert_eq!(gain_for_target(f64::NAN, -14.0), 1.0);
    }
}
