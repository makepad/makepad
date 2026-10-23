//! The two-grain time stretch (Score's sample voices, a deck's key lock):
//! a read head moves through the source at one rate while the sound is read
//! at another, by two grains half a grain apart, each faded in and out so
//! their weights sum to one, each restarting at the head as it ends. The
//! head's rate is the tempo; the grains' is the pitch.
//!
//! Pure and allocation-free: the caller owns the head and reads the source
//! at the points [`Grains::next`] hands back.

/// Two grains of `size` source frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grains {
    pub size: f64,
    /// Each grain's start and how far it has read, in source frames.
    grains: [(f64, f64); 2],
}

impl Grains {
    /// Grains reading `head` at their middle (where each is loudest): the
    /// second starts there, the first half a grain later. `dir` is the
    /// sign the grains read in.
    pub fn new(size: f64, head: f64, dir: f64) -> Grains {
        let mut grains = Grains { size, grains: [(0.0, 0.0); 2] };
        grains.restart(head, dir);
        grains
    }

    /// From `head` afresh, as [`new`](Self::new).
    pub fn restart(&mut self, head: f64, dir: f64) {
        let back = head - dir.signum() * self.size * 0.5;
        self.grains = [(back, 0.0), (back, self.size * 0.5)];
    }

    /// One output frame with the head at `head`: each grain's read point
    /// and weight (the weights sum to one). The grains then move `step`
    /// source frames (its sign is the reading direction); one that ends
    /// starts again around the head.
    pub fn next(&mut self, head: f64, step: f64) -> [(f64, f32); 2] {
        self.advance(head, step, |_, _, _| 0.0)
    }

    /// As [`next`](Self::next), but a grain that starts again starts within
    /// `radius` source frames of the head where the source sounds most like
    /// what the other grain reads on from there (`read`: the source, one
    /// channel or a mix of them), so the two cross in phase instead of
    /// beating against each other.
    pub fn next_aligned(&mut self, head: f64, step: f64, radius: f64, mut read: impl FnMut(f64) -> f32) -> [(f64, f32); 2] {
        self.advance(head, step, |ideal, other, stride| {
            // Coarse over the radius, then fine around the best.
            let score = |read: &mut dyn FnMut(f64) -> f32, start: f64| {
                let (mut cross, mut energy) = (0.0f32, 0.0f32);
                for tap in 0..ALIGN_TAPS {
                    let along = tap as f64 * stride;
                    let a = read(start + along);
                    cross += a * read(other + along);
                    energy += a * a;
                }
                cross / energy.max(1e-9).sqrt()
            };
            let mut best = (ideal, score(&mut read, ideal));
            let mut offset = -radius;
            while offset <= radius {
                let value = score(&mut read, ideal + offset);
                if value > best.1 {
                    best = (ideal + offset, value);
                }
                offset += ALIGN_COARSE;
            }
            let centre = best.0;
            for fine in 1..ALIGN_COARSE as i32 {
                for at in [centre - fine as f64, centre + fine as f64] {
                    let value = score(&mut read, at);
                    if value > best.1 {
                        best = (at, value);
                    }
                }
            }
            best.0 - ideal
        })
    }

    /// One frame: the taps, then every grain moved on, a grain that ended
    /// started again `start_at(its next read point from the ideal start,
    /// the other grain's next read point, the reading stride)` frames from
    /// the ideal start.
    fn advance(&mut self, head: f64, step: f64, mut start_at: impl FnMut(f64, f64, f64) -> f64) -> [(f64, f32); 2] {
        let dir = step.signum();
        let size = self.size;
        let mut taps = [(0.0, 0.0); 2];
        for (tap, (start, phase)) in taps.iter_mut().zip(self.grains.iter_mut()) {
            let weight = 1.0 - (2.0 * (*phase / size) - 1.0).abs() as f32;
            *tap = (*start + dir * *phase, weight);
            *phase += step.abs();
        }
        for index in 0..2 {
            let phase = self.grains[index].1;
            if phase < size {
                continue;
            }
            let phase = phase - size;
            let (other_start, other_phase) = self.grains[1 - index];
            let other = other_start + dir * other_phase;
            // The ideal start reads the head at the grain's middle.
            let ideal = head - dir * size * 0.5;
            let offset = start_at(ideal + dir * phase, other, dir * step.abs() * ALIGN_SPACING);
            self.grains[index] = (ideal + offset, phase);
        }
        taps
    }
}

/// Points compared, and how many reading steps apart, when a grain looks
/// for its phase; the coarse search's stride in source frames.
const ALIGN_TAPS: usize = 96;
const ALIGN_SPACING: f64 = 2.0;
const ALIGN_COARSE: f64 = 4.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weights_sum_to_one_and_the_grains_follow_the_head() {
        let mut grains = Grains::new(100.0, 1000.0, 1.0);
        let mut head = 1000.0;
        for _ in 0..1000 {
            let taps = grains.next(head, 1.0);
            assert!((taps[0].1 + taps[1].1 - 1.0).abs() < 1e-5);
            // The head moves at half the grains' rate: the reads stay
            // within a grain of it.
            for (at, _) in taps {
                assert!((at - head).abs() <= 100.0, "{at} vs {head}");
            }
            head += 0.5;
        }
    }

    #[test]
    fn at_equal_rates_the_loud_grain_reads_the_head() {
        let mut grains = Grains::new(64.0, 500.0, 1.0);
        let taps = grains.next(500.0, 1.0);
        // Grain 1 starts at its middle: the head, at full weight.
        assert_eq!(taps[1], (500.0, 1.0));
    }

    #[test]
    fn aligned_grains_cross_in_phase() {
        // A 441 Hz tone an octave up through 40 ms grains: aligned, its
        // upward crossings are the tone's; plain, the grains beat.
        let source = |x: f64| (x / 48_000.0 * 441.0 * std::f64::consts::TAU).sin() as f32;
        let count = |aligned: bool| {
            let mut grains = Grains::new(1920.0, 0.0, 1.0);
            let (mut head, mut last, mut crossings) = (0.0, 0.0f32, 0);
            for _ in 0..48_000 {
                let taps = if aligned { grains.next_aligned(head, 2.0, 256.0, source) } else { grains.next(head, 2.0) };
                let value: f32 = taps.iter().map(|(x, w)| source(*x) * w).sum();
                if last <= 0.0 && value > 0.0 {
                    crossings += 1;
                }
                last = value;
                head += 1.0;
            }
            crossings
        };
        let aligned = count(true);
        assert!((aligned as i32 - 882).abs() <= 3, "aligned: {aligned}");
    }
}

