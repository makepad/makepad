use makepad_dynamics::builder::compile;
use makepad_dynamics::data::Data;
use makepad_dynamics::forward::forward;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::name::name2id;
use makepad_dynamics::ray::{multi_ray, ray, NGROUP};
use makepad_dynamics::sdf::attach;
use makepad_dynamics::sdf_torus::Torus;
use makepad_dynamics::types::*;
use std::sync::Arc;

fn geom_id(m: &Model, name: &str) -> i32 {
    name2id(m, OBJ_GEOM, name)
}

/// The reference's ray casting model: a static sphere in group 1, a hinged sphere in group 0 and a
/// sphere in group 2 welded to the world, on the x axis at 1, 3 and 5.
fn casting() -> (Model, Data) {
    let sphere = |name: &str, x: f64, group: i32| GeomSpec {
        name: Some(name.into()), r#type: GEOM_SPHERE, size: [0.1, 0.0, 0.0], pos: [x, 0.0, 0.0], group, ..GeomSpec::default()
    };
    let mut s = ModelSpec::default();
    s.bodies[0].geoms.push(sphere("static_group1", 1.0, 1));
    s.bodies.push(BodySpec { name: Some("outer".into()), geoms: vec![sphere("group2", 5.0, 2)], ..BodySpec::default() });
    s.bodies.push(BodySpec {
        name: Some("inner".into()), parent: 1, mass: 1.0, ipos: [0.0; 3], inertia: [0.01; 3], joints: vec![JointSpec::default()],
        geoms: vec![sphere("group0", 3.0, 0)], ..BodySpec::default()
    });
    let m = compile(&s).expect("compile the ray casting model");
    let mut d = make_data(&m);
    forward(&m, &mut d);
    (m, d)
}

#[test]
fn ray_skips_geoms_by_group_static_body_and_visibility() {
    let (mut m, d) = casting();
    let [static_group1, group0, group2] = ["static_group1", "group0", "group2"].map(|n| geom_id(&m, n));
    let (pnt, vec) = ([0.0; 3], [1.0, 0.0, 0.0]);
    let cast = |m: &Model, groups: Option<&[u8; NGROUP]>, flg_static: bool, bodyexclude: i32| {
        let mut geomid = -2;
        let dist = ray(m, &d, &pnt, &vec, groups, flg_static, bodyexclude, Some(&mut geomid), None);
        (geomid, dist)
    };
    let expect = |(geomid, dist): (i32, f64), want: i32, x: f64, what: &str| {
        assert_eq!(geomid, want, "{what}: the hit geom");
        assert!((dist - x).abs() < 1e-12, "{what}: distance {dist}, expected {x}");
    };
    expect(cast(&m, None, true, -1), static_group1, 0.9, "no filter");
    let mut groups = [1u8; NGROUP];
    expect(cast(&m, Some(&groups), true, -1), static_group1, 0.9, "every group");
    groups[1] = 0;
    expect(cast(&m, Some(&groups), true, -1), group0, 2.9, "group 1 off");
    groups[0] = 0;
    expect(cast(&m, Some(&groups), true, -1), group2, 4.9, "groups 0 and 1 off");
    groups[2] = 0;
    expect(cast(&m, Some(&groups), true, -1), -1, -1.0, "groups 0 to 2 off");
    // the world's sphere and the one welded to it are static
    expect(cast(&m, None, false, -1), group0, 2.9, "static geoms off");
    let inner = name2id(&m, OBJ_BODY, "inner");
    expect(cast(&m, None, false, inner), -1, -1.0, "static geoms and the hinged body off");
    // a geom without a material is invisible at alpha 0
    m.geom_rgba[4*static_group1 as usize + 3] = 0.0;
    expect(cast(&m, None, true, -1), group0, 2.9, "the nearest sphere invisible");
}

const TORUS: [f64; 2] = [0.4, 0.12];

/// One geom of each kind a ray meets, each on its own free body in a row on y = 0 over an infinite
/// plane, with a field torus behind them facing -y.
fn scene() -> (Model, Data) {
    let mut s = ModelSpec::default();
    let c = 0.5f64.sqrt();
    s.bodies[0].geoms.push(GeomSpec { name: Some("plane".into()), r#type: GEOM_PLANE, size: [0.0, 0.0, 1.0], ..GeomSpec::default() });
    let h = 0.2f32;
    let vert: Vec<f32> = (0..8).flat_map(|v| [if v & 1 != 0 { h } else { -h }, if v & 2 != 0 { h } else { -h }, if v & 4 != 0 { h } else { -h }]).collect();
    s.meshes.push(MeshSpec { name: Some("cube".into()), vert, ..MeshSpec::default() });
    let field_box = GeomSpec { r#type: GEOM_BOX, size: [TORUS[0] + TORUS[1], TORUS[0] + TORUS[1], TORUS[1]], ..GeomSpec::default() };
    let geoms = [
        ("sphere", [-2.0, 0.0, 0.5], [1.0, 0.0, 0.0, 0.0], GeomSpec { r#type: GEOM_SPHERE, size: [0.25, 0.0, 0.0], ..GeomSpec::default() }),
        ("capsule", [-1.2, 0.0, 0.5], [c, c, 0.0, 0.0], GeomSpec { r#type: GEOM_CAPSULE, size: [0.15, 0.25, 0.0], ..GeomSpec::default() }),
        ("ellipsoid", [-0.4, 0.0, 0.5], [c, 0.0, 0.0, c], GeomSpec { r#type: GEOM_ELLIPSOID, size: [0.3, 0.2, 0.15], ..GeomSpec::default() }),
        ("cylinder", [0.4, 0.0, 0.5], [c, 0.0, c, 0.0], GeomSpec { r#type: GEOM_CYLINDER, size: [0.2, 0.25, 0.0], ..GeomSpec::default() }),
        ("box", [1.2, 0.0, 0.5], [0.9, 0.3, 0.2, 0.1], GeomSpec { r#type: GEOM_BOX, size: [0.25, 0.2, 0.15], ..GeomSpec::default() }),
        ("mesh", [2.0, 0.0, 0.5], [0.9, 0.1, 0.3, 0.2], GeomSpec { r#type: GEOM_MESH, meshname: Some("cube".into()), ..GeomSpec::default() }),
        ("torus", [0.0, 1.0, 1.3], [c, c, 0.0, 0.0], field_box),
    ];
    for (name, pos, quat, geom) in geoms {
        let mut quat = quat;
        let n = quat.iter().map(|q| q * q).sum::<f64>().sqrt();
        quat.iter_mut().for_each(|q| *q /= n);
        s.bodies.push(BodySpec {
            name: Some(name.into()), pos, quat, mass: 1.0, ipos: [0.0; 3], inertia: [0.01; 3],
            joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
            geoms: vec![GeomSpec { name: Some(name.into()), ..geom }],
            ..BodySpec::default()
        });
    }
    let mut m = compile(&s).expect("compile the ray scene");
    let torus = geom_id(&m, "torus") as usize;
    attach(&mut m, torus, Arc::new(Torus { radius: TORUS }));
    let mut d = make_data(&m);
    forward(&m, &mut d);
    (m, d)
}

#[test]
fn multi_ray_matches_ray_on_every_geom_type() {
    let (m, mut d) = scene();
    // a fan from in front of and above the row, and one ray of zero length
    let pnt = [0.0, -3.0, 2.0];
    let (na, nb) = (61, 41);
    let mut vec = Vec::new();
    for i in 0..na {
        for j in 0..nb {
            vec.extend([-0.8 + 1.6 * i as f64 / (na - 1) as f64, 1.0, -0.75 + 0.6 * j as f64 / (nb - 1) as f64]);
        }
    }
    vec.extend([0.0; 3]);
    let nray = vec.len() / 3;
    let mut dist = vec![0.0; nray];
    let mut geomid = vec![0; nray];
    let mut normal = vec![0.0; 3 * nray];
    multi_ray(&m, &mut d, &pnt, &vec, None, true, -1, Some(&mut geomid), &mut dist, Some(&mut normal), nray, MAXVAL);

    let mut hits = vec![0; m.ngeom];
    for r in 0..nray - 1 {
        let v: &[f64; 3] = vec[3 * r..3 * r + 3].try_into().unwrap();
        let (mut g, mut n) = (-2, [9.0; 3]);
        let x = ray(&m, &d, &pnt, v, None, true, -1, Some(&mut g), Some(&mut n));
        assert_eq!((x.to_bits(), g), (dist[r].to_bits(), geomid[r]), "ray {r}: the cast's distance and geom");
        assert_eq!(n, normal[3 * r..3 * r + 3], "ray {r}: the cast's normal");
        if g >= 0 {
            hits[g as usize] += 1;
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-9, "ray {r} on geom {g}: a unit normal, length {len}");
            assert!(n[0] * v[0] + n[1] * v[1] + n[2] * v[2] < 0.0, "ray {r} on geom {g}: the normal {n:?} faces the ray");
        }
    }
    for (g, &n) in hits.iter().enumerate() {
        assert!(n > 0, "geom {g} (type {}) is hit: {hits:?}", m.geom_type[g]);
    }
    let last = nray - 1;
    assert_eq!((dist[last], geomid[last], &normal[3 * last..]), (-1.0, -1, &[0.0; 3][..]), "a ray of zero length hits nothing");

    // with no reach, only the infinite plane is left, at the distances the full casts found for it
    let plane = geom_id(&m, "plane");
    let mut near_dist = vec![0.0; nray];
    let mut near_geom = vec![0; nray];
    multi_ray(&m, &mut d, &pnt, &vec, None, true, -1, Some(&mut near_geom), &mut near_dist, None, nray, 0.0);
    for r in 0..nray - 1 {
        assert!(near_geom[r] == plane || near_geom[r] == -1, "ray {r}: only the plane within reach, not geom {}", near_geom[r]);
        if geomid[r] == plane {
            assert_eq!((near_geom[r], near_dist[r]), (plane, dist[r]), "ray {r}: the plane at the same distance");
        }
    }

    // a field's distance scales with the ray's length
    let torus = geom_id(&m, "torus");
    let r = (0..nray - 1).find(|&r| geomid[r] == torus).expect("a ray on the torus");
    let v2 = [2.0 * vec[3 * r], 2.0 * vec[3 * r + 1], 2.0 * vec[3 * r + 2]];
    let mut g = -2;
    let x2 = ray(&m, &d, &pnt, &v2, None, true, -1, Some(&mut g), None);
    assert_eq!(g, torus, "the doubled ray meets the torus too");
    assert!((x2 - dist[r] / 2.0).abs() < 1e-5 * dist[r], "the doubled ray's distance {x2} is half of {}", dist[r]);
}
