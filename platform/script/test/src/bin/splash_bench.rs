// Splash interpreter micro-benchmarks.
//
// Each benchmark evals a script that defines a function `f`, then times
// repeated `vm.call(f, ..)` invocations so tokenize/parse cost is excluded.
// Workloads model what game scripts do per tick: loops, field access,
// method calls, object churn, array walks and host->script call overhead.
//
// Run: cargo run -p makepad-script-test --bin splash_bench --release

use makepad_script::*;
use std::time::Instant;

struct BenchResult {
    name: &'static str,
    ns_per_op: f64,
    ops: f64,
    check: ScriptValue,
}

fn new_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

// Evals `code` (which must end with an expression yielding a function),
// then calls it `outer` times, timing the best of `rounds` rounds.
// `ops` is the number of logical operations one call performs.
fn bench_call(
    vm: &mut ScriptVm,
    name: &'static str,
    ops: f64,
    rounds: usize,
    outer: usize,
    code: ScriptMod,
) -> BenchResult {
    // BENCH_ONLY=<name> loops one workload forever (for profilers)
    let only = std::env::var("BENCH_ONLY").ok();
    if let Some(only) = &only {
        if only != name {
            return BenchResult {
                name,
                ns_per_op: 0.0,
                ops: 0.0,
                check: NIL,
            };
        }
    }
    let f = vm.eval(code);
    if only.is_some() {
        loop {
            std::hint::black_box(vm.call(f, &[]));
        }
    }
    if f.is_err() || f.is_nil() {
        vm.drain_errors();
        panic!("bench {} did not produce a function: {:?}", name, f);
    }
    let mut best = f64::MAX;
    let mut check = NIL;
    for _ in 0..rounds {
        let start = Instant::now();
        for _ in 0..outer {
            check = vm.call(f, &[]);
        }
        let el = start.elapsed().as_secs_f64();
        let ns = el * 1e9 / (ops * outer as f64);
        if ns < best {
            best = ns;
        }
        vm.gc();
    }
    if check.is_err() {
        vm.drain_errors();
        panic!("bench {} returned error", name);
    }
    BenchResult {
        name,
        ns_per_op: best,
        ops: ops * outer as f64,
        check,
    }
}

fn main() {
    let vm = &mut new_vm();
    let mut results = Vec::new();

    // BENCH_DUMP=1: eval a probe script and dump its opcodes, then exit.
    // Used to verify what the slot resolver emitted.
    if std::env::var("BENCH_DUMP").is_ok() {
        let v = vm.eval(script! {
            let fib = |n| {
                if n < 2 {
                    return n
                }
                return fib(n - 1) + fib(n - 2)
            }
            let f = |dt| {
                let total = 0.0
                for a in 4 {
                    let dx = a * 2.0
                    let dz = a + 1.0
                    let best = 100.0
                    for b in 4 {
                        let d = dx * dz + b
                        if d < best { best = d }
                    }
                    total += best
                }
                total
            }
            f(0.016) + fib(6)
        });
        vm.drain_errors();
        println!("eval result: {}", v);
        let bodies = vm.bx.code.bodies.borrow();
        bodies[0].parser.dump_opcodes();
        return;
    }

    // 1. Arithmetic in a for-range loop: loop machinery + scope + arith.
    results.push(bench_call(
        vm,
        "for_range_arith",
        100_000.0,
        5,
        1,
        script! {
            let f = || {
                let acc = 0.0
                for i in 100000 {
                    acc += i * 0.5
                }
                acc
            }
            f
        },
    ));

    // 2. While loop: same work, different loop machinery.
    results.push(bench_call(
        vm,
        "while_arith",
        100_000.0,
        5,
        1,
        script! {
            let f = || {
                let acc = 0.0
                let i = 0
                while i < 100000 {
                    acc += i * 0.5
                    i += 1
                }
                acc
            }
            f
        },
    ));

    // 3. Scope variable resolution at depth (outer vars read in inner loop).
    results.push(bench_call(
        vm,
        "scope_depth_read",
        100_000.0,
        5,
        1,
        script! {
            let f = || {
                let a = 1.0
                let b = 2.0
                let c = 3.0
                let acc = 0.0
                for i in 100000 {
                    acc += a + b + c
                }
                acc
            }
            f
        },
    ));

    // 4. Script->script function calls (recursion): call/scope machinery.
    results.push(bench_call(
        vm,
        "fib_20",
        21891.0, // number of calls fib(20) performs
        5,
        1,
        script! {
            let fib = |n| {
                if n < 2 {
                    return n
                }
                return fib(n - 1) + fib(n - 2)
            }
            let f = || fib(20)
            f
        },
    ));

    // 5. Field read+write on an object in a loop (entity-style access).
    results.push(bench_call(
        vm,
        "field_rw",
        100_000.0,
        5,
        1,
        script! {
            let f = || {
                let o = { x: 0.0, y: 1.5 }
                for i in 100000 {
                    o.x = o.x + o.y
                }
                o.x
            }
            f
        },
    ));

    // 6. Method calls on an object in a loop.
    results.push(bench_call(
        vm,
        "method_call",
        50_000.0,
        5,
        1,
        script! {
            let obj = {
                x: 0.0,
                inc: |v| {
                    self.x += v
                    self.x
                }
            }
            let f = || {
                obj.x = 0.0
                for i in 50000 {
                    obj.inc(1.0)
                }
                obj.x
            }
            f
        },
    ));

    // 7. Object literal churn: create short-lived objects per iteration.
    results.push(bench_call(
        vm,
        "object_churn",
        50_000.0,
        5,
        1,
        script! {
            let f = || {
                let acc = 0.0
                for i in 50000 {
                    let p = { x: i, y: i * 2.0 }
                    acc += p.x + p.y
                }
                acc
            }
            f
        },
    ));

    // 8. Array iteration (for v in array).
    results.push(bench_call(
        vm,
        "array_iter",
        100_000.0,
        5,
        1,
        script! {
            let arr = []
            for i in 10000 { arr.push(i * 1.0) }
            let f = || {
                let acc = 0.0
                for r in 10 {
                    for v in arr {
                        acc += v
                    }
                }
                acc
            }
            f
        },
    ));

    // 9. Array index access in a range loop.
    results.push(bench_call(
        vm,
        "array_index",
        100_000.0,
        5,
        1,
        script! {
            let arr = []
            for i in 10000 { arr.push(i * 1.0) }
            let f = || {
                let acc = 0.0
                for r in 10 {
                    for i in 10000 {
                        acc += arr[i]
                    }
                }
                acc
            }
            f
        },
    ));

    // 9b. String-tag dispatch: mostly-failing string compares, the sandbox3d
    // brain-selector pattern (a.kind == "villager" / "critter" / ...).
    results.push(bench_call(
        vm,
        "string_cmp",
        100_000.0,
        5,
        1,
        script! {
            let kinds = ["villager", "critter", "nightmarehuggy", "guardian"]
            let f = || {
                let c = 0.0
                for r in 25000 {
                    for k in kinds {
                        if k == "guardian" { c += 1 }
                    }
                }
                c
            }
            f
        },
    ));

    // 10. Composite: sandbox3d-style game tick. 40 actors in an array,
    // string-kind brain dispatch, field math, an N^2 neighbor scan for one
    // kind, and native "verb" calls (mod.bench.nudge) like game.* verbs.
    {
        let bench_mod = vm.new_module(id!(bench));
        vm.add_method(bench_mod, id!(nudge), &[], |_vm, _args| NIL);
    }
    results.push(bench_call(
        vm,
        "game_tick_40",
        1.0,
        5,
        1000,
        script! {
            let actors = []
            for i in 40 {
                let kind = "car"
                let m = i % 4
                if m == 0 { kind = "villager" }
                else if m == 1 { kind = "critter" }
                else if m == 2 { kind = "guard" }
                actors.push({
                    id: i, kind: kind,
                    x: i * 3.0, y: 0.0, z: i * 7.0,
                    vx: 0.1, vz: 0.2,
                    hp: 100.0, t: 0.0
                })
            }
            let f = |dt| {
                for a in actors {
                    a.t += dt
                    if a.kind == "villager" {
                        a.x += a.vx * dt
                        a.z += a.vz * dt
                        let best = 1000000000.0
                        for b in actors {
                            if b.id != a.id {
                                let dx = b.x - a.x
                                let dz = b.z - a.z
                                let d = dx * dx + dz * dz
                                if d < best { best = d }
                            }
                        }
                        a.hp = best
                    } else if a.kind == "critter" {
                        a.x += a.vx
                        mod.bench.nudge(a.id, a.x, a.z)
                    } else if a.kind == "guard" {
                        if a.t > 1.0 { a.t = 0.0 }
                        mod.bench.nudge(a.id, a.x, a.z)
                    } else {
                        a.z += a.vz
                    }
                }
                actors[0].x
            }
            let f2 = || f(0.016)
            f2
        },
    ));

    // 11. Same loop as for_range_arith but executed the way the game host
    // runs scripts: instruction limit + wall-clock run budget installed.
    // The delta vs for_range_arith is the per-instruction accounting cost.
    if std::env::var("BENCH_ONLY").is_err() {
        let f = vm.eval(script! {
            let f = || {
                let acc = 0.0
                for i in 100000 {
                    acc += i * 0.5
                }
                acc
            }
            f
        });
        let mut best = f64::MAX;
        for _ in 0..5 {
            let start = Instant::now();
            // long deadlines: we want the per-instruction sampling cost, not
            // actual trips (a loaded machine can deschedule us past 64ms)
            vm.bx.run_budget = Some(ScriptRunBudget::from_durations(
                std::time::Duration::from_secs(10),
                std::time::Duration::from_secs(10),
                512,
            ));
            let check = vm.with_instruction_limit(1_000_000_000, |vm| vm.call(f, &[]));
            vm.bx.run_budget = None;
            let el = start.elapsed().as_secs_f64();
            assert!(!check.is_err());
            let ns = el * 1e9 / 100_000.0;
            if ns < best {
                best = ns;
            }
            vm.gc();
        }
        results.push(BenchResult {
            name: "arith_hostmode",
            ns_per_op: best,
            ops: 100_000.0,
            check: NIL,
        });
    }

    // 12. Host -> script call overhead: trivial fn called from Rust.
    {
        let f = vm.eval(script! {
            let f = |x| x
            f
        });
        let mut best = f64::MAX;
        let n = 100_000usize;
        for _ in 0..5 {
            let start = Instant::now();
            let mut check = NIL;
            for i in 0..n {
                check = vm.call(f, &[ScriptValue::from_f64(i as f64)]);
            }
            let el = start.elapsed().as_secs_f64();
            std::hint::black_box(check);
            let ns = el * 1e9 / n as f64;
            if ns < best {
                best = ns;
            }
            vm.gc();
        }
        results.push(BenchResult {
            name: "host_call",
            ns_per_op: best,
            ops: n as f64,
            check: NIL,
        });
    }

    // Report
    println!("\n=== splash_bench results (best of rounds, ns/op) ===");
    for r in &results {
        println!(
            "{:>18}  {:>10.1} ns/op   ({:.0} ops, check={})",
            r.name, r.ns_per_op, r.ops, r.check
        );
    }
    // machine-readable line for diffing
    print!("CSV:");
    for r in &results {
        print!("{}={:.1};", r.name, r.ns_per_op);
    }
    println!();
}
