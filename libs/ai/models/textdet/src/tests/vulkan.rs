use super::*;
use crate::vulkan::Detector;
use makepad_ai_vulkan::Device;

const ACTS: [Act; 6] = [
    Act::None,
    Act::Relu,
    Act::HardSwish,
    Act::HardSigmoid { slope: 0.173, offset: -0.061 },
    Act::HardSwishAffine { scale: 0.8, bias: -0.2 },
    Act::Sigmoid,
];

fn available() -> bool {
    match Device::shared() {
        Ok(_) => true,
        Err(why) => {
            eprintln!("textdet Vulkan comparison skipped: no usable Vulkan device ({why})");
            false
        }
    }
}

fn graph(ops: Vec<Op>, weights: Vec<Tensor>) -> Graph {
    Graph { ops, weights, pre: Preprocess::default(), post: Postprocess::default() }
}

fn compare(label: &str, graph: &Graph, detector: &mut Detector, input: Map) {
    let expected = cpu::run(graph, input.clone()).unwrap();
    let actual = detector.run(&input).unwrap_or_else(|why| panic!("{label}: {why}"));
    assert_eq!((actual.c, actual.h, actual.w), (expected.c, expected.h, expected.w), "{label}");
    assert_eq!(actual.data.len(), expected.data.len(), "{label}");
    for (i, (&a, &b)) in actual.data.iter().zip(&expected.data).enumerate() {
        // Equality also covers the -infinity of an all-padding max window.
        assert!(a == b || (a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-4 + 1e-4 * b.abs()),
            "{label}, input {}x{}x{}, output {}x{}x{}, element {i}: Vulkan {a}, CPU {b}",
            input.c, input.h, input.w, actual.c, actual.h, actual.w);
    }
}

fn check(label: &str, graph: Graph, sizes: &[(usize, usize)]) {
    // Only device discovery can skip. A broken shader or dispatch must fail.
    let mut detector = Detector::new(&graph).unwrap_or_else(|why| panic!("{label}: {why}"));
    for (i, &(h, w)) in sizes.iter().enumerate() {
        compare(label, &graph, &mut detector, synthetic_input(97 + i as u64, h, w));
    }
}

#[test]
fn synthetic_graph_matches_cpu_across_sizes_and_reuse() {
    if !available() { return; }
    for seed in [1, 17, 93] {
        check(&format!("synthetic seed {seed}"), synthetic_graph(seed),
            &[(32, 48), (64, 64), (96, 160), (32, 48), (96, 160)]);
    }
}

#[test]
fn single_convolutions_match_cpu() {
    if !available() { return; }
    let mut r = Rng(211);
    // M crosses a tile boundary; K and the spatial rows have unaligned tails.
    // Asymmetric pads and strides must not fall through the pointwise shortcut.
    let cases = [
        (35, 1, (1, 1), (1, 1), [0; 4]),
        (5, 1, (3, 5), (2, 1), [0, 2, 1, 3]),
        (7, 1, (1, 1), (2, 2), [1, 0, 0, 2]),
        (3, 3, (3, 3), (1, 1), [1, 0, 2, 1]),
        (3, 3, (5, 5), (2, 2), [0, 2, 3, 1]),
        (3, 3, (7, 7), (2, 1), [3, 2, 1, 4]),
        (6, 3, (3, 3), (2, 2), [1; 4]),
    ];
    for (a, act) in ACTS.into_iter().enumerate() {
        for (out_channels, groups, kernel, stride, pad) in cases {
            let mut weights = vec![r.tensor(&[out_channels, 3 / groups, kernel.0, kernel.1], 0.2)];
            let bias = if a % 2 == 0 {
                weights.push(r.tensor(&[out_channels], 0.3));
                Some(1)
            } else { None };
            let g = graph(vec![Op::Conv { input: 0, weight: 0, bias, out_channels, kernel, stride, pad, groups, act }], weights);
            let sizes: &[(usize, usize)] = if kernel == (1, 1) && stride == (1, 1) {
                &[(1, 1), (7, 9), (32, 48), (7, 9)]
            } else { &[(7, 9), (32, 48), (7, 9)] };
            check(&format!("conv {out_channels} groups {groups} {kernel:?} {stride:?} {pad:?} {act:?}"), g, sizes);
        }
    }
}

#[test]
fn chunked_convolution_matches_cpu_and_bounds_scratch() {
    if !available() { return; }
    let mut r = Rng(271);
    let g = graph(vec![Op::Conv {
        input: 0, weight: 0, bias: Some(1), out_channels: 35,
        kernel: (3, 3), stride: (1, 1), pad: [1; 4], groups: 1, act: Act::Sigmoid,
    }], vec![r.tensor(&[35, 3, 3, 3], 0.2), r.tensor(&[35], 0.3)]);
    // K=27; a budget for 39 columns rounds down to 36. The 77-pixel
    // output splits 36 + 36 + 5, crossing image rows and GEMM tiles.
    let budget = 27 * 39 * 4;
    let mut detector = Detector::with_scratch_budget(&g, budget).unwrap();
    compare("chunked conv, ragged tail", &g, &mut detector, synthetic_input(277, 7, 11));
    // Padded weights, resident input/output, one output-sized staging buffer,
    // and at most the requested scratch budget (all sizes are in bytes).
    let weights = (35 * 28 + 36) * 4;
    let maps = (3 + 35 + 35) * 80 * 4;
    assert!(detector.gpu_bytes() <= weights + maps + budget);
    // Reuse after shrinking, exact chunks, then growing and shrinking again.
    for (h, w) in [(1, 1), (8, 9), (9, 13), (7, 11)] {
        compare("chunked conv reuse", &g, &mut detector, synthetic_input(281, h, w));
    }
}

#[test]
fn single_transposed_convolutions_match_cpu() {
    if !available() { return; }
    let mut r = Rng(317);
    for (a, act) in ACTS.into_iter().enumerate() {
        for (kernel, stride) in [((2, 2), (2, 2)), ((2, 3), (2, 3)), ((3, 2), (2, 1)), ((2, 2), (3, 4)), ((1, 1), (1, 1))] {
            let out_channels = 5;
            let mut weights = vec![r.tensor(&[3, out_channels, kernel.0, kernel.1], 0.25)];
            let bias = if a % 2 == 0 {
                weights.push(r.tensor(&[out_channels], 0.3));
                Some(1)
            } else { None };
            check(&format!("transpose {kernel:?} {stride:?} {act:?}"),
                graph(vec![Op::ConvTranspose { input: 0, weight: 0, bias, out_channels, kernel, stride, act }], weights),
                &[(1, 1), (7, 9), (32, 48), (7, 9)]);
        }
    }
}

#[test]
fn single_elementwise_ops_match_cpu() {
    if !available() { return; }
    for act in ACTS {
        let g = graph(vec![Op::Activate { input: 0, act }], vec![]);
        let mut detector = Detector::new(&g).unwrap();
        for (h, w) in [(1, 1), (7, 9), (32, 48)] {
            let mut input = synthetic_input(419, h, w);
            for v in &mut input.data { *v *= 4.0; }
            compare(&format!("activate {act:?}"), &g, &mut detector, input);
        }
        check(&format!("add {act:?}"), graph(vec![Op::Add { a: 0, b: 0, act }], vec![]), &[(1, 1), (7, 9), (32, 48)]);
    }
    check("same-shape multiply", graph(vec![Op::Mul { x: 0, s: 0 }], vec![]), &[(1, 1), (7, 9), (32, 48)]);
    check("960x960 activation", graph(vec![Op::Activate { input: 0, act: Act::HardSwish }], vec![]), &[(960, 960), (7, 9)]);
}

#[test]
fn single_pooling_ops_match_cpu() {
    if !available() { return; }
    for max in [false, true] {
        for (kernel, stride, pad) in [(3, 2, 1), (2, 2, 0), (3, 1, 2), (1, 2, 3)] {
            check(&format!("pool max={max} k{kernel} s{stride} p{pad}"),
                graph(vec![Op::Pool { input: 0, kernel, stride, pad, max }], vec![]), &[(7, 9), (32, 48), (7, 9)]);
        }
    }
    check("global average", graph(vec![Op::GlobalAvgPool { input: 0 }], vec![]),
        &[(1, 1), (7, 9), (32, 48), (96, 160), (960, 960), (7, 9)]);
}

#[test]
fn single_resize_and_concat_ops_match_cpu() {
    if !available() { return; }
    for scale in [1, 2, 3] {
        check(&format!("nearest x{scale}"), graph(vec![Op::Upsample { input: 0, scale }], vec![]), &[(1, 1), (7, 9), (32, 48)]);
    }
    check("nearest like itself", graph(vec![Op::UpsampleLike { input: 0, like: 0 }], vec![]), &[(1, 1), (7, 9), (32, 48)]);
    check("concat repeated input", graph(vec![Op::Concat { inputs: vec![0, 0, 0] }], vec![]), &[(1, 1), (7, 9), (32, 48)]);
}

#[test]
fn broadcast_resize_and_branched_lifetimes_match_cpu() {
    if !available() { return; }
    // Graphs have one external input, so differing binary operand shapes and
    // a non-integer UpsampleLike ratio need a producer before the op under test.
    check("channel broadcast", graph(vec![Op::GlobalAvgPool { input: 0 }, Op::Mul { x: 0, s: 1 }], vec![]),
        &[(1, 1), (7, 9), (32, 48), (7, 9)]);
    for (input, like) in [(1, 0), (0, 1)] {
        check(&format!("non-integer nearest {input} -> {like}"), graph(vec![
            Op::Pool { input: 0, kernel: 3, stride: 2, pad: 0, max: false },
            Op::UpsampleLike { input, like },
        ], vec![]), &[(7, 9), (32, 48)]);
    }
    check("concat different values", graph(vec![
        Op::Activate { input: 0, act: Act::HardSwish },
        Op::Concat { inputs: vec![0, 1, 0, 1] },
    ], vec![]), &[(1, 1), (7, 9), (32, 48)]);
    check("dead branch and delayed reader", graph(vec![
        Op::Activate { input: 0, act: Act::Relu },
        Op::Activate { input: 0, act: Act::Sigmoid }, // No readers; reclaim immediately.
        Op::Activate { input: 1, act: Act::HardSwish },
        Op::Add { a: 1, b: 3, act: Act::HardSwish },
        Op::Mul { x: 3, s: 4 },
    ], vec![]), &[(7, 9), (32, 48), (96, 160), (7, 9)]);
}

#[test]
fn reduction_dispatch_splits_over_two_dimensions() {
    if !available() { return; }
    // One group per channel crosses the dispatch boundary with little memory.
    let g = graph(vec![Op::GlobalAvgPool { input: 0 }], vec![]);
    let mut detector = Detector::new(&g).unwrap();
    let mut r = Rng(523);
    let input = Map { c: 65536, h: 1, w: 1, data: (0..65536).map(|_| r.next()).collect() };
    compare("split reduction", &g, &mut detector, input);
}

/// Recreating the camera's worker must keep the same Vulkan device, even
/// when its thread-local kernel cache dies on every cycle.
#[test]
fn detectors_can_be_made_and_dropped_many_times() {
    if !available() { return; }
    for i in 0..100 {
        std::thread::spawn(move || {
            let graph = synthetic_graph(1);
            let mut detector = Detector::new(&graph).unwrap_or_else(|e| panic!("detector {i}: {e}"));
            compare(&format!("detector {i}"), &graph, &mut detector, synthetic_input(97, 32, 48));
        }).join().unwrap_or_else(|_| panic!("detector worker {i} panicked"));
    }
}
