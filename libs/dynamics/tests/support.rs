use makepad_dynamics::model::{make_model, MODEL_SIZES};
use makepad_dynamics::support::{differentiate_pos, integrate_pos, normalize_quat};
use makepad_dynamics::types::{JNT_BALL, JNT_FREE, JNT_HINGE, JNT_SLIDE};

#[test]
fn position_round_trip_and_quaternion_normalization() {
    let mut sizes = vec![0_i64; MODEL_SIZES.len()];
    for (name, value) in [("nbody", 5), ("njnt", 4), ("nq", 13), ("nv", 11)] {
        sizes[MODEL_SIZES.iter().position(|s| *s == name).unwrap()] = value;
    }
    let mut m = make_model(&sizes).unwrap();
    // Free: 7 positions/6 velocities; ball: 4/3; hinge and slide: 1/1 each.
    m.jnt_type.copy_from_slice(&[JNT_FREE, JNT_BALL, JNT_HINGE, JNT_SLIDE]);
    m.jnt_qposadr.copy_from_slice(&[0, 7, 11, 12]);
    m.jnt_dofadr.copy_from_slice(&[0, 6, 9, 10]);
    m.body_jntadr.copy_from_slice(&[-1, 0, 1, 2, 3]);
    m.body_jntnum.copy_from_slice(&[0, 1, 1, 1, 1]);

    let qpos = [0.25, -0.5, 0.75, 1.0, 0.0, 0.0, 0.0, 0.5, -0.5, 0.5, 0.5, 0.3, -0.2];
    let qvel = [0.2, -0.3, 0.4, -0.5, 0.6, 0.7, 0.8, -0.9, 1.0, -1.1, 1.2];
    let dt = 0.01;
    let mut integrated = qpos;
    integrate_pos(&m, &mut integrated, &qvel, dt);
    let mut differentiated = [0.0; 11];
    differentiate_pos(&m, &mut differentiated, dt, &qpos, &integrated);
    for i in 0..m.nv {
        assert!((differentiated[i] - qvel[i]).abs() < 1e-12, "velocity round trip at dof {i}: {} != {}", differentiated[i], qvel[i]);
    }

    let mut normalized = qpos;
    normalize_quat(&m, &mut normalized);
    assert_eq!(&normalized[3..7], &[1.0, 0.0, 0.0, 0.0], "identity quaternion stays exact");
    for adr in [3, 7] {
        let mut norm2 = 0.0;
        for i in 0..4 {
            norm2 += normalized[adr + i] * normalized[adr + i];
        }
        assert!((norm2 - 1.0).abs() < 1e-12, "quaternion at {adr} stays unit");
    }
}
