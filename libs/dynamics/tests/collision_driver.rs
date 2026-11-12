use makepad_dynamics::collision_driver::{sap, Packed32};
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::{make_model, MODEL_SIZES};

#[test]
fn sap_six_boxes_in_c_order() {
    let mut sizes = vec![0_i64; MODEL_SIZES.len()];
    sizes[MODEL_SIZES.iter().position(|s| *s == "nbody").unwrap()] = 1;
    let mut m = make_model(&sizes).unwrap();
    m.body_mocapid[0] = -1;
    m.body_treeid[0] = -1;
    let mut d = make_data(&m);

    // Rows are boxes: xmin, ymin, zmin, xmax, ymax, zmax.
    // Boxes 3 and 4 overlap on x but are separated on y and z respectively.
    // Minima at x=2 and x=3 precede maxima at the same value in C's SAP sort.
    let aamm = [
        0.0, 0.0, 0.0, 2.0, 1.0, 1.0,
        1.0, 0.5, 0.0, 3.0, 1.5, 1.0,
        2.0, 0.0, 0.0, 4.0, 1.0, 1.0,
        0.5, 3.0, 0.0, 2.5, 4.0, 1.0,
        1.5, 0.0, 3.0, 2.5, 1.0, 4.0,
        3.0, 0.0, 0.0, 5.0, 1.0, 1.0,
    ];
    let mut pairs = [Packed32::default(); 15];
    let n = sap(&mut d, &aamm, 6, 0, &mut pairs, 15, |_, _| true);
    assert_eq!(n, 5, "overlaps include touching bounds and prune both secondary axes");
    assert_eq!(
        &pairs[..n as usize],
        &[
            Packed32 { hi: 0, lo: 1 },
            Packed32 { hi: 0, lo: 2 },
            Packed32 { hi: 1, lo: 2 },
            Packed32 { hi: 1, lo: 5 },
            Packed32 { hi: 2, lo: 5 },
        ],
        "pairs by the earlier start, then the later one"
    );
    let mut prefix = [Packed32::default(); 2];
    assert_eq!(sap(&mut d, &aamm, 6, 0, &mut prefix, 2, |_, _| true), 3, "overflow returns maxpair + 1");
    assert_eq!(&prefix, &pairs[..2], "overflow keeps the first pairs");
}
