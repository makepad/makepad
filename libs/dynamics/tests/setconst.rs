use makepad_dynamics::io::make_data;
use makepad_dynamics::model::{make_model, MODEL_SIZES};
use makepad_dynamics::setconst::{set_const, set_const_groups, SCGROUP_FIXED};
use makepad_dynamics::types::{GEOM_BOX, GEOM_CAPSULE, GEOM_SPHERE};

#[test]
fn bounds_and_empty_world_constants() {
    let mut sizes = vec![0; MODEL_SIZES.len()];
    sizes[MODEL_SIZES.iter().position(|&name| name == "nbody").unwrap()] = 1;
    let mut m = make_model(&sizes).unwrap();
    m.body_treeid[0] = -1;
    m.body_mocapid[0] = -1;
    let mut d = make_data(&m);
    // Dirty derived fields so this verifies that the C assignments run.
    m.body_invweight0.fill(7.0);
    m.body_subtreemass[0] = 7.0;
    m.body_contype[0] = 7;
    m.body_conaffinity[0] = 7;
    m.body_margin[0] = 7.0;
    set_const(&mut m, &mut d);
    assert_eq!(m.body_invweight0, [0.0, 0.0], "world inverse weights");
    assert_eq!(m.body_subtreemass, [0.0], "empty world subtree mass");
    assert_eq!(m.body_treeid, [-1], "world has no dynamic tree");
    assert_eq!(m.body_contype, [0], "empty world collision type");
    assert_eq!(m.body_conaffinity, [0], "empty world collision affinity");
    assert_eq!(m.body_margin, [0.0], "empty world collision margin");
    assert_eq!(d.xpos, [0.0; 3], "world position");
    assert_eq!(d.xquat, [1.0, 0.0, 0.0, 0.0], "world orientation");
    assert_eq!(d.xipos, [0.0; 3], "world inertial position");
    assert_eq!(d.xiquat, [1.0, 0.0, 0.0, 0.0], "world inertial orientation");
    assert_eq!(d.xmat, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], "world rotation matrix");

    sizes[MODEL_SIZES.iter().position(|&name| name == "ngeom").unwrap()] = 3;
    let mut m = make_model(&sizes).unwrap();
    m.body_treeid[0] = -1;
    m.body_mocapid[0] = -1;
    let mut d = make_data(&m);
    m.body_geomnum[0] = 3;
    m.body_bvhadr[0] = -1;
    m.geom_type.copy_from_slice(&[GEOM_SPHERE, GEOM_CAPSULE, GEOM_BOX]);
    m.geom_size.copy_from_slice(&[2.0, 0.0, 0.0, 2.0, 3.0, 0.0, 1.0, 2.0, 2.0]);
    set_const_groups(&mut m, &mut d, SCGROUP_FIXED);
    // C: sphere r, capsule r+h, box sqrt(x*x+y*y+z*z) = sqrt(9).
    assert_eq!(m.geom_rbound, [2.0, 5.0, 3.0], "primitive bounding radii");
    assert_eq!(m.geom_aabb, [0.0, 0.0, 0.0, 2.0, 2.0, 2.0,
        0.0, 0.0, 0.0, 2.0, 2.0, 5.0,
        0.0, 0.0, 0.0, 1.0, 2.0, 2.0], "primitive box centers and half sizes");
}
