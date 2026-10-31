//! Cross-width differential: each kernel is compiled with vector code of 4 lanes and of 8 lanes
//! (`native_vec::with_lanes`) and run there, on the scalar native code and on the reference
//! interpreter. Every output word and the counted work (`RunStats::work`: what each element
//! counted plus its loops' run-time count, ctx `K_WORK`) must be the same bits at both widths
//! and on the interpreter, on one thread and on four, at sizes around multiples of 4 and 8 (the
//! tails run scalar). Corpus: the AI-style kernels (tests/support/ai_corpus.rs), the geometry
//! kernels (heightfield one pass / heights / normals, instances), the AI hub's fractal tile
//! kernels (f64: scalar at every width; and their f32 forms, whose escape loops vectorize),
//! emit kernels and divergent-loop kernels.
#![cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]

use makepad_script_compute::kernel::{compile, Access, Kernel, KernelRef};
use makepad_script_compute::native_vec;
use makepad_script_compute_sched::kernel::KernelExt;

#[path = "support/ai_corpus.rs"]
mod ai_corpus;
#[path = "support/fractal.rs"]
mod fractal;
#[path = "support/geometry.rs"]
mod geometry;

#[derive(PartialEq, Debug)]
struct Out {
    words: Vec<Vec<u32>>,
    work: u64,
    overflowed: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Interp,
    Scalar,
    /// vector code (the kernel's own width), on this many threads
    Vector(usize),
}

/// Deterministic input words: mostly ordinary floats of both signs, some integers' bit patterns
/// (a buffer read as u32 sees them), a few zeros.
fn data(len: usize, salt: u32) -> Vec<u32> {
    (0..len as u32)
        .map(|j| {
            let h = (j ^ salt).wrapping_mul(0x9E37_79B1).rotate_left(13).wrapping_mul(0x85EB_CA77);
            match h % 16 {
                0 => 0,
                1 => h >> 8,
                _ => (((h >> 4) % 2000) as f32 * 0.01 - 10.0).to_bits(),
            }
        })
        .collect()
}

/// The fractal tiles' view (x0, y0, step, julia re, julia im) for a `res` x `res` tile.
fn view(f64_words: bool, res: u32) -> Vec<u32> {
    let v = [-2.1f64, -1.3, 2.6 / res as f64, -0.7269, 0.1889];
    if f64_words {
        v.iter().flat_map(|x| [x.to_bits() as u32, (x.to_bits() >> 32) as u32]).collect()
    } else {
        v.iter().map(|x| (*x as f32).to_bits()).collect()
    }
}

fn run(k: &Kernel, n: usize, params: &[(&str, f32)], mode: Mode) -> Out {
    // (the runtime's own buffers, `#control`, are bound by the call)
    let bufs: Vec<(String, Access, u32)> = k.buffer_views().filter(|b| !b.name.starts_with('#')).map(|b| (b.name.to_string(), b.access, b.stride)).collect();
    let res = params.iter().find(|p| p.0 == "res").map_or(256, |p| p.1 as u32);
    let mut ins: Vec<Vec<u32>> = Vec::new();
    let mut outs: Vec<Vec<u32>> = Vec::new();
    for (name, access, stride) in &bufs {
        let len = (n * *stride as usize).max(1);
        match access {
            Access::Read if name == "view" => ins.push(view(*stride >= 2, res)),
            Access::Read => ins.push(data(len, name.len() as u32 * 7919)),
            _ => outs.push(vec![0x5555_5555; len]),
        }
    }
    let stats = {
        let mut c = k.call();
        c.set_simd(matches!(mode, Mode::Vector(_)));
        c.set_time(1.25);
        for (p, v) in params {
            c.set_param(p, *v);
        }
        let (mut ii, mut oo) = (ins.iter(), outs.iter_mut());
        for (name, access, _) in &bufs {
            match access {
                Access::Read => c.input_u32(name, ii.next().unwrap()).unwrap(),
                _ => c.output_u32(name, oo.next().unwrap()).unwrap(),
            }
        }
        match mode {
            Mode::Interp => c.run_interp(n),
            Mode::Vector(t) if t > 1 => c.run_parallel(n, t),
            _ => c.run(n),
        }
        .unwrap_or_else(|e| panic!("run of {} elements: {:?}", n, e))
    };
    Out { words: outs, work: stats.work, overflowed: stats.overflowed }
}

fn compile_at(src: &str, lanes: u32) -> KernelRef {
    native_vec::with_lanes(lanes, || compile(src)).unwrap_or_else(|e| panic!("{:?}\n{}", e, src))
}

/// The kernel `src` at both widths, scalar and interpreted: the same bits everywhere. Returns
/// whether it has vector code.
fn same_at_both_widths(name: &str, src: &str, sizes: &[usize], params: &[(&str, f32)]) -> bool {
    let k4 = compile_at(src, 4);
    let k8 = compile_at(src, 8);
    assert_eq!(k4.simd(), k8.simd(), "{}: vector code at one width only", name);
    if k4.simd() {
        assert_eq!((k4.vector_lanes(), k8.vector_lanes()), (4, 8), "{}: lanes", name);
    }
    for &n in sizes {
        let want = run(&k4, n, params, Mode::Interp);
        assert!(want.words.iter().flatten().any(|w| *w != 0x5555_5555), "{}: the kernel writes its outputs (n {})", name, n);
        for (lanes, k) in [(4, &k4), (8, &k8)] {
            for mode in [Mode::Scalar, Mode::Vector(1), Mode::Vector(4)] {
                if mode == Mode::Vector(4) && n < 256 {
                    continue;
                }
                let got = run(k, n, params, mode);
                if got != want {
                    let w = got.words.iter().zip(&want.words).find_map(|(a, b)| a.iter().zip(b).position(|(x, y)| x != y));
                    panic!("{}: x{} {:?} differs from the interpreter at n {} (params {:?}): work {} vs {}, first differing word {:?}", name, lanes, mode, n, params, got.work, want.work, w);
                }
            }
        }
    }
    k4.simd()
}

const SIZES: &[usize] = &[1, 3, 4, 5, 7, 8, 9, 12, 15, 16, 17, 31, 33, 64, 1000, 4103];

#[test]
fn the_ai_corpus_is_the_same_at_both_widths() {
    let mut vec = 0;
    for (name, src) in ai_corpus::AI_CORPUS {
        vec += same_at_both_widths(name, src, SIZES, &[]) as usize;
    }
    eprintln!("{} AI-style kernels, {} with vector code, the same bits at 4 and 8 lanes", ai_corpus::AI_CORPUS.len(), vec);
    assert!(vec * 2 > ai_corpus::AI_CORPUS.len(), "most AI-style kernels vectorize ({} of {})", vec, ai_corpus::AI_CORPUS.len());
}

#[test]
fn the_geometry_kernels_are_the_same_at_both_widths() {
    // heightfields of whole rows (W = 1024 vertices) and partial ones; instances at any count
    let sizes = [1, 7, 8, 13, 1024, 3 * 1024 + 9, 64 * 1024];
    for (name, src) in geometry::GEOMETRY {
        let params: &[(&str, f32)] = if *name == "instances" { &[("t", 1.5)] } else { &[("amp", 6.0)] };
        assert!(same_at_both_widths(name, src, &sizes, params), "{} vectorizes", name);
    }
}

#[test]
fn the_fractal_kernels_are_the_same_at_both_widths() {
    // a 37 x 37 tile (1369 texels) and partial tiles; every family and power, iteration caps
    // that leave lanes at different iterations
    let sizes = [1, 8, 9, 37, 37 * 37];
    for kind in 0..4 {
        let params = [("res", 37.0), ("max_iter", 96.0), ("kind", kind as f32)];
        assert!(!same_at_both_widths("fractal f64", fractal::FRACTAL_F64, &sizes, &params), "f64 kernels run scalar");
        assert!(same_at_both_widths("fractal f32", fractal::FRACTAL_F32, &sizes, &params), "the f32 fractal vectorizes");
    }
    for power in [2, 3, 5, 8] {
        let params = [("res", 37.0), ("max_iter", 64.0), ("power", power as f32)];
        same_at_both_widths("multibrot f64", fractal::MULTIBROT_F64, &sizes, &params);
        assert!(same_at_both_widths("multibrot f32", fractal::MULTIBROT_F32, &sizes, &params), "the f32 multibrot vectorizes");
    }
}

#[test]
fn emit_and_divergent_loop_kernels_are_the_same_at_both_widths() {
    let srcs = [
        ("emit", "let seg = emit_buffer(3, 4)\nfn primitive(i) { for k in 0..(i % 5) { if hash01(i, k) > 0.3 { emit(seg, float(i), float(k), hash01(i, k)) } } }"),
        ("frame", "let o = output(vec4)\nlet amp = param(1.0)\nfn element(i) {\n let a = [0.0; 5]\n let s = 0.0\n let k = 0\n while k < i % 13 { a[k % 5] = a[k % 5] + float(k) * amp\n if hash01(i, k) > 0.8 { break }\n k = k + 1 }\n if i % 3 == 0 { s = sin(float(i)) } elif i % 3 == 1 { s = sqrt(float(i)) } else { s = -1.0 }\n o[i] = vec4(a[0] + a[4], s, float(k), hash01(i, 7)) }"),
        ("division", "let o = output(vec2)\nfn f(i) { if i % 5 == 2 { return float(i / 3) }\n let x = i * 7919\n return float((x >> 3) ^ (x << 5) % 11) }\nfn element(i) { o[i] = vec2(f(i), f(i + 1)) }"),
        ("records", "let src = input(f32)\nlet o = output(vec4)\nlet t = param(0.0)\nfn element(i) {\n let p = vec3(src[i * 8], src[i * 8 + 1], src[i * 8 + 2])\n let q = vec4(src[i * 8 + 3], src[i * 8 + 4], src[i * 8 + 5], src[i * 8 + 6])\n o[i] = vec4(p.x + q.w, p.y * q.z, p.z - q.y + src[i * 8 + 7], q.x + t) }"),
    ];
    for (name, src) in srcs {
        assert!(same_at_both_widths(name, src, SIZES, &[("t", 0.75)]), "{} vectorizes", name);
    }
}

/// Under the host rule an eight-lane host installs four-lane code and moves a kernel to eight
/// lanes once it is hot (in the background): before and after, the same bits.
#[test]
fn a_hot_kernel_moves_to_eight_lanes_with_the_same_bits() {
    if std::env::var_os("MAKEPAD_KERNEL_LANES").is_some() {
        return;
    }
    let (_, src) = geometry::GEOMETRY[1];
    let k = compile(src).unwrap();
    assert_eq!(k.vector_lanes(), 4, "installs four-lane code");
    let n = 1024 * 70;
    let first = run(&k, n, &[("amp", 6.0)], Mode::Vector(1));
    if native_vec::host_lanes() != 8 {
        assert_eq!(k.vector_lanes(), 4);
        return;
    }
    // the upgrade compiles on a thread of its own: wait for it (it takes milliseconds)
    let mut waited = 0;
    while k.vector_lanes() != 8 && waited < 2000 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        waited += 1;
    }
    assert_eq!(k.vector_lanes(), 8, "hot after {} elements", n);
    assert_eq!(run(&k, n, &[("amp", 6.0)], Mode::Vector(1)), first, "eight lanes after the upgrade: the same bits");
    assert_eq!(run(&k, n, &[("amp", 6.0)], Mode::Interp), first);
}
