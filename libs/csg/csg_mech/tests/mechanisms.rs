// The two reference mechanisms end to end on the real pipeline: parts as csg_graph solids, joints planned
// for a 0.4 / 0.2 PLA profile, solids lowered and meshed (exact cylinders), the checks on the meshes'
// fields, and the box3d sim. Model frame Y up (print up), millimetres.
//
// Run with --nocapture for the numbers.

use makepad_csg_graph::eval::Evaluator;
use makepad_csg_graph::{Graph, NodeId, Op};
use makepad_csg_mech::sim::{simulate, Scenario, SimOptions};
use makepad_csg_mech::*;
use makepad_csg_sdf::mesh_field::{MeshData, MeshField};
use makepad_csg_sdf::Sdf3;

fn boxed(g: &mut Graph, min: P3, max: P3) -> NodeId {
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let b = g.push(Op::Box { size, radius: 0.0 }).unwrap();
    g.push(Op::Transform { source: b, at: [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, (min[2] + max[2]) * 0.5], rotate: [0.0; 3], scale: [1.0; 3] }).unwrap()
}

struct Built {
    names: Vec<String>,
    verts: Vec<Vec<P3>>,
    tris: Vec<Vec<[u32; 3]>>,
    /// The original parts' fields (before the joints).
    originals: Vec<MeshField>,
    /// Per final part: its mesh is closed.
    watertight: Vec<bool>,
}

fn mesh_field(m: &makepad_csg_mesh::TriMesh) -> MeshField {
    MeshField::new(MeshData { positions: m.vertices.iter().map(|p| [p.x as f32, p.y as f32, p.z as f32]).collect(), triangles: m.triangles.clone() })
}

fn build(mut g: Graph, nodes: &[NodeId], names: &[&str], plan: &Plan) -> Built {
    let finals: Vec<NodeId> = plan.solids.iter().map(|s| s.lower(&mut g, nodes, 96).unwrap()).collect();
    let mut ev = Evaluator::new();
    ev.begin();
    let mut b = Built { names: names.iter().map(|s| s.to_string()).collect(), verts: Vec::new(), tris: Vec::new(), originals: Vec::new(), watertight: Vec::new() };
    for n in nodes {
        b.originals.push(mesh_field(&ev.mesh(&g, *n, None).map_err(|e| e.message).unwrap()));
    }
    for (i, f) in finals.into_iter().enumerate() {
        let m = ev.mesh(&g, f, None).map_err(|e| e.message).unwrap();
        let mf = mesh_field(&m);
        println!("  mesh {}: {} triangles, watertight {}", names[i], m.triangles.len(), mf.watertight);
        b.watertight.push(mf.watertight);
        b.verts.push(m.vertices.iter().map(|p| [p.x, p.y, p.z]).collect());
        b.tris.push(m.triangles.clone());
    }
    b
}

fn fields<'a>(b: &'a Built, plan: &'a Plan) -> Vec<SolidField<'a>> {
    plan.solids.iter().map(|s| SolidField { solid: s, parts: b.originals.iter().map(|f| f as &(dyn makepad_csg_sdf::Sdf3 + Sync)).collect() }).collect()
}

fn finals<'a>(b: &'a Built, f: &'a [SolidField<'a>]) -> Vec<FinalPart<'a>> {
    (0..b.names.len()).map(|i| FinalPart { name: &b.names[i], field: &f[i], vertices: &b.verts[i], triangles: &b.tris[i] }).collect()
}

fn report_checks(checks: &[JointCheck]) {
    for c in checks {
        // a slider's ranges are millimetres, the others' angles
        let (u, unit): (fn(f64) -> f64, &str) = if c.kind == "slider" { (|x| x, "mm") } else { (f64::to_degrees, "deg") };
        println!(
            "  {} {}-{}: gap {:.9} (built {:.2}) | layer gap {:.2} | tack n0 {:.1} n1 {:.1} n2 {:.1} n3 {:.1} | risk {} ({:.1}) | field limits -{:.9}/+{:.9} {unit}, effective {:.1}..{:.1} | retention {:?} | {:.0} ms (gap {:.0} layers {:.0} limits {:.0})",
            c.kind,
            c.a,
            c.b,
            c.gap.gap,
            c.clearance,
            c.layers.layer_gap,
            c.layers.tack[0],
            c.layers.tack[1],
            c.layers.tack[2],
            c.layers.tack[3],
            c.risk.name(),
            c.risk_score,
            u(c.field.0),
            u(c.field.1),
            u(c.effective.0),
            u(c.effective.1),
            c.retention,
            c.millis,
            c.phases.0,
            c.phases.1,
            c.phases.2
        );
    }
}

#[test]
fn chain_of_three_hangs_and_articulates() {
    let profile = Profile::default();
    let mut g = Graph::new();
    let n = 3;
    let pitch = 12.0;
    let nodes: Vec<NodeId> = (0..n).map(|i| boxed(&mut g, [i as f64 * pitch, 0.0, -2.5], [(i + 1) as f64 * pitch, 6.0, 2.5])).collect();
    let boxes: Vec<PartBox> = (0..n).map(|i| PartBox { min: [i as f64 * pitch, 0.0, -2.5], max: [(i + 1) as f64 * pitch, 6.0, 2.5] }).collect();
    let decls: Vec<JointDecl> = (0..n - 1)
        .map(|i| JointDecl::Hinge(HingeDecl { a: i, b: i + 1, at: [(i + 1) as f64 * pitch, 0.0, 0.0], axis: [0.0, 1.0, 0.0], limits: None, pin: None, plate: None, clearance: None, clearance_axial: None, brake: None }))
        .collect();
    let anchors = vec![AnchorDecl { part: 0, at: [2.5, 3.0, 0.0], hole: Some(([0.0, 1.0, 0.0], 1.2)) }];
    let plan = plan(&boxes, &decls, &[], &anchors, &profile).unwrap();
    let names: Vec<String> = (0..n).map(|i| format!("link[{i}]")).collect();
    let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let built = build(g, &nodes, &name_refs, &plan);
    let fs = fields(&built, &plan);
    let parts = finals(&built, &fs);
    let checks = check_joints(&plan, &parts, Geometry::Design, &|| false);
    report_checks(&checks);
    for c in &checks {
        assert!((c.gap.gap - c.clearance).abs() < 0.036, "gap {} vs {}", c.gap.gap, c.clearance);
        assert_eq!(c.risk, Risk::Medium, "the checked chain's fuse verdict");
        // Measured on the original signed-field checker. The advance may
        // change its bracket, but not the limit beyond one angular sample.
        for (actual, before) in [(c.field.0, 99.9140625f64), (c.field.1, 99.75f64)] {
            assert!((actual - before.to_radians()).abs() <= 3f64.to_radians() / 128.0 + 1e-12, "chain sweep: {actual} vs {before} degrees");
        }
        assert!(c.ok());
    }
    let asm = assembly(&plan, &parts, &checks, 1.0);
    for scenario in [Scenario::Hang, Scenario::Shake] {
        let r = simulate(&asm, &SimOptions { scenario, seconds: 20.0, ..Default::default() }, &|| false).unwrap();
        assert!(r.articulates, "{} binding {:?}", r.scenario, r.binding);
        if r.scenario == "hang" {
            assert!(r.hangs_right);
        }
    }
    // deterministic
    let a = simulate(&asm, &SimOptions { scenario: Scenario::Shake, seconds: 3.0, ..Default::default() }, &|| false).unwrap();
    let b = simulate(&asm, &SimOptions { scenario: Scenario::Shake, seconds: 3.0, ..Default::default() }, &|| false).unwrap();
    assert_eq!(a.final_pose, b.final_pose, "the same run twice gives the same pose");
}

#[test]
fn a_ball_shoulder_printed_on_the_bed_turns() {
    let profile = Profile::default();
    let mut g = Graph::new();
    // Model frame (Y up): a torso lying on the bed, an upper arm out along +X from a ball shoulder
    // inside its flank, a forearm on an elbow hinge. The ball sits 1/sqrt(2) of its radius over the
    // bed: it prints flat on the bed in a socket open underneath, its underside no steeper than 45
    // degrees. Small, as the ball's field check costs seconds and falls with the ball's size.
    let r = 3.5;
    let centre = [6.5, r / 2f64.sqrt(), 0.0];
    let torso_box = PartBox { min: [-8.0, 0.0, -9.0], max: [8.0, 10.0, 9.0] };
    let upper_box = PartBox { min: [8.5, 0.0, -3.0], max: [22.0, 7.0, 3.0] };
    let fore_box = PartBox { min: [22.0, 0.0, -3.0], max: [34.0, 7.0, 3.0] };
    let nodes = vec![boxed(&mut g, torso_box.min, torso_box.max), boxed(&mut g, upper_box.min, upper_box.max), boxed(&mut g, fore_box.min, fore_box.max)];
    let ball = |at: P3| JointDecl::Ball(BallDecl { a: 0, b: 1, at, radius: Some(r), neck: None, axis: Some([1.0, 0.0, 0.0]), cone: 30f64.to_radians(), twist: None, clearance: None, wall: None });
    let elbow = JointDecl::Hinge(HingeDecl { a: 1, b: 2, at: [22.0, 0.0, 0.0], axis: [0.0, 1.0, 0.0], limits: Some((-110f64.to_radians(), 110f64.to_radians())), pin: None, plate: None, clearance: None, clearance_axial: None, brake: None });
    let anchors = vec![AnchorDecl { part: 0, at: [0.0, 5.0, 0.0], hole: None }];
    let boxes = [torso_box, upper_box, fore_box];
    // Over a socket floor the ball would print in mid-air: refused, saying where it prints.
    let floating = plan(&boxes, &[ball([6.5, 5.0, 0.0]), elbow.clone()], &[], &anchors, &profile).unwrap_err();
    assert!(floating.joint == 0 && floating.message.contains("mid-air") && floating.message.contains("above the bed"), "{floating:?}");
    // Too low, its socket's opening on the bed lets it out.
    let low = plan(&boxes, &[ball([6.5, 1.0, 0.0]), elbow.clone()], &[], &anchors, &profile).unwrap_err();
    assert!(low.message.contains("too low"), "{low:?}");
    let plan = plan(&boxes, &[ball(centre), elbow], &[], &anchors, &profile).unwrap();
    for (i, n) in plan.notes.iter().enumerate() {
        for x in n {
            println!("  note joint {i}: {}", x.text);
        }
    }
    assert!(matches!(plan.joints[0], JointGeom::Ball { seat: Seat::Bed { ground }, .. } if ground.abs() < 1e-9), "{:?}", plan.joints[0]);
    let built = build(g, &nodes, &["torso", "upper_arm", "forearm"], &plan);
    assert_eq!(built.watertight, vec![true; 3]);
    let fs = fields(&built, &plan);
    let parts = finals(&built, &fs);
    // The ball's flat is on the bed; nothing of the socket is under it, and the socket comes down
    // to the bed round it.
    let ball_low = built.verts[1].iter().filter(|v| (v[0] - centre[0]).hypot(v[2]) < r).map(|v| v[1]).fold(f64::INFINITY, f64::min);
    assert!(ball_low.abs() < 1e-6, "the ball's lowest point {ball_low}");
    let probe = |x: f64, y: f64, z: f64| makepad_csg_math::Vec3d { x, y, z };
    for y in [0.05, 0.3, 1.0] {
        assert!(fs[1].distance(probe(centre[0], y, 0.0)) < 0.0, "the ball at {y}");
        assert!(fs[0].distance(probe(centre[0], y, 0.0)) > 0.25, "no socket floor under the ball at {y}");
    }
    let lip = (r + plan.joints[0].clearance()) + 0.6;
    assert!(fs[0].distance(probe(centre[0] - lip, 0.05, 0.0)) < 0.0, "the socket stands on the bed round the ball");
    let checks = check_joints(&plan, &parts, Geometry::Design, &|| false);
    report_checks(&checks);
    let shoulder = &checks[0];
    assert_eq!(shoulder.kind, "ball");
    assert!((shoulder.gap.gap - shoulder.clearance).abs() < 0.036, "ball gap {} vs {}", shoulder.gap.gap, shoulder.clearance);
    assert!(shoulder.retention.is_some_and(|k| k >= 0.3), "held by {:?}", shoulder.retention);
    assert!(shoulder.effective.1 > 20f64.to_radians(), "the ball turns {:.1} degrees before anything touches", shoulder.effective.1.to_degrees());
    for c in &checks {
        assert!(c.ok(), "{} binds: {:?}", c.kind, c.binds);
    }
    let asm = assembly(&plan, &parts, &checks, 1.0);
    for scenario in [Scenario::Shake, Scenario::Pose(vec![20f64.to_radians(), 60f64.to_radians()])] {
        let r = simulate(&asm, &SimOptions { scenario, seconds: 8.0, ..Default::default() }, &|| false).unwrap();
        println!("  sim {}: articulates {} binding {:?} settle {:?} drift {:?} self-contacts {:?} | {:.0} ms", r.scenario, r.articulates, r.binding, r.settle, r.drift.map(f64::to_degrees), r.self_contacts, r.wall_ms);
        for j in &r.joints {
            println!("    {} range {:.1}..{:.1} last {:.1} deg, limits {:.1}..{:.1}, hits {}", j.name, j.min.to_degrees(), j.max.to_degrees(), j.last.to_degrees(), j.limits.0.to_degrees(), j.limits.1.to_degrees(), j.limit_hits);
        }
        if r.scenario == "shake" {
            assert!(r.articulates, "{:?}", r.binding);
            assert!(r.joints[0].moved, "the ball turns in the shake: {:?}", r.joints[0]);
        }
    }
}

/// Hold part 0 fixed and pull part 1 by a point of it to `to` (model frame) over a second
/// under real gravity: the joint's travel (or angle) where it ends up.
fn pulled(asm: &makepad_csg_mech::sim::Assembly, from: P3, to: &dyn Fn(f64) -> P3) -> f64 {
    use makepad_csg_mech::sim::{MechWorld, GRAVITY};
    let mut w = MechWorld::new(asm, [0.0, -GRAVITY, 0.0], None);
    w.grab(1, from, makepad_csg_mech::runner::POINTER_HZ);
    let dt = 1.0 / 240.0;
    for k in 0..360 {
        w.drag(to(((k + 1) as f64 / 240.0).min(1.0)), dt);
        w.step(dt);
    }
    w.angles(asm)[0]
}

/// A hinge holds its limits within the play its clearance gives at the moving part's far end: hung
/// from its fixed link, the other swings to its limit and rests there; pulled 35 degrees past it by a
/// firm grab, it stops there. A heavy part on a light one on a heavy one (a head on its collar on a
/// body), hung, swings into both limits and holds each.
#[test]
fn a_hinge_holds_its_limits_hung_and_pulled() {
    use makepad_csg_mech::sim::{MechWorld, GRAVITY};
    let profile = Profile::default();
    let dt = 1.0 / 240.0;
    // (part spans along X and widths), the hinges' limits: built, with each hinge's play
    let jointed = |spans: &[(f64, f64, f64)], limits: &[f64]| {
        let mut g = Graph::new();
        let nodes: Vec<NodeId> = spans.iter().map(|&(a, b, w)| boxed(&mut g, [a, 0.0, -w / 2.0], [b, 6.0, w / 2.0])).collect();
        let boxes: Vec<PartBox> = spans.iter().map(|&(a, b, w)| PartBox { min: [a, 0.0, -w / 2.0], max: [b, 6.0, w / 2.0] }).collect();
        let decls: Vec<JointDecl> = limits.iter().enumerate().map(|(i, l)| JointDecl::Hinge(HingeDecl { a: i, b: i + 1, at: [spans[i].1, 0.0, 0.0], axis: [0.0, 1.0, 0.0], limits: Some((-l.to_radians(), l.to_radians())), pin: None, plate: None, clearance: None, clearance_axial: None, brake: None })).collect();
        let anchors = vec![AnchorDecl { part: 0, at: [6.0, 3.0, 0.0], hole: None }];
        let p = plan(&boxes, &decls, &[], &anchors, &profile).unwrap();
        let names: Vec<String> = (0..spans.len()).map(|i| format!("part[{i}]")).collect();
        let refs: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
        let built = build(g, &nodes, &refs, &p);
        let play: Vec<f64> = (0..limits.len()).map(|i| {
            let x = spans[i].1;
            let reach = built.verts[i + 1].iter().map(|v| (v[0] - x).hypot(v[2])).fold(0.0, f64::max);
            (p.joints[i].clearance() / reach).atan()
        }).collect();
        let fs = fields(&built, &p);
        let parts = finals(&built, &fs);
        (assembly(&p, &parts, &[], 1.0), play)
    };
    // the most each joint turned, hung across the bed for 3 s
    let hung = |asm: &makepad_csg_mech::sim::Assembly, across: f64| {
        let mut w = MechWorld::new(asm, [0.0, 0.0, across * GRAVITY], None);
        let mut most = vec![0f64; asm.joints.len()];
        for _ in 0..720 {
            w.step(dt);
            for (m, a) in most.iter_mut().zip(w.angles(asm)) {
                *m = m.max(a.abs());
            }
        }
        most
    };
    let (pair, play) = jointed(&[(0.0, 12.0, 5.0), (12.0, 24.0, 5.0)], &[35.0]);
    let limit = 35f64.to_radians();
    for across in [-1.0, 1.0] {
        let most = hung(&pair, across)[0];
        println!("  hung {across:+}: most {:.2} deg, play {:.2}", most.to_degrees(), play[0].to_degrees());
        assert!(most > limit - 0.5f64.to_radians() && most < limit + play[0], "hung, it rests at its limit: {:.2} deg", most.to_degrees());
    }
    for side in [-1.0, 1.0] {
        let mut w = MechWorld::new(&pair, [0.0, -GRAVITY, 0.0], None);
        w.grab(1, [24.0, 3.0, 0.0], makepad_csg_mech::runner::POINTER_HZ);
        let to = 70f64.to_radians();
        for k in 0..480 {
            let a = side * to * ((k + 1) as f64 / 240.0).min(1.0);
            w.drag([12.0 + 12.0 * a.cos(), 3.0, -12.0 * a.sin()], dt);
            w.step(dt);
        }
        let at = w.angles(&pair)[0].abs();
        println!("  pulled {side:+} to 70 deg: at {:.2} deg", at.to_degrees());
        assert!(at > limit - 0.5f64.to_radians() && at < limit + play[0], "pulled past its limit, it stops there: {:.2} deg", at.to_degrees());
    }
    let (chain, play) = jointed(&[(0.0, 24.0, 10.0), (24.0, 29.0, 5.0), (29.0, 47.0, 14.0)], &[35.0, 25.0]);
    let limits = [35f64.to_radians(), 25f64.to_radians()];
    for across in [-1.0, 1.0] {
        let most = hung(&chain, across);
        println!("  chain hung {across:+}: most {:.2}, {:.2} deg, play {:.2}, {:.2}", most[0].to_degrees(), most[1].to_degrees(), play[0].to_degrees(), play[1].to_degrees());
        for k in 0..2 {
            assert!(most[k] < limits[k] + play[k], "joint {k} swung {:.2} deg past its limit, its play {:.2}", (most[k] - limits[k]).to_degrees(), play[k].to_degrees());
        }
    }
}

/// A slider and a bearing print as they stand: every part on the bed, the moving one held in by
/// a chevron ridge in the other's groove (each flank at most 45 degrees, so nothing needs
/// support), and they slide and turn as printed. A slider's carriage stops at the rail's end
/// stops; a bearing turns all the way round. Their checks are free, and in the sim a pull slides
/// the carriage to the end stop and turns the inner a quarter turn.
#[test]
fn a_slider_and_a_bearing_print_as_they_stand_and_move() {
    let profile = Profile::default();
    // A rail along X on the bed and a carriage straddling it, taller.
    let (rail, carriage) = (PartBox { min: [0.0, 0.0, -8.0], max: [60.0, 6.0, 8.0] }, PartBox { min: [20.0, 0.0, -12.0], max: [32.0, 16.0, 12.0] });
    let mut g = Graph::new();
    let nodes = vec![boxed(&mut g, rail.min, rail.max), boxed(&mut g, carriage.min, carriage.max)];
    let decl = |at: P3, axis: P3, travel: Option<(f64, f64)>| JointDecl::Slider(SliderDecl { a: 0, b: 1, at, axis, travel: (travel.map(|t| t.0), travel.map(|t| t.1)), width: None, depth: None, clearance: None });
    let anchors = vec![AnchorDecl { part: 0, at: [30.0, 3.0, 0.0], hole: None }];
    // Across the print's up it does not run; past the end stops it does not slide.
    assert!(plan(&[rail, carriage], &[decl([26.0, 0.0, 0.0], [0.0, 1.0, 0.0], None)], &[], &anchors, &profile).unwrap_err().message.contains("along the bed"));
    assert!(plan(&[rail, carriage], &[decl([26.0, 0.0, 0.0], [1.0, 0.0, 0.0], Some((-30.0, 5.0)))], &[], &anchors, &profile).unwrap_err().message.contains("end stops"));
    // Its underside over the rail climbs at 45 degrees: too low, its sides would hang on nothing.
    let low_carriage = PartBox { max: [32.0, 11.0, 12.0], ..carriage };
    assert!(plan(&[rail, low_carriage], &[decl([26.0, 0.0, 0.0], [1.0, 0.0, 0.0], None)], &[], &anchors, &profile).unwrap_err().message.contains("come apart"));
    let plan_s = plan(&[rail, carriage], &[decl([26.0, 0.0, 0.0], [1.0, 0.0, 0.0], None)], &[], &anchors, &profile).unwrap();
    let JointGeom::Slider(way) = &plan_s.joints[0] else { panic!("{:?}", plan_s.joints[0]) };
    let ch = way.chevron;
    println!("  slider: neck {:.2} depth {:.2} gap {:.2} slope {:.2} travel {:?} retention {:.2} clearance {:.3}", ch.neck, ch.depth, ch.gap, ch.slope(), way.travel, ch.retention(), ch.clearance());
    assert!(ch.slope() <= 1.0 + 1e-9, "flanks no flatter than 45 degrees");
    assert!(ch.retention() >= 0.3, "held in by {:.2}", ch.retention());
    assert!((way.travel.0 + 18.0).abs() < 1e-9 && (way.travel.1 - 26.0).abs() < 1e-9, "to the end stops, 2 mm from either end: {:?}", way.travel);
    let built = build(g, &nodes, &["rail", "carriage"], &plan_s);
    assert_eq!(built.watertight, vec![true; 2]);
    let low = |i: usize| built.verts[i].iter().map(|v| v[1]).fold(f64::INFINITY, f64::min);
    assert!(low(0).abs() < 1e-6 && low(1).abs() < 1e-6, "both on the bed: {} {}", low(0), low(1));
    let fs = fields(&built, &plan_s);
    let probe = |x: f64, y: f64, z: f64| makepad_csg_math::Vec3d { x, y, z };
    let mid = 0.5 * (ch.ground + ch.top);
    // On the bed the carriage's neck stands in the groove, the rail's cheeks beside it; halfway up
    // its ridge reaches past where the cheeks stand on the bed: it cannot lift out.
    assert!(fs[1].distance(probe(26.0, 0.05, ch.neck - 0.1)) < 0.0 && fs[0].distance(probe(26.0, 0.05, ch.neck - 0.1)) > 0.0, "the carriage on the bed");
    assert!(fs[0].distance(probe(26.0, 0.05, ch.neck + ch.gap + 0.1)) < 0.0, "the rail's cheek on the bed");
    assert!(fs[1].distance(probe(26.0, mid, ch.neck + ch.depth - 0.1)) < 0.0, "the ridge");
    assert!(fs[0].distance(probe(26.0, mid, ch.neck + ch.depth - 0.1)) > 0.0 && fs[0].distance(probe(26.0, mid, ch.neck + ch.depth + ch.gap + 0.1)) < 0.0, "the groove round it");
    // The carriage clears the rail's top, by two layers at the slot and climbing at 45 degrees out
    // from its neck, and stands on the bed past the rail's sides; past the travel the groove ends.
    let under = |s: f64| ch.zone_top() + s - ch.neck;
    assert!(fs[1].distance(probe(26.0, 6.0 + 0.5 * ch.c_z, ch.neck + 0.5 * ch.c_xy)) > 0.0, "over the slot");
    assert!(fs[1].distance(probe(26.0, under(5.0) - 0.2, 5.0)) > 0.0 && fs[1].distance(probe(26.0, under(5.0) + 0.2, 5.0)) < 0.0, "over the rail's top");
    assert!(fs[1].distance(probe(26.0, 1.0, 10.0)) < 0.0 && fs[0].distance(probe(26.0, 1.0, 8.0 + 0.5 * ch.c_xy)) > 0.0, "beside the rail");
    assert!(fs[0].distance(probe(1.0, mid, 0.0)) < 0.0 && fs[0].distance(probe(59.0, mid, 0.0)) < 0.0, "the end stops");
    let parts = finals(&built, &fs);
    let checks = check_joints(&plan_s, &parts, Geometry::Design, &|| false);
    report_checks(&checks);
    let c = &checks[0];
    assert_eq!(c.kind, "slider");
    assert!(c.ok(), "the slider binds: {:?}", c.binds);
    assert!((c.gap.gap - c.clearance).abs() < 0.05, "gap {} vs {}", c.gap.gap, c.clearance);
    assert!((c.effective.0 + 18.0).abs() < 0.6 && (c.effective.1 - 26.0).abs() < 0.6, "it slides to its end stops: {:?}", c.effective);
    let asm = assembly(&plan_s, &parts, &checks, 1.0);
    let grip = [26.0, 10.0, 0.0];
    // pulled on 20 mm it slides there; pulled 6 mm past its end stop it stops there, within a
    // print gap
    let free = pulled(&asm, grip, &|t| [26.0 + 20.0 * t, 10.0, 0.0]);
    let stopped = pulled(&asm, grip, &|t| [26.0 + 32.0 * t, 10.0, 0.0]);
    println!("  slider pulled on 20 mm: travels {free:.2}; pulled on 32 mm: {stopped:.2}");
    assert!((free - 20.0).abs() < 1.5, "pulled on, it slides: {free:.2}");
    assert!(stopped > 25.7 && stopped < 26.3, "pulled past its end stop, it stops there: {stopped:.2}");
    assert!(pulled(&asm, grip, &|t| [26.0, 10.0 + 20.0 * t, 0.0]).abs() < 1.0, "pulled up, it stays in the rail");

    // A plate on the bed and a disc on it, taller: a turntable.
    let (outer, inner) = (PartBox { min: [-15.0, 0.0, -15.0], max: [15.0, 6.0, 15.0] }, PartBox { min: [-9.0, 0.0, -9.0], max: [9.0, 10.0, 9.0] });
    let mut g = Graph::new();
    let nodes = vec![boxed(&mut g, outer.min, outer.max), boxed(&mut g, inner.min, inner.max)];
    let decl = JointDecl::Bearing(BearingDecl { a: 0, b: 1, at: [0.0, 0.0, 0.0], radius: None, depth: None, clearance: None });
    let anchors = vec![AnchorDecl { part: 0, at: [0.0, 3.0, 12.0], hole: None }];
    let plan_b = plan(&[outer, inner], &[decl], &[], &anchors, &profile).unwrap();
    let JointGeom::Bearing(race) = &plan_b.joints[0] else { panic!("{:?}", plan_b.joints[0]) };
    let ch = race.chevron;
    println!("  bearing: radius {:.2} depth {:.2} gap {:.2} retention {:.2}", ch.neck, ch.depth, ch.gap, ch.retention());
    assert!(ch.slope() <= 1.0 + 1e-9 && ch.retention() >= 0.3);
    let built = build(g, &nodes, &["outer", "inner"], &plan_b);
    assert_eq!(built.watertight, vec![true; 2]);
    let low = |i: usize| built.verts[i].iter().map(|v| v[1]).fold(f64::INFINITY, f64::min);
    assert!(low(0).abs() < 1e-6 && low(1).abs() < 1e-6, "both on the bed");
    let fs = fields(&built, &plan_b);
    let mid = 0.5 * (ch.ground + ch.top);
    assert!(fs[1].distance(probe(ch.neck - 0.1, 0.05, 0.0)) < 0.0 && fs[0].distance(probe(ch.neck + ch.gap + 0.1, 0.05, 0.0)) < 0.0, "inner and outer on the bed");
    assert!(fs[1].distance(probe(0.0, mid, ch.neck + ch.depth - 0.1)) < 0.0 && fs[0].distance(probe(0.0, mid, ch.neck + ch.depth - 0.1)) > 0.0, "the inner's ridge in the outer's groove");
    let parts = finals(&built, &fs);
    let checks = check_joints(&plan_b, &parts, Geometry::Design, &|| false);
    report_checks(&checks);
    let c = &checks[0];
    assert_eq!(c.kind, "bearing");
    assert!(c.ok(), "the bearing binds: {:?}", c.binds);
    assert!(c.effective.1 - c.effective.0 >= 2.0 * std::f64::consts::PI - 1e-6, "it turns all the way round: {:?}", c.effective);
    let asm = assembly(&plan_b, &parts, &checks, 1.0);
    assert!(asm.joints[0].kind.turns_freely());
    let quarter = std::f64::consts::FRAC_PI_2;
    let turned = pulled(&asm, [7.0, 10.0, 0.0], &|t| [7.0 * (quarter * t).cos(), 10.0, -7.0 * (quarter * t).sin()]);
    println!("  bearing turned a quarter: {:.1} deg", turned.to_degrees());
    assert!((turned.abs() - quarter).abs() < 8f64.to_radians(), "the inner turns a quarter turn as pulled: {:.1} deg", turned.to_degrees());
}

#[test]
fn runner_publishes_poses_and_takes_a_grab() {
    use makepad_csg_mech::runner::{Command, Runner};
    let profile = Profile::default();
    let mut g = Graph::new();
    let nodes: Vec<NodeId> = (0..3).map(|i| boxed(&mut g, [i as f64 * 12.0, 0.0, -2.5], [(i + 1) as f64 * 12.0, 6.0, 2.5])).collect();
    let boxes: Vec<PartBox> = (0..3).map(|i| PartBox { min: [i as f64 * 12.0, 0.0, -2.5], max: [(i + 1) as f64 * 12.0, 6.0, 2.5] }).collect();
    let decls: Vec<JointDecl> = (0..2).map(|i| JointDecl::Hinge(HingeDecl { a: i, b: i + 1, at: [(i + 1) as f64 * 12.0, 0.0, 0.0], axis: [0.0, 1.0, 0.0], limits: None, pin: None, plate: None, clearance: None, clearance_axial: None, brake: None })).collect();
    let anchors = vec![AnchorDecl { part: 0, at: [2.5, 3.0, 0.0], hole: Some(([0.0, 1.0, 0.0], 1.2)) }];
    let plan = plan(&boxes, &decls, &[], &anchors, &profile).unwrap();
    let built = build(g, &nodes, &["l0", "l1", "l2"], &plan);
    let fs = fields(&built, &plan);
    let parts = finals(&built, &fs);
    let asm = assembly(&plan, &parts, &[], 1.0);
    let gravity = makepad_csg_mech::geom::scale(asm.hang_gravity(), makepad_csg_mech::sim::GRAVITY);
    let mut r = Runner::start(asm, gravity);
    let s0 = r.latest();
    assert_eq!(s0.poses.len(), 3);
    r.send(Command::Play);
    // the answer to a command is a snapshot that has taken it up
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !r.answered() {
        assert!(std::time::Instant::now() < until, "the runner did not answer Play");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(r.latest().running, "answered, and not running");
    std::thread::sleep(std::time::Duration::from_millis(400));
    let s1 = r.latest();
    assert!(s1.frame > 40, "frames {}", s1.frame);
    assert!(s1.time > 0.15 && s1.running);
    // the last link has swung away from where it was printed
    let moved = makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(s1.poses[2].com, s0.poses[2].com));
    assert!(moved > 1.0, "moved {moved}");
    // grab the tip and pull it up along the print plane: it follows
    let tip = s1.poses[2].com;
    r.send(Command::Grab { body: 2, point: tip, hz: makepad_csg_mech::runner::POINTER_HZ });
    let target = [tip[0] + 10.0, tip[1], tip[2] + 30.0];
    r.send(Command::Drag { point: target });
    std::thread::sleep(std::time::Duration::from_millis(800));
    let s2 = r.latest();
    assert_eq!(s2.grabbed, Some(2));
    let off = makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(s2.poses[2].com, target));
    println!("runner: {} frames in {:.2} s sim, tip {:.1} mm from the drag target", s2.frame, s2.time, off);
    assert!(off < makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(tip, target)) * 0.5, "tip did not follow: {off}");
    r.send(Command::Release { velocity: None });
    r.send(Command::Pin { body: 2, point: s2.poses[2].com });
    r.send(Command::Pause);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let s = r.latest();
        if !s.running && s.pinned == vec![2] {
            break;
        }
        assert!(std::time::Instant::now() < until, "runner did not publish the pin and pause");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    r.send(Command::Reset);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let s = r.latest();
        if s.running && s.grabbed.is_none() && s.pinned.is_empty() {
            break;
        }
        assert!(std::time::Instant::now() < until, "Reset must clear the pin and resume");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // carried: the anchored link goes where the hand takes it and stays there, on the sim's own time
    let start = r.latest().poses[0].com;
    r.send(Command::Carry { at: [0.0, 0.0, 30.0], axis: [1.0, 0.0, 0.0], angle: 0.0 });
    let began = r.latest().time;
    loop {
        let s = r.latest();
        let by = makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(s.poses[0].com, start));
        if (by - 30.0).abs() < 1.0 {
            break;
        }
        assert!(s.time < began + 2.0, "the carried link is {by} mm from where it was, after {} s", s.time - began);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // shaken where the hand left it, and back there when the shake stops
    let carried = r.latest().poses[0].com;
    let off = |at: makepad_csg_mech::geom::P3| makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(at, carried));
    r.send(Command::Shake(true));
    let began = r.latest().time;
    while off(r.latest().poses[0].com) < 2.0 {
        assert!(r.latest().time < began + 2.0, "a shake doesn't move the carried link");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    r.send(Command::Shake(false));
    let began = r.latest().time;
    loop {
        let s = r.latest();
        if off(s.poses[0].com) <= 1.0 {
            break;
        }
        assert!(s.time < began + 2.0, "the shake left the link {} mm from where the hand did", off(s.poses[0].com));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let _ = r.take_events();
}

/// A two-link hinge (`clearance`: its in-layer and axial clearance, None: the profile's).
fn hinge_pair(clearance: Option<f64>) -> (Plan, Built) {
    let profile = Profile::default();
    let mut g = Graph::new();
    let pitch = 12.0;
    let nodes: Vec<NodeId> = (0..2).map(|i| boxed(&mut g, [i as f64 * pitch, 0.0, -2.5], [(i + 1) as f64 * pitch, 6.0, 2.5])).collect();
    let boxes: Vec<PartBox> = (0..2).map(|i| PartBox { min: [i as f64 * pitch, 0.0, -2.5], max: [(i + 1) as f64 * pitch, 6.0, 2.5] }).collect();
    let decls = vec![JointDecl::Hinge(HingeDecl { a: 0, b: 1, at: [pitch, 0.0, 0.0], axis: [0.0, 1.0, 0.0], limits: None, pin: None, plate: None, clearance, clearance_axial: clearance, brake: None })];
    let plan = plan(&boxes, &decls, &[], &[], &profile).unwrap();
    let built = build(g, &nodes, &["link[0]", "link[1]"], &plan);
    (plan, built)
}

/// A sink that keeps every step, and cancels once it has seen `cancel_after` of them.
struct Recorder {
    seen: std::sync::Mutex<Vec<(usize, Found)>>,
    cancel_after: usize,
}

impl CheckSink for Recorder {
    fn found(&self, joint: usize, found: Found) {
        self.seen.lock().unwrap().push((joint, found));
    }
    fn cancelled(&self) -> bool {
        self.seen.lock().unwrap().len() >= self.cancel_after
    }
}

/// A check's steps arrive in order and reach the end: the gap search's share searched rises to
/// all of it, every layer is sliced in turn, each way of the sweep tests rising angles and
/// stops. What they show is what the check measured (the closest gap and where, each tack, each
/// stop and its contact), and the answers are the same as a check that reports nothing. A
/// cancel stops it within one step, in every phase. A hinge printed with no clearance is not
/// free, and says why.
#[test]
fn a_check_reports_each_step_in_order_and_stops_within_one() {
    let (plan, built) = hinge_pair(None);
    let fs = fields(&built, &plan);
    let parts = finals(&built, &fs);
    let quiet = check_joints(&plan, &parts, Geometry::Design, &|| false);
    let sink = Recorder { seen: Default::default(), cancel_after: usize::MAX };
    let told = check_joints(&plan, &parts, Geometry::Design, &sink);
    let seen = sink.seen.into_inner().unwrap();
    assert_eq!(quiet.len(), 1);
    assert_eq!(told.len(), 1);
    let (q, c) = (&quiet[0], &told[0]);
    let answers = |c: &JointCheck| (c.gap, c.layers.clone(), c.risk, c.field, c.effective, c.turns.clone(), c.binds.clone());
    assert_eq!(answers(q), answers(c), "a check that reports its steps answers the same");
    assert!(c.ok() && c.binds.is_empty(), "the hinge is free: {:?}", c.binds);
    // in order: a few layers ahead, the gap, the layers, then each turn
    let phase = |f: &Found| match f {
        Found::Ahead { .. } | Found::Rows { .. } => 0,
        Found::Gap { .. } => 1,
        Found::Layer { .. } | Found::Tack(_) => 2,
        Found::Turn { turn, .. } | Found::Cells { turn, .. } | Found::Stop(Turned { turn, .. }) => 3 + turn,
    };
    assert!(seen.iter().all(|(j, _)| *j == 0));
    // (a layer's rows come in the phase they slice for)
    let phased: Vec<&Found> = seen.iter().map(|(_, f)| f).filter(|f| !matches!(f, Found::Rows { .. })).collect();
    assert!(phased.windows(2).all(|w| phase(w[0]) <= phase(w[1])), "the phases run in order");
    let mut rows = 0.0;
    for (_, f) in &seen {
        match f {
            Found::Rows { share } => {
                assert!(*share > rows && *share < 1.0, "a layer's rows rise: {share} after {rows}");
                rows = *share;
            }
            _ => rows = 0.0,
        }
    }
    let ahead: Vec<(usize, usize, usize, f64)> = seen.iter().filter_map(|(_, f)| if let Found::Ahead { sliced, of, layers, secs } = f { Some((*sliced, *of, *layers, *secs)) } else { None }).collect();
    let of = LAYERS_AHEAD.min(c.layers.layers);
    assert_eq!(ahead.iter().map(|a| (a.0, a.1, a.2)).collect::<Vec<_>>(), (0..=of).map(|k| (k, of, c.layers.layers)).collect::<Vec<_>>(), "the layers ahead, each in turn");
    assert!(ahead.windows(2).all(|w| w[0].3 <= w[1].3), "and their time only grows");
    let gaps: Vec<(f64, f64, P3)> = seen.iter().filter_map(|(_, f)| if let Found::Gap { searched, gap, at } = f { Some((*searched, *gap, *at)) } else { None }).collect();
    assert!(gaps.windows(2).all(|w| w[0].0 <= w[1].0 && w[1].1 <= w[0].1), "the share searched rises and the gap found only shrinks");
    let last = gaps.last().expect("the gap search reports");
    assert_eq!((last.0, last.1, last.2), (1.0, c.gap.gap, c.gap.at), "it ends having searched it all, at the gap it measured");
    let layers: Vec<(usize, usize)> = seen.iter().filter_map(|(_, f)| if let Found::Layer { k, n, .. } = f { Some((*k, *n)) } else { None }).collect();
    assert_eq!(layers, (1..=c.layers.layers).map(|k| (k, c.layers.layers)).collect::<Vec<_>>(), "every layer in turn");
    let tacks: Vec<Tack> = seen.iter().filter_map(|(_, f)| if let Found::Tack(t) = f { Some(*t) } else { None }).collect();
    assert_eq!(tacks, c.layers.tacks, "the tacks shown are the check's");
    for e in 0..4 {
        let sum: f64 = c.layers.tacks.iter().filter(|t| t.empty == e).map(|t| t.area).sum();
        assert!((sum - c.layers.tack[e]).abs() < 1e-9, "the tacks over {e} empty layers add up to the check's area: {sum} vs {}", c.layers.tack[e]);
    }
    for (k, t) in c.turns.iter().enumerate() {
        let angles: Vec<f64> = seen.iter().filter_map(|(_, f)| if let Found::Turn { turn, angle, .. } = f { (*turn == k).then_some(*angle) } else { None }).collect();
        assert!(!angles.is_empty(), "turn {k} tests angles");
        // the lattice rises, then the bisection closes in below the first contact
        let first_hit = angles.iter().position(|a| a.abs() > t.angle.abs() + 1e-12).unwrap_or(angles.len());
        assert!(angles[..first_hit].windows(2).all(|w| w[1].abs() > w[0].abs()), "turn {k} rises: {angles:?}");
        let stops: Vec<Turned> = seen.iter().filter_map(|(_, f)| if let Found::Stop(s) = f { (s.turn == k).then_some(*s) } else { None }).collect();
        assert_eq!(stops, vec![*t], "turn {k} stops once, where the check says");
        if let Some(p) = t.contact {
            // where they touch lies in the joint's region
            let (lo, hi) = plan.joints[0].region();
            assert!((0..3).all(|i| p[i] >= lo[i] - 1.0 && p[i] <= hi[i] + 1.0), "turn {k} touches at {p:?}, outside {lo:?}..{hi:?}");
        }
    }
    assert!(matches!(seen.last(), Some((_, Found::Stop(Turned { turn: 1, turns: 2, .. })))), "the last step is the last turn's stop");
    // a cancel in each phase: no step after the one it was raised on, and the joint left out
    let first_of = |p: usize| seen.iter().position(|(_, f)| phase(f) == p).unwrap() + 1;
    for at in [3, first_of(1), first_of(2), first_of(3) + 2] {
        let sink = Recorder { seen: Default::default(), cancel_after: at };
        let out = check_joints(&plan, &parts, Geometry::Design, &sink);
        assert!(out.is_empty(), "a cancelled check leaves its joint out");
        assert_eq!(sink.seen.into_inner().unwrap().len(), at, "it stops on the step it was cancelled on");
    }
    // printed with no clearance: not free, and why
    let (plan, built) = hinge_pair(Some(0.0));
    let fs = fields(&built, &plan);
    let parts = finals(&built, &fs);
    let fused = check_joints(&plan, &parts, Geometry::Design, &|| false);
    report_checks(&fused);
    let f = &fused[0];
    assert!(!f.ok(), "no clearance prints fused");
    assert_eq!(f.ok(), f.binds.is_empty(), "the binds say why");
    assert!(f.binds.iter().any(|b| matches!(b, Bind::Fused { .. })), "touching parts are fused: {:?}", f.binds);
    for b in &f.binds {
        if let Bind::Tacks(t) = b {
            assert!(f.layers.tacks.contains(t), "a tack it names is one it found");
        }
    }
}


/// Two blocks bonded to a TPU ribbon standing on edge through both, a free run between them: the
/// bond cuts the ribbon out of each block exactly (they print touching, fused); the soft joint sits
/// in the free run, as long as it, its section the ribbon's, and it bends only the ribbon's soft
/// way, about its tall side. Held by the first block, with gravity across the ribbon the second
/// sags until the ribbon's spring torque, E I / L times the bend, holds its weight; bonded to a
/// rigid ribbon it does not move.
#[test]
fn blocks_bonded_to_a_tpu_ribbon_bend_about_it() {
    use makepad_csg_mech::flex;
    use makepad_csg_mech::sim::{JointKind, MechWorld, GRAVITY};
    let profile = Profile::default();
    // the ribbon along X, 1.2 mm thin (Z) and 4 mm tall (Y); the blocks 6 mm apart on it
    let ribbon = PartBox { min: [-1.0, 1.0, -0.6], max: [55.0, 5.0, 0.6] };
    let (a, b) = (PartBox { min: [0.0, 0.0, -5.0], max: [18.0, 6.0, 5.0] }, PartBox { min: [24.0, 0.0, -5.0], max: [54.0, 6.0, 5.0] });
    let run = |flexible: bool| {
        let mut g = Graph::new();
        let nodes = vec![boxed(&mut g, ribbon.min, ribbon.max), boxed(&mut g, a.min, a.max), boxed(&mut g, b.min, b.max)];
        let bonds = vec![BondDecl { to: 0, parts: vec![1, 2], flexible }];
        let anchors = vec![AnchorDecl { part: 1, at: [9.0, 3.0, 0.0], hole: None }];
        let p = plan(&[ribbon, a, b], &[], &bonds, &anchors, &profile).unwrap();
        let built = build(g, &nodes, &["ribbon", "a", "b"], &p);
        (p, built)
    };
    let (p, built) = run(true);
    assert_eq!(built.watertight, vec![true; 3]);
    let fs = fields(&built, &p);
    let probe = |x: f64, y: f64, z: f64| makepad_csg_math::Vec3d { x, y, z };
    assert!(fs[1].distance(probe(9.0, 3.0, 0.45)) > 0.0 && fs[1].distance(probe(9.0, 3.0, 0.75)) < 0.0, "the block holds the ribbon with no gap");
    assert!(fs[0].distance(probe(9.0, 3.0, 0.45)) < 0.0, "the ribbon runs on through it");
    let parts = finals(&built, &fs);
    // the host measures the flexes on the final parts and keeps them in its plan
    let mut flexed = p.clone();
    flexed.flexes = flex::measure_all(&p.bonds, &parts).unwrap();
    let f = flexed.flexes[0];
    println!("  flex: run {:.2} mm at {:?}, area {:.2} mm2, moments {:.3} / {:.3} mm4, soft axis {:?}", f.length, f.pivot, f.section.area, f.section.moments.0, f.section.moments.1, f.section.soft_axis);
    assert!((f.length - 6.0).abs() < 0.25 && makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(f.pivot, [21.0, 3.0, 0.0])) < 0.2, "the free run between the blocks");
    assert!((f.section.area - 4.8).abs() < 0.2 && f.one_way() && f.section.soft_axis[1].abs() > 0.99, "a ribbon on edge bends about its tall side only");
    // a long, tall first part: the point midway between the centres is inside it, above the
    // ribbon, and off its line; the free run is still where the ribbon leaves it for the next
    let (a2, b2) = (PartBox { min: [0.0, 0.0, -5.0], max: [40.0, 20.0, 5.0] }, PartBox { min: [46.0, 0.0, -5.0], max: [54.0, 6.0, 5.0] });
    let mut g = Graph::new();
    let nodes = vec![boxed(&mut g, ribbon.min, ribbon.max), boxed(&mut g, a2.min, a2.max), boxed(&mut g, b2.min, b2.max)];
    let p2 = plan(&[ribbon, a2, b2], &[], &[BondDecl { to: 0, parts: vec![1, 2], flexible: true }], &[], &profile).unwrap();
    let built2 = build(g, &nodes, &["ribbon", "a", "b"], &p2);
    let fs2 = fields(&built2, &p2);
    let f2 = flex::measure(&p2.bonds[0], 0, &finals(&built2, &fs2)).unwrap();
    assert!((f2.length - 6.0).abs() < 0.25 && makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(f2.pivot, [43.0, 3.0, 0.0])) < 0.2 && f2.axis[0] > 0.999, "the free run past the long part: {f2:?}");
    let (soft, _) = f.stiffness(flex::FLEX_MODULUS);
    let asm = assembly(&flexed, &parts, &[], 1.0);
    let k = asm.joints.iter().position(|j| matches!(j.kind, JointKind::Flex { .. })).expect("a flex joint");
    assert!(asm.joints.iter().any(|j| j.kind == JointKind::Weld && j.a == 1 && j.b == 0), "the ribbon rides the first block");
    // gravity across the ribbon: the second block's weight about the pivot against the spring
    let sag = |asm: &makepad_csg_mech::sim::Assembly| {
        let mut w = MechWorld::new(asm, [0.0, 0.0, -GRAVITY], None);
        for _ in 0..720 {
            w.step(1.0 / 240.0);
        }
        (w.angles(asm), w.poses()[2].com)
    };
    let (angles, _) = sag(&asm);
    let lever = asm.bodies[2].mass.com[0] - f.pivot[0];
    let torque = asm.bodies[2].mass.mass * 1e-3 * 9.81 * lever;
    // k theta = torque cos theta
    let mut theta = torque / soft;
    for _ in 0..20 {
        theta = torque * theta.cos() / soft;
    }
    println!("  flex sag: {:.2} deg in the sim, {:.2} deg from k = {soft:.2} N mm/rad and {torque:.3} N mm", angles[k].abs().to_degrees(), theta.to_degrees());
    assert!((angles[k].abs() - theta).abs() < 0.15 * theta, "it sags as stiff as the ribbon: {:.2} vs {:.2} deg", angles[k].abs().to_degrees(), theta.to_degrees());
    // bonded to a rigid ribbon it is one body with it
    let (q, built) = run(false);
    let fs = fields(&built, &q);
    let parts = finals(&built, &fs);
    assert!(flex::measure_all(&q.bonds, &parts).unwrap().is_empty());
    let asm = assembly(&q, &parts, &[], 1.0);
    let rest = asm.bodies[2].mass.com;
    let (_, at) = sag(&asm);
    assert!(makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(at, rest)) < 0.2, "welded, it does not sag: {at:?} from {rest:?}");
}
