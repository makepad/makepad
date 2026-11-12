//! The planar 3x3 "same" conv's two paths on the GPU against a plain f64
//! reference: GEMMs over an f16 im2col slab, and cuDNN's implicit GEMM in
//! f16. Built with the CUDA kernels only; it needs a device, and cuDNN for
//! the second.
#![cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]

use makepad_ai_common::backend::{
    gpu_conv2d_planar_cached, gpu_conv2d_planar_cudnn_f16, gpu_cudnn_require, gpu_device_available, gpu_download,
    gpu_upload,
};

/// A conv's shape: odd sizes, so a slip at an edge or in a stride shows.
const IN: usize = 32;
const OUT: usize = 24;
const W: usize = 37;
const H: usize = 23;

struct Case {
    x: Vec<f32>,
    weights: Vec<f32>,
    bias: Vec<f32>,
    reference: Vec<f64>,
}

fn case() -> Case {
    let mut seed = 0x2545_f491u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let x: Vec<f32> = (0..IN * W * H).map(|_| rand()).collect();
    let weights: Vec<f32> = (0..OUT * IN * 9).map(|_| rand() * 0.1).collect();
    let bias: Vec<f32> = (0..OUT).map(|_| rand() * 0.5).collect();
    let mut reference = vec![0.0f64; OUT * W * H];
    for o in 0..OUT {
        for y in 0..H {
            for x0 in 0..W {
                let mut sum = bias[o] as f64;
                for i in 0..IN {
                    for ky in 0..3 {
                        for kx in 0..3 {
                            let (sy, sx) = (y as isize + ky as isize - 1, x0 as isize + kx as isize - 1);
                            if sy < 0 || sx < 0 || sy >= H as isize || sx >= W as isize {
                                continue;
                            }
                            let v = x[i * W * H + sy as usize * W + sx as usize] as f64;
                            sum += v * weights[((o * IN + i) * 3 + ky) * 3 + kx] as f64;
                        }
                    }
                }
                reference[o * W * H + y * W + x0] = sum;
            }
        }
    }
    Case { x, weights, bias, reference }
}

/// The largest error against the reference, as a share of its largest value.
fn worst(got: &[f32], reference: &[f64]) -> f64 {
    let top = reference.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    got.iter().zip(reference).fold(0.0f64, |m, (g, r)| m.max((*g as f64 - r).abs())) / top
}

#[test]
fn a_3x3_conv_as_gemms_over_im2col_matches_the_reference() {
    assert!(gpu_device_available(), "this test needs a CUDA device");
    let case = case();
    let x = gpu_upload(&case.x, IN, W * H).unwrap();
    let owner = makepad_ai_common::backend::WeightOwner::new("planar-conv-test");
    let out = gpu_conv2d_planar_cached(&x, W, H, &owner, "gemm", &case.weights, &case.bias, OUT, 3, 3, 1, 1)
        .unwrap();
    let got = gpu_download(&out).unwrap();
    let error = worst(&got, &case.reference);
    // Inputs and weights rounded to f16, f32 sums: about 1e-3 of the largest.
    assert!(error < 4e-3, "the GEMM conv is {error:.2e} of the largest value off");
}

#[test]
fn a_3x3_conv_on_cudnn_matches_the_reference() {
    assert!(gpu_device_available(), "this test needs a CUDA device");
    gpu_cudnn_require().unwrap();
    let case = case();
    let x = gpu_upload(&case.x, IN, W * H).unwrap();
    let owner = makepad_ai_common::backend::WeightOwner::new("planar-conv-test");
    let out = gpu_conv2d_planar_cudnn_f16(&x, W, H, &owner.key("cudnn"), &case.weights, &case.bias, OUT).unwrap();
    let got = gpu_download(&out).unwrap();
    let error = worst(&got, &case.reference);
    // As the GEMM path, and each output stored f16 before its bias.
    assert!(error < 4e-3, "the cuDNN conv is {error:.2e} of the largest value off");
}
