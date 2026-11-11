//! Sparse model structures, data creation and reset, and reference validation.
//!
//! Translated from `src/engine/engine_io.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust; owned storage replaces allocation and pointer setup.

use crate::data::{Data, SolverStat, TimerStat, WarningStat};
use crate::model::{enabled, Model};
use crate::types::*;
use crate::util_errmem::{error, warning_text};

const K_AWAKE: i32 = -(1 + MINAWAKE as i32);
const N_POS: [usize; 4] = [7, 4, 1, 1];
const N_VEL: [usize; 4] = [6, 3, 1, 1];

pub fn make_dof_dof_sparse(
    nv: usize,
    n_c: usize,
    n_d: usize,
    n_m: usize,
    dof_parentid: &[i32],
    dof_simplenum: &[i32],
    rownnz: &mut [i32],
    rowadr: &mut [i32],
    diag: Option<&mut [i32]>,
    colind: &mut [i32],
    reduced: bool,
    upper: bool,
    remaining: &mut [i32],
) {
    if nv == 0 {
        return;
    }
    rownnz[..nv].fill(0);
    for i in (0..nv).rev() {
        let mut j = i as i32;
        rownnz[i] += 1;
        if !(reduced && dof_simplenum[i] != 0) {
            loop {
                j = dof_parentid[j as usize];
                if j < 0 {
                    break;
                }
                rownnz[i] += 1;
                if upper {
                    rownnz[j as usize] += 1;
                }
            }
        }
    }
    rowadr[0] = 0;
    for i in 1..nv {
        rowadr[i] = rowadr[i - 1] + rownnz[i - 1];
    }
    remaining[..nv].copy_from_slice(&rownnz[..nv]);
    for i in (0..nv).rev() {
        remaining[i] -= 1;
        colind[(rowadr[i] + remaining[i]) as usize] = i as i32;
        if !(reduced && dof_simplenum[i] != 0) {
            let mut j = i as i32;
            loop {
                j = dof_parentid[j as usize];
                if j < 0 {
                    break;
                }
                remaining[i] -= 1;
                colind[(rowadr[i] + remaining[i]) as usize] = j;
                if upper {
                    remaining[j as usize] -= 1;
                    colind[(rowadr[j as usize] + remaining[j as usize]) as usize] = i as i32;
                }
            }
        }
    }
    for i in 0..nv {
        if remaining[i] != 0 {
            error("unexpected remaining");
        }
    }
    let expected_nnz = if upper {
        n_d
    } else if reduced {
        n_c
    } else {
        n_m
    };
    if (rowadr[nv - 1] + rownnz[nv - 1]) as usize != expected_nnz {
        error("sum of rownnz different from expected");
    }
    if let Some(diag) = diag {
        for i in 0..nv {
            let adr = rowadr[i] as usize;
            let mut j = 0;
            while colind[adr + j] < i as i32 && j < rownnz[i] as usize {
                j += 1;
            }
            if colind[adr + j] != i as i32 {
                error("diagonal index not found");
            }
            diag[i] = j as i32;
        }
    }
}

pub fn make_b_sparse(
    nv: usize,
    nbody: usize,
    n_b: usize,
    body_dofnum: &[i32],
    body_parentid: &[i32],
    body_dofadr: &[i32],
    b_rownnz: &mut [i32],
    b_rowadr: &mut [i32],
    b_colind: &mut [i32],
    count: &mut [i32],
) {
    b_rownnz[..nbody].fill(0);
    for i in (1..nbody).rev() {
        b_rownnz[i] += body_dofnum[i];
        b_rownnz[body_parentid[i] as usize] += b_rownnz[i];
    }
    if b_rownnz[0] != nv as i32 {
        error("rownnz[0] different from nv");
    }
    for i in 0..nbody {
        let mut j = body_parentid[i];
        while j > 0 {
            b_rownnz[i] += body_dofnum[j as usize];
            j = body_parentid[j as usize];
        }
    }
    b_rowadr[0] = 0;
    for i in 1..nbody {
        b_rowadr[i] = b_rowadr[i - 1] + b_rownnz[i - 1];
    }
    if n_b != (b_rowadr[nbody - 1] + b_rownnz[nbody - 1]) as usize {
        error("sum of rownnz different from nB");
    }
    count[..nbody].fill(0);
    for i in (1..nbody).rev() {
        for n in 0..body_dofnum[i] {
            b_colind[(b_rowadr[i] + count[i]) as usize] = body_dofadr[i] + n;
            count[i] += 1;
        }
        let par = body_parentid[i] as usize;
        for n in 0..count[i] {
            b_colind[(b_rowadr[par] + count[par]) as usize] = b_colind[(b_rowadr[i] + n) as usize];
            count[par] += 1;
        }
    }
    for i in 0..nbody {
        let mut par = body_parentid[i];
        while par > 0 {
            for n in 0..body_dofnum[par as usize] {
                b_colind[(b_rowadr[i] + count[i]) as usize] = body_dofadr[par as usize] + n;
                count[i] += 1;
            }
            par = body_parentid[par as usize];
        }
    }
    for i in 0..nbody {
        if b_rownnz[i] != count[i] {
            error("cnt different from rownnz");
        }
        if count[i] > 1 {
            crate::util_misc::insertion_sort_int(
                &mut b_colind[b_rowadr[i] as usize..],
                count[i] as usize,
            );
        }
    }
}

fn check_db_sparse(m: &Model) {
    for j in 0..m.nv {
        let i = m.dof_bodyid[j] as usize;
        if m.d_rownnz[j] != m.b_rownnz[i] {
            error("rows have different nnz");
        }
        for k in 0..m.d_rownnz[j] {
            if m.d_colind[(m.d_rowadr[j] + k) as usize] != m.b_colind[(m.b_rowadr[i] + k) as usize]
            {
                error("rows have different colind");
            }
        }
    }
}

fn copy_m2_sparse(
    nv: usize,
    dof_madr: &[i32],
    dof_simplenum: &[i32],
    dof_parentid: &[i32],
    rownnz: &[i32],
    rowadr: &[i32],
    src: &[i32],
    dst: &mut [i32],
    reduced: bool,
    upper: bool,
    remaining: &mut [i32],
) {
    remaining[..nv].copy_from_slice(&rownnz[..nv]);
    for i in (0..nv).rev() {
        let mut adr = dof_madr[i] as usize;
        remaining[i] -= 1;
        dst[(rowadr[i] + remaining[i]) as usize] = src[adr];
        adr += 1;
        if !(reduced && dof_simplenum[i] != 0) {
            let mut j = i as i32;
            loop {
                j = dof_parentid[j as usize];
                if j < 0 {
                    break;
                }
                remaining[i] -= 1;
                dst[(rowadr[i] + remaining[i]) as usize] = src[adr];
                if upper {
                    remaining[j as usize] -= 1;
                    dst[(rowadr[j as usize] + remaining[j as usize]) as usize] = src[adr];
                }
                adr += 1;
            }
        }
    }
    for i in 0..nv {
        if remaining[i] != 0 {
            error("unassigned index");
        }
    }
}

pub fn make_dof_dof_maps(
    nv: usize,
    n_m: usize,
    n_c: usize,
    _n_d: usize,
    dof_madr: &[i32],
    dof_simplenum: &[i32],
    dof_parentid: &[i32],
    d_rownnz: &[i32],
    d_rowadr: &[i32],
    d_colind: &[i32],
    m_rownnz: &[i32],
    m_rowadr: &[i32],
    m_colind: &[i32],
    map_m2d: &mut [i32],
    map_d2m: &mut [i32],
    map_m2m: &mut [i32],
    m: &mut [i32],
    scratch: &mut [i32],
) {
    crate::util_misc::lower2_sym_map(
        map_m2d, nv, d_rowadr, d_rownnz, d_colind, m_rowadr, m_rownnz, m_colind, scratch,
    );
    crate::util_misc::sparse_map(
        map_d2m, nv, m_rowadr, m_rownnz, m_colind, d_rowadr, d_rownnz, d_colind,
    );
    for i in 0..n_m {
        m[i] = i as i32;
    }
    map_m2m[..n_c].fill(-1);
    copy_m2_sparse(
        nv,
        dof_madr,
        dof_simplenum,
        dof_parentid,
        m_rownnz,
        m_rowadr,
        m,
        map_m2m,
        true,
        false,
        scratch,
    );
    for i in 0..n_c {
        if map_m2m[i] < 0 {
            error("unassigned index in mapM2M");
        }
    }
}

/// mj_makeRawData and mj_makeData; Vec allocation also replaces mj_setPtrData.
pub fn make_data(m: &Model) -> Data {
    let mut d = Data::allocate(m);
    d.tree_asleep.fill(K_AWAKE);
    crate::sleep::update_sleep_init(m, &mut d, true);
    // mj_initPlugin and initPluginOrFree are deferred with plugin callbacks.
    reset_data(m, &mut d);
    d
}

pub fn reset_plastic(m: &Model, d: &mut Data) {
    if m.nplastic == 0 {
        return;
    }
    for j in 0..m.njnt {
        let adr = m.jnt_plasticadr[j];
        if adr >= 0 {
            let n = N_POS[m.jnt_type[j] as usize];
            let qadr = m.jnt_qposadr[j] as usize;
            d.plastic[adr as usize..adr as usize + n]
                .copy_from_slice(&m.qpos_spring[qadr..qadr + n]);
        }
    }
    for i in 0..m.ntendon {
        let adr = m.tendon_plasticadr[i];
        if adr >= 0 {
            d.plastic[adr as usize..adr as usize + 2]
                .copy_from_slice(&m.tendon_lengthspring[2 * i..2 * i + 2]);
        }
    }
    for f in 0..m.nflex {
        let adr = m.flex_plasticadr[f];
        if adr >= 0 {
            let dim = m.flex_dim[f];
            let mut n = 0;
            if m.flex_interp[f] > 0 {
                n = 6
                    * m.flex_cellnum[3 * f]
                    * m.flex_cellnum[3 * f + 1]
                    * m.flex_cellnum[3 * f + 2];
            } else if m.flex_interp[f] < 0 {
                let cx = m.flex_cellnum[3 * f];
                let cy = m.flex_cellnum[3 * f + 1];
                let cz = m.flex_cellnum[3 * f + 2];
                n = 3 * 2 * (cy * cz + cx * cz + cx * cy);
            } else if dim == 1 {
                n = crate::core_util::rod_state_size(m, f);
            } else {
                if m.flex_plasticmode[f] & 2 != 0 {
                    n += (if dim == 2 { 3 } else { 6 }) * m.flex_elemnum[f];
                }
                if m.flex_plasticmode[f] & 1 != 0 {
                    n += m.flex_edgenum[f];
                }
            }
            d.plastic[adr as usize..(adr + n) as usize].fill(0.0);
            d.flex_hessian_valid[f] = 0;
        }
    }
}

fn reset_data_internal(m: &Model, d: &mut Data, debug_value: u8) {
    let dt = m.opt.timestep;
    if m.nhistory != 0 && dt <= 0.0 {
        warning_text(&format!(
            "history buffers require positive timestep, got {dt}"
        ));
    }
    let plugin_state = if d.nplugin != 0 {
        Some(d.plugin_state.clone())
    } else {
        None
    };
    let plugindata = if d.nplugin != 0 {
        Some(d.plugin_data.clone())
    } else {
        None
    };
    d.pstack = 0;
    d.pbase = 0;
    d.threadlock = 0;
    d.parena = 0;
    // Arena arrays retain their storage. The zero counts below invalidate their contents,
    // replacing C's NULL pointers without losing capacity on the next allocation.
    d.maxuse_stack = 0;
    d.maxuse_arena = 0;
    d.maxuse_con = 0;
    d.maxuse_efc = 0;
    d.warning.fill(WarningStat::default());
    d.timer.fill(TimerStat::default());
    d.solver.fill(SolverStat::default());
    d.solver_niter.fill(0);
    d.solver_nnz.fill(0);
    d.solver_fwdinv.fill(0.0);
    d.ncon = 0;
    d.sdf_seeds.clear();
    d.sdf_walks_bounded = 0;
    d.ne = 0;
    d.nf = 0;
    d.nl = 0;
    d.nefc = 0;
    d.n_j = 0;
    d.n_y = 0;
    d.n_a = 0;
    d.nisland = 0;
    d.nidof = 0;
    d.efm_active = 0;
    d.nefm_k = 0;
    d.nefmcon = 0;
    d.nefm_t = 0;
    d.nefm_a = 0;
    d.nefmdof = 0;
    d.nefm_l = 0;
    d.time = 0.0;
    d.energy.fill(0.0);
    d.flg_energypos = 0;
    d.flg_energyvel = 0;
    d.flg_subtreevel = 0;
    d.flg_rnepost = 0;
    d.fill_buffer(debug_value);
    // Pointer storage is inert until plugins are implemented.
    d.plugin_data.fill(0);
    d.qpos.fill(0.0);
    d.qvel.fill(0.0);
    d.act.fill(0.0);
    d.plastic.fill(0.0);
    reset_ctrl(m, d);
    d.eq_active.copy_from_slice(&m.eq_active0);
    d.qfrc_applied.fill(0.0);
    d.xfrc_applied.fill(0.0);
    d.qacc.fill(0.0);
    d.qacc_warmstart.fill(0.0);
    d.act_dot.fill(0.0);
    d.userdata.fill(0.0);
    d.mocap_pos.fill(0.0);
    d.mocap_quat.fill(0.0);
    d.flexvert_lambda.fill(0.0);
    d.flexvert_conage.fill(0);
    d.flex_hessian_valid.fill(0);

    for i in 0..m.nactuator {
        let n = m.actuator_history[2 * i];
        if n > 0 {
            let buf = &mut d.history[m.actuator_historyadr[i] as usize..];
            buf[0] = 0.0;
            buf[1] = (n - 1) as f64;
            for j in 0..n {
                buf[2 + j as usize] = -((n - j) as f64) * dt;
            }
            let dim = m.actuator_ctrlnum[i] as usize;
            let values = &mut buf[2 + n as usize..];
            values[..n as usize * dim].fill(0.0);
            if m.actuator_gaintype[i] == GAIN_SO3 && m.actuator_ctrlspec[i] == CHART_QUAT {
                for j in 0..n as usize {
                    values[4 * j] = 1.0;
                }
            }
        }
    }
    for i in 0..m.nsensor {
        let n = m.sensor_history[2 * i];
        if n > 0 {
            let dim = m.sensor_dim[i] as usize;
            let period = m.sensor_interval[2 * i];
            let phase = m.sensor_interval[2 * i + 1];
            let buf = &mut d.history[m.sensor_historyadr[i] as usize..];
            let t0 = if phase == 0.0 { -period } else { phase };
            buf[0] = if period > 0.0 { t0 } else { -dt };
            buf[1] = (n - 1) as f64;
            let times = &mut buf[2..2 + n as usize];
            if period > 0.0 {
                if dt <= 0.0 {
                    for j in 0..n {
                        times[j as usize] = t0 - (n - 1 - j) as f64 * period;
                    }
                } else if period <= dt {
                    let offset = if phase == 0.0 { n } else { n - 1 };
                    for j in 0..n {
                        times[j as usize] = -((offset - j) as f64) * dt;
                    }
                } else {
                    let mut next_k = 0.0;
                    let reltol = 1e-12;
                    for j in (0..n).rev() {
                        let mut r = (t0 - (n - 1 - j) as f64 * period) / dt;
                        let r_round = (r + 0.5).floor();
                        if (r - r_round).abs()
                            <= reltol * (if 1.0 > r.abs() { 1.0 } else { r.abs() })
                        {
                            r = r_round;
                        }
                        let mut k = r.ceil();
                        if j < n - 1 && k >= next_k {
                            k = next_k - 1.0;
                        }
                        times[j as usize] = k * dt;
                        next_k = k;
                    }
                }
            } else {
                for j in 0..n {
                    times[j as usize] = -((n - j) as f64) * dt;
                }
            }
            buf[2 + n as usize..2 + n as usize + n as usize * dim].fill(0.0);
        }
    }
    d.qpos.copy_from_slice(&m.qpos0);
    reset_plastic(m, d);
    if let Some(state) = plugin_state {
        d.plugin_state.copy_from_slice(&state);
        d.plugin_data.copy_from_slice(plugindata.as_ref().unwrap());
        d.plugin.copy_from_slice(&m.plugin);
        // Plugin reset callbacks are deferred with mj_initPlugin.
    }
    d.tree_asleep.fill(K_AWAKE);
    // sleep enabled: handle static bodies and trees marked as mjSLEEP_INIT
    if enabled(m, ENBL_SLEEP) {
        // count trees initialized as asleep
        let mut num_asleep_init = 0;
        for i in 0..m.ntree {
            num_asleep_init += (m.tree_sleep_policy[i] == SLEEP_INIT) as i32;
        }
        // update sleep arrays, treat static bodies as awake
        crate::sleep::update_sleep_init(m, d, true);
        // partial mj_fwdPosition, functions that update STATIC values
        if num_asleep_init == 0 {
            crate::core_smooth::kinematics(m, d);
            crate::core_smooth::com_pos(m, d);
            crate::core_smooth::camlight(m, d);
            crate::core_smooth::tendon(m, d);
        }
        // if any trees initialized as sleeping call entire mj_forward, put them to sleep
        else {
            crate::forward::forward(m, d);
            // mark asleep-init trees as ready to sleep
            for i in 0..m.ntree {
                let init = m.tree_sleep_policy[i] == SLEEP_INIT;
                d.tree_asleep[i] = if init { -1 } else { K_AWAKE };
            }
            let nslept = crate::sleep::sleep(m, d);
            // warn if any failed to sleep, leave them awake
            if nslept != num_asleep_init {
                // find root body of the first tree that could not be slept, wake the others
                let mut root = -1;
                for i in 0..m.ntree {
                    if m.tree_sleep_policy[i] == SLEEP_INIT && d.tree_asleep[i] < 0 {
                        if root < 0 { root = m.tree_bodyadr[i]; }
                        d.tree_asleep[i] = K_AWAKE;
                    }
                }
                // warn and continue
                let name = crate::name::id2name(m, OBJ_BODY, root).map(String::from_utf8_lossy).unwrap_or_default();
                warning_text(&format!("{num_asleep_init} trees were marked as sleep='init' but only {nslept} could be slept. \
                    Body '{name}' (id={root}) is the root of the first tree that could not be slept."));
            }
            // clear arrays to avoid MSAN errors upon mid-step wake
            d.qacc_smooth.fill(0.0);
            d.qfrc_smooth.fill(0.0);
            // clear arena
            crate::memory::clear_efc(d);
        }
    }
    crate::sleep::update_sleep(m, d);
    for i in 0..m.nbody {
        let id = m.body_mocapid[i];
        if id >= 0 {
            let id = id as usize;
            d.mocap_pos[3 * id..3 * id + 3].copy_from_slice(&m.body_pos[3 * i..3 * i + 3]);
            d.mocap_quat[4 * id..4 * id + 4].copy_from_slice(&m.body_quat[4 * i..4 * i + 4]);
        }
    }
    // The C NULL-body_mocapid fallback cannot occur for an allocated Model.
    check_db_sparse(m);
    d.signature = m.signature;
}

pub fn reset_ctrl(m: &Model, d: &mut Data) {
    d.ctrl.fill(0.0);
    for i in 0..m.nactuator {
        if m.actuator_gaintype[i] == GAIN_SO3 && m.actuator_ctrlspec[i] == CHART_QUAT {
            d.ctrl[m.actuator_ctrladr[i] as usize] = 1.0;
        }
    }
}

pub fn reset_data(m: &Model, d: &mut Data) {
    reset_data_internal(m, d, 0);
}

pub fn reset_data_debug(m: &Model, d: &mut Data, debug_value: u8) {
    reset_data_internal(m, d, debug_value);
}

pub fn reset_data_keyframe(m: &Model, d: &mut Data, key: i32) {
    reset_data_internal(m, d, 0);
    if key >= 0 && (key as usize) < m.nkey {
        let key = key as usize;
        d.time = m.key_time[key];
        d.qpos
            .copy_from_slice(&m.key_qpos[key * m.nq..(key + 1) * m.nq]);
        d.qvel
            .copy_from_slice(&m.key_qvel[key * m.nv..(key + 1) * m.nv]);
        d.act
            .copy_from_slice(&m.key_act[key * m.na..(key + 1) * m.na]);
        d.mocap_pos
            .copy_from_slice(&m.key_mpos[key * 3 * m.nmocap..(key + 1) * 3 * m.nmocap]);
        d.mocap_quat
            .copy_from_slice(&m.key_mquat[key * 4 * m.nmocap..(key + 1) * 4 * m.nmocap]);
        d.ctrl
            .copy_from_slice(&m.key_ctrl[key * m.nu..(key + 1) * m.nu]);
        d.plastic
            .copy_from_slice(&m.key_plastic[key * m.nplastic..(key + 1) * m.nplastic]);
    }
}

fn sensor_size(sensor_type: i32, sensor_dim: i32) -> i32 {
    match sensor_type {
        SENS_TOUCH | SENS_RANGEFINDER | SENS_JOINTPOS | SENS_JOINTVEL | SENS_TENDONPOS
        | SENS_TENDONVEL | SENS_ACTUATORPOS | SENS_ACTUATORVEL | SENS_ACTUATORFRC
        | SENS_JOINTACTFRC | SENS_TENDONACTFRC | SENS_JOINTLIMITPOS | SENS_JOINTLIMITVEL
        | SENS_JOINTLIMITFRC | SENS_TENDONLIMITPOS | SENS_TENDONLIMITVEL | SENS_TENDONLIMITFRC
        | SENS_GEOMDIST | SENS_INSIDESITE | SENS_E_POTENTIAL | SENS_E_KINETIC | SENS_CLOCK => 1,
        SENS_CAMPROJECTION => 2,
        SENS_ACCELEROMETER | SENS_VELOCIMETER | SENS_GYRO | SENS_FORCE | SENS_TORQUE
        | SENS_MAGNETOMETER | SENS_BALLANGVEL | SENS_FRAMEPOS | SENS_FRAMEXAXIS
        | SENS_FRAMEYAXIS | SENS_FRAMEZAXIS | SENS_FRAMELINVEL | SENS_FRAMEANGVEL
        | SENS_FRAMELINACC | SENS_FRAMEANGACC | SENS_SUBTREECOM | SENS_SUBTREELINVEL
        | SENS_SUBTREEANGMOM | SENS_GEOMNORMAL => 3,
        SENS_GEOMFROMTO => 6,
        SENS_BALLQUAT | SENS_FRAMEQUAT => 4,
        SENS_CONTACT | SENS_TACTILE | SENS_USER => sensor_dim,
        SENS_PLUGIN => -1,
        _ => -1,
    }
}

fn num_objects(m: &Model, objtype: i32) -> i32 {
    match objtype {
        OBJ_DEFAULT | OBJ_FRAME | OBJ_UNKNOWN | OBJ_MODEL => -1,
        OBJ_BODY | OBJ_XBODY => m.nbody as i32,
        OBJ_JOINT => m.njnt as i32,
        OBJ_DOF => m.nv as i32,
        OBJ_GEOM => m.ngeom as i32,
        OBJ_SITE => m.nsite as i32,
        OBJ_CAMERA => m.ncam as i32,
        OBJ_LIGHT => m.nlight as i32,
        OBJ_FLEX => m.nflex as i32,
        OBJ_MESH => m.nmesh as i32,
        OBJ_SKIN => m.nskin as i32,
        OBJ_HFIELD => m.nhfield as i32,
        OBJ_TEXTURE => m.ntex as i32,
        OBJ_MATERIAL => m.nmat as i32,
        OBJ_PAIR => m.npair as i32,
        OBJ_EXCLUDE => m.nexclude as i32,
        OBJ_EQUALITY => m.neq as i32,
        OBJ_TENDON => m.ntendon as i32,
        OBJ_ACTUATOR => m.nactuator as i32,
        OBJ_SENSOR => m.nsensor as i32,
        OBJ_NUMERIC => m.nnumeric as i32,
        OBJ_TEXT => m.ntext as i32,
        OBJ_TUPLE => m.ntuple as i32,
        OBJ_KEY => m.nkey as i32,
        OBJ_PLUGIN => m.nplugin as i32,
        NOBJECT => -2,
        _ => -2,
    }
}

fn validate_sparse(
    nrows: usize,
    ncols: usize,
    nnz: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    strict: bool,
) -> i32 {
    for r in 0..nrows {
        let nnz_r = rownnz[r];
        if nnz_r < 0 || nnz_r as usize > ncols {
            return 1;
        }
        let adr_r = rowadr[r];
        if adr_r < 0 || adr_r as usize + nnz_r as usize > nnz {
            return 2;
        }
        for k in 0..nnz_r as usize {
            let col = colind[adr_r as usize + k];
            if col < 0 || col as usize >= ncols {
                return 3;
            }
            if k > 0 {
                let prev = colind[adr_r as usize + k - 1];
                if if strict { col <= prev } else { col < prev } {
                    return 3;
                }
            }
        }
    }
    0
}

pub fn validate_references(m: &Model) -> Result<(), String> {
    // MJMODEL_REFERENCES; keep C field spellings in messages.
    macro_rules! references {
        ($($name:ident, $nadrs:expr, $ntarget:expr, $nums:expr, $cname:literal, $cnums:literal;)*) => {
            $(for i in 0..$nadrs {
                let nums: Option<&[i32]> = $nums;
                let adrsmin = m.$name[i] as i64;
                let num = nums.map_or(1, |nums| nums[i]) as i64;
                if num < 0 { return Err(concat!("Invalid model: ", $cnums, " is negative.").into()); }
                if num > i32::MAX as i64 { return Err(concat!("Invalid model: ", $cnums, " is too large.").into()); }
                let adrsmax = adrsmin + num;
                if adrsmax > $ntarget as i64 || adrsmin < -1 || (nums.is_some() && adrsmin == -1 && num > 0) {
                    return Err(concat!("Invalid model: ", $cname, " out of bounds.").into());
                }
            })*
        };
    }
    references! {
        body_parentid, m.nbody, m.nbody, None, "body_parentid", "0";
        body_rootid, m.nbody, m.nbody, None, "body_rootid", "0";
        body_weldid, m.nbody, m.nbody, None, "body_weldid", "0";
        body_mocapid, m.nbody, m.nmocap, None, "body_mocapid", "0";
        body_jntadr, m.nbody, m.njnt, Some(&m.body_jntnum), "body_jntadr", "m->body_jntnum";
        body_dofadr, m.nbody, m.nv, Some(&m.body_dofnum), "body_dofadr", "m->body_dofnum";
        body_geomadr, m.nbody, m.ngeom, Some(&m.body_geomnum), "body_geomadr", "m->body_geomnum";
        body_bvhadr, m.nbody, m.nbvh, Some(&m.body_bvhnum), "body_bvhadr", "m->body_bvhnum";
        body_plugin, m.nbody, m.nplugin, None, "body_plugin", "0";
        jnt_qposadr, m.njnt, m.nq, None, "jnt_qposadr", "0";
        jnt_dofadr, m.njnt, m.nv, None, "jnt_dofadr", "0";
        jnt_bodyid, m.njnt, m.nbody, None, "jnt_bodyid", "0";
        dof_bodyid, m.nv, m.nbody, None, "dof_bodyid", "0";
        dof_jntid, m.nv, m.njnt, None, "dof_jntid", "0";
        dof_parentid, m.nv, m.nv, None, "dof_parentid", "0";
        dof_madr, m.nv, m.n_m, None, "dof_Madr", "0";
        tree_bodyadr, m.ntree, m.nbody, Some(&m.tree_bodynum), "tree_bodyadr", "m->tree_bodynum";
        tree_dofadr, m.ntree, m.nv, Some(&m.tree_dofnum), "tree_dofadr", "m->tree_dofnum";
        geom_bodyid, m.ngeom, m.nbody, None, "geom_bodyid", "0";
        geom_matid, m.ngeom, m.nmat, None, "geom_matid", "0";
        site_bodyid, m.nsite, m.nbody, None, "site_bodyid", "0";
        site_matid, m.nsite, m.nmat, None, "site_matid", "0";
        cam_bodyid, m.ncam, m.nbody, None, "cam_bodyid", "0";
        cam_targetbodyid, m.ncam, m.nbody, None, "cam_targetbodyid", "0";
        light_bodyid, m.nlight, m.nbody, None, "light_bodyid", "0";
        light_targetbodyid, m.nlight, m.nbody, None, "light_targetbodyid", "0";
        mesh_vertadr, m.nmesh, m.nmeshvert, Some(&m.mesh_vertnum), "mesh_vertadr", "m->mesh_vertnum";
        mesh_normaladr, m.nmesh, m.nmeshnormal, Some(&m.mesh_normalnum), "mesh_normaladr", "m->mesh_normalnum";
        mesh_texcoordadr, m.nmesh, m.nmeshtexcoord, Some(&m.mesh_texcoordnum), "mesh_texcoordadr", "m->mesh_texcoordnum";
        mesh_faceadr, m.nmesh, m.nmeshface, Some(&m.mesh_facenum), "mesh_faceadr", "m->mesh_facenum";
        mesh_bvhadr, m.nmesh, m.nbvh, Some(&m.mesh_bvhnum), "mesh_bvhadr", "m->mesh_bvhnum";
        mesh_graphadr, m.nmesh, m.nmeshgraph, None, "mesh_graphadr", "0";
        mesh_polyadr, m.nmesh, m.nmeshpoly, Some(&m.mesh_polynum), "mesh_polyadr", "m->mesh_polynum";
        mesh_polyvertadr, m.nmeshpoly, m.nmeshpolyvert, Some(&m.mesh_polyvertnum), "mesh_polyvertadr", "m->mesh_polyvertnum";
        mesh_polymapadr, m.nmeshvert, m.nmeshpolymap, Some(&m.mesh_polymapnum), "mesh_polymapadr", "m->mesh_polymapnum";
        flex_vertadr, m.nflex, m.nflexvert, Some(&m.flex_vertnum), "flex_vertadr", "m->flex_vertnum";
        flex_edgeadr, m.nflex, m.nflexedge, Some(&m.flex_edgenum), "flex_edgeadr", "m->flex_edgenum";
        flex_elemadr, m.nflex, m.nflexelem, Some(&m.flex_elemnum), "flex_elemadr", "m->flex_elemnum";
        flex_matid, m.nflexelem, m.nmat, None, "flex_matid", "0";
        flex_texcoordadr, m.nflex, m.nflextexcoord, None, "flex_texcoordadr", "0";
        flex_elemdataadr, m.nflex, m.nflexelemdata, None, "flex_elemdataadr", "0";
        flex_elemedgeadr, m.nflex, m.nflexelemedge, None, "flex_elemedgeadr", "0";
        flex_shelldataadr, m.nflex, m.nflexshelldata, None, "flex_shelldataadr", "0";
        flex_edge, m.nflexedge*2, m.nflexvert, None, "flex_edge", "0";
        flex_elem, m.nflexelemdata, m.nflexvert, None, "flex_elem", "0";
        flex_elemedge, m.nflexelemedge, m.nflexedge, None, "flex_elemedge", "0";
        flex_shell, m.nflexshelldata, m.nflexvert, None, "flex_shell", "0";
        flex_bvhadr, m.nflex, m.nbvh, Some(&m.flex_bvhnum), "flex_bvhadr", "m->flex_bvhnum";
        skin_matid, m.nskin, m.nmat, None, "skin_matid", "0";
        skin_vertadr, m.nskin, m.nskinvert, Some(&m.skin_vertnum), "skin_vertadr", "m->skin_vertnum";
        skin_texcoordadr, m.nskin, m.nskintexvert, None, "skin_texcoordadr", "0";
        skin_faceadr, m.nskin, m.nskinface, Some(&m.skin_facenum), "skin_faceadr", "m->skin_facenum";
        skin_boneadr, m.nskin, m.nskinbone, Some(&m.skin_bonenum), "skin_boneadr", "m->skin_bonenum";
        skin_bonevertadr, m.nskinbone, m.nskinbonevert, Some(&m.skin_bonevertnum), "skin_bonevertadr", "m->skin_bonevertnum";
        skin_bonebodyid, m.nskinbone, m.nbody, None, "skin_bonebodyid", "0";
        skin_bonevertid, m.nskinbonevert, m.nskinvert, None, "skin_bonevertid", "0";
        pair_geom1, m.npair, m.ngeom, None, "pair_geom1", "0";
        pair_geom2, m.npair, m.ngeom, None, "pair_geom2", "0";
        actuator_plugin, m.nactuator, m.nplugin, None, "actuator_plugin", "0";
        actuator_actadr, m.nactuator, m.na, Some(&m.actuator_actnum), "actuator_actadr", "m->actuator_actnum";
        actuator_ctrladr, m.nactuator, m.nu, Some(&m.actuator_ctrlnum), "actuator_ctrladr", "m->actuator_ctrlnum";
        actuator_outadr, m.nactuator, m.nout, Some(&m.actuator_outnum), "actuator_outadr", "m->actuator_outnum";
        sensor_plugin, m.nsensor, m.nplugin, None, "sensor_plugin", "0";
        plugin_stateadr, m.nplugin, m.npluginstate, Some(&m.plugin_statenum), "plugin_stateadr", "m->plugin_statenum";
        plugin_attradr, m.nplugin, m.npluginattr, None, "plugin_attradr", "0";
        tendon_adr, m.ntendon, m.nwrap, Some(&m.tendon_num), "tendon_adr", "m->tendon_num";
        tendon_matid, m.ntendon, m.nmat, None, "tendon_matid", "0";
        tendon_treeid, m.ntendon*2, m.ntree, None, "tendon_treeid", "0";
        numeric_adr, m.nnumeric, m.nnumericdata, Some(&m.numeric_size), "numeric_adr", "m->numeric_size";
        text_adr, m.ntext, m.ntextdata, Some(&m.text_size), "text_adr", "m->text_size";
        tuple_adr, m.ntuple, m.ntupledata, Some(&m.tuple_size), "tuple_adr", "m->tuple_size";
        name_bodyadr, m.nbody, m.nnames, None, "name_bodyadr", "0";
        name_jntadr, m.njnt, m.nnames, None, "name_jntadr", "0";
        name_geomadr, m.ngeom, m.nnames, None, "name_geomadr", "0";
        name_siteadr, m.nsite, m.nnames, None, "name_siteadr", "0";
        name_camadr, m.ncam, m.nnames, None, "name_camadr", "0";
        name_lightadr, m.nlight, m.nnames, None, "name_lightadr", "0";
        name_meshadr, m.nmesh, m.nnames, None, "name_meshadr", "0";
        name_skinadr, m.nskin, m.nnames, None, "name_skinadr", "0";
        name_hfieldadr, m.nhfield, m.nnames, None, "name_hfieldadr", "0";
        name_texadr, m.ntex, m.nnames, None, "name_texadr", "0";
        name_matadr, m.nmat, m.nnames, None, "name_matadr", "0";
        name_pairadr, m.npair, m.nnames, None, "name_pairadr", "0";
        name_excludeadr, m.nexclude, m.nnames, None, "name_excludeadr", "0";
        name_eqadr, m.neq, m.nnames, None, "name_eqadr", "0";
        name_tendonadr, m.ntendon, m.nnames, None, "name_tendonadr", "0";
        name_actuatoradr, m.nactuator, m.nnames, None, "name_actuatoradr", "0";
        name_sensoradr, m.nsensor, m.nnames, None, "name_sensoradr", "0";
        name_numericadr, m.nnumeric, m.nnames, None, "name_numericadr", "0";
        name_textadr, m.ntext, m.nnames, None, "name_textadr", "0";
        name_tupleadr, m.ntuple, m.nnames, None, "name_tupleadr", "0";
        name_keyadr, m.nkey, m.nnames, None, "name_keyadr", "0";
        hfield_pathadr, m.nhfield, m.npaths, None, "hfield_pathadr", "0";
        mesh_pathadr, m.nmesh, m.npaths, None, "mesh_pathadr", "0";
        skin_pathadr, m.nskin, m.npaths, None, "skin_pathadr", "0";
        tex_pathadr, m.ntex, m.npaths, None, "tex_pathadr", "0";
        tex_contenttypeadr, m.ntex, m.npaths, None, "tex_contenttypeadr", "0";
    }
    macro_rules! sparse {
        ($name:literal, $nr:expr, $nc:expr, $nnz:expr, $rows:ident, $adr:ident, $col:ident, $strict:expr) => {
            match validate_sparse($nr, $nc, $nnz, &m.$rows, &m.$adr, &m.$col, $strict) {
                1 => return Err(concat!("Invalid model: ", $name, "_rownnz out of bounds.").into()),
                2 => return Err(concat!("Invalid model: ", $name, "_rowadr out of bounds.").into()),
                3 => return Err(concat!("Invalid model: ", $name, "_colind out of bounds.").into()),
                _ => {}
            }
        };
    }
    sparse!("M", m.nv, m.nv, m.n_c, m_rownnz, m_rowadr, m_colind, true);
    sparse!("D", m.nv, m.nv, m.n_d, d_rownnz, d_rowadr, d_colind, true);
    sparse!("B", m.nbody, m.nv, m.n_b, b_rownnz, b_rowadr, b_colind, true);
    sparse!(
        "ten_J",
        m.ntendon,
        m.nv,
        m.n_jten,
        ten_j_rownnz,
        ten_j_rowadr,
        ten_j_colind,
        false
    );
    sparse!(
        "flexedge_J",
        m.nflexedge,
        m.nv,
        m.n_jfe,
        flexedge_j_rownnz,
        flexedge_j_rowadr,
        flexedge_j_colind,
        true
    );
    sparse!(
        "efm0_L",
        m.nefm0dof,
        m.nefm0dof,
        m.nefm0l,
        efm0_l_rownnz,
        efm0_l_rowadr,
        efm0_l_colind,
        true
    );
    for i in 0..m.nbody {
        if (i > 0 && m.body_parentid[i] >= i as i32) || m.body_parentid[i] < 0 {
            return Err("Invalid model: bad body_parentid.".into());
        }
        if m.body_rootid[i] > i as i32 || m.body_rootid[i] < 0 {
            return Err("Invalid model: bad body_rootid.".into());
        }
        if m.body_weldid[i] > i as i32 || m.body_weldid[i] < 0 {
            return Err("Invalid model: bad body_weldid.".into());
        }
        if i == 0 {
            if m.b_rownnz[0] != m.nv as i32 || (m.nv > 0 && m.b_rowadr[0] != 0) {
                return Err("Invalid model: B_rownnz out of bounds.".into());
            }
        } else {
            let np = m.body_parentid[i] as usize;
            let mut c = 0;
            let mut cp = 0;
            while c < m.b_rownnz[i] && cp < m.b_rownnz[np] {
                let col = m.b_colind[(m.b_rowadr[i] + c) as usize];
                let colp = m.b_colind[(m.b_rowadr[np] + cp) as usize];
                if col == colp {
                    c += 1;
                    cp += 1;
                } else if col > colp {
                    cp += 1;
                } else {
                    break;
                }
            }
            if c < m.b_rownnz[i] {
                return Err("Invalid model: B_colind out of bounds.".into());
            }
            let mut ndof = 0_i64;
            let mut np_weld = m.body_weldid[np];
            while np_weld > 0 {
                ndof += m.body_dofnum[np_weld as usize] as i64;
                np_weld = m.body_weldid[m.body_parentid[np_weld as usize] as usize];
            }
            if ndof > m.b_rownnz[i] as i64 || ndof > m.b_rownnz[np] as i64 {
                return Err("Invalid model: B_rownnz out of bounds.".into());
            }
        }
    }
    for i in 0..m.njnt {
        if m.jnt_type[i] >= 4 || m.jnt_type[i] < 0 {
            return Err("Invalid model: jnt_type out of bounds.".into());
        }
        let jnt_qposadr = m.jnt_qposadr[i] as i64 + N_POS[m.jnt_type[i] as usize] as i64;
        if jnt_qposadr > m.nq as i64 || m.jnt_qposadr[i] < 0 {
            return Err("Invalid model: jnt_qposadr out of bounds.".into());
        }
        let jnt_dofadr = m.jnt_dofadr[i] as i64 + N_VEL[m.jnt_type[i] as usize] as i64;
        if jnt_dofadr > m.nv as i64 || m.jnt_dofadr[i] < 0 {
            return Err("Invalid model: jnt_dofadr out of bounds.".into());
        }
        if m.jnt_plasticadr[i] < -1
            || (m.jnt_plasticadr[i] >= 0
                && m.jnt_plasticadr[i] as usize + N_POS[m.jnt_type[i] as usize] > m.nplastic)
        {
            return Err("Invalid model: jnt_plasticadr out of bounds.".into());
        }
    }
    for i in 0..m.ntendon {
        if m.tendon_plasticadr[i] < -1
            || (m.tendon_plasticadr[i] >= 0 && m.tendon_plasticadr[i] as usize + 2 > m.nplastic)
        {
            return Err("Invalid model: tendon_plasticadr out of bounds.".into());
        }
    }
    let mut n_m_post = 0;
    for i in 0..m.nv {
        if m.dof_parentid[i] >= i as i32 {
            return Err("Invalid model: bad dof_parentid.".into());
        }
        let bodyid = m.dof_bodyid[i];
        if bodyid < 0 || bodyid as usize >= m.nbody {
            return Err("Invalid model: dof_bodyid out of bounds.".into());
        }
        if m.dof_madr[i] as usize != n_m_post {
            return Err("Invalid model: dof_Madr out of bounds.".into());
        }
        let mut nanc = 0;
        let mut j = i as i32;
        while j >= 0 {
            nanc += 1;
            j = m.dof_parentid[j as usize];
        }
        n_m_post += nanc as usize;
        let expected_nnz = if m.dof_simplenum[i] != 0 { 1 } else { nanc };
        if m.m_rownnz[i] != expected_nnz {
            return Err("Invalid model: M_rownnz out of bounds.".into());
        }
        let adr = m.m_rowadr[i] as usize;
        let mut k = expected_nnz - 1;
        let mut j = i as i32;
        while k >= 0 {
            if m.m_colind[adr + k as usize] != j {
                return Err("Invalid model: M_colind out of bounds.".into());
            }
            j = m.dof_parentid[j as usize];
            k -= 1;
        }
        if m.d_diag[i] != nanc - 1
            || m.d_diag[i] >= m.d_rownnz[i]
            || m.d_colind[(m.d_rowadr[i] + m.d_diag[i]) as usize] != i as i32
        {
            return Err("Invalid model: D_diag out of bounds.".into());
        }
        if m.d_rownnz[i] != m.b_rownnz[bodyid as usize] {
            return Err("Invalid model: D_rownnz out of bounds.".into());
        }
        for c in 0..m.d_rownnz[i] {
            if m.d_colind[(m.d_rowadr[i] + c) as usize]
                != m.b_colind[(m.b_rowadr[bodyid as usize] + c) as usize]
            {
                return Err("Invalid model: D_colind out of bounds.".into());
            }
        }
    }
    if n_m_post != m.n_m {
        return Err("Invalid model: dof_Madr out of bounds.".into());
    }
    for i in 0..m.n_c {
        if m.map_m2m[i] < 0 || m.map_m2m[i] as usize >= m.n_m {
            return Err("Invalid model: mapM2M out of bounds.".into());
        }
        if m.map_d2m[i] < 0 || m.map_d2m[i] as usize >= m.n_d {
            return Err("Invalid model: mapD2M out of bounds.".into());
        }
    }
    for i in 0..m.n_d {
        if m.map_m2d[i] < -1 || m.map_m2d[i] as i64 >= m.n_c as i64 {
            return Err("Invalid model: mapM2D out of bounds.".into());
        }
    }
    for i in 0..m.nefm0dof {
        if m.efm0_dofid[i] < 0 || m.efm0_dofid[i] as usize >= m.nv {
            return Err("Invalid model: efm0_dofid out of bounds.".into());
        }
    }
    for i in 0..m.nefm_cvert {
        if m.efm_c_perm[i] < -1 || m.efm_c_perm[i] as i64 >= m.nv as i64 {
            return Err("Invalid model: efmC_perm out of bounds.".into());
        }
    }
    for f in 0..m.nflex {
        let ncell = if m.flex_interp[f] != 0 {
            m.flex_cellnum[3 * f] * m.flex_cellnum[3 * f + 1] * m.flex_cellnum[3 * f + 2]
        } else {
            0
        };
        for v in 0..m.flex_vertnum[f] {
            let c = m.flex_vertcell[(m.flex_vertadr[f] + v) as usize];
            if if m.flex_interp[f] != 0 {
                c < 0 || c >= ncell
            } else {
                c != -1
            } {
                return Err("Invalid model: flex_vertcell out of bounds.".into());
            }
        }
        let dim = m.flex_dim[f];
        let mut nplastic = 0;
        if m.flex_interp[f] > 0 {
            nplastic = 6 * ncell;
        } else if m.flex_interp[f] < 0 {
            let cx = m.flex_cellnum[3 * f];
            let cy = m.flex_cellnum[3 * f + 1];
            let cz = m.flex_cellnum[3 * f + 2];
            nplastic = 3 * 2 * (cy * cz + cx * cz + cx * cy);
        } else if dim == 1 {
            if m.flex_plasticadr[f] >= 0 {
                nplastic = crate::core_util::rod_state_size(m, f);
            }
        } else {
            nplastic = if m.flex_plasticmode[f] & 2 != 0 {
                (if dim == 2 { 3 } else { 6 }) * m.flex_elemnum[f]
            } else {
                0
            };
            if m.flex_plasticmode[f] & 1 != 0 {
                nplastic += m.flex_edgenum[f];
            }
        }
        let mode = m.flex_plasticmode[f];
        if mode < 0
            || mode > 3
            || (mode != 0 && m.flex_plasticadr[f] < 0)
            || ((mode & 2 != 0) && dim < 2)
            || ((mode & 1 != 0) && (dim != 2 || m.flex_interp[f] != 0))
        {
            return Err("Invalid model: flex_plasticmode out of bounds.".into());
        }
        if m.flex_plasticadr[f] < -1
            || (m.flex_plasticadr[f] >= 0
                && m.flex_plasticadr[f] as i64 + nplastic as i64 > m.nplastic as i64)
        {
            return Err("Invalid model: flex_plasticadr out of bounds.".into());
        }
    }
    for i in 0..m.ngeom {
        if m.geom_condim[i] > 6 || m.geom_condim[i] < 0 {
            return Err("Invalid model: geom_condim out of bounds.".into());
        }
        if m.geom_type[i] == GEOM_HFIELD {
            if m.geom_dataid[i] as i64 >= m.nhfield as i64 || m.geom_dataid[i] < -1 {
                return Err("Invalid model: geom_dataid out of bounds.".into());
            }
        } else if m.geom_type[i] == GEOM_MESH || m.geom_type[i] == GEOM_SDF {
            if m.geom_dataid[i] as i64 >= m.nmesh as i64 || m.geom_dataid[i] < -1 {
                return Err("Invalid model: geom_dataid out of bounds.".into());
            }
        }
    }
    for i in 0..m.nsite {
        if m.site_type[i] == GEOM_MESH {
            if m.site_dataid[i] as i64 >= m.nmesh as i64 || m.site_dataid[i] < -1 {
                return Err("Invalid model: site_dataid out of bounds.".into());
            }
        }
    }
    for i in 0..m.nhfield {
        let hfield_adr = m.hfield_adr[i] as i64 + m.hfield_nrow[i] as i64 * m.hfield_ncol[i] as i64;
        if hfield_adr > m.nhfielddata as i64 || m.hfield_adr[i] < 0 {
            return Err("Invalid model: hfield_adr out of bounds.".into());
        }
    }
    for i in 0..m.ntex {
        if m.tex_adr[i] == -1 {
            if m.tex_pathadr[i] < 0 {
                return Err("Invalid model: by-reference texture has no path.".into());
            }
            continue;
        }
        let nbytes = m.tex_nchannel[i] as i64 * m.tex_height[i] as i64 * m.tex_width[i] as i64;
        if m.tex_adr[i] + nbytes > m.ntexdata as i64 || m.tex_adr[i] < 0 {
            return Err("Invalid model: tex_adr out of bounds.".into());
        }
    }
    for i in 0..m.nmesh {
        let vertnum = m.mesh_vertnum[i];
        let polynum = m.mesh_polynum[i];
        let normalnum = m.mesh_normalnum[i];
        let texcoordnum = m.mesh_texcoordnum[i];
        let has_texcoord = m.mesh_texcoordadr[i] >= 0;
        let faceadr = m.mesh_faceadr[i];
        for f in 0..m.mesh_facenum[i] {
            for k in 0..3 {
                let j = (3 * (faceadr + f) + k) as usize;
                let v = m.mesh_face[j];
                if v < 0 || v >= vertnum {
                    return Err("Invalid model: mesh_face vertex index out of bounds.".into());
                }
                let n = m.mesh_facenormal[j];
                if n < 0 || n >= normalnum {
                    return Err("Invalid model: mesh_facenormal normal index out of bounds.".into());
                }
                if has_texcoord {
                    let t = m.mesh_facetexcoord[j];
                    if t < 0 || t >= texcoordnum {
                        return Err(
                            "Invalid model: mesh_facetexcoord texcoord index out of bounds.".into(),
                        );
                    }
                }
            }
        }
        let polyadr = m.mesh_polyadr[i];
        for p in 0..polynum {
            let pvadr = m.mesh_polyvertadr[(polyadr + p) as usize];
            for j in 0..m.mesh_polyvertnum[(polyadr + p) as usize] {
                let v = m.mesh_polyvert[(pvadr + j) as usize];
                if v < 0 || v >= vertnum {
                    return Err("Invalid model: mesh_polyvert vertex index out of bounds.".into());
                }
            }
        }
        let vertadr = m.mesh_vertadr[i];
        for v in 0..vertnum {
            let pmadr = m.mesh_polymapadr[(vertadr + v) as usize];
            for j in 0..m.mesh_polymapnum[(vertadr + v) as usize] {
                let p = m.mesh_polymap[(pmadr + j) as usize];
                if p < 0 || p >= polynum {
                    return Err("Invalid model: mesh_polymap polygon index out of bounds.".into());
                }
            }
        }
    }
    for i in 0..m.npair {
        let pair_body1 = m.pair_signature[i] & 0xFFFF;
        if pair_body1 as i64 >= m.nbody as i64 || pair_body1 < 0 {
            return Err("Invalid model: pair_body1 out of bounds.".into());
        }
        let pair_body2 = ((m.pair_signature[i] as u32) >> 16) as i32;
        if pair_body2 as i64 >= m.nbody as i64 || pair_body2 < 0 {
            return Err("Invalid model: pair_body2 out of bounds.".into());
        }
    }
    for i in 0..m.neq {
        let obj1id = m.eq_obj1id[i];
        let obj2id = m.eq_obj2id[i];
        let objtype = m.eq_objtype[i];
        match m.eq_type[i] {
            EQ_JOINT => {
                if obj1id as i64 >= m.njnt as i64 || obj1id < 0 {
                    return Err("Invalid model: eq_obj1id out of bounds.".into());
                }
                if obj2id as i64 >= m.njnt as i64 || obj2id < -1 {
                    return Err("Invalid model: eq_obj2id out of bounds.".into());
                }
            }
            EQ_TENDON => {
                if obj1id as i64 >= m.ntendon as i64 || obj1id < 0 {
                    return Err("Invalid model: eq_obj1id out of bounds.".into());
                }
                if obj2id as i64 >= m.ntendon as i64 || obj2id < -1 {
                    return Err("Invalid model: eq_obj2id out of bounds.".into());
                }
            }
            EQ_WELD | EQ_CONNECT => {
                if objtype == OBJ_BODY {
                    if obj1id as i64 >= m.nbody as i64 || obj1id < 0 {
                        return Err("Invalid model: eq_obj1id out of bounds.".into());
                    }
                    if obj2id as i64 >= m.nbody as i64 || obj2id < 0 {
                        return Err("Invalid model: eq_obj2id out of bounds.".into());
                    }
                } else if objtype == OBJ_SITE {
                    if obj1id as i64 >= m.nsite as i64 || obj1id < 0 {
                        return Err("Invalid model: eq_obj1id out of bounds.".into());
                    }
                    if obj2id as i64 >= m.nsite as i64 || obj2id < 0 {
                        return Err("Invalid model: eq_obj2id out of bounds.".into());
                    }
                } else {
                    return Err("Invalid model: eq_objtype is not body or site.".into());
                }
            }
            EQ_FLEX | EQ_FLEXSTRAIN => {
                if obj1id as i64 >= m.nflex as i64 || obj1id < 0 {
                    return Err("Invalid model: eq_obj1id out of bounds.".into());
                }
                if obj2id != -1 {
                    return Err("Invalid model: eq_obj2id must be -1.".into());
                }
            }
            _ => error("unknown equality constraint type."),
        }
    }
    for i in 0..m.nwrap {
        let wrap_objid = m.wrap_objid[i];
        match m.wrap_type[i] {
            WRAP_NONE | WRAP_PULLEY => {}
            WRAP_JOINT => {
                if wrap_objid as i64 >= m.njnt as i64 || wrap_objid < 0 {
                    return Err("Invalid model: wrap_objid out of bounds.".into());
                }
            }
            WRAP_SITE => {
                if wrap_objid as i64 >= m.nsite as i64 || wrap_objid < 0 {
                    return Err("Invalid model: wrap_objid out of bounds.".into());
                }
            }
            WRAP_SPHERE | WRAP_CYLINDER => {
                if wrap_objid as i64 >= m.ngeom as i64 || wrap_objid < 0 {
                    return Err("Invalid model: wrap_objid out of bounds.".into());
                }
            }
            _ => {}
        }
    }
    for i in 0..m.nactuator {
        let actuator_trntype = m.actuator_trntype[i];
        let id = m.actuator_trnid[2 * i];
        let idslider = m.actuator_trnid[2 * i + 1];
        match actuator_trntype {
            TRN_JOINT | TRN_JOINTINPARENT => {
                if id < 0 || id as usize >= m.njnt {
                    return Err("Invalid model: actuator_trnid out of bounds.".into());
                }
            }
            TRN_TENDON => {
                if id < 0 || id as usize >= m.ntendon {
                    return Err("Invalid model: actuator_trnid out of bounds.".into());
                }
            }
            TRN_SITE => {
                if id < 0 || id as usize >= m.nsite {
                    return Err("Invalid model: actuator_trnid out of bounds.".into());
                }
            }
            TRN_SLIDERCRANK => {
                if id < 0 || id as usize >= m.nsite {
                    return Err("Invalid model: actuator_trnid out of bounds.".into());
                }
                if idslider < 0 || idslider as usize >= m.nsite {
                    return Err("Invalid model: actuator_trnid out of bounds.".into());
                }
            }
            TRN_BODY => {
                if id < 0 || id as usize >= m.nbody {
                    return Err("Invalid model: actuator_trnid out of bounds.".into());
                }
            }
            TRN_SO3 => {
                if idslider == -1 {
                    if id < 0 || id as usize >= m.njnt {
                        return Err("Invalid model: actuator_trnid out of bounds.".into());
                    }
                } else {
                    if id < 0
                        || id as usize >= m.nsite
                        || idslider < 0
                        || idslider as usize >= m.nsite
                    {
                        return Err("Invalid model: actuator_trnid out of bounds.".into());
                    }
                }
            }
            TRN_UNDEFINED => {}
            _ => {}
        }
    }
    for i in 0..m.nsensor {
        let sensor_type = m.sensor_type[i];
        // Plugin callbacks are deferred: the serialized sensor dimension is used
        // until mjpPlugin::nsensordata is available.
        let size = if sensor_type == SENS_PLUGIN {
            m.sensor_dim[i]
        } else {
            sensor_size(sensor_type, m.sensor_dim[i])
        };
        if size < 0 {
            return Err("Invalid model: Bad sensor_type.".into());
        }
        let sensor_adr = m.sensor_adr[i];
        if sensor_adr < 0 || sensor_adr as i64 + size as i64 > m.nsensordata as i64 {
            return Err("Invalid model: sensor_adr out of bounds.".into());
        }
        let mut nobj = num_objects(m, m.sensor_objtype[i]);
        if nobj == -2 {
            return Err("Invalid model: invalid sensor_objtype".into());
        }
        if nobj != -1 && (m.sensor_objid[i] < 0 || m.sensor_objid[i] >= nobj) {
            return Err("Invalid model: invalid sensor_objid".into());
        }
        nobj = num_objects(m, m.sensor_reftype[i]);
        if nobj == -2 {
            return Err("Invalid model: invalid sensor_reftype".into());
        }
        if nobj != -1 && (m.sensor_refid[i] < -1 || m.sensor_refid[i] >= nobj) {
            return Err("Invalid model: invalid sensor_refid".into());
        }
        if sensor_type == SENS_TACTILE {
            let parent_body = m.geom_bodyid[m.sensor_refid[i] as usize] as usize;
            let mut collision_geoms = 0;
            for b in 0..m.body_geomnum[parent_body] {
                let geom_id = (m.body_geomadr[parent_body] + b) as usize;
                if m.geom_contype[geom_id] != 0 || m.geom_conaffinity[geom_id] != 0 {
                    collision_geoms += 1;
                }
            }
            if collision_geoms == 0 {
                return Err("Touch sensor requires a body with at least one collision geom".into());
            }
        }
    }
    for i in 0..m.nexclude {
        let exclude_body1 = m.exclude_signature[i] & 0xFFFF;
        if exclude_body1 as i64 >= m.nbody as i64 || exclude_body1 < 0 {
            return Err("Invalid model: exclude_body1 out of bounds.".into());
        }
        let exclude_body2 = ((m.exclude_signature[i] as u32) >> 16) as i32;
        if exclude_body2 as i64 >= m.nbody as i64 || exclude_body2 < 0 {
            return Err("Invalid model: exclude_body2 out of bounds.".into());
        }
    }
    for i in 0..m.ntuple {
        for j in 0..m.tuple_size[i] {
            let adr = (m.tuple_adr[i] + j) as usize;
            let nobj = num_objects(m, m.tuple_objtype[adr]);
            if nobj == -2 {
                return Err("Invalid model: invalid tuple_objtype".into());
            }
            if nobj != -1 && (m.tuple_objid[adr] < 0 || m.tuple_objid[adr] >= nobj) {
                return Err("Invalid model: invalid tuple_objid".into());
            }
        }
    }
    Ok(())
}
