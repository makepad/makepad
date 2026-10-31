// Lookup tables of the nimbus granular texture engine: generated at first use
// from the formulas of the MIT generator resources/lookup_tables.py of the
// eurorack texture synthesizer (sin, window, xfade_in/out, sine_window_4096,
// grain_size), the mu-law decode table from the formula kept in dsp/mu_law.h,
// and the 45-tap 2x resampling filter of resources.cc ported as data (its
// generator, resources/src_filters.py, needs a Remez design we do not have).
// Original notices:
//
// Copyright 2014 Emilie Gillet. (resources/lookup_tables.py, resources/src_filters.py,
//                                resources.cc, dsp/mu_law.h, dsp/mu_law.cc)
//
// Author: Emilie Gillet (emilie.o.gillet@gmail.com)
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.
//
// See http://creativecommons.org/licenses/MIT/ for more information.
//
// -----------------------------------------------------------------------------

use std::f64::consts::PI;
use std::sync::OnceLock;

pub struct Tables {
    /// sin over 1.25 turns + 1 guard, 1024 steps per turn (1281).
    pub sin: Vec<f32>,
    /// Raised-cosine grain window, 4097.
    pub window: Vec<f32>,
    pub xfade_in: [f32; 17],
    pub xfade_out: [f32; 17],
    /// Power window for the 4x-overlap STFT, 4096.
    pub sine_window_4096: Vec<f32>,
    pub grain_size: Vec<f32>,
    /// Mu-law byte to 16-bit linear.
    pub ulaw: [i16; 256],
}

#[inline]
pub fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(build)
}

fn build() -> Tables {
    let sin = (0..1024 + 256 + 1).map(|i| (i as f64 / 1024.0 * PI * 2.0).sin() as f32).collect();
    let window = (0..=4096)
        .map(|i| {
            let t = i as f64 / 4096.0;
            (1.0 - ((t * PI).cos() + 1.0) / 2.0) as f32
        })
        .collect();
    let mut xfade_in = [0.0f32; 17];
    let mut xfade_out = [0.0f32; 17];
    for i in 0..17 {
        let mut t = i as f64 / 16.0;
        t = 1.04 * t - 0.02;
        if t < 0.0 {
            t = 0.0;
        }
        if t >= 1.0 {
            t = 1.0;
        }
        t *= PI / 2.0;
        xfade_in[i] = (t.sin() * 2f64.powf(-0.5)) as f32;
        xfade_out[i] = (t.cos() * 2f64.powf(-0.5)) as f32;
    }
    // Power window, compensated for perfect-ish reconstruction at overlap 2.
    let n = 4096;
    let power: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / n as f64;
            (1.0 - (2.0 * t - 1.0).powi(2)).powf(1.25)
        })
        .collect();
    let half = n / 2;
    let sine_window_4096 = (0..n)
        .map(|i| {
            let j = i % half;
            let comp = (power[j] * power[j] + power[j + half] * power[j + half]).sqrt();
            (power[i] / comp) as f32
        })
        .collect();
    let grain_size = (0..=256).map(|i| (1024.0 * 2f64.powf(i as f64 / 256.0 * 4.0)).floor() as f32).collect();
    let mut ulaw = [0i16; 256];
    for (i, u) in ulaw.iter_mut().enumerate() {
        let u_val = !(i as u8);
        let mut t: i32 = (((u_val & 0xf) as i32) << 3) + 0x84;
        t <<= ((u_val as u32) & 0x70) >> 4;
        *u = if u_val & 0x80 != 0 { (0x84 - t) as i16 } else { (t - 0x84) as i16 };
    }
    Tables { sin, window, xfade_in, xfade_out, sine_window_4096, grain_size, ulaw }
}

/// `Interpolate(table, index, size)`. At index = 1 the original reads one
/// entry past its tables (times a zero fraction); here that entry is the last.
#[inline]
pub fn interp(table: &[f32], index: f32, size: f32) -> f32 {
    let x = index * size;
    let i = x as i32;
    let f = x - i as f32;
    let i = (i.max(0) as usize).min(table.len() - 1);
    let a = table[i];
    let b = if i + 1 < table.len() { table[i + 1] } else { a };
    a + (b - a) * f
}

/// 45-tap half-band-ish lowpass for the 2x down/up converters (data).
pub static SRC_FILTER_1X_2_45: [f32; 45] = [
    -6.928606892e-04, -5.894682972e-03, 4.393903915e-04, 5.352009980e-03,
    1.833575577e-03, -7.103853054e-03, -5.275577768e-03, 7.999060050e-03,
    1.029879712e-02, -7.191125897e-03, -1.675763381e-02, 3.628265970e-03,
    2.423749384e-02, 4.020326715e-03, -3.208822586e-02, -1.775516900e-02,
    3.947412082e-02, 4.200610725e-02, -4.553678524e-02, -9.270618476e-02,
    4.952442102e-02, 3.157869177e-01, 4.528032253e-01, 3.157869177e-01,
    4.952442102e-02, -9.270618476e-02, -4.553678524e-02, 4.200610725e-02,
    3.947412082e-02, -1.775516900e-02, -3.208822586e-02, 4.020326715e-03,
    2.423749384e-02, 3.628265970e-03, -1.675763381e-02, -7.191125897e-03,
    1.029879712e-02, 7.999060050e-03, -5.275577768e-03, -7.103853054e-03,
    1.833575577e-03, 5.352009980e-03, 4.393903915e-04, -5.894682972e-03,
    -6.928606892e-04,
];

#[cfg(test)]
mod tests {
    use super::*;

    // Spot values of the original's pre-computed tables.
    #[test]
    fn generated_tables_match_original_spots() {
        let t = tables();
        assert_eq!(t.sin.len(), 1281);
        assert_eq!(t.window.len(), 4097);
        assert_eq!(t.sine_window_4096.len(), 4096);
        assert_eq!(t.grain_size.len(), 257);
        assert_eq!(t.ulaw[0], -32124);
        assert_eq!(t.ulaw[127], 0);
        assert_eq!(t.ulaw[128], 32124);
        assert_eq!(t.ulaw[200], 1372);
        assert_eq!(t.grain_size[3], 1057.0);
        assert!((t.sine_window_4096[1] - 1.725808584e-04).abs() < 1e-8);
        assert!((t.sine_window_4096[3] - 6.809730624e-04).abs() < 1e-8);
        assert!((t.xfade_in[1] - 4.994082069e-02).abs() < 1e-7);
        assert!((t.xfade_in[9] - 5.483582988e-01).abs() < 1e-7);
        assert_eq!(t.grain_size[0], 1024.0);
        assert_eq!(t.grain_size[256], 16384.0);
        assert!((t.xfade_in[16] - 0.70710678).abs() < 1e-6);
        assert!(t.xfade_out[16].abs() < 1e-6);
        assert!((t.window[2048] - 0.5).abs() < 1e-6);
    }
}
