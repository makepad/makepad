//! The register trace played on a model of the SID (three 24-bit
//! oscillators with their waveforms, combined waveforms, ring modulation
//! and sync, the ADSR with the chip's rates, the multimode filter and the
//! master volume): the original a cover is held against. A model, not a
//! cycle-exact chip: close in pitch, timing and timbre.

use super::machine::Trace;

const ATTACK_S: [f32; 16] = [0.002, 0.008, 0.016, 0.024, 0.038, 0.056, 0.068, 0.08, 0.1, 0.25, 0.5, 0.8, 1.0, 3.0, 5.0, 8.0];
const DECAY_S: [f32; 16] = [0.006, 0.024, 0.048, 0.072, 0.114, 0.168, 0.204, 0.24, 0.3, 0.75, 1.5, 2.4, 3.0, 9.0, 15.0, 24.0];

#[derive(Clone, Copy, Default)]
struct Osc {
    acc: u32,
    lfsr: u32,
    env: f32,
    stage: u8, // 0 attack, 1 decay/sustain, 2 release
    gate: bool,
}

#[derive(Clone, Copy, Default)]
struct Filter {
    low: f32,
    band: f32,
}

/// The trace as stereo at `rate` (the three voices spread a little).
pub fn render(trace: &Trace, mos8580: bool, rate: f32) -> (Vec<f32>, Vec<f32>) {
    let frames = (trace.seconds * rate as f64) as usize;
    let mut left = vec![0.0f32; frames];
    let mut right = vec![0.0f32; frames];
    let chips = trace.chips;
    let mut regs = vec![[0u8; 25]; chips];
    let mut osc = vec![Osc { lfsr: 0x7FFFF8, ..Default::default() }; chips * 3];
    let mut filters = vec![Filter::default(); chips];
    let step_clock = trace.clock / rate as f64;
    let mut w = 0;
    for i in 0..frames {
        let t = i as f64 / rate as f64;
        while w < trace.writes.len() && trace.writes[w].t <= t {
            let wr = trace.writes[w];
            let chip = wr.chip as usize;
            regs[chip][wr.reg as usize] = wr.value;
            if wr.reg % 7 == 4 && wr.reg < 21 {
                let o = &mut osc[chip * 3 + wr.reg as usize / 7];
                let gate = wr.value & 1 != 0;
                if gate && !o.gate {
                    o.stage = 0;
                } else if !gate && o.gate {
                    o.stage = 2;
                }
                o.gate = gate;
                if wr.value & 8 != 0 {
                    o.acc = 0;
                }
            }
            w += 1;
        }
        for chip in 0..chips {
            let r = &regs[chip];
            let mut direct = 0.0f32;
            let mut through = 0.0f32;
            let mut prev_msb_rise = [false; 3];
            let mut msbs = [false; 3];
            // Advance the oscillators first (sync needs the rises).
            for v in 0..3 {
                let o = &mut osc[chip * 3 + v];
                let base = v * 7;
                let freq = r[base] as u32 | (r[base + 1] as u32) << 8;
                let before = o.acc;
                let add = (freq as f64 * step_clock) as u32;
                o.acc = (o.acc + add) & 0xFF_FFFF;
                // Noise: the shift register clocks on bit 19 rising (as
                // often as it rises in this step).
                let rises = ((before & 0x7FFFF) as u64 + add as u64) >> 19;
                for _ in 0..rises.min(16) {
                    let bit = ((o.lfsr >> 22) ^ (o.lfsr >> 17)) & 1;
                    o.lfsr = ((o.lfsr << 1) | bit) & 0x7F_FFFF;
                }
                msbs[v] = o.acc & 0x80_0000 != 0;
                prev_msb_rise[v] = before & 0x80_0000 == 0 && msbs[v];
            }
            for v in 0..3 {
                let base = v * 7;
                let ctrl = r[base + 4];
                let src = (v + 2) % 3;
                if ctrl & 2 != 0 && prev_msb_rise[src] {
                    osc[chip * 3 + v].acc = 0;
                }
                let o = &mut osc[chip * 3 + v];
                let acc = o.acc;
                let pw = (r[base + 2] as u32 | ((r[base + 3] & 15) as u32) << 8) & 0xFFF;
                // 12-bit waveforms.
                let saw = acc >> 12;
                let ring = ctrl & 4 != 0 && msbs[src];
                let msb = (acc & 0x80_0000 != 0) ^ ring;
                let tri = if msb { (!acc >> 11) & 0xFFF } else { (acc >> 11) & 0xFFF };
                let pulse = if (acc >> 12) >= pw { 0xFFF } else { 0 };
                let l = o.lfsr;
                let noise = ((l >> 22 & 1) << 11 | (l >> 20 & 1) << 10 | (l >> 16 & 1) << 9 | (l >> 13 & 1) << 8 | (l >> 11 & 1) << 7 | (l >> 7 & 1) << 6 | (l >> 4 & 1) << 5 | (l >> 2 & 1) << 4) as u32;
                let mut out: u32 = 0xFFF;
                let mut any = false;
                for (bit, wave) in [(0x10, tri), (0x20, saw), (0x40, pulse), (0x80, noise)] {
                    if ctrl & bit != 0 {
                        out &= wave;
                        any = true;
                    }
                }
                let wave = if any { out as f32 / 2048.0 - 1.0 } else { 0.0 };
                // The envelope: attack linear, decay and release falling
                // exponentially, as the chip's counters sound.
                let (a, d) = (ATTACK_S[(r[base + 5] >> 4) as usize], DECAY_S[(r[base + 5] & 15) as usize]);
                let (s, rel) = ((r[base + 6] >> 4) as f32 / 15.0, DECAY_S[(r[base + 6] & 15) as usize]);
                match o.stage {
                    0 => {
                        o.env += 1.0 / (a * rate);
                        if o.env >= 1.0 {
                            o.env = 1.0;
                            o.stage = 1;
                        }
                    }
                    1 => {
                        if o.env > s {
                            o.env = s + (o.env - s) * (-4.0 / (d * rate)).exp();
                        }
                    }
                    _ => o.env *= (-4.0 / (rel * rate)).exp(),
                }
                let y = wave * o.env;
                // Voice 3 off ($D418 bit 7) silences it unless filtered.
                let filtered = r[0x17] & (1 << v) != 0;
                if filtered {
                    through += y;
                } else if !(v == 2 && r[0x18] & 0x80 != 0) {
                    direct += y;
                }
            }
            // The filter: a state-variable 12 dB filter.
            let cut = (r[0x15] & 7) as u16 | (r[0x16] as u16) << 3;
            // (the state-variable filter is stable to about a sixth of the rate)
            let hz = super::convert::cutoff_hz(cut, mos8580).min(rate / 6.5);
            let f = 2.0 * (std::f32::consts::PI * hz / rate).sin();
            let q = 1.0 / (0.707 + (r[0x17] >> 4) as f32 / 15.0 * 1.6);
            let fl = &mut filters[chip];
            let high = through - fl.low - q * fl.band;
            fl.band = (fl.band + f * high).clamp(-8.0, 8.0);
            fl.low = (fl.low + f * fl.band).clamp(-8.0, 8.0);
            let mode = r[0x18];
            let mut filt = 0.0;
            if mode & 0x10 != 0 {
                filt += fl.low;
            }
            if mode & 0x20 != 0 {
                filt += fl.band;
            }
            if mode & 0x40 != 0 {
                filt += high;
            }
            let vol = (mode & 15) as f32 / 15.0;
            let mix = (direct + filt) * vol * 0.25;
            let pan = if chips > 1 { [0.3, 0.7, 0.5][chip % 3] } else { 0.5 };
            left[i] += mix * (1.0 - pan) * 1.4;
            right[i] += mix * pan * 1.4;
        }
    }
    (left, right)
}
