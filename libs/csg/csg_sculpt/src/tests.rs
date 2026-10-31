use crate::brush::*;
use crate::layer::*;
use crate::material::*;
use crate::math::*;
use crate::ops::*;
use crate::record::*;
use crate::rig::*;
use crate::stroke::*;
use std::sync::Arc;

/// A tiny deterministic generator for sample points.
struct Rng(u32);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 = hash(self.0.wrapping_add(0x9E37_79B9));
        (self.0 >> 8) as f32 / 16_777_216.0
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
    fn point(&mut self, b: &Aabb) -> V3 {
        [self.range(b.lo[0], b.hi[0]), self.range(b.lo[1], b.hi[1]), self.range(b.lo[2], b.hi[2])]
    }
}

fn body() -> Program {
    // a torso, two limbs on a spline, a head, a twisted horn, a carved mouth
    Program::new(vec![
        Edit::shape(Fold::Union, 0.0, Shape::Ellipsoid { radii: [8.0, 11.0, 6.0] }, vec![]),
        Edit::shape(
            Fold::Union,
            3.0,
            Shape::tube(&[[6.0, 4.0, 0.0, 3.0], [12.0, 2.0, 2.0, 2.5], [16.0, -4.0, 1.0, 1.8], [17.0, -9.0, 0.0, 1.2]], true),
            vec![],
        ),
        Edit::shape(Fold::Union, 2.0, Shape::Blobs { balls: vec![[0.0, 14.0, 0.0, 4.5], [1.5, 16.0, 2.0, 3.0]], k: 2.5 }, vec![]),
        Edit::shape(
            Fold::Union,
            1.0,
            Shape::Tube { pts: vec![[0.0, 0.0, 0.0, 1.6], [0.0, 8.0, 0.0, 0.4]] },
            vec![Warp::Translate([2.0, 17.0, 0.0]), Warp::TwistY { rate: 0.3 }, Warp::BendX { rate: 0.05 }],
        ),
        Edit::shape(Fold::Sub, 1.0, Shape::RoundBox { half: [2.0, 0.6, 3.0], r: 0.5 }, vec![Warp::Translate([0.0, 13.0, 4.0])]),
        // a chamfered collar and a paint band
        Edit::shape(Fold::Union, 1.5, Shape::RoundBox { half: [4.0, 1.0, 4.0], r: 0.2 }, vec![Warp::Translate([0.0, 10.5, 0.0])]).chamfered(),
        Edit::shape(Fold::Paint, 0.0, Shape::RoundBox { half: [9.0, 1.5, 7.0], r: 0.0 }, vec![Warp::Translate([0.0, 2.0, 0.0])]),
        Edit::shape(
            Fold::Union,
            2.0,
            Shape::RoundBox { half: [3.0, 6.0, 3.0], r: 1.0 },
            vec![Warp::Translate([-10.0, -6.0, 0.0]), Warp::TaperY { k: -0.08 }],
        ),
    ])
}

#[test]
fn smin_is_exact_outside_the_blend() {
    for &(a, k) in &[(0.25f32, 1.0f32), (-3.0, 0.5), (1e-3, 2.0)] {
        assert_eq!(smin(a, a + 2.0 * k, k).to_bits(), a.to_bits());
        assert_eq!(smin(a + k * 1.5, a, k).to_bits(), a.to_bits());
        assert!(smin(a, a, k) < a); // blends below both inside the radius
    }
    assert_eq!(smin(FAR, 2.0, 1.0), 2.0);
}

#[test]
fn round_cone_matches_spheres_and_capsule() {
    let a = [0.0, 0.0, 0.0];
    let b = [10.0, 0.0, 0.0];
    for p in [[0.0, 5.0, 0.0], [5.0, 3.0, 1.0], [-4.0, 0.0, 0.0], [14.0, 2.0, 0.0]] {
        // equal radii: the capsule
        let cap = {
            let t = (p[0] / 10.0f32).clamp(0.0, 1.0);
            len(sub(p, [10.0 * t, 0.0, 0.0])) - 2.0
        };
        assert!((sd_round_cone(p, a, 2.0, b, 2.0) - cap).abs() < 1e-4, "{p:?}");
    }
    // the end caps are the spheres
    assert!((sd_round_cone([-5.0, 0.0, 0.0], a, 3.0, b, 1.0) - 2.0).abs() < 1e-4);
    assert!((sd_round_cone([15.0, 0.0, 0.0], a, 3.0, b, 1.0) - 4.0).abs() < 1e-4);
    // one sphere inside the other: the big one
    assert!((sd_round_cone([0.0, 9.0, 0.0], a, 8.0, [1.0, 0.0, 0.0], 2.0) - 1.0).abs() < 1e-4);
}

#[test]
fn tube_spline_passes_through_its_control_points() {
    let ctrl = [[0.0, 0.0, 0.0, 2.0], [10.0, 5.0, 0.0, 1.0], [20.0, 0.0, 5.0, 1.5], [25.0, -5.0, 5.0, 0.5]];
    let t = Shape::tube(&ctrl, true);
    if let Shape::Tube { pts } = &t {
        assert!(pts.len() > ctrl.len() * 4, "{} points", pts.len());
    }
    for c in ctrl {
        let d = t.distance([c[0], c[1], c[2]]);
        assert!((d + c[3]).abs() < 1e-3, "{d} at {c:?}");
    }
}

#[test]
fn every_program_is_a_safe_distance() {
    // |d(a) - d(b)| <= L |a - b|: the raymarcher's and the slicer's step rule
    let mut progs = vec![body()];
    progs.push(Program::new(vec![
        Edit::shape(Fold::Union, 0.0, Shape::Sphere { r: 8.0 }, vec![]),
        Edit::Mod(Mod::Displace { amp: 0.6, scale: 3.0, octaves: 3, seed: 7, region: Aabb::new([-10.0; 3], [10.0; 3]), margin: 2.0 }),
        Edit::Mod(Mod::Shell { t: 1.2, side: ShellSide::Inside }),
    ]));
    for prog in &progs {
        let l = prog.lipschitz();
        let b = prog.bounds().expand(3.0);
        let mut r = Rng(1);
        let mut worst = 0.0f32;
        for _ in 0..20000 {
            let a = r.point(&b);
            let dir = normalize([r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0)]);
            let h = r.range(0.01, 0.5);
            let c = add(a, scale(dir, h));
            let ratio = (prog.eval(a) - prog.eval(c)).abs() / h;
            worst = worst.max(ratio / l);
        }
        assert!(worst <= 1.0 + 2e-3, "gradient {worst} x the bound");
    }
}

#[test]
fn culled_eval_keeps_sign_and_the_inside() {
    let prog = body();
    let b = prog.bounds().expand(2.0);
    let mut r = Rng(2);
    for _ in 0..40000 {
        let p = r.point(&b);
        // culling is exact against the banded fold, for any band
        for band in [1.0, 4.0] {
            let (w, c) = (prog.eval_banded(p, band), prog.eval_culled(p, band));
            assert_eq!(w.to_bits(), c.to_bits(), "band {band} at {p:?}: {w} vs {c}");
        }
        // and the banded fold (band >= 2 x the largest blend) keeps the
        // exact fold's sign, and its value near the surface
        let e = prog.eval(p);
        let w = prog.eval_banded(p, 8.0);
        assert_eq!(e < 0.0, w < 0.0, "sign at {p:?}: {e} vs {w}");
        if e.abs() < 1.0 {
            assert!((e - w).abs() <= 1e-5, "near-surface value at {p:?}: {e} vs {w}");
        }
    }
}

#[test]
fn edit_bounds_hold_their_shape() {
    // outside an edit's box its term is at least the blend: the fold is a no-op
    let prog = body();
    let mut r = Rng(3);
    for e in &prog.edits {
        let Edit::Shape { blend, shape, warps, .. } = e else { continue };
        let Some(bx) = e.bounds(0.0) else { continue };
        let l = Edit::chain_lipschitz(shape, warps);
        let wide = bx.expand(10.0);
        for _ in 0..5000 {
            let p = r.point(&wide);
            if bx.contains(p) {
                continue;
            }
            let mut q = p;
            for w in warps {
                q = w.map(q);
            }
            let _ = l;
            assert!(Edit::shape(Fold::Union, 0.0, shape.clone(), warps.clone()).apply(FAR, p) >= *blend - 1e-4, "{p:?}");
        }
    }
}

#[test]
fn shell_mirror_and_offset() {
    let shell = Program::new(vec![
        Edit::shape(Fold::Union, 0.0, Shape::Sphere { r: 10.0 }, vec![]),
        Edit::Mod(Mod::Shell { t: 2.0, side: ShellSide::Inside }),
    ]);
    assert!(shell.eval([9.0, 0.0, 0.0]) < 0.0);
    assert!(shell.eval([7.0, 0.0, 0.0]) > 0.0);
    assert!(shell.eval([11.0, 0.0, 0.0]) > 0.0);
    let thick = Program::new(vec![
        Edit::shape(Fold::Union, 0.0, Shape::Sphere { r: 10.0 }, vec![]),
        Edit::Mod(Mod::Offset { t: 1.5 }),
    ]);
    assert!((thick.eval([0.0, 0.0, 0.0]) + 11.5).abs() < 1e-4);
    let half = Program::new(vec![Edit::shape(
        Fold::Union,
        0.0,
        Shape::Tube { pts: vec![[2.0, 0.0, 0.0, 1.0], [8.0, 3.0, 1.0, 2.0]] },
        vec![Warp::Mirror { axis: 0, at: 0.0 }],
    )]);
    let mut r = Rng(4);
    for _ in 0..1000 {
        let p = r.point(&Aabb::new([-12.0; 3], [12.0; 3]));
        assert_eq!(half.eval(p).to_bits(), half.eval([-p[0], p[1], p[2]]).to_bits());
    }
    let b = half.edits[0].bounds(0.0).unwrap();
    assert!(b.lo[0] < -9.0 && b.hi[0] > 9.0);
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

const CELL: f32 = 0.5;
const BAND: f32 = 2.0;

fn base() -> Base {
    let prog = Program::new(vec![
        Edit::shape(Fold::Union, 0.0, Shape::Sphere { r: 10.0 }, vec![]),
        Edit::shape(Fold::Union, 2.0, Shape::RoundBox { half: [12.0, 2.0, 6.0], r: 1.0 }, vec![Warp::Translate([0.0, -9.0, 0.0])]).with_material(1),
    ]);
    Arc::new(move |p| prog.eval_material(p))
}

fn arc(from: V3, to: V3, lift: f32, n: usize, pressure: impl Fn(f32) -> f32) -> Vec<[f32; 4]> {
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            let p = mix3(from, to, t);
            let up = lift * (std::f32::consts::PI * t).sin();
            let n = normalize(p);
            [p[0] + n[0] * up, p[1] + n[1] * up, p[2] + n[2] * up, pressure(t)]
        })
        .collect()
}

fn strokes() -> Vec<Stroke> {
    vec![
        Stroke::new(Mode::Add, 2.5, arc([-6.0, 7.0, 5.0], [6.0, 7.0, 5.0], 3.5, 9, |t| 0.5 + 0.5 * t)).with_blend(1.0).with_material(2),
        Stroke::new(Mode::Carve, 1.5, arc([0.0, 10.0, -2.0], [3.0, 8.0, -6.0], 0.0, 5, |_| 1.0)).with_blend(0.5),
        Stroke::new(Mode::Smooth, 3.0, arc([-5.0, 8.0, 4.0], [5.0, 8.0, 4.0], 0.5, 6, |_| 1.0)).with_strength(0.8),
        Stroke::new(Mode::Inflate, 3.0, arc([7.0, 0.0, 7.0], [7.0, 5.0, 5.0], 0.0, 4, |_| 0.9)).with_strength(0.7),
        Stroke::new(Mode::Flatten, 4.0, vec![[0.0, 10.0, 0.0, 1.0], [1.0, 10.0, 1.0, 1.0]]).with_plane([0.0, 1.0, 0.0], 9.0).with_strength(0.8),
        Stroke::new(Mode::Pinch, 2.0, arc([-9.0, 0.0, 4.0], [-6.0, 0.0, 8.0], 0.0, 4, |_| 1.0)).with_strength(0.6),
        Stroke::new(Mode::Grab, 4.0, vec![[0.0, -4.0, 9.5, 1.0], [0.0, -4.0, 10.5, 1.0], [0.5, -4.5, 11.3, 1.0]]).with_strength(1.0),
        Stroke::new(Mode::Add, 1.2, arc([-3.0, 4.0, 10.0], [3.0, 1.0, 10.0], 0.5, 7, |t| 1.0 - 0.6 * t)).with_blend(0.8).with_material(3),
        Stroke::new(Mode::Paint, 2.5, arc([-2.0, -2.0, 9.6], [6.0, -3.0, 7.0], 0.0, 5, |_| 1.0)).with_material(4),
        Stroke::new(Mode::Smooth, 2.0, arc([-3.0, 3.0, 9.5], [3.0, 2.0, 9.5], 0.0, 4, |_| 1.0)).with_strength(0.5),
    ]
}

/// Every sample of every brick either layer holds, bit for bit.
fn assert_same(a: &Layer, b: &Layer, what: &str) {
    let mut keys = a.keys();
    keys.extend(b.keys());
    let mut diff = 0;
    for k in &keys {
        for z in 0..BRICK {
            for y in 0..BRICK {
                for x in 0..BRICK {
                    let g = [k[0] * BRICK + x, k[1] * BRICK + y, k[2] * BRICK + z];
                    let (da, ma) = a.at_material(g);
                    let (db, mb) = b.at_material(g);
                    if da.to_bits() != db.to_bits() || ma != mb {
                        if diff < 5 {
                            eprintln!("{what}: {g:?} {da}/{ma} vs {db}/{mb}");
                        }
                        diff += 1;
                    }
                }
            }
        }
    }
    assert_eq!(diff, 0, "{what}: {diff} samples differ");
}

fn baked(strokes: &[Stroke], mirror: Option<usize>) -> Layer {
    let mut l = Layer::new(base(), CELL, BAND, mirror);
    l.bake(strokes);
    l
}

#[test]
fn strokes_shape_the_surface() {
    let s = strokes();
    let upto = |i: usize| baked(&s[..=i], None);
    let b = base();
    let l = upto(0);
    // the add stroke's centre line is now inside, above the old surface
    let c = s[0].path[4];
    assert!(b([c[0], c[1], c[2]]).0 > 0.0 && l.sample([c[0], c[1], c[2]]) < 0.0, "add");
    // the carve's centre is now outside
    let l = upto(1);
    let c = s[1].path[2];
    assert!(b([c[0], c[1], c[2]]).0 < 0.0 && l.sample([c[0], c[1], c[2]]) > 0.0, "carve");
    let l = upto(4);
    // flatten: the dome top at y=10 is cut toward the y=9 plane
    assert!(l.sample([0.0, 9.6, 0.0]) > 0.0, "flatten {}", l.sample([0.0, 9.6, 0.0]));
    // inflate pushed the surface out along the path
    let c = s[3].path[1];
    let n = normalize([c[0], c[1], c[2]]);
    let probe = add([c[0], c[1], c[2]], scale(n, 0.8));
    assert!(l.sample(probe) < b(probe).0, "inflate");
    let l = upto(6);
    // grab pulled material out toward +z
    assert!(l.sample([0.4, -4.4, 10.9]) < 0.0 && b([0.4, -4.4, 10.9]).0 > 0.0, "grab {}", l.sample([0.4, -4.4, 10.9]));
}

#[test]
fn influence_is_exactly_bounded() {
    let s = strokes();
    let l = baked(&s, None);
    let b = base();
    let all = s.iter().fold(Aabb::EMPTY, |a, st| a.union(&st.bounds(BAND)));
    let mut r = Rng(5);
    let wide = all.expand(8.0);
    let mut outside = 0;
    for _ in 0..20000 {
        let p = r.point(&wide);
        let g = [(p[0] / CELL).round() as i32, (p[1] / CELL).round() as i32, (p[2] / CELL).round() as i32];
        let q = [g[0] as f32 * CELL, g[1] as f32 * CELL, g[2] as f32 * CELL];
        if s.iter().any(|st| st.bounds(BAND).contains(q)) {
            continue;
        }
        outside += 1;
        assert_eq!(l.at(g).to_bits(), b(q).0.clamp(-BAND, BAND).to_bits(), "{q:?}");
    }
    assert!(outside > 1000);
}

#[test]
fn rebake_equals_full_bake() {
    let s = strokes();
    let mut l = baked(&s, None);
    let full = l.brick_count();

    // edit a stroke's radius and blend (the value-site edit)
    let mut s2 = s.clone();
    let old = s2[3].clone();
    s2[3].radius = 2.0;
    s2[3].strength = 0.4;
    let n = l.rebake(&s2, 3, &[old, s2[3].clone()]);
    assert_same(&l, &baked(&s2, None), "edit inflate");
    assert!(n < full, "edit wrote {n} of {full} bricks");

    // edit the first stroke: everything after it that reads neighbours follows
    let mut s3 = s2.clone();
    let old = s3[0].clone();
    s3[0].blend = 2.0;
    s3[0].radius = 3.0;
    l.rebake(&s3, 0, &[old, s3[0].clone()]);
    assert_same(&l, &baked(&s3, None), "edit first add");

    // delete a smooth stroke
    let mut s4 = s3.clone();
    let gone = s4.remove(2);
    l.rebake(&s4, 2, &[gone]);
    assert_same(&l, &baked(&s4, None), "delete smooth");

    // insert a carve before the grab
    let mut s5 = s4.clone();
    let ins = Stroke::new(Mode::Carve, 1.0, vec![[0.0, -4.0, 9.0, 1.0], [1.0, -3.0, 9.6, 1.0]]).with_blend(0.3);
    s5.insert(5, ins.clone());
    l.rebake(&s5, 5, &[ins]);
    assert_same(&l, &baked(&s5, None), "insert carve");
}

#[test]
fn rebake_with_symmetry() {
    let s = strokes();
    let mut l = baked(&s, Some(0));
    let mut s2 = s.clone();
    let old = s2[2].clone();
    s2[2].radius = 4.0;
    l.rebake(&s2, 2, &[old, s2[2].clone()]);
    assert_same(&l, &baked(&s2, Some(0)), "symmetric edit");
}

#[test]
fn symmetric_strokes_mirror_the_field() {
    // strokes away from the plane: the field is mirror symmetric (to rounding:
    // trilinear reads floor on the other side of zero)
    let s = vec![
        Stroke::new(Mode::Add, 2.0, arc([5.0, 6.0, 6.0], [8.0, 2.0, 5.0], 1.0, 5, |_| 1.0)).with_blend(0.8),
        Stroke::new(Mode::Grab, 3.0, vec![[8.0, 0.0, 6.0, 1.0], [9.0, 0.5, 6.5, 1.0]]),
    ];
    let l = baked(&s, Some(0));
    for k in l.keys() {
        for z in 0..BRICK {
            for y in 0..BRICK {
                for x in 0..BRICK {
                    let g = [k[0] * BRICK + x, k[1] * BRICK + y, k[2] * BRICK + z];
                    assert!((l.at(g) - l.at([-g[0], g[1], g[2]])).abs() <= 1e-5, "{g:?}");
                }
            }
        }
    }
}

fn draw_live(l: &mut Layer, template: &Stroke, samples: &[[f32; 4]]) -> (Stroke, usize) {
    let mut rec = Recorder::new(template.radius);
    rec.push([samples[0][0], samples[0][1], samples[0][2]], samples[0][3]);
    let mut live = l.live_begin(Stroke { path: rec.path(), ..template.clone() });
    let mut most = 0;
    for s in &samples[1..] {
        rec.push([s[0], s[1], s[2]], s[3]);
        l.live_update(&mut live, rec.path());
        most = most.max(live.last_touched);
    }
    (l.live_end(live), most)
}

fn dense(path: &[[f32; 4]], per: usize) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    for w in path.windows(2) {
        for i in 0..per {
            let t = i as f32 / per as f32;
            let mut q = [0.0; 4];
            for c in 0..4 {
                q[c] = mix(w[0][c], w[1][c], t) + 0.013 * ((i * 7 + c) as f32).sin();
            }
            q[3] = q[3].clamp(0.0, 1.0);
            out.push(q);
        }
    }
    out.push(*path.last().unwrap());
    out
}

#[test]
fn live_strokes_equal_the_committed_bake() {
    for mirror in [None, Some(0)] {
        let s = strokes();
        for (i, t) in s.iter().enumerate() {
            let prior = &s[..i];
            let mut l = baked(prior, mirror);
            let samples = dense(&t.path, 12);
            let (done, most) = draw_live(&mut l, t, &samples);
            let mut all = prior.to_vec();
            all.push(done.clone());
            assert_same(&l, &baked(&all, mirror), &format!("live {} mirror {mirror:?}", t.mode.name()));
            assert!(done.path.len() < samples.len(), "{}: {} of {} samples kept", t.mode.name(), done.path.len(), samples.len());
            let _ = most;
        }
    }
}

#[test]
fn live_updates_stay_bounded_and_cancel_restores() {
    // a long stroke: each update re-bakes a few bricks, not the stroke
    let base_layer = baked(&[], None);
    let mut l = baked(&[], None);
    let long = Stroke::new(Mode::Add, 1.5, arc([-40.0, 0.0, 12.0], [40.0, 0.0, 12.0], 2.0, 60, |_| 1.0)).with_blend(0.5);
    let samples = dense(&long.path, 6);
    let (done, most) = draw_live(&mut l, &long, &samples);
    let whole = keys_in(&done.bounds(BAND), CELL).len();
    assert!(most * 4 < whole, "an update touched {most} of the stroke's {whole} bricks");

    let mut l2 = baked(&[], None);
    let live = l2.live_begin(done.clone());
    l2.live_cancel(live);
    assert_same(&l2, &base_layer, "cancel");
}

#[test]
fn recorder_quantises_and_decimates() {
    assert_eq!(quantise(1.234_567, 2), "1.23".parse::<f64>().unwrap() as f32);
    let mut rec = Recorder::new(2.0);
    let mut raw = Vec::new();
    for i in 0..400 {
        let t = i as f32 / 399.0;
        let p = [20.0 * t, 3.0 * (t * 6.0).sin(), 0.0];
        raw.push(p);
        rec.push(p, 1.0);
    }
    let path = rec.path();
    assert!(path.len() < 60, "{} points kept", path.len());
    // every raw sample is within tol of the polyline (plus quantisation)
    for p in raw {
        let mut best = f32::MAX;
        for w in path.windows(2) {
            let (a, b) = ([w[0][0], w[0][1], w[0][2]], [w[1][0], w[1][1], w[1][2]]);
            let ab = sub(b, a);
            let t = (dot(sub(p, a), ab) / dot(ab, ab)).clamp(0.0, 1.0);
            best = best.min(len(sub(p, add(a, scale(ab, t)))));
        }
        assert!(best <= 0.05 * 2.0 + 0.01, "{best}");
    }
}

#[test]
fn materials_follow_the_strokes() {
    let s = strokes();
    let l = baked(&s, None);
    // the first add's tube is material 2, the base around it 0 (the program
    // gives the box material 1)
    let c = s[0].path[4];
    assert_eq!(l.material([c[0], c[1], c[2]]), 2);
    assert_eq!(l.material([0.0, -9.0, 0.0]), 1);
    // paint changed the material under the brush and nothing else
    let painted = s.iter().find(|t| t.mode == Mode::Paint).unwrap();
    let c = painted.path[2];
    let q = [c[0], c[1], c[2]];
    assert_eq!(l.material(q), 4);
    let i = s.iter().position(|t| t.mode == Mode::Paint).unwrap();
    let before = baked(&s[..i], None);
    let g = [(q[0] / CELL).round() as i32, (q[1] / CELL).round() as i32, (q[2] / CELL).round() as i32];
    assert_eq!(l.at(g).to_bits(), before.at(g).to_bits(), "paint moved the surface");
    // edits keep their materials through culling
    let prog = body_with_materials();
    let b = prog.bounds().expand(2.0);
    let mut r = Rng(9);
    for _ in 0..20000 {
        let p = r.point(&b);
        assert_eq!(prog.eval_banded_material(p, 1.0), prog.eval_culled_material(p, 1.0), "{p:?}");
    }
}

fn body_with_materials() -> Program {
    let mut prog = body();
    for (i, e) in prog.edits.iter_mut().enumerate() {
        *e = e.clone().with_material(i as u8 + 1);
    }
    prog
}

fn part(prog: Program, bone: u16, blend: f32) -> Part {
    let bounds = prog.bounds();
    Part { bone, blend, bounds, field: Arc::new(move |p| prog.eval_material(p)) }
}

fn rig() -> Object {
    let torso = Program::new(vec![Edit::shape(Fold::Union, 0.0, Shape::Ellipsoid { radii: [6.0, 9.0, 5.0] }, vec![]).with_material(1)]);
    let arm = Program::new(vec![Edit::shape(Fold::Union, 0.0, Shape::Tube { pts: vec![[5.0, 6.0, 0.0, 2.0], [16.0, 6.0, 0.0, 1.4]] }, vec![]).with_material(2)]);
    let head = Program::new(vec![Edit::shape(Fold::Union, 0.0, Shape::Sphere { r: 4.0 }, vec![Warp::Translate([0.0, 12.0, 0.0])]).with_material(3)]);
    Object { parts: vec![part(torso, 0, 0.0), part(arm, 1, 2.0), part(head, 2, 1.5)] }
}

#[test]
fn bone_groups_pose_and_instance() {
    let obj = rig();
    let rest = [Xform::IDENT; 3];
    let band = 2.0;
    // rest pose = the parts' smooth union in the bind frame
    let p = [10.0, 6.0, 0.0];
    let (d, m) = obj.eval(&rest, &Xform::IDENT, p, band);
    assert!(d < 0.0 && m == 2);
    // raise the arm 90 degrees about the shoulder: the old hand point is
    // outside, the raised one inside, and it stays the arm's material
    let shoulder = [5.0, 6.0, 0.0];
    let up = Xform::rot_z(std::f32::consts::FRAC_PI_2, shoulder).then(&Xform { at: scale(shoulder, -1.0), ..Xform::IDENT });
    let pose = [Xform::IDENT, up, Xform::IDENT];
    assert!(obj.eval(&pose, &Xform::IDENT, [14.0, 6.0, 0.0], band).0 > 0.0);
    let raised = obj.eval(&pose, &Xform::IDENT, [5.0, 15.0, 0.0], band);
    assert!(raised.0 < 0.0 && raised.1 == 2, "{raised:?}");
    // an instance moves and scales the whole posed object exactly
    let inst = Xform { rot: Xform::rot_z(0.4, [0.0; 3]).rot, at: [30.0, 2.0, -5.0], scale: 1.5 };
    let mut r = Rng(11);
    let region = Aabb::new([-20.0; 3], [25.0; 3]);
    for _ in 0..5000 {
        let x = r.point(&region);
        let (dl, ml) = obj.eval(&pose, &Xform::IDENT, x, band / 1.5);
        let (dw, mw) = obj.eval(&pose, &inst, inst.to_world(x), band);
        assert!((dw - dl * 1.5).abs() < 1e-3, "{x:?}: {dw} vs {}", dl * 1.5);
        if (dw - dl * 1.5).abs() < 1e-6 {
            assert_eq!(mw, ml);
        }
        // culling the posed parts is exact
        let w = inst.to_world(x);
        assert_eq!(obj.eval(&pose, &inst, w, band), obj.eval_culled(&pose, &inst, w, band), "{w:?}");
    }
}

#[test]
fn material_table_drives_the_look_without_rebaking() {
    let s = strokes();
    let l = baked(&s, None);
    let mut table = MaterialTable::default();
    table.set(2, Material { color: [1.0, 0.1, 0.1], metal: 0.9, rough: 0.2, emissive: 2.0, ..Material::default() });
    let c = s[0].path[4];
    let q = [c[0], c[1], c[2]];
    // deep in the material-2 tube every corner is slot 2
    let inside = l.surface(q, &table);
    assert!((inside.metal - 0.9).abs() < 1e-6 && (inside.emissive - 2.0).abs() < 1e-6, "{inside:?}");
    // a slider move: a table edit, the same bricks, the new look
    let keys = l.keys();
    table.set(2, Material { rough: 0.7, unlit: true, ..table.get(2) });
    assert_eq!(l.keys(), keys);
    let after = l.surface(q, &table);
    assert!((after.rough - 0.7).abs() < 1e-6);
    assert!(after.unlit);
    // a seam blends over one cell: halfway between a slot-2 and a slot-0 corner
    let half = blend(&table, [2, 0, 2, 0, 2, 0, 2, 0], [0.5, 0.0, 0.0]);
    assert!((half.metal - 0.45).abs() < 1e-6);
    // the upload layout
    assert_eq!(table.texels().len(), 2 * table.slots.len());
    assert_eq!(table.get(2).texels()[1], [0.9, 0.7, 2.0, 1.0]);
}

#[test]
fn kernel_parameter_blocks_round_trip() {
    for st in strokes() {
        let w = crate::kernel::pack(&st, CELL, BAND);
        let (back, cell, band) = crate::kernel::unpack(&w);
        assert_eq!(back, st);
        assert_eq!((cell, band), (CELL, BAND));
        assert_eq!(w[12] as u32, crate::kernel::passes(&st, CELL));
    }
}

#[test]
fn chamfer_and_paint_edits() {
    for &(a, k) in &[(0.3f32, 1.0f32), (-2.0, 0.5)] {
        assert_eq!(cmin(a, a + 1.5 * k, k).to_bits(), a.to_bits());
        assert_eq!(cmin(a, a + 2.0 * k, k).to_bits(), a.to_bits());
        assert!(cmin(a, a, k) < a);
    }
    // two perpendicular planes: the bevel x + y = k
    let k = 2.0;
    assert!((cmin(0.5, 0.5, k) - (0.5 + 0.5 - k) * 0.5).abs() < 1e-6);
    // paint: same distance, new material inside the paint shape only
    let prog = body_with_materials();
    let i = prog.edits.iter().position(|e| matches!(e, Edit::Shape { fold: Fold::Paint, .. })).unwrap();
    let before = Program::new(prog.edits[..i].to_vec());
    let upto = Program::new(prog.edits[..=i].to_vec());
    let inside = [7.0, 2.0, 0.0];
    assert_eq!(before.eval(inside).to_bits(), upto.eval(inside).to_bits());
    assert_eq!(upto.eval_material(inside).1, i as u8 + 1);
    assert_ne!(upto.eval_material([0.0, -6.0, 0.0]).1, i as u8 + 1);
}

#[test]
fn sculpt_spec_key_and_chamfered_strokes() {
    let spec = SculptSpec { cell: CELL, band: BAND, mirror: Some(0), strokes: strokes() };
    let mut other = spec.clone();
    assert_eq!(spec.key(), other.key());
    other.strokes[3].radius += 0.01;
    assert_ne!(spec.key(), other.key());
    let mut ch = spec.clone();
    ch.strokes[0].chamfer = true;
    assert_ne!(spec.key(), ch.key());
    assert_same(&spec.bake(base()), &baked(&spec.strokes, Some(0)), "spec bake");
    // a chamfered add differs from the round one only inside its box
    let a = ch.bake(base());
    let b = spec.bake(base());
    let bx = spec.strokes[0].bounds(BAND);
    let mut r = Rng(21);
    for _ in 0..5000 {
        let p = r.point(&bx.expand(6.0));
        let g = [(p[0] / CELL).round() as i32, (p[1] / CELL).round() as i32, (p[2] / CELL).round() as i32];
        let q = [g[0] as f32 * CELL, g[1] as f32 * CELL, g[2] as f32 * CELL];
        if !bx.contains(q) && !spec.strokes.iter().skip(1).any(|s| s.bounds(BAND).expand(4.0).contains(q)) {
            assert_eq!(a.at(g).to_bits(), b.at(g).to_bits(), "{q:?}");
        }
    }
}

fn brush_settings(mode: Mode) -> BrushSettings {
    BrushSettings { mode, radius: 2.0, blend: 0.5, strength: 0.8, material: 3, chamfer: true, bone: 7 }
}

#[test]
fn brush_stroke_equals_the_committed_bake() {
    let settings = brush_settings(Mode::Add);
    let mut brush = Brush::new(settings);
    let mut l = Layer::new(base(), CELL, BAND, None);
    let samples = arc([-6.0, 7.0, 5.0], [6.0, 7.0, 5.0], 3.5, 40, |t| 0.4 + 0.6 * t);
    let mut input = BrushInput { tip: [0.0; 3], pressure: 1.0, active: true, invert: false, smooth: false };
    assert!(!brush.is_drawing());
    for (i, p) in samples.iter().enumerate() {
        input.tip = [p[0], p[1], p[2]];
        input.pressure = p[3];
        let events: Vec<_> = brush.update(&mut l, &input).into_iter().filter(|e| !matches!(e, BrushEvent::Haptic { .. })).collect();
        if i == 0 {
            assert!(matches!(events.as_slice(), [BrushEvent::Began, BrushEvent::Updated { bricks }] if *bricks > 0));
        } else {
            assert!(matches!(events.as_slice(), [BrushEvent::Updated { bricks }] if *bricks > 0));
        }
        assert!(brush.is_drawing());
    }
    input.active = false;
    let done = brush.update(&mut l, &input).into_iter().find_map(|e| match e {
        BrushEvent::Committed(s) => Some(s),
        _ => None,
    }).unwrap();
    assert!(!brush.is_drawing());
    assert_eq!(done.mode, settings.mode);
    assert_eq!((done.radius, done.blend, done.strength), (settings.radius, settings.blend, settings.strength));
    assert_eq!((done.material, done.chamfer, done.bone), (settings.material, settings.chamfer, settings.bone));
    assert!(done.path.len() < samples.len(), "{} of {} points kept", done.path.len(), samples.len());
    assert_ne!(done.path.first().unwrap()[3], done.path.last().unwrap()[3]);
    assert_same(&l, &baked(&[done], None), "brush commit");
    assert!(brush.update(&mut l, &input).is_empty());
}

#[test]
fn brush_modifiers_latch_the_stroke_mode() {
    for (smooth, mode) in [(false, Mode::Carve), (true, Mode::Smooth)] {
        let mut brush = Brush::new(brush_settings(Mode::Add));
        let mut l = baked(&[], None);
        let mut input = BrushInput { tip: [0.0, 10.0, 0.0], pressure: 1.0, active: true, invert: true, smooth };
        assert_eq!(brush.effective_mode(&input), mode);
        brush.update(&mut l, &input);
        // Settings and modifiers changed during the stroke apply next time.
        brush.settings.mode = Mode::Paint;
        brush.settings.radius = 0.5;
        input.invert = false;
        input.smooth = !smooth;
        input.tip = [0.0, 0.0, 10.0];
        brush.update(&mut l, &input);
        input.active = false;
        let done = brush.update(&mut l, &input).into_iter().find_map(|e| match e {
            BrushEvent::Committed(s) => Some(s),
            _ => None,
        }).unwrap();
        assert_eq!(done.mode, mode);
        assert_eq!(done.radius, 2.0);
        let depth = if mode == Mode::Carve { 9.4 } else { 10.0 };
        assert_eq!(done.path[0], [0.0, depth, 0.0, 1.0]);
        assert_eq!(*done.path.last().unwrap(), [0.0, 0.0, depth, 1.0]);
        assert_same(&l, &baked(&[done], None), "brush modifiers");
    }
    let mut brush = Brush::new(brush_settings(Mode::Carve));
    let input = BrushInput { tip: [0.0, 12.0, 0.0], pressure: 1.0, active: false, invert: true, smooth: false };
    assert_eq!(brush.effective_mode(&input), Mode::Add);
    for mode in [Mode::Smooth, Mode::Inflate, Mode::Flatten, Mode::Pinch, Mode::Grab, Mode::Paint] {
        brush.settings.mode = mode;
        assert_eq!(brush.effective_mode(&input), mode);
    }
}

#[test]
fn brush_haptic_on_surface_crossing() {
    let mut brush = Brush::new(brush_settings(Mode::Add));
    let mut l = baked(&[], None);
    let mut input = BrushInput { tip: [0.0, 12.0, 0.0], pressure: 1.0, active: false, invert: false, smooth: false };
    assert!(brush.update(&mut l, &input).is_empty());
    assert!(brush.update(&mut l, &input).is_empty());
    input.tip = [0.0, 8.0, 0.0];
    assert_eq!(brush.update(&mut l, &input), vec![BrushEvent::Haptic { strength: 0.5 }]);
    assert!(brush.update(&mut l, &input).is_empty());
    assert!(!brush.is_drawing());
}

#[test]
fn brush_carve_haptic_every_four_updates() {
    let mut brush = Brush::new(BrushSettings { radius: 0.5, blend: 0.0, ..brush_settings(Mode::Carve) });
    let mut l = baked(&[], None);
    let mut input = BrushInput { tip: [0.0; 3], pressure: 1.0, active: true, invert: false, smooth: false };
    // Each tip is inside, away from all the earlier cuts.
    for i in 0..8 {
        input.tip = [if i & 1 == 0 { -4.0 } else { 4.0 }, if i & 2 == 0 { -4.0 } else { 4.0 }, if i & 4 == 0 { -4.0 } else { 4.0 }];
        assert!(l.sample(input.tip) < 0.0);
        let haptics: Vec<_> = brush.update(&mut l, &input).into_iter().filter(|e| matches!(e, BrushEvent::Haptic { .. })).collect();
        if (i + 1) % 4 == 0 {
            assert_eq!(haptics, vec![BrushEvent::Haptic { strength: 0.1 }]);
        } else {
            assert!(haptics.is_empty());
        }
    }
    brush.cancel(&mut l);
}

#[test]
fn brush_flatten_plane_follows_the_surface_normal() {
    let mut brush = Brush::new(brush_settings(Mode::Flatten));
    let mut l = baked(&[], None);
    let tip = [3.0, 8.0, 27.0f32.sqrt()];
    let mut input = BrushInput { tip, pressure: 1.0, active: true, invert: false, smooth: false };
    brush.update(&mut l, &input);
    input.tip = [0.0, 0.0, 10.0];
    brush.update(&mut l, &input);
    input.active = false;
    let done = brush.update(&mut l, &input).into_iter().find_map(|e| match e {
        BrushEvent::Committed(s) => Some(s),
        _ => None,
    }).unwrap();
    let n = [done.plane[0], done.plane[1], done.plane[2]];
    assert!(len(sub(n, normalize(tip))) < 0.01, "{n:?}");
    assert!((dot(n, tip) - done.plane[3]).abs() < 0.01);
    for v in done.plane {
        assert_eq!(v, quantise(v, 3));
    }
    assert_same(&l, &baked(&[done], None), "brush flatten");
}

#[test]
fn brush_cancel_restores_the_layer() {
    let mut brush = Brush::new(brush_settings(Mode::Add));
    let mut l = baked(&[], None);
    let mut input = BrushInput { tip: [0.0, 10.0, 0.0], pressure: 1.0, active: true, invert: false, smooth: false };
    brush.update(&mut l, &input);
    input.tip = [1.0, 10.0, 0.0];
    brush.update(&mut l, &input);
    assert!(brush.is_drawing());
    assert!(l.sample(input.tip) < -1.0);
    brush.cancel(&mut l);
    assert!(!brush.is_drawing());
    assert_same(&l, &baked(&[], None), "brush cancel");
    brush.cancel(&mut l);
    input.active = false;
    assert!(brush.update(&mut l, &input).iter().all(|e| matches!(e, BrushEvent::Haptic { .. })));
    input.active = true;
    assert!(brush.update(&mut l, &input).iter().any(|e| matches!(e, BrushEvent::Began)));
    brush.cancel(&mut l);
    assert_same(&l, &baked(&[], None), "brush restarted cancel");
}
