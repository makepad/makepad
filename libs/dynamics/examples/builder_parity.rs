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

fn run(out: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out)?;
    for spec in [chain(false), chain(true), slide_hinge(), free_box(), four_bar(), mocap_pendulum(), rings(), hull_box(), hull_ellipsoid(), hull_pair()] {
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
