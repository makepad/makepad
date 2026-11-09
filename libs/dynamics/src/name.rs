//! Object name hashing and lookup.
//!
//! Translated from `src/engine/engine_name.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::model::Model;
use crate::types::*;

// _getnumadr. The selected arm and all subsequent arms subtract from mapadr,
// preserving the C switch fallthrough order.
fn getnumadr<'a>(m: &'a Model, r#type: i32, padr: &mut &'a [i32], mapadr: &mut usize) -> usize {
    let lists: [(i32, usize, &[i32]); 23] = [
        (OBJ_BODY, m.nbody, &m.name_bodyadr),
        (OBJ_JOINT, m.njnt, &m.name_jntadr),
        (OBJ_GEOM, m.ngeom, &m.name_geomadr),
        (OBJ_SITE, m.nsite, &m.name_siteadr),
        (OBJ_CAMERA, m.ncam, &m.name_camadr),
        (OBJ_LIGHT, m.nlight, &m.name_lightadr),
        (OBJ_FLEX, m.nflex, &m.name_flexadr),
        (OBJ_MESH, m.nmesh, &m.name_meshadr),
        (OBJ_SKIN, m.nskin, &m.name_skinadr),
        (OBJ_HFIELD, m.nhfield, &m.name_hfieldadr),
        (OBJ_TEXTURE, m.ntex, &m.name_texadr),
        (OBJ_MATERIAL, m.nmat, &m.name_matadr),
        (OBJ_PAIR, m.npair, &m.name_pairadr),
        (OBJ_EXCLUDE, m.nexclude, &m.name_excludeadr),
        (OBJ_EQUALITY, m.neq, &m.name_eqadr),
        (OBJ_TENDON, m.ntendon, &m.name_tendonadr),
        (OBJ_ACTUATOR, m.nactuator, &m.name_actuatoradr),
        (OBJ_SENSOR, m.nsensor, &m.name_sensoradr),
        (OBJ_NUMERIC, m.nnumeric, &m.name_numericadr),
        (OBJ_TEXT, m.ntext, &m.name_textadr),
        (OBJ_TUPLE, m.ntuple, &m.name_tupleadr),
        (OBJ_KEY, m.nkey, &m.name_keyadr),
        (OBJ_PLUGIN, m.nplugin, &m.name_pluginadr),
    ];
    let r#type = if r#type == OBJ_XBODY { OBJ_BODY } else { r#type };
    let mut num = None;
    *mapadr = m.nnames_map;
    for (kind, count, addresses) in lists {
        if kind == r#type || num.is_some() {
            *mapadr -= 2 * count;
            if num.is_none() {
                *padr = addresses;
                num = Some(count);
            }
        }
    }
    if num.is_none() { *padr = &[]; }
    num.unwrap_or(0)
}

/// mj_hashString. The reference target's plain char is signed.
pub fn hash_string(s: &str, n: u64) -> u64 {
    let mut h = 5381_u64;
    for c in s.bytes() {
        if c == 0 { break; }
        h = (h << 5).wrapping_add(h) ^ (c as i8 as i64 as u64);
    }
    h % n
}

/// mj_name2id.
pub fn name2id(m: &Model, r#type: i32, name: &str) -> i32 {
    let mut mapadr = 0;
    let mut adr: &[i32] = &[];
    let num = 2 * getnumadr(m, r#type, &mut adr, &mut mapadr);
    if num != 0 {
        let hash = hash_string(name, num as u64) as usize;
        let mut i = hash;
        loop {
            let j = m.names_map[mapadr + i];
            if j < 0 { return -1; }
            let start = adr[j as usize] as usize;
            let bytes = &m.names[start..m.nnames];
            let mut equal = true;
            for (k, &c) in bytes.iter().enumerate() {
                let other = name.as_bytes().get(k).copied().unwrap_or(0);
                if c != other { equal = false; break; }
                if c == 0 { break; }
            }
            if equal { return j; }
            i += 1;
            if i == num { i = 0; }
            if i == hash { break; }
        }
    }
    -1
}

/// mj_id2name. Return the C string bytes without the terminating zero.
pub fn id2name(m: &Model, r#type: i32, id: i32) -> Option<&[u8]> {
    let mut mapadr = 0;
    let mut adr: &[i32] = &[];
    let num = getnumadr(m, r#type, &mut adr, &mut mapadr);
    if id >= 0 && (id as usize) < num && m.names[adr[id as usize] as usize] != 0 {
        let bytes = &m.names[adr[id as usize] as usize..];
        return Some(&bytes[..bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len())]);
    }
    None
}

// mj_actuatorInputName is outside the builder's actuator-free subset.
