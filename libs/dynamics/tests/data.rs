use makepad_dynamics::data_fields;
use makepad_dynamics::io::{
    make_b_sparse, make_data, make_dof_dof_maps, make_dof_dof_sparse, reset_data, reset_data_debug,
    reset_data_keyframe,
};
use makepad_dynamics::model::{make_model, MODEL_SIZES};
use makepad_dynamics::types::*;
use makepad_dynamics::util_errmem::{warning, warning_text_of};

macro_rules! check_lengths {
    (($m:ident, $d:ident) arrays { $($name:ident: $ty:ident [$nr:ident, $nc:literal],)* }) => {
        $(assert_eq!($d.$name.len(), $m.$nr * $nc, stringify!($name));)*
    };
}

#[test]
fn data_creation_reset_and_keyframe() {
    let mut sizes = vec![1_i64; MODEL_SIZES.len()];
    for (name, value) in [
        ("nq", 2),
        ("nv", 2),
        ("nbody", 2),
        ("njnt", 2),
        ("n_m", 3),
        ("n_c", 3),
        ("n_d", 4),
        ("n_b", 4),
        ("nplastic", 2),
        ("nu", 4),
        ("nhistory", 18),
        ("nplugin", 0),
        ("npluginstate", 0),
    ] {
        sizes[MODEL_SIZES.iter().position(|s| *s == name).unwrap()] = value;
    }
    let mut m = make_model(&sizes).unwrap();
    // Compiler-populated sizes are not arguments to mj_makeModel.
    m.nhistory = 18;
    m.qpos0.copy_from_slice(&[0.25, -0.5]);
    m.qpos_spring.copy_from_slice(&[0.75, -0.25]);
    m.jnt_type.fill(JNT_HINGE);
    m.jnt_qposadr.copy_from_slice(&[0, 1]);
    m.jnt_plasticadr.copy_from_slice(&[0, 1]);
    m.tendon_plasticadr.fill(-1);
    m.flex_plasticadr.fill(-1);
    m.body_parentid.copy_from_slice(&[0, 0]);
    m.body_rootid.copy_from_slice(&[0, 1]);
    m.body_treeid.copy_from_slice(&[-1, 0]);
    m.body_mocapid.copy_from_slice(&[0, -1]);
    m.body_pos[..3].copy_from_slice(&[1.0, 2.0, 3.0]);
    m.body_quat[..4].copy_from_slice(&[1.0, 0.0, 0.0, 0.0]);
    m.body_dofnum.copy_from_slice(&[0, 2]);
    m.body_dofadr.copy_from_slice(&[-1, 0]);
    m.dof_parentid.copy_from_slice(&[-1, 0]);
    m.dof_bodyid.fill(1);
    m.dof_madr.copy_from_slice(&[0, 1]);
    m.eq_active0.fill(1);
    m.signature = 123;
    m.actuator_gaintype.fill(GAIN_SO3);
    m.actuator_ctrlspec.fill(CHART_QUAT);
    m.actuator_ctrlnum.fill(4);
    m.actuator_history.copy_from_slice(&[2, 0]);
    m.sensor_history.copy_from_slice(&[2, 0]);
    m.sensor_historyadr.fill(12);
    m.sensor_dim.fill(1);
    m.sensor_interval.copy_from_slice(&[0.003, 0.0]);
    let mut scratch = vec![0; 3];
    make_dof_dof_sparse(
        m.nv,
        m.n_c,
        m.n_d,
        m.n_m,
        &m.dof_parentid,
        &m.dof_simplenum,
        &mut m.m_rownnz,
        &mut m.m_rowadr,
        None,
        &mut m.m_colind,
        true,
        false,
        &mut scratch,
    );
    make_dof_dof_sparse(
        m.nv,
        m.n_c,
        m.n_d,
        m.n_m,
        &m.dof_parentid,
        &m.dof_simplenum,
        &mut m.d_rownnz,
        &mut m.d_rowadr,
        Some(&mut m.d_diag),
        &mut m.d_colind,
        false,
        true,
        &mut scratch,
    );
    make_b_sparse(
        m.nv,
        m.nbody,
        m.n_b,
        &m.body_dofnum,
        &m.body_parentid,
        &m.body_dofadr,
        &mut m.b_rownnz,
        &mut m.b_rowadr,
        &mut m.b_colind,
        &mut scratch,
    );
    let mut legacy = vec![0; 3];
    make_dof_dof_maps(
        m.nv,
        m.n_m,
        m.n_c,
        m.n_d,
        &m.dof_madr,
        &m.dof_simplenum,
        &m.dof_parentid,
        &m.d_rownnz,
        &m.d_rowadr,
        &m.d_colind,
        &m.m_rownnz,
        &m.m_rowadr,
        &m.m_colind,
        &mut m.map_m2d,
        &mut m.map_d2m,
        &mut m.map_m2m,
        &mut legacy,
        &mut scratch,
    );
    assert_eq!(m.map_m2d, [0, 1, 1, 2], "lower-to-symmetric map");
    assert_eq!(m.map_d2m, [0, 2, 3], "symmetric-to-lower map");
    assert_eq!(m.map_m2m, [0, 2, 1], "legacy-to-CSR map");
    let mut d = make_data(&m);
    data_fields!(check_lengths(m, d));
    assert_eq!(d.qpos, m.qpos0);
    assert_eq!((d.time, d.ncon), (0.0, 0));
    assert!(d.contact.is_empty() && d.efc_j.is_empty() && d.efm_l.is_empty());
    assert_eq!(d.plastic, m.qpos_spring);
    assert_eq!(d.eq_active, m.eq_active0);
    assert_eq!(d.mocap_pos, [1.0, 2.0, 3.0]);
    assert_eq!(d.mocap_quat, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(d.ctrl, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        &d.history[..12],
        &[0.0, 1.0, -0.004, -0.002, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]
    );
    assert_eq!(&d.history[12..], &[-0.003, 1.0, -0.006, -0.002, 0.0, 0.0]);
    assert_eq!(d.tree_asleep, [-(1 + MINAWAKE as i32)]);
    assert_eq!(d.body_awake, [S_AWAKE, S_AWAKE]);
    assert_eq!(
        (d.ntree_awake, d.nbody_awake, d.nparent_awake, d.nv_awake),
        (1, 2, 1, 2)
    );
    assert_eq!(d.signature, m.signature);
    warning(&mut d, WARN_BADQPOS, 1);
    warning(&mut d, WARN_BADQPOS, 0);
    assert_eq!(
        (
            d.warning[WARN_BADQPOS as usize].number,
            d.warning[WARN_BADQPOS as usize].lastinfo
        ),
        (2, 0)
    );
    assert!(warning_text_of(WARN_CNSTRFULL, 1024).contains("above 1K bytes."));
    let mut v = d.scratch.num(8);
    let ptr = v.as_ptr();
    v.fill(1.0);
    d.scratch.put_num(v);
    let large = d.scratch.num(16);
    d.scratch.put_num(large);
    let v = d.scratch.num(4);
    assert_eq!(
        v.as_ptr(),
        ptr,
        "smallest fitting scratch allocation is reused"
    );
    assert_eq!(v, [0.0; 4]);
    d.scratch.put_num(v);
    let mut ints = d.scratch.int(3);
    ints.fill(4);
    d.scratch.put_int(ints);
    let ints = d.scratch.int(2);
    assert_eq!(ints, [0; 2]);
    d.scratch.put_int(ints);
    let mut bytes = d.scratch.byte(3);
    bytes.fill(5);
    d.scratch.put_byte(bytes);
    let bytes = d.scratch.byte(2);
    assert_eq!(bytes, [0; 2]);
    d.scratch.put_byte(bytes);
    d.qpos.fill(99.0);
    d.qvel.fill(99.0);
    d.time = 99.0;
    d.ncon = 5;
    d.eq_active.fill(0);
    d.flexvert_lambda.fill(8.0);
    d.flexvert_conage.fill(2);
    d.tree_asleep.fill(0);
    reset_data(&m, &mut d);
    data_fields!(check_lengths(m, d));
    assert_eq!(d.qpos, m.qpos0);
    assert_eq!(d.qvel, [0.0; 2]);
    assert_eq!((d.time, d.ncon), (0.0, 0));
    assert_eq!(d.warning[WARN_BADQPOS as usize].number, 0);
    assert_eq!(d.eq_active, m.eq_active0);
    assert_eq!(d.flexvert_lambda, [0.0]);
    assert_eq!(d.flexvert_conage, [0]);
    let v = d.scratch.num(4);
    assert_eq!(v.as_ptr(), ptr, "reset preserves scratch storage");
    d.scratch.put_num(v);
    reset_data_debug(&m, &mut d, 0x5a);
    assert_eq!(
        d.xpos[0].to_ne_bytes(),
        [0x5a; 8],
        "debug memset reaches computed buffers"
    );
    assert_eq!(d.qpos, m.qpos0, "debug reset restores state");
    m.key_time[0] = 2.5;
    m.key_qpos.copy_from_slice(&[0.5, 0.75]);
    m.key_qvel.copy_from_slice(&[1.0, -1.0]);
    m.key_act.fill(0.3);
    m.key_ctrl.copy_from_slice(&[0.0, 1.0, 0.0, 0.0]);
    m.key_mpos.copy_from_slice(&[4.0, 5.0, 6.0]);
    m.key_mquat.copy_from_slice(&[0.0, 1.0, 0.0, 0.0]);
    m.key_plastic.copy_from_slice(&[0.2, 0.4]);
    reset_data_keyframe(&m, &mut d, 0);
    assert_eq!(d.time, m.key_time[0]);
    assert_eq!(d.qpos, m.key_qpos);
    assert_eq!(d.qvel, m.key_qvel);
    assert_eq!(d.act, m.key_act);
    assert_eq!(d.ctrl, m.key_ctrl);
    assert_eq!(d.mocap_pos, m.key_mpos);
    assert_eq!(d.mocap_quat, m.key_mquat);
    assert_eq!(d.plastic, m.key_plastic);
    reset_data_keyframe(&m, &mut d, -1);
    assert_eq!(d.qpos, m.qpos0, "invalid key only resets");
}
