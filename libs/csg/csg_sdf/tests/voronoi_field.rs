//! Layer: integration.
//!
//! The kernel prelude's Voronoi cells (`voronoi3_nearest`, `voronoi3_border`, `voronoi3_edge`),
//! compiled by the kernel compiler, against brute-force f64 references over a 7x7x7 window of the
//! same seeds (the prelude's hash mirrored bit for bit), in a stretched lattice: the distance to
//! the cell's faces is exact wherever the window's bound does not cut it, and the distances to
//! the faces and to the edges never exceed the true ones (so both fields are distance bounds).

use makepad_csg_sdf::{SdfSplashExpr, SplashPoint};
use makepad_script::*;

fn make_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) }
}

/// A kernel from one Splash fn(p) expression.
fn field(src: &str) -> SdfSplashExpr {
    let mut vm = make_vm();
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", "voronoi_field", 0, 0, &format!("use mod.math.*\nlet f = {src}\n(f)"), &[]));
    assert!(vm.take_errors().is_empty(), "{src}");
    let f = v.as_object().expect("a fn");
    let _keep = vm.new_object_ref(f);
    SdfSplashExpr::compile(&mut vm, f, SplashPoint::Vec3, &[]).unwrap_or_else(|e| panic!("{src}: {e:?}"))
}

/// The prelude's `hash` (PCG output permutation), wrapping i32 arithmetic, `>>>` logical.
fn hash(x: i32) -> i32 {
    let s = x.wrapping_mul(747796405).wrapping_add(0xAC564B05u32 as i32);
    let w = ((((s as u32) >> (((s as u32) >> 28) + 4)) as i32) ^ s).wrapping_mul(277803737);
    (((w as u32) >> 22) as i32) ^ w
}

fn hash3(a: i32, b: i32, c: i32) -> i32 {
    hash(a ^ hash(b ^ hash(c.wrapping_add(0x7F4A7C15))))
}

fn seed(x: i32, y: i32, z: i32, random: f64, salt: i32) -> [f64; 3] {
    let h = hash3(x, y, z + salt * 7919);
    let u = [h & 1023, (h >> 10) & 1023, ((h as u32) >> 20) as i32 & 1023].map(|v| v as f64 / 1023.0);
    [x as f64 + 0.5 + (u[0] - 0.5) * random, y as f64 + 0.5 + (u[1] - 0.5) * random, z as f64 + 0.5 + (u[2] - 0.5) * random]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The nearest seed to q and the distance from p (q = m p) to its cell's faces, over all seeds
/// of the 7x7x7 cells round q (every seed that can matter for random <= 1).
fn brute(q: [f64; 3], m: [f64; 3], random: f64, salt: i32) -> ([f64; 3], f64) {
    let seeds = window(q, random, salt);
    let c = *seeds.iter().min_by(|a, b| dot(sub(**a, q), sub(**a, q)).total_cmp(&dot(sub(**b, q), sub(**b, q)))).unwrap();
    let a = sub(c, q);
    let face = seeds
        .iter()
        .filter(|s| dot(sub(**s, c), sub(**s, c)) > 1e-12)
        .map(|s| {
            let n = sub(*s, c);
            let w = [n[0] * m[0], n[1] * m[1], n[2] * m[2]];
            let b = sub(*s, q);
            (dot(b, b) - dot(a, a)) * 0.5 / dot(w, w).sqrt()
        })
        .fold(f64::INFINITY, f64::min);
    (c, face)
}

/// Seeds of the 7x7x7 cells round q.
fn window(q: [f64; 3], random: f64, salt: i32) -> Vec<[f64; 3]> {
    let i = q.map(|x| x.floor() as i32);
    let mut seeds = Vec::new();
    for z in -3..=3 {
        for y in -3..=3 {
            for x in -3..=3 {
                seeds.push(seed(i[0] + x, i[1] + y, i[2] + z, random, salt));
            }
        }
    }
    seeds
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// The distance from p (q = m p) to the nearest edge of the cells (where three meet), when it is
/// at most `reach`: an edge that near has its three seeds, and every seed nearer to its points,
/// within f1 + 2 reach of q, so the lines of those triples clipped by those seeds are exact there.
fn brute_edge(q: [f64; 3], m: [f64; 3], random: f64, salt: i32, reach: f64) -> Option<f64> {
    let mut seeds = window(q, random, salt);
    seeds.sort_by(|a, b| dot(sub(*a, q), sub(*a, q)).total_cmp(&dot(sub(*b, q), sub(*b, q))));
    let f1 = dot(sub(seeds[0], q), sub(seeds[0], q)).sqrt();
    let mmax = m[0].max(m[1]).max(m[2]);
    let near: Vec<[f64; 3]> = seeds.into_iter().take_while(|s| dot(sub(*s, q), sub(*s, q)).sqrt() <= f1 + 2.0 * reach * mmax).collect();
    let to_p = |x: [f64; 3]| [x[0] / m[0], x[1] / m[1], x[2] / m[2]];
    let mut best = f64::INFINITY;
    for i in 0..near.len() {
        for j in i + 1..near.len() {
            for k in j + 1..near.len() {
                let (a, b, c) = (near[i], near[j], near[k]);
                let (n1, n2) = (sub(b, a), sub(c, a));
                let dir = cross(n1, n2);
                let det = dot(n1, cross(n2, dir));
                if det.abs() < 1e-12 {
                    continue;
                }
                let (r1, r2, r3) = ((dot(b, b) - dot(a, a)) * 0.5, (dot(c, c) - dot(a, a)) * 0.5, dot(dir, q));
                let (u, v, w) = (cross(n2, dir), cross(dir, n1), cross(n1, n2));
                let x0 = [0, 1, 2].map(|e| (r1 * u[e] + r2 * v[e] + r3 * w[e]) / det);
                // where a (and so b and c) is nearer than every other seed: t in lo..hi
                let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
                for s in &near {
                    let n = sub(*s, a);
                    if dot(n, n) < 1e-12 || dot(sub(*s, b), sub(*s, b)) < 1e-12 || dot(sub(*s, c), sub(*s, c)) < 1e-12 {
                        continue;
                    }
                    let (slope, room) = (2.0 * dot(n, dir), dot(*s, *s) - dot(a, a) - 2.0 * dot(n, x0));
                    if slope.abs() < 1e-15 {
                        if room < 0.0 {
                            hi = f64::NEG_INFINITY;
                        }
                    } else if slope > 0.0 {
                        hi = hi.min(room / slope);
                    } else {
                        lo = lo.max(room / slope);
                    }
                }
                if lo > hi {
                    continue;
                }
                // the nearest point to p in p's metric
                let (x, d, qp) = (to_p(x0), to_p(dir), to_p(q));
                let t = (dot(sub(qp, x), d) / dot(d, d)).clamp(lo, hi);
                let e = sub([x[0] + t * d[0], x[1] + t * d[1], x[2] + t * d[2]], qp);
                best = best.min(dot(e, e).sqrt());
            }
        }
    }
    (best <= reach).then_some(best)
}

/// Deterministic points in a box (a 32-bit LCG).
fn points(n: usize, half: f64) -> Vec<[f64; 3]> {
    let mut s = 12345u32;
    let mut r = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s >> 8) as f64 / 16777216.0
    };
    (0..n).map(|_| [0; 3].map(|_| (r() * 2.0 - 1.0) * half)).collect()
}

fn eval(k: &SdfSplashExpr, ps: &[[f64; 3]]) -> Vec<f64> {
    let xyz: Vec<f32> = ps.iter().flat_map(|p| p.map(|x| x as f32)).collect();
    let mut out = vec![0f32; ps.len()];
    k.distance_batch(&xyz, &mut out);
    out.into_iter().map(|x| x as f64).collect()
}

#[test]
fn voronoi_cells_match_brute_force_and_bound_their_faces_and_edges() {
    let ps = points(200, 4.0);
    // a stretched lattice at the largest randomness the window is exact for
    let (m, random, salt) = ([0.8, 0.5, 0.8], 0.7, 3);
    let mq = format!("vec3({:.3}, {:.3}, {:.3})", m[0], m[1], m[2]);
    let border = |c: &str| format!("fn(p) {{\n let q = p * {mq}\n let s = voronoi3_nearest(q, {random}, {salt})\n return voronoi3_border(q, vec3(s.x, s.y, s.z), {mq}, {random}, {salt}, 0.00001).{c}\n}}");
    let (face, bound) = (eval(&field(&border("x")), &ps), eval(&field(&border("y")), &ps));
    let edge_k = field(&format!("|p| voronoi3_edge(p * {mq}, {mq}, {random}, {salt})"));
    let step = [0.011, -0.007, 0.013];
    let shifted: Vec<[f64; 3]> = ps.iter().map(|p| [p[0] + step[0], p[1] + step[1], p[2] + step[2]]).collect();
    let (edge, edge2) = (eval(&edge_k, &ps), eval(&edge_k, &shifted));
    let (mut exact, mut checked, mut strut, mut thick) = (0, 0, 0, 0);
    for (i, p) in ps.iter().enumerate() {
        let q = [p[0] * m[0], p[1] * m[1], p[2] * m[2]];
        let (_, truth) = brute(q, m, random, salt);
        assert!(face[i] <= truth + 1e-4, "a bound: {} over the true {truth} at {p:?}", face[i]);
        if bound[i] > truth + 1e-3 {
            assert!((face[i] - truth).abs() < 1e-4, "exact: {} against {truth} at {p:?}", face[i]);
            exact += 1;
        }
        assert!(edge[i] >= -1e-5 && (edge2[i] - edge[i]).abs() <= dot(step, step).sqrt() * 1.001 + 1e-5, "the edge field steps {} to {} at {p:?}", edge[i], edge2[i]);
        if let Some(truth) = brute_edge(q, m, random, salt, 0.6) {
            assert!(edge[i] <= truth + 1e-4, "an edge bound: {} over the true {truth} at {p:?}", edge[i]);
            checked += 1;
            // struts of a fifth of a cell: the bound adds little material
            strut += (truth < 0.2) as usize;
            thick += (edge[i] < 0.2) as usize;
        }
    }
    // the window's bound cuts in only near a far corner of its jitter
    assert!(exact * 100 >= ps.len() * 95, "{exact} of {} exact", ps.len());
    assert!(checked * 2 >= ps.len() && strut > 10, "{checked} edge distances checked, {strut} in a strut");
    assert!(thick * 100 <= strut * 140, "{thick} points within the struts' bound, {strut} within the struts");
}
