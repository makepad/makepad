use makepad_dynamics::collision_convex::{box_support, convex, ellipsoid_support, init_ccd_obj};
use makepad_dynamics::collision_gjk::CcdObj;
use makepad_dynamics::data::PreContact;
use makepad_dynamics::io::make_data;
use makepad_dynamics::memory::Scratch;
use makepad_dynamics::model::{make_model, MODEL_SIZES};
use makepad_dynamics::types::{GEOM_BOX, GEOM_CAPSULE, GEOM_ELLIPSOID, MAXCONPAIR};

#[test]
fn axis_supports_and_capsule_box_overlap() {
    let mut sizes = vec![0; MODEL_SIZES.len()];
    sizes[MODEL_SIZES.iter().position(|&s| s == "nbody").unwrap()] = 1;
    sizes[MODEL_SIZES.iter().position(|&s| s == "ngeom").unwrap()] = 3;
    let mut m = make_model(&sizes).unwrap();
    m.body_treeid.fill(-1);
    m.body_mocapid.fill(-1);
    m.body_quat[0] = 1.0;
    m.geom_type.copy_from_slice(&[GEOM_BOX, GEOM_ELLIPSOID, GEOM_CAPSULE]);
    m.geom_dataid.fill(-1);
    m.geom_size.copy_from_slice(&[1.0, 1.0, 1.0, 1.0, 2.0, 4.0, 0.5, 0.25, 0.0]);
    m.geom_rbound.copy_from_slice(&[3.0_f64.sqrt(), 4.0, 0.75]);
    m.opt.ccd_iterations = 100;
    m.opt.ccd_tolerance = 1e-12;
    let mut d = make_data(&m);
    for mat in d.geom_xmat.chunks_exact_mut(9) {
        mat.copy_from_slice(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }
    d.geom_xpos[6] = 1.25;
    let mut box_obj = CcdObj::default();
    let mut ellipsoid_obj = CcdObj::default();
    init_ccd_obj(&mut box_obj, &m, Some(&d), 0, 0.0);
    init_ccd_obj(&mut ellipsoid_obj, &m, Some(&d), 1, 0.0);
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let mut dir = [0.0; 3];
            dir[axis] = sign;
            let mut res = [0.0; 3];
            let mut corner = [1.0; 3];
            corner[axis] = sign;
            box_support(&mut res, &box_obj, &dir);
            assert_eq!(res, corner, "box support axis {axis}, sign {sign}");
            let mut extreme = [0.0; 3];
            extreme[axis] = sign * [1.0, 2.0, 4.0][axis];
            ellipsoid_support(&mut res, &ellipsoid_obj, &dir);
            assert_eq!(res, extreme, "ellipsoid support axis {axis}, sign {sign}");
        }
    }
    let mut contacts = [PreContact::default(); MAXCONPAIR];
    // Repeat with the same scratch to exercise the returned CCD buffer.
    let mut scratch = Scratch::default();
    for pass in 0..2 {
        let ncon = convex(&m, &d, &mut scratch, &mut contacts, 2, 0, 0.0);
        assert!(ncon > 0, "capsule-box overlap on pass {pass}");
        for contact in &contacts[..ncon] {
            assert!((contact.dist + 0.25).abs() < 1e-9,
                "capsule-box signed depth on pass {pass}: {}", contact.dist);
        }
    }
}
