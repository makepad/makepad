// Rust port of stmlib fft/shy_fft.h (real FFT after Laurent de Soras'
// FFTReal) as the nimbus spectral mode uses it: float, 4096 points, rotation
// phasor. Output layout: re[0..n/2], im at [n/2 + k]. Original notice:
//
// Copyright 2014 Emilie Gillet. (stmlib fft/shy_fft.h)
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

pub const FFT_SIZE: usize = 4096;
const NUM_PASSES: usize = 12;
const SQRT_2_DIV_2: f32 = 0.707_106_77;

/// 8-bit bit reversal.
fn bit_rev_256() -> [u8; 256] {
    let mut t = [0u8; 256];
    for (i, v) in t.iter_mut().enumerate() {
        *v = (i as u8).reverse_bits();
    }
    t
}

#[derive(Clone)]
pub struct ShyFft {
    sin_cos_lut: [f32; (NUM_PASSES - 3) * 2],
    cos: f32,
    sin: f32,
    real: f32,
    imag: f32,
    bit_rev: [u8; 256],
}

impl Default for ShyFft {
    fn default() -> Self {
        let mut f = ShyFft { sin_cos_lut: [0.0; (NUM_PASSES - 3) * 2], cos: 0.0, sin: 0.0, real: 0.0, imag: 0.0, bit_rev: bit_rev_256() };
        f.init();
        f
    }
}

impl ShyFft {
    pub fn init(&mut self) {
        for pass in 3..NUM_PASSES {
            let index = (pass - 3) << 1;
            let angle = std::f32::consts::PI / (1u32 << pass) as f32;
            self.sin_cos_lut[index] = angle.cos();
            self.sin_cos_lut[index + 1] = angle.sin();
        }
    }

    #[inline]
    fn start(&mut self, pass: usize) {
        let index = (pass - 3) << 1;
        self.cos = self.sin_cos_lut[index];
        self.real = self.cos;
        self.sin = self.sin_cos_lut[index + 1];
        self.imag = self.sin;
    }

    #[inline]
    fn rotate(&mut self) {
        let temp = self.cos * self.real - self.sin * self.imag;
        self.sin = self.cos * self.imag + self.sin * self.real;
        self.cos = temp;
    }

    #[inline]
    fn r0(&self, i: usize) -> usize {
        (((self.bit_rev[i & 0xff] as usize) << 8) | self.bit_rev[i >> 8] as usize) >> (16 - NUM_PASSES)
    }

    /// Forward transform; `input` is used as workspace (lost).
    pub fn direct(&mut self, input: &mut [f32], output: &mut [f32]) {
        let size = FFT_SIZE;
        // First and second pass.
        for i in (0..size).step_by(4) {
            let s = &*input;
            let r0 = self.r0(i);
            let r1 = r0 + 2 * (size >> 2);
            let r2 = r0 + (size >> 2);
            let r3 = r0 + 3 * (size >> 2);
            let d = &mut output[i..i + 4];
            d[1] = s[r0] - s[r1];
            d[3] = s[r2] - s[r3];
            let a = s[r0] + s[r1];
            let b = s[r2] + s[r3];
            d[0] = a + b;
            d[2] = a - b;
        }
        // Third pass: output -> input.
        {
            let s = &*output;
            let d = &mut *input;
            for i in (0..size).step_by(8) {
                d[i] = s[i] + s[i + 4];
                d[i + 4] = s[i] - s[i + 4];
                d[i + 2] = s[i + 2];
                d[i + 6] = s[i + 6];
                let v = (s[i + 5] - s[i + 7]) * SQRT_2_DIV_2;
                d[i + 1] = s[i + 1] + v;
                d[i + 3] = s[i + 1] - v;
                let v = (s[i + 5] + s[i + 7]) * SQRT_2_DIV_2;
                d[i + 5] = v + s[i + 3];
                d[i + 7] = v - s[i + 3];
            }
        }
        // Remaining passes; `src_is_input` = where the last pass wrote.
        let mut src_is_input = true;
        for pass in 3..NUM_PASSES {
            let n = 1 << pass;
            let n_2 = n >> 1;
            let (s, d): (&[f32], &mut [f32]) =
                if src_is_input { (&*input, &mut *output) } else { (&*output, &mut *input) };
            let mut i = 0;
            while i < size {
                d[i] = s[i] + s[i + n];
                d[i + n] = s[i] - s[i + n];
                d[i + n_2] = s[i + n_2];
                d[i + n + n_2] = s[i + n + n_2];
                self.start(pass);
                for j in 1..n_2 {
                    let c = self.cos;
                    let sn = self.sin;
                    let v = s[i + n + j] * c - s[i + n + n_2 + j] * sn;
                    d[i + j] = s[i + j] + v;
                    d[i + n - j] = s[i + j] - v;
                    let v = s[i + n + j] * sn + s[i + n + n_2 + j] * c;
                    d[i + n + j] = v + s[i + n_2 + j];
                    d[i + 2 * n - j] = v - s[i + n_2 + j];
                    self.rotate();
                }
                i += n << 1;
            }
            src_is_input = !src_is_input;
        }
        // The result is in the last destination.
        if src_is_input {
            output[..size].copy_from_slice(&input[..size]);
        }
    }

    /// Inverse transform (unnormalised); `input` is used as workspace (lost).
    pub fn inverse(&mut self, input: &mut [f32], output: &mut [f32]) {
        let size = FFT_SIZE;
        let mut d_is_output = true;
        let mut pass = NUM_PASSES - 1;
        while pass >= 3 {
            let n = 1 << pass;
            let n_2 = n >> 1;
            let (s, d): (&[f32], &mut [f32]) =
                if d_is_output { (&*input, &mut *output) } else { (&*output, &mut *input) };
            let mut i = 0;
            while i < size {
                d[i] = s[i] + s[i + n];
                d[i + n] = s[i] - s[i + n];
                d[i + n_2] = s[i + n_2] * 2.0;
                d[i + n + n_2] = s[i + n + n_2] * 2.0;
                self.start(pass);
                for j in 1..n_2 {
                    d[i + j] = s[i + j] + s[i + n - j];
                    d[i + n_2 + j] = s[i + n + j] - s[i + 2 * n - j];
                    let c = self.cos;
                    let sn = self.sin;
                    let vr = s[i + j] - s[i + n - j];
                    let vi = s[i + n + j] + s[i + 2 * n - j];
                    d[i + n + j] = vr * c + vi * sn;
                    d[i + n + n_2 + j] = vi * c - vr * sn;
                    self.rotate();
                }
                i += n << 1;
            }
            d_is_output = !d_is_output;
            pass -= 1;
        }
        // After the flip, `d_is_output` means the data sits in `input`.
        if d_is_output {
            output[..size].copy_from_slice(&input[..size]);
        }
        {
            let s = &*output;
            let d = &mut *input;
            for i in (0..size).step_by(8) {
                d[i] = s[i] + s[i + 4];
                d[i + 4] = s[i] - s[i + 4];
                d[i + 2] = s[i + 2] * 2.0;
                d[i + 6] = s[i + 6] * 2.0;
                d[i + 1] = s[i + 1] + s[i + 3];
                d[i + 3] = s[i + 5] - s[i + 7];
                let vr = s[i + 1] - s[i + 3];
                let vi = s[i + 5] + s[i + 7];
                d[i + 5] = (vr + vi) * SQRT_2_DIV_2;
                d[i + 7] = (vi - vr) * SQRT_2_DIV_2;
            }
        }
        for i in (0..size).step_by(4) {
            let s = &input[i..i + 4];
            let r0 = self.r0(i);
            let r1 = r0 + 2 * (size >> 2);
            let r2 = r0 + (size >> 2);
            let r3 = r0 + 3 * (size >> 2);
            let b_0 = s[0] + s[2];
            let b_2 = s[0] - s[2];
            let b_1 = s[1] * 2.0;
            let b_3 = s[3] * 2.0;
            output[r0] = b_0 + b_1;
            output[r1] = b_0 - b_1;
            output[r2] = b_2 + b_3;
            output[r3] = b_2 - b_3;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_matches_dft_bin_and_round_trips() {
        let mut fft = ShyFft::default();
        let mut x = vec![0.0f32; FFT_SIZE];
        for (i, v) in x.iter_mut().enumerate() {
            *v = (2.0 * std::f32::consts::PI * 37.0 * i as f32 / FFT_SIZE as f32).cos()
                + 0.5 * (2.0 * std::f32::consts::PI * 100.0 * i as f32 / FFT_SIZE as f32).sin();
        }
        let orig = x.clone();
        let mut out = vec![0.0f32; FFT_SIZE];
        fft.direct(&mut x, &mut out);
        let h = FFT_SIZE / 2;
        // cos at bin 37 -> re = n/2; sin at bin 100 -> |im| = n/4.
        assert!((out[37] - h as f32).abs() < 0.5, "{}", out[37]);
        assert!((out[h + 100].abs() - (FFT_SIZE / 4) as f32).abs() < 0.5, "{}", out[h + 100]);
        assert!(out[50].abs() < 0.1 && out[h + 50].abs() < 0.1);
        let mut back = vec![0.0f32; FFT_SIZE];
        fft.inverse(&mut out, &mut back);
        for i in 0..FFT_SIZE {
            assert!((back[i] / FFT_SIZE as f32 - orig[i]).abs() < 1e-3, "{i}");
        }
    }
}
