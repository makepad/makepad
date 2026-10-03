//! Hostile host calls: hand-built AIR the validator must refuse (a foreign
//! index, wrong arity, bad slices, results used before the call or defined
//! twice, a call in an audio program), declared costs that must bound what
//! a built-in really takes on adversarial input, and a slow component
//! stopped by a cancel within the admitted element latency.

use makepad_script_compute::admission::{DeviceLimits, JobBudget, Ledger, Origin};
use makepad_script_compute::host::{self, HostError, HostFn, HostSlice, SliceRaw, SliceSig, Tier};
use makepad_script_compute::ir::{self, Op, Program, RawBuf, Regions, SliceArg, Stmt, Ty, Val};
use makepad_script_compute::kernel::{compile_with, KernelError};
use makepad_script_compute::Backend;

fn regions(io: bool) -> Regions {
    Regions { ctx: 32, state: 4, shared: 4, frame: 4, shared_writable: false, bufs: vec![false, false, true], io }
}

/// `v0 = 0; v1 = 4; [v2 =] call f(slices ...)` with the given pieces.
fn call(f: u16, args: Vec<Val>, slices: Vec<SliceArg>, rets: Vec<Val>, after: Vec<Stmt>) -> Program {
    let mut body = vec![Stmt::Def(Val(0), Op::ConstI(0)), Stmt::Def(Val(1), Op::ConstI(4))];
    body.push(Stmt::CallHost { f, args, slices, rets });
    body.extend(after);
    Program { vals: vec![Ty::I32, Ty::I32, Ty::I32, Ty::I32], vars: vec![], body, frame_words: 4, ..Default::default() }
}

fn fill(args: &[u32], s: &[HostSlice], rets: &mut [u32]) -> Result<(), HostError> {
    for i in 0..s[0].len() {
        s[0].set(i, args[0]);
    }
    rets[0] = s[0].len() as u32;
    Ok(())
}

fn hostile_fill() -> u16 {
    host::register(HostFn {
        name: "hostile.fill",
        params: &[Ty::I32],
        slices: &[SliceSig { max_words: 8, writable: true }],
        rets: &[Ty::I32],
        cost: |lens| 4 + 2 * lens[0] as u64,
        misses: 1,
        tier: Tier::X,
        call: fill,
        doc: "",
    })
    .unwrap()
}

#[test]
fn the_validator_refuses_hostile_calls() {
    let f = hostile_fill();
    let slice = |buf: u8| SliceArg { buf, off: Val(0), len: Val(1) };
    let ok = call(f, vec![Val(0)], vec![slice(2)], vec![Val(2)], vec![Stmt::Def(Val(3), Op::Get(ir::Var(0)))]);
    // The well-formed call (with a harmless trailing statement dropped).
    let mut good = ok.clone();
    good.body.pop();
    assert!(ir::validate(&good, &regions(false)).is_ok(), "{:?}", ir::validate(&good, &regions(false)));
    let cases: Vec<(&str, Program, Regions)> = vec![
        ("foreign index", call(u16::MAX - 1, vec![Val(0)], vec![slice(2)], vec![Val(2)], vec![]), regions(false)),
        ("too few args", call(f, vec![], vec![slice(2)], vec![Val(2)], vec![]), regions(false)),
        ("too many slices", call(f, vec![Val(0)], vec![slice(2), slice(2)], vec![Val(2)], vec![]), regions(false)),
        ("no result", call(f, vec![Val(0)], vec![slice(2)], vec![], vec![]), regions(false)),
        ("slice of a buffer that does not exist", call(f, vec![Val(0)], vec![slice(9)], vec![Val(2)], vec![]), regions(false)),
        ("writes a read-only buffer", call(f, vec![Val(0)], vec![slice(1)], vec![Val(2)], vec![]), regions(false)),
        ("writes the control word", call(f, vec![Val(0)], vec![slice(0)], vec![Val(2)], vec![]), regions(false)),
        ("argument not defined", call(f, vec![Val(3)], vec![slice(2)], vec![Val(2)], vec![]), regions(false)),
        ("result defined twice", call(f, vec![Val(0)], vec![slice(2)], vec![Val(0)], vec![]), regions(false)),
        ("in an audio program", call(f, vec![Val(0)], vec![slice(2)], vec![Val(2)], vec![]), regions(true)),
    ];
    for (name, p, r) in cases {
        assert!(ir::validate(&p, &r).is_err(), "{name}: accepted");
    }
    // A result used before the call that defines it.
    let mut early = good.clone();
    early.body.insert(2, Stmt::Def(Val(3), Op::Bin(ir::Bin::AddI, Val(2), Val(1))));
    assert!(ir::validate(&early, &regions(false)).is_err(), "a result used before its call");
    // A result defined inside a branch is not visible after it.
    let mut scoped = good.clone();
    let c = scoped.body.pop().unwrap();
    scoped.vals.push(Ty::Bool);
    scoped.body.push(Stmt::Def(Val(4), Op::ConstB(true)));
    scoped.body.push(Stmt::If(Val(4), vec![c], vec![]));
    scoped.body.push(Stmt::Def(Val(3), Op::Bin(ir::Bin::AddI, Val(2), Val(1))));
    assert!(ir::validate(&scoped, &regions(false)).is_err(), "a branch-local result used after the branch");
}

#[test]
fn run_time_slices_never_leave_their_buffer() {
    let f = hostile_fill();
    // Guard words on both sides; hostile offsets and lengths.
    for (off, len) in [(0u32, 8u32), (6, 100), (u32::MAX, 4), (3, u32::MAX), (7, 1), (8, 1)] {
        let mut mem = vec![0xAAAA_AAAAu32; 10];
        let inner = mem[1..9].as_mut_ptr();
        let bufs = [RawBuf { ptr: std::ptr::null_mut(), len: 0, writable: false }, RawBuf { ptr: inner, len: 8, writable: true }];
        let mut rets = [0u32];
        let ok = host::invoke(f, &[7], &[SliceRaw { buf: 1, off, len }], &mut rets, &bufs);
        assert!(mem[0] == 0xAAAA_AAAA && mem[9] == 0xAAAA_AAAA, "({off}, {len}) wrote outside");
        let fits = (off as u64) + (len as u64) <= 8;
        assert_eq!(ok, fits, "({off}, {len}): a clamped slice must be reported");
        // A slice on a buffer index that is not bound fails, writes nothing.
        let ok = host::invoke(f, &[7], &[SliceRaw { buf: 5, off: 0, len: 1 }], &mut rets, &bufs);
        assert!(!ok && rets[0] == 0);
    }
}

/// Points in the adversarial polygons.
const N: usize = 1024;

/// A polygon that is hard for ear clipping: a comb of reflex teeth (few
/// ears, long scans), a self-intersecting 97-turn star (no ears), and a
/// spiky star.
fn adversarial(kind: u32) -> Vec<f32> {
    let n = N;
    let mut pts = Vec::with_capacity(2 * n);
    for i in 0..n {
        let t = i as f32 / n as f32;
        let (x, y) = match kind {
            0 => (t * 100.0, if i % 2 == 0 { 0.0 } else { 50.0 + (i % 7) as f32 }),
            1 => {
                let a = t * std::f32::consts::TAU * 97.0;
                (a.cos() * 50.0, a.sin() * 50.0)
            }
            _ => {
                let a = t * std::f32::consts::TAU;
                let r = if i % 2 == 0 { 50.0 } else { 5.0 };
                (a.cos() * r, a.sin() * r)
            }
        };
        pts.push(x);
        pts.push(y);
    }
    pts
}

#[test]
fn built_in_costs_bound_their_adversarial_work() {
    let f = host::find("poly.triangulate").unwrap();
    let h = host::get(f).unwrap();
    let words = [2 * N as u32, 3 * (N as u32 - 2)];
    // The declared cost at these input sizes (what a call is charged): the
    // ops it counts on adversarial inputs stay within it.
    let declared = h.cost_of(&words);
    for kind in 0..3 {
        let ran = host::triangulate_ops(&adversarial(kind));
        eprintln!("poly.triangulate adversarial {kind}: {ran} ops (declared worst {declared})");
        assert!(ran <= declared, "adversarial {kind}: ran {ran} ops, more than its declared {declared}");
    }
}

/// ~0.25 ms of work per call, declared with margin.
fn slow(args: &[u32], _s: &[HostSlice], rets: &mut [u32]) -> Result<(), HostError> {
    let mut x = args[0] as f64;
    for k in 0..50_000 {
        x = (x * 1.000_001 + k as f64).sqrt();
    }
    rets[0] = (x as f32).to_bits();
    Ok(())
}

#[test]
fn a_slow_component_is_admitted_by_its_cost_and_stopped_by_cancel() {
    let f = host::register(HostFn { name: "hostile.slow", params: &[Ty::I32], slices: &[], rets: &[Ty::F32], cost: |_| 1_200_000, misses: 0, tier: Tier::D, call: slow, doc: "" }).unwrap();
    assert!(host::get(f).is_some());
    let ledger = Ledger::new(DeviceLimits::default());
    let src = "let o = output(f32)\nfn element(i) { o[i] = hostile.slow(i) }";
    let twice = "let o = output(f32)\nfn element(i) { o[i] = hostile.slow(i) + hostile.slow(i + 1) }";
    let budget = JobBudget { work: 1 << 30 };
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(src, &[], backend).unwrap_or_else(|e| panic!("{:?}", e));
        assert!(ledger.admit(&k, 1, Origin::Ai, &budget).is_ok(), "one call fits the element bound");
        // Two calls per element: refused (here already by the compiler's
        // per-element cap, else by admission).
        let refused = compile_with(twice, &[], backend).map_or(true, |k2| ledger.admit(&k2, 1, Origin::Ai, &budget).is_err());
        assert!(refused, "two calls per element do not fit");
        let n = 50_000;
        let mut out = vec![0.0f32; n];
        let mut c = k.call();
        c.output("o", &mut out).unwrap();
        // A cancelled call runs no element (a cancel lands at the next
        // element: tests/admission.rs).
        c.cancel_token().cancel();
        let r = if backend == Backend::Interp { c.run_interp(n) } else { c.run(n) };
        assert!(matches!(r, Err(KernelError::Cancelled)));
        drop(c);
        assert!(out.iter().all(|x| *x == 0.0), "{:?}: nothing ran", backend);
    }
}

#[test]
fn an_untrusted_call_on_a_run_time_size_gets_a_per_call_limit() {
    // The slice length is data: admission cannot bound it statically, so the
    // ticket carries a per-call op limit that the call enforces.
    let src = "let pts = input(f32)\nlet tris = output(i32, 1, 0, tris)\nlet o = output(i32)\nfn element(i) { o[i] = poly.triangulate(pts, 0, int(pts[0]), tris, 0, 3000) }";
    let k = compile_with(src, &[], Backend::Native).unwrap_or_else(|e| panic!("{:?}", e));
    let ledger = Ledger::new(DeviceLimits::default());
    let budget = JobBudget { work: 1 << 30 };
    let t = ledger.admit(&k, 1, Origin::Ai, &budget).unwrap();
    let limit = t.host_call_limit();
    assert!(limit > 0, "untrusted open calls are limited");
    assert!(t.estimate().element_ops <= DeviceLimits::default().element_work, "the element bound holds with the limit");
    // Trusted code: no per-call limit (its budget bounds what it runs).
    assert_eq!(ledger.admit(&k, 1, Origin::Host, &budget).unwrap().host_call_limit(), 0, "trusted: no limit");
    let f = host::find("poly.triangulate").unwrap();
    let h = host::get(f).unwrap();
    // A small polygon fits the limit; a large one is refused before it runs.
    let small = [2 * 64u32, 3 * 62];
    let big = [2 * 60_000u32, 3000];
    assert!(h.cost_of(&small) <= limit && h.cost_of(&big) > limit, "limit {limit}");
    for (words, fits) in [(small, true), (big, false)] {
        let mut pts: Vec<f32> = (0..words[0] as usize / 2).flat_map(|i| {
            let a = i as f32 / (words[0] / 2) as f32 * std::f32::consts::TAU;
            [a.cos(), a.sin()]
        }).collect();
        let mut tris = vec![0u32; words[1] as usize];
        let bufs = [
            RawBuf { ptr: pts.as_mut_ptr() as *mut u32, len: pts.len(), writable: false },
            RawBuf { ptr: tris.as_mut_ptr(), len: tris.len(), writable: true },
        ];
        let mut rets = [0u32];
        let ok = host::invoke_limited(f, &[], &[SliceRaw { buf: 0, off: 0, len: words[0] }, SliceRaw { buf: 1, off: 0, len: words[1] }], &mut rets, &bufs, limit);
        assert_eq!(ok, fits, "{words:?}");
        if !fits {
            assert!(tris.iter().all(|w| *w == 0), "a refused call does not run");
        }
    }
}
