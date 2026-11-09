use makepad_dynamics::model::{make_model, MODEL_SIZES};
use makepad_dynamics::sensor::apply_cutoff;
use makepad_dynamics::types::{DATATYPE_POSITIVE, DATATYPE_REAL, SENS_JOINTPOS};

#[test]
fn cutoff_clips_real_both_sides_and_positive_only_above() {
    let mut sizes = vec![0_i64; MODEL_SIZES.len()];
    sizes[MODEL_SIZES.iter().position(|s| *s == "nbody").unwrap()] = 1;
    sizes[MODEL_SIZES.iter().position(|s| *s == "nsensor").unwrap()] = 1;
    let mut m = make_model(&sizes).unwrap();
    m.sensor_type[0] = SENS_JOINTPOS;
    m.sensor_dim[0] = 5;
    m.sensor_cutoff[0] = 2.0;
    m.sensor_datatype[0] = DATATYPE_REAL;
    let mut real = [-3.0, -2.0, 0.5, 2.0, 3.0];
    apply_cutoff(&m, 0, &mut real);
    assert_eq!(real, [-2.0, -2.0, 0.5, 2.0, 2.0], "real values clip at both bounds");
    m.sensor_datatype[0] = DATATYPE_POSITIVE;
    let mut positive = [-3.0, -2.0, 0.5, 2.0, 3.0];
    apply_cutoff(&m, 0, &mut positive);
    assert_eq!(positive, [-3.0, -2.0, 0.5, 2.0, 2.0], "positive data caps above and preserves negative values");
}
