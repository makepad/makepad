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
}

fn mesh_field(m: &makepad_csg_mesh::TriMesh) -> MeshField {
    MeshField::new(MeshData { positions: m.vertices.iter().map(|p| [p.x as f32, p.y as f32, p.z as f32]).collect(), triangles: m.triangles.clone() })
}

fn build(mut g: Graph, nodes: &[NodeId], names: &[&str], plan: &Plan) -> Built {
    let finals: Vec<NodeId> = plan.solids.iter().map(|s| s.lower(&mut g, nodes, 96).unwrap()).collect();
    let mut ev = Evaluator::new();
    ev.begin();
    let mut b = Built { names: names.iter().map(|s| s.to_string()).collect(), verts: Vec::new(), tris: Vec::new(), originals: Vec::new() };
    for n in nodes {
        b.originals.push(mesh_field(&ev.mesh(&g, *n, None).map_err(|e| e.message).unwrap()));
    }
    for (i, f) in finals.into_iter().enumerate() {
        let m = ev.mesh(&g, f, None).map_err(|e| e.message).unwrap();
        let mf = mesh_field(&m);
        println!("  mesh {}: {} triangles, watertight {}", names[i], m.triangles.len(), mf.watertight);
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
        println!(
            "  {} {}-{}: gap {:.3} (built {:.2}) | layer gap {:.2} | tack n0 {:.1} n1 {:.1} n2 {:.1} n3 {:.1} | risk {} ({:.1}) | field limits -{:.1}/+{:.1} deg, effective {:.1}..{:.1} | retention {:?} | {:.0} ms (gap {:.0} layers {:.0} limits {:.0})",
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
            c.field.0.to_degrees(),
            c.field.1.to_degrees(),
            c.effective.0.to_degrees(),
            c.effective.1.to_degrees(),
            c.retention,
            c.millis,
            c.phases.0,
            c.phases.1,
            c.phases.2
        );
    }
}

#[test]
fn chain_of_eight_hangs_and_articulates() {
    let profile = Profile::default();
    let mut g = Graph::new();
    let n = 8;
    let pitch = 12.0;
    let nodes: Vec<NodeId> = (0..n).map(|i| boxed(&mut g, [i as f64 * pitch, 0.0, -2.5], [(i + 1) as f64 * pitch, 6.0, 2.5])).collect();
    let boxes: Vec<PartBox> = (0..n).map(|i| PartBox { min: [i as f64 * pitch, 0.0, -2.5], max: [(i + 1) as f64 * pitch, 6.0, 2.5] }).collect();
    let decls: Vec<JointDecl> = (0..n - 1)
        .map(|i| JointDecl::Hinge(HingeDecl { a: i, b: i + 1, at: [(i + 1) as f64 * pitch, 0.0, 0.0], axis: [0.0, 1.0, 0.0], limits: None, pin: None, plate: None, clearance: None, clearance_axial: None }))
        .collect();
    let anchors = vec![AnchorDecl { part: 0, at: [2.5, 3.0, 0.0], hole: Some(([0.0, 1.0, 0.0], 1.2)) }];
    let plan = plan(&boxes, &decls, &anchors, &profile).unwrap();
    let names: Vec<String> = (0..n).map(|i| format!("link[{i}]")).collect();
    let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let t = std::time::Instant::now();
    let built = build(g, &nodes, &name_refs, &plan);
    println!("chain: meshed in {:.0} ms; {:?}", t.elapsed().as_secs_f64() * 1e3, plan.joints[0]);
    let fs = fields(&built, &plan);
    let parts = finals(&built, &fs);
    let t = std::time::Instant::now();
    let checks = check_joints(&plan, &parts, Geometry::Design, &|| false);
    println!("chain: checks {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    report_checks(&checks);
    for c in &checks {
        assert!((c.gap.gap - c.clearance).abs() < 0.05, "gap {} vs {}", c.gap.gap, c.clearance);
        assert!(c.risk <= Risk::Medium, "{:?}", c.risk);
        assert!(c.field.0 > 45f64.to_radians() && c.field.1 > 45f64.to_radians());
        assert!(c.ok());
    }
    let asm = assembly(&plan, &parts, &checks, 1.0);
    for scenario in [Scenario::Hang, Scenario::Shake] {
        let r = simulate(&asm, &SimOptions { scenario, seconds: 20.0, ..Default::default() }, &|| false).unwrap();
        println!(
            "  sim {}: articulates {} hangs_right {} binding {:?} settle {:?} self-contacts {:?} | {} steps {:.0} ms",
            r.scenario, r.articulates, r.hangs_right, r.binding, r.settle, r.self_contacts, r.steps, r.wall_ms
        );
        for j in &r.joints {
            println!("    {} range {:.1}..{:.1} last {:.1} deg, limits {:.1}..{:.1}, hits {}, dead band {:.1}, hang error {:.1}, peak {:.4} N", j.name, j.min.to_degrees(), j.max.to_degrees(), j.last.to_degrees(), j.limits.0.to_degrees(), j.limits.1.to_degrees(), j.limit_hits, j.dead_band.to_degrees(), j.hang_error.to_degrees(), j.peak_force);
        }
        assert!(r.articulates, "{:?}", r.binding);
        if r.scenario == "hang" {
            assert!(r.hangs_right);
        }
    }
    // deterministic
    let a = simulate(&asm, &SimOptions { scenario: Scenario::Shake, seconds: 3.0, ..Default::default() }, &|| false).unwrap();
    let b = simulate(&asm, &SimOptions { scenario: Scenario::Shake, seconds: 3.0, ..Default::default() }, &|| false).unwrap();
    assert_eq!(a.final_pose, b.final_pose);
}

#[test]
fn puppet_arm_ball_shoulder_hinge_elbow() {
    let profile = Profile::default();
    let mut g = Graph::new();
    // model frame (Y up): torso lying on the bed, the arm along +Z beside it
    let torso_box = PartBox { min: [-8.0, 0.0, -26.0], max: [8.0, 15.0, 0.0] };
    let upper_box = PartBox { min: [11.0, 0.0, -20.0], max: [16.0, 11.0, -6.0] };
    let fore_box = PartBox { min: [11.0, 0.0, -6.0], max: [16.0, 11.0, 8.0] };
    let nodes = vec![boxed(&mut g, torso_box.min, torso_box.max), boxed(&mut g, upper_box.min, upper_box.max), boxed(&mut g, fore_box.min, fore_box.max)];
    let shoulder = JointDecl::Ball(BallDecl { a: 0, b: 1, at: [13.5, 7.5, -20.0], radius: Some(5.5), neck: Some(2.0), axis: Some([0.0, 0.0, 1.0]), cone: 35f64.to_radians(), twist: None, clearance: None, wall: Some(2.0) });
    let elbow = JointDecl::Hinge(HingeDecl { a: 1, b: 2, at: [13.5, 0.0, -6.0], axis: [0.0, 1.0, 0.0], limits: Some((-110f64.to_radians(), 110f64.to_radians())), pin: None, plate: None, clearance: None, clearance_axial: None });
    let anchors = vec![AnchorDecl { part: 0, at: [0.0, 7.5, -13.0], hole: None }];
    // the torso is shorter than the shell: extend its box to the socket for sizing
    let plan = plan(&[torso_box, upper_box, fore_box], &[shoulder, elbow], &anchors, &profile).unwrap();
    for (i, n) in plan.notes.iter().enumerate() {
        for x in n {
            println!("  note joint {i}: {}", x.text);
        }
    }
    let built = build(g, &nodes, &["torso", "upper_arm", "forearm"], &plan);
    let fs = fields(&built, &plan);
    let parts = finals(&built, &fs);
    let checks = check_joints(&plan, &parts, Geometry::Design, &|| false);
    println!("puppet:");
    report_checks(&checks);
    for c in &checks {
        assert!(c.gap.gap > 0.25, "{} gap {}", c.kind, c.gap.gap);
        assert!(c.risk <= Risk::Medium, "{} {:?}", c.kind, c.risk);
        assert!(c.ok(), "{c:?}");
    }
    let asm = assembly(&plan, &parts, &checks, 1.0);
    for scenario in [Scenario::Hang, Scenario::Shake, Scenario::Pose(vec![20f64.to_radians(), 60f64.to_radians()])] {
        let r = simulate(&asm, &SimOptions { scenario, seconds: 8.0, ..Default::default() }, &|| false).unwrap();
        println!("  sim {}: articulates {} hangs_right {} binding {:?} settle {:?} drift {:?} self-contacts {:?} | {:.0} ms", r.scenario, r.articulates, r.hangs_right, r.binding, r.settle, r.drift.map(f64::to_degrees), r.self_contacts, r.wall_ms);
        for j in &r.joints {
            println!("    {} range {:.1}..{:.1} last {:.1} deg, limits {:.1}..{:.1}, hits {}, dead band {:.1}, hang error {:.1}", j.name, j.min.to_degrees(), j.max.to_degrees(), j.last.to_degrees(), j.limits.0.to_degrees(), j.limits.1.to_degrees(), j.limit_hits, j.dead_band.to_degrees(), j.hang_error.to_degrees());
        }
        if r.scenario == "shake" {
            assert!(r.articulates, "{:?}", r.binding);
        }
    }
}

#[test]
fn runner_publishes_poses_and_takes_a_grab() {
    use makepad_csg_mech::runner::{Command, Runner};
    let profile = Profile::default();
    let mut g = Graph::new();
    let nodes: Vec<NodeId> = (0..3).map(|i| boxed(&mut g, [i as f64 * 12.0, 0.0, -2.5], [(i + 1) as f64 * 12.0, 6.0, 2.5])).collect();
    let boxes: Vec<PartBox> = (0..3).map(|i| PartBox { min: [i as f64 * 12.0, 0.0, -2.5], max: [(i + 1) as f64 * 12.0, 6.0, 2.5] }).collect();
    let decls: Vec<JointDecl> = (0..2).map(|i| JointDecl::Hinge(HingeDecl { a: i, b: i + 1, at: [(i + 1) as f64 * 12.0, 0.0, 0.0], axis: [0.0, 1.0, 0.0], limits: None, pin: None, plate: None, clearance: None, clearance_axial: None })).collect();
    let anchors = vec![AnchorDecl { part: 0, at: [2.5, 3.0, 0.0], hole: Some(([0.0, 1.0, 0.0], 1.2)) }];
    let plan = plan(&boxes, &decls, &anchors, &profile).unwrap();
    let built = build(g, &nodes, &["l0", "l1", "l2"], &plan);
    let fs = fields(&built, &plan);
    let parts = finals(&built, &fs);
    let asm = assembly(&plan, &parts, &[], 1.0);
    let gravity = makepad_csg_mech::geom::scale(asm.hang_gravity(), makepad_csg_mech::sim::GRAVITY);
    let mut r = Runner::start(asm, gravity);
    let s0 = r.latest();
    assert_eq!(s0.poses.len(), 3);
    r.send(Command::Play);
    std::thread::sleep(std::time::Duration::from_millis(400));
    let s1 = r.latest();
    assert!(s1.frame > 40, "frames {}", s1.frame);
    assert!(s1.time > 0.15 && s1.running);
    // the last link has swung away from where it was printed
    let moved = makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(s1.poses[2].com, s0.poses[2].com));
    assert!(moved > 1.0, "moved {moved}");
    // grab the tip and pull it up along the print plane: it follows
    let tip = s1.poses[2].com;
    r.send(Command::Grab { body: 2, point: tip });
    let target = [tip[0] + 10.0, tip[1], tip[2] + 30.0];
    r.send(Command::Drag { point: target });
    std::thread::sleep(std::time::Duration::from_millis(800));
    let s2 = r.latest();
    assert_eq!(s2.grabbed, Some(2));
    let off = makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(s2.poses[2].com, target));
    println!("runner: {} frames in {:.2} s sim, tip {:.1} mm from the drag target", s2.frame, s2.time, off);
    assert!(off < makepad_csg_mech::geom::len(makepad_csg_mech::geom::sub(tip, target)) * 0.5, "tip did not follow: {off}");
    r.send(Command::Release);
    r.send(Command::Reset);
    std::thread::sleep(std::time::Duration::from_millis(50));
    let s3 = r.latest();
    assert!(!s3.running);
    let _ = r.take_events();
}
