//! Layer: stitch-op.
//!
//! Per-opcode tests for the NONSTANDARD float math opcodes (prefix 0xE0,
//! opt-in via `Extensions::ext_math`): scalar f32/f64 and packed f32x4
//! sin, cos, tan, asin, acos, atan, exp, ln, atan2, pow, rmin, rmax, rem.
//!
//! Every op is compared bit-for-bit against the host Rust function it is
//! specified to match, over an edge corpus including NaN, infinities,
//! signed zeros and denormals. A wrong lane here points at exactly one
//! handler in exec.rs/simd.rs.

use makepad_stitch::{Engine, Extensions, Linker, Module, Store, V128, Val};

// A tiny Wasm binary emitter, just enough for one-function modules.

fn leb(mut val: u32, out: &mut Vec<u8>) {
    loop {
        let byte = (val & 0x7F) as u8;
        val >>= 7;
        if val == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

fn section(id: u8, payload: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    leb(payload.len() as u32, out);
    out.extend_from_slice(payload);
}

/// Builds a module with a single exported function "f" with the given
/// param/result types (value type bytes) and raw body code (without the
/// trailing `end`).
fn build_module(params: &[u8], results: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
    // Type section
    let mut payload = Vec::new();
    leb(1, &mut payload);
    payload.push(0x60);
    leb(params.len() as u32, &mut payload);
    payload.extend_from_slice(params);
    leb(results.len() as u32, &mut payload);
    payload.extend_from_slice(results);
    section(1, &payload, &mut out);
    // Function section
    section(3, &[1, 0], &mut out);
    // Export section
    let mut payload = Vec::new();
    leb(1, &mut payload);
    leb(1, &mut payload);
    payload.push(b'f');
    payload.push(0x00); // func export
    leb(0, &mut payload);
    section(7, &payload, &mut out);
    // Code section
    let mut func = Vec::new();
    leb(0, &mut func); // no locals
    func.extend_from_slice(body);
    func.push(0x0B); // end
    let mut payload = Vec::new();
    leb(1, &mut payload);
    leb(func.len() as u32, &mut payload);
    payload.extend_from_slice(&func);
    section(10, &payload, &mut out);
    out
}

struct Runner {
    store: Store,
    instance: makepad_stitch::Instance,
}

impl Runner {
    fn new(bytes: &[u8]) -> Runner {
        let engine = Engine::new_with_extensions(Extensions { ext_math: true });
        let mut store = Store::new(engine.clone());
        let module = Module::new(&engine, bytes).unwrap();
        let instance = Linker::new().instantiate(&mut store, &module).unwrap();
        Runner { store, instance }
    }

    fn call(&mut self, args: &[Val], results: &mut [Val]) {
        let func = self.instance.exported_func("f").unwrap();
        func.call(&mut self.store, args, results).unwrap();
    }
}

const F32_EDGES: &[f32] = &[
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    -0.75,
    2.5,
    -7.25,
    std::f32::consts::PI,
    -std::f32::consts::PI,
    1.0e-40,          // denormal
    -1.0e-40,         // negative denormal
    f32::MIN_POSITIVE,
    f32::MAX,
    f32::MIN,
    1.0e10,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NAN,
    100.5,
];

const F64_EDGES: &[f64] = &[
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    -0.75,
    2.5,
    -7.25,
    std::f64::consts::PI,
    -std::f64::consts::PI,
    1.0e-310, // denormal
    -1.0e-310,
    f64::MIN_POSITIVE,
    f64::MAX,
    f64::MIN,
    1.0e10,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    100.5,
];

// Prefix and subopcodes (see decode_ext_math_instr in code.rs).
const EXT: u8 = 0xE0;

fn scalar_f32_un_op(sub: u8, host: fn(f32) -> f32, name: &str) {
    // Stack-operand variant: f (param f32) (result f32) = op(x)
    let bytes = build_module(&[0x7D], &[0x7D], &[0x20, 0x00, EXT, sub]);
    let mut runner = Runner::new(&bytes);
    // Register-operand variant: op(op(x)) makes the inner result flow
    // through the float register into the outer op.
    let bytes_r = build_module(&[0x7D], &[0x7D], &[0x20, 0x00, EXT, sub, EXT, sub]);
    let mut runner_r = Runner::new(&bytes_r);
    for &x in F32_EDGES {
        let mut results = [Val::F32(0.0)];
        runner.call(&[Val::F32(x)], &mut results);
        let actual = results[0].to_f32().unwrap();
        let expected = host(x);
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{name}_s({x:?}): got {actual:?}, want {expected:?}"
        );
        runner_r.call(&[Val::F32(x)], &mut results);
        let actual = results[0].to_f32().unwrap();
        let expected = host(host(x));
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{name}_r({x:?}): got {actual:?}, want {expected:?}"
        );
    }
}

fn scalar_f64_un_op(sub: u8, host: fn(f64) -> f64, name: &str) {
    let bytes = build_module(&[0x7C], &[0x7C], &[0x20, 0x00, EXT, sub]);
    let mut runner = Runner::new(&bytes);
    let bytes_r = build_module(&[0x7C], &[0x7C], &[0x20, 0x00, EXT, sub, EXT, sub]);
    let mut runner_r = Runner::new(&bytes_r);
    for &x in F64_EDGES {
        let mut results = [Val::F64(0.0)];
        runner.call(&[Val::F64(x)], &mut results);
        let actual = results[0].to_f64().unwrap();
        let expected = host(x);
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{name}_s({x:?}): got {actual:?}, want {expected:?}"
        );
        runner_r.call(&[Val::F64(x)], &mut results);
        let actual = results[0].to_f64().unwrap();
        let expected = host(host(x));
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{name}_r({x:?}): got {actual:?}, want {expected:?}"
        );
    }
}

fn scalar_f32_bin_op(sub: u8, host: fn(f32, f32) -> f32, name: &str) {
    // ss variant
    let bytes = build_module(&[0x7D, 0x7D], &[0x7D], &[0x20, 0x00, 0x20, 0x01, EXT, sub]);
    let mut runner = Runner::new(&bytes);
    // rs variant: first operand comes out of the float register.
    let bytes_rs = build_module(
        &[0x7D, 0x7D],
        &[0x7D],
        &[0x20, 0x00, EXT, sub_id_f32_neg_free(), 0x20, 0x01, EXT, sub],
    );
    let mut runner_rs = Runner::new(&bytes_rs);
    // sr variant: second operand comes out of the float register.
    let bytes_sr = build_module(
        &[0x7D, 0x7D],
        &[0x7D],
        &[0x20, 0x00, 0x20, 0x01, EXT, sub_id_f32_neg_free(), EXT, sub],
    );
    let mut runner_sr = Runner::new(&bytes_sr);
    for &a in F32_EDGES {
        for &b in F32_EDGES {
            let mut results = [Val::F32(0.0)];
            runner.call(&[Val::F32(a), Val::F32(b)], &mut results);
            let actual = results[0].to_f32().unwrap();
            let expected = host(a, b);
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "{name}_ss({a:?}, {b:?}): got {actual:?}, want {expected:?}"
            );
            runner_rs.call(&[Val::F32(a), Val::F32(b)], &mut results);
            let actual = results[0].to_f32().unwrap();
            let expected = host(a.sin(), b);
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "{name}_rs({a:?}, {b:?}): got {actual:?}, want {expected:?}"
            );
            runner_sr.call(&[Val::F32(a), Val::F32(b)], &mut results);
            let actual = results[0].to_f32().unwrap();
            let expected = host(a, b.sin());
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "{name}_sr({a:?}, {b:?}): got {actual:?}, want {expected:?}"
            );
        }
    }
}

/// The "free" unary op used to force a value into the float register in the
/// bin-op variant tests: f32.sin (subopcode 0x00).
fn sub_id_f32_neg_free() -> u8 {
    0x00
}

/// Emits `f32.const val`.
fn f32_const(val: f32, out: &mut Vec<u8>) {
    out.push(0x43);
    out.extend_from_slice(&val.to_le_bytes());
}

/// Covers the immediate-operand variants (is, ir, si, ri) of a scalar f32
/// binary op, with 2.5 as the immediate.
fn scalar_f32_bin_op_imm(sub: u8, host: fn(f32, f32) -> f32, name: &str) {
    const IMM: f32 = 2.5;
    // is: (op (f32.const IMM) (local.get 0))
    let mut body = Vec::new();
    f32_const(IMM, &mut body);
    body.extend_from_slice(&[0x20, 0x00, EXT, sub]);
    let mut runner_is = Runner::new(&build_module(&[0x7D], &[0x7D], &body));
    // si: (op (local.get 0) (f32.const IMM))
    let mut body = Vec::new();
    body.extend_from_slice(&[0x20, 0x00]);
    f32_const(IMM, &mut body);
    body.extend_from_slice(&[EXT, sub]);
    let mut runner_si = Runner::new(&build_module(&[0x7D], &[0x7D], &body));
    // ir: (op (f32.const IMM) (f32.sin (local.get 0))) - second operand in reg
    let mut body = Vec::new();
    f32_const(IMM, &mut body);
    body.extend_from_slice(&[0x20, 0x00, EXT, 0x00, EXT, sub]);
    let mut runner_ir = Runner::new(&build_module(&[0x7D], &[0x7D], &body));
    // ri: (op (f32.sin (local.get 0)) (f32.const IMM)) - first operand in reg
    let mut body = Vec::new();
    body.extend_from_slice(&[0x20, 0x00, EXT, 0x00]);
    f32_const(IMM, &mut body);
    body.extend_from_slice(&[EXT, sub]);
    let mut runner_ri = Runner::new(&build_module(&[0x7D], &[0x7D], &body));
    for &x in F32_EDGES {
        let mut results = [Val::F32(0.0)];
        runner_is.call(&[Val::F32(x)], &mut results);
        assert_eq!(
            results[0].to_f32().unwrap().to_bits(),
            host(IMM, x).to_bits(),
            "{name}_is({IMM:?}, {x:?})"
        );
        runner_si.call(&[Val::F32(x)], &mut results);
        assert_eq!(
            results[0].to_f32().unwrap().to_bits(),
            host(x, IMM).to_bits(),
            "{name}_si({x:?}, {IMM:?})"
        );
        runner_ir.call(&[Val::F32(x)], &mut results);
        assert_eq!(
            results[0].to_f32().unwrap().to_bits(),
            host(IMM, x.sin()).to_bits(),
            "{name}_ir({IMM:?}, sin({x:?}))"
        );
        runner_ri.call(&[Val::F32(x)], &mut results);
        assert_eq!(
            results[0].to_f32().unwrap().to_bits(),
            host(x.sin(), IMM).to_bits(),
            "{name}_ri(sin({x:?}), {IMM:?})"
        );
    }
}

fn scalar_f64_bin_op(sub: u8, host: fn(f64, f64) -> f64, name: &str) {
    let bytes = build_module(&[0x7C, 0x7C], &[0x7C], &[0x20, 0x00, 0x20, 0x01, EXT, sub]);
    let mut runner = Runner::new(&bytes);
    for &a in F64_EDGES {
        for &b in F64_EDGES {
            let mut results = [Val::F64(0.0)];
            runner.call(&[Val::F64(a), Val::F64(b)], &mut results);
            let actual = results[0].to_f64().unwrap();
            let expected = host(a, b);
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "{name}_ss({a:?}, {b:?}): got {actual:?}, want {expected:?}"
            );
        }
    }
}

fn packed_un_op(sub: u8, host: fn(f32) -> f32, name: &str) {
    // f (param v128) (result v128)
    let bytes = build_module(&[0x7B], &[0x7B], &[0x20, 0x00, EXT, sub]);
    let mut runner = Runner::new(&bytes);
    for chunk in F32_EDGES.chunks(4) {
        let mut lanes = [0f32; 4];
        lanes[..chunk.len()].copy_from_slice(chunk);
        let mut results = [Val::V128(V128::ZERO)];
        runner.call(&[Val::V128(V128::from_f32x4(lanes))], &mut results);
        let actual = results[0].to_v128().unwrap().to_f32x4();
        for lane in 0..4 {
            let expected = host(lanes[lane]);
            assert_eq!(
                actual[lane].to_bits(),
                expected.to_bits(),
                "{name} lane {lane} of {lanes:?}: got {:?}, want {expected:?}",
                actual[lane]
            );
        }
    }
}

fn packed_bin_op(sub: u8, host: fn(f32, f32) -> f32, name: &str) {
    let bytes = build_module(&[0x7B, 0x7B], &[0x7B], &[0x20, 0x00, 0x20, 0x01, EXT, sub]);
    let mut runner = Runner::new(&bytes);
    for chunk_a in F32_EDGES.chunks(4) {
        for chunk_b in F32_EDGES.chunks(4) {
            let mut a = [0f32; 4];
            a[..chunk_a.len()].copy_from_slice(chunk_a);
            let mut b = [0f32; 4];
            b[..chunk_b.len()].copy_from_slice(chunk_b);
            let mut results = [Val::V128(V128::ZERO)];
            runner.call(
                &[Val::V128(V128::from_f32x4(a)), Val::V128(V128::from_f32x4(b))],
                &mut results,
            );
            let actual = results[0].to_v128().unwrap().to_f32x4();
            for lane in 0..4 {
                let expected = host(a[lane], b[lane]);
                assert_eq!(
                    actual[lane].to_bits(),
                    expected.to_bits(),
                    "{name} lane {lane} of {a:?}, {b:?}: got {:?}, want {expected:?}",
                    actual[lane]
                );
            }
        }
    }
}

#[test]
fn scalar_f32_ops() {
    scalar_f32_un_op(0x00, f32::sin, "f32_sin");
    scalar_f32_un_op(0x01, f32::cos, "f32_cos");
    scalar_f32_un_op(0x02, f32::tan, "f32_tan");
    scalar_f32_un_op(0x03, f32::asin, "f32_asin");
    scalar_f32_un_op(0x04, f32::acos, "f32_acos");
    scalar_f32_un_op(0x05, f32::atan, "f32_atan");
    scalar_f32_un_op(0x06, f32::exp, "f32_exp");
    scalar_f32_un_op(0x07, f32::ln, "f32_ln");
    scalar_f32_bin_op(0x08, f32::atan2, "f32_atan2");
    scalar_f32_bin_op(0x09, f32::powf, "f32_pow");
    scalar_f32_bin_op(0x0A, f32::min, "f32_rmin");
    scalar_f32_bin_op(0x0B, f32::max, "f32_rmax");
    scalar_f32_bin_op(0x0C, |a, b| a % b, "f32_rem");
    scalar_f32_bin_op_imm(0x08, f32::atan2, "f32_atan2");
    scalar_f32_bin_op_imm(0x09, f32::powf, "f32_pow");
    scalar_f32_bin_op_imm(0x0A, f32::min, "f32_rmin");
    scalar_f32_bin_op_imm(0x0B, f32::max, "f32_rmax");
    scalar_f32_bin_op_imm(0x0C, |a, b| a % b, "f32_rem");
}

#[test]
fn scalar_f64_ops() {
    scalar_f64_un_op(0x10, f64::sin, "f64_sin");
    scalar_f64_un_op(0x11, f64::cos, "f64_cos");
    scalar_f64_un_op(0x12, f64::tan, "f64_tan");
    scalar_f64_un_op(0x13, f64::asin, "f64_asin");
    scalar_f64_un_op(0x14, f64::acos, "f64_acos");
    scalar_f64_un_op(0x15, f64::atan, "f64_atan");
    scalar_f64_un_op(0x16, f64::exp, "f64_exp");
    scalar_f64_un_op(0x17, f64::ln, "f64_ln");
    scalar_f64_bin_op(0x18, f64::atan2, "f64_atan2");
    scalar_f64_bin_op(0x19, f64::powf, "f64_pow");
    scalar_f64_bin_op(0x1A, f64::min, "f64_rmin");
    scalar_f64_bin_op(0x1B, f64::max, "f64_rmax");
    scalar_f64_bin_op(0x1C, |a, b| a % b, "f64_rem");
}

#[test]
fn packed_f32x4_ops() {
    packed_un_op(0x20, f32::sin, "f32x4_sin");
    packed_un_op(0x21, f32::cos, "f32x4_cos");
    packed_un_op(0x22, f32::tan, "f32x4_tan");
    packed_un_op(0x23, f32::asin, "f32x4_asin");
    packed_un_op(0x24, f32::acos, "f32x4_acos");
    packed_un_op(0x25, f32::atan, "f32x4_atan");
    packed_un_op(0x26, f32::exp, "f32x4_exp");
    packed_un_op(0x27, f32::ln, "f32x4_ln");
    packed_bin_op(0x28, f32::atan2, "f32x4_atan2");
    packed_bin_op(0x29, f32::powf, "f32x4_pow");
    packed_bin_op(0x2A, f32::min, "f32x4_rmin");
    packed_bin_op(0x2B, f32::max, "f32x4_rmax");
    packed_bin_op(0x2C, |a, b| a % b, "f32x4_rem");
}

fn packed_dot_op(sub: u8, w: usize, name: &str) {
    let bytes = build_module(&[0x7B, 0x7B], &[0x7D], &[0x20, 0x00, 0x20, 0x01, EXT, sub]);
    let mut runner = Runner::new(&bytes);
    for chunk_a in F32_EDGES.chunks(4) {
        for chunk_b in F32_EDGES.chunks(4) {
            let mut a = [0f32; 4];
            a[..chunk_a.len()].copy_from_slice(chunk_a);
            let mut b = [0f32; 4];
            b[..chunk_b.len()].copy_from_slice(chunk_b);
            let mut results = [Val::F32(0.0)];
            runner.call(
                &[Val::V128(V128::from_f32x4(a)), Val::V128(V128::from_f32x4(b))],
                &mut results,
            );
            let actual = results[0].to_f32().unwrap();
            // Left-associated host reference.
            let mut expected = a[0] * b[0];
            for lane in 1..w {
                expected += a[lane] * b[lane];
            }
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "{name}({a:?}, {b:?}): got {actual:?}, want {expected:?}"
            );
        }
    }
}

#[test]
fn packed_dot_reductions() {
    packed_dot_op(0x2D, 2, "f32x4_dot2");
    packed_dot_op(0x2E, 3, "f32x4_dot3");
    packed_dot_op(0x2F, 4, "f32x4_dot4");
}

/// A module using a 0xE0 opcode must be rejected by an engine without
/// `ext_math` — standard Wasm behavior is unchanged.
#[test]
fn ext_math_is_gated() {
    let bytes = build_module(&[0x7D], &[0x7D], &[0x20, 0x00, EXT, 0x00]);
    // With the extension: fine.
    let engine = Engine::new_with_extensions(Extensions { ext_math: true });
    assert!(Module::new(&engine, &bytes).is_ok());
    // Without: "illegal opcode".
    let engine = Engine::new();
    assert!(Module::new(&engine, &bytes).is_err());
}
