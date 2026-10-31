//! The module as a tracker plays it, straight from the replay (each note's
//! sample at its pitch, level and pan every tick, mixed dry): the original
//! a cover through the Score is held against.

use super::replay::Played;
use super::Module;

/// Stereo at `rate`.
pub struct Audio {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub rate: f32,
}

/// The seconds at each beat of a played module's tempo map.
pub fn seconds_at(played: &Played, beat: f64) -> f64 {
    let mut t = 0.0;
    let mut at = 0.0;
    let mut bpm = played.tempo.first().map_or(125.0, |t| t.1);
    let last_row = played.end - 1.0 / played.rows_per_beat as f64 - 1e-9;
    for &(b, next) in played.tempo.iter().skip(1) {
        if b >= beat || b >= last_row {
            break;
        }
        let next = next.clamp(20.0, 400.0);
        t += (b - at) * 60.0 / bpm;
        at = b;
        bpm = next;
    }
    t + (beat - at) * 60.0 / bpm
}

pub fn render(m: &Module, played: &Played, rate: f32) -> Audio {
    let total = (seconds_at(played, played.end) * rate as f64) as usize + rate as usize;
    let mut left = vec![0.0f32; total];
    let mut right = vec![0.0f32; total];
    for n in &played.notes {
        let s = &m.samples[n.sample];
        if s.left.len() < 2 {
            continue;
        }
        let lp = s.sustain.or(s.looped);
        let mut pos = n.offset as f64;
        let end_frame = ((seconds_at(played, n.end)) * rate as f64) as usize;
        for (i, t) in n.ticks.iter().enumerate() {
            let from = (seconds_at(played, t.beat) * rate as f64) as usize;
            let to = n.ticks.get(i + 1).map_or(end_frame, |u| (seconds_at(played, u.beat) * rate as f64) as usize).min(end_frame).min(total);
            let pitch = t.pitch + t.vibrato + t.arpeggio;
            let step = s.rate * 2f64.powf((pitch as f64 - 60.0) / 12.0) / rate as f64;
            let p = (t.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            let (gl, gr) = (p.cos() * t.level, p.sin() * t.level);
            for f in from..to {
                if let Some(l) = lp {
                    if pos >= l.to as f64 {
                        pos = l.from as f64 + (pos - l.to as f64) % ((l.to - l.from).max(1) as f64);
                    }
                }
                let at = pos as usize;
                if at + 1 >= s.left.len() {
                    break;
                }
                let frac = (pos - at as f64) as f32;
                let a = s.left[at] + (s.left[at + 1] - s.left[at]) * frac;
                let b = s.right.as_ref().map_or(a, |r| r[at] + (r[at + 1] - r[at]) * frac);
                left[f] += a * gl;
                right[f] += b * gr;
                pos += step;
            }
        }
    }
    // A plain gain to keep the peaks under full scale.
    let peak = left.iter().chain(right.iter()).fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let g = 0.9 / peak;
    left.iter_mut().chain(right.iter_mut()).for_each(|v| *v *= g);
    Audio { left, right, rate }
}
