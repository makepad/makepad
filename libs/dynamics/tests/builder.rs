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
