//! Sleep bookkeeping derived from tree sleep state.
//!
//! Translated from `src/engine/engine_sleep.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::data::Data;
use crate::model::Model;
use crate::types::*;

pub fn update_sleep_init(m: &Model, d: &mut Data, flg_staticawake: bool) {
    let mut ntree_awake = 0;
    for i in 0..m.ntree {
        d.tree_awake[i] = i32::from(d.tree_asleep[i] < 0);
        ntree_awake += d.tree_awake[i];
    }
    d.ntree_awake = ntree_awake;
    let mut nbody_awake = 0;
    let mut nparent_awake = 0;
    for i in 0..m.nbody {
        if m.body_treeid[i] < 0 {
            d.body_awake[i] = if m.body_mocapid[m.body_rootid[i] as usize] >= 0 {
                S_AWAKE
            } else if flg_staticawake {
                S_AWAKE
            } else {
                S_STATIC
            };
        } else {
            d.body_awake[i] = if d.tree_awake[m.body_treeid[i] as usize] != 0 {
                S_AWAKE
            } else {
                S_ASLEEP
            };
        }
        if d.body_awake[i] != S_ASLEEP {
            d.body_awake_ind[nbody_awake] = i as i32;
            nbody_awake += 1;
        }
        if i != 0 && d.body_awake[m.body_parentid[i] as usize] != S_ASLEEP {
            d.parent_awake_ind[nparent_awake] = i as i32;
            nparent_awake += 1;
        }
    }
    d.nbody_awake = nbody_awake as i32;
    d.nparent_awake = nparent_awake as i32;
    let mut nv_awake = 0;
    for i in 0..m.nv {
        let bodyid = m.dof_bodyid[i] as usize;
        if m.body_treeid[bodyid] >= 0 && d.body_awake[bodyid] == S_AWAKE {
            d.dof_awake_ind[nv_awake] = i as i32;
            nv_awake += 1;
        }
    }
    d.nv_awake = nv_awake as i32;
}

pub fn update_sleep(m: &Model, d: &mut Data) {
    update_sleep_init(m, d, false);
}

// Return 1 if the weighted infinity norm is smaller than tol.
fn is_smaller(vec: &[f64], weight: &[f64], n: usize, tol: f64) -> i32 {
    let mut max = 0.0;
    for i in 0..n {
        max = crate::util_misc::max(max, weight[i] * vec[i].abs());
        if max >= tol {
            return 0;
        }
    }
    1
}

// Return 1 if tree i can sleep.
fn tree_can_sleep(m: &Model, d: &Data, i: usize, tol: f64) -> i32 {
    if m.tree_sleep_policy[i] == SLEEP_NEVER || m.tree_sleep_policy[i] == SLEEP_AUTO_NEVER {
        return 0;
    }
    let mut adr = m.tree_bodyadr[i] as usize;
    let mut num = m.tree_bodynum[i] as usize;
    // C checks the bytes, so -0.0 is nonzero too. f64 has no padding bytes.
    let xfrc = &d.xfrc_applied[6 * adr..6 * (adr + num)];
    let bytes = unsafe { std::slice::from_raw_parts(xfrc.as_ptr().cast::<u8>(), 6 * num * 8) };
    if crate::util_misc::is_zero_byte(bytes, bytes.len()) == 0 {
        return 0;
    }
    adr = m.tree_dofadr[i] as usize;
    num = m.tree_dofnum[i] as usize;
    let qfrc = &d.qfrc_applied[adr..adr + num];
    let bytes = unsafe { std::slice::from_raw_parts(qfrc.as_ptr().cast::<u8>(), num * 8) };
    if crate::util_misc::is_zero_byte(bytes, bytes.len()) == 0 {
        return 0;
    }
    if tol != 0.0 {
        is_smaller(&d.qvel[adr..], &m.dof_length[adr..], num, tol)
    } else {
        let qvel = &d.qvel[adr..adr + num];
        let bytes = unsafe { std::slice::from_raw_parts(qvel.as_ptr().cast::<u8>(), num * 8) };
        crate::util_misc::is_zero_byte(bytes, bytes.len())
    }
}

// Return the smallest tree in the sleep cycle starting at i, or -1 on error.
pub fn sleep_cycle(tree_asleep: &[i32], ntree: usize, i: i32) -> i32 {
    if i < 0 || i as usize >= ntree {
        return -1;
    }
    let mut smallest = i;
    let mut current = i;
    let mut count = 0;
    loop {
        if count > ntree {
            return -1;
        }
        let next = tree_asleep[current as usize];
        if next < 0 || next as usize >= ntree {
            return -1;
        }
        if next < smallest {
            smallest = next;
        }
        current = next;
        count += 1;
        if current == i {
            break;
        }
    }
    smallest
}

fn plural(n: usize) -> &'static str {
    if n > 1 {
        "s"
    } else {
        ""
    }
}

// Rust equivalent of the log-only C %6.*g conversion (2 or 3 significant digits).
fn log_time(time: f64, precision: usize) -> String {
    let text = if time.is_finite() {
        let scientific = format!("{:.*e}", precision - 1, time);
        let (mantissa, exponent) = scientific.split_once('e').unwrap();
        let exponent: i32 = exponent.parse().unwrap();
        if exponent < -4 || exponent >= precision as i32 {
            let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
            format!("{mantissa}e{exponent:+03}")
        } else {
            let decimals = (precision as i32 - 1 - exponent).max(0) as usize;
            let fixed = format!("{time:.decimals$}");
            if decimals != 0 {
                fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
            } else {
                fixed
            }
        }
    } else if time.is_nan() {
        if time.is_sign_negative() {
            "-nan"
        } else {
            "nan"
        }
        .to_owned()
    } else if time.is_sign_negative() {
        "-inf".to_owned()
    } else {
        "inf".to_owned()
    };
    format!("{text:>6}")
}

// Wake tree i and its associated cycle; return the number of trees woken.
pub fn wake_island(
    tree_asleep: &mut [i32],
    ntree: usize,
    i: i32,
    wakeval: i32,
    reason: Option<&str>,
    time: f64,
) -> i32 {
    let mut nwoke = 0;
    if i < 0 || i as usize >= ntree {
        panic!("invalid tree {i}");
    }
    let asleep_val = tree_asleep[i as usize];
    if asleep_val < 0 {
        tree_asleep[i as usize] = wakeval.min(asleep_val);
        return nwoke;
    } else {
        let mut current = i;
        let mut woke_trees = [0; 1024];
        loop {
            let next = tree_asleep[current as usize];
            if next < 0 || next as usize >= ntree {
                panic!("invalid sleep state index {next} when waking tree {i}");
            }
            tree_asleep[current as usize] = wakeval;
            if nwoke < 1024 {
                woke_trees[nwoke as usize] = current;
            }
            nwoke += 1;
            current = next;
            if current == i || nwoke as usize >= ntree {
                break;
            }
        }
        if current != i {
            panic!("tree {i} is not in a cycle");
        }
        if let Some(reason) = reason.filter(|_| crate::util_errmem::is_topic_enabled(TOPIC_SLEEP)) {
            let nprint = (nwoke as usize).min(1024);
            let mut buf = format!(
                "t={}, woke due to {} tree{} ",
                log_time(time, 3),
                reason,
                plural(nprint)
            );
            for j in 0..nprint {
                // MJ_APPEND_BUF clamps the buffer to 1023 bytes.
                if buf.len() < 1023 {
                    buf.push_str(&format!(
                        "{}{}",
                        woke_trees[j],
                        if j == nprint - 1 { "" } else { " " }
                    ));
                }
            }
            let mut msg = crate::util_errmem::LogMessage {
                level: LOG_DEBUG,
                topic: TOPIC_SLEEP,
                func: Some("mj_wakeIsland"),
                ..Default::default()
            };
            crate::util_misc::strncpy(&mut msg.subject, buf.as_bytes(), 1024);
            crate::util_errmem::message(&msg);
        }
    }
    nwoke
}

const K_AWAKE: i32 = -(1 + MINAWAKE as i32);

// Non-metric integrators cannot sleep constraint-free flexes.
fn wake_non_metric_flex(m: &Model, d: &mut Data, flg_wake: bool) -> i32 {
    let mut nwoke = 0;
    if crate::core_util::is_metric(m) != 0 {
        return 0;
    }
    for f in 0..m.nflex {
        if m.flex_edgeequality[f] != 0 || crate::core_util::eff_flex_possible(m, f) == 0 {
            continue;
        }
        let num = if m.flex_interp[f] != 0 {
            m.flex_nodenum[f]
        } else {
            m.flex_vertnum[f]
        };
        let bodyid = if m.flex_interp[f] != 0 {
            &m.flex_nodebodyid[m.flex_nodeadr[f] as usize..]
        } else {
            &m.flex_vertbodyid[m.flex_vertadr[f] as usize..]
        };
        for j in 0..num as usize {
            let t = m.body_treeid[bodyid[j] as usize];
            if t < 0 {
                continue;
            }
            if d.tree_asleep[t as usize] >= 0 {
                if flg_wake {
                    nwoke += wake_island(
                        &mut d.tree_asleep,
                        m.ntree,
                        t,
                        K_AWAKE,
                        Some("perturbation"),
                        d.time,
                    );
                }
            } else {
                d.tree_asleep[t as usize] = K_AWAKE;
            }
        }
        // Typed closures keep the subsequent bookkeeping type-checked without
        // unreachable-code warnings at calls into the deferred rod feature.
        if (|| -> bool { crate::unported!("mj_rodHasTwist") })() {
            let nedge = m.flex_bending[m.flex_bendingadr[f] as usize] as i32 + 1;
            for e in 0..nedge {
                let jnt = (|_: &Model, _: usize, _: i32| -> i32 {
                    crate::unported!("mj_rodChainTwistJnt")
                })(m, f, e);
                if jnt < 0 {
                    continue;
                }
                let t = m.body_treeid[m.jnt_bodyid[jnt as usize] as usize];
                if t < 0 {
                    continue;
                }
                if d.tree_asleep[t as usize] >= 0 {
                    if flg_wake {
                        nwoke += wake_island(
                            &mut d.tree_asleep,
                            m.ntree,
                            t,
                            K_AWAKE,
                            Some("perturbation"),
                            d.time,
                        );
                    }
                } else {
                    d.tree_asleep[t as usize] = K_AWAKE;
                }
            }
        }
    }
    nwoke
}

// Wake sleeping trees due to changes by the user.
pub fn wake(m: &Model, d: &mut Data) -> i32 {
    let ntree = m.ntree;
    let mut nwoke = 0;
    if m.opt.enableflags & ENBL_SLEEP == 0 {
        if d.ntree_awake < ntree as i32 {
            crate::util_misc::fill_int(&mut d.tree_asleep, K_AWAKE, ntree);
        }
        return ntree as i32 - d.ntree_awake;
    }
    if d.ntree_awake < ntree as i32 {
        nwoke += wake_non_metric_flex(m, d, true);
    }
    for i in 0..ntree {
        let asleep = d.tree_asleep[i] >= 0;
        if !asleep {
            continue;
        }
        if d.tree_awake[i] != 0 || tree_can_sleep(m, d, i, 0.0) == 0 {
            nwoke += wake_island(
                &mut d.tree_asleep,
                ntree,
                i as i32,
                K_AWAKE,
                Some("perturbation"),
                d.time,
            );
        }
    }
    nwoke
}

// Get a representative body from a flex contact side.
pub fn flex_body(m: &Model, con: &crate::data::Contact, side: usize) -> i32 {
    let f = con.flex[side] as usize;
    if con.vert[side] >= 0 && m.flex_interp[f] == 0 {
        return m.flex_vertbodyid[(m.flex_vertadr[f] + con.vert[side]) as usize];
    }
    if con.elem[side] >= 0 && m.flex_interp[f] == 0 {
        let dim = m.flex_dim[f] as usize;
        let edata =
            &m.flex_elem[m.flex_elemdataadr[f] as usize + con.elem[side] as usize * (dim + 1)..];
        let vertbodyid = &m.flex_vertbodyid[m.flex_vertadr[f] as usize..];
        for j in 0..=dim {
            let b = vertbodyid[edata[j] as usize];
            if m.body_treeid[b as usize] >= 0 {
                return b;
            }
        }
        return vertbodyid[edata[0] as usize];
    }
    let nodenum = m.flex_nodenum[f] as usize;
    let nodebodyid = &m.flex_nodebodyid[m.flex_nodeadr[f] as usize..];
    for j in 0..nodenum {
        let b = nodebodyid[j];
        if m.body_treeid[b as usize] >= 0 {
            return b;
        }
    }
    nodebodyid[0]
}

// Wake sleeping trees with collision contact.
pub fn wake_collision(m: &Model, d: &mut Data) -> i32 {
    let ntree = m.ntree;
    let ncon = d.ncon as usize;
    let mut nwoke = 0;
    if m.opt.enableflags & ENBL_SLEEP == 0 {
        return nwoke;
    }
    for i in 0..ncon {
        let con = &d.contact[i];
        let b1 = if con.geom[0] >= 0 {
            m.geom_bodyid[con.geom[0] as usize]
        } else {
            flex_body(m, con, 0)
        };
        let b2 = if con.geom[1] >= 0 {
            m.geom_bodyid[con.geom[1] as usize]
        } else {
            flex_body(m, con, 1)
        };
        let tree1 = m.body_treeid[b1 as usize];
        let tree2 = m.body_treeid[b2 as usize];
        if tree1 < 0 || tree2 < 0 {
            let tree = if tree1 < 0 { tree2 } else { tree1 };
            let b = if tree1 < 0 { b1 } else { b2 };
            if tree >= 0 && d.tree_awake[tree as usize] == 0 && d.body_awake[b as usize] == S_AWAKE
            {
                nwoke += wake_island(
                    &mut d.tree_asleep,
                    ntree,
                    tree,
                    K_AWAKE,
                    Some("mocap contact with"),
                    d.time,
                );
            }
            continue;
        }
        let awake1 = d.tree_awake[tree1 as usize];
        let awake2 = d.tree_awake[tree2 as usize];
        if awake1 != 0 && awake2 != 0 {
            continue;
        }
        if awake1 == 0 && awake2 == 0 {
            panic!("contact between sleeping bodies {b1} and {b2}");
        }
        let sleeping_tree = if awake1 != 0 { tree2 } else { tree1 };
        let wakeval = if awake1 != 0 {
            d.tree_asleep[tree1 as usize]
        } else {
            d.tree_asleep[tree2 as usize]
        };
        nwoke += wake_island(
            &mut d.tree_asleep,
            ntree,
            sleeping_tree,
            wakeval,
            Some("contact"),
            d.time,
        );
    }
    nwoke
}

// Wake sleeping trees with a constrained or metric tendon to a waking tree.
pub fn wake_tendon(m: &Model, d: &mut Data) -> i32 {
    let mut nwoke = 0;
    if m.opt.enableflags & ENBL_SLEEP == 0 {
        return nwoke;
    }
    for i in 0..m.ntendon {
        let treenum = m.tendon_treenum[i];
        if treenum < 2 {
            continue;
        }
        let has_limit = m.opt.disableflags & DSBL_CONSTRAINT == 0
            && m.opt.disableflags & DSBL_LIMIT == 0
            && treenum == 2
            && crate::core_util::tendon_limit(m, &d.ten_length, i) != 0;
        let has_friction = m.opt.disableflags & DSBL_CONSTRAINT == 0
            && m.opt.disableflags & DSBL_FRICTIONLOSS == 0
            && treenum == 2
            && m.tendon_frictionloss[i] > 0.0;
        let is_constraint = has_limit || has_friction;
        let is_metric =
            crate::core_util::is_metric(m) != 0 && crate::core_util::eff_tendon_possible(m, i) != 0;
        if !is_constraint && !is_metric {
            continue;
        }
        if treenum == 2 {
            let tree1 = m.tendon_treeid[2 * i];
            let tree2 = m.tendon_treeid[2 * i + 1];
            let awake1 = d.tree_awake[tree1 as usize];
            let awake2 = d.tree_awake[tree2 as usize];
            if awake1 != awake2 {
                let sleeping_tree = if awake1 != 0 { tree2 } else { tree1 };
                let wakeval = if awake1 != 0 {
                    d.tree_asleep[tree1 as usize]
                } else {
                    d.tree_asleep[tree2 as usize]
                };
                let reason = if is_constraint {
                    "tendon constraint"
                } else {
                    "tendon metric"
                };
                nwoke += wake_island(
                    &mut d.tree_asleep,
                    m.ntree,
                    sleeping_tree,
                    wakeval,
                    Some(reason),
                    d.time,
                );
            }
        } else {
            let mut any_awake = false;
            let mut wakeval = 0;
            let start = m.ten_j_rowadr[i] as usize;
            let end = start + m.ten_j_rownnz[i] as usize;
            for j in start..end {
                let tree = m.dof_treeid[m.ten_j_colind[j] as usize] as usize;
                if d.tree_awake[tree] != 0 {
                    any_awake = true;
                    wakeval = wakeval.min(d.tree_asleep[tree]);
                }
            }
            if any_awake {
                for j in start..end {
                    let tree = m.dof_treeid[m.ten_j_colind[j] as usize];
                    if d.tree_asleep[tree as usize] >= 0 {
                        nwoke += wake_island(
                            &mut d.tree_asleep,
                            m.ntree,
                            tree,
                            wakeval,
                            Some("tendon metric"),
                            d.time,
                        );
                    }
                }
            }
        }
    }
    nwoke
}

// Wake sleeping trees with an equality to a waking tree.
pub fn wake_equality(m: &Model, d: &mut Data) -> i32 {
    let mut nwoke = 0;
    if m.opt.enableflags & ENBL_SLEEP == 0 {
        return nwoke;
    }
    for i in 0..m.neq {
        if d.eq_active[i] == 0 {
            continue;
        }
        let eqtype = m.eq_type[i];
        let id1 = m.eq_obj1id[i];
        let id2 = m.eq_obj2id[i];
        let (tree1, tree2);
        // Every arm reaching the common code assigns both bodies.
        let (body1, body2);
        match eqtype {
            EQ_CONNECT | EQ_WELD => {
                if m.eq_objtype[i] == OBJ_BODY {
                    body1 = id1;
                    body2 = id2;
                } else {
                    body1 = m.site_bodyid[id1 as usize];
                    body2 = m.site_bodyid[id2 as usize];
                }
                tree1 = m.body_treeid[body1 as usize];
                tree2 = m.body_treeid[body2 as usize];
            }
            EQ_JOINT => {
                body1 = if id1 >= 0 {
                    m.jnt_bodyid[id1 as usize]
                } else {
                    -1
                };
                body2 = if id2 >= 0 {
                    m.jnt_bodyid[id2 as usize]
                } else {
                    -1
                };
                tree1 = if body1 >= 0 {
                    m.body_treeid[body1 as usize]
                } else {
                    -1
                };
                tree2 = if body2 >= 0 {
                    m.body_treeid[body2 as usize]
                } else {
                    -1
                };
            }
            EQ_TENDON => panic!("tendon equality does not yet support sleeping"),
            EQ_FLEX | EQ_FLEXSTRAIN => {
                let f = id1 as usize;
                let (num, adr, bodyid);
                if m.flex_interp[f] != 0 {
                    num = m.flex_nodenum[f] as usize;
                    adr = m.flex_nodeadr[f] as usize;
                    bodyid = &m.flex_nodebodyid;
                } else {
                    num = m.flex_vertnum[f] as usize;
                    adr = m.flex_vertadr[f] as usize;
                    bodyid = &m.flex_vertbodyid;
                }
                let nedge = if (|| -> bool { crate::unported!("mj_rodHasTwist") })() {
                    m.flex_bending[m.flex_bendingadr[f] as usize] as i32 + 1
                } else {
                    0
                };
                let mut any_awake = false;
                let mut wakeval = 0;
                for j in 0..num {
                    let treeid = m.body_treeid[bodyid[adr + j] as usize];
                    if treeid >= 0 && d.tree_awake[treeid as usize] != 0 {
                        any_awake = true;
                        wakeval = wakeval.min(d.tree_asleep[treeid as usize]);
                    }
                }
                for e in 0..nedge {
                    let jnt = (|_: &Model, _: usize, _: i32| -> i32 {
                        crate::unported!("mj_rodChainTwistJnt")
                    })(m, f, e);
                    let treeid = if jnt >= 0 {
                        m.body_treeid[m.jnt_bodyid[jnt as usize] as usize]
                    } else {
                        -1
                    };
                    if treeid >= 0 && d.tree_awake[treeid as usize] != 0 {
                        any_awake = true;
                        wakeval = wakeval.min(d.tree_asleep[treeid as usize]);
                    }
                }
                if any_awake {
                    for j in 0..num {
                        let treeid = m.body_treeid[bodyid[adr + j] as usize];
                        if treeid >= 0 && d.tree_asleep[treeid as usize] >= 0 {
                            nwoke += wake_island(
                                &mut d.tree_asleep,
                                m.ntree,
                                treeid,
                                wakeval,
                                Some("flex equality"),
                                d.time,
                            );
                        }
                    }
                    for e in 0..nedge {
                        let jnt = (|_: &Model, _: usize, _: i32| -> i32 {
                            crate::unported!("mj_rodChainTwistJnt")
                        })(m, f, e);
                        let treeid = if jnt >= 0 {
                            m.body_treeid[m.jnt_bodyid[jnt as usize] as usize]
                        } else {
                            -1
                        };
                        if treeid >= 0 && d.tree_asleep[treeid as usize] >= 0 {
                            nwoke += wake_island(
                                &mut d.tree_asleep,
                                m.ntree,
                                treeid,
                                wakeval,
                                Some("flex equality"),
                                d.time,
                            );
                        }
                    }
                }
                continue;
            }
            _ => continue,
        }
        let s1 = if tree1 >= 0 {
            d.tree_awake[tree1 as usize]
        } else if body1 >= 0 {
            d.body_awake[body1 as usize]
        } else {
            S_STATIC
        };
        let s2 = if tree2 >= 0 {
            d.tree_awake[tree2 as usize]
        } else if body2 >= 0 {
            d.body_awake[body2 as usize]
        } else {
            S_STATIC
        };
        if s1 != S_ASLEEP && s2 != S_ASLEEP {
            continue;
        }
        if s1 == S_STATIC || s2 == S_STATIC {
            continue;
        }
        if tree1 == tree2 {
            continue;
        }
        if s1 == S_ASLEEP && s2 == S_ASLEEP {
            let cycle1 = sleep_cycle(&d.tree_asleep, m.ntree, tree1);
            let cycle2 = sleep_cycle(&d.tree_asleep, m.ntree, tree2);
            if cycle1 != cycle2 {
                let nwoke1 = wake_island(
                    &mut d.tree_asleep,
                    m.ntree,
                    tree1,
                    K_AWAKE,
                    Some("equality"),
                    d.time,
                );
                let nwoke2 = wake_island(
                    &mut d.tree_asleep,
                    m.ntree,
                    tree2,
                    K_AWAKE,
                    Some("equality"),
                    d.time,
                );
                nwoke += nwoke1 + nwoke2;
            }
            continue;
        }
        let sleeping_tree = if s1 == S_ASLEEP { tree1 } else { tree2 };
        nwoke += wake_island(
            &mut d.tree_asleep,
            m.ntree,
            sleeping_tree,
            K_AWAKE,
            Some("equality"),
            d.time,
        );
    }
    nwoke
}

// Put n trees to sleep (create cycle), zero their velocity and acceleration.
fn sleep_trees(m: &Model, d: &mut Data, tree: &[i32], n: usize) {
    for i in 0..n {
        let current = tree[i] as usize;
        let next = if i == n - 1 { tree[0] } else { tree[i + 1] };
        if d.tree_asleep[current] == -1 {
            d.tree_asleep[current] = next;
        } else if d.tree_asleep[current] >= 0 {
            panic!("trying to sleep tree {i} which is already asleep");
        } else {
            panic!("trying to sleep tree {i} which is not ready to sleep");
        }
        let adr = m.tree_dofadr[current] as usize;
        let num = m.tree_dofnum[current] as usize;
        crate::util_blas::zero(&mut d.qvel[adr..], num);
        crate::util_blas::zero(&mut d.qacc[adr..], num);
        crate::util_blas::zero(&mut d.cdof_dot[6 * adr..], 6 * num);
        let badr = m.tree_bodyadr[current] as usize;
        let bnum = m.tree_bodynum[current] as usize;
        crate::util_blas::zero(&mut d.cvel[6 * badr..], 6 * bnum);
    }
    if crate::util_errmem::is_topic_enabled(TOPIC_SLEEP) {
        let mut buf = format!("t={}, slept tree{} ", log_time(d.time, 2), plural(n));
        for i in 0..n {
            if buf.len() < 1023 {
                buf.push_str(&format!("{}{}", tree[i], if i == n - 1 { "" } else { " " }));
            }
        }
        let mut msg = crate::util_errmem::LogMessage {
            level: LOG_DEBUG,
            topic: TOPIC_SLEEP,
            func: Some("mj_sleepTrees"),
            ..Default::default()
        };
        crate::util_misc::strncpy(&mut msg.subject, buf.as_bytes(), 1024);
        crate::util_errmem::message(&msg);
    }
}

// Put trees to sleep according to tolerance; return the number slept.
pub fn sleep(m: &Model, d: &mut Data) -> i32 {
    let ntree = m.ntree;
    let nisland = d.nisland as usize;
    let mut nslept = 0;
    if m.opt.enableflags & ENBL_SLEEP == 0 {
        return nslept;
    }
    if d.nefc != 0 && nisland == 0 {
        return nslept;
    }
    for i in 0..ntree {
        if d.tree_asleep[i] >= 0 {
            continue;
        }
        if tree_can_sleep(m, d, i, m.opt.sleep_tolerance) != 0 {
            d.tree_asleep[i] += i32::from(d.tree_asleep[i] < -1);
        } else {
            d.tree_asleep[i] = -(1 + MINAWAKE as i32);
        }
    }
    wake_non_metric_flex(m, d, false);
    for i in 0..nisland {
        let mut can_sleep = true;
        let start = d.island_itreeadr[i] as usize;
        let end = start + d.island_ntree[i] as usize;
        for j in start..end {
            let tree_asleep = d.tree_asleep[d.map_itree2tree[j] as usize];
            if tree_asleep < -1 {
                can_sleep = false;
                break;
            } else if tree_asleep >= 0 {
                panic!("found sleeping tree {} in island {i}", d.map_itree2tree[j]);
            }
        }
        if can_sleep {
            let n = d.island_ntree[i] as usize;
            // Copy the aliased input into reusable scratch before mutating Data.
            let mut tree = d.scratch.int(n);
            tree[..n].copy_from_slice(&d.map_itree2tree[start..start + n]);
            sleep_trees(m, d, &tree, n);
            d.scratch.put_int(tree);
            nslept += n as i32;
        }
    }
    let start = if nisland != 0 {
        (d.island_itreeadr[nisland - 1] + d.island_ntree[nisland - 1]) as usize
    } else {
        0
    };
    for j in start..ntree {
        let i = if nisland != 0 {
            d.map_itree2tree[j]
        } else {
            j as i32
        };
        if d.tree_asleep[i as usize] == -1 {
            sleep_trees(m, d, &[i], 1);
            nslept += 1;
        }
    }
    nslept
}

// Return sleep state of tendon i.
fn tendon_sleep_state(m: &Model, d: &Data, i: usize) -> i32 {
    let treenum = m.tendon_treenum[i];
    if treenum == 0 {
        return S_STATIC;
    }
    let id1 = m.tendon_treeid[2 * i] as usize;
    if treenum == 1 {
        return if d.tree_awake[id1] != 0 {
            S_AWAKE
        } else {
            S_ASLEEP
        };
    }
    let id2 = m.tendon_treeid[2 * i + 1] as usize;
    if treenum == 2 {
        return if d.tree_awake[id1] != 0 || d.tree_awake[id2] != 0 {
            S_AWAKE
        } else {
            S_ASLEEP
        };
    }
    S_AWAKE
}

// Return sleep state of actuator i.
fn actuator_sleep_state(m: &Model, d: &Data, i: usize) -> i32 {
    let trnid = m.actuator_trnid[i * 2];
    match m.actuator_trntype[i] {
        TRN_JOINT | TRN_JOINTINPARENT => sleep_state(m, d, OBJ_JOINT, trnid),
        TRN_SLIDERCRANK => {
            let s1 = sleep_state(m, d, OBJ_SITE, trnid);
            let s2 = sleep_state(m, d, OBJ_SITE, m.actuator_trnid[i * 2 + 1]);
            if s1 == S_AWAKE || s2 == S_AWAKE {
                S_AWAKE
            } else {
                S_ASLEEP
            }
        }
        TRN_TENDON => tendon_sleep_state(m, d, trnid as usize),
        TRN_SITE => {
            if m.actuator_trnid[i * 2 + 1] == -1 {
                return sleep_state(m, d, OBJ_SITE, trnid);
            }
            let s1 = sleep_state(m, d, OBJ_SITE, trnid);
            let s2 = sleep_state(m, d, OBJ_SITE, m.actuator_trnid[i * 2 + 1]);
            if s1 == S_AWAKE || s2 == S_AWAKE {
                return S_AWAKE;
            }
            if s1 == S_ASLEEP || s2 == S_ASLEEP {
                S_ASLEEP
            } else {
                S_STATIC
            }
        }
        TRN_SO3 => {
            if m.actuator_trnid[i * 2 + 1] == -1 {
                return sleep_state(m, d, OBJ_JOINT, trnid);
            }
            let s1 = sleep_state(m, d, OBJ_SITE, trnid);
            let s2 = sleep_state(m, d, OBJ_SITE, m.actuator_trnid[i * 2 + 1]);
            if s1 == S_AWAKE || s2 == S_AWAKE {
                S_AWAKE
            } else {
                S_ASLEEP
            }
        }
        TRN_BODY => sleep_state(m, d, OBJ_BODY, trnid),
        TRN_UNDEFINED => S_AWAKE,
        _ => S_AWAKE,
    }
}

// Return sleep state of equality i.
fn equality_sleep_state(m: &Model, d: &Data, i: usize) -> i32 {
    let eqtype = m.eq_type[i];
    let objtype = match eqtype {
        EQ_CONNECT | EQ_WELD => m.eq_objtype[i],
        EQ_JOINT => OBJ_JOINT,
        EQ_TENDON => OBJ_TENDON,
        EQ_FLEX | EQ_FLEXSTRAIN => OBJ_FLEX,
        _ => return S_AWAKE,
    };
    let id1 = m.eq_obj1id[i];
    let id2 = m.eq_obj2id[i];
    let s1 = if id1 >= 0 {
        sleep_state(m, d, objtype, id1)
    } else {
        S_STATIC
    };
    let s2 = if id2 >= 0 {
        sleep_state(m, d, objtype, id2)
    } else {
        S_STATIC
    };
    let neither_awake = s1 != S_AWAKE && s2 != S_AWAKE;
    if neither_awake {
        S_ASLEEP
    } else {
        S_AWAKE
    }
}

// Return sleep state of the kinematic subtree rooted at body i.
fn subtree_sleep_state(m: &Model, d: &Data, i: usize) -> i32 {
    if d.body_awake[i] != S_STATIC {
        return d.body_awake[i];
    }
    let mut has_asleep = false;
    for b in i + 1..m.nbody {
        let mut p = b;
        while p > i {
            p = m.body_parentid[p] as usize;
        }
        if p != i {
            break;
        }
        if d.body_awake[b] == S_AWAKE {
            return S_AWAKE;
        }
        if d.body_awake[b] == S_ASLEEP {
            has_asleep = true;
        }
    }
    if has_asleep {
        S_ASLEEP
    } else {
        S_STATIC
    }
}

// Return sleep state of sensor i (never STATIC).
fn sensor_sleep_state(m: &Model, d: &Data, i: usize) -> i32 {
    let type_ = m.sensor_type[i];
    let objtype = m.sensor_objtype[i];
    let reftype = m.sensor_reftype[i];
    let objid = m.sensor_objid[i];
    let refid = m.sensor_refid[i];
    match type_ {
        SENS_USER | SENS_PLUGIN => return S_AWAKE,
        SENS_CONTACT => {
            if objtype == OBJ_SITE || reftype == OBJ_SITE {
                return S_AWAKE;
            }
        }
        SENS_RANGEFINDER => return S_AWAKE,
        SENS_TACTILE => {
            return if sleep_state(m, d, reftype, refid) == S_ASLEEP {
                S_ASLEEP
            } else {
                S_AWAKE
            }
        }
        _ => {}
    }
    let s_obj = if (type_ == SENS_CONTACT && objtype == OBJ_XBODY)
        || (type_ == SENS_INSIDESITE && objtype == OBJ_XBODY)
        || (type_ == SENS_INSIDESITE
            && objtype == OBJ_BODY
            && objid > 0
            && m.body_mass[objid as usize] < MINVAL
            && m.body_subtreemass[objid as usize] >= MINVAL)
    {
        subtree_sleep_state(m, d, objid as usize)
    } else {
        sleep_state(m, d, objtype, objid)
    };
    let s_ref = if type_ == SENS_CONTACT && reftype == OBJ_XBODY {
        subtree_sleep_state(m, d, refid as usize)
    } else {
        sleep_state(m, d, reftype, refid)
    };
    if objtype == OBJ_UNKNOWN && reftype == OBJ_UNKNOWN {
        return S_AWAKE;
    }
    if objtype == OBJ_UNKNOWN {
        return if s_ref == S_ASLEEP { S_ASLEEP } else { S_AWAKE };
    } else if reftype == OBJ_UNKNOWN {
        return if s_obj == S_ASLEEP { S_ASLEEP } else { S_AWAKE };
    }
    if s_obj == S_AWAKE || s_ref == S_AWAKE {
        return S_AWAKE;
    }
    S_ASLEEP
}

// Return sleep state of object i.
pub fn sleep_state(m: &Model, d: &Data, type_: i32, i: i32) -> i32 {
    let i = i as usize;
    match type_ {
        OBJ_BODY | OBJ_XBODY => d.body_awake[i],
        OBJ_JOINT => d.body_awake[m.jnt_bodyid[i] as usize],
        OBJ_SITE => d.body_awake[m.site_bodyid[i] as usize],
        OBJ_DOF => d.body_awake[m.dof_bodyid[i] as usize],
        OBJ_GEOM => d.body_awake[m.geom_bodyid[i] as usize],
        OBJ_CAMERA => d.body_awake[m.cam_bodyid[i] as usize],
        OBJ_LIGHT => d.body_awake[m.light_bodyid[i] as usize],
        OBJ_MESH => S_STATIC,
        OBJ_EQUALITY => equality_sleep_state(m, d, i),
        OBJ_TENDON => tendon_sleep_state(m, d, i),
        OBJ_ACTUATOR => actuator_sleep_state(m, d, i),
        OBJ_SENSOR => sensor_sleep_state(m, d, i),
        OBJ_FLEX => {
            let (num, adr, bodyid);
            if m.flex_interp[i] != 0 {
                num = m.flex_nodenum[i] as usize;
                adr = m.flex_nodeadr[i] as usize;
                bodyid = &m.flex_nodebodyid;
            } else {
                num = m.flex_vertnum[i] as usize;
                adr = m.flex_vertadr[i] as usize;
                bodyid = &m.flex_vertbodyid;
            }
            let mut has_asleep = false;
            for j in 0..num {
                let b = bodyid[adr + j] as usize;
                if m.body_treeid[b] >= 0 {
                    if d.body_awake[b] == S_AWAKE {
                        return S_AWAKE;
                    }
                    has_asleep = true;
                }
            }
            if (|| -> bool { crate::unported!("mj_rodHasTwist") })() {
                let nedge = m.flex_bending[m.flex_bendingadr[i] as usize] as i32 + 1;
                for e in 0..nedge {
                    let jnt = (|_: &Model, _: usize, _: i32| -> i32 {
                        crate::unported!("mj_rodChainTwistJnt")
                    })(m, i, e);
                    if jnt >= 0 {
                        let b = m.jnt_bodyid[jnt as usize] as usize;
                        if m.body_treeid[b] >= 0 {
                            if d.body_awake[b] == S_AWAKE {
                                return S_AWAKE;
                            }
                            has_asleep = true;
                        }
                    }
                }
            }
            if has_asleep {
                S_ASLEEP
            } else {
                S_STATIC
            }
        }
        OBJ_UNKNOWN => S_AWAKE,
        _ => {
            if let Some(typename) = crate::util_misc::type2_str(type_) {
                panic!("unsupported object type '{typename}'");
            } else {
                panic!("unsupported object type {type_}");
            }
        }
    }
}
