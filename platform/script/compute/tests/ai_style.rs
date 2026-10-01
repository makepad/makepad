//! The compiler accepts kernels written the way a language model writes
//! them (tests/support/ai_corpus.rs) and computes what they say: every
//! corpus kernel compiles, gives the hand-computed values, and runs
//! bit-equal natively (four wide where it vectorizes, and scalar) and on
//! the interpreter. Where a construct cannot be compiled the error names
//! the construct and the reason, at the read.

#[path = "support/ai_corpus.rs"]
mod ai_corpus;

use ai_corpus::AI_CORPUS;
use makepad_script_compute::kernel::{compile_with, Kernel};
use makepad_script_compute::Backend;

/// The values element `i` must write (f32 kernels: one word).
fn expected(name: &str, i: usize) -> Vec<f32> {
    let x = i as f32;
    let fi = i as i32;
    match name {
        "helpers_call_helpers" => vec![(1.0 - (x * x + 16.0).sqrt() / 10.0).clamp(0.0, 1.0)],
        "nested_fn" => {
            let t = x / 4.0;
            vec![t * t * (3.0 - 2.0 * t)]
        }
        "closure_captures" => vec![3.0 * 2.0 + x],
        "closures_call_closures" => vec![1.0 + 2.0 * x],
        "closure_mutates_captured" => vec![6.0 + x],
        "top_level_fn_value" => vec![x * x * x * x],
        "param_named_like_top_level" => vec![2.0 * x + 3.0],
        "params_named_like_prelude" => vec![0.5 * x + 3.0],
        "local_named_like_builtin" => vec![x * 2.0 + 5.0],
        "palette" => {
            let bg = [26.0 / 255.0, 26.0 / 255.0, 46.0 / 255.0, 1.0f32];
            let t = x / 3.0;
            let c: Vec<f32> = bg.iter().map(|b| b + (1.0 - b) * t).collect();
            vec![c[0], c[1] + 1.0, 128.0 / 255.0, c[3]]
        }
        "const_vectors" => vec![2.0, 4.0 + x, 6.0, 2.0],
        "object_let" => vec![40.0 * x, 1.0, 100.0 - 0.1, 5.0],
        "object_count_field" => vec![2.0 * x],
        "object_with_strings" => vec![2.5 * x],
        "objects_array_runtime_index" => {
            let k = i % 3;
            vec![[1.0, -1.0, 0.0][k], [0.0, 1.0, 0.0][k], [2.0, 0.5, 1.0][k], 4.0]
        }
        "objects_array_loop" => vec![2.0 + x],
        "object_passed_to_helper" => vec![x - 2.0],
        "object_of_arrays" => vec![[0.0, 0.25, 1.0][i % 3] + [1.0, 6.0, 11.0][i % 3] + 10.0],
        "local_object" => vec![2.0 * (x + 1.0)],
        "struct_methods_style" => vec![x + 2.0],
        "early_returns" => vec![if x < 1.0 { 0.0 } else if x < 3.0 { 1.0 } else { 2.0 }],
        "loops_with_break" => {
            let (mut n, mut v) = (0, x + 1.0);
            while v <= 20.0 {
                v *= 2.0;
                n += 1;
            }
            let m = fi.max(1);
            let first = (0..10).find(|k| k * k > fi).unwrap_or(-1);
            vec![n as f32 * 100.0 + m as f32 * 10.0 + first as f32]
        }
        "if_as_value" => {
            let y = if x > 1.5 { x * 10.0 } else { -x };
            let z = if i % 2 == 0 { 1.0 } else if i == 1 { 2.0 } else { 3.0 };
            let w = if i > 0 && i < 3 { 100.0 } else { 0.0 };
            vec![y + z + w + [1000.0, 2000.0, 3000.0][i % 3]]
        }
        "shadowing" => vec![3.0 * x + 1.0],
        "mixed_int_float" => vec![(fi / 2) as f32 + 0.5 * x + (2.0 * x + 1.0) + 1.5 * (x + 1.0) + (i % 8) as f32 / 8.0 + fi.min(2) as f32],
        "int_times_fraction" => vec![0.75 * x],
        "swizzles" => vec![4.0, x, 9.0, 4.0],
        "const_keyword_and_annotations" => vec![2.0 * x + 0.5],
        other => panic!("no expected values for `{}`", other),
    }
}

fn run(k: &Kernel, n: usize, words: usize, simd: bool) -> Vec<f32> {
    let mut o = vec![0.0f32; n * words];
    let mut c = k.call();
    c.set_simd(simd);
    c.output("o", &mut o).unwrap();
    c.run(n).unwrap();
    o
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn ai_style_kernels_compile_and_compute_what_they_say() {
    let mut failed = Vec::new();
    for (name, src) in AI_CORPUS {
        let native = match compile_with(src, &[], Backend::Native) {
            Ok(k) => k,
            Err(e) => {
                failed.push(format!("{}: {}", name, e.iter().map(|e| format!("{} at {:?}", e.message, e.line_col(src))).collect::<Vec<_>>().join("; ")));
                continue;
            }
        };
        let interp = compile_with(src, &[], Backend::Interp).unwrap();
        let words = expected(name, 0).len();
        let n = 9;
        let got = run(&interp, n, words, false);
        for i in 0..n {
            let want = expected(name, i);
            let have = &got[i * words..(i + 1) * words];
            let close = want.iter().zip(have).all(|(w, h)| (w - h).abs() <= 1e-5 * w.abs().max(1.0));
            if !close {
                failed.push(format!("{}: element {}: expected {:?}, got {:?}", name, i, want, have));
                break;
            }
        }
        // Every backend gives the interpreter's bits.
        for n in [n, 4096 + 7] {
            let reference = bits(&run(&interp, n, words, false));
            assert_eq!(bits(&run(&native, n, words, false)), reference, "{}: native scalar != interpreter (n {})", name, n);
            assert_eq!(bits(&run(&native, n, words, true)), reference, "{}: native four-wide != interpreter (n {})", name, n);
        }
    }
    assert!(failed.is_empty(), "{} of {} AI-style kernels fail:\n{}", failed.len(), AI_CORPUS.len(), failed.join("\n"));
}

fn compile_err(src: &str) -> String {
    match compile_with(src, &[], Backend::Interp) {
        Ok(_) => panic!("expected an error from:\n{}", src),
        Err(e) => e.iter().map(|e| format!("{} at {:?}", e.message, e.line_col(src))).collect::<Vec<_>>().join("; "),
    }
}

#[test]
fn constant_object_reads_fold_to_constants() {
    // A constant path into a plain object (a field, a constant index, an
    // unrolled loop's index) is the constant: no table load is left.
    for (name, src) in AI_CORPUS {
        if matches!(*name, "object_let" | "object_count_field" | "objects_array_loop" | "object_with_strings") {
            let k = compile_with(src, &[], Backend::Interp).unwrap();
            let air = format!("{:?}", k.program());
            assert!(!air.contains("Shared"), "{}: a table load is left:\n{}", name, air);
        }
    }
    // A runtime index reads the table.
    let src = AI_CORPUS.iter().find(|(n, _)| *n == "objects_array_runtime_index").unwrap().1;
    let k = compile_with(src, &[], Backend::Interp).unwrap();
    assert!(format!("{:?}", k.program()).contains("Shared"));
}

#[test]
fn what_a_kernel_cannot_take_is_an_error_at_the_read_naming_why() {
    // Strings in objects: the other fields read; the string is the error.
    let e = compile_err("let o = output(f32)\nlet STYLE = {name: \"hero\", width: 2.5}\nfn element(i) { o[i] = STYLE.name }");
    assert!(e.contains("field `name` is a string") && e.contains("(3, "), "{}", e);
    // A field computed from a param: only reading that field fails.
    let src = "let o = output(f32)\nlet amp = param(1.0)\nlet CFG = {speed: amp * 2.0, size: 3.0}\nfn element(i) { o[i] = CFG.size }";
    compile_with(src, &[], Backend::Interp).unwrap_or_else(|e| panic!("{:?}", e));
    let e = compile_err("let o = output(f32)\nlet amp = param(1.0)\nlet CFG = {speed: amp * 2.0, size: 3.0}\nfn element(i) { o[i] = CFG.speed }");
    assert!(e.contains("field `speed` is not a constant") && e.contains("read params"), "{}", e);
    // Fields that are not kernel code at all: the others read.
    let src = "let o = output(f32)\nlet K = {a: nil, b: [1, \"x\"], c: 2.0}\nfn element(i) { o[i] = K.c }";
    compile_with(src, &[], Backend::Interp).unwrap_or_else(|e| panic!("{:?}", e));
    let e = compile_err("let o = output(f32)\nlet K = {a: nil, b: [1, \"x\"], c: 2.0}\nfn element(i) { o[i] = K.b[0] }");
    assert!(e.contains("field `b` is a value kernel code cannot hold"), "{}", e);
    // A function kept in an object.
    let e = compile_err("let o = output(f32)\nlet K = {f: fn(x) { x * 2.0 }, s: 1.0}\nfn element(i) { o[i] = K.f(1.0) + K.s }");
    assert!(e.contains("function"), "{}", e);
    // An object made from a prototype (its inherited fields are unknown).
    let e = compile_err("let o = output(f32)\nlet C = {cam: Camera3D{fov: 40}, k: 1.0}\nfn element(i) { o[i] = C.cam.fov }");
    assert!(e.contains("made from `Camera3D`"), "{}", e);
    // Objects of one array with different fields.
    let e = compile_err("let o = output(f32)\nlet L = [{a: 1.0, b: 2.0}, {a: 1.0, c: 2.0}]\nfn element(i) { o[i] = L[i % 2].a }");
    assert!(e.contains("same fields"), "{}", e);
    // Tables are constants.
    let e = compile_err("let o = output(f32)\nlet C = {a: 1.0}\nfn element(i) { C.a = 2.0\n o[i] = C.a }");
    assert!(e.contains("read-only"), "{}", e);
    // Function values are called, never stored or passed.
    let e = compile_err("let o = output(f32)\nfn apply(g, x) { g(x) }\nfn element(i) { let f = fn(x) { x * 2.0 }\n o[i] = apply(f, 1.0) }");
    assert!(e.contains("`f` is a function: call it"), "{}", e);
    // A local function has no recursion.
    let e = compile_err("let o = output(f32)\nfn element(i) { let f = fn(x) { if x > 0.0 { f(x - 1.0) } else { 0.0 } }\n o[i] = f(3.0) }");
    assert!(e.contains("`f` calls itself"), "{}", e);
}

// -- a source-level generator of the same constructs --------------------------
//
// The AIR fuzzers (tests/support/kernel_gen.rs) start below the source:
// they never meet objects, local functions or palette constants. This one
// writes kernels from those constructs with random constants and shapes,
// and evaluates the same expression trees in Rust (f64, with a magnitude
// bound for the tolerance: kernels fold literals in f64 and fuse
// multiply-adds).

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn num(&mut self) -> f64 {
        (self.below(64) as f64 - 32.0) / 8.0
    }
}

/// The program's constants: objects, an array of objects, colours, vectors.
struct Consts {
    /// `O{k} = {a, v: vec3, inner: {b}}`
    objs: Vec<(f64, [f64; 3], f64)>,
    /// `T = [{a, v: vec3}, ..]` (written with fields in either order)
    table: Vec<(f64, [f64; 3])>,
    /// `C{k} = #rrggbb` channels / 255
    colours: Vec<[f64; 3]>,
    /// `V{k} = vec3(..)`
    vecs: Vec<[f64; 3]>,
}

/// An expression's source and its value as a function of x (the element's
/// float(i) * 0.25) and the closure's argument y, with a magnitude bound.
#[derive(Clone)]
enum G {
    X,
    Y,
    Num(f64),
    Leaf(String, f64),
    /// `T[i % n].a` / `T[i % n].v.y`
    Row(bool),
    Add(Box<G>, Box<G>),
    Sub(Box<G>, Box<G>),
    Mul(Box<G>, f64),
    Min(Box<G>, Box<G>),
    If(Box<G>, Box<G>, Box<G>, Box<G>),
    Helper(usize, Box<G>, Box<G>),
    Closure(Box<G>),
}

struct Prog {
    c: Consts,
    /// Helpers `h{k}(x, y)`: body, early-return threshold on x.
    helpers: Vec<(G, f64, G)>,
    closure: G,
    out: G,
}

fn gen_expr(r: &mut Rng, depth: u32, c: &Consts, helpers: usize, closure: bool, y: bool) -> G {
    if depth == 0 || r.below(4) == 0 {
        return match r.below(if y { 9 } else { 8 }) {
            0 => G::X,
            1 => G::Num(r.num()),
            2 => {
                let k = r.below(c.objs.len() as u64) as usize;
                match r.below(3) {
                    0 => G::Leaf(format!("O{}.a", k), c.objs[k].0),
                    1 => G::Leaf(format!("O{}.inner.b", k), c.objs[k].2),
                    _ => {
                        let l = r.below(3) as usize;
                        G::Leaf(format!("O{}.v.{}", k, ["x", "y", "z"][l]), c.objs[k].1[l])
                    }
                }
            }
            3 => {
                let k = r.below(c.table.len() as u64) as usize;
                if r.below(2) == 0 {
                    G::Leaf(format!("T[{}].a", k), c.table[k].0)
                } else {
                    G::Leaf(format!("T[{}].v.z", k), c.table[k].1[2])
                }
            }
            4 => G::Row(r.below(2) == 0),
            5 => {
                let k = r.below(c.colours.len() as u64) as usize;
                let l = r.below(3) as usize;
                G::Leaf(format!("C{}.{}", k, ["r", "g", "b"][l]), c.colours[k][l])
            }
            6 => {
                let k = r.below(c.vecs.len() as u64) as usize;
                let l = r.below(3) as usize;
                G::Leaf(format!("V{}.{}", k, ["x", "y", "z"][l]), c.vecs[k][l])
            }
            7 => G::X,
            _ => G::Y,
        };
    }
    let sub = |r: &mut Rng| Box::new(gen_expr(r, depth - 1, c, helpers, closure, y));
    match r.below(8) {
        0 => G::Add(sub(r), sub(r)),
        1 => G::Sub(sub(r), sub(r)),
        2 => G::Mul(sub(r), r.num()),
        3 => G::Min(sub(r), sub(r)),
        4 => G::If(sub(r), sub(r), sub(r), sub(r)),
        5 if helpers > 0 => G::Helper(r.below(helpers as u64) as usize, sub(r), sub(r)),
        6 if closure => G::Closure(sub(r)),
        _ => G::Add(sub(r), sub(r)),
    }
}

fn src_of(g: &G, xn: &str, yn: &str) -> String {
    let s = |g: &G| src_of(g, xn, yn);
    match g {
        G::X => xn.into(),
        G::Y => yn.into(),
        G::Num(v) => format!("{:?}", v),
        G::Leaf(s, _) => s.clone(),
        G::Row(a) => if *a { "T[i % NT].a".into() } else { "T[i % NT].v.y".into() },
        G::Add(a, b) => format!("({} + {})", s(a), s(b)),
        G::Sub(a, b) => format!("({} - {})", s(a), s(b)),
        G::Mul(a, k) => format!("({} * {:?})", s(a), k),
        G::Min(a, b) => format!("min({}, {})", s(a), s(b)),
        G::If(a, b, t, e) => format!("(if {} > {} {{ {} }} else {{ {} }})", s(a), s(b), s(t), s(e)),
        G::Helper(k, a, b) => format!("h{}({}, {})", k, s(a), s(b)),
        G::Closure(a) => format!("f({})", s(a)),
    }
}

/// (value, magnitude bound); None where a comparison is too close to call
/// (the kernel's rounding may take the other branch).
fn eval(g: &G, p: &Prog, x: f64, y: f64, i: usize) -> Option<(f64, f64)> {
    let e = |g: &G| eval(g, p, x, y, i);
    Some(match g {
        G::X => (x, x.abs()),
        G::Y => (y, y.abs()),
        G::Num(v) | G::Leaf(_, v) => (*v, v.abs()),
        G::Row(a) => {
            let row = &p.c.table[i % p.c.table.len()];
            let v = if *a { row.0 } else { row.1[1] };
            (v, v.abs())
        }
        G::Add(a, b) => {
            let (a, b) = (e(a)?, e(b)?);
            (a.0 + b.0, a.1 + b.1)
        }
        G::Sub(a, b) => {
            let (a, b) = (e(a)?, e(b)?);
            (a.0 - b.0, a.1 + b.1)
        }
        G::Mul(a, k) => {
            let a = e(a)?;
            (a.0 * k, a.1 * k.abs())
        }
        G::Min(a, b) => {
            let (a, b) = (e(a)?, e(b)?);
            (a.0.min(b.0), a.1.max(b.1))
        }
        G::If(a, b, t, f) => {
            let (a, b) = (e(a)?, e(b)?);
            if (a.0 - b.0).abs() <= 1e-3 * (1.0 + a.1 + b.1) {
                return None;
            }
            if a.0 > b.0 { e(t)? } else { e(f)? }
        }
        G::Helper(k, a, b) => {
            let (a, b) = (e(a)?, e(b)?);
            let (body, at, early) = &p.helpers[*k];
            if (a.0 - at).abs() <= 1e-3 * (1.0 + a.1) {
                return None;
            }
            let (v, m) = if a.0 > *at { eval(early, p, a.0, b.0, i)? } else { eval(body, p, a.0, b.0, i)? };
            (v, m + a.1 + b.1)
        }
        G::Closure(a) => {
            let a = e(a)?;
            let (v, m) = eval(&p.closure, p, x, a.0, i)?;
            (v, m + a.1)
        }
    })
}

fn gen_program(r: &mut Rng) -> (Prog, String) {
    let n = |r: &mut Rng| r.num();
    let v3 = |r: &mut Rng| [r.num(), r.num(), r.num()];
    let c = Consts {
        objs: (0..1 + r.below(3)).map(|_| (n(r), v3(r), n(r))).collect(),
        table: (0..2 + r.below(3)).map(|_| (n(r), v3(r))).collect(),
        colours: (0..1 + r.below(2)).map(|_| [r.below(256) as f64 / 255.0, r.below(256) as f64 / 255.0, r.below(256) as f64 / 255.0]).collect(),
        vecs: (0..1 + r.below(2)).map(|_| v3(r)).collect(),
    };
    let mut src = String::from("let o = output(f32)\n");
    let f = |v: f64| format!("{:?}", v);
    for (k, (a, v, b)) in c.objs.iter().enumerate() {
        src += &format!("let O{} = {{a: {}, v: vec3({}, {}, {}), inner: {{b: {}}}}}\n", k, f(*a), f(v[0]), f(v[1]), f(v[2]), f(*b));
    }
    src += &format!("let NT = {}\nlet T = [\n", c.table.len());
    for (k, (a, v)) in c.table.iter().enumerate() {
        if k % 2 == 0 {
            src += &format!("    {{a: {}, v: vec3({}, {}, {})}}\n", f(*a), f(v[0]), f(v[1]), f(v[2]));
        } else {
            src += &format!("    {{v: vec3({}, {}, {}), a: {}}}\n", f(v[0]), f(v[1]), f(v[2]), f(*a));
        }
    }
    src += "]\n";
    for (k, col) in c.colours.iter().enumerate() {
        src += &format!("let C{} = #{:02x}{:02x}{:02x}\n", k, (col[0] * 255.0).round() as u32, (col[1] * 255.0).round() as u32, (col[2] * 255.0).round() as u32);
    }
    for (k, v) in c.vecs.iter().enumerate() {
        src += &format!("let V{} = vec3({}, {}, {})\n", k, f(v[0]), f(v[1]), f(v[2]));
    }
    // Helpers (each may call the earlier ones), with an early return; the
    // parameter `x` shadows nothing, `y` is named like the closure's.
    let mut helpers = Vec::new();
    for k in 0..r.below(3) as usize {
        let body = gen_expr(r, 2, &c, k, false, true);
        let early = gen_expr(r, 1, &c, k, false, true);
        let at = n(r);
        // Helpers see only their parameters and constants (no `i`).
        let body = strip_rows(body);
        let early = strip_rows(early);
        src += &format!("let h{} = fn(x, y) {{\n    if x > {} {{ return {} }}\n    {}\n}}\n", k, f(at), src_of(&early, "x", "y"), src_of(&body, "x", "y"));
        helpers.push((body, at, early));
    }
    let closure = gen_expr(r, 2, &c, helpers.len(), false, true);
    let out = gen_expr(r, 4, &c, helpers.len(), true, false);
    src += &format!(
        "fn element(i) {{\n    let x = float(i) * 0.25\n    let f = fn(y) {{ {} }}\n    o[i] = {}\n}}\n",
        src_of(&closure, "x", "y"),
        src_of(&out, "x", "y")
    );
    (Prog { c, helpers, closure, out }, src)
}

fn strip_rows(g: G) -> G {
    let b = |g: Box<G>| Box::new(strip_rows(*g));
    match g {
        G::Row(_) => G::Num(1.5),
        G::Add(a, c) => G::Add(b(a), b(c)),
        G::Sub(a, c) => G::Sub(b(a), b(c)),
        G::Mul(a, k) => G::Mul(b(a), k),
        G::Min(a, c) => G::Min(b(a), b(c)),
        G::If(a, c, t, e) => G::If(b(a), b(c), b(t), b(e)),
        G::Helper(k, a, c) => G::Helper(k, b(a), b(c)),
        G::Closure(a) => G::Closure(b(a)),
        g => g,
    }
}

#[test]
fn generated_ai_style_kernels_compile_and_compute_what_they_say() {
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut r = Rng(env("AI_STYLE_SEED", 0x9E37_79B9_7F4A_7C15) | 1);
    let rounds = env("AI_STYLE_ROUNDS", 300);
    let (mut checked, mut skipped) = (0, 0);
    for round in 0..rounds {
        let (p, src) = gen_program(&mut r);
        let native = compile_with(&src, &[], Backend::Native).unwrap_or_else(|e| panic!("round {}: {:?}\n{}", round, e, src));
        let interp = compile_with(&src, &[], Backend::Interp).unwrap();
        let n = 11;
        let got = run(&interp, n, 1, false);
        assert_eq!(bits(&run(&native, n, 1, false)), bits(&got), "round {}: native != interpreter\n{}", round, src);
        assert_eq!(bits(&run(&native, n, 1, true)), bits(&got), "round {}: four-wide != interpreter\n{}", round, src);
        for (i, have) in got.iter().enumerate() {
            let x = i as f64 * 0.25;
            match eval(&p.out, &p, x, 0.0, i) {
                None => skipped += 1,
                Some((want, mag)) => {
                    assert!((*have as f64 - want).abs() <= 1e-4 * (1.0 + mag), "round {}: element {}: expected {}, got {}\n{}", round, i, want, have, src);
                    checked += 1;
                }
            }
        }
    }
    eprintln!("{} generated kernels: {} values checked, {} too close to a branch to call", rounds, checked, skipped);
}
