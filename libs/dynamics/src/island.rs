//! Constraint islands, union-find, and island-local solver arrays.
//!
//! Translated from `src/engine/engine_island.c` and `engine_island.h` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2023 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::core_util::{
    contact_body_weight, eff_actuator_possible, eff_couplings, eff_flex_any, eff_flex_possible,
    eff_tendon_possible, is_metric, is_sparse,
};
use crate::data::{Contact, Data};
use crate::model::Model;
use crate::types::*;
use crate::util_misc::{copy_int, fill_int, zero_int};
use crate::util_sparse::compare;

// mjMAXFLEXINTERPW from engine_core_util.h (not yet exported by core_util.rs).
const MAXFLEXINTERPW: usize = 98;

// Allocate island arrays on the arena; return 1 on success, 0 on failure.
fn arena_alloc_island(m: &Model, d: &mut Data) -> i32 {
    // C can fail here only from arena overflow. These Vecs grow instead, so
    // that failure cannot occur; retain their lengths/capacities on smaller steps.
    macro_rules! grow {
        ($field:ident, $n:expr, $zero:expr) => {
            if d.$field.len() < $n {
                d.$field.resize($n, $zero);
            }
        };
    }
    let nisland = d.nisland as usize;
    let nidof = d.nidof as usize;
    let nefc = d.nefc as usize;
    // MJDATA_ARENA_POINTERS_ISLAND, in its allocation order.
    grow!(tree_island, m.ntree, 0);
    grow!(island_ntree, nisland, 0);
    grow!(island_itreeadr, nisland, 0);
    grow!(map_itree2tree, m.ntree, 0);
    grow!(dof_island, m.nv, 0);
    grow!(island_nv, nisland, 0);
    grow!(island_idofadr, nisland, 0);
    grow!(island_dofadr, nisland, 0);
    grow!(map_dof2idof, m.nv, 0);
    grow!(map_idof2dof, m.nv, 0);
    grow!(ifrc_smooth, nidof, 0.0);
    grow!(iacc_smooth, nidof, 0.0);
    grow!(iacc, nidof, 0.0);
    grow!(efc_island, nefc, 0);
    grow!(island_ne, nisland, 0);
    grow!(island_nf, nisland, 0);
    grow!(island_nefc, nisland, 0);
    grow!(island_iefcadr, nisland, 0);
    grow!(map_efc2iefc, nefc, 0);
    grow!(map_iefc2efc, nefc, 0);
    grow!(iefc_type, nefc, 0);
    grow!(iefc_id, nefc, 0);
    grow!(iefc_frictionloss, nefc, 0.0);
    grow!(iefc_d, nefc, 0.0);
    grow!(iefc_r, nefc, 0.0);
    grow!(iefc_aref, nefc, 0.0);
    grow!(iefc_state, nefc, 0);
    grow!(iefc_force, nefc, 0.0);
    grow!(ifrc_constraint, nidof, 0.0);
    1
}

// Find the canonical root of an active tree and compress its path.
pub fn dsu_root(parent: &mut [i32], mut tree: i32) -> i32 {
    let mut root = tree;
    while parent[root as usize] != root {
        root = parent[root as usize];
    }
    while parent[tree as usize] != tree {
        let next = parent[tree as usize];
        parent[tree as usize] = root;
        tree = next;
    }
    root
}

// Activate and union two incident trees; -1 denotes a static endpoint.
pub fn dsu_merge(parent: &mut [i32], mut tree1: i32, mut tree2: i32) {
    if tree1 == -1 && tree2 == -1 {
        panic!("self-incidence of the static tree");
    }
    if tree1 == -1 {
        tree1 = tree2;
    }
    if tree2 == -1 {
        tree2 = tree1;
    }
    if parent[tree1 as usize] == -1 {
        parent[tree1 as usize] = tree1;
    }
    if parent[tree2 as usize] == -1 {
        parent[tree2 as usize] = tree2;
    }
    if parent[tree1 as usize] == parent[tree2 as usize] {
        return;
    }
    let root1 = dsu_root(parent, tree1);
    let root2 = dsu_root(parent, tree2);
    if root1 < root2 {
        parent[root2 as usize] = root1;
    } else if root2 < root1 {
        parent[root1 as usize] = root2;
    }
}

// Assign deterministic island ids in ascending canonical-root order.
pub fn dsu_assign(
    island: &mut [i32],
    parent: &mut [i32],
    tree_dofnum: &[i32],
    ntree: usize,
    nidof: &mut i32,
) -> i32 {
    let mut nisland = 0;
    *nidof = 0;
    for tree in 0..ntree {
        if parent[tree] == -1 {
            island[tree] = -1;
            continue;
        }
        if parent[tree] == tree as i32 {
            island[tree] = nisland;
            nisland += 1;
        } else {
            // Union links larger roots to smaller roots, so this predecessor
            // has already been compressed and assigned an island.
            parent[tree] = parent[parent[tree] as usize];
            island[tree] = island[parent[tree] as usize];
        }
        *nidof += tree_dofnum[tree];
    }
    nisland
}

// Find disjoint subgraphs given a sparse symmetric adjacency matrix.
// Vertices without edges get -1; column indices need not be unique or sorted.
pub fn flood_fill(
    island: &mut [i32],
    nr: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    stack: &mut [i32],
) -> i32 {
    let mut nisland = 0;
    fill_int(island, -1, nr);
    for i in 0..nr {
        if island[i] != -1 || rownnz[i] == 0 {
            continue;
        }
        let mut nstack = 0;
        stack[nstack] = i as i32;
        nstack += 1;
        while nstack != 0 {
            nstack -= 1;
            let v = stack[nstack] as usize;
            if island[v] != -1 {
                continue;
            }
            island[v] = nisland;
            copy_int(&mut stack[nstack..], &colind[rowadr[v] as usize..], rownnz[v] as usize);
            nstack += rownnz[v] as usize;
        }
        nisland += 1;
    }
    nisland
}

// State of the iterator for finding trees involved in a constraint.
struct TreeIter {
    trees: [i32; 2], // Precalculated trees; -2: empty/sentinel.
    jac_idx: i32,   // Current Jacobian lookup index; -1: scan disabled.
    tree_prev: i32, // Previous tree in ongoing scan.
}

// Return next tree of constraint i; -2: no more trees.
fn tree_next(m: &Model, d: &Data, i: usize, iter: &mut TreeIter) -> i32 {
    if iter.trees[0] != -2 {
        let tree = iter.trees[0];
        iter.trees[0] = iter.trees[1];
        iter.trees[1] = -2;
        return tree;
    }
    if iter.jac_idx == -1 {
        return -2;
    }
    let mut j = iter.jac_idx;
    let mut tree_next = -2;
    if is_sparse(m) != 0 {
        let rownnz = d.efc_j_rownnz[i];
        let colind = &d.efc_j_colind[d.efc_j_rowadr[i] as usize..];
        while j < rownnz {
            let tree_j = m.dof_treeid[colind[j as usize] as usize];
            if tree_j != iter.tree_prev {
                tree_next = tree_j;
                break;
            }
            j += 1;
        }
    } else {
        let nv = m.nv;
        let j_mat = &d.efc_j[nv * i..]; // C's J, distinct from loop index j.
        while j < nv as i32 {
            if j_mat[j as usize] != 0.0 {
                let tree_j = m.dof_treeid[j as usize];
                if tree_j != iter.tree_prev {
                    tree_next = tree_j;
                    break;
                }
                j = m.tree_dofadr[tree_j as usize] + m.tree_dofnum[tree_j as usize] - 1;
            }
            j += 1;
        }
    }
    iter.jac_idx = j;
    if tree_next != -2 {
        iter.tree_prev = tree_next;
    }
    tree_next
}

// Initialize the tree iterator, handling special cases.
fn tree_iter_init(m: &Model, d: &Data, i: usize, iter: &mut TreeIter) {
    iter.trees[0] = -2;
    iter.trees[1] = -2;
    iter.jac_idx = -1;
    iter.tree_prev = -1;
    let efc_type = d.efc_type[i];
    let efc_id = d.efc_id[i];
    if efc_type == CNSTR_FRICTION_DOF {
        iter.trees[0] = m.dof_treeid[efc_id as usize];
    } else if efc_type == CNSTR_LIMIT_JOINT {
        iter.trees[0] = m.dof_treeid[m.jnt_dofadr[efc_id as usize] as usize];
    } else if efc_type == CNSTR_EQUALITY
        && (m.eq_type[efc_id as usize] == EQ_CONNECT || m.eq_type[efc_id as usize] == EQ_WELD)
    {
        let mut b1 = m.eq_obj1id[efc_id as usize];
        let mut b2 = m.eq_obj2id[efc_id as usize];
        if m.eq_objtype[efc_id as usize] == OBJ_SITE {
            b1 = m.site_bodyid[b1 as usize];
            b2 = m.site_bodyid[b2 as usize];
        }
        iter.trees[0] = m.body_treeid[b1 as usize];
        iter.trees[1] = m.body_treeid[b2 as usize];
        if iter.trees[0] < 0 && iter.trees[1] < 0 {
            panic!("equality {} is between two static bodies", efc_id);
        }
    } else {
        iter.jac_idx = 0;
    }
}

// Whether repeated scalar rows require separate tree scans.
fn is_flex_equality(m: &Model, efc_type: i32, efc_id: i32) -> bool {
    efc_type == CNSTR_EQUALITY
        && (m.eq_type[efc_id as usize] == EQ_FLEX || m.eq_type[efc_id as usize] == EQ_FLEXSTRAIN)
}

// Union the contact's body trees; flex Jacobian rows can vanish and cannot
// provide the complete incidence. Return the first tree, or -1 if none.
fn union_contact_trees(m: &Model, d: &Data, parent: &mut [i32], con: &Contact) -> i32 {
    if con.geom[0] >= 0 && con.geom[1] >= 0 {
        let mut tree1 = m.body_treeid[m.geom_bodyid[con.geom[0] as usize] as usize];
        let mut tree2 = m.body_treeid[m.geom_bodyid[con.geom[1] as usize] as usize];
        if tree1 < 0 {
            tree1 = tree2;
            tree2 = -1;
        }
        if tree1 >= 0 {
            dsu_merge(parent, tree1, tree2);
        }
        return tree1;
    }
    let mut body = [0; 2 * MAXFLEXINTERPW];
    let mut nb = contact_body_weight(m, d, con, 0, &mut body, None);
    nb += contact_body_weight(m, d, con, 1, &mut body[nb..], None);
    let mut tree1 = -1;
    for j in 0..nb {
        let tree2 = m.body_treeid[body[j] as usize];
        if tree2 < 0 || tree2 == tree1 {
            continue;
        }
        if tree1 < 0 {
            tree1 = tree2;
            dsu_merge(parent, tree1, -1);
        } else {
            dsu_merge(parent, tree1, tree2);
        }
    }
    tree1
}

// Union awake trees in the tendon's static Jacobian coupling footprint.
fn union_tendon_trees(m: &Model, d: &Data, parent: &mut [i32], i: usize) {
    let start = m.ten_j_rowadr[i];
    let end = start + m.ten_j_rownnz[i];
    let mut tree1 = -1;
    for j in start..end {
        let tree2 = m.dof_treeid[m.ten_j_colind[j as usize] as usize];
        if d.tree_awake[tree2 as usize] == 0 {
            continue;
        }
        if tree1 < 0 {
            tree1 = tree2;
        } else if tree1 != tree2 {
            dsu_merge(parent, tree1, tree2);
        }
    }
}

// Activate and union all trees with direct incidence in a constraint.
fn union_constraint_trees(
    m: &Model,
    d: &Data,
    parent: &mut [i32],
    efc_tree: &mut [i32],
    err_i: &mut i32,
) -> Option<&'static str> {
    let nefc = d.nefc as usize;
    let mut efc_type = -1;
    let mut efc_id = -1;
    for i in 0..nefc {
        if i > 0 && efc_type == d.efc_type[i] && efc_id == d.efc_id[i] {
            if !is_flex_equality(m, efc_type, efc_id) {
                efc_tree[i] = efc_tree[i - 1];
                continue;
            }
        }
        efc_type = d.efc_type[i];
        efc_id = d.efc_id[i];
        if efc_type == CNSTR_CONTACT_FRICTIONLESS
            || efc_type == CNSTR_CONTACT_PYRAMIDAL
            || efc_type == CNSTR_CONTACT_ELLIPTIC
        {
            efc_tree[i] = union_contact_trees(m, d, parent, &d.contact[efc_id as usize]);
            if efc_tree[i] < 0 {
                *err_i = efc_id;
                return Some("contact %d is between two static bodies");
            }
            continue;
        }
        let mut iter = TreeIter { trees: [0; 2], jac_idx: 0, tree_prev: 0 };
        tree_iter_init(m, d, i, &mut iter);
        let mut tree1 = tree_next(m, d, i, &mut iter);
        if tree1 != -2 {
            let mut tree2 = tree_next(m, d, i, &mut iter);
            efc_tree[i] = if tree1 >= 0 { tree1 } else { tree2 };
            if efc_tree[i] < 0 {
                *err_i = i as i32;
                return Some("constraint %d is between two static bodies");
            }
            if tree2 == -2 {
                dsu_merge(parent, tree1, -1);
            } else {
                while tree2 != -2 {
                    dsu_merge(parent, tree1, tree2);
                    tree1 = tree2;
                    tree2 = tree_next(m, d, i, &mut iter);
                }
            }
        } else {
            *err_i = i as i32;
            return Some("no tree found for constraint %d");
        }
    }

    // Metric flexes couple their awake vertex/node trees without efc rows.
    for f in 0..m.nflex {
        if eff_flex_possible(m, f) == 0 {
            continue;
        }
        let (num, adr, bodyid);
        if m.flex_interp[f] != 0 {
            num = m.flex_nodenum[f];
            adr = m.flex_nodeadr[f];
            bodyid = &m.flex_nodebodyid;
        } else {
            num = m.flex_vertnum[f];
            adr = m.flex_vertadr[f];
            bodyid = &m.flex_vertbodyid;
        }
        let mut tree1 = -1;
        for j in 0..num {
            let tree2 = m.body_treeid[bodyid[(adr + j) as usize] as usize];
            if tree2 < 0 || tree2 == tree1 || d.tree_awake[tree2 as usize] == 0 {
                continue;
            }
            if tree1 < 0 {
                tree1 = tree2;
            } else {
                dsu_merge(parent, tree1, tree2);
            }
        }
        // Rod twist is not translated. Its conditional and the
        // mj_rodChainTwistJnt loop cannot be reached past this untranslated call.
        crate::unported!("mj_rodHasTwist");
    }

    // Passive flex contacts carry metric terms without efc rows.
    if is_metric(m) != 0 {
        for i in 0..d.ncon as usize {
            let con = &d.contact[i];
            if con.exclude != 4 {
                continue;
            }
            let mut body = [0; 2 * MAXFLEXINTERPW];
            let mut nb = contact_body_weight(m, d, con, 0, &mut body, None);
            nb += contact_body_weight(m, d, con, 1, &mut body[nb..], None);
            let mut tree1 = -1;
            for j in 0..nb {
                let tree2 = m.body_treeid[body[j] as usize];
                if tree2 < 0 || tree2 == tree1 || d.tree_awake[tree2 as usize] == 0 {
                    continue;
                }
                if tree1 < 0 {
                    tree1 = tree2;
                } else {
                    dsu_merge(parent, tree1, tree2);
                }
            }
        }
    }

    // Tendon and actuator metric terms couple their static transmission trees.
    if eff_couplings(m) != 0 {
        for i in 0..m.ntendon {
            if m.tendon_treenum[i] < 2 || eff_tendon_possible(m, i) == 0 {
                continue;
            }
            union_tendon_trees(m, d, parent, i);
        }
        for i in 0..m.nactuator {
            if eff_actuator_possible(m, i) == 0 {
                continue;
            }
            let mut tree1 = -1;
            let mut tree2 = -1;
            let tid = m.actuator_trnid[2 * i];
            let tid2 = m.actuator_trnid[2 * i + 1];
            match m.actuator_trntype[i] {
                TRN_TENDON => union_tendon_trees(m, d, parent, tid as usize),
                TRN_SITE | TRN_SLIDERCRANK => {
                    tree1 = m.body_treeid[m.site_bodyid[tid as usize] as usize];
                    if tid2 >= 0 {
                        tree2 = m.body_treeid[m.site_bodyid[tid2 as usize] as usize];
                    }
                }
                TRN_SO3 => {
                    if tid2 >= 0 {
                        tree1 = m.body_treeid[m.site_bodyid[tid as usize] as usize];
                        tree2 = m.body_treeid[m.site_bodyid[tid2 as usize] as usize];
                    }
                }
                _ => {} // joint, jointinparent, body: single tree, no edge.
            }
            if tree1 >= 0 && tree2 >= 0 && tree1 != tree2
                && d.tree_awake[tree1 as usize] != 0 && d.tree_awake[tree2 as usize] != 0
            {
                dsu_merge(parent, tree1, tree2);
            }
        }
    }
    None
}

// Discover islands and construct the island-local solver views.
pub fn island(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let nefc = d.nefc as usize;
    let ntree = m.ntree;
    if m.opt.disableflags & DSBL_ISLAND != 0
        || (nefc == 0 && !(m.opt.enableflags & ENBL_SLEEP != 0
            && is_metric(m) != 0 && eff_flex_any(m) != 0))
    {
        d.nidof = 0;
        d.nisland = 0;
        return;
    }
    let mut efc_tree = d.scratch.int(nefc);
    let mut parent = d.scratch.int(ntree);
    fill_int(&mut parent, -1, ntree);
    let mut err_i = -1;
    let err_msg = union_constraint_trees(m, d, &mut parent, &mut efc_tree, &mut err_i);
    if let Some(err_msg) = err_msg {
        d.scratch.put_int(efc_tree);
        d.scratch.put_int(parent);
        panic!("{}", err_msg.replace("%d", &err_i.to_string()));
    }
    let mut tree_island = d.scratch.int(ntree);
    let mut nidof = 0;
    d.nisland = dsu_assign(&mut tree_island, &mut parent, &m.tree_dofnum, ntree, &mut nidof);
    if d.nisland == 0 {
        d.nidof = 0;
        d.scratch.put_int(efc_tree);
        d.scratch.put_int(parent);
        d.scratch.put_int(tree_island);
        return;
    }
    d.nidof = nidof;
    if arena_alloc_island(m, d) == 0 {
        d.scratch.put_int(efc_tree);
        d.scratch.put_int(parent);
        d.scratch.put_int(tree_island);
        return;
    }
    let nisland = d.nisland as usize;

    // Trees.
    copy_int(&mut d.tree_island, &tree_island, ntree);
    zero_int(&mut d.island_ntree, nisland);
    for i in 0..ntree {
        let island = tree_island[i];
        if island >= 0 {
            d.island_ntree[island as usize] += 1;
        }
    }
    d.island_itreeadr[0] = 0;
    for i in 1..nisland {
        d.island_itreeadr[i] = d.island_itreeadr[i - 1] + d.island_ntree[i - 1];
    }
    let last_tree = d.island_itreeadr[nisland - 1] + d.island_ntree[nisland - 1];
    let mut island_ntree2 = d.scratch.int(nisland + 1);
    zero_int(&mut island_ntree2, nisland + 1);
    for i in 0..ntree {
        let island = tree_island[i];
        if island >= 0 {
            let island = island as usize;
            let adr = d.island_itreeadr[island] + island_ntree2[island];
            island_ntree2[island] += 1;
            d.map_itree2tree[adr as usize] = i as i32;
        } else {
            let adr = last_tree + island_ntree2[nisland];
            island_ntree2[nisland] += 1;
            d.map_itree2tree[adr as usize] = i as i32;
        }
    }
    if compare(&island_ntree2, &d.island_ntree, nisland) == 0 {
        panic!("island_ntree miscount");
    }
    if last_tree + island_ntree2[nisland] != ntree as i32 {
        panic!("miscount of unconstrained trees");
    }

    // Degrees of freedom.
    zero_int(&mut d.island_nv, nisland);
    for i in 0..nv {
        let island = tree_island[m.dof_treeid[i] as usize];
        d.dof_island[i] = island;
        if island >= 0 {
            d.island_nv[island as usize] += 1;
        }
    }
    d.island_idofadr[0] = 0;
    for i in 1..nisland {
        d.island_idofadr[i] = d.island_idofadr[i - 1] + d.island_nv[i - 1];
    }
    let mut island_nv2 = d.scratch.int(nisland + 1);
    zero_int(&mut island_nv2, nisland + 1);
    for dof in 0..nv {
        let island = d.dof_island[dof];
        let idof;
        if island >= 0 {
            let island = island as usize;
            idof = d.island_idofadr[island] + island_nv2[island];
            island_nv2[island] += 1;
        } else {
            idof = nidof + island_nv2[nisland];
            island_nv2[nisland] += 1;
        }
        d.map_dof2idof[dof] = idof;
        d.map_idof2dof[idof as usize] = dof as i32;
    }
    if compare(&island_nv2, &d.island_nv, nisland) == 0 {
        panic!("island_nv miscount");
    }
    if nidof + island_nv2[nisland] != nv as i32 {
        panic!("miscount of unconstrained dofs");
    }
    for i in 0..nisland {
        d.island_dofadr[i] = d.map_idof2dof[d.island_idofadr[i] as usize];
    }

    // Constraints.
    zero_int(&mut d.island_ne, nisland);
    zero_int(&mut d.island_nf, nisland);
    zero_int(&mut d.island_nefc, nisland);
    for i in 0..nefc {
        d.efc_island[i] = tree_island[efc_tree[i] as usize];
        let island = d.efc_island[i] as usize;
        d.island_nefc[island] += 1;
        match d.efc_type[i] {
            CNSTR_EQUALITY => d.island_ne[island] += 1,
            CNSTR_FRICTION_DOF | CNSTR_FRICTION_TENDON => d.island_nf[island] += 1,
            _ => {}
        }
    }
    d.island_iefcadr[0] = 0;
    for i in 1..nisland {
        d.island_iefcadr[i] = d.island_iefcadr[i - 1] + d.island_nefc[i - 1];
    }
    let island_nefc2 = &mut island_nv2;
    zero_int(island_nefc2, nisland);
    for c in 0..nefc {
        let island = d.efc_island[c] as usize;
        let ic = d.island_iefcadr[island] + island_nefc2[island];
        island_nefc2[island] += 1;
        d.map_efc2iefc[c] = ic;
        d.map_iefc2efc[ic as usize] = c as i32;
        d.iefc_type[ic as usize] = d.efc_type[c];
        d.iefc_id[ic as usize] = d.efc_id[c];
        d.iefc_frictionloss[ic as usize] = d.efc_frictionloss[c];
        d.iefc_d[ic as usize] = d.efc_d[c];
        d.iefc_r[ic as usize] = d.efc_r[c];
    }
    if compare(island_nefc2, &d.island_nefc, nisland) == 0 {
        panic!("island_nefc miscount");
    }
    d.scratch.put_int(efc_tree);
    d.scratch.put_int(parent);
    d.scratch.put_int(tree_island);
    d.scratch.put_int(island_ntree2);
    d.scratch.put_int(island_nv2);
}
