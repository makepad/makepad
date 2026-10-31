//! Real-device correctness and timestamp benchmarks; run tests serially.
use makepad_ai_vulkan::*;
use makepad_script::makepad_math::{f16_bits_to_f32, f32_to_f16_bits};

fn data(len: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..len).map(|_| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        ((state & 65535) as f32 - 32768.0) / 32768.0
    }).collect()
}
fn halves(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|v| f32_to_f16_bits(*v).to_le_bytes()).collect()
}
fn rounded(v: &[f32]) -> Vec<f32> {
    v.iter().map(|v| f16_bits_to_f32(f32_to_f16_bits(*v))).collect()
}

#[derive(Clone)]
struct Reference { values: Vec<f32>, tolerance: Vec<f32> }

fn reference([n, k, m]: [u32; 3], w: &[f32], x: &[f32]) -> Reference {
    let (n, k, m) = (n as usize, k as usize, m as usize);
    let mut values = Vec::with_capacity(n * m);
    let mut tolerance = Vec::with_capacity(n * m);
    // Contiguous inner loops let the host vectorize the full reference.
    let mut row = vec![0.0f64; m];
    let mut magnitude = vec![0.0f64; m];
    for r in 0..n {
        row.fill(0.0);
        magnitude.fill(0.0);
        for d in 0..k {
            let a = w[r * k + d] as f64;
            for c in 0..m {
                let product = a * x[d * m + c] as f64;
                row[c] += product;
                magnitude[c] += product.abs();
            }
        }
        values.extend(row.iter().map(|v| *v as f32));
        tolerance.extend(magnitude.iter().map(|v| 2e-5 + *v as f32 * 2e-6));
    }
    Reference { values, tolerance }
}

fn assert_auto_mode(device: &Device, activations: ActivationType, weights: ActivationType, gemm: &Gemm) {
    let selected = std::env::var("MAKEPAD_GEMM_KERNEL").ok().and_then(|name| GemmMode::from_name(&name, weights).ok())
        .filter(|mode| Gemm::supports(device, activations, *mode).is_ok());
    let tuned = GemmMode::Tuned { name: "gemm_tune_4x8_k8_w128", weights };
    if let Some(selected) = selected { assert_eq!(gemm.mode, selected); }
    else if Gemm::supports(device, activations, tuned).is_ok() { assert_eq!(gemm.mode, tuned); }
    else if Gemm::supports(device, activations, GemmMode::Register { rows: 4, weights }).is_ok() {
        assert_eq!(gemm.mode, GemmMode::Register { rows: 4, weights });
    }
}

fn run(device: &Device, library: &mut Library, dims: [u32; 3], activations: ActivationType, mode: GemmMode, expected: Option<&Reference>) {
    let [n, k, m] = dims;
    let w = data((n * k) as usize, 0x13579);
    let x = data((k * m) as usize, 0x24681);
    let wbytes = halves(&w);
    let xbytes = if activations == ActivationType::F16 { halves(&x) } else { x.iter().flat_map(|v| v.to_le_bytes()).collect() };
    let wb = device.buffer(wbytes.len() as u64, BufferKind::Device).unwrap();
    let xb = device.buffer(xbytes.len() as u64, BufferKind::Device).unwrap();
    let db = device.buffer(u64::from(n) * u64::from(m) * 4 + 64, BufferKind::Device).unwrap();
    if !wbytes.is_empty() { device.upload(&wb, 0, &wbytes).unwrap(); }
    if !xbytes.is_empty() { device.upload(&xb, 0, &xbytes).unwrap(); }
    device.upload(&db, u64::from(n) * u64::from(m) * 4, &[0x5a; 64]).unwrap();
    let gemm = Gemm::new(device, library, n, k, m, activations, mode).unwrap();
    if matches!(mode, GemmMode::Auto | GemmMode::Shared) {
        assert!(gemm.shape.is_none());
    }
    if mode == GemmMode::Auto {
        assert_auto_mode(device, activations, ActivationType::F16, &gemm);
    }
    let mut stream = device.stream(2).unwrap();
    stream.begin().unwrap();
    stream.mark("gemm");
    gemm.encode(&mut stream, &wb, &xb, &db).unwrap();
    stream.mark("end");
    stream.submit_wait().unwrap();
    let output = device.download(&db, 0, (n * m) as usize * 4 + 64).unwrap();
    assert_eq!(&output[(n * m) as usize * 4..], &[0x5a; 64], "output guard");
    let output: Vec<f32> = output[..(n * m) as usize * 4].chunks_exact(4).map(|v| f32::from_le_bytes(v.try_into().unwrap())).collect();
    let benchmark = expected.is_some();
    let local_reference;
    let expected = match expected {
        Some(reference) => reference,
        None => {
            local_reference = float_reference(dims, &w, &x, activations, gemm.mode);
            &local_reference
        }
    };
    let mut max_abs = 0.0f32;
    for at in 0..(n * m) as usize {
        let r = at / m as usize;
        let c = at % m as usize;
        let value = expected.values[at];
        let error = (output[at] - value).abs();
        max_abs = max_abs.max(error);
        // FP32 accumulation order varies between matrix and scalar paths.
        let tolerance = expected.tolerance[at];
        // MAKEPAD_GEMM_REPORT_ONLY: report the error instead of failing
        // (devices whose matrix units round differently).
        if !benchmark || std::env::var("MAKEPAD_GEMM_REPORT_ONLY").is_err() {
            assert!(output[at].is_finite() && error <= tolerance, "{dims:?} {activations:?} {mode:?} at ({r},{c}): {} expected {value} error {error} tolerance {tolerance}", output[at]);
        }
    }
    if benchmark {
        for _ in 0..3 { stream.submit_wait().unwrap(); }
        let mut times = Vec::new();
        for _ in 0..11 {
            stream.submit_wait().unwrap();
            times.push(stream.timings().unwrap()[0].1);
        }
        times.sort_by(f64::total_cmp);
        let ms = times[times.len() / 2];
        let gflops = 2.0 * n as f64 * k as f64 * m as f64 / (ms * 1e6);
        eprintln!("GEMM {dims:?} {activations:?} {mode:?} {} subgroup={} median={ms:.4}ms {gflops:.1} GFLOPS max_abs={max_abs:.6}", gemm.kernel.name, device.features.subgroup_size);
    }
}

#[test]
fn gemm_matches_cpu() {
    // This test overrides capability limits; keep its device isolated.
    let mut device = Device::new().expect("real Vulkan device required for GEMM validation");
    eprintln!("{}", device.features.description());
    for shape in &device.features.cooperative_matrix_shapes { eprintln!("{shape:?}"); }
    cooperative_operations(&device);
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    for activations in [ActivationType::F32, ActivationType::F16] {
        for mode in [GemmMode::Auto, GemmMode::Shared] {
            for dims in [[64, 64, 64], [128, 96, 152], [67, 37, 71], [1, 1, 1], [17, 0, 19]] {
                run(&device, &mut library, dims, activations, mode, None);
            }
        }
    }
    let shapes = device.features.cooperative_matrix_shapes.clone();
    let invocation_limit = device.features.max_workgroup_invocations;
    if device.features.cooperative_matrix && !shapes.is_empty() {
        for subgroups in [1, 2] {
            device.features.max_workgroup_invocations = device.features.subgroup_size * subgroups;
            for activations in [ActivationType::F32, ActivationType::F16] {
                run(&device, &mut library, [67, 40, 76], activations, GemmMode::Auto, None);
            }
        }
        device.features.max_workgroup_invocations = invocation_limit;
    }
    device.features.max_workgroup_invocations = 128;
    run(&device, &mut library, [67, 37, 71], ActivationType::F32, GemmMode::Shared, None);
    device.features.max_workgroup_invocations = invocation_limit;
    for dims in [(16, 8, 16), (16, 8, 8)] {
        device.features.cooperative_matrix_shapes = shapes.iter().copied().filter(|s| (s.m, s.n, s.k) == dims).collect();
        if !device.features.cooperative_matrix_shapes.is_empty() {
            run(&device, &mut library, [65, 37, 71], ActivationType::F32, GemmMode::Auto, None);
        }
    }
    device.features.cooperative_matrix_shapes.clear();
    run(&device, &mut library, [35, 33, 37], ActivationType::F32, GemmMode::Auto, None);
    device.features.cooperative_matrix = false;
    run(&device, &mut library, [35, 33, 37], ActivationType::F16, GemmMode::Auto, None);
}

#[test]
#[ignore = "GPU benchmark; run separately from other GPU tests"]
fn gemm_benchmark() {
    let device = Device::shared().unwrap();
    eprintln!("{}", device.features.description());
    for shape in &device.features.cooperative_matrix_shapes { eprintln!("{shape:?}"); }
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    let variants = supported_variants(&device, true);
    assert!(!variants.is_empty(), "no supported variants selected");
    // One phone binary can select names (comma-separated substrings), dimensions
    // (n,k,m;n,k,m) and sample count without recompiling.
    let dimensions = std::env::var("MAKEPAD_GEMM_DIMS").unwrap_or_else(|_| "1280,1280,1500;5120,1280,1500;1024,1024,1024;4096,1024,1024;1024,4096,1024;3072,1024,1024".into());
    let samples = std::env::var("MAKEPAD_GEMM_SAMPLES").map(|v| v.parse::<usize>().unwrap()).unwrap_or(11);
    assert!(samples > 0);
    eprintln!("GEMM_DATA normal weights std=1/sqrt(k), normal activations, column 0 has 24x outliers every 97 rows; NT includes bias; errors vs original f32 CPU reference, kernel_abs vs stored/rounded inputs; h8/h16 use f16 arithmetic bounds; median includes quantisation and its barrier, excludes upload/compile/readback");
    for entry in dimensions.split(';') {
        let dims: Vec<u32> = entry.split(',').map(|v| v.trim().parse().unwrap()).collect();
        let dims: [u32; 3] = dims.try_into().expect("dimensions must be n,k,m");
        let mut case = ExperimentData::new(dims);
        for variant in &variants {
            experiment(&device, &mut library, &mut case, variant, samples);
        }
    }
}

#[derive(Clone, Copy)]
enum ExperimentMode {
    Float(ActivationType, GemmMode),
    FloatNt(ActivationType, GemmMode),
    Integer(Int8GemmMode),
}

struct Variant { name: String, mode: ExperimentMode }

fn supported_variants(device: &Device, filter: bool) -> Vec<Variant> {
    let mut all = Vec::new();
    for x in [ActivationType::F32, ActivationType::F16] {
        let xn = if x == ActivationType::F32 { "f32" } else { "f16" };
        for (name, mode) in [("auto", GemmMode::Auto), ("shared_2x2", GemmMode::Shared)] {
            all.push(Variant { name: format!("f16_{name}_x{xn}"), mode: ExperimentMode::Float(x, mode) });
        }
        for rows in [4, 8] {
            all.push(Variant { name: format!("f16_register_{rows}x4_x{xn}"), mode: ExperimentMode::Float(x, GemmMode::Register { rows, weights: ActivationType::F16 }) });
        }
        for shape in [[16, 16, 16], [64, 16, 16]] {
            for subgroup in [32, 128] {
                let [r, c, d] = shape;
                all.push(Variant { name: format!("f16_coop_{r}x{c}x{d}_sg{subgroup}_x{xn}"), mode: ExperimentMode::Float(x, GemmMode::Cooperative { shape, subgroup }) });
            }
        }
    }
    for rows in [4, 8] {
        all.push(Variant { name: format!("f32_register_{rows}x4"), mode: ExperimentMode::Float(ActivationType::F32, GemmMode::Register { rows, weights: ActivationType::F32 }) });
        all.push(Variant { name: format!("i8_dot4_{rows}x4"), mode: ExperimentMode::Integer(Int8GemmMode::Dot4 { rows }) });
    }
    all.push(Variant { name: "gemm_register_nt_4x4".into(), mode: ExperimentMode::FloatNt(ActivationType::F16, GemmMode::Register { rows: 4, weights: ActivationType::F16 }) });
    all.push(Variant { name: "gemm_register_nt_8x4".into(), mode: ExperimentMode::FloatNt(ActivationType::F16, GemmMode::Register { rows: 8, weights: ActivationType::F16 }) });
    for c in GEMM_CANDIDATES {
        let mode = GemmMode::from_name(c.name, ActivationType::F16).unwrap();
        all.push(Variant { name: c.name.into(), mode: ExperimentMode::Float(ActivationType::F32, mode) });
        all.push(Variant { name: c.nt_name.into(), mode: ExperimentMode::FloatNt(ActivationType::F16, mode) });
        // Exercise both packed half loaders in correctness runs. The phone
        // model-path comparison uses f32 activations and f16 weights.
        if !filter {
            all.push(Variant { name: format!("{}_xf16", c.name), mode: ExperimentMode::Float(ActivationType::F16, mode) });
            all.push(Variant { name: format!("{}_wf32", c.name), mode: ExperimentMode::Float(ActivationType::F32, GemmMode::from_name(c.name, ActivationType::F32).unwrap()) });
        }
    }
    // The depth-32 shape provides a native validation path when a device
    // exposes that integer shape instead of the phone's depth-16 shapes.
    for shape in [[16, 16, 16], [64, 16, 16], [16, 16, 32]] {
        for subgroup in [32, 128] {
            let [r, c, d] = shape;
            all.push(Variant { name: format!("i8_coop_{r}x{c}x{d}_sg{subgroup}"), mode: ExperimentMode::Integer(Int8GemmMode::Cooperative { shape, subgroup }) });
        }
    }
    let selected = if filter { std::env::var("MAKEPAD_GEMM_VARIANTS").ok() } else { None };
    all.into_iter().filter(|v| {
        // Preserve substring selection; =name selects exactly one prototype.
        if selected.as_ref().is_some_and(|names| !names.split(',').any(|name| {
            let name = name.trim();
            if let Some(exact) = name.strip_prefix('=') { v.name == exact } else { v.name.contains(name) }
        })) { return false; }
        let support = match v.mode {
            ExperimentMode::Float(x, mode) => Gemm::supports(device, x, mode),
            ExperimentMode::FloatNt(_, mode) => Gemm::supports(device, ActivationType::F32, mode),
            ExperimentMode::Integer(mode) => Int8Gemm::supports(device, mode),
        };
        if let Err(reason) = support { eprintln!("GEMM_SKIP {}: {reason}", v.name); return false; }
        true
    }).collect()
}

fn float_reference(dims: [u32; 3], w: &[f32], x: &[f32], activations: ActivationType, mode: GemmMode) -> Reference {
    let candidate = mode.candidate();
    let half_shared = candidate.is_some_and(|c| c.half_shared)
        || matches!(mode, GemmMode::Shared | GemmMode::Cooperative { .. });
    let w = if mode.weights() == ActivationType::F16 || half_shared { rounded(w) } else { w.to_vec() };
    let x = if activations == ActivationType::F16 || half_shared { rounded(x) } else { x.to_vec() };
    let mut expected = reference(dims, &w, &x);
    if let Some(chain) = candidate.map(|c| c.half_chain).filter(|c| *c != 0) {
        half_chain_tolerance(&mut expected, dims[1], chain);
    }
    expected
}

fn half_chain_tolerance(expected: &mut Reference, k: u32, chain: u32) {
        // A product rounding plus at most chain additions, u=2^-11. This
        // covers contracted and uncontracted half multiply-adds. Each chain
        // starts at zero; only its final value enters the f32 running sum.
        // Absolute underflow error is <= 2^-24 per product/add (including
        // a possible double rounding). Keep full-f32 tolerances unchanged.
        let nu = (chain + 1) as f32 / 2048.0;
        let gamma = nu / (1.0 - nu);
        for tolerance in &mut expected.tolerance {
            let magnitude = (*tolerance - 2e-5).max(0.0) / 2e-6;
            *tolerance += gamma * magnitude + k as f32 * (2.0 / 16777216.0);
        }
}

// Deterministic normal data; the reference always starts with the original
// f32 weights and activations, before either half conversion or quantisation.
fn normal_data(len: usize, seed: u32) -> Vec<f32> {
    let uniform = data(len.div_ceil(2) * 2, seed);
    let mut values = Vec::with_capacity(uniform.len());
    for pair in uniform.chunks_exact(2) {
        let radius = (-2.0 * ((pair[0] + 1.0001) * 0.49995).ln()).sqrt();
        let phase = pair[1] * std::f32::consts::PI;
        values.push(radius * phase.cos());
        values.push(radius * phase.sin());
    }
    values.truncate(len);
    values
}

struct ExperimentData {
    dims: [u32; 3],
    w: Vec<f32>,
    x: Vec<f32>,
    original: Reference,
    references: std::collections::HashMap<u8, Reference>,
    packed: Option<[Vec<u8>; 4]>,
}

impl ExperimentData {
    fn new(dims: [u32; 3]) -> Self {
        let [n, k, m] = dims;
        let mut w = normal_data((n * k) as usize, 0x13579);
        for v in &mut w { *v /= (k.max(1) as f32).sqrt(); }
        let mut x = normal_data((k * m) as usize, 0x24681);
        if m > 0 {
            for d in (0..k as usize).step_by(97) { x[d * m as usize] *= 24.0; }
        }
        // Zero row/column exercise zero scales without replacing the outlier
        // column or all the data in a 1x1 test.
        if n > 1 { w[..k as usize].fill(0.0); }
        if m > 1 { for d in 0..k as usize { x[d * m as usize + m as usize - 1] = 0.0; } }
        let original = reference(dims, &w, &x);
        Self { dims, w, x, original, references: Default::default(), packed: None }
    }

    fn expected(&mut self, mode: ExperimentMode) -> &Reference {
        let key = match mode {
            ExperimentMode::Integer(_) => 4,
            ExperimentMode::Float(x, mode) => reference_key(x, mode),
            ExperimentMode::FloatNt(_, mode) => reference_key(ActivationType::F32, mode),
        };
        if key == 0 { return &self.original; }
        // h8/h16 have the same rounded operands as hs. Reuse its expensive
        // full CPU product on phone-sized matrices; only their bounds differ.
        if key > 4 && !self.references.contains_key(&key) {
            let base = self.references.entry(3).or_insert_with(|| reference(self.dims, &rounded(&self.w), &rounded(&self.x)));
            let mut expected = base.clone();
            half_chain_tolerance(&mut expected, self.dims[1], u32::from(key / 4));
            self.references.insert(key, expected);
        }
        self.references.entry(key).or_insert_with(|| {
            assert_ne!(key, 4, "validate quantisation before comparing GEMM");
            match mode {
                ExperimentMode::Float(x, mode) => float_reference(self.dims, &self.w, &self.x, x, mode),
                ExperimentMode::FloatNt(_, mode) => float_reference(self.dims, &self.w, &self.x, ActivationType::F32, mode),
                ExperimentMode::Integer(_) => unreachable!(),
            }
        })
    }
}

fn reference_key(x: ActivationType, mode: GemmMode) -> u8 {
    let half = mode.candidate().is_some_and(|c| c.half_shared) || matches!(mode, GemmMode::Shared | GemmMode::Cooperative { .. });
    u8::from(mode.weights() == ActivationType::F16 || half) + 2 * u8::from(x == ActivationType::F16 || half)
        + mode.candidate().map_or(0, |c| c.half_chain as u8 * 4)
}

// Validate quantisation independently: exact scale and span sums, zero
// padding, and nearest signed code to within the rounding error of f32
// division/multiplication. GPU reciprocal rounding can choose the adjacent
// code at a halfway boundary; the integer GEMM reference uses those verified
// codes, while all reported accuracy remains relative to the original f32.
fn verify_quantization(device: &Device, gemm: &Int8Gemm, weights: &Int8Weights, case: &mut ExperimentData) {
    let [n, k, m] = case.dims.map(|v| v as usize);
    let kp = k.div_ceil(64) * 64;
    let spans = kp.div_ceil(32768).max(1);
    let read = |buffer: &Buffer, len| if len == 0 { Vec::new() } else { device.download(buffer, 0, len).unwrap() };
    let packed = [read(&weights.codes, n * kp), read(&weights.metadata, n * spans * 8),
        read(&gemm.activation_codes, m * kp), read(&gemm.activation_metadata, m * spans * 8)];
    if let Some(previous) = &case.packed {
        assert_eq!(packed, *previous, "quantisation must be identical across GEMM variants");
        return;
    }
    let mut decoded = [vec![0.0; n * k], vec![0.0; k * m]];
    for (operand, count) in [n, m].into_iter().enumerate() {
        let source = if operand == 0 { &case.w } else { &case.x };
        let index = |r: usize, d: usize| if operand == 0 { r * k + d } else { d * m + r };
        let codes = &packed[operand * 2];
        let meta = &packed[operand * 2 + 1];
        let meta_value = |at: usize| f32::from_le_bytes(meta[at..at + 4].try_into().unwrap());
        for r in 0..count {
            let amax = (0..k).fold(0.0f32, |a, d| a.max(source[index(r, d)].abs()));
            let expected_scale = amax / 127.0;
            for span in 0..spans {
                let at = (span * count + r) * 8;
                let scale = meta_value(at);
                assert!(scale.is_finite() && (scale - expected_scale).abs() <= expected_scale * 2.0 * f32::EPSILON,
                    "operand {operand} row/column {r} scale {scale} expected {expected_scale}");
                let start = span * 32768;
                let end = (start + 32768).min(kp);
                let mut sum = 0i32;
                for d in start..end {
                    let code = codes[r * kp + d] as i8;
                    assert_ne!(code, -128);
                    sum += i32::from(code);
                    if d >= k || amax == 0.0 {
                        assert_eq!(code, 0, "zero/padding operand {operand} [{r},{d}]");
                    } else {
                        let normalized = f64::from(source[index(r, d)]) / f64::from(expected_scale);
                        assert!((f64::from(code) - normalized).abs() <= 0.5 + 4.0 * f64::from(f32::EPSILON) * 127.0,
                            "operand {operand} [{r},{d}] code {code} normalized {normalized}");
                    }
                    if d < k { decoded[operand][index(r, d)] = f32::from(code) * scale; }
                }
                assert_eq!(meta_value(at + 4), sum as f32, "operand {operand} row/column {r} span {span} sum");
            }
        }
    }
    case.references.insert(4, reference(case.dims, &decoded[0], &decoded[1]));
    case.packed = Some(packed);
}

fn bytes(v: &[f32]) -> Vec<u8> { v.iter().flat_map(|v| v.to_le_bytes()).collect() }

fn experiment(device: &Device, library: &mut Library, case: &mut ExperimentData, variant: &Variant, samples: usize) {
    let [n, k, m] = case.dims;
    let output_bytes = (n * m) as usize * 4;
    let dst = device.buffer(output_bytes as u64 + 64, BufferKind::Device).unwrap();
    // A NaN prefix catches missing output writes, including K=0.
    if output_bytes > 0 { device.upload(&dst, 0, &vec![0xff; output_bytes]).unwrap(); }
    device.upload(&dst, output_bytes as u64, &[0x5a; 64]).unwrap();
    let mut stream = device.stream(3).unwrap();
    // Keep the prepared kernel and every addressed buffer alive through replay.
    let (float, integer, weights, packed_weights, activations, bias_buffer);
    let bias = data(n as usize, 43);
    match variant.mode {
        ExperimentMode::Float(x, mode) => {
            let half_w = mode.weights() == ActivationType::F16;
            let wbytes = if half_w { halves(&case.w) } else { bytes(&case.w) };
            let xbytes = if x == ActivationType::F16 { halves(&case.x) } else { bytes(&case.x) };
            weights = Some(device.buffer(wbytes.len() as u64, BufferKind::Device).unwrap());
            activations = device.buffer(xbytes.len() as u64, BufferKind::Device).unwrap();
            if !wbytes.is_empty() { device.upload(weights.as_ref().unwrap(), 0, &wbytes).unwrap(); }
            if !xbytes.is_empty() { device.upload(&activations, 0, &xbytes).unwrap(); }
            float = Some(Gemm::new(device, library, n, k, m, x, mode).unwrap());
            integer = None;
            packed_weights = None;
            bias_buffer = None;
            stream.begin().unwrap();
            stream.mark("gemm");
            float.as_ref().unwrap().encode(&mut stream, weights.as_ref().unwrap(), &activations, &dst).unwrap();
        }
        ExperimentMode::FloatNt(w, mode) => {
            let wbytes = if w == ActivationType::F16 { halves(&case.w) } else { bytes(&case.w) };
            let mut transposed = vec![0.0; case.x.len()];
            for d in 0..k {
                for c in 0..m { transposed[(c * k + d) as usize] = case.x[(d * m + c) as usize]; }
            }
            weights = Some(device.buffer(wbytes.len() as u64, BufferKind::Device).unwrap());
            activations = device.buffer(transposed.len() as u64 * 4, BufferKind::Device).unwrap();
            bias_buffer = Some(device.buffer(u64::from(n) * 4, BufferKind::Device).unwrap());
            if !wbytes.is_empty() { device.upload(weights.as_ref().unwrap(), 0, &wbytes).unwrap(); }
            if !transposed.is_empty() { device.upload(&activations, 0, &bytes(&transposed)).unwrap(); }
            if !bias.is_empty() { device.upload(bias_buffer.as_ref().unwrap(), 0, &bytes(&bias)).unwrap(); }
            float = Some(Gemm::new_nt_with_mode(device, library, n, k, m, w, k, mode).unwrap());
            integer = None;
            packed_weights = None;
            stream.begin().unwrap();
            stream.mark("gemm");
            float.as_ref().unwrap().encode_bias(&mut stream, weights.as_ref().unwrap(), &activations, &dst, bias_buffer.as_ref()).unwrap();
        }
        ExperimentMode::Integer(mode) => {
            weights = None;
            float = None;
            packed_weights = Some(Int8Weights::upload(device, n, k, &case.w).unwrap());
            bias_buffer = None;
            let xbytes = bytes(&case.x);
            activations = device.buffer(xbytes.len() as u64, BufferKind::Device).unwrap();
            if !xbytes.is_empty() { device.upload(&activations, 0, &xbytes).unwrap(); }
            integer = Some(Int8Gemm::new(device, library, n, k, m, mode).unwrap());
            let gemm = integer.as_ref().unwrap();
            stream.begin().unwrap();
            stream.mark("quant");
            gemm.encode_quantize(&mut stream, &activations).unwrap();
            stream.barrier();
            stream.mark("gemm");
            gemm.encode_matmul(&mut stream, packed_weights.as_ref().unwrap(), &dst).unwrap();
        }
    }
    stream.mark("end");
    stream.submit_wait().unwrap();
    let output = device.download(&dst, 0, output_bytes + 64).unwrap();
    assert_eq!(&output[output_bytes..], &[0x5a; 64], "{} output guard", variant.name);
    let mut output: Vec<f32> = output[..output_bytes].chunks_exact(4).map(|v| f32::from_le_bytes(v.try_into().unwrap())).collect();
    let nt = matches!(variant.mode, ExperimentMode::FloatNt(_, _));
    if nt {
        let storage = output.clone();
        for r in 0..n {
            for c in 0..m { output[(r * m + c) as usize] = storage[(c * n + r) as usize]; }
        }
    }
    if let Some(gemm) = &integer { verify_quantization(device, gemm, packed_weights.as_ref().unwrap(), case); }
    let mode = match variant.mode {
        ExperimentMode::Float(x, _) => ExperimentMode::Float(x, float.as_ref().unwrap().mode),
        ExperimentMode::FloatNt(w, _) => ExperimentMode::FloatNt(w, float.as_ref().unwrap().mode),
        mode => mode,
    };
    let expected = case.expected(mode);
    let mut kernel_abs = 0.0f32;
    let report_only = samples > 0 && std::env::var_os("MAKEPAD_GEMM_REPORT_ONLY").is_some();
    for (at, &v) in output.iter().enumerate() {
        assert!(v.is_finite(), "{} non-finite output at {at}", variant.name);
        let want = expected.values[at] + if nt { bias[at / m as usize] } else { 0.0 };
        let error = (v - want).abs();
        kernel_abs = kernel_abs.max(error);
        if !report_only {
            assert!(error <= expected.tolerance[at], "{} {n},{k},{m} at {at}: {v} expected {} error {error} tolerance {}", variant.name, expected.values[at], expected.tolerance[at]);
        }
    }
    if samples > 0 {
        for _ in 0..3 { stream.submit_wait().unwrap(); }
        let mut times = Vec::with_capacity(samples);
        let mut quants = Vec::with_capacity(samples);
        let mut kernels = Vec::with_capacity(samples);
        for _ in 0..samples {
            stream.submit_wait().unwrap();
            let timings = stream.timings().unwrap();
            times.push(timings.iter().map(|v| v.1).sum::<f64>());
            quants.push(if timings.len() == 2 { timings[0].1 } else { 0.0 });
            kernels.push(timings.last().unwrap().1);
        }
        for values in [&mut times, &mut quants, &mut kernels] { values.sort_by(f64::total_cmp); }
        let ms = times[samples / 2];
        let quant = quants[samples / 2];
        let kernel = kernels[samples / 2];
        let gflops = 2.0 * n as f64 * k as f64 * m as f64 / (ms * 1e6);
        let kernel_gflops = 2.0 * n as f64 * k as f64 * m as f64 / (kernel * 1e6);
        let mut max_abs = 0.0f32;
        let mut squared = [0.0f64; 2];
        let mut norm = [0.0f64; 2];
        for (at, &v) in output.iter().enumerate() {
            let want = case.original.values[at] + if nt { bias[at / m as usize] } else { 0.0 };
            let err = (v - want).abs();
            max_abs = max_abs.max(err);
            let tail = usize::from(at % m as usize == 0);
            squared[tail] += f64::from(err).powi(2);
            norm[tail] += f64::from(want).powi(2);
        }
        let random_rel = (squared[0] / norm[0].max(f64::MIN_POSITIVE)).sqrt();
        let tail_rel = (squared[1] / norm[1].max(f64::MIN_POSITIVE)).sqrt();
        let rmse = ((squared[0] + squared[1]) / output.len().max(1) as f64).sqrt();
        let kernel_info = float.as_ref().map(|g| format!(" shader={} wg={} shared_bytes={} subgroup={}", g.kernel.name,
            g.kernel.workgroup()[0], g.kernel.layout.shared_bytes,
            g.mode.candidate().and_then(|c| c.subgroup).map_or_else(|| "default".into(), |s| s.to_string()))).unwrap_or_default();
        eprintln!("GEMM [{n},{k},{m}] {} median={ms:.4}ms {gflops:.1}GFLOPS max_abs={max_abs:.6} rmse={rmse:.6} random_rel_l2={random_rel:.6} tail_rel_l2={tail_rel:.6} quant={quant:.4}ms kernel={kernel:.4}ms kernel_GFLOPS={kernel_gflops:.1} kernel_abs={kernel_abs:.6}{kernel_info}", variant.name);
    }
    drop((float, integer, weights, packed_weights, activations, bias_buffer));
}

#[test]
fn gemm_variants_match_cpu() {
    let device = Device::shared().expect("real Vulkan device required");
    eprintln!("{}", device.features.description());
    let variants = supported_variants(&device, false);
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    for c in GEMM_CANDIDATES {
        assert_eq!(GemmMode::from_name(c.name, ActivationType::F16).unwrap(), GemmMode::from_name(c.nt_name, ActivationType::F16).unwrap());
        for name in [c.name, c.nt_name] {
            for (half_w, half_x, vector, half_vector8) in [(1, 0, 1, 1), (1, 1, 1, 0), (1, 1, 0, 0), (0, 0, 0, 0)] {
                let compiled = library.compile_constants(name, &[("half_w", half_w), ("half_x", half_x),
                    ("vector_w", vector), ("vector_x", vector), ("half_vector8", half_vector8)]).unwrap();
                assert_eq!(compiled.layout.shared_bytes, c.shared_bytes, "{name}");
                assert!(compiled.layout.shared_bytes <= 32 * 1024, "{name}");
                assert_eq!(compiled.layout.workgroup, [c.workgroup, 1, 1], "{name}");
                if let Some(result) = makepad_script::shader_ir_spirv::spirv_val(&compiled.spirv) { result.unwrap(); }
            }
        }
    }
    // Compile and validate the phone-only shapes and every loader path even
    // on a device that cannot execute those cooperative shapes/subgroups.
    for subgroup in [32, 128] {
        for count in [1, 2, 4] {
            for name in ["gemm_i8_coop_16x16x16", "gemm_i8_coop_64x16x16", "gemm_i8_coop_16x16x32"] {
                let compiled = library.compile_constants(name, &[("workgroup_size", subgroup * count), ("subgroups", count)]).unwrap();
                assert!(compiled.layout.shared_bytes <= 32 * 1024);
                if let Some(result) = makepad_script::shader_ir_spirv::spirv_val(&compiled.spirv) { result.unwrap(); }
            }
            for half_x in [0, 1] {
                for (vector, half_vector8) in [(0, 0), (1, 0), (1, 1)] {
                    let compiled = library.compile_constants("gemm_coop_64x16x16", &[
                        ("workgroup_size", subgroup * count), ("subgroups", count), ("half_x", half_x),
                        ("vector_w", vector), ("vector_x", vector), ("half_vector8", half_vector8),
                    ]).unwrap();
                    assert!(compiled.layout.shared_bytes <= 32 * 1024);
                    if let Some(result) = makepad_script::shader_ir_spirv::spirv_val(&compiled.spirv) { result.unwrap(); }
                }
            }
        }
    }
    for dims in [[64, 64, 64], [128, 96, 152], [67, 40, 76], [67, 37, 71], [129, 33, 131], [17, 7, 19], [17, 9, 19], [17, 15, 19], [17, 17, 19], [1, 1, 1], [17, 0, 19], [0, 32, 7], [7, 32, 0], [3, 32769, 5]] {
        let mut case = ExperimentData::new(dims);
        for variant in &variants { experiment(&device, &mut library, &mut case, variant, 0); }
    }
    // Dyadic inputs have exact products and exact half-chain sums. Demand
    // the normal f32 tolerance here as an independent indexing/ordering
    // check, in addition to the analytic half-rounding bound on normal data.
    let mut exact = ExperimentData::new([129, 33, 131]);
    for (i, v) in exact.w.iter_mut().enumerate() { *v = (i as i32 % 7 - 3) as f32 * 0.125; }
    for (i, v) in exact.x.iter_mut().enumerate() { *v = (i as i32 % 5 - 2) as f32 * 0.25; }
    exact.original = reference(exact.dims, &exact.w, &exact.x);
    for key in [35, 67] { exact.references.insert(key, reference(exact.dims, &exact.w, &exact.x)); }
    for variant in &variants {
        if !matches!(variant.mode, ExperimentMode::Integer(_)) { experiment(&device, &mut library, &mut exact, variant, 0); }
    }
    // Saturated signed codes stress the raw unsigned dot sum and three
    // ordered spans, including both signs and the padded final span.
    let mut case = ExperimentData::new([2, 65537, 4]);
    case.w.fill(1.0);
    for (at, v) in case.x.iter_mut().enumerate() { *v = if at % 2 == 0 { 1.0 } else { -1.0 }; }
    case.original = reference(case.dims, &case.w, &case.x);
    for variant in &variants {
        if matches!(variant.mode, ExperimentMode::Integer(_)) { experiment(&device, &mut library, &mut case, variant, 0); }
    }
}

#[test]
fn gemm_nt_bias_matches_cpu() {
    let device = Device::shared().expect("real Vulkan device required");
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    for [n, k, m] in [[67u32, 37, 71], [129, 33, 131], [65, 40, 69], [128, 64, 128], [9, 0, 7], [1, 1, 1], [0, 8, 7], [9, 8, 0]] {
        for weights in [ActivationType::F16, ActivationType::F32] {
            for stride in [k, k.div_ceil(8) * 8, k + 3] {
                let mut w = data((n * k) as usize, 19);
                if weights == ActivationType::F16 { w = rounded(&w); }
                let x = data((k * m) as usize, 37);
                let bias = data(n as usize, 43);
                let mut padded = vec![0.0; (n * stride) as usize];
                let mut transposed = vec![0.0; (k * m) as usize];
                for r in 0..n {
                    for d in 0..k { padded[(r * stride + d) as usize] = w[(r * k + d) as usize]; }
                }
                for d in 0..k {
                    for c in 0..m { transposed[(c * k + d) as usize] = x[(d * m + c) as usize]; }
                }
                let wb = if weights == ActivationType::F16 { halves(&padded) } else { bytes(&padded) };
                let wbuf = device.buffer(wb.len() as u64, BufferKind::Device).unwrap();
                let xbuf = device.buffer(u64::from(k * m) * 4, BufferKind::Device).unwrap();
                let bbuf = device.buffer(u64::from(n) * 4, BufferKind::Device).unwrap();
                let size = (n * m) as usize * 4;
                let dst = device.buffer(size as u64 + 128, BufferKind::Device).unwrap();
                if !wb.is_empty() { device.upload(&wbuf, 0, &wb).unwrap(); }
                if !transposed.is_empty() { device.upload(&xbuf, 0, &bytes(&transposed)).unwrap(); }
                if !bias.is_empty() { device.upload(&bbuf, 0, &bytes(&bias)).unwrap(); }
                let modes = std::iter::once(GemmMode::Auto).chain(std::iter::once(GemmMode::Register { rows: 8, weights }))
                    .chain(GEMM_CANDIDATES.iter().map(|c| GemmMode::from_name(c.name, weights).unwrap()));
                for mode in modes.filter(|mode| Gemm::supports(&device, ActivationType::F32, *mode).is_ok()) {
                let gemm = if mode == GemmMode::Auto { Gemm::new_nt(&device, &mut library, n, k, m, weights, stride).unwrap() }
                    else { Gemm::new_nt_with_mode(&device, &mut library, n, k, m, weights, stride, mode).unwrap() };
                if mode == GemmMode::Auto { assert_auto_mode(&device, ActivationType::F32, weights, &gemm); }
                let expected = float_reference([n, k, m], &w, &x, ActivationType::F32, gemm.mode);
                for with_bias in [false, true] {
                    device.upload(&dst, 0, &vec![0x5a; size + 128]).unwrap();
                    let mut stream = device.stream(0).unwrap();
                    stream.begin().unwrap();
                    gemm.encode_addresses(&mut stream, wbuf.address, xbuf.address, dst.address + 64,
                        with_bias.then_some(bbuf.address)).unwrap();
                    for _ in 0..2 {
                        stream.submit_wait().unwrap();
                        let result = device.download(&dst, 0, size + 128).unwrap();
                        assert_eq!(&result[..64], &[0x5a; 64]);
                        assert_eq!(&result[size + 64..], &[0x5a; 64]);
                        for c in 0..m {
                            for r in 0..n {
                                let at = (c * n + r) as usize * 4 + 64;
                                let got = f32::from_le_bytes(result[at..at + 4].try_into().unwrap());
                                let idx = (r * m + c) as usize;
                                let want = expected.values[idx] + if with_bias { bias[r as usize] } else { 0.0 };
                                assert!(got.is_finite() && (got - want).abs() <= expected.tolerance[idx],
                                    "NT [{n},{k},{m}] {weights:?} {mode:?} stride={stride} bias={with_bias} [{r},{c}]: {got} vs {want}");
                            }
                        }
                    }
                }
                }
            }
        }
    }
    for weights in [0, 1] {
        for vector in [0, 1] {
            let compiled = library.compile_constants("gemm_register_nt_4x4", &[("half_w", weights), ("vector_w", vector), ("vector_x", vector)]).unwrap();
            if let Some(result) = makepad_script::shader_ir_spirv::spirv_val(&compiled.spirv) { result.unwrap(); }
        }
    }
}

#[test]
fn gemm_int8_nt_bias_matches_cpu() {
    let device = Device::shared().expect("real Vulkan device required");
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    for [n, k, m] in [[67, 37, 71], [65, 64, 68], [17, 0, 19], [1, 1, 1], [0, 32, 7], [7, 32, 0], [3, 32769, 5], [2, 65537, 4]] {
        let mut case = ExperimentData::new([n, k, m]);
        if k == 65537 {
            case.w.fill(1.0);
            for (i, v) in case.x.iter_mut().enumerate() { *v = if i % 2 == 0 { 1.0 } else { -1.0 }; }
        }
        let weights = Int8Weights::upload(&device, n, k, &case.w).unwrap();
        let mut x = vec![0.0; (k * m) as usize];
        for d in 0..k {
            for c in 0..m { x[(c * k + d) as usize] = case.x[(d * m + c) as usize]; }
        }
        let bias = data(n as usize, 47);
        let xbuf = device.buffer(x.len() as u64 * 4, BufferKind::Device).unwrap();
        let bbuf = device.buffer(bias.len() as u64 * 4, BufferKind::Device).unwrap();
        let size = (n * m) as usize * 4;
        let dst = device.buffer(size as u64 + 128, BufferKind::Device).unwrap();
        if !x.is_empty() { device.upload(&xbuf, 0, &bytes(&x)).unwrap(); }
        if !bias.is_empty() { device.upload(&bbuf, 0, &bytes(&bias)).unwrap(); }
        let gemm = Int8Gemm::new_nt(&device, &mut library, n, k, m).unwrap();
        assert_eq!(gemm.kernel.name, "gemm_i8_dot4_nt_4x4");
        for with_bias in [false, true] {
            device.upload(&dst, 0, &vec![0x5a; size + 128]).unwrap();
            let mut stream = device.stream(0).unwrap();
            stream.begin().unwrap();
            stream.barrier();
            gemm.encode_addresses(&mut stream, &weights, xbuf.address, dst.address + 64, with_bias.then_some(bbuf.address)).unwrap();
            for _ in 0..2 {
                stream.submit_wait().unwrap();
                verify_quantization(&device, &gemm, &weights, &mut case);
                let expected = case.expected(ExperimentMode::Integer(Int8GemmMode::Dot4 { rows: 4 }));
                let result = device.download(&dst, 0, size + 128).unwrap();
                assert_eq!(&result[..64], &[0x5a; 64]);
                assert_eq!(&result[size + 64..], &[0x5a; 64]);
                for c in 0..m {
                    for r in 0..n {
                        let at = (c * n + r) as usize * 4 + 64;
                        let got = f32::from_le_bytes(result[at..at + 4].try_into().unwrap());
                        let idx = (r * m + c) as usize;
                        let want = expected.values[idx] + if with_bias { bias[r as usize] } else { 0.0 };
                        assert!(got.is_finite() && (got - want).abs() <= expected.tolerance[idx],
                            "int8 NT [{n},{k},{m}] bias={with_bias} [{r},{c}]: {got} vs {want}");
                    }
                }
            }
        }
    }
}

fn cooperative_operations(device: &Device) {
    use makepad_vulkan::vk;
    let source = include_str!("../kernels/cooperative_test.splash");
    let mut library = Library::new(&[("cooperative_test.splash", source)]).unwrap();
    for (name, component) in [("float_matrices", vk::ComponentTypeKHR::FLOAT16), ("signed_matrices", vk::ComponentTypeKHR::SINT8), ("unsigned_matrices", vk::ComponentTypeKHR::UINT8)] {
        let depth = if component == vk::ComponentTypeKHR::FLOAT16 { 16 } else { 32 };
        if !device.features.cooperative_matrix_shapes.iter().any(|s| s.m == 16 && s.n == 16 && s.k == depth && s.a == component && s.b == component) { continue; }
        let compiled = library.compile_constants(name, &[("workgroup_size", device.features.subgroup_size)]).unwrap();
        if let Some(r) = makepad_script::shader_ir_spirv::spirv_val(&compiled.spirv) { r.unwrap(); }
        let kernel = device.kernel(name, &compiled, Some(device.features.subgroup_size)).unwrap();
        let depth = depth as usize;
        let signed = component != vk::ComponentTypeKHR::UINT8;
        let a: Vec<f32> = (0..16*depth).map(|i| (i % 7) as f32 - if signed { 3.0 } else { 0.0 }).collect();
        let b: Vec<f32> = (0..16*depth).map(|i| (i % 5) as f32 - if signed { 2.0 } else { 0.0 }).collect();
        let floating = component == vk::ComponentTypeKHR::FLOAT16;
        let mut ab = if floating { halves(&a) } else { a.iter().map(|v| *v as i8 as u8).collect() };
        let bb = if floating { halves(&b) } else { b.iter().map(|v| *v as i8 as u8).collect() };
        if floating { ab.extend_from_slice(&bb); }
        let wa = device.buffer(ab.len() as u64, BufferKind::Device).unwrap();
        let wb = device.buffer(bb.len() as u64, BufferKind::Device).unwrap();
        let dst = device.buffer(256*4, BufferKind::Device).unwrap();
        device.upload(&wa, 0, &ab).unwrap();
        device.upload(&wb, 0, &bb).unwrap();
        let mut stream = device.stream(0).unwrap();
        stream.begin().unwrap();
        stream.dispatch(&kernel.args().buffer("src", wa.address).buffer("src_b", wb.address).buffer("dst", dst.address).u32("stride", 16), [1, 1, 1]);
        stream.submit_wait().unwrap();
        let out = device.download(&dst, 0, 256*4).unwrap();
        for r in 0..16 {
            for c in 0..16 {
                let expected: f32 = (0..depth).map(|d| a[r*depth+d] * b[c*depth+d]).sum::<f32>() * if floating { 0.5 } else { 2.0 };
                let at = if floating { c*16+r } else { r*16+c } * 4;
                let bytes: [u8;4] = out[at..at+4].try_into().unwrap();
                let value = if floating { f32::from_le_bytes(bytes) } else if signed { i32::from_le_bytes(bytes) as f32 } else { u32::from_le_bytes(bytes) as f32 };
                assert_eq!(value, expected, "{name} [{r},{c}]");
            }
        }
    }
}
