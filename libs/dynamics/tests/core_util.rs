use makepad_dynamics::core_util::{body_chain, jac, merge_chain};
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::{make_model, MODEL_SIZES};
use makepad_dynamics::types::JNT_HINGE;

#[test]
fn body_chains_and_point_jacobian() {
    let mut sizes = vec![0_i64; MODEL_SIZES.len()];
    for (name, value) in [("nbody", 5), ("nq", 2), ("nv", 2), ("njnt", 2)] {
        sizes[MODEL_SIZES.iter().position(|s| *s == name).unwrap()] = value;
    }
    let mut m = make_model(&sizes).unwrap();
    // world -> hinge 1 -> hinge 2 -> welded 3; body 4 is welded to world.
    m.body_parentid.copy_from_slice(&[0, 0, 1, 2, 0]);
    m.body_weldid.copy_from_slice(&[0, 1, 2, 2, 0]);
    m.body_rootid.copy_from_slice(&[0, 1, 1, 1, 4]);
    m.body_dofnum.copy_from_slice(&[0, 1, 1, 0, 0]);
    m.body_dofadr.copy_from_slice(&[-1, 0, 1, -1, -1]);
    m.body_simple.copy_from_slice(&[1, 1, 0, 0, 0]);
    m.body_jntnum.copy_from_slice(&[0, 1, 1, 0, 0]);
    m.body_jntadr.copy_from_slice(&[-1, 0, 1, -1, -1]);
    m.body_mocapid.fill(-1);
    m.body_treeid.fill(-1);
    m.dof_parentid.copy_from_slice(&[-1, 0]);
    m.dof_bodyid.copy_from_slice(&[1, 2]);
    m.dof_jntid.copy_from_slice(&[0, 1]);
    m.jnt_type.fill(JNT_HINGE);
    m.jnt_bodyid.copy_from_slice(&[1, 2]);
    m.jnt_dofadr.copy_from_slice(&[0, 1]);
    m.jnt_qposadr.copy_from_slice(&[0, 1]);
    m.jnt_plasticadr.fill(-1);
    let mut d = make_data(&m);
    let mut chain = [-7; 2];
    assert_eq!(body_chain(&m, 0, &mut chain), 0, "world has no chain");
    assert_eq!(body_chain(&m, 1, &mut chain), 1, "simple hinge chain");
    assert_eq!(chain[0], 0, "simple hinge address");
    assert_eq!(
        body_chain(&m, 3, &mut chain),
        2,
        "welded body inherits ancestors"
    );
    assert_eq!(chain, [0, 1], "chain is in increasing dof order");
    assert_eq!(
        body_chain(&m, 4, &mut chain),
        0,
        "world-welded body is fixed"
    );
    for (b1, b2) in [(1, 3), (3, 1), (0, 3), (3, 3)] {
        let n = merge_chain(&m, &mut chain, b1, b2, 0);
        assert_eq!(&chain[..n], &[0, 1], "merged ancestors for {b1}, {b2}");
    }
    let n = merge_chain(&m, &mut chain, 1, 3, 1);
    assert_eq!(&chain[..n], &[1], "common ancestor excluded");
    assert_eq!(
        merge_chain(&m, &mut chain, 3, 3, 1),
        0,
        "identical chains cancel"
    );
    assert_eq!(
        merge_chain(&m, &mut chain, 0, 4, 0),
        0,
        "both weld roots fixed"
    );

    // Same allocated tree, with two independent hinges: no common dof to skip.
    m.body_parentid[2] = 0;
    m.dof_parentid[1] = -1;
    let n = merge_chain(&m, &mut chain, 1, 3, 1);
    assert_eq!(&chain[..n], &[0, 1], "disjoint chains retain both dofs");
    m.body_parentid[2] = 1;
    m.dof_parentid[1] = 0;

    d.subtree_com[3..6].copy_from_slice(&[1.0, 2.0, 3.0]);
    d.xpos[9..12].copy_from_slice(&[3.0, 5.0, 7.0]);
    d.cdof
        .copy_from_slice(&[0.0, 0.0, 1.0, 1.0, 2.0, 3.0, 0.0, 1.0, 0.0, -1.0, 0.0, 2.0]);
    let point = [d.xpos[9], d.xpos[10], d.xpos[11]];
    let mut jacp = [99.0; 6];
    let mut jacr = [99.0; 6];
    jac(&m, &d, Some(&mut jacp), Some(&mut jacr), &point, 3);
    // offset=(2,3,4): z cross offset=(-3,2,0); y cross offset=(4,0,-2).
    // Add cdof translations: columns (-2,4,3), (3,0,0), stored row-major.
    assert_eq!(
        jacp,
        [-2.0, 3.0, 4.0, 0.0, 3.0, 0.0],
        "linear Jacobian cross products"
    );
    assert_eq!(
        jacr,
        [0.0, 0.0, 0.0, 1.0, 1.0, 0.0],
        "angular Jacobian axes"
    );
    jac(&m, &d, Some(&mut jacp), None, &point, 1);
    assert_eq!(
        jacp,
        [-2.0, 0.0, 4.0, 0.0, 3.0, 0.0],
        "descendant dof cleared"
    );
    jac(&m, &d, None, Some(&mut jacr), &point, 3);
    assert_eq!(jacr, [0.0, 0.0, 0.0, 1.0, 1.0, 0.0], "rotation-only output");
    jac(&m, &d, Some(&mut jacp), Some(&mut jacr), &point, 4);
    assert_eq!(jacp, [0.0; 6], "fixed body translation cleared");
    assert_eq!(jacr, [0.0; 6], "fixed body rotation cleared");
}
