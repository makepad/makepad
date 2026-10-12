//! A host function called in a loop must not grow the native stack.
//!
//! Every threaded instruction ends in a sibling call to the next one. A
//! handler that keeps something alive in its frame across that call leaves
//! the frame behind, so a module that calls a host function many times in one
//! invocation runs out of native stack.

use makepad_stitch::{Engine, Func, Linker, Module, Store, Val};

const CALLS: i32 = 1_000_000;

const WAT: &str = r#"
(module
  (type $t (func (param i32) (result i32)))
  (import "env" "tick" (func $tick (type $t)))
  (table 1 funcref)
  (elem (i32.const 0) $tick)
  (func (export "direct") (param $n i32) (result i32)
    (local $i i32) (local $acc i32)
    (block $done
      (loop $next
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $acc (call $tick (local.get $acc)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $next)))
    (local.get $acc))
  (func (export "indirect") (param $n i32) (result i32)
    (local $i i32) (local $acc i32)
    (block $done
      (loop $next
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $acc (call_indirect (type $t) (local.get $acc) (i32.const 0)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $next)))
    (local.get $acc)))
"#;

/// Calls `export(CALLS)` on a thread with a 1 MiB stack: a leak of even a
/// few hundred bytes per host call overflows it long before the loop ends.
fn run(export: &'static str) -> i32 {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || {
            let buf = wast::parser::ParseBuffer::new(WAT).unwrap();
            let mut wat: wast::Wat = wast::parser::parse(&buf).unwrap();
            let bytes = wat.encode().unwrap();
            let engine = Engine::new();
            let mut store = Store::new(engine);
            let module = Module::new(store.engine(), &bytes).unwrap();
            let mut linker = Linker::new();
            let tick = Func::wrap(&mut store, |x: i32| -> i32 { x + 1 });
            linker.define("env", "tick", tick);
            let instance = linker.instantiate(&mut store, &module).unwrap();
            let mut results = [Val::I32(0)];
            instance
                .exported_func(export)
                .unwrap()
                .call(&mut store, &[Val::I32(CALLS)], &mut results)
                .unwrap();
            results[0].to_i32().unwrap()
        })
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn host_calls_in_a_loop_keep_the_native_stack_flat() {
    assert_eq!(run("direct"), CALLS);
}

#[test]
fn indirect_host_calls_in_a_loop_keep_the_native_stack_flat() {
    assert_eq!(run("indirect"), CALLS);
}
