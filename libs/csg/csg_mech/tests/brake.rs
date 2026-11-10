// A braked hinge end to end: a thigh and a shin lying on the bed with the pin across the bed (the
// way a figure printed on its back has its knees), a shoe part round the shin's eye; planned for
// the default 0.4 / 0.2 profile, lowered and meshed, checked on the meshes' fields and simmed.
// Model frame Y up (print up), millimetres.

use makepad_csg_graph::eval::Evaluator;
use makepad_csg_graph::{BoolMode, Graph, NodeId, Op};
use makepad_csg_mech::geom::madd;
use makepad_csg_mech::sim::{simulate, Scenario, SimOptions, GRAVITY};
use makepad_csg_mech::*;
use makepad_csg_sdf::mesh_field::{MeshData, MeshField};
use makepad_csg_sdf::Sdf3;

fn boxed(g: &mut Graph, b: &PartBox) -> NodeId {
    let size = [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
    let n = g.push(Op::Box { size, radius: 0.0 }).unwrap();
    g.push(Op::Transform { source: n, at: b.center(), rotate: [0.0; 3], scale: [1.0; 3] }).unwrap()
}

fn mesh_field(m: &makepad_csg_mesh::TriMesh) -> MeshField {
    MeshField::new(MeshData { positions: m.vertices.iter().map(|p| [p.x as f32, p.y as f32, p.z as f32]).collect(), triangles: m.triangles.clone() })
}

/// Thigh (0) along -Z, shin (1) along +Z, meeting at the knee on the Z = 0 line; the pin along X
/// `JY` over the bed. The shoe part (2) is a block round the knee.
const JY: f64 = 7.0;

fn boxes() -> [PartBox; 3] {
    [
        PartBox { min: [-6.0, 0.0, -40.0], max: [6.0, 2.0 * JY, 0.0] },
        PartBox { min: [-6.0, 0.0, 0.0], max: [6.0, 2.0 * JY, 40.0] },
        PartBox { min: [-6.0, 0.0, -9.0], max: [6.0, 2.0 * JY, 9.0] },
    ]
}

fn knee(brake: Option<BrakeDecl>) -> JointDecl {
    JointDecl::Hinge(HingeDecl { a: 0, b: 1, at: [0.0, JY, 0.0], axis: [1.0, 0.0, 0.0], limits: None, pin: Some(2.5), plate: None, clearance: None, clearance_axial: None, brake, motor: None })
}

struct Built {
    originals: Vec<MeshField>,
    verts: Vec<Vec<P3>>,
    tris: Vec<Vec<[u32; 3]>>,
    watertight: Vec<bool>,
}

/// The leg as it is built: the knee's knuckle columns `rk` round (the hinge's eye), the pin a hair
/// higher over the bed; the thigh and the shin each a slab on the bed up to the pin and a column
/// round it, the shoe part a block round the knee.
struct Leg {
    rk: f64,
}

impl Leg {
    fn jy(&self) -> f64 {
        self.rk + 0.05
    }
    fn knee(&self, brake: Option<BrakeDecl>) -> JointDecl {
        JointDecl::Hinge(HingeDecl { a: 0, b: 1, at: [0.0, self.jy(), 0.0], axis: [1.0, 0.0, 0.0], limits: None, pin: Some(2.5), plate: None, clearance: None, clearance_axial: None, brake, motor: None })
    }
    fn slabs(&self) -> [PartBox; 3] {
        let jy = self.jy();
        [PartBox { min: [-6.0, 0.0, -40.0], max: [6.0, jy, 0.0] }, PartBox { min: [-6.0, 0.0, 0.0], max: [6.0, jy, 40.0] }, PartBox { min: [-6.0, 0.0, -9.0], max: [6.0, 2.0 * jy, 9.0] }]
    }
    fn boxes(&self) -> Vec<PartBox> {
        let (jy, rk) = (self.jy(), self.rk);
        let [t, s, shoe] = self.slabs();
        vec![PartBox { min: t.min, max: [6.0, jy + rk, rk] }, PartBox { min: [-6.0, 0.0, -rk], max: s.max }, shoe]
    }
    fn nodes(&self, g: &mut Graph, parts: usize) -> Vec<NodeId> {
        let slabs = self.slabs();
        (0..parts)
            .map(|i| {
                let slab = boxed(g, &slabs[i]);
                if i == 2 {
                    return slab;
                }
                let column = Prim::cyl([-6.0, self.jy(), 0.0], [6.0, self.jy(), 0.0], self.rk).lower(g, 96).unwrap();
                g.push(Op::Boolean { mode: BoolMode::Union, a: slab, b: vec![column] }).unwrap()
            })
            .collect()
    }
}

fn build(leg: &Leg, plan: &Plan, parts: usize) -> Built {
    let mut g = Graph::new();
    let nodes = leg.nodes(&mut g, parts);
    let finals: Vec<NodeId> = plan.solids.iter().map(|s| s.lower(&mut g, &nodes, 96).unwrap()).collect();
    let mut ev = Evaluator::new();
    ev.begin();
    let originals = nodes.iter().map(|n| mesh_field(&ev.mesh(&g, *n, None).map_err(|e| e.message).unwrap())).collect();
    let mut b = Built { originals, verts: Vec::new(), tris: Vec::new(), watertight: Vec::new() };
    for f in finals {
        let m = ev.mesh(&g, f, None).map_err(|e| e.message).unwrap();
        b.watertight.push(mesh_field(&m).watertight);
        b.verts.push(m.vertices.iter().map(|p| [p.x, p.y, p.z]).collect());
        b.tris.push(m.triangles.clone());
    }
    b
}

fn brake_of(plan: &Plan) -> Brake {
    match &plan.joints[0] {
        JointGeom::Hinge { brake: Some(b), .. } => b.clone(),
        other => panic!("no brake: {other:?}"),
    }
}

/// The teeth print in the pin's grooves a clearance clear of them: the knee turns free a few
/// degrees each way, then the pin's round comes under the teeth and presses them in, the whole
/// squeeze from well short of a quarter turn on, the same each way and each half turn. A hold
/// asked for shortens the teeth to it.
#[test]
fn a_brake_turns_free_at_its_printed_angle_and_holds_past_it() {
    let profile = Profile::default();
    let planned = plan(&boxes(), &[knee(Some(BrakeDecl { shoe: 2, hold: None }))], &[], &[], &profile).unwrap();
    let b = brake_of(&planned);
    let deg = |d: f64| d.to_radians();
    assert_eq!(b.squeeze(0.0), 0.0);
    assert!((b.squeeze(deg(90.0)) - BRAKE_SQUEEZE).abs() < 1e-9, "{}", b.squeeze(deg(90.0)));
    for a in [deg(17.0), deg(40.0), deg(75.0)] {
        assert!((b.squeeze(a) - b.squeeze(-a)).abs() < 1e-9 && (b.squeeze(a) - b.squeeze(a + std::f64::consts::PI)).abs() < 1e-9, "alike each way and each half turn at {a}");
    }
    let (free, whole) = b.band();
    assert!(free > deg(3.0) && free < deg(15.0), "free within {:.1} degrees", free.to_degrees());
    assert!(whole > free && whole < deg(35.0), "holds fully from {:.1} degrees", whole.to_degrees());
    let mut last = 0.0;
    for k in 0..=90 {
        let s = b.squeeze(deg(k as f64));
        assert!(s >= last - 1e-9, "the squeeze only grows toward a quarter turn: {s} at {k}");
        last = s;
    }
    let most = b.hold();
    assert!(most > 5.0 && most < 200.0, "it holds {most:.1} N mm");
    assert_eq!(b.torque(deg(90.0)), most);
    let asked = plan(&boxes(), &[knee(Some(BrakeDecl { shoe: 2, hold: Some(0.5 * most) }))], &[], &[], &profile).unwrap();
    let half = brake_of(&asked);
    assert!((half.hold() - 0.5 * most).abs() < 1e-6 * most, "asked {:.2}, holds {:.2}", 0.5 * most, half.hold());
    assert!(half.teeth.1 - half.teeth.0 < b.teeth.1 - b.teeth.0);
    assert!(planned.notes[0].iter().any(|n| n.text.starts_with("brake:")), "{:?}", planned.notes[0]);
}

/// A brake's shoe is a part of its own: not one the hinge joins, not one any joint joins, and not
/// another brake's. Its pin must be thick enough for its grooves.
#[test]
fn a_brake_needs_a_shoe_of_its_own_and_a_pin_to_groove() {
    let profile = Profile::default();
    let mut parts = boxes().to_vec();
    parts.push(PartBox { min: [-6.0, 0.0, 40.0], max: [6.0, 2.0 * JY, 70.0] });
    parts.push(PartBox { min: [-6.0, 0.0, 31.0], max: [6.0, 2.0 * JY, 49.0] });
    let ankle = |brake: Option<BrakeDecl>| JointDecl::Hinge(HingeDecl { a: 1, b: 3, at: [0.0, JY, 40.0], axis: [1.0, 0.0, 0.0], limits: None, pin: Some(2.5), plate: None, clearance: None, clearance_axial: None, brake, motor: None });
    let fails = |decls: &[JointDecl]| plan(&parts, decls, &[], &[], &profile).unwrap_err().message;
    assert!(fails(&[knee(Some(BrakeDecl { shoe: 1, hold: None }))]).contains("of its own"));
    assert!(fails(&[knee(Some(BrakeDecl { shoe: 3, hold: None })), ankle(None)]).contains("a joint joins"));
    assert!(fails(&[knee(Some(BrakeDecl { shoe: 2, hold: None })), ankle(Some(BrakeDecl { shoe: 2, hold: None }))]).contains("of its own"));
    plan(&parts, &[knee(Some(BrakeDecl { shoe: 2, hold: None })), ankle(Some(BrakeDecl { shoe: 4, hold: None }))], &[], &[], &profile).unwrap();
    let thin = JointDecl::Hinge(HingeDecl { a: 0, b: 1, at: [0.0, JY, 0.0], axis: [1.0, 0.0, 0.0], limits: None, pin: Some(1.0), plate: None, clearance: None, clearance_axial: None, brake: Some(BrakeDecl { shoe: 2, hold: None }), motor: None });
    assert!(fails(&[thin]).contains("a brake needs a pin"));
}

/// Built and meshed, the braked knee prints free: the shoes keep the hinge's clearance from the
/// thigh's pin as they print and the shoe part is only its two shoes, set in the shin's eye. Held
/// by the thigh, the shin raised 60 degrees and let go under gravity holds there; without the brake
/// it falls. Hung from the thigh, the shin still drops through its free band.
#[test]
fn a_braked_knee_prints_free_and_holds_a_raised_shin() {
    let profile = Profile::default();
    let raised = |brake: bool| -> (f64, Vec<JointCheck>) {
        let shoe = brake.then_some(BrakeDecl { shoe: 2, hold: None });
        let n = if brake { 3 } else { 2 };
        // the knuckle columns as round as the hinge's eye
        let sized = plan(&boxes()[..n], &[knee(shoe.clone())], &[], &[], &profile).unwrap();
        let JointGeom::Hinge { r_plate, .. } = sized.joints[0] else { unreachable!() };
        let leg = Leg { rk: r_plate };
        let anchors = [AnchorDecl { part: 0, at: [0.0, 1.0, -30.0], hole: None }];
        let plan = plan(&leg.boxes()[..n], &[leg.knee(shoe)], &[], &anchors, &profile).unwrap();
        let JointGeom::Hinge { r_plate: built_round, .. } = plan.joints[0] else { unreachable!() };
        assert_eq!(built_round, r_plate);
        let built = build(&leg, &plan, n);
        assert_eq!(built.watertight, vec![true; n]);
        let fs: Vec<SolidField> = plan.solids.iter().map(|s| SolidField { solid: s, parts: built.originals.iter().map(|f| f as &(dyn Sdf3 + Sync)).collect() }).collect();
        let names = ["thigh", "shin", "shoe"];
        let parts: Vec<FinalPart> = (0..n).map(|i| FinalPart { name: names[i], field: &fs[i], vertices: &built.verts[i], triangles: &built.tris[i] }).collect();
        if brake {
            let b = brake_of(&plan);
            let probe = |p: P3| makepad_csg_math::Vec3d { x: p[0], y: p[1], z: p[2] };
            let at = |r: f64, way: f64| probe(madd([0.0, leg.jy(), 0.0], b.dir, r * way));
            for way in [1.0, -1.0] {
                // the tooth's tip and the shoe behind it are the shoe's; the shin's eye wraps it
                assert!(fs[2].distance(at(b.tip + 0.05, way)) < 0.0, "the tooth's tip");
                assert!(fs[2].distance(at(0.5 * (b.bore + b.outer), way)) < 0.0, "the shoe");
                assert!(fs[1].distance(at(0.5 * (b.bore + b.outer), way)) > 0.0, "the shin's pocket");
                assert!(fs[1].distance(at(b.outer + 0.6, way)) < 0.0, "the eye round the shoe");
                // the groove in the pin under the tooth
                assert!(fs[0].distance(at(b.tip - 0.05, way)) > 0.0, "the pin's groove");
            }
            // nothing of the shoe part outside the knee
            assert!(fs[2].distance(probe([0.0, leg.jy(), 8.5])) > 0.0 && fs[2].distance(probe([0.0, 0.5, 0.0])) > 0.0);
        }
        let checks = check_joints(&plan, &parts, Geometry::Design, &|| false);
        for c in &checks {
            assert!(c.ok(), "brake {brake}: {:?} gap {:.3} (built {:.2})", c.binds, c.gap.gap, c.clearance);
        }
        let asm = assembly(&plan, &parts, &checks, 1.0);
        assert_eq!(asm.joints[0].brake.is_some(), brake);
        // raised: turned back about the pin's axis
        let mut pose = vec![-60f64.to_radians()];
        pose.resize(asm.joints.len(), 0.0);
        let r = simulate(&asm, &SimOptions { scenario: Scenario::Pose(pose), seconds: 3.0, gravity: Some([0.0, -GRAVITY, 0.0]), ..Default::default() }, &|| false).unwrap();
        println!("brake {brake}: raised 60, ends at {:.1} degrees, drift {:.1}", r.joints[0].last.to_degrees(), r.drift.unwrap_or(0.0).to_degrees());
        if brake {
            let s = simulate(&asm, &SimOptions { scenario: Scenario::Hang, seconds: 3.0, ..Default::default() }, &|| false).unwrap();
            println!("hung: the shin ends at {:.1} degrees", s.joints[0].last.to_degrees());
            assert!(s.joints[0].moved && s.joints[0].last.abs() < 25f64.to_radians(), "hung, the braked knee drops through its free band and holds: {:?}", s.joints[0]);
        }
        (r.drift.unwrap(), checks)
    };
    let (held, _) = raised(true);
    assert!(held < 2f64.to_radians(), "the brake holds the raised shin: it drifts {:.1} degrees", held.to_degrees());
    let (fell, _) = raised(false);
    assert!(fell > 20f64.to_radians(), "without the brake the shin falls: it drifts {:.1} degrees", fell.to_degrees());
}
