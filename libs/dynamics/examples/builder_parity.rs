//! Small reference-compiler fixtures, in millimetres, grams and seconds.
use makepad_dynamics::model_spec::*;
use makepad_dynamics::types::*;
use makepad_dynamics::xml_export::write_mjcf;
use std::path::Path;

fn model(name: &str) -> ModelSpec {
    let mut s = ModelSpec { modelname: Some(name.into()), ..ModelSpec::default() };
    s.option.gravity = [0.0, 0.0, -9810.0];
    s.option.timestep = 1.0 / 240.0;
    s
}

fn capsule(name: &str, fromto: [f64; 6]) -> GeomSpec {
    GeomSpec {
        name: Some(name.into()), r#type: GEOM_CAPSULE, fromto,
        size: [5.0, 0.0, 0.0], density: 0.001,
        ..GeomSpec::default()
    }
}

fn link(name: &str, parent: usize, pos: [f64; 3], joint_type: i32) -> BodySpec {
    BodySpec {
        name: Some(name.into()), parent, pos, mass: 100.0,
        ipos: [0.0, 0.0, -50.0], inertia: [83333.0, 83333.0, 1250.0],
        joints: vec![JointSpec {
            name: Some(format!("{name}_joint")), r#type: joint_type,
            axis: [0.0, 1.0, 0.0], ..JointSpec::default()
        }],
        geoms: vec![capsule(&format!("{name}_geom"), [0.0, 0.0, 0.0, 0.0, 0.0, -100.0])],
        ..BodySpec::default()
    }
}

fn chain(ball: bool) -> ModelSpec {
    let mut s = model(if ball { "ball_chain" } else { "hinge_chain" });
    s.bodies.push(BodySpec {
        name: Some("base".into()), pos: [0.0, 0.0, 600.0], ipos: [0.0; 3],
        mass: 1000.0, inertia: [10000.0; 3],
        geoms: vec![GeomSpec { size: [10.0; 3], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    for i in 0..5 {
        let mut b = link(&format!("link{i}"), i + 1,
            [0.0, 0.0, if i == 0 { 0.0 } else { -100.0 }],
            if ball { JNT_BALL } else { JNT_HINGE });
        if ball {
            b.joints[0].range = [0.0, 1.2];
            b.joints[0].damping = [200.0, 0.0, 0.0];
        }
        if i == 4 {
            b.sites.push(SiteSpec {
                name: Some("tip".into()), pos: [0.0, 0.0, -100.0],
                size: [2.0; 3], ..SiteSpec::default()
            });
        }
        s.bodies.push(b);
    }
    s
}

fn slide_hinge() -> ModelSpec {
    let mut s = model("slide_hinge");
    let mut b = link("carriage", 0, [0.0, 0.0, 200.0], JNT_SLIDE);
    b.joints[0] = JointSpec {
        name: Some("slide".into()), r#type: JNT_SLIDE, axis: [1.0, 0.0, 0.0],
        range: [-25.0, 25.0], stiffness: [300.0, 0.0, 0.0], springref: 3.0,
        damping: [20.0, 0.0, 0.0], frictionloss: 5.0, armature: 2.0,
        ..JointSpec::default()
    };
    b.joints.push(JointSpec {
        name: Some("hinge".into()), axis: [0.0, 1.0, 0.0], range: [-0.7, 0.9],
        stiffness: [5000.0, 0.0, 0.0], springref: 0.1, damping: [300.0, 0.0, 0.0],
        frictionloss: 100.0, armature: 10.0, ..JointSpec::default()
    });
    s.bodies.push(b);
    s
}

fn free_box() -> ModelSpec {
    let mut s = model("free_box");
    s.option.cone = CONE_ELLIPTIC;
    let contact = GeomSpec {
        condim: 3, solref: [0.015, 0.8], solimp: [0.85, 0.97, 0.5, 0.5, 2.0],
        friction: [0.8, 0.02, 0.001], margin: 0.1, density: 0.001,
        ..GeomSpec::default()
    };
    s.bodies[0].geoms.push(GeomSpec {
        name: Some("floor".into()), r#type: GEOM_PLANE, size: [500.0, 500.0, 10.0],
        ..contact.clone()
    });
    s.bodies.push(BodySpec {
        name: Some("box".into()), pos: [0.0, 0.0, 10.0], ipos: [0.0; 3], mass: 80.0,
        fullinertia: [10000.0, 12000.0, 14000.0, 0.0, 0.0, 0.0],
        joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
        geoms: vec![GeomSpec { r#type: GEOM_BOX, size: [20.0, 15.0, 10.0], ..contact }],
        ..BodySpec::default()
    });
    s
}

fn connect(name1: &str, name2: &str, anchor: [f64; 3]) -> EqualitySpec {
    let mut e = EqualitySpec {
        name1: Some(name1.into()), name2: Some(name2.into()), objtype: OBJ_BODY,
        ..EqualitySpec::default()
    };
    e.data[..3].copy_from_slice(&anchor);
    e
}

fn four_bar() -> ModelSpec {
    let mut s = model("four_bar");
    s.bodies.push(link("left", 0, [0.0, 0.0, 200.0], JNT_HINGE));
    let mut bar = link("bar", 1, [0.0, 0.0, -100.0], JNT_HINGE);
    bar.ipos = [50.0, 0.0, 0.0];
    bar.inertia = [1250.0, 83333.0, 83333.0];
    bar.geoms[0].fromto = [0.0, 0.0, 0.0, 100.0, 0.0, 0.0];
    s.bodies.push(bar);
    s.bodies.push(link("right", 0, [100.0, 0.0, 200.0], JNT_HINGE));
    s.equalities.push(connect("bar", "right", [100.0, 0.0, 0.0]));
    s
}

fn mocap_pendulum() -> ModelSpec {
    let mut s = model("mocap_pendulum");
    s.bodies.push(BodySpec {
        name: Some("handle".into()), pos: [0.0, 0.0, 400.0], ipos: [0.0; 3],
        mocap: true, mass: 1.0, inertia: [1.0; 3],
        sites: vec![SiteSpec { size: [3.0; 3], ..SiteSpec::default() }],
        ..BodySpec::default()
    });
    s.bodies.push(link("pendulum", 0, [0.0, 0.0, 400.0], JNT_FREE));
    s.equalities.push(connect("handle", "pendulum", [0.0; 3]));
    s
}

fn rings() -> ModelSpec {
    let mut s = model("rings");
    s.bodies[0].geoms.push(GeomSpec {
        r#type: GEOM_PLANE, size: [500.0, 500.0, 10.0], ..GeomSpec::default()
    });
    for ring in 0..2 {
        let name = format!("ring{ring}");
        let mut b = BodySpec {
            name: Some(name.clone()), pos: [50.0 * ring as f64, 0.0, 150.0],
            ipos: [0.0; 3], mass: 160.0,
            inertia: if ring == 0 { [288000.0, 288000.0, 576000.0] }
                else { [288000.0, 576000.0, 288000.0] },
            joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
            ..BodySpec::default()
        };
        let point = |i: usize| {
            let angle = (i % 8) as f64 * std::f64::consts::TAU / 8.0;
            let x = 60.0 * angle.cos();
            let y = 60.0 * angle.sin();
            if ring == 0 { [x, y, 0.0] } else { [x, 0.0, y] }
        };
        for i in 0..8 {
            let a = point(i);
            let z = point(i + 1);
            let mut g = capsule(&format!("{name}_segment{i}"), [a[0], a[1], a[2], z[0], z[1], z[2]]);
            g.contype = 1 << ring;
            g.conaffinity = 3;
            b.geoms.push(g);
        }
        s.bodies.push(b);
    }
    s.pairs.push(PairSpec {
        name: Some("ring_contact".into()), geomname1: Some("ring0_segment0".into()),
        geomname2: Some("ring1_segment4".into()), ..PairSpec::default()
    });
    s.excludes.push(ExcludeSpec {
        name: Some("floor_exclusion".into()), bodyname1: Some("world".into()),
        bodyname2: Some("ring1".into())
    });
    s
}

// The eight corners of a box [0, x] x [0, y] x [0, z]; every coordinate is exact in a float.
fn corners(x: f32, y: f32, z: f32) -> Vec<f32> {
    let mut v = Vec::new();
    for i in 0..8 {
        v.extend([if i & 1 != 0 { x } else { 0.0 }, if i & 2 != 0 { y } else { 0.0 }, if i & 4 != 0 { z } else { 0.0 }]);
    }
    v
}

fn mesh_geom(name: &str, mesh: &str) -> GeomSpec {
    GeomSpec {
        name: Some(name.into()), meshname: Some(mesh.into()), r#type: GEOM_MESH,
        condim: 3, solref: [0.015, 0.8], solimp: [0.85, 0.97, 0.5, 0.5, 2.0],
        friction: [0.8, 0.02, 0.001], margin: 0.1, ..GeomSpec::default()
    }
}

fn floor() -> GeomSpec {
    GeomSpec { name: Some("floor".into()), r#type: GEOM_PLANE, size: [500.0, 500.0, 10.0], ..GeomSpec::default() }
}

// (h) The eight corners of a box as a convex hull, on a free body.
fn hull_box() -> ModelSpec {
    let mut s = model("hull_box");
    s.meshes.push(MeshSpec { name: Some("block".into()), vert: corners(40.0, 30.0, 20.0), ..MeshSpec::default() });
    s.bodies[0].geoms.push(floor());
    s.bodies.push(BodySpec {
        name: Some("block".into()), pos: [0.0, 0.0, 60.0], ipos: [0.0; 3], mass: 80.0,
        inertia: [8666.0, 13333.0, 16666.0],
        joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
        geoms: vec![mesh_geom("block_geom", "block")],
        ..BodySpec::default()
    });
    s
}

// (i) Twenty points of an ellipsoid (a golden-angle spiral, no four coplanar) on a hinge.
fn hull_ellipsoid() -> ModelSpec {
    let mut s = model("hull_ellipsoid");
    let mut vert = Vec::new();
    for k in 0..20 {
        let z = 1.0 - (2.0 * k as f64 + 1.0) / 20.0;
        let r = (1.0 - z * z).sqrt();
        let angle = k as f64 * 2.399963229728653;
        vert.extend([(30.0 * r * angle.cos()) as f32, (22.0 * r * angle.sin()) as f32, (15.0 * z - 40.0) as f32]);
    }
    s.meshes.push(MeshSpec { name: Some("stone".into()), vert, ..MeshSpec::default() });
    s.bodies.push(BodySpec {
        name: Some("pendulum".into()), pos: [0.0, 0.0, 120.0], ipos: [0.0, 0.0, -40.0], mass: 50.0,
        inertia: [30000.0, 30000.0, 20000.0],
        joints: vec![JointSpec { name: Some("hinge".into()), axis: [0.0, 1.0, 0.0], ..JointSpec::default() }],
        geoms: vec![mesh_geom("stone_geom", "stone")],
        ..BodySpec::default()
    });
    s
}

// (j) Two box-like hulls on one body, the second turned and moved.
fn hull_pair() -> ModelSpec {
    let mut s = model("hull_pair");
    s.meshes.push(MeshSpec { name: Some("block_a".into()), vert: corners(40.0, 30.0, 20.0), ..MeshSpec::default() });
    s.meshes.push(MeshSpec { name: Some("block_b".into()), vert: corners(25.0, 25.0, 15.0), ..MeshSpec::default() });
    s.bodies[0].geoms.push(floor());
    let angle: f64 = 0.3;
    let mut second = mesh_geom("block_b_geom", "block_b");
    second.pos = [60.0, 5.0, 0.0];
    second.quat = [(angle / 2.0).cos(), 0.0, 0.0, (angle / 2.0).sin()];
    s.bodies.push(BodySpec {
        name: Some("pair".into()), pos: [0.0, 0.0, 60.0], ipos: [30.0, 10.0, 10.0], mass: 200.0,
        inertia: [30000.0, 60000.0, 70000.0],
        joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
        geoms: vec![mesh_geom("block_a_geom", "block_a"), second],
        ..BodySpec::default()
    });
    s
}

fn site(name: &str, pos: [f64; 3]) -> SiteSpec {
    SiteSpec { name: Some(name.into()), pos, size: [5.0; 3], ..SiteSpec::default() }
}

fn wrap_geom(name: &str, r#type: i32, pos: [f64; 3], quat: [f64; 4], size: [f64; 3]) -> GeomSpec {
    GeomSpec {
        name: Some(name.into()), r#type, pos, quat, size, contype: 0, conaffinity: 0,
        density: 0.001, ..GeomSpec::default()
    }
}

// Spatial tendons over a cylinder with a side site, over a sphere, and through a pulley,
// and a fixed tendon coupling two hinges.
fn tendon_arm() -> ModelSpec {
    let mut s = model("tendon_arm");
    s.bodies[0].sites.extend([site("anchor", [0.0, 0.0, 1500.0]), site("anchor2", [300.0, 0.0, 1500.0])]);
    let mut upper = link("upper", 0, [0.0, 0.0, 1000.0], JNT_HINGE);
    let half = std::f64::consts::FRAC_1_SQRT_2;
    upper.geoms.push(wrap_geom("elbow_cyl", GEOM_CYLINDER, [0.0, 0.0, -100.0], [half, half, 0.0, 0.0], [20.0, 15.0, 0.0]));
    upper.sites.extend([site("elbow_side", [30.0, 0.0, -100.0]), site("upper_s", [10.0, 0.0, -30.0])]);
    s.bodies.push(upper);
    let mut lower = link("lower", 1, [0.0, 0.0, -100.0], JNT_HINGE);
    lower.joints[0].range = [-2.0, 2.0];
    lower.sites.push(site("lower_s", [10.0, 0.0, -70.0]));
    s.bodies.push(lower);
    let mut hand = link("hand", 2, [0.0, 0.0, -100.0], JNT_BALL);
    hand.joints[0].range = [0.0, 0.6];
    hand.geoms.push(wrap_geom("hand_wrap", GEOM_SPHERE, [0.0, 0.0, -50.0], [1.0, 0.0, 0.0, 0.0], [25.0, 0.0, 0.0]));
    hand.sites.push(site("hand_s", [0.0, 0.0, -100.0]));
    s.bodies.push(hand);
    s.tendons.push(TendonSpec {
        name: Some("flexor".into()), stiffness: [50.0, 0.0, 0.0], damping: [0.5, 0.0, 0.0],
        springlength: [600.0, 600.0], width: 5.0,
        path: vec![WrapSpec::site("anchor"), WrapSpec::site("upper_s"), WrapSpec::geom("elbow_cyl", Some("elbow_side")), WrapSpec::site("lower_s")],
        ..TendonSpec::default()
    });
    s.tendons.push(TendonSpec {
        name: Some("over_hand".into()), range: [0.0, 400.0],
        path: vec![WrapSpec::site("lower_s"), WrapSpec::geom("hand_wrap", None), WrapSpec::site("hand_s")],
        ..TendonSpec::default()
    });
    s.tendons.push(TendonSpec {
        name: Some("pulley_pair".into()), stiffness: [5.0, 0.0, 0.0], springlength: [1200.0, 1200.0],
        path: vec![WrapSpec::site("anchor"), WrapSpec::site("upper_s"), WrapSpec::pulley(2.0), WrapSpec::site("anchor2"), WrapSpec::site("lower_s")],
        ..TendonSpec::default()
    });
    s.tendons.push(TendonSpec {
        name: Some("coupling".into()), limited: 1, range: [-0.5, 0.5], stiffness: [1.0, 0.0, 0.0],
        path: vec![WrapSpec::joint("upper_joint", 1.0), WrapSpec::joint("lower_joint", -0.5)],
        ..TendonSpec::default()
    });
    s
}

// A puppet on strings: one-sided length limits from a bar to a free torso and its arms, with
// friction loss and damping; one string has armature, which demotes its bodies from simple,
// and one stretches plastically.
fn puppet_strings() -> ModelSpec {
    let mut s = model("puppet_strings");
    s.bodies[0].sites.push(site("hook", [600.0, 0.0, 900.0]));
    let mut bar = link("bar", 0, [0.0, 0.0, 1000.0], JNT_HINGE);
    bar.joints[0].axis = [1.0, 0.0, 0.0];
    bar.sites.extend([site("bar_l", [-150.0, 0.0, 0.0]), site("bar_c", [0.0; 3]), site("bar_r", [150.0, 0.0, 0.0])]);
    s.bodies.push(bar);
    let mut torso = link("torso", 0, [0.0, 0.0, 300.0], JNT_FREE);
    torso.sites.push(site("head", [0.0, 0.0, 20.0]));
    s.bodies.push(torso);
    let mut arm_l = link("arm_l", 2, [-70.0, 0.0, -10.0], JNT_HINGE);
    arm_l.joints[0].range = [-2.5, 0.3];
    arm_l.sites.push(site("hand_l", [0.0, 0.0, -100.0]));
    s.bodies.push(arm_l);
    let mut arm_r = link("arm_r", 2, [70.0, 0.0, -10.0], JNT_BALL);
    arm_r.joints[0].range = [0.0, 1.2];
    arm_r.sites.push(site("hand_r", [0.0, 0.0, -100.0]));
    s.bodies.push(arm_r);
    let string = |name: &str, from: &str, to: &str, length: f64| TendonSpec {
        name: Some(name.into()), range: [0.0, length], width: 2.0,
        path: vec![WrapSpec::site(from), WrapSpec::site(to)],
        ..TendonSpec::default()
    };
    s.tendons.push(string("head_string", "bar_c", "head", 700.0));
    s.tendons.push(TendonSpec { frictionloss: 10000.0, ..string("left_string", "bar_l", "hand_l", 800.0) });
    s.tendons.push(TendonSpec { damping: [10.0, 0.0, 0.0], armature: 1.0, ..string("right_string", "bar_r", "hand_r", 800.0) });
    s.tendons.push(TendonSpec {
        limited: 0, stiffness: [100.0, 0.0, 0.0], springyield: 5000.0,
        ..string("sag_string", "hook", "head", 0.0)
    });
    s
}

fn on(name: &str, trntype: i32, target: &str) -> ActuatorSpec {
    ActuatorSpec { name: Some(name.into()), ..ActuatorSpec::on(trntype, target) }
}

// An actuator made by one of the shortcut setters, on fields set beforehand.
fn shortcut(mut a: ActuatorSpec, set: impl FnOnce(&mut ActuatorSpec) -> Result<(), String>) -> ActuatorSpec {
    set(&mut a).expect("a valid shortcut");
    a
}

fn gear(first: [f64; 3]) -> [f64; 6] { [first[0], first[1], first[2], 0.0, 0.0, 0.0] }

// Servos on a hinge arm and a carriage: position (kv and an inherited range; a damping ratio with
// a time constant), intvelocity with an inherited activation range, velocity in the parent frame,
// a geared motor with a force clamp, damper, cylinder, a pid with integral, anti-windup, slew and
// feedforward, and actuators on fixed tendons (one with armature and damping, which demotes the
// carriage from simple; one with an inherited range).
fn servo_arm() -> ModelSpec {
    let mut s = model("servo_arm");
    let mut upper = link("upper", 0, [0.0, 0.0, 1000.0], JNT_HINGE);
    upper.joints[0].range = [-2.0, 2.0];
    s.bodies.push(upper);
    let mut lower = link("lower", 1, [0.0, 0.0, -100.0], JNT_HINGE);
    lower.joints[0].range = [-1.5, 1.5];
    s.bodies.push(lower);
    s.bodies.push(BodySpec {
        name: Some("carriage".into()), pos: [300.0, 0.0, 500.0], ipos: [0.0; 3], mass: 100.0, inertia: [1000.0; 3],
        joints: vec![JointSpec { name: Some("rail".into()), r#type: JNT_SLIDE, axis: [1.0, 0.0, 0.0], range: [-50.0, 50.0], ..JointSpec::default() }],
        geoms: vec![GeomSpec { r#type: GEOM_BOX, size: [10.0; 3], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    s.tendons.push(TendonSpec {
        name: Some("elbow_pair".into()), limited: 1, range: [-1.0, 1.0],
        path: vec![WrapSpec::joint("upper_joint", 1.0), WrapSpec::joint("lower_joint", -0.5)],
        ..TendonSpec::default()
    });
    s.tendons.push(TendonSpec { name: Some("rail_belt".into()), path: vec![WrapSpec::joint("rail", 1.0)], ..TendonSpec::default() });
    s.actuators.extend([
        shortcut(on("shoulder", TRN_JOINT, "upper_joint"), |a| a.set_to_position(2e8, Some(2e7), None, None, 0.9)),
        shortcut(ActuatorSpec { gear: gear([50.0, 0.0, 0.0]), forcerange: [-0.5, 0.5], ctrlrange: [-1.0, 1.0], ..on("shoulder_motor", TRN_JOINT, "upper_joint") }, |a| a.set_to_motor()),
        shortcut(on("elbow", TRN_JOINT, "lower_joint"), |a| a.set_to_position(1e8, None, Some(1.0), Some(0.02), 0.0)),
        shortcut(on("elbow_speed", TRN_JOINT, "lower_joint"), |a| a.set_to_int_velocity(1e7, Some(1e6), None, None, 1.0)),
        shortcut(on("elbow_drag", TRN_JOINTINPARENT, "lower_joint"), |a| a.set_to_velocity(1e6)),
        shortcut(ActuatorSpec { ctrlrange: [0.0, 1.0], ..on("rail_brake", TRN_JOINT, "rail") }, |a| a.set_to_damper(1000.0)),
        shortcut(ActuatorSpec { ctrlrange: [0.0, 2.0], ..on("rail_ram", TRN_JOINT, "rail") }, |a| a.set_to_cylinder(0.05, 0.0, 300.0, -1.0)),
        shortcut(ActuatorSpec { ctrlrange: [-40.0, 40.0], velrange: [-100.0, 100.0], ffrange: [-1e4, 1e4], forcerange: [-5e4, 5e4], ..on("rail_pid", TRN_JOINT, "rail") },
            |a| a.set_to_pid(1e5, Some(2e3), None, Some(1e5), Some(1e3), Some(100.0), 0.0, INPUT_POS | INPUT_VEL | INPUT_FF)),
        shortcut(ActuatorSpec { armature: 50.0, damping: [100.0, 0.0, 0.0], ..on("belt_motor", TRN_TENDON, "rail_belt") }, |a| a.set_to_motor()),
        shortcut(on("pair_servo", TRN_TENDON, "elbow_pair"), |a| a.set_to_position(5e7, Some(5e6), None, None, 1.0)),
    ]);
    s
}

// SO3 servos on a ball joint (expmap, damping ratio) and on a site against a reference site
// (quat), site motors with and without a reference site, a ball motor in the parent frame, a
// motor on a free joint, adhesion on a body resting on the floor, and a slider-crank.
fn site_actuators() -> ModelSpec {
    let mut s = model("site_actuators");
    s.bodies[0].geoms.push(GeomSpec { name: Some("floor".into()), r#type: GEOM_PLANE, size: [2000.0, 2000.0, 10.0], ..GeomSpec::default() });
    s.bodies[0].sites.extend([site("anchor", [0.0, 0.0, 1100.0]), site("slider_base", [600.0, 0.0, 700.0])]);
    // the balls' links spin about their length too, so the servos' twist is no stiffer than a step
    let mut wrist = link("wrist", 0, [0.0, 0.0, 1000.0], JNT_BALL);
    wrist.inertia[2] = 40000.0;
    wrist.sites.push(site("wrist_s", [0.0, 0.0, -100.0]));
    s.bodies.push(wrist);
    let mut gimbal = link("gimbal", 0, [300.0, 0.0, 1000.0], JNT_BALL);
    gimbal.inertia[2] = 40000.0;
    gimbal.sites.push(site("gimbal_s", [0.0, 0.0, -100.0]));
    s.bodies.push(gimbal);
    s.bodies.push(BodySpec {
        name: Some("pad".into()), pos: [-300.0, 0.0, 10.0], ipos: [0.0; 3], mass: 80.0, inertia: [5000.0; 3],
        joints: vec![JointSpec { name: Some("pad_free".into()), r#type: JNT_FREE, ..JointSpec::default() }],
        geoms: vec![GeomSpec { r#type: GEOM_BOX, size: [20.0, 20.0, 10.0], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    let mut crank = link("crank", 0, [600.0, 0.0, 900.0], JNT_HINGE);
    crank.sites.push(site("crank_pin", [0.0, 0.0, -50.0]));
    s.bodies.push(crank);
    let anchor = Some("anchor".to_string());
    s.actuators.extend([
        shortcut(on("wrist_servo", TRN_JOINT, "wrist_joint"), |a| a.set_to_orientation(5e7, None, Some(1.0), 0)),
        shortcut(ActuatorSpec { refsite: anchor.clone(), ..on("gimbal_servo", TRN_SITE, "gimbal_s") }, |a| a.set_to_orientation(5e7, Some(5e5), None, CHART_QUAT)),
        shortcut(ActuatorSpec { refsite: anchor, gear: gear([0.0, 1e4, 0.0]), ..on("gimbal_push", TRN_SITE, "gimbal_s") }, |a| a.set_to_motor()),
        shortcut(ActuatorSpec { gear: [0.0, 0.0, 0.0, 0.0, 0.0, 1e6], ..on("wrist_twist", TRN_SITE, "wrist_s") }, |a| a.set_to_motor()),
        shortcut(ActuatorSpec { gear: gear([1e6, 0.0, 5e5]), ..on("wrist_parent", TRN_JOINTINPARENT, "wrist_joint") }, |a| a.set_to_motor()),
        shortcut(ActuatorSpec { gear: gear([5e4, 0.0, 0.0]), ..on("pad_push", TRN_JOINT, "pad_free") }, |a| a.set_to_motor()),
        shortcut(ActuatorSpec { ctrlrange: [0.0, 1.0], ..on("pad_grip", TRN_BODY, "pad") }, |a| a.set_to_adhesion(1e5)),
        shortcut(ActuatorSpec { slidersite: Some("slider_base".into()), cranklength: 150.0, gear: gear([1e5, 0.0, 0.0]), ..on("crank_rod", TRN_SLIDERCRANK, "crank_pin") }, |a| a.set_to_motor()),
    ]);
    s
}

// DC motors: a voltage-driven one with cogging, and a geared, current-limited one with an
// electrical time constant, a position and velocity controller and a thermal model.
fn dc_motors() -> ModelSpec {
    let mut s = model("dc_motors");
    s.bodies.push(link("rotor", 0, [0.0, 0.0, 1000.0], JNT_HINGE));
    s.bodies.push(link("geared", 0, [300.0, 0.0, 1000.0], JNT_HINGE));
    s.actuators.extend([
        shortcut(on("drive", TRN_JOINT, "rotor_joint"), |a| a.set_to_dc_motor(&DcMotorSpec {
            motorconst: Some([3000.0, 3000.0]), resistance: 2.0, cogging: Some([100.0, 6.0, 0.0]), ..DcMotorSpec::default()
        })),
        shortcut(ActuatorSpec { gear: gear([2.0, 0.0, 0.0]), ..on("servo", TRN_JOINT, "geared_joint") }, |a| a.set_to_dc_motor(&DcMotorSpec {
            motorconst: Some([3000.0, 3000.0]), resistance: 2.0, saturation: Some([5e5, 0.0, 0.0]), inductance: Some([0.0, 0.02]),
            controller: Some([50.0, 0.0, 1.0, 0.0, 0.0, 0.0]), thermal: Some([2.0, 50.0, 0.0, 0.004, 25.0, 25.0]),
            ctrlspec: INPUT_POS | INPUT_VEL, ..DcMotorSpec::default()
        })),
    ]);
    s
}

fn run(out: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out)?;
    for spec in [chain(false), chain(true), slide_hinge(), free_box(), four_bar(), mocap_pendulum(), rings(), hull_box(), hull_ellipsoid(), hull_pair(), tendon_arm(), puppet_strings(), servo_arm(), site_actuators(), dc_motors()] {
        let name = spec.modelname.as_deref().unwrap();
        let path = out.join(format!("{name}.xml"));
        std::fs::write(&path, write_mjcf(&spec))?;
        let model = makepad_dynamics::builder::compile(&spec)
            .map_err(std::io::Error::other)?;
        std::fs::write(out.join(format!("{name}.ours.mjb")), makepad_dynamics::model_file::save_model(&model))?;
        println!("{}", path.display());
    }
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        eprintln!("usage: builder_parity <out dir>");
        std::process::exit(1);
    }
    if let Err(error) = run(Path::new(&args[0])) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
