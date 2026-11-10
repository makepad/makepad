use makepad_dynamics::builder::compile;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::types::*;

#[test]
fn hinge_chain_reference_topology() {
    let mut spec = ModelSpec { modelname: Some("hinge_chain".into()), ..ModelSpec::default() };
    spec.option.gravity = [0.0, 0.0, -9810.0];
    spec.option.timestep = 1.0 / 240.0;
    spec.bodies.push(BodySpec {
        name: Some("base".into()), pos: [0.0, 0.0, 600.0], ipos: [0.0; 3],
        mass: 1000.0, inertia: [10000.0; 3],
        geoms: vec![GeomSpec { size: [10.0; 3], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    for i in 0..5 {
        spec.bodies.push(BodySpec {
            name: Some(format!("link{i}")), parent: i+1,
            pos: [0.0, 0.0, if i == 0 { 0.0 } else { -100.0 }],
            mass: 100.0, ipos: [0.0, 0.0, -50.0], inertia: [83333.0, 83333.0, 1250.0],
            joints: vec![JointSpec {
                name: Some(format!("link{i}_joint")), axis: [0.0, 1.0, 0.0], ..JointSpec::default()
            }],
            geoms: vec![GeomSpec {
                name: Some(format!("link{i}_geom")), r#type: GEOM_CAPSULE,
                size: [5.0, 0.0, 0.0], density: 0.001,
                fromto: [0.0, 0.0, 0.0, 0.0, 0.0, -100.0], ..GeomSpec::default()
            }],
            ..BodySpec::default()
        });
    }
    spec.bodies[6].sites.push(SiteSpec {
        name: Some("tip".into()), pos: [0.0, 0.0, -100.0], size: [2.0; 3], ..SiteSpec::default()
    });
    let m = compile(&spec).expect("compile fixture (a)");
    assert_eq!((m.nbody, m.njnt, m.nv), (7, 5, 5), "reference body, joint and velocity counts");
    assert_eq!(m.body_dofadr, [-1, -1, 0, 1, 2, 3, 4], "reference body DOF addresses");
    assert_eq!(m.dof_parentid, [-1, 0, 1, 2, 3], "reference DOF ancestry");
    assert_eq!(m.geom_bodyid, [1, 2, 3, 4, 5, 6], "reference geom ownership");
}

#[test]
fn rings_reference_hierarchy() {
    let mut s = ModelSpec { modelname: Some("rings".into()), ..ModelSpec::default() };
    s.option.gravity = [0.0, 0.0, -9810.0];
    s.option.timestep = 1.0 / 240.0;
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
            let mut g = GeomSpec {
                name: Some(format!("{name}_segment{i}")), r#type: GEOM_CAPSULE,
                fromto: [a[0], a[1], a[2], z[0], z[1], z[2]],
                size: [5.0, 0.0, 0.0], density: 0.001, ..GeomSpec::default()
            };
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
    let m = compile(&s).expect("compile fixture (g)");
    assert_eq!((m.nbvh, m.nbvhstatic, m.nbvhdynamic), (31, 31, 0), "reference hierarchy sizes");
    assert_eq!(m.body_bvhadr, [0, 1, 16], "reference body hierarchy addresses");
    assert_eq!(m.body_bvhnum, [1, 15, 15], "reference body hierarchy node counts");
    assert_eq!((m.bvh_aabb.len(), m.bvh_child.len(), m.bvh_depth.len(), m.bvh_nodeid.len()),
        (186, 62, 31, 31), "reference hierarchy array sizes");
    for body in 0..m.nbody {
        let adr = m.body_bvhadr[body] as usize;
        let num = m.body_bvhnum[body] as usize;
        let mut geoms: Vec<_> = m.bvh_nodeid[adr..adr+num].iter().copied().filter(|&g| g >= 0).collect();
        geoms.sort_unstable();
        let first = m.body_geomadr[body];
        assert_eq!(geoms, (first..first+m.body_geomnum[body]).collect::<Vec<_>>(),
            "every collision geom appears once in its body's hierarchy");
        assert_eq!(m.bvh_depth[adr], 0, "each body hierarchy starts at depth zero");
    }
}

#[test]
fn convex_mesh_reference_counts() {
    let mut spec = ModelSpec { modelname: Some("hull_box".into()), ..ModelSpec::default() };
    let mut vert = Vec::new();
    for i in 0..8 {
        vert.extend([if i & 1 != 0 { 40.0 } else { 0.0 }, if i & 2 != 0 { 30.0 } else { 0.0 }, if i & 4 != 0 { 20.0 } else { 0.0 }]);
    }
    spec.meshes.push(MeshSpec { name: Some("block".into()), vert, ..MeshSpec::default() });
    spec.bodies.push(BodySpec {
        name: Some("block".into()), pos: [0.0, 0.0, 60.0], ipos: [0.0; 3], mass: 80.0,
        inertia: [8666.0, 13333.0, 16666.0],
        joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
        geoms: vec![GeomSpec { r#type: GEOM_MESH, meshname: Some("block".into()), ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    let m = compile(&spec).expect("compile fixture (h)");
    // the reference compiler's counts for a box hull: 8 corners, 12 triangles, and a graph of
    // 2 + 3 * 8 + 6 * 12 ints
    assert_eq!((m.nmesh, m.nmeshvert, m.nmeshface, m.nmeshgraph), (1, 8, 12, 98), "box hull counts");
    assert_eq!(m.geom_dataid, [0], "the geom's mesh");
    // the mesh frame is its centre of mass, which the geom's pose takes up
    for (got, want) in m.geom_pos.iter().zip([20.0, 15.0, 10.0]) {
        assert!((got - want).abs() < 1e-9, "geom pose through the mesh frame: {:?}", m.geom_pos);
    }
}

// Strings from a bar to a free torso and its arm, one led over a cylinder with a side site, one
// through a pulley, and a fixed tendon coupling the bar to the arm (millimetres).
fn strung_puppet() -> ModelSpec {
    let mut s = ModelSpec { modelname: Some("strung_puppet".into()), ..ModelSpec::default() };
    s.option.gravity = [0.0, 0.0, -9810.0];
    let site = |name: &str, pos: [f64; 3]| SiteSpec { name: Some(name.into()), pos, ..SiteSpec::default() };
    let capsule = || GeomSpec { r#type: GEOM_CAPSULE, size: [5.0, 0.0, 0.0], fromto: [0.0, 0.0, 0.0, 0.0, 0.0, -100.0], density: 0.001, ..GeomSpec::default() };
    let body = |name: &str, parent: usize, pos: [f64; 3], joint: i32, axis: [f64; 3]| BodySpec {
        name: Some(name.into()), parent, pos, mass: 100.0, ipos: [0.0; 3], inertia: [80000.0, 80000.0, 1500.0],
        joints: vec![JointSpec { name: Some(format!("{name}_joint")), r#type: joint, axis, ..JointSpec::default() }],
        geoms: vec![capsule()],
        ..BodySpec::default()
    };
    s.bodies[0].sites.push(site("hook", [600.0, 0.0, 900.0]));
    let mut bar = body("bar", 0, [0.0, 0.0, 1000.0], JNT_HINGE, [1.0, 0.0, 0.0]);
    bar.sites.extend([site("bar_l", [-150.0, 0.0, 0.0]), site("bar_r", [150.0, 0.0, 0.0])]);
    let mut torso = body("torso", 0, [0.0, 0.0, 300.0], JNT_FREE, [0.0, 0.0, 1.0]);
    torso.sites.push(site("head", [0.0, 0.0, 20.0]));
    let mut arm = body("arm", 2, [70.0, 0.0, -10.0], JNT_HINGE, [0.0, 1.0, 0.0]);
    arm.geoms.push(GeomSpec { name: Some("wheel".into()), r#type: GEOM_CYLINDER, size: [20.0, 10.0, 0.0], contype: 0, conaffinity: 0, ..GeomSpec::default() });
    arm.sites.extend([site("side", [30.0, 0.0, 0.0]), site("hand", [0.0, 0.0, -100.0])]);
    s.bodies.extend([bar, torso, arm]);
    let path = |wraps: &[WrapSpec]| wraps.to_vec();
    s.tendons = vec![
        TendonSpec { name: Some("head".into()), range: [0.0, 700.0], stiffness: [10.0, 0.0, 0.0], springyield: 50.0,
            path: path(&[WrapSpec::site("bar_l"), WrapSpec::site("head")]), ..TendonSpec::default() },
        TendonSpec { name: Some("hand".into()), armature: 1.0,
            path: path(&[WrapSpec::site("bar_r"), WrapSpec::site("hand")]), ..TendonSpec::default() },
        TendonSpec { name: Some("over".into()),
            path: path(&[WrapSpec::site("hook"), WrapSpec::geom("wheel", Some("side")), WrapSpec::site("hand")]), ..TendonSpec::default() },
        TendonSpec { name: Some("pulley".into()),
            path: path(&[WrapSpec::site("bar_l"), WrapSpec::site("head"), WrapSpec::pulley(2.0), WrapSpec::site("bar_r"), WrapSpec::site("hand")]), ..TendonSpec::default() },
        TendonSpec { name: Some("coupling".into()),
            path: path(&[WrapSpec::joint("bar_joint", 1.0), WrapSpec::joint("arm_joint", -0.5)]), ..TendonSpec::default() },
    ];
    s
}

#[test]
fn tendon_paths_and_refusals() {
    let spec = strung_puppet();
    let m = compile(&spec).expect("compile the strung puppet");
    // Dofs: bar 0, torso 1..7, arm 7. Sites: hook 0, bar_l 1, bar_r 2, head 3, side 4, hand 5.
    assert_eq!((m.ntendon, m.nwrap, m.nv), (5, 14, 8), "tendon, wrap and dof counts");
    assert_eq!(m.tendon_type, [TENDON_SPATIAL, TENDON_SPATIAL, TENDON_SPATIAL, TENDON_SPATIAL, TENDON_FIXED], "joints only make a fixed tendon");
    assert_eq!((m.tendon_adr.as_slice(), m.tendon_num.as_slice()), ([0, 2, 4, 7, 12].as_slice(), [2, 2, 3, 5, 2].as_slice()), "wrap addresses");
    assert_eq!(m.wrap_type, [WRAP_SITE, WRAP_SITE, WRAP_SITE, WRAP_SITE, WRAP_SITE, WRAP_CYLINDER, WRAP_SITE,
        WRAP_SITE, WRAP_SITE, WRAP_PULLEY, WRAP_SITE, WRAP_SITE, WRAP_JOINT, WRAP_JOINT], "a geom wrap takes the geom's shape");
    assert_eq!(m.wrap_objid, [1, 3, 2, 5, 0, 3, 5, 1, 3, -1, 2, 5, 0, 2], "wrap objects");
    assert_eq!(m.wrap_prm, [0.0, 0.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0, -0.5], "side site, divisor and coefficients");
    assert_eq!(m.ten_j_rownnz, [7, 8, 7, 8, 2], "Jacobian nonzeros: the dofs above each spatial path, one per fixed joint");
    assert_eq!(m.tendon_limited, [1, 0, 0, 0, 0], "a range limits a tendon");
    assert_eq!((m.nplastic, m.tendon_plasticadr[0], m.tendon_plasticadr[1]), (2, 0, -1), "a yielding tendon takes two plastic slots");
    let rest = (150.0f64 * 150.0 + 680.0 * 680.0).sqrt();
    assert!((m.tendon_lengthspring[0] - rest).abs() < 1e-9 && m.tendon_lengthspring[2..4] == [m.tendon_length0[1]; 2], "an unset spring length is the length at rest");
    assert!(m.body_simple[1] == 0 && m.body_simple[3] == 0, "tendon armature makes its bodies non-simple");
    let mut plain = spec.clone();
    plain.tendons[1].armature = 0.0;
    assert_ne!(compile(&plain).unwrap().body_simple[1], 0, "without armature the bar on its hinge is simple");

    let refuse = |edit: &dyn Fn(&mut ModelSpec), expected: &str| {
        let mut s = spec.clone();
        edit(&mut s);
        let err = compile(&s).err().unwrap_or_default();
        assert!(err.contains(expected), "expected a refusal with '{expected}', got '{err}'");
    };
    refuse(&|s| s.tendons[0].path.clear(), "path cannot be empty");
    refuse(&|s| s.tendons[0].path[1] = WrapSpec::site("nowhere"), "site 'nowhere' not found");
    refuse(&|s| s.tendons[2].path[1] = WrapSpec::geom("bar_geom", None), "geom 'bar_geom' not found");
    refuse(&|s| { s.bodies[1].geoms[0].name = Some("bar_geom".into()); s.tendons[2].path[1] = WrapSpec::geom("bar_geom", None); }, "is not sphere or cylinder");
    refuse(&|s| s.tendons[3].path.push(WrapSpec::pulley(1.0)), "path ends with pulley");
    refuse(&|s| { s.tendons[2].path.remove(0); }, "not bracketed by sites");
    refuse(&|s| s.tendons[2].armature = 1.0, "geom wrapping not supported by tendon armature");
    refuse(&|s| s.tendons[4].path[0] = WrapSpec::joint("torso_joint", 1.0), "must be slide or hinge");
    refuse(&|s| { s.tendons[1].limited = 1; }, "invalid limits in tendon");
    refuse(&|s| s.tendons[0].path.truncate(1), "spatial path must contain at least two objects");
}

#[test]
fn actuator_blocks_and_refusals() {
    let mut spec = strung_puppet();
    spec.bodies[3].joints[0].range = [-1.0, 1.0];
    let on = |name: &str, trntype: i32, target: &str| ActuatorSpec { name: Some(name.into()), ..ActuatorSpec::on(trntype, target) };
    let mut pid = ActuatorSpec { velrange: [-5.0, 5.0], ..on("pid", TRN_JOINT, "arm_joint") };
    pid.set_to_pid(1e5, Some(2e3), None, Some(1e5), Some(1e3), Some(100.0), 0.0, INPUT_POS | INPUT_VEL | INPUT_FF).unwrap();
    let mut servo = on("servo", TRN_JOINT, "arm_joint");
    servo.set_to_position(1e6, Some(1e4), None, None, 0.5).unwrap();
    let mut orient = ActuatorSpec { refsite: Some("hook".into()), ..on("orient", TRN_SITE, "head") };
    orient.set_to_orientation(1e6, Some(1e4), None, 0).unwrap();
    spec.actuators = vec![pid, servo, orient];
    let m = compile(&spec).expect("compile the actuated puppet");
    assert_eq!((m.nactuator, m.nu, m.nout, m.na), (3, 7, 5, 2), "pid [pos vel ff] with slew and integral states, a position servo, an expmap SO3 servo");
    assert_eq!((m.actuator_ctrladr.as_slice(), m.actuator_outadr.as_slice(), m.actuator_actadr.as_slice()), ([0, 3, 4].as_slice(), [0, 1, 2].as_slice(), [0, -1, -1].as_slice()), "input, output and activation blocks");
    assert_eq!(m.actuator_trntype, [TRN_JOINT, TRN_JOINT, TRN_SO3], "an SO3 servo on a site compiles to the SO3 transmission");
    assert_eq!((&m.actuator_ctrlrange[2..6], &m.actuator_ctrllimited[1..3]), ([-5.0, 5.0, 0.0, 0.0].as_slice(), [1, 0].as_slice()), "pid velocity input takes velrange; an empty ffrange leaves ff unlimited");
    assert_eq!(&m.actuator_ctrlrange[6..8], [-0.5, 0.5], "inheritrange 0.5 halves the joint range");
    assert_eq!(m.n_jmom, 1 + 1 + 3 * m.nv, "moment non-zeros: one per hinge row, three dense rows for a site against a reference site");

    let refuse = |edit: &dyn Fn(&mut ModelSpec), expected: &str| {
        let mut s = spec.clone();
        edit(&mut s);
        let err = compile(&s).err().unwrap_or_default();
        assert!(err.contains(expected), "expected a refusal with '{expected}', got '{err}'");
    };
    refuse(&|s| s.actuators[1].gaintype = GAIN_MUSCLE, "muscles are outside the builder subset");
    refuse(&|s| s.actuators[1].nsample = 4, "control history");
    refuse(&|s| s.actuators[0].target = Some("nowhere".into()), "unknown transmission target 'nowhere'");
    refuse(&|s| s.actuators[1].target = Some("bar_joint".into()), "has no range defined");
    refuse(&|s| s.actuators[2].refsite = None, "so3 site transmission requires refsite");
}
