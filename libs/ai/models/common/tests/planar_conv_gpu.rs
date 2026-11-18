//! The planar 3x3 conv's paths on the GPU against a plain f64 reference:
//! GEMMs over an f16 im2col slab, and our own implicit GEMM on f16 tensor
//! cores (the "same" conv, the stride-2 downsample, and on f16 maps the
//! same conv into a block of rows). Built with the
//! CUDA kernels only; it needs a device (sm_80 or later for the second).
#![cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]

use makepad_ai_common::backend::{
    gpu_conv2d_planar_cached, gpu_conv3x3_planar_f16_mma, gpu_device_available, gpu_download,
    gpu_realesrgan_conv3x3_f16, gpu_to_f16, gpu_upload,
};

/// A conv's shape: odd sizes, so a slip at an edge or in a stride shows.
/// `OUT` channels, unless a case asks for others.
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

/// `stride` 1: the "same" conv (pad 1 all round). `stride` 2: the
/// downsample, its taps past the right and bottom edges zero (pad 0 on the
/// left and top), its output `out_size`. `out` output channels.
fn case(stride: usize, out: usize) -> Case {
    let mut seed = 0x2545_f491u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let x: Vec<f32> = (0..IN * W * H).map(|_| rand()).collect();
    let weights: Vec<f32> = (0..out * IN * 9).map(|_| rand() * 0.1).collect();
    let bias: Vec<f32> = (0..out).map(|_| rand() * 0.5).collect();
    let (ow, oh) = (out_size(W, stride), out_size(H, stride));
    let pad = if stride == 1 { 1 } else { 0 };
    let mut reference = vec![0.0f64; out * ow * oh];
    for o in 0..out {
        for y in 0..oh {
            for x0 in 0..ow {
                let mut sum = bias[o] as f64;
                for i in 0..IN {
                    for ky in 0..3 {
                        for kx in 0..3 {
                            let sy = (y * stride + ky) as isize - pad;
                            let sx = (x0 * stride + kx) as isize - pad;
                            if sy < 0 || sx < 0 || sy >= H as isize || sx >= W as isize {
                                continue;
                            }
                            let v = x[i * W * H + sy as usize * W + sx as usize] as f64;
                            sum += v * weights[((o * IN + i) * 3 + ky) * 3 + kx] as f64;
                        }
                    }
                }
                reference[o * ow * oh + y * ow + x0] = sum;
            }
        }
    }
    Case { x, weights, bias, reference }
}

/// An output side: the input's for stride 1; for stride 2, with the one
/// zero row or column past the far edge, (side + 1 - 3) / 2 + 1.
fn out_size(side: usize, stride: usize) -> usize {
    if stride == 1 {
        side
    } else {
        (side + 1 - 3) / 2 + 1
    }
}

/// The largest error against the reference, as a share of its largest value.
fn worst(got: &[f32], reference: &[f64]) -> f64 {
    let top = reference.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    got.iter().zip(reference).fold(0.0f64, |m, (g, r)| m.max((*g as f64 - r).abs())) / top
}

#[test]
fn a_3x3_conv_as_gemms_over_im2col_matches_the_reference() {
    assert!(gpu_device_available(), "this test needs a CUDA device");
    let case = case(1, OUT);
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
fn a_3x3_conv_on_our_f16_tensor_core_gemm_matches_the_reference_at_strides_1_and_2() {
    assert!(gpu_device_available(), "this test needs a CUDA device");
    let owner = makepad_ai_common::backend::WeightOwner::new("planar-conv-test");
    // 24 channels: the 32-channel block at stride 1; 72: two 64-channel
    // blocks, the second nearly all padding; stride 2 takes 64 always.
    for (stride, channels) in [(1, OUT), (1, 72), (2, OUT)] {
        let case = case(stride, channels);
        let x = gpu_upload(&case.x, IN, W * H).unwrap();
        let key = owner.key(&format!("mma{stride}x{channels}"));
        let (out, ow, oh) =
            gpu_conv3x3_planar_f16_mma(&x, W, H, stride, &key, &case.weights, &case.bias, channels).unwrap();
        assert_eq!((ow, oh), (out_size(W, stride), out_size(H, stride)), "stride {stride}: the output's size");
        let got = gpu_download(&out).unwrap();
        let error = worst(&got, &case.reference);
        // As the GEMM path: inputs and weights rounded to f16, f32 sums, the output f32.
        assert!(error < 4e-3, "stride {stride}, {channels} channels: our tensor-core conv is {error:.2e} of the largest value off");
    }
}

#[test]
fn a_3x3_conv_on_f16_maps_reads_its_rows_and_writes_only_its_block() {
    assert!(gpu_device_available(), "this test needs a CUDA device");
    let owner = makepad_ai_common::backend::WeightOwner::new("planar-conv-test");
    let case = case(1, OUT);
    let plane = W * H;
    // 16 rows past the input's channels, and an output block from row 8 in
    // 16 rows more: neither the rows after the input nor those round the
    // block may be read or written.
    let mut x = case.x.clone();
    x.extend(std::iter::repeat(1000.0).take(16 * plane));
    let x = gpu_to_f16(&gpu_upload(&x, IN + 16, plane).unwrap()).unwrap();
    let out = gpu_to_f16(&gpu_upload(&vec![-7.0; (OUT + 16) * plane], OUT + 16, plane).unwrap()).unwrap();
    gpu_realesrgan_conv3x3_f16(&x, IN, W, H, &owner, "f16-rows", &case.weights, OUT, &out, 8).unwrap();
    let got = gpu_download(&out).unwrap();
    let (before, rest) = got.split_at(8 * plane);
    let (block, after) = rest.split_at(OUT * plane);
    assert!(before.iter().chain(after).all(|v| *v == -7.0), "the rows round the block are untouched");
    // Biasless: the reference less its bias.
    let reference: Vec<f64> =
        case.reference.iter().enumerate().map(|(i, r)| r - case.bias[i / plane] as f64).collect();
    let error = worst(block, &reference);
    // As the f32 maps, and each output rounded to f16.
    assert!(error < 4e-3, "the f16-map conv is {error:.2e} of the largest value off");
}
