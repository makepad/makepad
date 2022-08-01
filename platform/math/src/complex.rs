
// very basic complex types just enough to write an fft below

#[derive(Clone, Copy)]
pub struct ComplexF32 {pub re: f32, pub im: f32}

#[derive(Clone, Copy)]
pub struct ComplexF64 {pub re: f64, pub im: f64}

impl ComplexF32{
    pub fn magnitude(self)->f32{(self.re*self.re + self.im *self.im).sqrt()}
}

pub fn cf64(re: f64, im: f64) -> ComplexF64 {ComplexF64 {re, im}}
pub fn cf32(re: f32, im: f32) -> ComplexF32 {ComplexF32 {re, im}}

impl From<ComplexF32> for ComplexF64 {
    fn from(v: ComplexF32) -> Self {
        cf64(v.re as f64, v.im as f64)
    }
}

impl From<ComplexF64> for ComplexF32 {
    fn from(v: ComplexF64) -> Self {
        cf32(v.re as f32, v.im as f32)
    }
}

impl std::ops::Mul<ComplexF64> for ComplexF64 {
    type Output = ComplexF64;
    fn mul(self, rhs: ComplexF64) -> ComplexF64 {
        cf64(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re
        )
    }
}

impl std::ops::Add<ComplexF64> for ComplexF64 {
    type Output = ComplexF64;
    fn add(self, rhs: ComplexF64) -> ComplexF64 {
        cf64(self.re + rhs.re, self.im + rhs.im)
    }
}

impl std::ops::Sub<ComplexF64> for ComplexF64 {
    type Output = ComplexF64;
    fn sub(self, rhs: ComplexF64) -> ComplexF64 {
        cf64(self.re - rhs.re, self.im - rhs.im)
    }
}

// FFT algo, ported from https://github.com/rshuston/FFT-C/ rewritten with a few Rust types.

fn fft_f32_recursive_pow2_inner(data: &mut [ComplexF32], scratch: &mut [ComplexF32], n: usize, theta_pi: f64, stride: usize) {
    if stride < n {
        let stride2 = stride * 2;
        fft_f32_recursive_pow2_inner(scratch, data, n, theta_pi, stride2);
        fft_f32_recursive_pow2_inner(&mut scratch[stride..], &mut data[stride..], n, theta_pi, stride2);
        
        let theta = (stride2 as f64 * theta_pi) / n as f64;
        let wn = cf64(theta.cos(), theta.sin());
        let mut wnk = cf64(1.0, 0.0);

        for k in (0..n).step_by(stride2) {
            let kd2 = k >> 1;
            let kpnd2 = (k + n) >> 1;
            
            let u: ComplexF64 = scratch[k].into();
            let t: ComplexF64 = wnk * scratch[k + stride].into();
            
            data[kd2] = (u + t).into();
            data[kpnd2] = (u - t).into();
            
            wnk = wnk * wn;
        }
    }
}

use std::f64::consts::PI;

pub fn fft_f32_recursive_pow2_forward(data: &mut [ComplexF32], scratch: &mut [ComplexF32]) {
    fft_f32_recursive_pow2(data, scratch, -PI)
}

pub fn fft_f32_recursive_pow2_inverse(data: &mut [ComplexF32], scratch: &mut [ComplexF32]) {
    fft_f32_recursive_pow2(data, scratch, PI)
}

fn fft_f32_recursive_pow2(data: &mut [ComplexF32], scratch: &mut [ComplexF32], theta_pi: f64) {
    let n = data.len();
    if data.len() != scratch.len() {panic!()}
    fn is_power_of_2(n: usize)->bool{n != 0 && (!(n & (n - 1))) != 0 }
    if !is_power_of_2(n){ // check power of two
        panic!();
    };
    scratch.copy_from_slice(data);
    fft_f32_recursive_pow2_inner(data, scratch, n, theta_pi, 1);
}
