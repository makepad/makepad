use makepad_dynamics::collision_box::{box_box, sphere_box};
use makepad_dynamics::collision_primitive::{plane_capsule, plane_sphere, sphere_sphere};
use makepad_dynamics::data::PreContact;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::{make_model, MODEL_SIZES};
use makepad_dynamics::types::{GEOM_BOX, GEOM_CAPSULE, GEOM_PLANE, GEOM_SPHERE};

#[test]
fn analytic_contact_depth_position_and_frame() {
    let mut sizes = vec![0_i64; MODEL_SIZES.len()];
    sizes[MODEL_SIZES.iter().position(|s| *s == "nbody").unwrap()] = 1;
    sizes[MODEL_SIZES.iter().position(|s| *s == "ngeom").unwrap()] = 4;
    let mut m = make_model(&sizes).unwrap();
    m.body_mocapid[0] = -1;
    m.body_treeid[0] = -1;
    m.geom_type
        .copy_from_slice(&[GEOM_PLANE, GEOM_SPHERE, GEOM_CAPSULE, GEOM_BOX]);
    m.geom_size
        .copy_from_slice(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5, 1.0, 0.0, 1.0, 1.0, 1.0]);
    let mut d = make_data(&m);
    let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    for g in 0..4 {
        d.geom_xmat[9 * g..9 * g + 9].copy_from_slice(&identity);
    }
    d.geom_xpos.fill(0.0);
    let mut con = [PreContact::default(); 12];

    // Sphere bottom is -1/4; its midpoint with z=0 is -1/8.
    d.geom_xpos[5] = 0.75;
    assert_eq!(plane_sphere(&m, &mut d, &mut con, 0, 1, 0.0), 1);
    assert_eq!(con[0].dist, -0.25, "sphere-plane depth");
    assert_eq!(con[0].pos, [0.0, 0.0, -0.125], "sphere-plane midpoint");
    assert_eq!(con[0].normal, [0.0, 0.0, 1.0], "sphere-plane frame normal");
    assert_eq!(con[0].tangent, [0.0; 3], "sphere-plane unspecified tangent");

    // Horizontal capsule endpoints at (+/-1, 0, 1/4), radius 1/2.
    d.geom_xpos[8] = 0.25;
    d.geom_xmat[18..27].copy_from_slice(&[0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0]);
    assert_eq!(plane_capsule(&m, &mut d, &mut con, 0, 2, 0.0), 2);
    for (k, x) in [1.0, -1.0].into_iter().enumerate() {
        assert_eq!(con[k].dist, -0.25, "capsule-plane depth {k}");
        assert_eq!(con[k].pos, [x, 0.0, -0.125], "capsule-plane midpoint {k}");
        assert_eq!(con[k].normal, [0.0, 0.0, 1.0], "capsule-plane normal {k}");
        assert_eq!(con[k].tangent, [1.0, 0.0, 0.0], "capsule axis tangent {k}");
    }

    // Reuse the capsule slot as a sphere of radius 1/2: centers 5/4 apart,
    // penetration 1/4, midpoint 1 - 1/8 = 7/8 from the first center.
    m.geom_type[2] = GEOM_SPHERE;
    d.geom_xpos[6..9].copy_from_slice(&[1.25, 0.0, 0.75]);
    assert_eq!(sphere_sphere(&m, &mut d, &mut con, 1, 2, 0.0), 1);
    assert_eq!(con[0].dist, -0.25, "sphere-sphere depth");
    assert_eq!(con[0].pos, [0.875, 0.0, 0.75], "sphere-sphere midpoint");
    assert_eq!(con[0].normal, [1.0, 0.0, 0.0], "sphere-sphere normal");
    assert_eq!(
        con[0].tangent, [0.0; 3],
        "sphere-sphere unspecified tangent"
    );

    // Sphere's leftmost point is 3/4; box face is x=1, midpoint x=7/8.
    d.geom_xpos[3..6].copy_from_slice(&[1.75, 0.0, 0.0]);
    assert_eq!(sphere_box(&m, &mut d, &mut con, 1, 3, 0.0), 1);
    assert_eq!(con[0].dist, -0.25, "sphere-box depth");
    assert_eq!(con[0].pos, [0.875, 0.0, 0.0], "sphere-box midpoint");
    assert_eq!(con[0].normal, [-1.0, 0.0, 0.0], "normal points toward box");
    assert_eq!(con[0].tangent, [0.0; 3], "sphere-box unspecified tangent");
    d.geom_xpos[3] = 2.25;
    assert_eq!(
        sphere_box(&m, &mut d, &mut con, 1, 3, 0.0),
        0,
        "separated sphere-box"
    );

    // Reuse the first slot as a unit box. Faces x=1 and x=1/2 overlap
    // by 1/2; the four contact positions have x=3/4, y,z=+/-1.
    m.geom_type[0] = GEOM_BOX;
    m.geom_size[..3].copy_from_slice(&[1.0; 3]);
    d.geom_xpos[9] = 1.5;
    assert_eq!(
        box_box(&m, &mut d, &mut con, 0, 3, 0.0),
        4,
        "box face manifold"
    );
    for (k, (y, z)) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]
        .into_iter()
        .enumerate()
    {
        assert_eq!(con[k].dist, -0.5, "box-box depth {k}");
        assert_eq!(con[k].pos, [0.75, y, z], "box-box midpoint {k}");
        assert_eq!(con[k].normal, [1.0, 0.0, 0.0], "box-box normal {k}");
        assert_eq!(con[k].tangent, [0.0; 3], "box-box unspecified tangent {k}");
    }
}
