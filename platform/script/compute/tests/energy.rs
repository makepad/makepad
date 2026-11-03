//! Integer loop unrolling must preserve the rolled loop's arithmetic.
#![cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]

use makepad_script_compute::kernel::{compile_with, Kernel};
use makepad_script_compute::{native_vec, Backend};
use makepad_script_compute_sched::kernel::KernelExt;

#[test]
fn unrolled_integer_division() {
    let src = "let out = output(f32)\nfn element(i) {\nfor a in 0..2 { out[i * 2 + a] = float(a / 2) }\n}";
    let mut results = Vec::new();
    for backend in [Backend::Interp, Backend::Native] {
        let k = compile_with(src, &[], backend).unwrap();
        let mut out = [0.0f32; 2];
        let mut c = k.call();
        c.set_simd(false);
        c.output("out", &mut out).unwrap();
        if backend == Backend::Interp {
            c.run_interp(1).unwrap();
        } else {
            c.run(1).unwrap();
        }
        drop(c);
        results.push(out);
    }
    assert_eq!(results, vec![[0.0, 0.0]; 2], "integer loop division on Interp and Native");
}

#[test]
fn unrolled_integer_differential() {
    // Literal bounds unroll; a parameter bound keeps the same loop rolled.
    // Check both against host arithmetic, since Interp and Native share lowering.
    for (start, end, divisor) in [(0, 2, 2), (-3, 3, 2), (0, 8, 3), (2, 10, 4)] {
        let count = (end - start) as usize;
        let values = format!("vec4(float(a / {divisor}), float((a / 2) % 2), a / 2.0, float((a + 5) * 3 / 2))");
        let src = format!(
            "let out = output(vec4)\nlet rolled = output(vec4)\nlet end = param({end}, -8, 16)\n\
             fn element(i) {{\n\
             for a in {start}..{end} {{ if a == i % {count} + ({start}) {{ out[i] = {values} }} }}\n\
             for a in {start}..int(end) {{ if a == i % {count} + ({start}) {{ rolled[i] = {values} }} }}\n}}"
        );
        let expected: Vec<f32> = (0..17)
            .flat_map(|i| {
                let a = start + i % (end - start);
                [(a / divisor) as f32, ((a / 2) % 2) as f32, a as f32 / 2.0, ((a + 5) * 3 / 2) as f32]
            })
            .collect();
        for lanes in [4, 8] {
            let k = native_vec::with_lanes(lanes, || compile_with(&src, &[], Backend::Native)).unwrap();
            assert!(k.simd(), "generated loop must exercise SIMD");
            assert_eq!(k.vector_lanes(), lanes);
            for (interp, simd) in [(true, false), (false, false), (false, true)] {
                let mut out = vec![f32::NAN; expected.len()];
                let mut rolled = out.clone();
                let mut c = k.call();
                c.set_simd(simd);
                c.output("out", &mut out).unwrap();
                c.output("rolled", &mut rolled).unwrap();
                let stats = if interp { c.run_interp(17) } else { c.run(17) }.unwrap();
                assert!(!stats.overflowed);
                drop(c);
                assert_eq!(out, expected, "unrolled {start}..{end}, divisor={divisor}, lanes={lanes}, interp={interp}, simd={simd}");
                assert_eq!(rolled, expected, "rolled {start}..{end}, divisor={divisor}, lanes={lanes}, interp={interp}, simd={simd}");
            }
        }
    }
}

fn dshape(k: usize, axis: usize, p: [f64; 3]) -> f64 {
    let s = [(2 * (k & 1)) as f64 - 1.0, (2 * ((k >> 1) & 1)) as f64 - 1.0, (2 * (k >> 2)) as f64 - 1.0];
    let mut v = 0.25 * s[axis];
    for a in 0..3 {
        if a != axis {
            v *= 1.0 + p[a] * s[a];
        }
    }
    v
}

fn element_stiffness(nu: f64) -> Vec<f64> {
    let lam = nu / ((1.0 + nu) * (1.0 - 2.0 * nu));
    let mu = 1.0 / (2.0 * (1.0 + nu));
    let mut d = [[0.0; 6]; 6];
    for i in 0..3 {
        for j in 0..3 {
            d[i][j] = lam;
        }
        d[i][i] = lam + 2.0 * mu;
        d[i + 3][i + 3] = mu;
    }
    let g = 1.0 / 3f64.sqrt();
    let mut k0 = vec![0.0; 576];
    for q in 0..8 {
        let p = [g * ((2 * (q & 1)) as f64 - 1.0), g * ((2 * ((q >> 1) & 1)) as f64 - 1.0), g * ((2 * (q >> 2)) as f64 - 1.0)];
        let mut b = [[0.0; 24]; 6];
        for k in 0..8 {
            let (gx, gy, gz) = (dshape(k, 0, p), dshape(k, 1, p), dshape(k, 2, p));
            b[0][3 * k] = gx;
            b[1][3 * k + 1] = gy;
            b[2][3 * k + 2] = gz;
            b[3][3 * k] = gy;
            b[3][3 * k + 1] = gx;
            b[4][3 * k + 1] = gz;
            b[4][3 * k + 2] = gy;
            b[5][3 * k] = gz;
            b[5][3 * k + 2] = gx;
        }
        for i in 0..24 {
            for j in 0..24 {
                let mut acc = 0.0;
                for r in 0..6 {
                    for c in 0..6 {
                        acc += b[r][i] * d[r][c] * b[c][j];
                    }
                }
                k0[i * 24 + j] += acc * 0.125;
            }
        }
    }
    k0
}

const HEAD: &str = "let u = input(vec3)\nlet k0 = input(f32)\nlet xp = input(f32)\nlet dc = output(f32)\nlet nx = param(1.0, 1.0, 4096.0)\nlet ny = param(1.0, 1.0, 4096.0)\nlet penal = param(3.0, 1.0, 8.0)\nlet emin = param(0.000000001, 0.0, 1.0)\n";

const REPRO: &str = "fn element(i) {
    let ex = int(nx)
    let ey = int(ny)
    let px = ex + 1
    let py = ey + 1
    let e0 = i % ex
    let e1 = (i / ex) % ey
    let e2 = i / (ex * ey)
    var acc = 0.0
    for a in 0..24 {
        let an = a / 3
        let ma = ((e2 + an / 4) * py + e1 + (an / 2) % 2) * px + e0 + an % 2
        let va = u[ma]
        let ua = if a % 3 == 0 { va.x } else if a % 3 == 1 { va.y } else { va.z }
        var row = 0.0
        for bn in 0..8 {
            let mb = ((e2 + bn / 4) * py + e1 + (bn / 2) % 2) * px + e0 + bn % 2
            let vb = u[mb]
            let k = a * 24 + bn * 3
            row = row + k0[k] * vb.x + k0[k + 1] * vb.y + k0[k + 2] * vb.z
        }
        acc = acc + ua * row
    }
    dc[i] = -penal * pow(xp[i], penal - 1.0) * (1.0 - emin) * acc
}";

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        ((self.next() >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

#[test]
fn energy_original_body() {
    let (nx, ny, nz) = (12usize, 4usize, 4usize);
    let nodes = (nx + 1) * (ny + 1) * (nz + 1);
    let elems = nx * ny * nz;
    let k0d = element_stiffness(0.3);
    let k0: Vec<f32> = k0d.iter().map(|x| *x as f32).collect();
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let u: Vec<f32> = (0..nodes * 3).map(|_| r.unit()).collect();
    let xp = vec![0.4f32; elems];
    // f64 reference of u_e^T K0 u_e
    let mut want = vec![0.0f64; elems];
    for i in 0..elems {
        let (e0, e1, e2) = (i % nx, (i / nx) % ny, i / (nx * ny));
        let node = |c: usize| ((e2 + (c >> 2)) * (ny + 1) + e1 + ((c >> 1) & 1)) * (nx + 1) + e0 + (c & 1);
        let mut acc = 0.0;
        for a in 0..24 {
            for b in 0..24 {
                acc += u[node(a / 3) * 3 + a % 3] as f64 * k0d[a * 24 + b] * u[node(b / 3) * 3 + b % 3] as f64;
            }
        }
        want[i] = -3.0 * 0.4f64.powf(2.0) * acc;
    }
    let src = format!("{HEAD}{REPRO}");
    let run = |k: &Kernel, interp: bool, simd: bool| {
        let mut dc = vec![0.0f32; elems];
        let mut c = k.call();
        c.set_simd(simd);
        c.set_param("nx", nx as f32);
        c.set_param("ny", ny as f32);
        c.input("u", &u).unwrap();
        c.input("k0", &k0).unwrap();
        c.input("xp", &xp).unwrap();
        c.output("dc", &mut dc).unwrap();
        let stats = if interp { c.run_interp(elems) } else { c.run(elems) }.unwrap();
        assert!(!stats.overflowed);
        dc
    };
    let report = |name: &str, dc: &[f32]| {
        let negative = dc.iter().filter(|x| **x < 0.0).count();
        let bad = dc.iter().zip(&want).filter(|(g, w)| ((**g as f64) - **w).abs() > 1e-3 * w.abs().max(1e-3)).count();
        eprintln!("{name}: dc<0 {negative}/{elems}, off the f64 reference {bad}/{elems}");
        assert_eq!(negative, elems, "{name}: PSD energy must give negative sensitivities");
        assert_eq!(bad, 0, "{name}: energy must match independent f64 reference");
    };
    let ki = compile_with(&src, &[], Backend::Interp).unwrap_or_else(|e| panic!("{e:?}"));
    let i = run(&ki, true, false);
    report("interp", &i);
    let kn = compile_with(&src, &[], Backend::Native).unwrap_or_else(|e| panic!("{e:?}"));
    let s = run(&kn, false, false);
    report("scalar", &s);
    for lanes in [4u32, 8] {
        let kv = native_vec::with_lanes(lanes, || compile_with(&src, &[], Backend::Native)).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(kv.simd(), "energy must exercise SIMD");
        assert_eq!(kv.vector_lanes(), lanes);
        let v = run(&kv, false, true);
        report(&format!("x{lanes}"), &v);
    }
}
