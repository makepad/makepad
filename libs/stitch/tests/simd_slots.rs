//! Layer: slots.
//!
//! Adversarial tests for v128 values interacting with stitch's stack-slot
//! and register allocation. A v128 occupies exactly one 16-byte stack slot
//! (see stack.rs) and is never register-resident; these tests attack the
//! places where that could go wrong: results aliasing their own input
//! slots, register preservation around v128 ops, packed values living
//! across branches, merges and loop back-edges, deep expression nesting,
//! and v128 crossing call frames.
//!
//! All modules are written in WAT (encoded with the `wast` crate) using
//! only spec SIMD instructions, so failures localize to the v128 execution
//! machinery, not to the decoder or the nonstandard ops.

use makepad_stitch::{Engine, Instance, Linker, Module, Store, V128, Val};

struct Rig {
    store: Store,
    instance: Instance,
}

impl Rig {
    fn new(wat: &str) -> Rig {
        let buf = wast::parser::ParseBuffer::new(wat).unwrap();
        let mut wat = wast::parser::parse::<wast::Wat>(&buf).unwrap();
        let bytes = wat.encode().unwrap();
        let engine = Engine::new();
        let mut store = Store::new(engine.clone());
        let module = Module::new(&engine, &bytes).unwrap();
        let instance = Linker::new().instantiate(&mut store, &module).unwrap();
        Rig { store, instance }
    }

    fn call_v(&mut self, name: &str, args: &[Val]) -> [f32; 4] {
        let func = self.instance.exported_func(name).unwrap();
        let mut results = [Val::V128(V128::ZERO)];
        func.call(&mut self.store, args, &mut results).unwrap();
        results[0].to_v128().unwrap().to_f32x4()
    }

    fn call_f32(&mut self, name: &str, args: &[Val]) -> f32 {
        let func = self.instance.exported_func(name).unwrap();
        let mut results = [Val::F32(0.0)];
        func.call(&mut self.store, args, &mut results).unwrap();
        results[0].to_f32().unwrap()
    }
}

fn v(lanes: [f32; 4]) -> Val {
    Val::V128(V128::from_f32x4(lanes))
}

/// Result slot aliasing an input slot: `a = a.yzxw * a` style — the result
/// of the shuffle lands in a temp that the multiply then reuses as its own
/// destination.
#[test]
fn slot_alias_shuffle_self_multiply() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "f") (param v128) (result v128)
                (f32x4.mul
                    (i8x16.shuffle 4 5 6 7 8 9 10 11 0 1 2 3 12 13 14 15
                        (local.get 0) (local.get 0))
                    (local.get 0))))"#,
    );
    let a = [2.0f32, 3.0, 5.0, 7.0];
    let out = rig.call_v("f", &[v(a)]);
    // yzxw * xyzw
    assert_eq!(out, [3.0 * 2.0, 5.0 * 3.0, 2.0 * 5.0, 7.0 * 7.0]);
}

/// `v = v * splat(dot(v, w))` — a scalar extracted from packed math flows
/// through the float register while packed temporaries are live.
#[test]
fn scalar_register_crosses_packed_ops() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "f") (param v128 v128) (result v128)
                (local $t v128)
                (local.set $t (f32x4.mul (local.get 0) (local.get 1)))
                (f32x4.mul
                    (local.get 0)
                    (f32x4.splat
                        (f32.add
                            (f32.add
                                (f32x4.extract_lane 0 (local.get $t))
                                (f32x4.extract_lane 1 (local.get $t)))
                            (f32x4.extract_lane 2 (local.get $t)))))))"#,
    );
    let a = [1.0f32, 2.0, 3.0, 0.0];
    let b = [4.0f32, 5.0, 6.0, 0.0];
    let dot = 1.0f32 * 4.0 + 2.0 * 5.0 + 3.0 * 6.0;
    let out = rig.call_v("f", &[v(a), v(b)]);
    assert_eq!(out, [1.0 * dot, 2.0 * dot, 3.0 * dot, 0.0]);
}

/// Typed and untyped select over v128, with the condition both on the
/// stack and in the integer register.
#[test]
fn select_v128_all_condition_kinds() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "typed") (param v128 v128 i32) (result v128)
                (select (result v128) (local.get 0) (local.get 1) (local.get 2)))
            (func (export "untyped") (param v128 v128 i32) (result v128)
                (select (local.get 0) (local.get 1) (local.get 2)))
            (func (export "cond_in_reg") (param v128 v128 i32) (result v128)
                (select (result v128)
                    (local.get 0)
                    (local.get 1)
                    (i32.add (local.get 2) (i32.const 0)))))"#,
    );
    let a = [1.0f32, 2.0, 3.0, 4.0];
    let b = [5.0f32, 6.0, 7.0, 8.0];
    for name in ["typed", "untyped", "cond_in_reg"] {
        assert_eq!(rig.call_v(name, &[v(a), v(b), Val::I32(1)]), a, "{name}, cond=1");
        assert_eq!(rig.call_v(name, &[v(a), v(b), Val::I32(0)]), b, "{name}, cond=0");
    }
}

/// v128 as a block/if result and live across a branch merge point.
#[test]
fn v128_across_branches() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "if_result") (param v128 v128 i32) (result v128)
                (if (result v128) (local.get 2)
                    (then (f32x4.add (local.get 0) (local.get 1)))
                    (else (f32x4.sub (local.get 0) (local.get 1)))))
            (func (export "br_result") (param v128 i32) (result v128)
                (block (result v128)
                    (f32x4.neg (local.get 0))
                    (br_if 0 (local.get 1))
                    (drop)
                    (local.get 0))))"#,
    );
    let a = [1.0f32, 2.0, 3.0, 4.0];
    let b = [0.5f32, 0.25, 0.125, 8.0];
    assert_eq!(
        rig.call_v("if_result", &[v(a), v(b), Val::I32(1)]),
        [1.5, 2.25, 3.125, 12.0]
    );
    assert_eq!(
        rig.call_v("if_result", &[v(a), v(b), Val::I32(0)]),
        [0.5, 1.75, 2.875, -4.0]
    );
    assert_eq!(
        rig.call_v("br_result", &[v(a), Val::I32(1)]),
        [-1.0, -2.0, -3.0, -4.0]
    );
    assert_eq!(rig.call_v("br_result", &[v(a), Val::I32(0)]), a);
}

/// A v128 accumulator across a loop back-edge, with scalar loop state in
/// the integer register.
#[test]
fn v128_loop_accumulator() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "f") (param v128 i32) (result v128)
                (local $acc v128)
                (local $i i32)
                (local.set $acc (v128.const f32x4 0 0 0 0))
                (block
                    (loop
                        (br_if 1 (i32.ge_u (local.get $i) (local.get 1)))
                        (local.set $acc (f32x4.add (local.get $acc) (local.get 0)))
                        (local.set $i (i32.add (local.get $i) (i32.const 1)))
                        (br 0)))
                (local.get $acc)))"#,
    );
    let a = [1.0f32, 2.0, 3.0, 4.0];
    assert_eq!(rig.call_v("f", &[v(a), Val::I32(5)]), [5.0, 10.0, 15.0, 20.0]);
    assert_eq!(rig.call_v("f", &[v(a), Val::I32(0)]), [0.0, 0.0, 0.0, 0.0]);
}

/// Many live packed temporaries at once: an 8-deep tree of adds where
/// every leaf is a distinct shuffle of the input, forcing a tall stack of
/// v128 temp slots.
#[test]
fn deep_packed_temporaries() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "f") (param v128) (result v128)
                (f32x4.add
                    (f32x4.add
                        (f32x4.add
                            (local.get 0)
                            (i8x16.shuffle 4 5 6 7 0 1 2 3 12 13 14 15 8 9 10 11
                                (local.get 0) (local.get 0)))
                        (f32x4.add
                            (i8x16.shuffle 8 9 10 11 12 13 14 15 0 1 2 3 4 5 6 7
                                (local.get 0) (local.get 0))
                            (i8x16.shuffle 12 13 14 15 8 9 10 11 4 5 6 7 0 1 2 3
                                (local.get 0) (local.get 0))))
                    (f32x4.add
                        (f32x4.add
                            (f32x4.mul (local.get 0) (local.get 0))
                            (f32x4.sub (local.get 0) (local.get 0)))
                        (f32x4.add
                            (f32x4.neg (local.get 0))
                            (f32x4.abs (local.get 0)))))))"#,
    );
    let a = [1.0f32, -2.0, 3.0, -4.0];
    // Sum of all four rotations of a is the lane-sum in every lane.
    let lane_sum = 1.0f32 + -2.0 + 3.0 + -4.0;
    let expected: Vec<f32> = (0..4)
        .map(|lane| {
            let x: f32 = a[lane];
            lane_sum + (x * x + 0.0) + (-x + x.abs())
        })
        .collect();
    assert_eq!(rig.call_v("f", &[v(a)]).to_vec(), expected);
}

/// Scalar and packed values interleaved: f32, i32 and v128 temporaries all
/// live at once, in both orders, so register preservation has to happen
/// around v128 stack traffic.
#[test]
fn interleaved_scalar_and_packed() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "f") (param f32 v128 f32 v128 i32) (result f32)
                (f32.add
                    (f32.mul
                        (local.get 0)
                        (f32x4.extract_lane 1 (f32x4.add (local.get 1) (local.get 3))))
                    (f32.mul
                        (local.get 2)
                        (f32.convert_i32_s (local.get 4))))))"#,
    );
    let out = rig.call_f32(
        "f",
        &[
            Val::F32(2.0),
            v([1.0, 10.0, 3.0, 4.0]),
            Val::F32(0.5),
            v([5.0, 20.0, 7.0, 8.0]),
            Val::I32(6),
        ],
    );
    assert_eq!(out, 2.0 * 30.0 + 0.5 * 6.0);
}

/// v128 params and results crossing a wasm-to-wasm call frame.
#[test]
fn v128_across_calls() {
    let mut rig = Rig::new(
        r#"(module
            (func $inner (param v128 f32) (result v128)
                (f32x4.mul (local.get 0) (f32x4.splat (local.get 1))))
            (func (export "f") (param v128) (result v128)
                (f32x4.add
                    (call $inner (local.get 0) (f32.const 2.0))
                    (call $inner (local.get 0) (f32.const 3.0)))))"#,
    );
    let a = [1.0f32, 2.0, 3.0, 4.0];
    assert_eq!(rig.call_v("f", &[v(a)]), [5.0, 10.0, 15.0, 20.0]);
}

/// local.tee with v128 while another v128 opd refers to the same local
/// (the preserve-local path).
#[test]
fn v128_local_tee_preserve() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "f") (param v128) (result v128)
                (local $x v128)
                (local.set $x (local.get 0))
                ;; push $x as a local operand, then overwrite $x, then use both
                (f32x4.sub
                    (local.get $x)
                    (local.tee $x (f32x4.add (local.get $x) (local.get $x))))))"#,
    );
    let a = [1.0f32, 2.0, 3.0, 4.0];
    // old_x - (x = old_x + old_x)  =>  -old_x
    assert_eq!(rig.call_v("f", &[v(a)]), [-1.0, -2.0, -3.0, -4.0]);
}

/// v128.load / v128.store against linear memory, including the unaligned
/// and offset forms and the OOB trap boundary.
#[test]
fn v128_memory_roundtrip() {
    let mut rig = Rig::new(
        r#"(module
            (memory 1)
            (func (export "rt") (param i32 v128) (result v128)
                (v128.store (local.get 0) (local.get 1))
                (v128.load (local.get 0)))
            (func (export "rt_offset") (param i32 v128) (result v128)
                (v128.store offset=4 (local.get 0) (local.get 1))
                (v128.load offset=4 (local.get 0))))"#,
    );
    let a = [1.5f32, -2.5, 3.25, f32::NAN];
    for addr in [0u32, 4, 6, 65536 - 16] {
        let out = rig.call_v("rt", &[Val::I32(addr as i32), v(a)]);
        assert_eq!(out[..3], a[..3], "addr={addr}");
        assert!(out[3].is_nan());
    }
    let out = rig.call_v("rt_offset", &[Val::I32(16), v(a)]);
    assert_eq!(out[..3], a[..3]);
    // One byte past the end must trap.
    let func = rig.instance.exported_func("rt").unwrap();
    let mut results = [Val::V128(V128::ZERO)];
    assert!(func
        .call(
            &mut rig.store,
            &[Val::I32(65536 - 15), v(a)],
            &mut results
        )
        .is_err());
}

/// v128.const materialization interleaved with locals and shuffles.
#[test]
fn v128_const_and_bitselect() {
    let mut rig = Rig::new(
        r#"(module
            (func (export "f") (param v128) (result v128)
                (v128.bitselect
                    (local.get 0)
                    (v128.const f32x4 9 9 9 9)
                    (v128.const i32x4 0xFFFFFFFF 0 0xFFFFFFFF 0))))"#,
    );
    let a = [1.0f32, 2.0, 3.0, 4.0];
    assert_eq!(rig.call_v("f", &[v(a)]), [1.0, 9.0, 3.0, 9.0]);
}
