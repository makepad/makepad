// The striker: a felt beater or a wooden stick tip against a drum head or a
// cymbal, as a Hunt-Crossley contact integrated against the local wave
// impedance of the struck surface. This is the piano hammer's structure
// (libs/piano_model/src/hammer.rs) with the string replaced by a membrane or
// a plate, and it is where the velocity -> brightness law of every voice
// comes from: a hard blow is not a louder soft blow, the force pulse gets
// SHORTER as the impact speed rises, so its spectrum extends higher.
//
//   compression      u = x - y_local                       (m)
//   force            F = K u^p (1 + lambda du/dt),  F >= 0  (N)
//   striker          m x'' = -F
//   surface point    y_local' = F / R                       (m/s)
//
// The surface reaction depends on the surface. A CYMBAL is one-way: bronze
// is stiff and dispersive, its driving-point impedance is purely resistive
// (R = 8 sqrt(D rho h), ~80 N s/m for 1 mm bronze), the point recedes at
// F/R and the stick is gone in 0.2-0.5 ms before anything returns. A
// MEMBRANE is two-way: the head under the striker is the modal bank's own
// displacement at the strike point (y_bank = sum psi_m q_m over every mode,
// fed back with a one-sample delay), so the stick pushes the head, the
// head's modes take the momentum, the (0,1) motion carries the stick away
// and then, a quarter period later, reverses and throws it off. The
// contact spring is the head's local dimple (a Mylar head dents several mm
// under a stick), far softer than wood on Mylar, so the force is a smooth
// 3-7 ms hump on toms, ~5 ms on a snare, 10-15 ms for a kick beater that
// buries — with little energy above a few hundred hertz, which is what the
// reference drums demand: the 12" tom carries 15-20 dB less 300-2000 Hz
// energy in its first 50 ms than any sub-millisecond pulse produces.
// (The truncated modal sum stands in for the wave impedance: for the first
// ~1/f_max the modes move in phase like a small local mass, then dephase,
// which is exactly energy leaving the strike point as outgoing waves.)
//
// Boundedness: F is non-negative and clamped, the striker only ever
// decelerates while pressing, the bank is a contraction, the displacement
// feedback enters through a one-sample delay with a per-sample loop gain of
// k_contact sum(psi^2 / M) / fs^2 (< 0.01 for every membrane design; the
// rim contact of the side stick, whose spring is stiff, reads the head with
// its head_drive weight of 0.2), and a hard timeout ends any contact.
//
// With p > 1 (Hertz sphere p = 1.5; felt 2-3) the initial pulse shortens
// with speed as v^{-(p-1)/(p+1)}, pushing the force spectrum upward: a
// fortissimo hit is brighter than a pianissimo one, not just louder. The
// dissipative (1 + lambda u') term makes loading stiffer than unloading
// (hysteresis), which skews the pulse forward and rounds the rebound.
//
// Integration: symplectic Euler at NSUB substeps per audio sample. The
// stiffest contact in the kit (stick tip on a cymbal, K = 3e9, u ~ 1e-4 m)
// has a contact resonance around 12 kHz, giving w dt ~ 0.2 at 8 x 48 kHz:
// stable with a good energy balance over the ~10 substeps such a 50 us
// contact lasts (at 4 substeps the rebound energy wandered enough with
// speed that a harder bell stroke could come out quieter); the audio-rate
// force is the substep mean (a free anti-alias filter on the pulse). All
// f64: compressions of 1e-5 m to powers up to 3 are past f32's grace.

pub const NSUB: usize = 8;

#[derive(Clone, Copy)]
pub struct Striker {
    /// Effective striking mass (kg): beater head + a share of the rod, or
    /// the stick tip's dynamic mass.
    pub mass: f64,
    /// Contact stiffness K (N / m^p) and exponent p.
    pub k: f64,
    pub p: f64,
    /// Hunt-Crossley dissipation (s/m).
    pub lambda: f64,
    /// Driving-point impedance of the struck surface (N s / m).
    pub r_point: f64,
    /// Impact speed at velocity 0 and 1 (m/s); the map between is a power.
    pub v_min: f64,
    pub v_max: f64,
    pub v_curve: f64,
    /// Force clamp (N) — a safety net far above musical peaks.
    pub f_max: f64,
    /// Contact timeout (ms).
    pub timeout_ms: f64,
    /// Relaxation time (s) of the local recession into the modal bank
    /// (~a/c of the struck surface); 0 keeps the one-way contact.
    pub relax_s: f64,
    /// Constant force (N) pulling the striker back off the surface: the
    /// kick pedal's return spring. Without it a 45 g beater rides a loose
    /// head for a full half period, mass-loading the (0,1) mode down to
    /// ~25 Hz for 15 ms; the reference kick's first cycles are at 57 Hz.
    pub retract: f64,
}

impl Striker {
    pub fn speed(&self, velocity: f32) -> f64 {
        let v = (velocity.clamp(0.0, 1.0) as f64).powf(self.v_curve);
        self.v_min + (self.v_max - self.v_min) * v
    }
}

#[derive(Clone, Copy)]
pub struct Contact {
    pub active: bool,
    x: f64,
    v: f64,
    y: f64,
    relax: f64,
    u_prev: f64,
    steps: u32,
    timeout: u32,
    dt: f64,
    inv_m: f64,
    inv_r: f64,
    k: f64,
    p: f64,
    lambda: f64,
    f_max: f64,
    retract: f64,
}

impl Contact {
    pub const fn idle() -> Self {
        Self {
            active: false,
            x: 0.0,
            v: 0.0,
            y: 0.0,
            relax: 0.0,
            u_prev: 0.0,
            steps: 0,
            timeout: 0,
            dt: 0.0,
            inv_m: 0.0,
            inv_r: 0.0,
            k: 0.0,
            p: 1.0,
            lambda: 0.0,
            f_max: 0.0,
            retract: 0.0,
        }
    }

    /// Starts a strike at impact speed `v0` (m/s) with the striker just
    /// touching the surface.
    pub fn strike(&mut self, s: &Striker, v0: f64, fs: f32) {
        self.active = v0 > 0.0;
        self.x = 0.0;
        self.v = v0;
        self.y = 0.0;
        self.u_prev = 0.0;
        self.steps = 0;
        self.dt = 1.0 / (fs as f64 * NSUB as f64);
        self.relax = if s.relax_s > 0.0 { self.dt / s.relax_s } else { 0.0 };
        self.timeout = (s.timeout_ms * 1e-3 * fs as f64) as u32 + 1;
        self.inv_m = 1.0 / s.mass;
        self.inv_r = 1.0 / s.r_point;
        self.k = s.k;
        self.p = s.p;
        self.lambda = s.lambda;
        self.f_max = s.f_max;
        self.retract = s.retract;
    }

    /// One audio sample of contact: the mean force over the substeps (N),
    /// 0 when the striker has left. `y_bank` is the struck surface's
    /// displacement under the striker from the modal bank (m, positive
    /// away from the striker), 0 for a one-way contact.
    #[inline]
    pub fn step(&mut self, y_bank: f64) -> f32 {
        if !self.active {
            return 0.0;
        }
        let mut f_acc = 0.0;
        for _ in 0..NSUB {
            let u = self.x - self.y - y_bank;
            let f = if u > 0.0 {
                let du = (u - self.u_prev) / self.dt;
                let hc = (1.0 + self.lambda * du).clamp(0.2, 3.0);
                (self.k * u.powf(self.p) * hc).min(self.f_max)
            } else {
                0.0
            };
            self.u_prev = u;
            self.v -= (f + self.retract) * self.inv_m * self.dt;
            self.x += self.v * self.dt;
            self.y += f * self.inv_r * self.dt - self.y * self.relax;
            f_acc += f;
        }
        self.steps += 1;
        // The striker has left: clear of the surface and separating (or the
        // safety timeout fired).
        let u = self.x - self.y - y_bank;
        if (u < 0.0 && u < self.u_prev && self.steps > 2) || self.steps > self.timeout {
            self.active = false;
        }
        (f_acc / NSUB as f64) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pulse(s: &Striker, v0: f64, fs: f32) -> (f64, f64, Vec<f32>) {
        let mut c = Contact::idle();
        c.strike(s, v0, fs);
        let mut f = Vec::new();
        while c.active {
            f.push(c.step(0.0));
        }
        let peak = f.iter().cloned().fold(0.0f32, f32::max) as f64;
        let half = f.iter().filter(|&&v| v as f64 > 0.5 * peak).count() as f64 / fs as f64;
        (peak, half, f)
    }

    #[test]
    fn harder_hits_are_shorter_and_stronger() {
        let s = Striker {
            mass: 0.045,
            k: 2.0e8,
            p: 2.2,
            lambda: 0.05,
            r_point: 60.0,
            v_min: 1.0,
            v_max: 6.0,
            v_curve: 1.0,
            f_max: 5000.0,
            timeout_ms: 40.0,
            relax_s: 0.0,
            retract: 0.0,
        };
        let (p1, w1, _) = pulse(&s, 1.5, 48000.0);
        let (p2, w2, _) = pulse(&s, 6.0, 48000.0);
        assert!(p2 > 2.0 * p1, "{p1} {p2}");
        assert!(w2 < w1, "{w1} {w2}");
        assert!(w1 < 0.02 && w1 > 0.001, "{w1}");
    }

    #[test]
    fn pulse_is_finite_and_ends() {
        let s = Striker {
            mass: 0.008,
            k: 1.0e9,
            p: 1.5,
            lambda: 0.02,
            r_point: 80.0,
            v_min: 0.5,
            v_max: 12.0,
            v_curve: 1.0,
            f_max: 5000.0,
            timeout_ms: 10.0,
            relax_s: 0.0,
            retract: 0.0,
        };
        let (_, _, f) = pulse(&s, 12.0, 96000.0);
        assert!(f.iter().all(|v| v.is_finite() && *v >= 0.0));
        assert!(f.len() < 960);
    }
}
