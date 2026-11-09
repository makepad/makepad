use makepad_dynamics::island::dsu_assign;

#[test]
fn dsu_assign_parent_forest() {
    // Five trees, three components: {0, 2}, {1}, {3, 4}.
    // C visits roots 0, 1, 3 in order, assigning islands 0, 1, 2.
    // Trees 2 and 4 inherit their root's island. The active singleton 1
    // receives an island even though it contributes no DOFs.
    let mut parent = [0, 1, 0, 3, 3];
    let tree_dofnum = [1, 0, 2, 1, 1];
    let mut island = [-1; 5];
    let mut nidof = -1;
    let nisland = dsu_assign(&mut island, &mut parent, &tree_dofnum, 5, &mut nidof);
    assert_eq!(nisland, 3, "one island per canonical root");
    assert_eq!(island, [0, 1, 0, 2, 2], "ascending-root island ids");
    assert_eq!(nidof, 5, "C accumulation: 1 + 0 + 2 + 1 + 1");
    assert_eq!(parent, [0, 1, 0, 3, 3], "forest is already compressed");
}
