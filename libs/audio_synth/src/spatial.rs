//! 3D placement of a mono source for a stereo listener: distance model, air
//! absorption, equal-power pan with interaural time and level differences
//! (a spherical-head approximation of an HRTF), a front/back pinna cue,
//! doppler, and an occlusion low-pass hook.
//!
//! [`place`] is the geometry (control rate, once per block); [`Spatial`] is
//! the per-voice signal path that glides to what `place` computed, so a
//! voice moving across the head never zippers or clicks.

use crate::dsp::{finite, OnePole, Smooth};

pub type V3 = [f32; 3];

#[inline]
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Speed of sound, m/s.
pub const SPEED_OF_SOUND: f32 = 343.0;
/// Half the interaural delay across a ~17.5 cm head, seconds.
const MAX_ITD: f32 = 0.00066;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener3 {
    pub pos: V3,
    pub forward: V3,
    pub right: V3,
    pub up: V3,
    pub vel: V3,
}

impl Default for Listener3 {
    fn default() -> Self {
        Listener3 {
            pos: [0.0; 3],
            forward: [0.0, 0.0, -1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            vel: [0.0; 3],
        }
    }
}

/// How a source falls off with distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distance {
    /// Full level inside this radius (the source's own size), metres.
    pub near: f32,
    /// Silent at and beyond this, metres (a smooth fade into it).
    pub range: f32,
    /// 1 = inverse distance (physical, −6 dB per doubling); lower is gentler.
    pub rolloff: f32,
}

impl Default for Distance {
    fn default() -> Self {
        Distance { near: 1.0, range: 40.0, rolloff: 1.0 }
    }
}

impl Distance {
    pub fn gain(&self, d: f32) -> f32 {
        let range = self.range.max(0.01);
        if d >= range {
            return 0.0;
        }
        let near = self.near.clamp(0.01, range);
        let inv = if d <= near { 1.0 } else { near / (near + self.rolloff * (d - near)) };
        let t = d / range;
        // Fade the last stretch to exactly zero so culling is inaudible.
        let fade = (1.0 - t * t * t * t).max(0.0);
        inv * fade
    }
}

/// What a source should sound like from where the listener is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement3 {
    pub gain: f32,
    /// -1 hard left .. +1 hard right (sine of the azimuth).
    pub pan: f32,
    /// 1 straight ahead .. -1 straight behind.
    pub front: f32,
    pub distance: f32,
    /// Playback-rate factor from relative radial velocity.
    pub doppler: f32,
    /// Air absorption corner, Hz.
    pub air_hz: f32,
}

impl Placement3 {
    pub const FLAT: Placement3 =
        Placement3 { gain: 1.0, pan: 0.0, front: 1.0, distance: 0.0, doppler: 1.0, air_hz: 20_000.0 };
}

/// Geometry for a source at `pos` moving at `vel`.
pub fn place(listener: &Listener3, pos: V3, vel: V3, dist: &Distance, doppler_scale: f32) -> Placement3 {
    let d = sub(pos, listener.pos);
    let distance = dot(d, d).sqrt();
    let gain = dist.gain(distance);
    if distance < 1.0e-3 {
        return Placement3 { gain, ..Placement3::FLAT };
    }
    let inv = 1.0 / distance;
    let dir = [d[0] * inv, d[1] * inv, d[2] * inv];
    let lateral = dot(dir, listener.right);
    let front = dot(dir, listener.forward);
    // Near field: the pan settles to centre as the source reaches the head,
    // so something at your feet does not flip channels.
    let near = (distance / dist.near.max(0.25)).min(1.0);
    let pan = (lateral * near).clamp(-1.0, 1.0);
    // Doppler: positive radial speed = moving apart. Clamp so a teleport or
    // a respawn never shrieks.
    let v_src = dot(vel, dir);
    let v_lis = dot(listener.vel, dir);
    let doppler = if doppler_scale > 0.0 {
        let c = SPEED_OF_SOUND;
        let ratio = (c + v_lis * doppler_scale) / (c + v_src * doppler_scale);
        finite(ratio, 1.0).clamp(0.5, 2.0)
    } else {
        1.0
    };
    // ISO 9613-ish: high frequencies die first. ~10 kHz at 100 m, ~3 kHz at
    // 600 m.
    let air_hz = (20_000.0 / (1.0 + distance / 90.0)).max(1_500.0);
    Placement3 { gain, pan, front, distance, doppler, air_hz }
}

/// Per-voice spatial processor: mono in, stereo out.
#[derive(Clone, Copy)]
pub struct Spatial {
    rate: f32,
    gain: Smooth,
    pan: Smooth,
    front: Smooth,
    air_hz: f32,
    occlusion: Smooth,
    air: OnePole,
    shadow_l: OnePole,
    shadow_r: OnePole,
    pinna: OnePole,
    itd: [f32; 128],
    itd_at: usize,
    coef: f32,
    ctl_count: u32,
    /// Pan-law gains, recomputed at control rate from the gliding pan.
    gl: f32,
    gr: f32,
    /// Width of the pan (0 = mono, 1 = full). Interior/2D voices use less.
    pub width: f32,
    /// Skip all HRTF-ish filtering (2D UI sounds): plain equal-power pan.
    pub flat: bool,
}

impl Spatial {
    pub fn new(rate: f32) -> Self {
        let rate = finite(rate, 48_000.0).clamp(8_000.0, 384_000.0);
        Spatial {
            rate,
            gain: Smooth::new(0.0),
            pan: Smooth::new(0.0),
            front: Smooth::new(1.0),
            air_hz: 20_000.0,
            occlusion: Smooth::new(0.0),
            air: OnePole::new(20_000.0, rate),
            shadow_l: OnePole::new(20_000.0, rate),
            shadow_r: OnePole::new(20_000.0, rate),
            pinna: OnePole::new(9_000.0, rate),
            itd: [0.0; 128],
            itd_at: 0,
            coef: Smooth::coef(0.012, rate),
            ctl_count: 0,
            gl: core::f32::consts::FRAC_1_SQRT_2,
            gr: core::f32::consts::FRAC_1_SQRT_2,
            width: 1.0,
            flat: false,
        }
    }

    /// Start at a placement without gliding from silence/centre (a new voice).
    pub fn snap(&mut self, p: &Placement3, occlusion: f32) {
        self.set(p, occlusion);
        self.gain.value = self.gain.target;
        self.pan.value = self.pan.target;
        self.front.value = self.front.target;
        self.occlusion.value = self.occlusion.target;
        self.ctl_count = 0;
        let a = (self.pan.value + 1.0) * core::f32::consts::FRAC_PI_4;
        self.gl = a.cos();
        self.gr = a.sin();
    }

    pub fn set(&mut self, p: &Placement3, occlusion: f32) {
        self.gain.target = finite(p.gain, 0.0).max(0.0);
        self.pan.target = finite(p.pan, 0.0).clamp(-1.0, 1.0) * self.width;
        self.front.target = finite(p.front, 1.0).clamp(-1.0, 1.0);
        self.air_hz = finite(p.air_hz, 20_000.0);
        self.occlusion.target = finite(occlusion, 0.0).clamp(0.0, 1.0);
    }

    pub fn gain(&self) -> f32 {
        self.gain.value.max(self.gain.target)
    }

    /// One sample through the head.
    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        let g = self.gain.next(self.coef);
        let pan = self.pan.next(self.coef);
        // Control rate (every 16 samples): the pan law and filter retunes.
        let ctl = self.ctl_count == 0;
        self.ctl_count = (self.ctl_count + 1) % 16;
        if ctl {
            let a = (pan + 1.0) * core::f32::consts::FRAC_PI_4;
            self.gl = a.cos();
            self.gr = a.sin();
        }
        let (gl, gr) = (self.gl, self.gr);
        if self.flat {
            return (x * g * gl, x * g * gr);
        }
        let front = self.front.next(self.coef);
        let occ = self.occlusion.next(self.coef);
        if ctl {
            let air = self.air_hz * (1.0 - 0.85 * occ);
            self.air.set(air.max(300.0), self.rate);
            // Far ear: head shadow deepens with |azimuth|.
            let s = pan.abs();
            let far = 20_000.0 * (1.0 - s) + 1_800.0 * s;
            if pan >= 0.0 {
                self.shadow_l.set(far, self.rate);
                self.shadow_r.set(20_000.0, self.rate);
            } else {
                self.shadow_r.set(far, self.rate);
                self.shadow_l.set(20_000.0, self.rate);
            }
            self.pinna.set(if front < 0.0 { 9_000.0 + 11_000.0 * (1.0 + front) } else { 20_000.0 }, self.rate);
        }
        let y = self.pinna.lp(self.air.lp(x)) * g;
        // Interaural delay on the far ear.
        self.itd[self.itd_at] = y;
        let delay = (pan.abs() * MAX_ITD * self.rate).min(126.0);
        let di = delay as usize;
        let frac = delay - di as f32;
        let i0 = (self.itd_at + 128 - di) & 127;
        let i1 = (self.itd_at + 128 - di - 1) & 127;
        let delayed = self.itd[i0] * (1.0 - frac) + self.itd[i1] * frac;
        self.itd_at = (self.itd_at + 1) & 127;
        let (l_in, r_in) = if pan >= 0.0 { (delayed, y) } else { (y, delayed) };
        (self.shadow_l.lp(l_in) * gl, self.shadow_r.lp(r_in) * gr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_falls_off_and_reaches_zero_at_range() {
        let d = Distance { near: 1.0, range: 100.0, rolloff: 1.0 };
        assert_eq!(d.gain(0.5), 1.0);
        assert!(d.gain(2.0) < 0.6 && d.gain(2.0) > 0.4);
        assert!(d.gain(50.0) < d.gain(10.0));
        assert_eq!(d.gain(100.0), 0.0);
    }

    #[test]
    fn a_source_approaching_is_pitched_up_and_receding_down() {
        let l = Listener3::default();
        let dist = Distance { near: 1.0, range: 500.0, rolloff: 1.0 };
        // Ahead at -z, moving toward the listener (+z) at 30 m/s.
        let coming = place(&l, [0.0, 0.0, -50.0], [0.0, 0.0, 30.0], &dist, 1.0);
        let going = place(&l, [0.0, 0.0, -50.0], [0.0, 0.0, -30.0], &dist, 1.0);
        assert!(coming.doppler > 1.05, "{}", coming.doppler);
        assert!(going.doppler < 0.95, "{}", going.doppler);
    }

    #[test]
    fn right_is_louder_in_the_right_ear_and_arrives_first() {
        let l = Listener3::default();
        let p = place(&l, [10.0, 0.0, 0.0], [0.0; 3], &Distance::default(), 1.0);
        assert!(p.pan > 0.9);
        let mut s = Spatial::new(48_000.0);
        s.snap(&p, 0.0);
        let mut first_l = None;
        let mut first_r = None;
        let (mut el, mut er) = (0.0, 0.0);
        for i in 0..2000 {
            let x = if i == 100 { 1.0 } else { 0.0 };
            let (a, b) = s.process(x);
            if first_l.is_none() && a.abs() > 1.0e-4 {
                first_l = Some(i);
            }
            if first_r.is_none() && b.abs() > 1.0e-4 {
                first_r = Some(i);
            }
            el += a * a;
            er += b * b;
        }
        assert!(er > el * 4.0, "{er} vs {el}");
        assert!(first_r.unwrap() < first_l.unwrap_or(usize::MAX));
    }
}
