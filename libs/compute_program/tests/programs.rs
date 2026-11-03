//! The proof programs at their smallest sizes, on the CPU.
use makepad_compute_program::programs::{ELASTIC, FLOW, FRACTAL};
use makepad_compute_program::*;
use std::sync::Arc;

fn obj(fields: &[(&str, Value)]) -> Value {
    Value::object(fields)
}

fn nums(v: &[f64]) -> Value {
    Value::Array(v.iter().map(|x| Value::Number(*x)).collect())
}

fn open(name: &str, source: &str, config: SessionConfig) -> Session {
    Session::open(&Program::new(name, source), config)
}

fn tile_words(output: &Output) -> Vec<u32> {
    match &output.data {
        OutputData::Words(w) => w.to_vec(),
        other => panic!("a tile is words: {other:?}"),
    }
}

/// Waits for the next version of output `name`.
fn next_output(session: &Session, name: &str, after: u64) -> Output {
    loop {
        let u = session.wait();
        if let Some(Err(e)) = &u.end {
            panic!("the program ended: {e}");
        }
        if let Some(o) = u.output(name).filter(|o| o.version > after) {
            return o.clone();
        }
    }
}

/// A tile message: `at` is the top-left corner and the texel step.
fn tile(slot: u32, at: [f64; 3], res: usize, kind: u32, power: u32, max_iter: u32, de: bool, trap: bool) -> Value {
    let n = |x: f64| Value::Number(x);
    obj(&[
        ("slot", n(slot as f64)),
        ("x", n(at[0])),
        ("y", n(at[1])),
        ("step", n(at[2])),
        ("res", n(res as f64)),
        ("kind", n(kind as f64)),
        ("power", n(power as f64)),
        ("max_iter", n(max_iter as f64)),
        ("de", n(de as u32 as f64)),
        ("trap", n(trap as u32 as f64)),
        ("julia", nums(&[-0.123, 0.745])),
    ])
}

/// Every tile of `tiles` through one session, one at a time: the words of each.
fn render(backend: Backend, tiles: &[Value]) -> Vec<Vec<u32>> {
    let session = open("fractal.splash", FRACTAL, SessionConfig { backend, ..SessionConfig::default() });
    let mut versions = [0u64; 2];
    let out = tiles
        .iter()
        .enumerate()
        .map(|(k, t)| {
            let slot = k % 2;
            let rev = session.send("tile", t.clone());
            let o = next_output(&session, ["tile0", "tile1"][slot], versions[slot]);
            versions[slot] = o.version;
            assert_eq!(o.rev, rev, "the tile of its own message");
            tile_words(&o)
        })
        .collect();
    session.send("stop", true.into());
    assert_eq!(session.wait_end(), Ok(Value::Number(tiles.len() as f64)));
    out
}

#[test]
fn fractal_tiles_are_the_same_bits_native_and_interpreted_for_every_kind() {
    let res = 8;
    let whole = [-2.0, -1.25, 2.5 / res as f64];
    let edge = [-0.7436, 0.1318, 0.0004 / res as f64];
    let mut tiles = Vec::new();
    for (de, trap) in [(false, false), (true, false), (false, true)] {
        for kind in 0..5 {
            tiles.push(tile(tiles.len() as u32 % 2, whole, res, kind, 4, 200, de, trap));
        }
    }
    // past one pass (2048 iterations): the orbit state carried over three passes
    tiles.push(tile(1, edge, res, 0, 2, 4100, true, false));
    tiles.push(tile(0, whole, res, 5, 3, 64, false, false));
    tiles.push(tile(1, [0.3 - 0.25, 0.1 - 0.25, 0.5 / res as f64], 2 * res, 5, 5, 64, false, false));
    let native = render(Backend::Native, &tiles);
    assert_eq!(native, render(Backend::Interp, &tiles));
    // The main cardioid is inside up front; the far corner escapes at once.
    let first = &native[0];
    let at = |x: f64, y: f64| first[(((y - whole[1]) / whole[2]) as usize) * res + ((x - whole[0]) / whole[2]) as usize];
    assert_eq!(at(-0.1, 0.0) >> 24, 255);
    assert_eq!(at(-1.9, -1.2) >> 24, 0);
    // Newton: every texel of the whole view reaches one of the three roots.
    assert!(native[native.len() - 2].iter().all(|w| (1..=3).contains(&(w >> 24))));
}

#[test]
fn the_next_fractal_tile_waits_while_one_renders() {
    let session = open("fractal.splash", FRACTAL, SessionConfig::default());
    let a = session.send("tile", tile(0, [-2.0, -1.25, 0.1], 16, 0, 2, 300, false, false));
    // `took` says the program has the tile: the next one can wait in its inbox.
    loop {
        let u = session.wait();
        if u.value("took") == Some(&Value::Number(a as f64)) {
            break;
        }
        assert!(u.end.is_none(), "{:?}", u.end);
    }
    let b = session.send("tile", tile(1, [-1.0, -0.5, 0.05], 16, 1, 2, 300, false, false));
    let t0 = next_output(&session, "tile0", 0);
    let t1 = next_output(&session, "tile1", 0);
    assert_eq!((t0.rev, t1.rev), (a, b));
    assert_eq!(tile_words(&t0).len(), 256);
    session.send("stop", true.into());
    assert_eq!(session.wait_end(), Ok(Value::Number(2.0)));
}

// ---------------------------------------------------------------------------------------------
// Elasticity: the f64 reference

/// d N_k / d axis at a point of the unit hex8 (d/dx = 2 d/dxi).
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

struct Case {
    n: [usize; 3],
    fixed: Vec<f32>,
    load: Vec<f32>,
}

/// Fixed at x = 0, a unit load down (-z) spread over the free end's top edge.
fn cantilever(n: [usize; 3]) -> Case {
    let p = [n[0] + 1, n[1] + 1, n[2] + 1];
    let nodes = p[0] * p[1] * p[2];
    let mut fixed = vec![0.0; nodes];
    let mut load = vec![0.0; nodes * 3];
    for z in 0..p[2] {
        for y in 0..p[1] {
            for x in 0..p[0] {
                let i = (z * p[1] + y) * p[0] + x;
                if x == 0 {
                    fixed[i] = 1.0;
                }
                if x == n[0] && z == n[2] {
                    load[3 * i + 2] = -1.0 / p[1] as f32;
                }
            }
        }
    }
    Case { n, fixed, load }
}

/// K u = f by Gaussian elimination in f64 (fixed dofs: identity rows and columns).
fn dense_solve(case: &Case, nu: f64) -> Vec<f64> {
    let [nx, ny, nz] = case.n;
    let p = [nx + 1, ny + 1, nz + 1];
    let dofs = p[0] * p[1] * p[2] * 3;
    let k0 = element_stiffness(nu);
    let mut k = vec![0.0f64; dofs * dofs];
    for ez in 0..nz {
        for ey in 0..ny {
            for ex in 0..nx {
                let node = |c: usize| ((ez + (c >> 2)) * p[1] + ey + ((c >> 1) & 1)) * p[0] + ex + (c & 1);
                for a in 0..24 {
                    for b in 0..24 {
                        let (ga, gb) = (node(a / 3) * 3 + a % 3, node(b / 3) * 3 + b % 3);
                        k[ga * dofs + gb] += k0[a * 24 + b];
                    }
                }
            }
        }
    }
    let mut f: Vec<f64> = case.load.iter().map(|x| *x as f64).collect();
    for i in 0..dofs {
        if case.fixed[i / 3] > 0.5 {
            for j in 0..dofs {
                k[i * dofs + j] = 0.0;
                k[j * dofs + i] = 0.0;
            }
            k[i * dofs + i] = 1.0;
            f[i] = 0.0;
        }
    }
    for c in 0..dofs {
        let piv = (c..dofs).max_by(|a, b| k[a * dofs + c].abs().total_cmp(&k[b * dofs + c].abs())).unwrap();
        for j in 0..dofs {
            k.swap(c * dofs + j, piv * dofs + j);
        }
        f.swap(c, piv);
        for r in c + 1..dofs {
            let m = k[r * dofs + c] / k[c * dofs + c];
            if m != 0.0 {
                for j in c..dofs {
                    k[r * dofs + j] -= m * k[c * dofs + j];
                }
                f[r] -= m * f[c];
            }
        }
    }
    let mut u = vec![0.0; dofs];
    for r in (0..dofs).rev() {
        let mut acc = f[r];
        for j in r + 1..dofs {
            acc -= k[r * dofs + j] * u[j];
        }
        u[r] = acc / k[r * dofs + r];
    }
    u
}

fn elastic_config(case: &Case, extra: &[(&str, Value)], threads: usize, backend: Backend) -> SessionConfig {
    let mut fields = vec![
        ("nx", (case.n[0] as f64).into()),
        ("ny", (case.n[1] as f64).into()),
        ("nz", (case.n[2] as f64).into()),
        ("nu", 0.3.into()),
        ("tol", 0.000001.into()),
        ("max_its", 400.0.into()),
        ("design_its", 0.0.into()),
    ];
    fields.extend(extra.iter().cloned());
    SessionConfig {
        params: obj(&fields),
        inputs: vec![
            Input::f32("fixed", &case.fixed),
            Input::words("load", ElemTy::Vec3, case.load.iter().map(|x| x.to_bits()).collect::<Arc<[u32]>>()),
        ],
        threads,
        backend,
        ..SessionConfig::default()
    }
}

#[test]
fn cantilever_solve_matches_a_dense_f64_solve() {
    let case = cantilever([4, 2, 2]);
    let reference = dense_solve(&case, 0.3);
    let session = open("elastic.splash", ELASTIC, elastic_config(&case, &[], 4, Backend::Native));
    let result = session.wait_end().unwrap_or_else(|e| panic!("{e}"));
    let u = result.get("u").and_then(|u| u.numbers()).expect("u");
    let its = result.get("its").and_then(|v| v.as_number()).expect("its");
    assert!(its < 400.0, "converged in {its} iterations");
    let scale = reference.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    for (i, (a, b)) in u.iter().zip(&reference).enumerate() {
        assert!((a - b).abs() <= 1e-4 * scale, "dof {i}: {a} vs {b} (max {scale})");
    }
}

#[test]
fn simp_design_steps_and_steers_mid_run() {
    let case = cantilever([12, 4, 4]);
    let extra = [
        ("design_its", 3.0.into()),
        ("volfrac", 0.4.into()),
        ("penal", 3.0.into()),
        ("rmin", 1.5.into()),
        ("emin", 0.000000001.into()),
        ("move", 0.2.into()),
        ("tol", 0.001.into()),
        ("lockstep", true.into()),
    ];
    let session = open("elastic.splash", ELASTIC, elastic_config(&case, &extra, 4, Backend::Native));
    let mut seen = Vec::new();
    while seen.len() < 3 {
        let u = session.wait();
        if let Some(Err(e)) = &u.end {
            panic!("{e}");
        }
        if let Some(v) = u.value("iteration") {
            let it = v.get("it").and_then(|x| x.as_number()).unwrap();
            if seen.last().map_or(true, |(last, _, _, _)| *last < it) {
                let c = v.get("compliance").and_then(|x| x.as_number()).unwrap();
                let vol = v.get("volume").and_then(|x| x.as_number()).unwrap();
                let multiplier = v.get("multiplier").and_then(|x| x.as_number()).unwrap();
                seen.push((it, c, vol, multiplier));
                if it == 1.0 {
                    // Steer: the load moves to the free end's bottom edge; the design goes on.
                    let p = [case.n[0] + 1, case.n[1] + 1, case.n[2] + 1];
                    let mut load = vec![0.0; p[0] * p[1] * p[2] * 3];
                    for y in 0..p[1] {
                        load[3 * (y * p[0] + case.n[0]) + 2] = -1.0 / p[1] as f64;
                    }
                    session.send("load", nums(&load));
                } else {
                    session.send("go", true.into());
                }
            }
        }
    }
    let result = session.wait_end().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.get("its").and_then(|x| x.as_number()), Some(3.0));
    let its: Vec<f64> = seen.iter().map(|s| s.0).collect();
    assert_eq!(its, vec![1.0, 2.0, 3.0], "the iteration count goes on through the steer");
    for (_, _, vol, _) in &seen {
        assert!((vol - 0.4).abs() <= 1e-4, "volume {vol}: {seen:?}");
    }
    assert!(seen[2].1 < seen[1].1, "compliance falls under the new load: {seen:?}");
}

#[test]
fn flow_conserves_mass_and_its_drag_is_the_same_on_any_worker_count() {
    let params = obj(&[
        ("nx", 24.0.into()),
        ("ny", 12.0.into()),
        ("nz", 12.0.into()),
        ("tau", 0.8.into()),
        ("force", 0.00001.into()),
        ("radius", 3.0.into()),
        ("steps", 50.0.into()),
        ("report", 10.0.into()),
    ]);
    let run = |threads: usize| {
        let session = open("flow.splash", FLOW, SessionConfig { params: params.clone(), threads, ..SessionConfig::default() });
        session.wait_end().unwrap_or_else(|e| panic!("{e}"))
    };
    let one = run(1);
    let many = run(8);
    assert_eq!(one, many, "the same bits on 1 and 8 workers");
    let mass0 = one.get("mass0").and_then(|x| x.as_number()).unwrap();
    let mass = one.get("mass").and_then(|x| x.as_number()).unwrap();
    assert!(((mass - mass0) / mass0).abs() < 1e-5, "mass {mass0} -> {mass}");
    let drags = one.get("drags").and_then(|d| d.numbers()).unwrap();
    assert_eq!(drags.len(), 5);
    assert!(drags.iter().all(|d| d.is_finite()) && drags[4] > 0.0, "drag along the flow: {drags:?}");
}
