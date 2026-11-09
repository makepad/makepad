use makepad_dynamics::collision_gjk::{
    ccd, CcdBuffer, CcdConfig, CcdObj, CcdObjData, CcdStatus, CcdSupport, EPA_SUCCESS, MAX_LIMIT,
};
use makepad_dynamics::types::GEOM_BOX;
use std::cell::Cell;

fn center(res: &mut [f64; 3], obj: &CcdObj<'_>) {
    *res = obj.pos;
}

fn unit_box(x: f64) -> CcdObj<'static> {
    CcdObj {
        geom: 0,
        geom_type: GEOM_BOX,
        size: [1.0, 1.0, 1.0, 0.0],
        pos: [x, 0.0, 0.0],
        mat: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        vertindex: Cell::new(0),
        meshindex: Cell::new(0),
        flex: -1,
        elem: -1,
        vert: -1,
        margin: 0.0,
        rotate: [1.0, 0.0, 0.0, 0.0],
        suppvert: [0.0; 12],
        suppind: [0; 4],
        nsupp: 0,
        data: CcdObjData::default(),
        center,
        support: CcdSupport::Box,
    }
}

#[test]
fn convex_distance_and_epa_penetration() {
    let mut a = unit_box(0.0);
    let mut b = unit_box(3.0);
    let mut buffer = CcdBuffer::default();
    let mut config = CcdConfig {
        max_iterations: 100,
        tolerance: 1e-12,
        max_contacts: 4,
        dist_cutoff: MAX_LIMIT,
        buffer: Some(&mut buffer),
        npolygonmax: 0,
        nmeshdegmax: 0,
    };
    let mut status = CcdStatus::default();
    let distance = ccd(&mut config, &mut status, &mut a, &mut b);
    assert!(
        (distance - 1.0).abs() < 1e-12,
        "separated box distance: {distance}"
    );
    b.pos[0] = 1.5;
    let distance = ccd(&mut config, &mut status, &mut a, &mut b);
    assert_eq!(status.epa_status, EPA_SUCCESS, "penetration must use EPA");
    assert!(
        (distance + 0.5).abs() < 1e-9,
        "EPA signed depth: {distance}"
    );
    assert_eq!(status.nx, 4, "aligned box faces produce four contacts");
    for i in 0..status.nx {
        assert!(
            (status.dist[i] + 0.5).abs() < 1e-9,
            "manifold depth at contact {i}"
        );
        assert!(
            (status.x1[3 * i] - 1.0).abs() < 1e-9,
            "first box witness at contact {i}"
        );
        assert!(
            (status.x2[3 * i] - 0.5).abs() < 1e-9,
            "second box witness at contact {i}"
        );
    }
}
