// Sympathetic resonance: every string whose damper is off the string (pedal
// down, key held, or the undamped top octaves) is rung by the bridge motion
// of whatever else is sounding. One small modal bank per key (first ~12
// partials), driven one-directionally from the summed bridge force bus and
// mixed back into the soundboard. One-directional coupling forgoes the tiny
// energy the played strings would lose back through the bridge, but it makes
// instability structurally impossible: there is no loop anywhere.

use crate::keys::KeyDesign;
use crate::modal::{run_modes, KernelPath, MAX_CHUNK};

pub struct SymBank {
    pub zr: Vec<f32>,
    pub zi: Vec<f32>,
    pub eff_cr: Vec<f32>,
    pub eff_ci: Vec<f32>,
    pub active: bool,
    pub eng: f32,       // damper engagement baked into eff coeffs
    pub off_ticks: u32, // ring-out countdown once the damper re-engages
}

impl SymBank {
    pub fn new(key: &KeyDesign) -> Self {
        let n = key.sym_modes;
        let mut s = Self {
            zr: vec![0.0; n],
            zi: vec![0.0; n],
            eff_cr: vec![0.0; n],
            eff_ci: vec![0.0; n],
            active: false,
            eng: 1.0,
            off_ticks: 0,
        };
        s.rebuild(key, 1.0);
        s
    }

    /// Bake damper engagement `eng` (0 = lifted) into effective rotations.
    /// Same-angle radius interpolation: lerping (cr,ci) between the sustain
    /// rotation and the damped rotation is exact radius interpolation.
    pub fn rebuild(&mut self, key: &KeyDesign, eng: f32) {
        self.eng = eng;
        for m in 0..self.eff_cr.len() {
            let k = 1.0 + (key.sym_damp_mul[m] - 1.0) * eng;
            self.eff_cr[m] = key.sym_cr[m] * k;
            self.eff_ci[m] = key.sym_ci[m] * k;
        }
    }

    pub fn render(&mut self, key: &KeyDesign, path: KernelPath, bus: &[f32], in_gain: f32, acc: &mut [f32]) {
        debug_assert!(bus.len() <= MAX_CHUNK);
        run_modes(
            path,
            &mut self.zr,
            &mut self.zi,
            &self.eff_cr,
            &self.eff_ci,
            &key.sym_gin,
            &key.sym_gout,
            bus,
            in_gain,
            acc,
        );
    }

    pub fn clear(&mut self) {
        self.zr.fill(0.0);
        self.zi.fill(0.0);
        self.active = false;
    }
}
