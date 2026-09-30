//! Splash modules in kernels: the stdlib and user or AI-written libraries
//! resolve, inline and validate the same way; plus the kernel stdlib
//! modules checked against Rust references.

use makepad_script_compute::kernel::{compile, compile_with_modules, Kernel};
use makepad_script_compute::module::{Module, STD};
use makepad_script_compute::sched::{InlineExecutor, Job};
use makepad_script_compute::Backend;
use std::sync::Arc;

fn run(k: &Arc<Kernel>, n: usize, inputs: &[(&str, Vec<f32>)], out: &str, words: usize) -> Vec<f32> {
    let mut j = Job::new(k.clone(), n);
    for (name, data) in inputs {
        j.input(name, data.clone().into()).unwrap();
    }
    j.output(out, vec![0.0; n * words]).unwrap();
    j.run(&InlineExecutor, 1).unwrap();
    j.out(out).unwrap().to_vec()
}

fn std_src(path: &str) -> &'static str {
    STD.iter().find(|(p, _)| *p == path).unwrap().1
}

#[test]
fn a_library_module_compiles_to_the_same_code_as_the_stdlib() {
    // The same text, once as std and once as an AI-written library module.
    let via_std = "use std.poly.*\nlet pts = input(vec2)\nlet o = output(f32)\nfn element(i) { o[i] = area(pts, 4) + float(winding(pts, 4)) }";
    let via_lib = "use lib(\"ai/shapes\", \"7\") as shapes\nlet pts = input(vec2)\nlet o = output(f32)\nfn element(i) { o[i] = shapes.area(pts, 4) + float(shapes.winding(pts, 4)) }";
    let a = compile(via_std).unwrap_or_else(|e| panic!("{:?}", e));
    let lib = [Module { path: "lib:ai/shapes@7", source: std_src("std.poly") }];
    let b = compile_with_modules(via_lib, &[], Backend::Native, &lib).unwrap_or_else(|e| panic!("{:?}", e));
    assert_eq!(format!("{:?}", a.program()), format!("{:?}", b.program()), "the same AIR");
    // And a document's own fn: the same again.
    let own = format!("{}\nlet pts = input(vec2)\nlet o = output(f32)\nfn element(i) {{ o[i] = area(pts, 4) + float(winding(pts, 4)) }}", std_src("std.poly"));
    let c = compile(&own).unwrap_or_else(|e| panic!("{:?}", e));
    assert_eq!(format!("{:?}", a.program()), format!("{:?}", c.program()));
    let pts = vec![0.0, 0.0, 2.0, 0.0, 2.0, 3.0, 0.0, 3.0];
    assert_eq!(run(&b, 1, &[("pts", pts)], "o", 1), vec![7.0]);
}

#[test]
fn resolution_rules() {
    // Qualified std calls need no `use`.
    let k = compile("let pts = input(vec2)\nlet o = output(f32)\nfn element(i) { o[i] = poly.perimeter(pts, 4) }").unwrap_or_else(|e| panic!("{:?}", e));
    assert_eq!(run(&k, 1, &[("pts", vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0])], "o", 1), vec![4.0]);
    // The shared stdlib is in scope unqualified by default...
    let k = compile("let o = output(f32)\nfn element(i) { o[i] = knob(0.5) }").unwrap();
    assert_eq!(run(&k, 1, &[], "o", 1), vec![1.0]);
    // ...and a kernel's own item shadows it; kernel modules are qualified
    // unless imported.
    let k = compile("let o = output(f32)\nfn knob(a) { 42.0 }\nfn element(i) { o[i] = knob(0.0) }").unwrap();
    assert_eq!(run(&k, 1, &[], "o", 1), vec![42.0]);
    assert!(compile("let pts = input(vec2)\nlet o = output(f32)\nfn element(i) { o[i] = perimeter(pts, 4) }").is_err());
    let k = compile("use std.poly.*\nlet pts = input(vec2)\nlet o = output(f32)\nfn element(i) { o[i] = perimeter(pts, 4) }").unwrap();
    assert_eq!(run(&k, 1, &[("pts", vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0])], "o", 1), vec![4.0]);
    // Two globs bringing one name: an error at the second `use`.
    let m = [Module { path: "lib:a@1", source: "fn f(x) { x }" }, Module { path: "lib:b@1", source: "fn f(x) { x * 2.0 }" }];
    let src = "use lib(\"a\", \"1\").*\nuse lib(\"b\", \"1\").*\nlet o = output(f32)\nfn element(i) { o[i] = f(1.0) }";
    let e = compile_with_modules(src, &[], Backend::Native, &m).map(|_| ()).unwrap_err();
    assert!(e[0].message.contains("brought already"), "{}", e[0].message);
    // Modules may use each other, and their locals shadow their own names.
    let m = [
        Module { path: "lib:base@1", source: "let K = 3.0\nfn twice(x) { x * 2.0 }" },
        Module { path: "lib:top@1", source: "use lib(\"base\", \"1\") as base\nfn f(x) { let twice = 10.0\n base.twice(x) + twice + base.K }" },
    ];
    let src = "use lib(\"top\", \"1\") as top\nlet o = output(f32)\nfn element(i) { o[i] = top.f(1.0) }";
    let k = compile_with_modules(src, &[], Backend::Native, &m).unwrap_or_else(|e| panic!("{:?}", e));
    assert_eq!(run(&k, 1, &[], "o", 1), vec![15.0]);
    // A module's constant, qualified from the kernel.
    let src = "use lib(\"base\", \"1\") as base\nlet o = output(f32)\nfn element(i) { o[i] = base.K }";
    let k = compile_with_modules(src, &[], Backend::Native, &m).unwrap_or_else(|e| panic!("{:?}", e));
    assert_eq!(run(&k, 1, &[], "o", 1), vec![3.0]);
    // Missing modules, cycles, errors inside modules: named, at the use or call.
    let e = compile("use std.nope.*\nlet o = output(f32)\nfn element(i) { o[i] = 1.0 }").map(|_| ()).unwrap_err();
    assert!(e[0].message.contains("no module `std.nope`"), "{}", e[0].message);
    let m = [Module { path: "lib:a@1", source: "use lib(\"b\", \"1\") as b\nfn f(x) { b.g(x) }" }, Module { path: "lib:b@1", source: "use lib(\"a\", \"1\") as a\nfn g(x) { a.f(x) }" }];
    let e = compile_with_modules("use lib(\"a\", \"1\") as a\nlet o = output(f32)\nfn element(i) { o[i] = a.f(1.0) }", &[], Backend::Native, &m).map(|_| ()).unwrap_err();
    assert!(e[0].message.contains("imports itself"), "{}", e[0].message);
    let m = [Module { path: "lib:bad@1", source: "fn f(x) { x + vec3(1.0, 2.0) }" }];
    let src = "use lib(\"bad\", \"1\") as bad\nlet o = output(f32)\nfn element(i) { o[i] = bad.f(1.0) }";
    let e = compile_with_modules(src, &[], Backend::Native, &m).map(|_| ()).unwrap_err();
    let at = src.find("bad.f").unwrap();
    assert!(e[0].start >= at && e[0].start < at + 10, "reported at {} ({}), want the call at {}", e[0].start, e[0].message, at);
    let m = [Module { path: "lib:syntax@1", source: "fn f(x) { x + }" }];
    let e = compile_with_modules("use lib(\"syntax\", \"1\") as s\nlet o = output(f32)\nfn element(i) { o[i] = s.f(1.0) }", &[], Backend::Native, &m).map(|_| ()).unwrap_err();
    assert!(e[0].message.contains("in module `lib:syntax@1` (line 1"), "{}", e[0].message);
}

#[test]
fn unused_modules_cost_nothing() {
    let a = compile("let o = output(f32)\nfn element(i) { o[i] = float(i) }").unwrap();
    let b = compile("use std.field.*\nuse std.curve as c\nlet o = output(f32)\nfn element(i) { o[i] = float(i) }").unwrap();
    assert_eq!(format!("{:?}", a.program()), format!("{:?}", b.program()));
    assert_eq!(a.shared_words(), b.shared_words());
}

// -- the stdlib against Rust references ------------------------------------------

/// Marching squares in Rust (the same case table and saddle rule).
fn isolines_ref(g: &[f32], w: usize, h: usize, level: f32) -> Vec<[f32; 4]> {
    let corner = |cx: usize, cz: usize, k: usize| {
        let x = (cx + (k == 1 || k == 2) as usize).min(w - 1);
        let z = (cz + (k >= 2) as usize).min(h - 1);
        g[z * w + x]
    };
    let table: [[i32; 4]; 16] = [
        [-1, -1, -1, -1], [3, 0, -1, -1], [0, 1, -1, -1], [3, 1, -1, -1], [1, 2, -1, -1], [3, 0, 1, 2], [0, 2, -1, -1], [3, 2, -1, -1],
        [2, 3, -1, -1], [0, 2, -1, -1], [0, 1, 2, 3], [1, 2, -1, -1], [1, 3, -1, -1], [0, 1, -1, -1], [3, 0, -1, -1], [-1, -1, -1, -1],
    ];
    let mut out = Vec::new();
    for cz in 0..h - 1 {
        for cx in 0..w - 1 {
            let c: Vec<f32> = (0..4).map(|k| corner(cx, cz, k)).collect();
            let mut m = 0;
            for k in 0..4 {
                if c[k] >= level {
                    m |= 1 << k;
                }
            }
            let centre = (c[0] + c[1] + c[2] + c[3]) * 0.25;
            if m == 5 && centre >= level {
                m = 10;
            } else if m == 10 && centre >= level {
                m = 5;
            }
            let pt = |e: usize| {
                let p = c[e];
                let q = c[(e + 1) % 4];
                let t = if q != p { ((level - p) / (q - p)).clamp(0.0, 1.0) } else { 0.5 };
                let x0 = cx as f32 + if e == 1 || e == 2 { 1.0 } else { 0.0 };
                let z0 = cz as f32 + if e >= 2 { 1.0 } else { 0.0 };
                let x1 = cx as f32 + if e == 0 || e == 1 { 1.0 } else { 0.0 };
                let z1 = cz as f32 + if e == 1 || e == 2 { 1.0 } else { 0.0 };
                [x0 + (x1 - x0) * t, z0 + (z1 - z0) * t]
            };
            for s in 0..2 {
                let (e0, e1) = (table[m][2 * s], table[m][2 * s + 1]);
                if e0 >= 0 {
                    let (p, q) = (pt(e0 as usize), pt(e1 as usize));
                    out.push([p[0], p[1], q[0], q[1]]);
                }
            }
        }
    }
    out
}

#[test]
fn field_isolines_match_a_rust_marching_squares() {
    let (w, h) = (37usize, 23usize);
    let g: Vec<f32> = (0..w * h).map(|i| ((i % w) as f32 * 0.37).sin() * 3.0 + ((i / w) as f32 * 0.21).cos() * 2.0).collect();
    let src = "let W = 37\nlet H = 23\nlet g = input(f32)\nlet level = param(0.5)\nlet seg = emit_buffer(4, 2)\nfn primitive(i) {\n let cx = i % (W - 1)\n let cz = i / (W - 1)\n for k in 0..field.segments(g, W, H, level, cx, cz) { emit(seg, field.segment(g, W, H, level, cx, cz, k)) } }";
    let k = compile(src).unwrap_or_else(|e| panic!("{:?}", e));
    for level in [0.5f32, -1.25, 2.0] {
        let cells = (w - 1) * (h - 1);
        let mut j = Job::new(k.clone(), cells);
        j.set_param("level", level);
        j.input("g", g.clone().into()).unwrap();
        j.output("seg", vec![0.0; cells * 8]).unwrap();
        j.output_u32("seg_count", vec![0; cells]).unwrap();
        j.run(&InlineExecutor, 1).unwrap();
        let flat = makepad_script_compute::kernel::compact(j.out("seg").unwrap(), j.out_u32("seg_count").unwrap(), 4, 2);
        let want: Vec<f32> = isolines_ref(&g, w, h, level).into_iter().flatten().collect();
        assert!(!want.is_empty());
        assert_eq!(flat.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), want.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), "level {}", level);
    }
}

#[test]
fn curves_polygons_ease_anim_beats_and_camera() {
    // Curve: arc-length sampling with a cumulative table.
    let pts = vec![0.0f32, 0.0, 3.0, 0.0, 3.0, 4.0, 0.0, 4.0];
    let cum = vec![0.0f32, 3.0, 7.0, 10.0];
    let k = compile("let pts = input(vec2)\nlet cum = input(f32)\nlet o = output(vec2)\nfn element(i) { o[i] = curve.sample(pts, cum, 4, float(i) / 10.0) }").unwrap();
    let o = run(&k, 11, &[("pts", pts.clone()), ("cum", cum)], "o", 2);
    assert_eq!(&o[0..2], &[0.0, 0.0]);
    assert_eq!(&o[6..8], &[3.0, 0.0]);
    assert_eq!(&o[10..12], &[3.0, 2.0]);
    assert_eq!(&o[20..22], &[0.0, 4.0]);
    let k = compile("let pts = input(vec2)\nlet o = output(f32)\nfn element(i) { o[i] = curve.cum_at(pts, i) }").unwrap();
    assert_eq!(run(&k, 4, &[("pts", pts.clone())], "o", 1), vec![0.0, 3.0, 7.0, 10.0]);
    // Polygon: area, containment.
    let k = compile("let pts = input(vec2)\nlet q = input(vec2)\nlet o = output(f32)\nfn element(i) { o[i] = if poly.contains(pts, 4, q[i]) { 1.0 } else { 0.0 } }").unwrap();
    let q = vec![1.0, 1.0, 5.0, 1.0, 2.9, 3.9, -0.1, 2.0];
    assert_eq!(run(&k, 4, &[("pts", pts), ("q", q)], "o", 1), vec![1.0, 0.0, 1.0, 0.0]);
    // Ease: Motion's polynomial curves bit for bit (f64, rounded once).
    let motion_in_out_cubic = |u: f64| if u < 0.5 { 0.5 * (2.0 * u).powi(3) } else { 1.0 - 0.5 * (2.0 - 2.0 * u) * (2.0 - 2.0 * u) * (2.0 - 2.0 * u) };
    let motion_out_back = |u: f64| {
        let v = 1.0 - u;
        let (c1, c3) = (1.70158f64, 2.70158f64);
        1.0 - (c3 * v * v * v - c1 * v * v)
    };
    let k = compile("let u = input(f32)\nlet o = output(vec2)\nfn element(i) { o[i] = vec2(ease.in_out_cubic(u[i]), ease.out_back(u[i])) }").unwrap();
    let us: Vec<f32> = (0..=64).map(|k| k as f32 / 64.0).chain([-0.5, 1.5, 0.3333]).collect();
    let o = run(&k, us.len(), &[("u", us.clone())], "o", 2);
    for (k, u) in us.iter().enumerate() {
        let d = *u as f64;
        let (a, b) = if !(d > 0.0) { (0.0, 0.0) } else if d >= 1.0 { (1.0, 1.0) } else { (motion_in_out_cubic(d), motion_out_back(d)) };
        // Motion's in_out_cubic: 0.5 * u^3 as u*u*u (powi(3) here is exact for these).
        let a = if d > 0.0 && d < 0.5 { 0.5 * ((2.0 * d) * (2.0 * d) * (2.0 * d)) } else { a };
        assert_eq!(o[2 * k].to_bits(), (a as f32).to_bits(), "in_out_cubic({})", u);
        assert_eq!(o[2 * k + 1].to_bits(), (b as f32).to_bits(), "out_back({})", u);
    }
    // Anim: keyframes and pulses.
    let k = compile("let t = input(f32)\nlet times = input(f32)\nlet vals = input(f32)\nlet o = output(vec2)\nfn element(i) { o[i] = vec2(anim.keys(times, vals, 3, t[i]), anim.pulse_sum(times, 3, t[i], 0.5)) }").unwrap();
    let o = run(&k, 4, &[("t", vec![-1.0, 0.5, 1.5, 9.0]), ("times", vec![0.0, 1.0, 2.0]), ("vals", vec![10.0, 20.0, 40.0])], "o", 2);
    assert_eq!([o[0], o[2], o[4], o[6]], [10.0, 15.0, 30.0, 40.0]);
    assert_eq!(o[1], 0.0);
    assert!((o[3] - 0.5).abs() < 1e-6);
    // Beats.
    let k = compile("let beats = input(f32)\nlet t = input(f32)\nlet o = output(f32)\nfn element(i) { o[i] = beat.beat_at(beats, 4, t[i]) }").unwrap();
    let o = run(&k, 3, &[("beats", vec![0.0, 0.5, 1.0, 1.5]), ("t", vec![0.25, 1.25, 2.0])], "o", 1);
    assert_eq!(o, vec![0.5, 2.5, 4.0]);
    // Camera: identity view-projection.
    let k = compile("let o = output(vec3)\nlet vp = param(1.0)\nfn element(i) { let m = mat4(vp, 0.0, 0.0, 0.0, 0.0, vp, 0.0, 0.0, 0.0, 0.0, vp, 0.0, 0.0, 0.0, 0.0, 1.0)\n o[i] = vec3(cam.project(m, vec3(0.5, -0.25, 0.1)).xy, if cam.box_in_view(m, vec3(2.0), vec3(3.0)) { 1.0 } else { 0.0 }) }").unwrap();
    assert_eq!(run(&k, 1, &[], "o", 3), vec![0.5, -0.25, 0.0]);
}

#[test]
fn the_shared_stdlib_compiles_in_kernels() {
    let k = compile("let o = output(vec4)\nfn element(i) { let p = rot2(vec2(1.0, 0.0), 1.5707963)\n o[i] = vec4(p, sd_box(p, vec2(0.5)), knob(0.5) + luma(hsv2rgb(vec3(0.3, 0.5, 0.8))) * 0.0 + bayer4(vec2(float(i), 1.0)) * 0.0) }").unwrap_or_else(|e| panic!("{:?}", e));
    let o = run(&k, 1, &[], "o", 4);
    assert!((o[0]).abs() < 1e-6 && (o[1] - 1.0).abs() < 1e-6, "{:?}", o);
    assert_eq!(o[3], 1.0);
    // Shader-only helpers (screen-space derivatives) fail only when called.
    let e = compile("let o = output(f32)\nfn element(i) { o[i] = aa_step(0.5, float(i)) }").map(|_| ()).unwrap_err();
    assert!(e[0].message.contains("dFdx"), "{}", e[0].message);
}
