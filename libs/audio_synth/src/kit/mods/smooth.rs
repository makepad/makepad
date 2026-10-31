//! The controller smoother (spec 6.1) used by most scene-level sources
//! (mod wheel, breath, expression, sustain, channel AT, macros, ...) and by
//! MIDI-learned parameters. One step per block.

/// Smoothing mode (a user preference, not stored in patches).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SmoothMode {
    /// Speed proportional to distance; never snaps (the reference default).
    #[default]
    Legacy,
    /// Legacy that snaps within 0.0025.
    SlowExp,
    /// Faster coefficient, snaps within 0.005.
    FastExp,
    /// Constant step over ~50 blocks at 44.1 kHz, then snap.
    FastLine,
    /// No smoothing.
    Direct,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerSmoother {
    pub mode: SmoothMode,
    target: f32,
    value: f32,
    start: f32,
}

impl ControllerSmoother {
    pub fn new(mode: SmoothMode) -> ControllerSmoother {
        ControllerSmoother { mode, ..ControllerSmoother::default() }
    }

    /// The smoother for MIDI-learned parameters: Legacy promoted to SlowExp.
    pub fn for_learned(mode: SmoothMode) -> ControllerSmoother {
        ControllerSmoother::new(if mode == SmoothMode::Legacy { SmoothMode::SlowExp } else { mode })
    }

    pub fn set_target(&mut self, t: f32) {
        if t != self.target {
            self.start = self.value;
            self.target = t;
        }
    }

    /// Jump to `t` (load, reset).
    pub fn snap(&mut self, t: f32) {
        self.target = t;
        self.value = t;
        self.start = t;
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.value
    }

    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }

    /// Whether the value is within `eps` of the target (learned parameters
    /// stop smoothing within 0.001).
    #[inline]
    pub fn settled(&self, eps: f32) -> bool {
        (self.target - self.value).abs() < eps
    }

    /// One block at host rate `fs`.
    pub fn process(&mut self, fs: f32) -> f32 {
        let t = self.target;
        let v = self.value;
        let b = (t - v).abs();
        let exp = |coef: f32| (coef * (44100.0 / fs) * b).clamp(0.0, 1.0);
        self.value = match self.mode {
            SmoothMode::Direct => t,
            SmoothMode::Legacy => v + exp(0.9) * (t - v),
            SmoothMode::SlowExp => {
                if b < 0.0025 { t } else { v + exp(0.9) * (t - v) }
            }
            SmoothMode::FastExp => {
                if b < 0.005 { t } else { v + exp(0.99) * (t - v) }
            }
            SmoothMode::FastLine => {
                let step = (t - self.start) / (50.0 * fs / 44100.0);
                if b <= step.abs() { t } else { v + step }
            }
        };
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        let fs = 44100.0;
        let mut s = ControllerSmoother::new(SmoothMode::Legacy);
        s.set_target(1.0);
        s.process(fs);
        assert!((s.value() - 0.9).abs() < 1e-6);
        for _ in 0..10_000 {
            s.process(fs);
        }
        assert!(s.value() < 1.0 && s.value() > 0.99);
        let mut s = ControllerSmoother::new(SmoothMode::SlowExp);
        s.set_target(1.0);
        for _ in 0..10_000 {
            s.process(fs);
        }
        assert_eq!(s.value(), 1.0);
        let mut s = ControllerSmoother::new(SmoothMode::FastLine);
        s.set_target(1.0);
        let mut n = 0;
        while s.value() != 1.0 {
            s.process(fs);
            n += 1;
        }
        assert!((49..=51).contains(&n), "{n}");
        let mut s = ControllerSmoother::new(SmoothMode::Direct);
        s.set_target(0.3);
        assert_eq!(s.process(fs), 0.3);
        assert_eq!(ControllerSmoother::for_learned(SmoothMode::Legacy).mode, SmoothMode::SlowExp);
    }
}
