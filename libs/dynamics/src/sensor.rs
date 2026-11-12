//! Position, velocity, acceleration, contact and tactile sensors, and energy.
//!
//! Translated from `src/engine/engine_sensor.c` and `src/engine/engine_sensor.h`
//! of the engine named in CREDITS.md (Apache-2.0,
//! Copyright 2021 DeepMind Technologies Limited); changed: translated to Rust.

use crate::collision_convex::{hausdorff, init_ccd_obj, init_ccd_obj_site};
use crate::collision_gjk::CcdObj;
use crate::collision_sdf::{distance, get_sdf, SdfQuery};
use crate::core_smooth::{rne_post_constraint, subtree_vel};
use crate::core_util::{contact_force, object_acceleration, object_velocity};
use crate::data::Data;
use crate::inline::{at3, at3_mut, at4, at4_mut, at6, at6_mut, at9, at9_mut};
use crate::model::Model;
use crate::ray;
use crate::sleep::sleep_state;
use crate::sort::partial_sort;
use crate::support::{cam_intrinsics, condata_size, geom_distance, inside_site, mul_m, CONDATA_SIZE};
use crate::thread::{dispatch, num_thread};
use crate::types::*;
use crate::util_blas::*;
use crate::util_misc::{cam_pixel_ray, clip, is_zero, max, min, poly_potential};
use crate::util_spatial::{cross, quat2mat, sub_quat, transform_spatial};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ContactInfo {
    criterion: f64,
    id: usize,
    flip: i32,
}

fn contact_info_compare(a: &ContactInfo, b: &ContactInfo) -> i32 {
    if a.criterion < b.criterion { return -1; }
    if a.criterion > b.criterion { return 1; }
    if a.id < b.id { return -1; }
    if a.id > b.id { return 1; }
    0
}
// ContactSelect is the engine_sort.h partial_sort specialization below.

struct TactileTaskArgs<'a> {
    // C stores sensor_id and nchannel but never reads them.
    mesh_id: usize,
    geom_id: usize,
    parent_weld: usize,
    ncontact: usize,
    contact_geom_ids: &'a [i32],
    start_taxel: usize,
    end_taxel: usize,
    sensordata: &'a mut [f64],
}

fn tactile_taxel_batch(m: &Model, d: &mut Data, t: &mut TactileTaskArgs<'_>) {
    let mesh_id = t.mesh_id;
    let geom_id = t.geom_id;
    let parent_weld = t.parent_weld;
    let ncon = m.mesh_vertnum[mesh_id] as usize;
    let geom_pos = at3(&d.geom_xpos, 3 * geom_id);
    let geom_mat = at9(&d.geom_xmat, 9 * geom_id);
    let mesh_vert = &m.mesh_vert[3 * m.mesh_vertadr[mesh_id] as usize..];
    let mesh_normal = &m.mesh_normal[3 * m.mesh_normaladr[mesh_id] as usize..];
    let has_frame = m.mesh_normalnum[mesh_id] == 3 * m.mesh_vertnum[mesh_id];
    let normal_stride = if has_frame { 9 } else { 3 };
    for j in t.start_taxel..t.end_taxel {
        let pos = [mesh_vert[3*j] as f64, mesh_vert[3*j+1] as f64, mesh_vert[3*j+2] as f64];
        let mut xpos = [0.0; 3];
        mul_mat_vec3(&mut xpos, geom_mat, &pos);
        add_to3(&mut xpos, geom_pos);
        for g in 0..t.ncontact {
            let geom = t.contact_geom_ids[g] as usize;
            if geom == geom_id { continue; }
            let body = m.geom_bodyid[geom] as usize;
            let mut sdf_instance = [-1, -1];
            let mut geomtype = [GEOM_SDF, GEOM_SPHERE];
            let mut sdf_ptr = [None, None];
            if m.geom_type[geom] == GEOM_SDF {
                sdf_instance[0] = m.geom_plugin[geom];
                sdf_ptr[0] = get_sdf(m, geom as i32);
            } else if m.geom_type[geom] == GEOM_MESH {
                sdf_instance[0] = m.geom_dataid[geom];
                geomtype[0] = m.geom_type[geom];
            } else {
                sdf_instance[0] = geom as i32;
                geomtype[0] = m.geom_type[geom];
            }
            if geomtype[0] == GEOM_MESH && m.mesh_octadr[m.geom_dataid[geom] as usize] == -1 {
                continue;
            }
            let geom_sdf = SdfQuery {
                id: &sdf_instance[..1], r#type: SDFTYPE_SINGLE,
                plugin: &sdf_ptr[..1], geomtype: &geomtype[..1],
                // Unused in C for a SINGLE query.
                relpos: &[0.0; 3], relmat: &[0.0; 9],
            };
            let mut tmp = [0.0; 3];
            let mut lpos = [0.0; 3];
            sub3(&mut tmp, &xpos, at3(&d.geom_xpos, 3*geom));
            mul_mat_tvec3(&mut lpos, at9(&d.geom_xmat, 9*geom), &tmp);
            if sdf_ptr[0].is_some() {
                let mut mesh_mat = [0.0; 9];
                quat2mat(&mut mesh_mat, at4(&m.mesh_quat, 4*m.geom_dataid[geom] as usize));
                let input = lpos;
                mul_mat_vec3(&mut lpos, &mesh_mat, &input);
                add_to3(&mut lpos, at3(&m.mesh_pos, 3*m.geom_dataid[geom] as usize));
            }
            let depth = min(distance(m, d, &geom_sdf, &lpos), 0.0);
            if depth == 0.0 { continue; }
            let mut vel_sensor = [0.0; 6];
            let mut vel_other = [0.0; 6];
            let mut vel_rel = [0.0; 3];
            transform_spatial(&mut vel_sensor, at6(&d.cvel, 6*parent_weld), 0, &xpos,
                at3(&d.subtree_com, 3*m.body_rootid[parent_weld] as usize), None);
            transform_spatial(&mut vel_other, at6(&d.cvel, 6*body), 0, &xpos,
                at3(&d.subtree_com, 3*m.body_rootid[body] as usize), None);
            sub3(&mut vel_rel, at3(&vel_sensor, 3), at3(&vel_other, 3));
            t.sensordata[j] = max(t.sensordata[j], -depth);
            if has_frame {
                let mut vel_rel_geom = [0.0; 3];
                mul_mat_tvec3(&mut vel_rel_geom, geom_mat, &vel_rel);
                let tang1 = [mesh_normal[normal_stride*j+3] as f64,
                    mesh_normal[normal_stride*j+4] as f64, mesh_normal[normal_stride*j+5] as f64];
                let tang2 = [mesh_normal[normal_stride*j+6] as f64,
                    mesh_normal[normal_stride*j+7] as f64, mesh_normal[normal_stride*j+8] as f64];
                t.sensordata[ncon+j] += dot3(&vel_rel_geom, &tang1).abs();
                t.sensordata[2*ncon+j] += dot3(&vel_rel_geom, &tang2).abs();
            }
        }
    }
}

fn tactile_task(m: &Model, d: &mut Data, args: &mut [TactileTaskArgs<'_>; 1], _thread_id: usize, task_id: usize) {
    tactile_taxel_batch(m, d, &mut args[task_id]);
}

// Public so the integration test can exercise C's static helper directly.
pub fn apply_cutoff(m: &Model, i: usize, data: &mut [f64]) {
    let cutoff = m.sensor_cutoff[i];
    if cutoff <= 0.0 { return; }
    let r#type = m.sensor_type[i];
    if r#type == SENS_CONTACT || r#type == SENS_GEOMFROMTO { return; }
    let dim = m.sensor_dim[i] as usize;
    for j in 0..dim {
        if m.sensor_datatype[i] == DATATYPE_REAL {
            data[j] = clip(data[j], -cutoff, cutoff);
        } else if m.sensor_datatype[i] == DATATYPE_POSITIVE {
            data[j] = min(cutoff, data[j]);
        }
    }
}

fn get_xpos_xmat<'a>(d: &'a Data, r#type: i32, id: usize, sensor_id: usize,
    xpos: &mut &'a [f64; 3], xmat: &mut &'a [f64; 9]) {
    match r#type {
        OBJ_XBODY => { *xpos = at3(&d.xpos, 3*id); *xmat = at9(&d.xmat, 9*id); }
        OBJ_BODY => { *xpos = at3(&d.xipos, 3*id); *xmat = at9(&d.ximat, 9*id); }
        OBJ_GEOM => { *xpos = at3(&d.geom_xpos, 3*id); *xmat = at9(&d.geom_xmat, 9*id); }
        OBJ_SITE => { *xpos = at3(&d.site_xpos, 3*id); *xmat = at9(&d.site_xmat, 9*id); }
        OBJ_CAMERA => { *xpos = at3(&d.cam_xpos, 3*id); *xmat = at9(&d.cam_xmat, 9*id); }
        _ => panic!("invalid object type in sensor {}", sensor_id),
    }
}

fn get_xquat<'a>(m: &Model, d: &'a Data, r#type: i32, id: usize,
    sensor_id: usize, cam_quat: &'a mut [f64; 4]) -> &'a [f64; 4] {
    match r#type {
        OBJ_XBODY => at4(&d.xquat, 4*id),
        OBJ_BODY => at4(&d.xiquat, 4*id),
        OBJ_GEOM => at4(&d.geom_xquat, 4*id),
        OBJ_SITE => at4(&d.site_xquat, 4*id),
        OBJ_CAMERA => {
            crate::inline::mul_quat(cam_quat, at4(&d.xquat, 4*m.cam_bodyid[id] as usize), at4(&m.cam_quat, 4*id));
            cam_quat
        }
        _ => panic!("invalid object type in sensor {}", sensor_id),
    }
}

fn cam_project(sensordata: &mut [f64], target_xpos: &[f64; 3], cam_xpos: &[f64; 3],
    cam_xmat: &[f64; 9], cam_res: &[i32], cam_fovy: f64, cam_intrinsic: &[f32], cam_sensorsize: &[f32]) {
    let (fx, fy);
    if cam_sensorsize[0] != 0.0 && cam_sensorsize[1] != 0.0 {
        // Both divisions and multiplications have float precision in C.
        fx = (cam_intrinsic[0] / cam_sensorsize[0] * cam_res[0] as f32) as f64;
        fy = (cam_intrinsic[1] / cam_sensorsize[1] * cam_res[1] as f32) as f64;
    } else {
        fy = 0.5 / (cam_fovy * PI / 360.0).tan() * cam_res[1] as f64;
        fx = fy;
    }
    let mut relative_pos = [0.0; 3];
    sub3(&mut relative_pos, target_xpos, cam_xpos);
    let mut cam_pos = [0.0; 3];
    mul_mat_tvec(&mut cam_pos, cam_xmat, &relative_pos, 3, 3);
    let mut denom = cam_pos[2];
    if denom.abs() < MINVAL {
        if denom < 0.0 { denom = min(denom, -MINVAL); }
        else { denom = max(denom, MINVAL); }
    }
    sensordata[0] = -fx * (cam_pos[0] / denom) + 0.5 * cam_res[0] as f64;
    sensordata[1] = fy * (cam_pos[1] / denom) + 0.5 * cam_res[1] as f64;
}

fn check_match(m: &Model, mut body: i32, geom: i32, r#type: i32, id: i32) -> bool {
    if r#type == OBJ_UNKNOWN { return true; }
    if r#type == OBJ_SITE { return true; }
    if r#type == OBJ_GEOM { return id == geom; }
    if r#type == OBJ_BODY { return id == body; }
    if r#type == OBJ_XBODY {
        while body > id { body = m.body_parentid[body as usize]; }
        return body == id;
    }
    false
}

fn match_contact(m: &Model, d: &Data, conid: usize, type1: i32, id1: i32, type2: i32, id2: i32) -> i32 {
    if type1 == OBJ_UNKNOWN && type2 == OBJ_UNKNOWN { return 1; }
    if type1 == OBJ_SITE && inside_site(m, d, id1 as usize, &d.contact[conid].pos) == 0 { return 0; }
    let geom1 = d.contact[conid].geom[0];
    let geom2 = d.contact[conid].geom[1];
    let body1 = if geom1 >= 0 { m.geom_bodyid[geom1 as usize] } else { crate::unported!("mj_flexBody") };
    let body2 = if geom2 >= 0 { m.geom_bodyid[geom2 as usize] } else { crate::unported!("mj_flexBody") };
    let match11 = check_match(m, body1, geom1, type1, id1);
    let match12 = check_match(m, body2, geom2, type1, id1);
    let match21 = check_match(m, body1, geom1, type2, id2);
    let match22 = check_match(m, body2, geom2, type2, id2);
    if !match11 && !match12 { return 0; }
    if !match21 && !match22 { return 0; }
    if type1 != OBJ_UNKNOWN && type2 != OBJ_UNKNOWN {
        let order_regular = match11 && match22;
        let order_reverse = match12 && match21;
        if order_regular && !order_reverse { return 1; }
        if order_reverse && !order_regular { return -1; }
        if order_regular && order_reverse { return 1; }
    } else if type1 != OBJ_UNKNOWN { return if match11 { 1 } else { -1 }; }
    else if type2 != OBJ_UNKNOWN { return if match22 { 1 } else { -1 }; }
    0
}

// C data pointers are offsets into the single output buffer.
fn copy_sensor_data(m: &Model, d: &mut Data, sensordata: &mut [f64],
    data: &[Option<usize>; NCONDATA as usize], id: usize, flg_flip: i32, nfound: usize) {
    if let Some(k) = data[CONDATA_FOUND as usize] { sensordata[k] = nfound as f64; }
    if data[CONDATA_FORCE as usize].is_some() || data[CONDATA_TORQUE as usize].is_some() {
        let mut forcetorque = [0.0; 6];
        contact_force(m, d, id as i32, &mut forcetorque);
        if let Some(k) = data[CONDATA_FORCE as usize] {
            copy3(at3_mut(sensordata, k), at3(&forcetorque, 0));
            if flg_flip != 0 { sensordata[k+2] *= -1.0; }
        }
        if let Some(k) = data[CONDATA_TORQUE as usize] {
            copy3(at3_mut(sensordata, k), at3(&forcetorque, 3));
            if flg_flip != 0 { sensordata[k+2] *= -1.0; }
        }
    }
    if let Some(k) = data[CONDATA_DIST as usize] { sensordata[k] = d.contact[id].dist; }
    if let Some(k) = data[CONDATA_POS as usize] { copy3(at3_mut(sensordata, k), &d.contact[id].pos); }
    if let Some(k) = data[CONDATA_NORMAL as usize] {
        copy3(at3_mut(sensordata, k), at3(&d.contact[id].frame, 0));
        if flg_flip != 0 { let input = *at3(sensordata, k); scl3(at3_mut(sensordata, k), &input, -1.0); }
    }
    if let Some(k) = data[CONDATA_TANGENT as usize] {
        copy3(at3_mut(sensordata, k), at3(&d.contact[id].frame, 3));
        if flg_flip != 0 { let input = *at3(sensordata, k); scl3(at3_mut(sensordata, k), &input, -1.0); }
    }
    if data[CONDATA_LINVEL as usize].is_some() || data[CONDATA_ANGVEL as usize].is_some() {
        let con = &d.contact[id];
        let mut linvel = [0.0; 3];
        let mut angvel = [0.0; 3];
        if con.geom[0] >= 0 && con.geom[1] >= 0 {
            let mut bid = [0; 2];
            let mut vel = [[0.0; 6]; 2];
            for s in 0..2 {
                bid[s] = m.geom_bodyid[con.geom[s] as usize] as usize;
                transform_spatial(&mut vel[s], at6(&d.cvel, 6*bid[s]), 0, &con.pos,
                    at3(&d.subtree_com, 3*m.body_rootid[bid[s]] as usize), None);
            }
            sub3(&mut angvel, at3(&vel[1], 0), at3(&vel[0], 0));
            sub3(&mut linvel, at3(&vel[1], 3), at3(&vel[0], 3));
        } else {
            // Flex vertex and interpolated paths, including their contact Jacobian.
            crate::unported!("mj_contactJacobian");
        }
        if let Some(k) = data[CONDATA_LINVEL as usize] {
            mul_mat_vec3(at3_mut(sensordata, k), &con.frame, &linvel);
            if flg_flip != 0 { sensordata[k+2] *= -1.0; }
        }
        if let Some(k) = data[CONDATA_ANGVEL as usize] {
            mul_mat_vec3(at3_mut(sensordata, k), &con.frame, &angvel);
            if flg_flip != 0 { sensordata[k+2] *= -1.0; }
        }
    }
}

fn total_wrench(force: &mut [f64; 3], torque: &mut [f64; 3], point: &[f64; 3], n: usize,
    wrench: &[f64], pos: &[f64], frame: &[f64]) {
    zero3(force);
    zero3(torque);
    for j in 0..n {
        let mut force_j = [0.0; 3];
        let mut torque_j = [0.0; 3];
        mul_mat_tvec3(&mut force_j, at9(frame, 9*j), at3(wrench, 6*j));
        mul_mat_tvec3(&mut torque_j, at9(frame, 9*j), at3(wrench, 6*j+3));
        add_to3(force, &force_j);
        add_to3(torque, &torque_j);
        let mut diff = [0.0; 3];
        sub3(&mut diff, at3(pos, 3*j), point);
        let mut induced_torque = [0.0; 3];
        cross(&mut induced_torque, &diff, &force_j);
        add_to3(torque, &induced_torque);
    }
}

fn compute_contact_sensor(m: &Model, d: &mut Data, i: usize, sensordata: &mut [f64]) {
    const REDUCE_MINDIST: i32 = 1;
    const REDUCE_MAXFORCE: i32 = 2;
    const REDUCE_NETFORCE: i32 = 3;
    let objtype = m.sensor_objtype[i];
    let objid = m.sensor_objid[i];
    let dataspec = m.sensor_intprm[i*NSENS];
    let size = condata_size(dataspec) as usize;
    let dim = m.sensor_dim[i] as usize;
    let num = dim / size;
    let reftype = m.sensor_reftype[i];
    let refid = m.sensor_refid[i];
    let reduce = m.sensor_intprm[i*NSENS+1];
    zero(sensordata, dim);
    let mut data = [None; NCONDATA as usize];
    let mut adr = 0;
    for j in 0..NCONDATA as usize {
        if dataspec & (1 << j) != 0 {
            data[j] = Some(adr);
            adr += CONDATA_SIZE[j] as usize;
        }
    }
    let mut nmatch = 0;
    let mut r#match = d.scratch.contact_info(d.ncon as usize);
    for j in 0..d.ncon as usize {
        let match_j = match_contact(m, d, j, objtype, objid, reftype, refid);
        if match_j == 0 { continue; }
        r#match[nmatch].id = j;
        r#match[nmatch].flip = i32::from(match_j < 0);
        if reduce == REDUCE_MINDIST { r#match[nmatch].criterion = d.contact[j].dist; }
        else if reduce == REDUCE_MAXFORCE {
            let mut forcetorque = [0.0; 6];
            contact_force(m, d, j as i32, &mut forcetorque);
            r#match[nmatch].criterion = -dot3(at3(&forcetorque, 0), at3(&forcetorque, 0));
        }
        nmatch += 1;
    }
    let nslot = num.min(nmatch);
    if reduce == REDUCE_MINDIST || reduce == REDUCE_MAXFORCE {
        let mut heap = d.scratch.contact_info(nslot);
        partial_sort(&mut r#match, &mut heap, nmatch, nslot, &mut contact_info_compare);
        d.scratch.put_contact_info(heap);
    } else if reduce == REDUCE_NETFORCE {
        if nmatch == 0 { d.scratch.put_contact_info(r#match); return; }
        let mut wrench = d.scratch.num(nmatch*6);
        let mut pos = d.scratch.num(nmatch*3);
        let mut frame = d.scratch.num(nmatch*9);
        for j in 0..nmatch {
            let conid = r#match[j].id;
            contact_force(m, d, conid as i32, at6_mut(&mut wrench, 6*j));
            copy3(at3_mut(&mut pos, 3*j), &d.contact[conid].pos);
            copy9(at9_mut(&mut frame, 9*j), &d.contact[conid].frame);
            if r#match[j].flip != 0 {
                let input = *at6(&wrench, 6*j);
                scl(&mut wrench[6*j..], &input, -1.0, 6);
            }
        }
        let mut point = [0.0; 3];
        let mut total_force = 0.0;
        for j in 0..nmatch {
            let weight = norm3(at3(&wrench, 6*j));
            add_to_scl3(&mut point, at3(&pos, 3*j), weight);
            total_force += weight;
        }
        let input = point;
        scl3(&mut point, &input, 1.0 / if total_force > MINVAL { total_force } else { MINVAL });
        let mut force = [0.0; 3];
        let mut torque = [0.0; 3];
        total_wrench(&mut force, &mut torque, &point, nmatch, &wrench, &pos, &frame);
        if let Some(k) = data[CONDATA_FOUND as usize] { sensordata[k] = nmatch as f64; }
        if let Some(k) = data[CONDATA_FORCE as usize] { copy3(at3_mut(sensordata, k), &force); }
        if let Some(k) = data[CONDATA_TORQUE as usize] { copy3(at3_mut(sensordata, k), &torque); }
        if let Some(k) = data[CONDATA_DIST as usize] { sensordata[k] = 0.0; }
        if let Some(k) = data[CONDATA_POS as usize] { copy3(at3_mut(sensordata, k), &point); }
        if let Some(k) = data[CONDATA_NORMAL as usize] { sensordata[k] = 1.0; }
        if let Some(k) = data[CONDATA_TANGENT as usize] { sensordata[k+1] = 1.0; }
        d.scratch.put_num(wrench);
        d.scratch.put_num(pos);
        d.scratch.put_num(frame);
        d.scratch.put_contact_info(r#match);
        return;
    }
    for j in 0..nslot {
        copy_sensor_data(m, d, sensordata, &data, r#match[j].id, r#match[j].flip, nmatch);
        for k in 0..NCONDATA as usize {
            if let Some(adr) = &mut data[k] { *adr += size; }
        }
    }
    d.scratch.put_contact_info(r#match);
}

// One ray's rangefinder data, the fields `dataspec` selects in order, written from `k` on; returns the
// index after them. A missed ray (`dist` < 0) reads zero direction, point and normal and depth -1. Depth
// is the hit's depth along the camera's view axis for a camera (`cam`: its position and z axis), the
// distance for a site.
fn fill_raydata(out: &mut [f64], mut k: usize, dataspec: i32, dist: f64, origin: &[f64; 3], direction: &[f64; 3],
    normal: &[f64; 3], cam: Option<(&[f64; 3], &[f64; 3])>) -> usize {
    let hit = dist >= 0.0;
    let has = |field: i32| dataspec & (1 << field) != 0;
    if has(RAYDATA_DIST) {
        out[k] = dist;
        k += 1;
    }
    if has(RAYDATA_DIR) {
        if hit { copy3(at3_mut(out, k), direction); } else { zero3(at3_mut(out, k)); }
        k += 3;
    }
    if has(RAYDATA_ORIGIN) {
        copy3(at3_mut(out, k), origin);
        k += 3;
    }
    // the hit point, for the point and depth fields
    let mut point = [0.0; 3];
    if (has(RAYDATA_POINT) || has(RAYDATA_DEPTH)) && hit {
        add_scl3(&mut point, origin, direction, dist);
    }
    if has(RAYDATA_POINT) {
        copy3(at3_mut(out, k), &point);
        k += 3;
    }
    if has(RAYDATA_NORMAL) {
        if hit { copy3(at3_mut(out, k), normal); } else { zero3(at3_mut(out, k)); }
        k += 3;
    }
    if has(RAYDATA_DEPTH) {
        out[k] = if !hit {
            -1.0
        } else if let Some((cam_xpos, cam_z)) = cam {
            let mut delta = [0.0; 3];
            sub3(&mut delta, &point, cam_xpos);
            -dot3(&delta, cam_z)
        } else {
            dist
        };
        k += 1;
    }
    k
}

fn compute_sensor_pos(m: &Model, d: &mut Data, i: usize, sensordata: &mut [f64]) {
    let ne = d.ne as usize;
    let nf = d.nf as usize;
    let nefc = d.nefc as usize;
    let r#type = m.sensor_type[i];
    let objtype = m.sensor_objtype[i];
    let objid = m.sensor_objid[i] as usize;
    let refid = m.sensor_refid[i];
    let reftype = m.sensor_reftype[i];
    let mut rvec = [0.0; 3];
    let mut xpos = &[0.0; 3];
    let mut xmat = &[0.0; 9];
    let mut xpos_ref = &[0.0; 3];
    let mut xmat_ref = &[0.0; 9];
    match r#type {
        SENS_MAGNETOMETER => mul_mat_tvec(sensordata, &d.site_xmat[9*objid..], &m.opt.magnetic, 3, 3),
        SENS_CAMPROJECTION => {
            let refid = refid as usize;
            cam_project(sensordata, at3(&d.site_xpos, 3*objid), at3(&d.cam_xpos, 3*refid),
                at9(&d.cam_xmat, 9*refid), &m.cam_resolution[2*refid..], m.cam_fovy[refid],
                &m.cam_intrinsic[4*refid..], &m.cam_sensorsize[2*refid..]);
        }
        SENS_RANGEFINDER => {
            let dataspec = m.sensor_intprm[i*NSENS];
            let want_normal = dataspec & (1 << RAYDATA_NORMAL) != 0;
            if objtype == OBJ_SITE {
                // one ray along the site's z axis
                rvec = [d.site_xmat[9*objid+2], d.site_xmat[9*objid+5], d.site_xmat[9*objid+8]];
                let origin = *at3(&d.site_xpos, 3*objid);
                let mut normal = [0.0; 3];
                let dist = ray::ray(m, d, &origin, &rvec, None, true, m.site_bodyid[objid], None,
                    if want_normal { Some(&mut normal) } else { None });
                fill_raydata(sensordata, 0, dataspec, dist, &origin, &rvec, &normal, None);
            } else {
                // a depth image: one ray per pixel, row by row
                let width = m.cam_resolution[2*objid] as usize;
                let height = m.cam_resolution[2*objid+1] as usize;
                let bodyexclude = m.cam_bodyid[objid];
                let cam_xpos = *at3(&d.cam_xpos, 3*objid);
                let cam_xmat = *at9(&d.cam_xmat, 9*objid);
                let projection = m.cam_projection[objid];
                // the camera's z axis, against its view direction
                let cam_z = [cam_xmat[2], cam_xmat[5], cam_xmat[8]];
                let cam = Some((&cam_xpos, &cam_z));
                let (fx, fy, cx, cy, ortho_extent) = cam_intrinsics(m, objid);
                let mut k = 0;
                if projection == PROJ_PERSPECTIVE {
                    // the rays share the camera position as their origin
                    let npixel = width * height;
                    let mut vec = d.scratch.num(3*npixel);
                    let mut dist = d.scratch.num(npixel);
                    let mut normals = if want_normal { Some(d.scratch.num(3*npixel)) } else { None };
                    let mut origin = [0.0; 3];
                    for row in 0..height {
                        for col in 0..width {
                            let idx = row*width + col;
                            cam_pixel_ray(&mut origin, at3_mut(&mut vec, 3*idx), &cam_xpos, &cam_xmat,
                                col as i32, row as i32, fx, fy, cx, cy, projection, ortho_extent);
                        }
                    }
                    ray::multi_ray(m, d, &cam_xpos, &vec, None, true, bodyexclude, None, &mut dist,
                        normals.as_deref_mut(), npixel, MAXVAL);
                    for idx in 0..npixel {
                        let normal = normals.as_deref().map_or([0.0; 3], |n| *at3(n, 3*idx));
                        k = fill_raydata(sensordata, k, dataspec, dist[idx], &cam_xpos, at3(&vec, 3*idx), &normal, cam);
                    }
                    d.scratch.put_num(vec);
                    d.scratch.put_num(dist);
                    if let Some(normals) = normals { d.scratch.put_num(normals); }
                } else {
                    // parallel rays from points on the image plane
                    for row in 0..height {
                        for col in 0..width {
                            let mut origin = [0.0; 3];
                            let mut direction = [0.0; 3];
                            cam_pixel_ray(&mut origin, &mut direction, &cam_xpos, &cam_xmat,
                                col as i32, row as i32, fx, fy, cx, cy, projection, ortho_extent);
                            let mut normal = [0.0; 3];
                            let dist = ray::ray(m, d, &origin, &direction, None, true, bodyexclude, None, Some(&mut normal));
                            k = fill_raydata(sensordata, k, dataspec, dist, &origin, &direction, &normal, cam);
                        }
                    }
                }
            }
        }
        SENS_JOINTPOS => sensordata[0] = d.qpos[m.jnt_qposadr[objid] as usize],
        SENS_TENDONPOS => sensordata[0] = d.ten_length[objid],
        SENS_ACTUATORPOS => copy(sensordata, &d.actuator_length[m.actuator_outadr[objid] as usize..], m.sensor_dim[i] as usize),
        SENS_BALLQUAT => {
            copy4(at4_mut(sensordata, 0), at4(&d.qpos, m.jnt_qposadr[objid] as usize));
            normalize4(at4_mut(sensordata, 0));
        }
        SENS_JOINTLIMITPOS => {
            sensordata[0] = 0.0;
            for j in ne+nf..nefc {
                if d.efc_type[j] == CNSTR_LIMIT_JOINT && d.efc_id[j] == objid as i32 {
                    sensordata[0] = d.efc_pos[j] - d.efc_margin[j]; break;
                }
            }
        }
        SENS_TENDONLIMITPOS => {
            sensordata[0] = 0.0;
            for j in ne+nf..nefc {
                if d.efc_type[j] == CNSTR_LIMIT_TENDON && d.efc_id[j] == objid as i32 {
                    sensordata[0] = d.efc_pos[j] - d.efc_margin[j]; break;
                }
            }
        }
        SENS_FRAMEPOS | SENS_FRAMEXAXIS | SENS_FRAMEYAXIS | SENS_FRAMEZAXIS => {
            get_xpos_xmat(d, objtype, objid, i, &mut xpos, &mut xmat);
            if refid == -1 {
                if r#type == SENS_FRAMEPOS { copy3(at3_mut(sensordata, 0), xpos); }
                else {
                    let offset = (r#type - SENS_FRAMEXAXIS) as usize;
                    sensordata[0] = xmat[offset];
                    sensordata[1] = xmat[offset+3];
                    sensordata[2] = xmat[offset+6];
                }
            } else {
                get_xpos_xmat(d, reftype, refid as usize, i, &mut xpos_ref, &mut xmat_ref);
                if r#type == SENS_FRAMEPOS {
                    sub3(&mut rvec, xpos, xpos_ref);
                    mul_mat_tvec3(at3_mut(sensordata, 0), xmat_ref, &rvec);
                } else {
                    let offset = (r#type - SENS_FRAMEXAXIS) as usize;
                    let axis = [xmat[offset], xmat[offset+3], xmat[offset+6]];
                    mul_mat_tvec3(at3_mut(sensordata, 0), xmat_ref, &axis);
                }
            }
        }
        SENS_FRAMEQUAT => {
            let mut cam_obj = [0.0; 4];
            let objquat = get_xquat(m, d, objtype, objid, i, &mut cam_obj);
            if refid == -1 { crate::inline::copy4(at4_mut(sensordata, 0), objquat); }
            else {
                let mut cam_ref = [0.0; 4];
                let mut negref = [0.0; 4];
                let refquat = get_xquat(m, d, reftype, refid as usize, i, &mut cam_ref);
                crate::inline::neg_quat(&mut negref, refquat);
                crate::inline::mul_quat(at4_mut(sensordata, 0), &negref, objquat);
            }
        }
        SENS_SUBTREECOM => copy3(at3_mut(sensordata, 0), at3(&d.subtree_com, 3*objid)),
        SENS_INSIDESITE => {
            if m.sensor_intprm[i*NSENS] != 0 {
                let mut obj1 = CcdObj::default();
                let mut obj2 = CcdObj::default();
                init_ccd_obj_site(&mut obj2, m, d, refid, 0.0);
                let niter = m.opt.ccd_iterations;
                let tol = m.opt.ccd_tolerance;
                if objtype == OBJ_GEOM {
                    init_ccd_obj(&mut obj1, m, Some(d), objid as i32, 0.0);
                    sensordata[0] = hausdorff(&mut obj1, &mut obj2, niter, 0.5, tol);
                } else if objtype == OBJ_SITE {
                    init_ccd_obj_site(&mut obj1, m, d, objid as i32, 0.0);
                    sensordata[0] = hausdorff(&mut obj1, &mut obj2, niter, 0.5, tol);
                } else if objtype == OBJ_BODY {
                    let n1 = m.body_geomnum[objid];
                    let id1 = m.body_geomadr[objid];
                    let mut max_dist = -MAXVAL;
                    for g in id1..id1+n1 {
                        init_ccd_obj(&mut obj1, m, Some(d), g, 0.0);
                        max_dist = max(max_dist, hausdorff(&mut obj1, &mut obj2, niter, 0.5, tol));
                    }
                    sensordata[0] = max_dist;
                } else if objtype == OBJ_XBODY {
                    let mut max_dist = -MAXVAL;
                    for g in 0..m.ngeom {
                        let mut b = m.geom_bodyid[g];
                        while b > objid as i32 { b = m.body_parentid[b as usize]; }
                        if b == objid as i32 {
                            init_ccd_obj(&mut obj1, m, Some(d), g as i32, 0.0);
                            max_dist = max(max_dist, hausdorff(&mut obj1, &mut obj2, niter, 0.5, tol));
                        }
                    }
                    sensordata[0] = max_dist;
                }
                return;
            }
            get_xpos_xmat(d, objtype, objid, i, &mut xpos, &mut xmat);
            if objtype == OBJ_BODY && objid > 0 && m.body_mass[objid] < MINVAL && m.body_subtreemass[objid] >= MINVAL {
                xpos = at3(&d.subtree_com, 3*objid);
            }
            sensordata[0] = inside_site(m, d, refid as usize, xpos) as f64;
        }
        SENS_GEOMDIST | SENS_GEOMNORMAL | SENS_GEOMFROMTO => {
            let cutoff = m.sensor_cutoff[i];
            let mut dist = cutoff;
            let mut fromto = [0.0; 6];
            let (n1, id1) = if objtype == OBJ_BODY { (m.body_geomnum[objid], m.body_geomadr[objid]) } else { (1, objid as i32) };
            let (n2, id2) = if reftype == OBJ_BODY { (m.body_geomnum[refid as usize], m.body_geomadr[refid as usize]) } else { (1, refid) };
            for geom1 in id1..id1+n1 {
                for geom2 in id2..id2+n2 {
                    let mut fromto_new = [0.0; 6];
                    let dist_new = geom_distance(m, d, geom1, geom2, cutoff, Some(&mut fromto_new));
                    if dist_new < dist { dist = dist_new; copy(&mut fromto, &fromto_new, 6); }
                }
            }
            if r#type == SENS_GEOMDIST { sensordata[0] = dist; }
            else if r#type == SENS_GEOMNORMAL {
                let mut normal = [fromto[3]-fromto[0], fromto[4]-fromto[1], fromto[5]-fromto[2]];
                if normal[0] != 0.0 || normal[1] != 0.0 || normal[2] != 0.0 { normalize3(&mut normal); }
                copy3(at3_mut(sensordata, 0), &normal);
            } else { copy(sensordata, &fromto, 6); }
        }
        SENS_E_POTENTIAL => {
            if d.flg_energypos == 0 { energy_pos(m, d); }
            sensordata[0] = d.energy[0];
        }
        SENS_E_KINETIC => {
            if d.flg_energyvel == 0 { energy_vel(m, d); }
            sensordata[0] = d.energy[1];
        }
        SENS_CLOCK => sensordata[0] = d.time,
        SENS_CONTACT => compute_contact_sensor(m, d, i, sensordata),
        _ => panic!("invalid sensor type in POS stage, sensor {}", i),
    }
}

fn compute_sensor_vel(m: &Model, d: &mut Data, i: usize, sensordata: &mut [f64]) {
    let ne = d.ne as usize;
    let nf = d.nf as usize;
    let nefc = d.nefc as usize;
    let r#type = m.sensor_type[i];
    let objtype = m.sensor_objtype[i];
    let objid = m.sensor_objid[i] as usize;
    let refid = m.sensor_refid[i];
    let reftype = m.sensor_reftype[i];
    let mut xvel = [0.0; 6];
    if d.flg_subtreevel == 0 && (r#type == SENS_SUBTREELINVEL || r#type == SENS_SUBTREEANGMOM) { subtree_vel(m, d); }
    match r#type {
        SENS_VELOCIMETER => {
            object_velocity(m, d, OBJ_SITE, objid, &mut xvel, 1);
            copy3(at3_mut(sensordata, 0), at3(&xvel, 3));
        }
        SENS_GYRO => {
            object_velocity(m, d, OBJ_SITE, objid, &mut xvel, 1);
            copy3(at3_mut(sensordata, 0), at3(&xvel, 0));
        }
        SENS_JOINTVEL => sensordata[0] = d.qvel[m.jnt_dofadr[objid] as usize],
        SENS_TENDONVEL => sensordata[0] = d.ten_velocity[objid],
        SENS_ACTUATORVEL => copy(sensordata, &d.actuator_velocity[m.actuator_outadr[objid] as usize..], m.sensor_dim[i] as usize),
        SENS_BALLANGVEL => copy3(at3_mut(sensordata, 0), at3(&d.qvel, m.jnt_dofadr[objid] as usize)),
        SENS_JOINTLIMITVEL => {
            sensordata[0] = 0.0;
            for j in ne+nf..nefc {
                if d.efc_type[j] == CNSTR_LIMIT_JOINT && d.efc_id[j] == objid as i32 { sensordata[0] = d.efc_vel[j]; break; }
            }
        }
        SENS_TENDONLIMITVEL => {
            sensordata[0] = 0.0;
            for j in ne+nf..nefc {
                if d.efc_type[j] == CNSTR_LIMIT_TENDON && d.efc_id[j] == objid as i32 { sensordata[0] = d.efc_vel[j]; break; }
            }
        }
        SENS_FRAMELINVEL | SENS_FRAMEANGVEL => {
            object_velocity(m, d, objtype, objid, &mut xvel, 0);
            if refid > -1 {
                let mut xpos = &[0.0; 3];
                let mut xmat = &[0.0; 9];
                let mut xpos_ref = &[0.0; 3];
                let mut xmat_ref = &[0.0; 9];
                let mut xvel_ref = [0.0; 6];
                let mut rel_vel = [0.0; 6];
                // cross is the C local, distinct from the cross() helper.
                let mut cross = [0.0; 3];
                let mut rvec = [0.0; 3];
                get_xpos_xmat(d, objtype, objid, i, &mut xpos, &mut xmat);
                get_xpos_xmat(d, reftype, refid as usize, i, &mut xpos_ref, &mut xmat_ref);
                object_velocity(m, d, reftype, refid as usize, &mut xvel_ref, 0);
                sub(&mut rel_vel, &xvel, &xvel_ref, 6);
                sub3(&mut rvec, xpos, xpos_ref);
                crate::util_spatial::cross(&mut cross, &rvec, at3(&xvel_ref, 0));
                add_to3(at3_mut(&mut rel_vel, 3), &cross);
                mul_mat_tvec3(at3_mut(&mut xvel, 0), xmat_ref, at3(&rel_vel, 0));
                mul_mat_tvec3(at3_mut(&mut xvel, 3), xmat_ref, at3(&rel_vel, 3));
            }
            if r#type == SENS_FRAMELINVEL { copy3(at3_mut(sensordata, 0), at3(&xvel, 3)); }
            else { copy3(at3_mut(sensordata, 0), at3(&xvel, 0)); }
        }
        SENS_SUBTREELINVEL => copy3(at3_mut(sensordata, 0), at3(&d.subtree_linvel, 3*objid)),
        SENS_SUBTREEANGMOM => copy3(at3_mut(sensordata, 0), at3(&d.subtree_angmom, 3*objid)),
        SENS_CONTACT => compute_contact_sensor(m, d, i, sensordata),
        SENS_TACTILE => {
            let mesh_id = m.sensor_objid[i] as usize;
            let geom_id = m.sensor_refid[i] as usize;
            let parent_body = m.geom_bodyid[geom_id] as usize;
            let parent_weld = m.body_weldid[parent_body] as usize;
            // nchannel is assigned to the C task but is never read by it.
            let nchannel = m.sensor_dim[i] / m.mesh_vertnum[mesh_id];
            let _ = nchannel;
            zero(sensordata, m.sensor_dim[i] as usize);
            let mut seen_geom = d.scratch.byte(m.ngeom);
            seen_geom.fill(0);
            let mut contact_geom_ids = d.scratch.int((2*d.ncon as usize).min(m.ngeom));
            let mut ncontact = 0;
            for k in 0..d.ncon as usize {
                let g1 = d.contact[k].geom[0];
                let g2 = d.contact[k].geom[1];
                let body1 = if g1 >= 0 { m.body_weldid[m.geom_bodyid[g1 as usize] as usize] } else { crate::unported!("mj_flexBody") };
                let body2 = if g2 >= 0 { m.body_weldid[m.geom_bodyid[g2 as usize] as usize] } else { crate::unported!("mj_flexBody") };
                if body1 == parent_weld as i32 && g2 >= 0 && (g2 as usize) < m.ngeom && g2 != geom_id as i32 && seen_geom[g2 as usize] == 0 {
                    seen_geom[g2 as usize] = 1;
                    contact_geom_ids[ncontact] = g2; ncontact += 1;
                }
                if body2 == parent_weld as i32 && g1 >= 0 && (g1 as usize) < m.ngeom && g1 != geom_id as i32 && seen_geom[g1 as usize] == 0 {
                    seen_geom[g1 as usize] = 1;
                    contact_geom_ids[ncontact] = g1; ncontact += 1;
                }
            }
            if ncontact == 0 {
                d.scratch.put_byte(seen_geom);
                d.scratch.put_int(contact_geom_ids);
                return;
            }
            let ncon = m.mesh_vertnum[mesh_id] as usize;
            let k_tactile_parallel_threshold = 1000;
            let nthread = num_thread(d);
            if nthread > 0 && ncon >= k_tactile_parallel_threshold {
                let batch_size = (ncon + nthread - 1) / nthread;
                let ntask = (ncon + batch_size - 1) / batch_size;
                // thread.rs has one calling thread: ntask is exactly one. The
                // task descriptor stays on the Rust stack instead of an arena.
                let mut task_args = [TactileTaskArgs { mesh_id, geom_id, parent_weld, ncontact,
                    contact_geom_ids: &contact_geom_ids, start_taxel: 0,
                    end_taxel: batch_size.min(ncon), sensordata }];
                dispatch(m, d, &mut tactile_task, &mut task_args, ntask);
            } else {
                let mut args = TactileTaskArgs { mesh_id, geom_id, parent_weld, ncontact,
                    contact_geom_ids: &contact_geom_ids, start_taxel: 0, end_taxel: ncon, sensordata };
                tactile_taxel_batch(m, d, &mut args);
            }
            d.scratch.put_byte(seen_geom);
            d.scratch.put_int(contact_geom_ids);
        }
        _ => panic!("invalid type in VEL stage, sensor {}", i),
    }
}

fn compute_sensor_acc(m: &Model, d: &mut Data, i: usize, sensordata: &mut [f64]) {
    let ne = d.ne as usize;
    let nf = d.nf as usize;
    let nefc = d.nefc as usize;
    let nactuator = m.nactuator;
    let r#type = m.sensor_type[i];
    let objtype = m.sensor_objtype[i];
    let objid = m.sensor_objid[i] as usize;
    let mut tmp = [0.0; 6];
    let mut conforce = [0.0; 6];
    let mut conray = [0.0; 3];
    if d.flg_rnepost == 0 && (r#type == SENS_ACCELEROMETER || r#type == SENS_FORCE ||
        r#type == SENS_TORQUE || r#type == SENS_FRAMELINACC || r#type == SENS_FRAMEANGACC) {
        rne_post_constraint(m, d);
    }
    match r#type {
        SENS_TOUCH => {
            let bodyid = m.site_bodyid[objid];
            sensordata[0] = 0.0;
            for j in 0..d.ncon as usize {
                let con = &d.contact[j];
                let mut conbody = [0; 2];
                for k in 0..2 {
                    conbody[k] = if con.geom[k] >= 0 { m.geom_bodyid[con.geom[k] as usize] }
                        else { crate::unported!("mj_flexBody") };
                }
                if con.efc_address >= 0 && (bodyid == conbody[0] || bodyid == conbody[1]) {
                    contact_force(m, d, j as i32, &mut conforce);
                    if conforce[0] <= 0.0 { continue; }
                    scl3(&mut conray, at3(&con.frame, 0), conforce[0]);
                    normalize3(&mut conray);
                    if bodyid == conbody[1] {
                        let input = conray;
                        scl3(&mut conray, &input, -1.0);
                    }
                    if crate::ray::ray_geom(at3(&d.site_xpos, 3*objid), Some(at9(&d.site_xmat, 9*objid)),
                        at3(&m.site_size, 3*objid), &con.pos, &conray, m.site_type[objid], None) >= 0.0 {
                        sensordata[0] += conforce[0];
                    }
                }
            }
        }
        SENS_CONTACT => compute_contact_sensor(m, d, i, sensordata),
        SENS_ACCELEROMETER => {
            object_acceleration(m, d, OBJ_SITE, objid, &mut tmp, 1);
            copy3(at3_mut(sensordata, 0), at3(&tmp, 3));
        }
        SENS_FORCE => {
            let bodyid = m.site_bodyid[objid] as usize;
            let rootid = m.body_rootid[bodyid] as usize;
            transform_spatial(&mut tmp, at6(&d.cfrc_int, 6*bodyid), 1,
                at3(&d.site_xpos, 3*objid), at3(&d.subtree_com, 3*rootid), Some(at9(&d.site_xmat, 9*objid)));
            copy3(at3_mut(sensordata, 0), at3(&tmp, 3));
        }
        SENS_TORQUE => {
            let bodyid = m.site_bodyid[objid] as usize;
            let rootid = m.body_rootid[bodyid] as usize;
            transform_spatial(&mut tmp, at6(&d.cfrc_int, 6*bodyid), 1,
                at3(&d.site_xpos, 3*objid), at3(&d.subtree_com, 3*rootid), Some(at9(&d.site_xmat, 9*objid)));
            copy3(at3_mut(sensordata, 0), at3(&tmp, 0));
        }
        SENS_ACTUATORFRC => copy(sensordata, &d.actuator_force[m.actuator_outadr[objid] as usize..], m.sensor_dim[i] as usize),
        SENS_JOINTACTFRC => sensordata[0] = d.qfrc_actuator[m.jnt_dofadr[objid] as usize],
        SENS_TENDONACTFRC => {
            let mut frc = 0.0;
            for j in 0..nactuator {
                if m.actuator_trntype[j] == TRN_TENDON && m.actuator_trnid[2*j] == objid as i32 {
                    frc += d.actuator_force[m.actuator_outadr[j] as usize];
                }
            }
            sensordata[0] = frc;
        }
        SENS_JOINTLIMITFRC => {
            sensordata[0] = 0.0;
            for j in ne+nf..nefc {
                if d.efc_type[j] == CNSTR_LIMIT_JOINT && d.efc_id[j] == objid as i32 { sensordata[0] = d.efc_force[j]; break; }
            }
        }
        SENS_TENDONLIMITFRC => {
            sensordata[0] = 0.0;
            for j in ne+nf..nefc {
                if d.efc_type[j] == CNSTR_LIMIT_TENDON && d.efc_id[j] == objid as i32 { sensordata[0] = d.efc_force[j]; break; }
            }
        }
        SENS_FRAMELINACC | SENS_FRAMEANGACC => {
            object_acceleration(m, d, objtype, objid, &mut tmp, 0);
            if r#type == SENS_FRAMELINACC { copy3(at3_mut(sensordata, 0), at3(&tmp, 3)); }
            else { copy3(at3_mut(sensordata, 0), at3(&tmp, 0)); }
        }
        _ => panic!("invalid type in ACC stage, sensor {}", i),
    }
}

pub fn compute_sensor(m: &Model, d: &mut Data, i: usize, sensordata: &mut [f64]) {
    match m.sensor_needstage[i] {
        STAGE_POS => compute_sensor_pos(m, d, i, sensordata),
        STAGE_VEL => compute_sensor_vel(m, d, i, sensordata),
        STAGE_ACC => compute_sensor_acc(m, d, i, sensordata),
        _ => panic!("invalid sensor stage for sensor {}", i),
    }
    apply_cutoff(m, i, sensordata);
}

fn compute_or_read_sensor(m: &Model, d: &mut Data, i: usize, sensordata: &mut [f64]) {
    let nsample = m.sensor_history[2*i];
    if nsample <= 0 { compute_sensor(m, d, i, sensordata); return; }
    let delay = m.sensor_delay[i];
    if delay > 0.0 { crate::unported!("mj_readSensor"); }
    let interval = m.sensor_interval[2*i];
    if interval > 0.0 {
        let historyadr = m.sensor_historyadr[i] as usize;
        let buf = &d.history[historyadr..];
        let time_prev = buf[0];
        if time_prev + interval <= d.time { compute_sensor(m, d, i, sensordata); }
        else { crate::unported!("mj_readSensor"); }
        return;
    }
    compute_sensor(m, d, i, sensordata);
}

fn compute_user_sensors(m: &Model, d: &mut Data, stage: i32) {
    // mjcb_sensor is unset: global callbacks are not part of the port.
    for i in 0..m.nsensor {
        if m.sensor_type[i] == SENS_USER && m.sensor_needstage[i] == stage {
            apply_cutoff(m, i, &mut d.sensordata[m.sensor_adr[i] as usize..]);
        }
    }
}

// The loop calls compute for plugins with mjPLUGIN_SENSOR at this stage; every plugin the port
// admits is an SDF plugin (check_supported), which has no SENSOR capability, so it calls nothing.
fn compute_plugin_sensors(_m: &Model, _d: &mut Data, _stage: i32) {}

pub fn sensor_pos(m: &Model, d: &mut Data) {
    let nsensor = m.nsensor;
    let mut nusersensor = 0;
    if m.opt.disableflags & DSBL_SENSOR != 0 { return; }
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && (d.nbody_awake as usize) < m.nbody;
    for i in 0..nsensor {
        let r#type = m.sensor_type[i];
        if sleep_filter && m.sensor_history[2*i] == 0 && sleep_state(m, d, OBJ_SENSOR, i as i32) == S_ASLEEP { continue; }
        if r#type == SENS_PLUGIN { continue; }
        if m.sensor_needstage[i] == STAGE_POS {
            let adr = m.sensor_adr[i] as usize;
            let dim = m.sensor_dim[i] as usize;
            if r#type == SENS_USER {
                zero(&mut d.sensordata[adr..], dim);
                nusersensor += 1;
            } else {
                // Copy out/in keeps the C output separate from mutable Data;
                // callees do not read sensordata. Scratch capacity is retained.
                let mut sensordata = d.scratch.num(dim);
                copy(&mut sensordata, &d.sensordata[adr..], dim);
                compute_or_read_sensor(m, d, i, &mut sensordata);
                copy(&mut d.sensordata[adr..], &sensordata, dim);
                d.scratch.put_num(sensordata);
            }
        }
    }
    if nusersensor != 0 { compute_user_sensors(m, d, STAGE_POS); }
    compute_plugin_sensors(m, d, STAGE_POS);
}

pub fn sensor_vel(m: &Model, d: &mut Data) {
    let mut nusersensor = 0;
    if m.opt.disableflags & DSBL_SENSOR != 0 { return; }
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && (d.nbody_awake as usize) < m.nbody;
    for i in 0..m.nsensor {
        if m.sensor_type[i] == SENS_PLUGIN { continue; }
        if sleep_filter && m.sensor_history[2*i] == 0 && sleep_state(m, d, OBJ_SENSOR, i as i32) == S_ASLEEP { continue; }
        if m.sensor_needstage[i] == STAGE_VEL {
            let r#type = m.sensor_type[i];
            let adr = m.sensor_adr[i] as usize;
            let dim = m.sensor_dim[i] as usize;
            if r#type == SENS_USER {
                if d.flg_subtreevel == 0 { subtree_vel(m, d); }
                zero(&mut d.sensordata[adr..], dim);
                nusersensor += 1;
            } else {
                let mut sensordata = d.scratch.num(dim);
                copy(&mut sensordata, &d.sensordata[adr..], dim);
                compute_or_read_sensor(m, d, i, &mut sensordata);
                copy(&mut d.sensordata[adr..], &sensordata, dim);
                d.scratch.put_num(sensordata);
            }
        }
    }
    if nusersensor != 0 { compute_user_sensors(m, d, STAGE_VEL); }
    compute_plugin_sensors(m, d, STAGE_VEL);
}

pub fn sensor_acc(m: &Model, d: &mut Data) {
    let mut nusersensor = 0;
    if m.opt.disableflags & DSBL_SENSOR != 0 { return; }
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && (d.nbody_awake as usize) < m.nbody;
    for i in 0..m.nsensor {
        if sleep_filter && m.sensor_history[2*i] == 0 && sleep_state(m, d, OBJ_SENSOR, i as i32) == S_ASLEEP { continue; }
        if m.sensor_type[i] == SENS_PLUGIN { continue; }
        if m.sensor_needstage[i] == STAGE_ACC {
            let r#type = m.sensor_type[i];
            let adr = m.sensor_adr[i] as usize;
            let dim = m.sensor_dim[i] as usize;
            if r#type == SENS_USER {
                if d.flg_rnepost == 0 { rne_post_constraint(m, d); }
                zero(&mut d.sensordata[adr..], dim);
                nusersensor += 1;
            } else {
                let mut sensordata = d.scratch.num(dim);
                copy(&mut sensordata, &d.sensordata[adr..], dim);
                compute_or_read_sensor(m, d, i, &mut sensordata);
                copy(&mut d.sensordata[adr..], &sensordata, dim);
                d.scratch.put_num(sensordata);
            }
        }
    }
    if nusersensor != 0 { compute_user_sensors(m, d, STAGE_ACC); }
    compute_plugin_sensors(m, d, STAGE_ACC);
}

pub fn energy_pos(m: &Model, d: &mut Data) {
    let mut dif = [0.0; 3];
    let mut quat = [0.0; 4];
    d.energy[0] = 0.0;
    if m.opt.disableflags & DSBL_GRAVITY == 0 {
        for i in 1..m.nbody {
            d.energy[0] -= m.body_mass[i] * dot3(&m.opt.gravity, at3(&d.xipos, 3*i));
        }
    }
    if m.opt.disableflags & DSBL_SPRING == 0 {
        let nbody = m.nbody;
        for b in 1..nbody {
            let jnt_start = m.body_jntadr[b];
            let jnt_end = jnt_start + m.body_jntnum[b];
            for j in jnt_start..jnt_end {
                let j = j as usize;
                let stiffness = m.jnt_stiffness[j];
                let poly = &m.jnt_stiffnesspoly[NPOLY*j..];
                if stiffness == 0.0 && is_zero(poly, NPOLY) != 0 { continue; }
                let mut padr = m.jnt_qposadr[j] as usize;
                let mut qspring = if m.jnt_plasticadr[j] >= 0 {
                    &d.plastic[m.jnt_plasticadr[j] as usize..]
                } else { &m.qpos_spring[padr..] };
                match m.jnt_type[j] {
                    JNT_FREE | JNT_BALL => {
                        if m.jnt_type[j] == JNT_FREE {
                            sub3(&mut dif, at3(&d.qpos, padr), at3(qspring, 0));
                            let x = norm3(&dif);
                            d.energy[0] += poly_potential(stiffness, poly, x, NPOLY, 0);
                            padr += 3;
                            qspring = &qspring[3..];
                        }
                        // FREE falls through to BALL. C normalizes quat, then
                        // deliberately uses the original qpos in subQuat.
                        copy4(&mut quat, at4(&d.qpos, padr));
                        normalize4(&mut quat);
                        sub_quat(&mut dif, at4(&d.qpos, padr), at4(qspring, 0));
                        let x = norm3(&dif);
                        d.energy[0] += poly_potential(stiffness, poly, x, NPOLY, 0);
                    }
                    JNT_SLIDE | JNT_HINGE => {
                        let x = d.qpos[padr] - qspring[0];
                        d.energy[0] += poly_potential(stiffness, poly, x, NPOLY, 0);
                    }
                    _ => {}
                }
            }
        }
    }
    if m.opt.disableflags & DSBL_SPRING == 0 {
        for i in 0..m.ntendon {
            let stiffness = m.tendon_stiffness[i];
            let poly = &m.tendon_stiffnesspoly[NPOLY*i..];
            let length = d.ten_length[i];
            let lengthspring = if m.tendon_plasticadr[i] >= 0 {
                &d.plastic[m.tendon_plasticadr[i] as usize..]
            } else { &m.tendon_lengthspring[2*i..] };
            let lower = lengthspring[0];
            let upper = lengthspring[1];
            let x = if length > upper { length - upper } else if length < lower { length - lower } else { 0.0 };
            d.energy[0] += poly_potential(stiffness, poly, x, NPOLY, 0);
        }
    }
    if m.opt.disableflags & DSBL_SPRING == 0 {
        for i in 0..m.nflex {
            let stiffness = m.flex_edgestiffness[i];
            if m.flex_rigid[i] != 0 || m.flex_interp[i] != 0 || stiffness == 0.0 { continue; }
            crate::unported!("mj_energyPos: flex edge springs");
        }
    }
    d.flg_energypos = 1;
}

pub fn energy_vel(m: &Model, d: &mut Data) {
    let mut vec = d.scratch.num(m.nv);
    mul_m(m, d, &mut vec, &d.qvel);
    d.energy[1] = 0.5 * dot(&vec, &d.qvel, m.nv);
    d.scratch.put_num(vec);
    d.flg_energyvel = 1;
}
