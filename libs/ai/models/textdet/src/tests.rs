use crate::cpu::{self, Map};
use crate::ir::{Act, Graph, Op, Postprocess, Preprocess, Tensor};
use crate::{file, post};

#[cfg(any(target_os = "linux", target_os = "android"))]
mod vulkan;

/// Small deterministic generator (tests only).
pub(crate) struct Rng(u64);

impl Rng {
    pub fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
    pub fn tensor(&mut self, shape: &[usize], scale: f32) -> Tensor {
        let n = shape.iter().product();
        Tensor { shape: shape.to_vec(), data: (0..n).map(|_| self.next() * scale).collect() }
    }
}

/// A miniature detector touching every op kind the converter emits: a strided
/// stem, depthwise + pointwise blocks with squeeze-excitation, pooling, an
/// FPN-style top-down path, concat and a DB head (conv + 2x transposed conv
/// twice + sigmoid). Output `[1, H, W]` for an input `[3, H, W]`, H, W
/// multiples of 8.
pub(crate) fn synthetic_graph(seed: u64) -> Graph {
    let mut r = Rng(seed);
    let mut weights = Vec::new();
    let mut add = |t: Tensor| {
        weights.push(t);
        weights.len() - 1
    };
    let hs = Act::HardSigmoid { slope: 1.0 / 6.0, offset: 0.5 };
    let mut ops = Vec::new();
    // v1: stem 3 -> 8, k3 s2 (H/2)
    let w = add(r.tensor(&[8, 3, 3, 3], 0.4));
    let b = add(r.tensor(&[8], 0.1));
    ops.push(Op::Conv { input: 0, weight: w, bias: Some(b), out_channels: 8, kernel: (3, 3), stride: (2, 2), pad: [1, 1, 1, 1], groups: 1, act: Act::HardSwish });
    // v2: depthwise k5 s2 (H/4)
    let w = add(r.tensor(&[8, 1, 5, 5], 0.3));
    let b = add(r.tensor(&[8], 0.1));
    ops.push(Op::Conv { input: 1, weight: w, bias: Some(b), out_channels: 8, kernel: (5, 5), stride: (2, 2), pad: [2, 2, 2, 2], groups: 8, act: Act::HardSwish });
    // v3: SE: global pool -> 1x1 relu -> 1x1 hardsigmoid -> mul
    ops.push(Op::GlobalAvgPool { input: 2 });
    let w = add(r.tensor(&[4, 8, 1, 1], 0.5));
    let b = add(r.tensor(&[4], 0.1));
    ops.push(Op::Conv { input: 3, weight: w, bias: Some(b), out_channels: 4, kernel: (1, 1), stride: (1, 1), pad: [0; 4], groups: 1, act: Act::Relu });
    let w = add(r.tensor(&[8, 4, 1, 1], 0.5));
    let b = add(r.tensor(&[8], 0.1));
    ops.push(Op::Conv { input: 4, weight: w, bias: Some(b), out_channels: 8, kernel: (1, 1), stride: (1, 1), pad: [0; 4], groups: 1, act: hs });
    ops.push(Op::Mul { x: 2, s: 5 }); // v6
    // v7: pointwise 8 -> 16
    let w = add(r.tensor(&[16, 8, 1, 1], 0.4));
    let b = add(r.tensor(&[16], 0.1));
    ops.push(Op::Conv { input: 6, weight: w, bias: Some(b), out_channels: 16, kernel: (1, 1), stride: (1, 1), pad: [0; 4], groups: 1, act: Act::HardSwish });
    // v8: max pool k3 s2 p1 (H/8), v9 avg pool k2 s2 of v7 (H/8)
    ops.push(Op::Pool { input: 7, kernel: 3, stride: 2, pad: 1, max: true });
    ops.push(Op::Pool { input: 7, kernel: 2, stride: 2, pad: 0, max: false });
    ops.push(Op::Add { a: 8, b: 9, act: Act::None }); // v10 (H/8, 16)
    // FPN: lateral 1x1 to 8 channels at H/8 and H/4, top-down add
    let w = add(r.tensor(&[8, 16, 1, 1], 0.4));
    ops.push(Op::Conv { input: 10, weight: w, bias: None, out_channels: 8, kernel: (1, 1), stride: (1, 1), pad: [0; 4], groups: 1, act: Act::None }); // v11
    let w = add(r.tensor(&[8, 16, 1, 1], 0.4));
    ops.push(Op::Conv { input: 7, weight: w, bias: None, out_channels: 8, kernel: (1, 1), stride: (1, 1), pad: [0; 4], groups: 1, act: Act::None }); // v12 (H/4)
    ops.push(Op::UpsampleLike { input: 11, like: 12 }); // v13
    ops.push(Op::Add { a: 12, b: 13, act: Act::None }); // v14
    // 3x3 outputs, one upsampled x2 to H/4, concat
    let w = add(r.tensor(&[4, 8, 3, 3], 0.3));
    ops.push(Op::Conv { input: 11, weight: w, bias: None, out_channels: 4, kernel: (3, 3), stride: (1, 1), pad: [1, 1, 1, 1], groups: 1, act: Act::None }); // v15 (H/8)
    ops.push(Op::Upsample { input: 15, scale: 2 }); // v16 (H/4)
    let w = add(r.tensor(&[4, 8, 3, 3], 0.3));
    ops.push(Op::Conv { input: 14, weight: w, bias: None, out_channels: 4, kernel: (3, 3), stride: (1, 1), pad: [1, 1, 1, 1], groups: 1, act: Act::None }); // v17
    ops.push(Op::Concat { inputs: vec![16, 17] }); // v18 (8 ch, H/4)
    // DB head: 3x3 conv relu, convT k2 s2 relu, convT k2 s2 -> sigmoid
    let w = add(r.tensor(&[4, 8, 3, 3], 0.3));
    let b = add(r.tensor(&[4], 0.1));
    ops.push(Op::Conv { input: 18, weight: w, bias: Some(b), out_channels: 4, kernel: (3, 3), stride: (1, 1), pad: [1, 1, 1, 1], groups: 1, act: Act::Relu }); // v19
    let w = add(r.tensor(&[4, 4, 2, 2], 0.5));
    let b = add(r.tensor(&[4], 0.1));
    ops.push(Op::ConvTranspose { input: 19, weight: w, bias: Some(b), out_channels: 4, kernel: (2, 2), stride: (2, 2), act: Act::Relu }); // v20 (H/2)
    let w = add(r.tensor(&[4, 1, 2, 2], 0.5));
    let b = add(r.tensor(&[1], 0.1));
    ops.push(Op::ConvTranspose { input: 20, weight: w, bias: Some(b), out_channels: 1, kernel: (2, 2), stride: (2, 2), act: Act::None }); // v21 (H)
    ops.push(Op::Activate { input: 21, act: Act::Sigmoid }); // v22
    Graph { ops, weights, pre: Preprocess::default(), post: Postprocess::default() }
}

pub(crate) fn synthetic_input(seed: u64, h: usize, w: usize) -> Map {
    let mut r = Rng(seed);
    Map { c: 3, h, w, data: (0..3 * h * w).map(|_| r.next() * 2.0).collect() }
}

#[test]
fn synthetic_graph_runs_on_the_cpu() {
    let g = synthetic_graph(1);
    g.validate().unwrap();
    let out = cpu::run(&g, synthetic_input(2, 32, 48)).unwrap();
    assert_eq!((out.c, out.h, out.w), (1, 32, 48));
    assert!(out.data.iter().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
fn convolution_matches_a_hand_computed_case() {
    // 1 channel 3x3 input, 2x2 kernel of ones, stride 1, no pad -> 2x2 sums.
    let x = Map { c: 1, h: 3, w: 3, data: (1..=9).map(|v| v as f32).collect() };
    let y = cpu::conv(&x, &[1.0; 4], Some(&[0.5]), 1, (2, 2), (1, 1), [0; 4], 1, Act::None).unwrap();
    assert_eq!(y.data, vec![12.5, 16.5, 24.5, 28.5]);
    // Transposed 2x2 stride 2 of a 1x1 input copies the kernel.
    let x = Map { c: 1, h: 1, w: 1, data: vec![2.0] };
    let y = cpu::conv_transpose(&x, &[1.0, 2.0, 3.0, 4.0], None, 1, (2, 2), (2, 2), Act::None).unwrap();
    assert_eq!(y.data, vec![2.0, 4.0, 6.0, 8.0]);
}

#[test]
fn the_file_round_trips() {
    let g = synthetic_graph(3);
    let dir = std::env::temp_dir().join(format!("textdet-roundtrip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("det.gguf");
    let _ = std::fs::remove_file(&path);
    file::save(&g, &path, "synthetic").unwrap();
    let back = file::load(&path).unwrap();
    assert_eq!(back, g);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn boxes_come_out_in_reading_order_as_fractions() {
    // A 64x32 map with two "lines" on one row and one below.
    let (w, h) = (64, 32);
    let mut prob = Map::zeros(1, h, w);
    let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize| {
        for y in y0..y1 {
            for x in x0..x1 {
                prob.data[y * w + x] = 0.9;
            }
        }
    };
    fill(44, 4, 60, 10);
    fill(4, 4, 30, 10);
    fill(4, 20, 50, 28);
    let boxes = post::boxes(&prob, (1.0, 1.0), (w, h), &Postprocess::default());
    assert_eq!(boxes.len(), 3);
    assert!(boxes[0].x < boxes[1].x && boxes[0].y < 0.2 && boxes[2].y + boxes[2].h / 2.0 > 0.5);
    for b in &boxes {
        assert!(b.x >= 0.0 && b.y >= 0.0 && b.x + b.w <= 1.0 + 1e-6 && b.y + b.h <= 1.0 + 1e-6);
        assert!((b.score - 0.9).abs() < 1e-5);
    }
    // Grown beyond the kernel (unclip), so wider than 26/64.
    assert!(boxes[0].w > 26.0 / 64.0);
}

#[test]
fn weak_or_tiny_regions_are_dropped() {
    let (w, h) = (32, 32);
    let mut prob = Map::zeros(1, h, w);
    for y in 2..10 { for x in 2..20 { prob.data[y * w + x] = 0.4; } } // above thresh, below box_thresh
    prob.data[20 * w + 20] = 0.99; // one pixel
    assert!(post::boxes(&prob, (1.0, 1.0), (w, h), &Postprocess::default()).is_empty());
}

#[test]
fn preparing_scales_to_multiples_and_normalises() {
    let pre = Preprocess::default();
    assert_eq!(post::input_size(4000, 3000, &pre), (960, 736));
    assert_eq!(post::input_size(100, 50, &pre), (96, 64));
    let rgba = vec![255u8; 100 * 50 * 4];
    let p = post::prepare(100, 50, &rgba, &pre).unwrap();
    assert_eq!((p.input.c, p.input.h, p.input.w), (3, 64, 96));
    // White in BGR order: channel 0 is blue.
    assert!((p.input.data[0] - (1.0 - pre.mean[0]) / pre.std[0]).abs() < 1e-5);
}

#[test]
fn detect_runs_end_to_end_on_the_cpu() {
    let mut det = crate::TextDetector::cpu(synthetic_graph(5)).unwrap();
    let rgba: Vec<u8> = (0..80 * 60 * 4).map(|i| (i * 37 % 251) as u8).collect();
    let boxes = det.detect(&crate::Frame { width: 80, height: 60, rgba: &rgba }).unwrap();
    for b in boxes {
        assert!(b.w > 0.0 && b.h > 0.0);
    }
    assert_eq!(det.last_timings().input, (96, 64));
}

#[test]
fn overlapping_pieces_of_a_line_merge() {
    let b = |x: f32, w: f32| post::LineBox { x, y: 0.4, w, h: 0.05, score: 0.9 };
    let merged = post::merge_touching(vec![b(0.1, 0.2), b(0.28, 0.1), b(0.5, 0.1)]);
    assert_eq!(merged.len(), 2);
    assert!((merged[0].x - 0.1).abs() < 1e-6 && (merged[0].w - 0.28).abs() < 1e-6);
}
