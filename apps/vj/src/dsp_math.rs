//! The arithmetic the audio and analysis code kept writing out again.
//!
//! Nothing here is new: every function is a form that already appeared in
//! two or more modules, lifted so there is one of it. That is the whole
//! point, so each one keeps the EXACT expression its callers had — a
//! rounding difference here would move a golden buffer or an analysis
//! result, and the change would look like a bug in whatever noticed it.

/// A ratio as decibels, with a floor instead of an infinity at silence.
///
/// The floor is 1e-12, about -240 dB: far below anything audible, and low
/// enough that a meter reading it says "silence" rather than "broken".
#[inline]
pub fn ratio_to_db(ratio: f32) -> f32 {
    20.0 * ratio.max(1e-12).log10()
}

/// The same reading at double precision, for the measurements that work
/// there.
#[inline]
pub fn ratio_to_db_f64(ratio: f64) -> f64 {
    20.0 * ratio.max(1e-12).log10()
}

/// Decibels back to a ratio.
#[inline]
pub fn db_to_ratio(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

/// A 16-bit stereo frame as one number in -1..1.
///
/// The halving and the scaling are both exact powers of two, so this is the
/// same value the callers computed inline, to the bit.
#[inline]
pub fn mono(frame: [i16; 2]) -> f32 {
    (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0
}

/// The same fold at double precision, for the analysis that works there.
#[inline]
pub fn mono_f64(frame: [i16; 2]) -> f64 {
    (frame[0] as f64 + frame[1] as f64) * 0.5 / 32768.0
}

/// The stereo pair to take from one interleaved frame of any width.
///
/// The first PAIR, and not the first channel with the last -- which is
/// what three separate decode paths did. It is right for mono, right for
/// stereo, and wrong for everything else: every interleaving order in use
/// puts the front pair first, so the LAST channel of a six-channel file is
/// a rear or the low-frequency effects send. A film soundtrack played
/// through it came out as front-left in one ear and the subwoofer feed in
/// the other.
///
/// Mono duplicates rather than going silent on one side. `None` only for
/// a frame with no samples in it at all, which is a caller error rather
/// than a channel layout.
#[inline]
pub fn stereo_pair<T: Copy>(frame: &[T]) -> Option<[T; 2]> {
    match (frame.first(), frame.get(1)) {
        (Some(&left), Some(&right)) => Some([left, right]),
        (Some(&mono), None) => Some([mono, mono]),
        _ => None,
    }
}

/// `a` towards `b` by `t`.
///
/// Written as `a + (b - a) * t` rather than `a * (1 - t) + b * t`: the two
/// disagree in the last bit, and the callers -- the loop-wrap and seek
/// crossfades among them -- were all written the first way.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The same blend on a stereo frame.
#[inline]
pub fn lerp_frame(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t)]
}

/// The periodic Hann window at one index.
///
/// Periodic, not symmetric: `1 - cos` over `len` rather than `len - 1`, so
/// two of these at 50% overlap sum to exactly one and an overlap-add leaves
/// no ripple.
#[inline]
pub fn hann(index: usize, len: usize) -> f32 {
    0.5 - 0.5 * (2.0 * std::f32::consts::PI * index as f32 / len as f32).cos()
}

/// The same window at double precision.
#[inline]
pub fn hann_f64(index: usize, len: usize) -> f64 {
    0.5 - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / len as f64).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ratio_becomes_decibels_and_comes_back() {
        assert!((ratio_to_db(1.0) - 0.0).abs() < 1e-6, "unity is 0 dB");
        assert!((ratio_to_db(0.5) + 6.0206).abs() < 1e-3, "half is about -6 dB");
        assert!((db_to_ratio(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_ratio(6.0206) - 2.0).abs() < 1e-3);
        for ratio in [0.001f32, 0.1, 0.7071, 1.0, 2.0] {
            let round_trip = db_to_ratio(ratio_to_db(ratio));
            assert!((round_trip - ratio).abs() < ratio * 1e-4, "{ratio} came back {round_trip}");
        }
    }

    #[test]
    fn both_precisions_read_the_same_ratio_the_same_way() {
        for ratio in [1.0f64, 0.5, 0.001, 2.0] {
            let wide = ratio_to_db_f64(ratio);
            let narrow = ratio_to_db(ratio as f32) as f64;
            assert!((wide - narrow).abs() < 1e-4, "{ratio}: {wide} vs {narrow}");
        }
        assert!(ratio_to_db_f64(0.0).is_finite(), "silence has a floor here too");
    }

    #[test]
    fn silence_has_a_floor_rather_than_an_infinity() {
        assert!(ratio_to_db(0.0).is_finite(), "a log of zero would be -inf");
        assert!(ratio_to_db(0.0) < -200.0, "and it is still unmistakably silence");
    }

    #[test]
    fn a_stereo_pair_folds_to_one_full_scale_number() {
        assert!((mono([32767, 32767]) - 0.99997).abs() < 1e-4, "full scale is about 1");
        assert!((mono([-32768, -32768]) + 1.0).abs() < 1e-6, "and the floor is exactly -1");
        assert_eq!(mono([0, 0]), 0.0);
        assert_eq!(mono([16384, -16384]), 0.0, "opposite channels cancel");
        assert!((mono([16384, 0]) - 0.25).abs() < 1e-6, "one channel at half is a quarter");
    }

    #[test]
    fn the_fold_is_the_expression_its_callers_already_used() {
        // Bit-exact, not merely close: the analysis cache is keyed on the
        // numbers this produces.
        for pair in [[0i16, 0], [1, -1], [12345, -321], [32767, -32768], [-4, 9001]] {
            let had = (pair[0] as f32 + pair[1] as f32) * 0.5 / 32768.0;
            assert_eq!(mono(pair), had, "{pair:?}");
            let had64 = (pair[0] as f64 + pair[1] as f64) * 0.5 / 32768.0;
            assert_eq!(mono_f64(pair), had64, "{pair:?}");
        }
    }

    #[test]
    fn a_blend_lands_on_its_ends_and_halfway_between() {
        assert_eq!(lerp(2.0, 6.0, 0.0), 2.0);
        assert_eq!(lerp(2.0, 6.0, 1.0), 6.0);
        assert_eq!(lerp(2.0, 6.0, 0.5), 4.0);
        assert_eq!(lerp_frame([0.0, 1.0], [1.0, 0.0], 0.25), [0.25, 0.75]);
    }

    #[test]
    fn the_blend_is_the_expression_the_mixer_already_used() {
        // `a + (b - a) * t`, not `a * (1 - t) + b * t`: the two disagree in
        // the last bit, and one of the callers is the loop-wrap crossfade.
        for (a, b, t) in [(0.1f32, 0.7, 0.3), (-1.0, 1.0, 0.9), (0.5, 0.5, 0.5)] {
            assert_eq!(lerp(a, b, t), a + (b - a) * t);
        }
    }

    #[test]
    fn the_window_is_periodic_so_two_at_half_overlap_sum_to_one() {
        const N: usize = 8;
        assert_eq!(hann(0, N), 0.0, "a periodic window starts at zero");
        for index in 0..N / 2 {
            let sum = hann(index, N) + hann(index + N / 2, N);
            assert!((sum - 1.0).abs() < 1e-6, "index {index} summed to {sum}");
        }
    }

    #[test]
    fn the_window_is_the_expression_its_callers_already_used() {
        for (index, len) in [(0usize, 1024usize), (1, 1024), (511, 1024), (777, 2048)] {
            let had = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * index as f32 / len as f32).cos();
            assert_eq!(hann(index, len), had, "{index}/{len}");
            let had64 =
                0.5 - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / len as f64).cos();
            assert_eq!(hann_f64(index, len), had64, "{index}/{len}");
        }
    }
    /// The front pair, whatever the file's width.
    ///
    /// The defect this replaces: three decode paths took the first channel
    /// and the LAST one. Right for mono and stereo, and wrong for
    /// everything else -- a six-channel film soundtrack came out as
    /// front-left in one ear and the low-frequency effects send in the
    /// other, which is a channel nobody has ever wanted in a headphone.
    #[test]
    fn a_wide_frame_gives_up_its_front_pair_and_not_its_last_channel() {
        // Front L, front R, centre, LFE, rear L, rear R.
        let surround = [10i16, 20, 30, 40, 50, 60];
        assert_eq!(stereo_pair(&surround), Some([10, 20]));
        assert_eq!(stereo_pair(&[7i16, 8]), Some([7, 8]), "stereo is itself");
        assert_eq!(stereo_pair(&[7i16]), Some([7, 7]), "mono goes to both ears");
        assert_eq!(stereo_pair(&[1.0f32, 2.0, 3.0]), Some([1.0, 2.0]), "and at any type");
        assert_eq!(stereo_pair::<i16>(&[]), None, "no samples is not a layout");
    }
}
