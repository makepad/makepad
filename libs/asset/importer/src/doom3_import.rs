//! Doom 3 / id Tech 4 classic importer.
//!
//! Local-folder only. Converts original formats into catalog payloads — never
//! ships `.pk4` / `.md5mesh` / `.proc` / `.lwo` as asset types:
//!
//! - PK4 (zip) → extracted files for the parsers below
//! - MD5 mesh (`MD5Version 10`) → bind-pose `Character` / `Weapon` / `Prop` GLB
//! - `.proc` compiled world → one flat `World` GLB (visible surfaces + atlas)
//! - `.lwo` LightWave (LWO2 LAYR/PNTS/POLS + TXUV) → `Prop` GLB, or `None`
//! - TGA / PNG / DDS + `.mtr` → mesh albedo via the texture bank
//! - WAV is copied by [`crate::classic_import`] after expand
//!
//! Pack identity (license, download, credits) lives on the source, not here.

use crate::classic_import::{encode_png_rgba, ClassicAsset};
use crate::quake3_import::{decode_tga, expand_pk3, Pk3File, Q3TexBank};
use makepad_asset_data::AssetKind;
use makepad_gltf::{write_glb_mesh_textured, GlbTexturedMesh};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// id Tech 4 units ARE inches: the engine's own `DOOM_TO_METERS(x)` is
/// `x * 0.0254` (`Game_local.h`), the 74-unit player (`pm_normalheight`)
/// stands 1.88 m and looks out at 68 u = 1.73 m. It was 1/32 (a 2.31 m
/// marine) until 2026-08-26.
const SCALE: f32 = 0.0254;

// ---------------------------------------------------------------------------
// PK4 = zip (same container as Q3 PK3)
// ---------------------------------------------------------------------------

pub fn expand_pk4(
    path: &Path,
    out_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<Vec<Pk3File>, String> {
    expand_pk3(path, out_dir, warnings)
}

pub fn is_fan_mission_rel(rel: &str) -> bool {
    let n = rel.replace('\\', "/").to_ascii_lowercase();
    n.starts_with("fms/")
        || n.contains("/fms/")
        || n.starts_with("fanmissions/")
        || n.contains("/fanmissions/")
        || n.starts_with("fms\\")
}

// ---------------------------------------------------------------------------
// MD5 mesh (id Tech 4)
// ---------------------------------------------------------------------------

pub fn convert_md5mesh(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<ClassicAsset, String> {
    convert_md5mesh_with_bank(bytes, rel, staged, source_id, &Q3TexBank::default())
}

pub fn convert_md5mesh_with_bank(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
) -> Result<ClassicAsset, String> {
    let mesh = md5_to_mesh(bytes)?;
    let kind = classify_md5(rel);
    let folder = match kind {
        AssetKind::Weapon => "weapons",
        AssetKind::Character => "characters",
        _ => "props",
    };
    let slug = path_slug(rel);
    let key = format!("{folder}/{slug}");
    write_mesh_asset(staged, &key, kind, source_id, &["md5mesh", &slug], &mesh, textures)
}

fn classify_md5(rel: &str) -> AssetKind {
    let n = rel.replace('\\', "/").to_ascii_lowercase();
    let character = n.contains("models/md5/chars")
        || n.contains("/chars/")
        || path_has_seg(&n, "player")
        || path_has_seg(&n, "ai")
        || n.contains("/ai_")
        || n.contains("ai/");
    if character {
        return AssetKind::Character;
    }
    if n.contains("weapon") || n.contains("/weapons/") {
        return AssetKind::Weapon;
    }
    AssetKind::Prop
}

fn path_has_seg(path: &str, seg: &str) -> bool {
    path.split('/').any(|p| p == seg || p.starts_with(&format!("{seg}_")))
}

struct MeshPart {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    shader: String,
}

struct ConvertedMesh {
    parts: Vec<MeshPart>,
}

fn md5_to_mesh(bytes: &[u8]) -> Result<ConvertedMesh, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "md5mesh is not UTF-8".to_string())?;
    let toks = lex(text)?;
    let mut p = 0usize;
    expect_ident(&toks, &mut p, "MD5Version")?;
    let ver = expect_int(&toks, &mut p)?;
    if ver != 10 {
        return Err(format!("unsupported MD5Version {ver} (need 10)"));
    }
    // optional commandline "..."
    if ident_eq(&toks, p, "commandline") {
        p += 1;
        let _ = take_string(&toks, &mut p);
    }
    expect_ident(&toks, &mut p, "numJoints")?;
    let num_joints = expect_count(&toks, &mut p, "numJoints")?;
    expect_ident(&toks, &mut p, "numMeshes")?;
    let num_meshes = expect_count(&toks, &mut p, "numMeshes")?;
    if num_joints == 0 || num_meshes == 0 {
        return Err("md5mesh has no joints/meshes".into());
    }
    expect_ident(&toks, &mut p, "joints")?;
    expect_lbrace(&toks, &mut p)?;
    let mut joints = Vec::with_capacity(num_joints);
    for _ in 0..num_joints {
        let _name = expect_string(&toks, &mut p)?;
        let _parent = expect_int(&toks, &mut p)?;
        let pos = expect_paren3(&toks, &mut p)?;
        let qxyz = expect_paren3(&toks, &mut p)?;
        joints.push(JointBind {
            pos,
            quat: quat_from_xyz(qxyz),
        });
    }
    expect_rbrace(&toks, &mut p)?;

    let mut parts = Vec::new();
    for _ in 0..num_meshes {
        expect_ident(&toks, &mut p, "mesh")?;
        expect_lbrace(&toks, &mut p)?;
        let mut mesh_shader = String::new();
        let mut verts: Vec<Md5Vert> = Vec::new();
        let mut tris: Vec<[usize; 3]> = Vec::new();
        let mut weights: Vec<Md5Weight> = Vec::new();
        while p < toks.len() && !matches!(toks[p], Tok::RBrace) {
            if ident_eq(&toks, p, "shader") {
                p += 1;
                mesh_shader = expect_string(&toks, &mut p)?;
                continue;
            }
            if ident_eq(&toks, p, "numverts") {
                p += 1;
                let n = expect_count(&toks, &mut p, "numverts")?;
                verts = vec![Md5Vert::default(); n];
                for _ in 0..n {
                    expect_ident(&toks, &mut p, "vert")?;
                    let idx = expect_int(&toks, &mut p)? as usize;
                    let uv = expect_paren2(&toks, &mut p)?;
                    let start = expect_int(&toks, &mut p)? as usize;
                    let count = expect_int(&toks, &mut p)? as usize;
                    if idx < verts.len() {
                        verts[idx] = Md5Vert { uv, start, count };
                    }
                }
                continue;
            }
            if ident_eq(&toks, p, "numtris") {
                p += 1;
                let n = expect_count(&toks, &mut p, "numtris")?;
                tris.clear();
                for _ in 0..n {
                    expect_ident(&toks, &mut p, "tri")?;
                    let _idx = expect_int(&toks, &mut p)?;
                    let a = expect_int(&toks, &mut p)? as usize;
                    let b = expect_int(&toks, &mut p)? as usize;
                    let c = expect_int(&toks, &mut p)? as usize;
                    tris.push([a, b, c]);
                }
                continue;
            }
            if ident_eq(&toks, p, "numweights") {
                p += 1;
                let n = expect_count(&toks, &mut p, "numweights")?;
                weights = vec![Md5Weight::default(); n];
                for _ in 0..n {
                    expect_ident(&toks, &mut p, "weight")?;
                    let idx = expect_int(&toks, &mut p)? as usize;
                    let joint = expect_int(&toks, &mut p)? as usize;
                    let bias = expect_float(&toks, &mut p)? as f32;
                    let pos = expect_paren3(&toks, &mut p)?;
                    if idx < weights.len() {
                        weights[idx] = Md5Weight { joint, bias, pos };
                    }
                }
                continue;
            }
            return Err(format!("unexpected token in md5 mesh: {}", toks[p].describe()));
        }
        expect_rbrace(&toks, &mut p)?;
        if is_utility_material(&mesh_shader) {
            continue;
        }
        let posed = bind_pose_verts(&verts, &weights, &joints)?;
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        for [a, b, c] in tris {
            if a >= posed.len() || b >= posed.len() || c >= posed.len() {
                continue;
            }
            for vi in [a, b, c] {
                positions.push(xform(posed[vi]));
                // MD5 stores image-space UVs (v=0 at the top of the TGA/DDS),
                // which is already glTF. Flipping here put hair tiles on faces.
                let uv = verts.get(vi).map(|v| v.uv).unwrap_or([0.0, 0.0]);
                uvs.push(uv);
                indices.push(indices.len() as u32);
            }
        }
        if !positions.is_empty() {
            parts.push(MeshPart {
                positions,
                uvs,
                indices,
                shader: mesh_shader,
            });
        }
    }
    if parts.is_empty() {
        return Err("md5mesh produced no triangles".into());
    }
    Ok(ConvertedMesh { parts })
}

#[derive(Clone, Copy, Default)]
struct Md5Vert {
    uv: [f32; 2],
    start: usize,
    count: usize,
}

#[derive(Clone, Copy, Default)]
struct Md5Weight {
    joint: usize,
    bias: f32,
    pos: [f32; 3],
}

#[derive(Clone, Copy)]
struct JointBind {
    pos: [f32; 3],
    quat: [f32; 4],
}

fn bind_pose_verts(
    verts: &[Md5Vert],
    weights: &[Md5Weight],
    joints: &[JointBind],
) -> Result<Vec<[f32; 3]>, String> {
    let mut out = Vec::with_capacity(verts.len());
    for v in verts {
        let mut acc = [0.0f32; 3];
        if v.count == 0 {
            return Err("md5 vert has no weights".into());
        }
        for k in 0..v.count {
            let wi = v.start + k;
            let w = weights.get(wi).ok_or("md5 weight oob")?;
            let j = joints.get(w.joint).ok_or("md5 joint oob")?;
            let rotated = quat_rotate(j.quat, w.pos);
            acc[0] += (j.pos[0] + rotated[0]) * w.bias;
            acc[1] += (j.pos[1] + rotated[1]) * w.bias;
            acc[2] += (j.pos[2] + rotated[2]) * w.bias;
        }
        out.push(acc);
    }
    Ok(out)
}

fn quat_from_xyz(xyz: [f32; 3]) -> [f32; 4] {
    let t = 1.0 - xyz[0] * xyz[0] - xyz[1] * xyz[1] - xyz[2] * xyz[2];
    let w = if t < 0.0 { 0.0 } else { -t.sqrt() };
    [xyz[0], xyz[1], xyz[2], w]
}

fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    [
        v[0] * (1.0 - 2.0 * (yy + zz)) + v[1] * 2.0 * (xy - wz) + v[2] * 2.0 * (xz + wy),
        v[0] * 2.0 * (xy + wz) + v[1] * (1.0 - 2.0 * (xx + zz)) + v[2] * 2.0 * (yz - wx),
        v[0] * 2.0 * (xz - wy) + v[1] * 2.0 * (yz + wx) + v[2] * (1.0 - 2.0 * (xx + yy)),
    ]
}

// ---------------------------------------------------------------------------
// .proc worlds
// ---------------------------------------------------------------------------

pub fn convert_proc(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<Vec<ClassicAsset>, String> {
    convert_proc_with_bank(bytes, rel, staged, source_id, &Q3TexBank::default())
}

pub fn convert_proc_with_bank(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
) -> Result<Vec<ClassicAsset>, String> {
    let (positions, uvs, indices, used) = proc_to_mesh(bytes, textures)?;
    let slug = stem_slug(rel);
    let key = format!("worlds/{slug}");
    let rel_path = format!("{key}.glb");
    let atlas = pack_gray_or_bank(textures, &used);
    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &positions,
        normals: None,
        uvs: &uvs,
        indices: &indices,
        base_color_png: &atlas,
        metallic_roughness_png: None,
        double_sided: false,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return Err("GLB writer failed".into());
    }
    write_bytes(staged, &rel_path, &glb)?;
    let icon_rel = crate::world_preview::write_spawn_preview(&staged.join(&rel_path))
        .ok()
        .map(|_| format!("{key}.png"));
    let mut assets = vec![ClassicAsset {
        key,
        kind: AssetKind::World,
        rel_path,
        tags: vec![
            "world".into(),
            source_id.into(),
            "proc".into(),
            slug,
            "no-portals".into(),
        ],
        icon_rel,
    }];
    let mut seen = BTreeSet::new();
    for name in used {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(img) = textures.get(&name) else {
            continue;
        };
        let Ok(png) = encode_png_rgba(&img.rgba, img.w, img.h) else {
            continue;
        };
        let tslug = sanitize_slug(&name);
        let tkey = format!("textures/{tslug}");
        let trel = format!("{tkey}.png");
        if write_bytes(staged, &trel, &png).is_ok() {
            assets.push(ClassicAsset {
                key: tkey,
                kind: AssetKind::Texture,
                rel_path: trel,
                tags: vec!["texture".into(), source_id.into(), tslug],
                icon_rel: None,
            });
        }
    }
    Ok(assets)
}

fn proc_to_mesh(
    bytes: &[u8],
    textures: &Q3TexBank,
) -> Result<(Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>, Vec<String>), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "proc is not UTF-8".to_string())?;
    let toks = lex(text)?;
    let mut p = 0usize;
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let mut used = Vec::new();
    let mut found_model = false;
    while p < toks.len() {
        if ident_eq(&toks, p, "model") {
            p += 1;
            expect_lbrace(&toks, &mut p)?;
            found_model = true;
            let name = take_string(&toks, &mut p).unwrap_or_default();
            // optional surface count
            if matches!(toks.get(p), Some(Tok::Number(_))) {
                p += 1;
            }
            let mut model_positions = Vec::new();
            let mut model_uvs = Vec::new();
            let mut model_indices = Vec::new();
            while p < toks.len() && !matches!(toks[p], Tok::RBrace) {
                if ident_eq(&toks, p, "surface") {
                    p += 1;
                    // optional surface id before `{`
                    if matches!(toks.get(p), Some(Tok::Number(_))) {
                        p += 1;
                    }
                    expect_lbrace(&toks, &mut p)?;
                    let mat = take_string(&toks, &mut p).unwrap_or_default();
                    let nverts = expect_count(&toks, &mut p, "proc numVerts")?;
                    let nidx = expect_count(&toks, &mut p, "proc numIndexes")?;
                    let mut verts = Vec::with_capacity(nverts);
                    for _ in 0..nverts {
                        verts.push(expect_proc_vert(&toks, &mut p)?);
                    }
                    let mut idxs = Vec::with_capacity(nidx);
                    for _ in 0..nidx {
                        idxs.push(expect_int(&toks, &mut p)? as usize);
                    }
                    expect_rbrace(&toks, &mut p)?;
                    if verts.is_empty() || idxs.len() < 3 {
                        continue;
                    }
                    used.push(mat.clone());
                    let slot = lookup_slot(&mat, textures);
                    for tri in idxs.chunks_exact(3) {
                        if tri[0] >= verts.len() || tri[1] >= verts.len() || tri[2] >= verts.len() {
                            continue;
                        }
                        for &vi in tri {
                            let v = verts[vi];
                            model_positions.push(xform(v.pos));
                            model_uvs.push(slot_uv(slot, v.st[0], 1.0 - v.st[1]));
                            model_indices.push(model_indices.len() as u32);
                        }
                    }
                    continue;
                }
                // skip unknown inner tokens (shadow helpers, leftover numbers)
                p += 1;
            }
            expect_rbrace(&toks, &mut p)?;
            if skip_proc_model(&name, model_indices.is_empty()) {
                continue;
            }
            let base = positions.len() as u32;
            positions.extend(model_positions);
            uvs.extend(model_uvs);
            indices.extend(model_indices.into_iter().map(|i| i + base));
            continue;
        }
        if ident_eq(&toks, p, "shadowModel")
            || ident_eq(&toks, p, "interAreaPortals")
            || ident_eq(&toks, p, "nodes")
        {
            p += 1;
            skip_braced(&toks, &mut p);
            continue;
        }
        p += 1;
    }
    if !found_model {
        return Err("not an id Tech 4 .proc (no model block)".into());
    }
    if positions.is_empty() {
        return Err("proc produced no triangles".into());
    }
    Ok((positions, uvs, indices, used))
}

fn skip_proc_model(name: &str, empty: bool) -> bool {
    if empty {
        return true;
    }
    let n = name.to_ascii_lowercase();
    if n.starts_with("_prelight") {
        return true;
    }
    // Empty `_area` helpers are already caught by `empty`. Named `_area*`
    // with geometry *is* the compiled world — keep it.
    false
}

#[derive(Clone, Copy)]
struct ProcVert {
    pos: [f32; 3],
    st: [f32; 2],
}

fn expect_proc_vert(toks: &[Tok], p: &mut usize) -> Result<ProcVert, String> {
    let paren = matches!(toks.get(*p), Some(Tok::LParen));
    if paren {
        *p += 1;
    }
    let x = expect_float(toks, p)? as f32;
    let y = expect_float(toks, p)? as f32;
    let z = expect_float(toks, p)? as f32;
    let s = expect_float(toks, p)? as f32;
    let t = expect_float(toks, p)? as f32;
    let _nx = expect_float(toks, p)?;
    let _ny = expect_float(toks, p)?;
    let _nz = expect_float(toks, p)?;
    if paren {
        expect_rparen(toks, p)?;
    }
    Ok(ProcVert {
        pos: [x, y, z],
        st: [s, t],
    })
}

fn skip_braced(toks: &[Tok], p: &mut usize) {
    if !matches!(toks.get(*p), Some(Tok::LBrace)) {
        return;
    }
    let mut depth = 0i32;
    while *p < toks.len() {
        match toks[*p] {
            Tok::LBrace => depth += 1,
            Tok::RBrace => {
                depth -= 1;
                *p += 1;
                if depth <= 0 {
                    return;
                }
                continue;
            }
            _ => {}
        }
        *p += 1;
    }
}

// ---------------------------------------------------------------------------
// LWO (optional simple LWO2)
// ---------------------------------------------------------------------------

pub fn convert_lwo(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<Option<ClassicAsset>, String> {
    convert_lwo_with_bank(bytes, rel, staged, source_id, &Q3TexBank::default())
}

pub fn convert_lwo_with_bank(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
) -> Result<Option<ClassicAsset>, String> {
    let Some(mesh) = parse_lwo(bytes) else {
        return Ok(None);
    };
    if mesh.parts.is_empty() {
        return Ok(None);
    }
    let slug = path_slug(rel);
    let key = format!("props/{slug}");
    Ok(Some(write_mesh_asset(
        staged,
        &key,
        AssetKind::Prop,
        source_id,
        &["lwo", &slug],
        &mesh,
        textures,
    )?))
}

fn parse_lwo(bytes: &[u8]) -> Option<ConvertedMesh> {
    if bytes.len() < 12 || &bytes[0..4] != b"FORM" {
        return None;
    }
    let form = &bytes[8..12];
    if form != b"LWO2" && form != b"LWOB" {
        return None;
    }
    let mut off = 12usize;
    let mut points: Vec<[f32; 3]> = Vec::new();
    let mut polys: Vec<Vec<u32>> = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    let mut surf_of_poly: Vec<u16> = Vec::new();
    let mut vmap: Vec<[f32; 2]> = Vec::new();
    let mut vmad: Vec<(u32, u32, [f32; 2])> = Vec::new();
    while off + 8 <= bytes.len() {
        let tag = &bytes[off..off + 4];
        let size = be_u32(bytes, off + 4) as usize;
        off += 8;
        if off + size > bytes.len() {
            break;
        }
        let data = &bytes[off..off + size];
        if tag == b"TAGS" {
            tags = parse_lwo_tags(data);
        } else if tag == b"PNTS" {
            points.clear();
            let mut i = 0;
            while i + 12 <= data.len() {
                let x = be_f32(data, i);
                let y = be_f32(data, i + 4);
                let z = be_f32(data, i + 8);
                points.push(lwo_xform([x, y, z]));
                i += 12;
            }
            vmap = vec![[0.0, 0.0]; points.len()];
        } else if tag == b"POLS" && data.len() >= 4 {
            let kind = &data[0..4];
            if kind == b"FACE" || kind == b"PTCH" {
                let mut i = 4;
                while i + 2 <= data.len() {
                    let nv_raw = be_u16(data, i);
                    i += 2;
                    let nv = (nv_raw & 0x03FF) as usize;
                    let mut idxs = Vec::with_capacity(nv);
                    let mut ok = true;
                    for _ in 0..nv {
                        match read_vx(data, i) {
                            Some((idx, next)) => {
                                idxs.push(idx);
                                i = next;
                            }
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok && idxs.len() >= 3 {
                        polys.push(idxs);
                    } else if !ok {
                        break;
                    }
                }
            }
        } else if tag == b"PTAG" && data.len() >= 4 && &data[0..4] == b"SURF" {
            surf_of_poly = parse_ptag_surf(&data[4..], polys.len());
        } else if tag == b"VMAP" {
            if let Some(uvs) = parse_vmap_txuv(data, points.len()) {
                vmap = uvs;
            }
        } else if tag == b"VMAD" {
            if let Some(disc) = parse_vmad_txuv(data) {
                vmad = disc;
            }
        }
        off += size;
        if size % 2 == 1 {
            off += 1;
        }
    }
    if points.is_empty() || polys.is_empty() {
        return None;
    }
    if vmap.len() < points.len() {
        vmap.resize(points.len(), [0.0, 0.0]);
    }
    let mut vmad_lookup = std::collections::HashMap::new();
    for (vert, poly, uv) in vmad {
        vmad_lookup.insert((poly, vert), uv);
    }
    let mut by_shader: std::collections::BTreeMap<String, MeshPart> = std::collections::BTreeMap::new();
    for (pi, poly) in polys.iter().enumerate() {
        let surf_name = surf_of_poly
            .get(pi)
            .and_then(|&idx| tags.get(idx as usize))
            .cloned()
            .unwrap_or_default();
        if is_utility_material(&surf_name) {
            continue;
        }
        let part = by_shader.entry(surf_name.clone()).or_insert_with(|| MeshPart {
            positions: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            shader: surf_name,
        });
        for i in 1..poly.len().saturating_sub(1) {
            let trio = [poly[0], poly[i], poly[i + 1]];
            if trio.iter().any(|&vi| vi as usize >= points.len()) {
                continue;
            }
            for &vi in &trio {
                part.positions.push(points[vi as usize]);
                let uv = vmad_lookup
                    .get(&(pi as u32, vi))
                    .copied()
                    .unwrap_or_else(|| vmap.get(vi as usize).copied().unwrap_or([0.0, 0.0]));
                part.uvs.push([uv[0], 1.0 - uv[1]]);
                part.indices.push(part.indices.len() as u32);
            }
        }
    }
    let parts: Vec<MeshPart> = by_shader.into_values().filter(|p| !p.positions.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    Some(ConvertedMesh { parts })
}

fn parse_lwo_tags(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        match read_lwo_string(data, i) {
            Some((s, next)) => {
                if !s.is_empty() {
                    out.push(s);
                }
                i = next;
            }
            None => break,
        }
    }
    out
}

fn read_lwo_string(data: &[u8], i: usize) -> Option<(String, usize)> {
    if i >= data.len() {
        return None;
    }
    let end = data[i..].iter().position(|&b| b == 0)?;
    let s = String::from_utf8_lossy(&data[i..i + end]).into_owned();
    let mut next = i + end + 1;
    if (end + 1) % 2 == 1 {
        next += 1;
    }
    Some((s, next))
}

fn parse_ptag_surf(data: &[u8], n_poly: usize) -> Vec<u16> {
    let mut out = vec![0u16; n_poly];
    let mut i = 0usize;
    while i + 2 <= data.len() {
        let Some((poly, after)) = read_vx(data, i) else {
            break;
        };
        if after + 2 > data.len() {
            break;
        }
        let tag = be_u16(data, after);
        if (poly as usize) < out.len() {
            out[poly as usize] = tag;
        }
        i = after + 2;
    }
    out
}

fn parse_vmap_txuv(data: &[u8], n_pts: usize) -> Option<Vec<[f32; 2]>> {
    if data.len() < 6 || &data[0..4] != b"TXUV" {
        return None;
    }
    let dim = be_u16(data, 4) as usize;
    if dim != 2 {
        return None;
    }
    let (_, mut i) = read_lwo_string(data, 6)?;
    let mut uvs = vec![[0.0, 0.0]; n_pts.max(1)];
    while i < data.len() {
        let (idx, after) = read_vx(data, i)?;
        if after + 8 > data.len() {
            break;
        }
        let u = be_f32(data, after);
        let v = be_f32(data, after + 4);
        if (idx as usize) < uvs.len() {
            uvs[idx as usize] = [u, v];
        }
        i = after + 8;
    }
    Some(uvs)
}

fn parse_vmad_txuv(data: &[u8]) -> Option<Vec<(u32, u32, [f32; 2])>> {
    if data.len() < 6 || &data[0..4] != b"TXUV" {
        return None;
    }
    let dim = be_u16(data, 4) as usize;
    if dim != 2 {
        return None;
    }
    let (_, mut i) = read_lwo_string(data, 6)?;
    let mut out = Vec::new();
    while i < data.len() {
        let (vert, a1) = read_vx(data, i)?;
        let (poly, a2) = read_vx(data, a1)?;
        if a2 + 8 > data.len() {
            break;
        }
        let u = be_f32(data, a2);
        let v = be_f32(data, a2 + 4);
        out.push((vert, poly, [u, v]));
        i = a2 + 8;
    }
    Some(out)
}

fn read_vx(data: &[u8], i: usize) -> Option<(u32, usize)> {
    if i >= data.len() {
        return None;
    }
    if data[i] == 0xFF {
        if i + 4 > data.len() {
            return None;
        }
        let idx = ((data[i + 1] as u32) << 16) | ((data[i + 2] as u32) << 8) | data[i + 3] as u32;
        Some((idx, i + 4))
    } else {
        if i + 2 > data.len() {
            return None;
        }
        Some((be_u16(data, i) as u32, i + 2))
    }
}

// ---------------------------------------------------------------------------
// TGA / PNG textures
// ---------------------------------------------------------------------------

pub fn convert_texture(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<Option<ClassicAsset>, String> {
    let decoded = if bytes.starts_with(b"\x89PNG") {
        decode_png(bytes).ok()
    } else {
        decode_tga(bytes).ok()
    };
    let Some((rgba, w, h)) = decoded else {
        return Ok(None);
    };
    if w == 0 || h == 0 {
        return Ok(None);
    }
    let png = encode_png_rgba(&rgba, w, h)?;
    let slug = path_slug(rel);
    let key = format!("textures/{slug}");
    let rel_path = format!("{key}.png");
    write_bytes(staged, &rel_path, &png)?;
    Ok(Some(ClassicAsset {
        key,
        kind: AssetKind::Texture,
        rel_path,
        tags: vec!["texture".into(), source_id.into(), slug],
        icon_rel: None,
    }))
}

fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
    let mut dec = makepad_zune_png::PngDecoder::new(ZCursor::new(bytes));
    dec.decode_headers().map_err(|e| format!("png: {e}"))?;
    let (w, h) = dec.dimensions().ok_or_else(|| "png dimensions".to_string())?;
    let comps = dec.colorspace().map(|c| c.num_components()).unwrap_or(0);
    let raw = dec.decode_raw().map_err(|e| format!("png decode: {e}"))?;
    let n = w as usize * h as usize;
    let rgba = match comps {
        4 if raw.len() >= n * 4 => raw[..n * 4].to_vec(),
        3 if raw.len() >= n * 3 => {
            let mut o = Vec::with_capacity(n * 4);
            for c in raw.chunks_exact(3).take(n) {
                o.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
            o
        }
        1 if raw.len() >= n => {
            let mut o = Vec::with_capacity(n * 4);
            for &c in raw.iter().take(n) {
                o.extend_from_slice(&[c, c, c, 255]);
            }
            o
        }
        _ => return Err("png channels".into()),
    };
    Ok((rgba, w as u32, h as u32))
}

// ---------------------------------------------------------------------------
// Atlas / GLB helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct AtlasSlot {
    uv: [f32; 4],
}

fn lookup_slot(name: &str, textures: &Q3TexBank) -> AtlasSlot {
    if textures.get(name).is_some() {
        // One-texture-per-world fallback: full atlas when we only embed gray
        // or a single image. Multi-material packing uses gray tiles unless
        // the bank has the named image — UVs stay in 0..1 of that tile.
        AtlasSlot { uv: [0.0, 0.0, 1.0, 1.0] }
    } else {
        AtlasSlot { uv: [0.0, 0.0, 1.0, 1.0] }
    }
}

fn slot_uv(slot: AtlasSlot, local_u: f32, local_v: f32) -> [f32; 2] {
    let u = local_u.rem_euclid(1.0);
    let v = local_v.rem_euclid(1.0);
    let du = slot.uv[2] - slot.uv[0];
    let dv = slot.uv[3] - slot.uv[1];
    [slot.uv[0] + du * u, slot.uv[1] + dv * v]
}

const ICON_DIM: usize = 256;

fn is_utility_material(name: &str) -> bool {
    let n = name.replace('\\', "/").to_ascii_lowercase();
    n.is_empty()
        || n == "dkblu"
        || n.contains("nodraw")
        || n.contains("shadow")
        || n.contains("textures/common/")
        || n.contains("textures\\common\\")
}

fn gray_tile() -> crate::quake3_import::Q3Image {
    crate::quake3_import::Q3Image {
        w: 64,
        h: 64,
        rgba: vec![0xB4; 64 * 64 * 4],
    }
}

fn resolve_albedo(textures: &Q3TexBank, shader: &str) -> crate::quake3_import::Q3Image {
    if !is_utility_material(shader) {
        if let Some(img) = textures.resolve(shader) {
            return img;
        }
    }
    gray_tile()
}

/// One atlas tile per unique shader so head/body/prop skins keep their own UVs.
fn pack_shader_atlas(
    textures: &Q3TexBank,
    shaders: &[String],
) -> (crate::quake3_import::Q3Image, Vec<[f32; 4]>) {
    let n = shaders.len().max(1);
    let cols = (n as f32).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    const TILE: u32 = 256;
    const PAD: u32 = 2;
    let cell = TILE + PAD * 2;
    let aw = (cols as u32) * cell;
    let ah = (rows as u32) * cell;
    let mut rgba = vec![0u8; aw as usize * ah as usize * 4];
    let mut slots = Vec::with_capacity(n);
    for (i, shader) in shaders.iter().enumerate() {
        let img = resolve_albedo(textures, shader);
        let col = (i % cols) as u32;
        let row = (i / cols) as u32;
        let ox = col * cell + PAD;
        let oy = row * cell + PAD;
        blit_scaled(&mut rgba, aw, ox, oy, TILE, TILE, &img);
        let du = TILE as f32 / aw as f32;
        let dv = TILE as f32 / ah as f32;
        slots.push([
            ox as f32 / aw as f32,
            oy as f32 / ah as f32,
            ox as f32 / aw as f32 + du,
            oy as f32 / ah as f32 + dv,
        ]);
    }
    if slots.is_empty() {
        slots.push([0.0, 0.0, 1.0, 1.0]);
    }
    (
        crate::quake3_import::Q3Image {
            w: aw,
            h: ah,
            rgba,
        },
        slots,
    )
}

fn blit_scaled(
    dest: &mut [u8],
    dest_w: u32,
    ox: u32,
    oy: u32,
    dw: u32,
    dh: u32,
    src: &crate::quake3_import::Q3Image,
) {
    if src.w == 0 || src.h == 0 {
        return;
    }
    for y in 0..dh {
        let sy = (y * src.h / dh) as usize;
        for x in 0..dw {
            let sx = (x * src.w / dw) as usize;
            let si = (sy * src.w as usize + sx) * 4;
            let di = ((oy + y) as usize * dest_w as usize + (ox + x) as usize) * 4;
            if si + 4 <= src.rgba.len() && di + 4 <= dest.len() {
                dest[di..di + 4].copy_from_slice(&src.rgba[si..si + 4]);
            }
        }
    }
}

fn remap_uv(uv: [f32; 2], slot: [f32; 4]) -> [f32; 2] {
    let u = uv[0].rem_euclid(1.0);
    let v = uv[1].rem_euclid(1.0);
    [
        slot[0] + u * (slot[2] - slot[0]),
        slot[1] + v * (slot[3] - slot[1]),
    ]
}

fn write_mesh_asset(
    staged: &Path,
    key: &str,
    kind: AssetKind,
    source_id: &str,
    extra_tags: &[&str],
    mesh: &ConvertedMesh,
    textures: &Q3TexBank,
) -> Result<ClassicAsset, String> {
    let mut shaders = Vec::new();
    for part in &mesh.parts {
        if !shaders.iter().any(|s| s == &part.shader) {
            shaders.push(part.shader.clone());
        }
    }
    let (img, slots) = pack_shader_atlas(textures, &shaders);
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for part in &mesh.parts {
        let slot_i = shaders.iter().position(|s| s == &part.shader).unwrap_or(0);
        let slot = slots[slot_i.min(slots.len() - 1)];
        let base = positions.len() as u32;
        positions.extend_from_slice(&part.positions);
        for uv in &part.uvs {
            uvs.push(remap_uv(*uv, slot));
        }
        for i in &part.indices {
            indices.push(base + *i);
        }
    }
    let png = encode_png_rgba(&img.rgba, img.w, img.h)?;
    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &positions,
        normals: None,
        uvs: &uvs,
        indices: &indices,
        base_color_png: &png,
        metallic_roughness_png: None,
        double_sided: false,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return Err("GLB writer failed".into());
    }
    let rel_path = format!("{key}.glb");
    write_bytes(staged, &rel_path, &glb)?;
    let yaw = -std::f32::consts::FRAC_PI_2 + 0.35;
    let tile = crate::anim_icon::raster_mesh_icon(
        &positions,
        &indices,
        &uvs,
        &img.rgba,
        img.w as usize,
        img.h as usize,
        yaw,
        ICON_DIM,
    );
    let icon_png = encode_png_rgba(&tile, ICON_DIM as u32, ICON_DIM as u32)?;
    let icon_rel = format!("{key}.png");
    write_bytes(staged, &icon_rel, &icon_png)?;
    let kind_tag = match kind {
        AssetKind::Weapon => "weapon",
        AssetKind::Character => "character",
        _ => "prop",
    };
    let mut tags = vec![kind_tag.into(), source_id.into()];
    for t in extra_tags {
        tags.push((*t).into());
    }
    Ok(ClassicAsset {
        key: key.to_string(),
        kind,
        rel_path,
        tags,
        icon_rel: Some(icon_rel),
    })
}

// ---------------------------------------------------------------------------
// MD5 anim → Doom-style stateful billboard (idle / walk / run / jump / death)
// ---------------------------------------------------------------------------

const BB_DIM: usize = 128;
const BB_ROTS: u8 = 8;
const BB_MAX_ACTORS: usize = 28;

struct Md5Anim {
    #[allow(dead_code)]
    frame_rate: u32,
    hierarchy: Vec<AnimJoint>,
    base_pos: Vec<[f32; 3]>,
    base_quat: Vec<[f32; 4]>,
    frames: Vec<Vec<f32>>,
}

struct AnimJoint {
    parent: i32,
    flags: u32,
    start: usize,
}

/// Raster posed character meshes into `.billboard` actors (walk, death, …).
pub fn assemble_md5_billboards(
    meshes: &[(PathBuf, String)],
    anims: &[(PathBuf, String)],
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
    mut on_progress: impl FnMut(&str),
) -> Vec<ClassicAsset> {
    if meshes.is_empty() || anims.is_empty() {
        return Vec::new();
    }
    let actors = pick_billboard_actors(meshes);
    let classified = classify_anims(anims);
    let mut out = Vec::new();
    for (path, rel) in actors {
        on_progress(&format!("billboard {rel}"));
        match convert_md5_billboard(&path, &rel, &classified, staged, source_id, textures) {
            Ok(Some(asset)) => out.push(asset),
            Ok(None) => {}
            Err(e) => on_progress(&format!("billboard skip {rel}: {e}")),
        }
        if out.len() >= BB_MAX_ACTORS {
            break;
        }
    }
    out
}

fn pick_billboard_actors(meshes: &[(PathBuf, String)]) -> Vec<(PathBuf, String)> {
    let mut by_stem: BTreeMap<String, (PathBuf, String, u8)> = BTreeMap::new();
    for (path, rel) in meshes {
        let n = rel.replace('\\', "/").to_ascii_lowercase();
        if !n.contains("models/md5/chars/") {
            continue;
        }
        if n.contains("/heads/") || n.contains("_view") || n.contains("/weapons/") {
            continue;
        }
        if classify_md5(rel) != AssetKind::Character {
            continue;
        }
        let lod = if n.contains("_lowest") {
            3
        } else if n.contains("_low") {
            2
        } else if n.contains("_med") {
            1
        } else {
            0
        };
        let stem = n
            .trim_end_matches(".md5mesh")
            .trim_end_matches("_lowest")
            .trim_end_matches("_low")
            .trim_end_matches("_med")
            .to_string();
        match by_stem.get(&stem) {
            Some((_, _, have)) if *have <= lod => {}
            _ => {
                by_stem.insert(stem, (path.clone(), rel.clone(), lod));
            }
        }
    }
    let mut list: Vec<(PathBuf, String, u8)> = by_stem.into_values().collect();
    list.sort_by(|a, b| {
        let score = |rel: &str| -> i32 {
            let n = rel.to_ascii_lowercase();
            let mut s = 0i32;
            if n.contains("monster") || n.contains("undead") || n.contains("animal") {
                s -= 4;
            }
            if n.contains("steambot") || n.contains("spider") {
                s -= 3;
            }
            if n.contains("guard") || n.contains("thief") {
                s -= 1;
            }
            s
        };
        score(&a.1).cmp(&score(&b.1)).then(a.1.cmp(&b.1))
    });
    list.into_iter().map(|(p, r, _)| (p, r)).collect()
}

fn classify_anims(anims: &[(PathBuf, String)]) -> Vec<(PathBuf, String, &'static str)> {
    let mut out = Vec::new();
    for (path, rel) in anims {
        if let Some(state) = anim_state_name(rel) {
            out.push((path.clone(), rel.clone(), state));
        }
    }
    out
}

fn anim_state_name(rel: &str) -> Option<&'static str> {
    let stem = rel
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(rel)
        .trim_end_matches(".md5anim")
        .to_ascii_lowercase();
    if stem.starts_with("head_") || stem.contains("sit") || stem.contains("sleep") {
        return None;
    }
    let exact = [
        ("idle", "idle"),
        ("walk", "walk"),
        ("run", "run"),
        ("jump", "jump"),
        ("death", "death"),
        ("die", "death"),
        ("pain", "pain"),
        ("attack", "attack"),
    ];
    for (key, state) in exact {
        if stem == key || stem.ends_with(&format!("_{key}")) {
            return Some(state);
        }
    }
    if stem.contains("walkcycle") || stem == "walk1" {
        return Some("walk");
    }
    if stem.contains("meleeattack") || stem.contains("attack_") {
        return Some("attack");
    }
    if stem.contains("pain") {
        return Some("pain");
    }
    if stem.contains("death") || stem.contains("_die") {
        return Some("death");
    }
    if stem.contains("jump") {
        return Some("jump");
    }
    None
}

fn pick_anim_for<'a>(
    classified: &'a [(PathBuf, String, &'static str)],
    mesh_rel: &str,
    state: &'static str,
) -> Option<&'a PathBuf> {
    let mesh_n = mesh_rel.replace('\\', "/").to_ascii_lowercase();
    let mesh_dir = mesh_n.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut best: Option<(&PathBuf, i32)> = None;
    for (path, rel, st) in classified {
        if *st != state {
            continue;
        }
        let n = rel.replace('\\', "/").to_ascii_lowercase();
        let dir = n.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let stem = n.rsplit('/').next().unwrap_or("");
        let mut score;
        if dir == mesh_dir {
            score = 0;
        } else if n.contains("/animations/short/") {
            score = 10;
        } else if n.contains("/animations/") {
            score = 20;
        } else if mesh_dir.split('/').any(|p| dir.contains(p) && p.len() > 4) {
            score = 5;
        } else {
            continue;
        }
        if stem == format!("{state}.md5anim") {
            score -= 2;
        }
        if stem.contains("torch") || stem.contains("sword") || stem.contains("bow") {
            score += 4;
        }
        match best {
            Some((_, s)) if s <= score => {}
            _ => best = Some((path, score)),
        }
    }
    best.map(|(p, _)| p)
}

fn convert_md5_billboard(
    mesh_path: &Path,
    mesh_rel: &str,
    classified: &[(PathBuf, String, &'static str)],
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
) -> Result<Option<ClassicAsset>, String> {
    let bytes = std::fs::read(mesh_path).map_err(|e| e.to_string())?;
    let rig = parse_md5_rig(&bytes)?;
    if rig.surfaces.is_empty() {
        return Ok(None);
    }
    let wanted = ["idle", "walk", "run", "jump", "attack", "pain", "death"];
    let mut clips: Vec<(&str, Md5Anim)> = Vec::new();
    for state in wanted {
        let Some(path) = pick_anim_for(classified, mesh_rel, state) else {
            continue;
        };
        let raw = std::fs::read(path).map_err(|e| e.to_string())?;
        let Ok(anim) = parse_md5anim(&raw) else {
            continue;
        };
        if anim.hierarchy.len() != rig.joints.len() {
            continue;
        }
        clips.push((state, anim));
    }
    if clips.is_empty() {
        return Ok(None);
    }
    let (atlas, slots, shaders) = rig_atlas(&rig, textures);
    let slug = path_slug(mesh_rel);
    let key = format!("billboards/{slug}");
    let manifest_dir = key.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    let mut frames = Vec::new();
    let mut states = Vec::new();
    let letters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut letter_i = 0usize;
    for (state, anim) in &clips {
        let n_take = match *state {
            "idle" => 2,
            "walk" | "run" => 4,
            "jump" | "attack" | "death" => 4,
            "pain" => 2,
            _ => 3,
        };
        let idxs = sample_frame_indices(anim.frames.len(), n_take);
        let first = frames.len();
        for &fi in &idxs {
            let joints = anim.world_joints(fi);
            let posed = pose_rig(&rig, &joints)?;
            let (positions, uvs, indices) = flatten_posed(&posed, &shaders, &slots);
            let letter = letters[letter_i.min(letters.len() - 1)] as char;
            letter_i += 1;
            for rot in 1u8..=BB_ROTS {
                let yaw = -std::f32::consts::FRAC_PI_2 + (rot as f32 - 1.0) * std::f32::consts::FRAC_PI_4;
                let tile = crate::anim_icon::raster_mesh_icon_clear(
                    &positions,
                    &indices,
                    &uvs,
                    &atlas.rgba,
                    atlas.w as usize,
                    atlas.h as usize,
                    yaw,
                    BB_DIM,
                    [0, 0, 0, 0],
                );
                let staged_rel = format!("{key}_{}_{rot}.png", letter.to_ascii_lowercase());
                let png = encode_png_rgba(&tile, BB_DIM as u32, BB_DIM as u32)?;
                write_bytes(staged, &staged_rel, &png)?;
                // `file` is relative to the MANIFEST, not to staged: the
                // manifest lands beside these frames in `billboards/`.
                let file = staged_rel
                    .strip_prefix(&format!("{manifest_dir}/"))
                    .unwrap_or(&staged_rel)
                    .to_string();
                frames.push(crate::stateful_billboard::SpriteFrame {
                    letter,
                    rot,
                    w: BB_DIM as u32,
                    h: BB_DIM as u32,
                    file,
                    flip: false,
                    cell: None,
                });
            }
        }
        let last = frames.len();
        if last > first {
            states.push(crate::stateful_billboard::AnimState {
                name: (*state).into(),
                first,
                last,
                r#loop: matches!(*state, "idle" | "walk" | "run"),
                fps: match *state {
                    "walk" | "run" => 10,
                    "attack" => 12,
                    "death" => 6,
                    _ => 8,
                },
            });
        }
    }
    if frames.is_empty() {
        return Ok(None);
    }
    let preview = states
        .iter()
        .find(|s| s.name == "walk" || s.name == "idle")
        .or_else(|| states.first())
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "idle".into());
    let facings = frames.iter().map(|f| f.rot).max().unwrap_or(1);
    let mut bb = crate::stateful_billboard::StatefulBillboard {
        prefix: slug.clone(),
        role: crate::stateful_billboard::SpriteRole::Character,
        preview: preview.clone(),
        facings,
        mirrors: if facings >= 5 { 8 } else { 0 },
        states,
        frames,
        sheet: None,
        actor: None,
        weapon: None,
        metres_per_pixel: 0.0,
    };
    let rel_path = format!("{key}.billboard");
    // One packed sheet per actor, never one PNG per rendered pose.
    let manifest = staged.join(&rel_path);
    if let Some(parent) = manifest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let written = crate::billboard_sheet::write_with_sheet(&manifest, &mut bb)
        .map_err(|e| format!("{rel_path}: {e}"))?;
    for frame in &written.consumed {
        let _ = std::fs::remove_file(frame);
    }
    let icon_rel = written
        .thumb
        .as_deref()
        .unwrap_or(&written.sheet)
        .strip_prefix(staged)
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"));
    Ok(Some(ClassicAsset {
        key,
        kind: AssetKind::Billboard,
        rel_path,
        tags: vec![
            "character".into(),
            "billboard".into(),
            source_id.into(),
            slug,
            "md5anim".into(),
        ],
        icon_rel,
    }))
}

struct Md5Rig {
    joints: Vec<Md5Joint>,
    surfaces: Vec<Md5Surface>,
}

struct Md5Joint {
    #[allow(dead_code)]
    parent: i32,
    #[allow(dead_code)]
    pos: [f32; 3],
    #[allow(dead_code)]
    quat: [f32; 4],
}

struct Md5Surface {
    shader: String,
    verts: Vec<Md5Vert>,
    tris: Vec<[usize; 3]>,
    weights: Vec<Md5Weight>,
}

fn parse_md5_rig(bytes: &[u8]) -> Result<Md5Rig, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "md5mesh is not UTF-8".to_string())?;
    let toks = lex(text)?;
    let mut p = 0usize;
    expect_ident(&toks, &mut p, "MD5Version")?;
    let ver = expect_int(&toks, &mut p)?;
    if ver != 10 {
        return Err(format!("unsupported MD5Version {ver}"));
    }
    if ident_eq(&toks, p, "commandline") {
        p += 1;
        let _ = take_string(&toks, &mut p);
    }
    expect_ident(&toks, &mut p, "numJoints")?;
    let num_joints = expect_count(&toks, &mut p, "numJoints")?;
    expect_ident(&toks, &mut p, "numMeshes")?;
    let num_meshes = expect_count(&toks, &mut p, "numMeshes")?;
    expect_ident(&toks, &mut p, "joints")?;
    expect_lbrace(&toks, &mut p)?;
    let mut joints = Vec::with_capacity(num_joints);
    for _ in 0..num_joints {
        let _name = expect_string(&toks, &mut p)?;
        let parent = expect_int(&toks, &mut p)? as i32;
        let pos = expect_paren3(&toks, &mut p)?;
        let qxyz = expect_paren3(&toks, &mut p)?;
        joints.push(Md5Joint {
            parent,
            pos,
            quat: quat_from_xyz(qxyz),
        });
    }
    expect_rbrace(&toks, &mut p)?;
    let mut surfaces = Vec::new();
    for _ in 0..num_meshes {
        expect_ident(&toks, &mut p, "mesh")?;
        expect_lbrace(&toks, &mut p)?;
        let mut shader = String::new();
        let mut verts = Vec::new();
        let mut tris = Vec::new();
        let mut weights = Vec::new();
        while p < toks.len() && !matches!(toks[p], Tok::RBrace) {
            if ident_eq(&toks, p, "shader") {
                p += 1;
                shader = expect_string(&toks, &mut p)?;
                continue;
            }
            if ident_eq(&toks, p, "numverts") {
                p += 1;
                let n = expect_count(&toks, &mut p, "numverts")?;
                verts = vec![Md5Vert::default(); n];
                for _ in 0..n {
                    expect_ident(&toks, &mut p, "vert")?;
                    let idx = expect_int(&toks, &mut p)? as usize;
                    let uv = expect_paren2(&toks, &mut p)?;
                    let start = expect_int(&toks, &mut p)? as usize;
                    let count = expect_int(&toks, &mut p)? as usize;
                    if idx < verts.len() {
                        verts[idx] = Md5Vert { uv, start, count };
                    }
                }
                continue;
            }
            if ident_eq(&toks, p, "numtris") {
                p += 1;
                let n = expect_count(&toks, &mut p, "numtris")?;
                tris.clear();
                for _ in 0..n {
                    expect_ident(&toks, &mut p, "tri")?;
                    let _idx = expect_int(&toks, &mut p)?;
                    let a = expect_int(&toks, &mut p)? as usize;
                    let b = expect_int(&toks, &mut p)? as usize;
                    let c = expect_int(&toks, &mut p)? as usize;
                    tris.push([a, b, c]);
                }
                continue;
            }
            if ident_eq(&toks, p, "numweights") {
                p += 1;
                let n = expect_count(&toks, &mut p, "numweights")?;
                weights = vec![Md5Weight::default(); n];
                for _ in 0..n {
                    expect_ident(&toks, &mut p, "weight")?;
                    let idx = expect_int(&toks, &mut p)? as usize;
                    let joint = expect_int(&toks, &mut p)? as usize;
                    let bias = expect_float(&toks, &mut p)? as f32;
                    let pos = expect_paren3(&toks, &mut p)?;
                    if idx < weights.len() {
                        weights[idx] = Md5Weight { joint, bias, pos };
                    }
                }
                continue;
            }
            return Err(format!("unexpected token in md5 mesh: {}", toks[p].describe()));
        }
        expect_rbrace(&toks, &mut p)?;
        if is_utility_material(&shader) || verts.is_empty() {
            continue;
        }
        surfaces.push(Md5Surface {
            shader,
            verts,
            tris,
            weights,
        });
    }
    Ok(Md5Rig { joints, surfaces })
}

fn parse_md5anim(bytes: &[u8]) -> Result<Md5Anim, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "md5anim is not UTF-8".to_string())?;
    let toks = lex(text)?;
    let mut p = 0usize;
    expect_ident(&toks, &mut p, "MD5Version")?;
    let ver = expect_int(&toks, &mut p)?;
    if ver != 10 {
        return Err(format!("unsupported MD5Version {ver}"));
    }
    if ident_eq(&toks, p, "commandline") {
        p += 1;
        let _ = take_string(&toks, &mut p);
    }
    expect_ident(&toks, &mut p, "numFrames")?;
    let num_frames = expect_count(&toks, &mut p, "numFrames")?;
    expect_ident(&toks, &mut p, "numJoints")?;
    let num_joints = expect_count(&toks, &mut p, "numJoints")?;
    expect_ident(&toks, &mut p, "frameRate")?;
    let frame_rate = expect_int(&toks, &mut p)? as u32;
    expect_ident(&toks, &mut p, "numAnimatedComponents")?;
    let num_comp = expect_count(&toks, &mut p, "numAnimatedComponents")?;
    expect_ident(&toks, &mut p, "hierarchy")?;
    expect_lbrace(&toks, &mut p)?;
    let mut hierarchy = Vec::with_capacity(num_joints);
    for _ in 0..num_joints {
        let _name = expect_string(&toks, &mut p)?;
        let parent = expect_int(&toks, &mut p)? as i32;
        let flags = expect_int(&toks, &mut p)? as u32;
        let start = expect_int(&toks, &mut p)? as usize;
        hierarchy.push(AnimJoint {
            parent,
            flags,
            start,
        });
    }
    expect_rbrace(&toks, &mut p)?;
    if ident_eq(&toks, p, "bounds") {
        p += 1;
        expect_lbrace(&toks, &mut p)?;
        while p < toks.len() && !matches!(toks[p], Tok::RBrace) {
            p += 1;
        }
        expect_rbrace(&toks, &mut p)?;
    }
    expect_ident(&toks, &mut p, "baseframe")?;
    expect_lbrace(&toks, &mut p)?;
    let mut base_pos = Vec::with_capacity(num_joints);
    let mut base_quat = Vec::with_capacity(num_joints);
    for _ in 0..num_joints {
        let pos = expect_paren3(&toks, &mut p)?;
        let qxyz = expect_paren3(&toks, &mut p)?;
        base_pos.push(pos);
        base_quat.push(quat_from_xyz(qxyz));
    }
    expect_rbrace(&toks, &mut p)?;
    let mut frames = Vec::with_capacity(num_frames);
    for _ in 0..num_frames {
        expect_ident(&toks, &mut p, "frame")?;
        let _idx = expect_int(&toks, &mut p)?;
        expect_lbrace(&toks, &mut p)?;
        let mut comp = Vec::with_capacity(num_comp);
        while p < toks.len() && !matches!(toks[p], Tok::RBrace) {
            comp.push(expect_float(&toks, &mut p)? as f32);
        }
        expect_rbrace(&toks, &mut p)?;
        frames.push(comp);
    }
    Ok(Md5Anim {
        frame_rate,
        hierarchy,
        base_pos,
        base_quat,
        frames,
    })
}

impl Md5Anim {
    fn world_joints(&self, frame: usize) -> Vec<JointBind> {
        let comps = self.frames.get(frame).map(|c| c.as_slice()).unwrap_or(&[]);
        let mut locals: Vec<JointBind> = Vec::with_capacity(self.hierarchy.len());
        for (i, h) in self.hierarchy.iter().enumerate() {
            let mut pos = self.base_pos.get(i).copied().unwrap_or([0.0; 3]);
            let mut qxyz = [
                self.base_quat.get(i).map(|q| q[0]).unwrap_or(0.0),
                self.base_quat.get(i).map(|q| q[1]).unwrap_or(0.0),
                self.base_quat.get(i).map(|q| q[2]).unwrap_or(0.0),
            ];
            let mut c = h.start;
            if h.flags & 1 != 0 {
                if let Some(v) = comps.get(c) {
                    pos[0] = *v;
                }
                c += 1;
            }
            if h.flags & 2 != 0 {
                if let Some(v) = comps.get(c) {
                    pos[1] = *v;
                }
                c += 1;
            }
            if h.flags & 4 != 0 {
                if let Some(v) = comps.get(c) {
                    pos[2] = *v;
                }
                c += 1;
            }
            if h.flags & 8 != 0 {
                if let Some(v) = comps.get(c) {
                    qxyz[0] = *v;
                }
                c += 1;
            }
            if h.flags & 16 != 0 {
                if let Some(v) = comps.get(c) {
                    qxyz[1] = *v;
                }
                c += 1;
            }
            if h.flags & 32 != 0 {
                if let Some(v) = comps.get(c) {
                    qxyz[2] = *v;
                }
            }
            locals.push(JointBind {
                pos,
                quat: quat_from_xyz(qxyz),
            });
        }
        let mut world = Vec::with_capacity(locals.len());
        for (i, h) in self.hierarchy.iter().enumerate() {
            if h.parent < 0 {
                world.push(locals[i]);
                continue;
            }
            let parent = world.get(h.parent as usize).copied().unwrap_or(locals[i]);
            let rotated = quat_rotate(parent.quat, locals[i].pos);
            world.push(JointBind {
                pos: [
                    parent.pos[0] + rotated[0],
                    parent.pos[1] + rotated[1],
                    parent.pos[2] + rotated[2],
                ],
                quat: quat_mul(parent.quat, locals[i].quat),
            });
        }
        world
    }
}

fn pose_rig(rig: &Md5Rig, joints: &[JointBind]) -> Result<Vec<MeshPart>, String> {
    let mut parts = Vec::new();
    for surf in &rig.surfaces {
        let posed = bind_pose_verts(&surf.verts, &surf.weights, joints)?;
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        for [a, b, c] in &surf.tris {
            if *a >= posed.len() || *b >= posed.len() || *c >= posed.len() {
                continue;
            }
            for vi in [*a, *b, *c] {
                positions.push(xform(posed[vi]));
                uvs.push(surf.verts.get(vi).map(|v| v.uv).unwrap_or([0.0, 0.0]));
                indices.push(indices.len() as u32);
            }
        }
        if !positions.is_empty() {
            parts.push(MeshPart {
                positions,
                uvs,
                indices,
                shader: surf.shader.clone(),
            });
        }
    }
    Ok(parts)
}

fn rig_atlas(
    rig: &Md5Rig,
    textures: &Q3TexBank,
) -> (crate::quake3_import::Q3Image, Vec<[f32; 4]>, Vec<String>) {
    let mut shaders = Vec::new();
    for s in &rig.surfaces {
        if !shaders.iter().any(|x| x == &s.shader) {
            shaders.push(s.shader.clone());
        }
    }
    let (img, slots) = pack_shader_atlas(textures, &shaders);
    (img, slots, shaders)
}

fn flatten_posed(
    parts: &[MeshPart],
    shaders: &[String],
    slots: &[[f32; 4]],
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for part in parts {
        let slot_i = shaders.iter().position(|s| s == &part.shader).unwrap_or(0);
        let slot = slots[slot_i.min(slots.len().saturating_sub(1))];
        let base = positions.len() as u32;
        positions.extend_from_slice(&part.positions);
        for uv in &part.uvs {
            uvs.push(remap_uv(*uv, slot));
        }
        for i in &part.indices {
            indices.push(base + *i);
        }
    }
    (positions, uvs, indices)
}

fn sample_frame_indices(n: usize, take: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let take = take.clamp(1, n);
    if take == 1 {
        return vec![0];
    }
    (0..take).map(|i| i * (n - 1) / (take - 1)).collect()
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// Parse id Tech 4 `.mtr` files and alias material names to their diffusemap.
pub fn apply_mtr_aliases(bank: &mut Q3TexBank, root: &Path) {
    if !root.exists() {
        return;
    }
    let mut files = Vec::new();
    walk_mtr(root, &mut files, 0);
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (mat, map) in parse_mtr_aliases(&text) {
            bank.add_alias(&mat, &map);
        }
    }
}

fn walk_mtr(dir: &Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    if depth > 24 || out.len() > 4096 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk_mtr(&path, out, depth + 1);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("mtr"))
        {
            out.push(path);
        }
    }
}

fn parse_mtr_aliases(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut captured = false;
    let mut depth = 0i32;
    for raw in src.lines() {
        let line = strip_mtr_comment(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let opens = trimmed.chars().filter(|&c| c == '{').count() as i32;
        let closes = trimmed.chars().filter(|&c| c == '}').count() as i32;
        if depth == 0 {
            let head = trimmed.trim_end_matches('{').trim();
            if !head.is_empty() {
                current = Some(head.to_string());
                captured = false;
            }
        } else if !captured {
            if let Some(map) = mtr_diffusemap(trimmed) {
                if let Some(mat) = current.clone() {
                    out.push((mat, map));
                    captured = true;
                }
            }
        }
        depth += opens - closes;
        if depth < 0 {
            depth = 0;
        }
        if depth == 0 && opens == 0 {
            // closed (or never opened) — next identifier starts a new material
            if closes > 0 {
                current = None;
                captured = false;
            }
        }
    }
    out
}

fn strip_mtr_comment(line: &str) -> String {
    if let Some(i) = line.find("//") {
        line[..i].to_string()
    } else {
        line.to_string()
    }
}

fn mtr_diffusemap(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    // Match only at the start of the token. TDM often glues the path on:
    // `diffusemapmodels/...`, `blenddiffusemap` / `mapmodels/...`.
    // Do not match `diffusemap` inside `blenddiffusemap` as an empty path.
    if let Some(rest) = strip_mtr_kw(trimmed, &lower, "diffusemap") {
        return clean_mtr_path(rest);
    }
    if lower == "blenddiffusemap" || lower.starts_with("blenddiffusemap ") {
        return None;
    }
    if lower.starts_with("blend ") && lower.contains("diffusemap") {
        return None;
    }
    if let Some(rest) = strip_mtr_kw(trimmed, &lower, "map") {
        if rest.is_empty()
            || rest.eq_ignore_ascii_case("addnormals")
            || rest.to_ascii_lowercase().starts_with("addnormals")
        {
            return None;
        }
        return clean_mtr_path(rest);
    }
    None
}

/// Keyword at the start of `trimmed`. Allows a glued path (`mapmodels/...`).
fn strip_mtr_kw<'a>(trimmed: &'a str, lower: &str, kw: &str) -> Option<&'a str> {
    if !lower.starts_with(kw) {
        return None;
    }
    let rest = &trimmed[kw.len()..];
    if rest.is_empty() {
        return None;
    }
    Some(rest)
}

fn clean_mtr_path(raw: &str) -> Option<String> {
    let s = raw
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '{' || c == '}');
    if s.is_empty() || s.starts_with("addnormals") || s.starts_with("heightmap") {
        return None;
    }
    let s = s.split_whitespace().next().unwrap_or(s);
    if s.is_empty() || s == "*" {
        return None;
    }
    Some(s.replace('\\', "/"))
}

fn pack_gray_or_bank(textures: &Q3TexBank, used: &[String]) -> Vec<u8> {
    for name in used {
        if is_utility_material(name) {
            continue;
        }
        if let Some(img) = textures.resolve(name) {
            if let Ok(png) = encode_png_rgba(&img.rgba, img.w, img.h) {
                return png;
            }
        }
    }
    gray_png(64, 64)
}

fn gray_png(w: u32, h: u32) -> Vec<u8> {
    encode_png_rgba(&vec![0x80; (w as usize) * (h as usize) * 4], w, h)
        .unwrap_or_else(|_| b"\x89PNG\r\n\x1a\n".to_vec())
}

#[allow(dead_code)]
fn write_gray_glb(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Result<Vec<u8>, String> {
    let png = gray_png(64, 64);
    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions,
        normals: None,
        uvs,
        indices,
        base_color_png: &png,
        metallic_roughness_png: None,
        double_sided: false,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return Err("GLB writer failed".into());
    }
    Ok(glb)
}

pub(crate) fn xform(p: [f32; 3]) -> [f32; 3] {
    // id Tech 4 is Z-up; glTF / engine is Y-up.
    [p[0] * SCALE, p[2] * SCALE, -p[1] * SCALE]
}

fn lwo_xform(p: [f32; 3]) -> [f32; 3] {
    // LightWave LWO2 is Y-up already — do not apply the Z-up id Tech swap
    // or props lie on their side while MD5 characters stand.
    [p[0] * SCALE, p[1] * SCALE, -p[2] * SCALE]
}

fn write_bytes(staged: &Path, rel: &str, bytes: &[u8]) -> Result<(), String> {
    let dest = staged.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, bytes).map_err(|e| e.to_string())
}

fn stem_slug(rel: &str) -> String {
    let name = rel.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or(&name);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    sanitize_slug(stem)
}

fn path_slug(rel: &str) -> String {
    let n = rel.replace('\\', "/");
    let n = n.rsplit_once('.').map(|(s, _)| s).unwrap_or(&n);
    let parts: Vec<&str> = n.split('/').filter(|s| !s.is_empty()).collect();
    let take = if parts.len() >= 2 {
        &parts[parts.len() - 2..]
    } else {
        &parts[..]
    };
    sanitize_slug(&take.join("-"))
}

fn sanitize_slug(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "asset".into()
    } else {
        out
    }
}

fn be_u16(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        return 0;
    }
    u16::from_be_bytes([b[o], b[o + 1]])
}

fn be_u32(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        return 0;
    }
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn be_f32(b: &[u8], o: usize) -> f32 {
    f32::from_bits(be_u32(b, o))
}

// ---------------------------------------------------------------------------
// Lexer (MD5 + .proc text)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Tok {
    Ident(String),
    String(String),
    Number(f64),
    LBrace,
    RBrace,
    LParen,
    RParen,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Ident(s) | Tok::String(s) => s.clone(),
            Tok::Number(n) => format!("{n}"),
            Tok::LBrace => "{".into(),
            Tok::RBrace => "}".into(),
            Tok::LParen => "(".into(),
            Tok::RParen => ")".into(),
        }
    }
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = i.saturating_add(2);
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match c {
            b'{' => {
                out.push(Tok::LBrace);
                i += 1;
            }
            b'}' => {
                out.push(Tok::RBrace);
                i += 1;
            }
            b'(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            b'"' => {
                i += 1;
                let start = i;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
                let s = String::from_utf8_lossy(&b[start..i]).into_owned();
                if i < b.len() {
                    i += 1;
                }
                out.push(Tok::String(s));
            }
            _ if c == b'-' || c == b'+' || c.is_ascii_digit() || c == b'.' => {
                let start = i;
                i += 1;
                while i < b.len()
                    && (b[i].is_ascii_digit()
                        || b[i] == b'.'
                        || b[i] == b'e'
                        || b[i] == b'E'
                        || b[i] == b'+'
                        || b[i] == b'-')
                {
                    // keep sign only right after e/E
                    if (b[i] == b'+' || b[i] == b'-') && b[i - 1] != b'e' && b[i - 1] != b'E' {
                        break;
                    }
                    i += 1;
                }
                let raw = std::str::from_utf8(&b[start..i]).unwrap_or("");
                let n = raw
                    .parse::<f64>()
                    .map_err(|_| format!("bad number {raw}"))?;
                out.push(Tok::Number(n));
            }
            _ if is_ident_start(c) => {
                let start = i;
                i += 1;
                while i < b.len() && is_ident_cont(b[i]) {
                    i += 1;
                }
                out.push(Tok::Ident(
                    String::from_utf8_lossy(&b[start..i]).into_owned(),
                ));
            }
            _ => {
                // skip stray punctuation
                i += 1;
            }
        }
    }
    Ok(out)
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'/' || c == b'\\'
}

fn is_ident_cont(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit() || c == b'.' || c == b'-' || c == b'+'
}

fn ident_eq(toks: &[Tok], p: usize, name: &str) -> bool {
    matches!(toks.get(p), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(name))
}

fn expect_ident(toks: &[Tok], p: &mut usize, name: &str) -> Result<(), String> {
    if ident_eq(toks, *p, name) {
        *p += 1;
        Ok(())
    } else {
        Err(format!(
            "expected {name}, got {}",
            toks.get(*p).map(|t| t.describe()).unwrap_or_else(|| "eof".into())
        ))
    }
}

fn expect_string(toks: &[Tok], p: &mut usize) -> Result<String, String> {
    match toks.get(*p) {
        Some(Tok::String(s)) | Some(Tok::Ident(s)) => {
            *p += 1;
            Ok(s.clone())
        }
        other => Err(format!(
            "expected string, got {}",
            other.map(|t| t.describe()).unwrap_or_else(|| "eof".into())
        )),
    }
}

fn take_string(toks: &[Tok], p: &mut usize) -> Option<String> {
    match toks.get(*p) {
        Some(Tok::String(s)) | Some(Tok::Ident(s)) => {
            *p += 1;
            Some(s.clone())
        }
        _ => None,
    }
}

fn expect_float(toks: &[Tok], p: &mut usize) -> Result<f64, String> {
    match toks.get(*p) {
        Some(Tok::Number(n)) => {
            *p += 1;
            Ok(*n)
        }
        other => Err(format!(
            "expected number, got {}",
            other.map(|t| t.describe()).unwrap_or_else(|| "eof".into())
        )),
    }
}

fn expect_int(toks: &[Tok], p: &mut usize) -> Result<i64, String> {
    Ok(expect_float(toks, p)? as i64)
}

/// Count field feeding an allocation: reject negatives and absurd sizes.
fn expect_count(toks: &[Tok], p: &mut usize, what: &str) -> Result<usize, String> {
    let n = expect_int(toks, p)?;
    if !(0..=1_000_000).contains(&n) {
        return Err(format!("{what} {n} out of range"));
    }
    Ok(n as usize)
}

fn expect_lbrace(toks: &[Tok], p: &mut usize) -> Result<(), String> {
    if matches!(toks.get(*p), Some(Tok::LBrace)) {
        *p += 1;
        Ok(())
    } else {
        Err("expected {".into())
    }
}

fn expect_rbrace(toks: &[Tok], p: &mut usize) -> Result<(), String> {
    if matches!(toks.get(*p), Some(Tok::RBrace)) {
        *p += 1;
        Ok(())
    } else {
        Err("expected }".into())
    }
}

fn expect_rparen(toks: &[Tok], p: &mut usize) -> Result<(), String> {
    if matches!(toks.get(*p), Some(Tok::RParen)) {
        *p += 1;
        Ok(())
    } else {
        Err("expected )".into())
    }
}

fn expect_paren3(toks: &[Tok], p: &mut usize) -> Result<[f32; 3], String> {
    if !matches!(toks.get(*p), Some(Tok::LParen)) {
        return Err("expected (vec3)".into());
    }
    *p += 1;
    let x = expect_float(toks, p)? as f32;
    let y = expect_float(toks, p)? as f32;
    let z = expect_float(toks, p)? as f32;
    expect_rparen(toks, p)?;
    Ok([x, y, z])
}

fn expect_paren2(toks: &[Tok], p: &mut usize) -> Result<[f32; 2], String> {
    if !matches!(toks.get(*p), Some(Tok::LParen)) {
        return Err("expected (uv)".into());
    }
    *p += 1;
    let u = expect_float(toks, p)? as f32;
    let v = expect_float(toks, p)? as f32;
    expect_rparen(toks, p)?;
    Ok([u, v])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_zip_file::{ZipMethod, ZipWriter};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "d3imp-{name}-{nanos}-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    fn minimal_md5() -> String {
        r#"MD5Version 10
commandline ""

numJoints 1
numMeshes 1

joints {
	"origin"	-1 ( 0 0 0 ) ( 0 0 0 )
}

mesh {
	shader "textures/test"

	numverts 3
	vert 0 ( 0.0 0.0 ) 0 1
	vert 1 ( 1.0 0.0 ) 1 1
	vert 2 ( 0.0 1.0 ) 2 1

	numtris 1
	tri 0 0 1 2

	numweights 3
	weight 0 0 1.0 ( 0 0 0 )
	weight 1 0 1.0 ( 32 0 0 )
	weight 2 0 1.0 ( 0 32 0 )
}
"#
        .into()
    }

    fn minimal_proc() -> String {
        r#"mapProcFile003

model { /* name */ "test"
/* numSurfaces */ 1
surface { /* material */ "textures/test" /* numVerts */ 3 /* numIndexes */ 3
( 0 0 0 0 0 0 0 1 )
( 32 0 0 1 0 0 0 1 )
( 0 32 0 0 1 0 0 1 )
0 1 2
}
}
"#
        .into()
    }

    #[test]
    fn reject_garbage_md5() {
        let staged = tmp_dir("badmd5");
        let err = convert_md5mesh(b"not an md5 mesh", "models/x.md5mesh", &staged, "d3")
            .unwrap_err();
        assert!(
            err.contains("MD5Version") || err.contains("UTF-8") || err.contains("expected"),
            "got {err}"
        );
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn minimal_md5_one_tri_gltf() {
        let staged = tmp_dir("md5");
        let asset = convert_md5mesh(
            minimal_md5().as_bytes(),
            "models/md5/chars/guard.md5mesh",
            &staged,
            "d3",
        )
        .expect("convert");
        assert_eq!(asset.kind, AssetKind::Character);
        assert!(asset.rel_path.ends_with(".glb"));
        let glb = std::fs::read(staged.join(&asset.rel_path)).expect("glb");
        assert!(glb.starts_with(b"glTF"), "not a glTF/GLB");
        assert!(glb.len() > 200);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn md5_weapon_and_prop_kinds() {
        let staged = tmp_dir("md5kind");
        let w = convert_md5mesh(
            minimal_md5().as_bytes(),
            "models/md5/weapons/sword.md5mesh",
            &staged,
            "d3",
        )
        .unwrap();
        assert_eq!(w.kind, AssetKind::Weapon);
        let p = convert_md5mesh(
            minimal_md5().as_bytes(),
            "models/md5/props/crate.md5mesh",
            &staged,
            "d3",
        )
        .unwrap();
        assert_eq!(p.kind, AssetKind::Prop);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn minimal_proc_one_triangle_gltf() {
        let staged = tmp_dir("proc");
        let assets = convert_proc(
            minimal_proc().as_bytes(),
            "maps/test.map.proc",
            &staged,
            "d3",
        )
        .expect("convert");
        let world = assets
            .iter()
            .find(|a| a.kind == AssetKind::World)
            .expect("world");
        assert_eq!(world.rel_path, "worlds/test-map.glb");
        let glb = std::fs::read(staged.join(&world.rel_path)).expect("glb");
        assert!(glb.starts_with(b"glTF"), "not a glTF/GLB");
        assert!(glb.len() > 200);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn expand_pk4_stored_zip() {
        let mut w = ZipWriter::new();
        w.add("maps/test.proc", b"mapProcFile003\n", ZipMethod::Store)
            .unwrap();
        let bytes = w.finish().unwrap();
        let root = tmp_dir("pk4");
        let pk4 = root.join("tdm_base01.pk4");
        std::fs::write(&pk4, bytes).unwrap();
        let out = root.join("out");
        let mut warnings = Vec::new();
        let files = expand_pk4(&pk4, &out, &mut warnings).expect("expand");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel, "maps/test.proc");
        assert_eq!(std::fs::read(&files[0].path).unwrap(), b"mapProcFile003\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lwo_garbage_returns_none() {
        let staged = tmp_dir("lwobad");
        let out = convert_lwo(b"not a lightwave file", "models/x.lwo", &staged, "d3")
            .expect("ok none");
        assert!(out.is_none());
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn fan_mission_paths_are_skipped() {
        assert!(is_fan_mission_rel("fms/mymission/map.proc"));
        assert!(is_fan_mission_rel("darkmod/fms/foo/bar.md5mesh"));
        assert!(!is_fan_mission_rel("maps/tdm_training.proc"));
    }

    #[test]
    fn md5_writes_raster_icon() {
        let staged = tmp_dir("md5icon");
        let asset = convert_md5mesh(
            minimal_md5().as_bytes(),
            "models/md5/props/crate.md5mesh",
            &staged,
            "d3",
        )
        .expect("convert");
        let icon = asset.icon_rel.expect("icon");
        let png = std::fs::read(staged.join(&icon)).expect("icon bytes");
        assert!(png.starts_with(b"\x89PNG"), "icon is png");
        assert!(png.len() > 200);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn md5_binds_bank_albedo() {
        let staged = tmp_dir("md5tex");
        let mut bank = Q3TexBank::default();
        let mut rgba = vec![0u8; 8 * 8 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[200, 40, 20, 255]);
        }
        bank.images.insert(
            "textures/test".into(),
            crate::quake3_import::Q3Image { w: 8, h: 8, rgba },
        );
        let asset = convert_md5mesh_with_bank(
            minimal_md5().as_bytes(),
            "models/md5/props/crate.md5mesh",
            &staged,
            "d3",
            &bank,
        )
        .expect("convert");
        let glb = std::fs::read(staged.join(&asset.rel_path)).expect("glb");
        // The atlas rides in the BIN chunk as a compressed PNG, so look at
        // its decoded pixels rather than at raw bytes in the container.
        let at = glb
            .windows(8)
            .position(|w| w == b"\x89PNG\r\n\x1a\n")
            .expect("embedded png");
        let (rgba, _, _) =
            crate::classic_import::decode_png_stored(&glb[at..]).expect("decode atlas");
        assert!(
            rgba.chunks_exact(4).any(|px| px == [200, 40, 20, 255]),
            "albedo in glb"
        );
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn mtr_aliases_diffusemap_and_glued_keyword() {
        let src = r#"
tdm_oldchest_wood
{
    wood
    diffusemap  models/darkmod/props/textures/chest_d
    bumpmap models/darkmod/props/textures/chest_local
}

torturechair
{
diffusemapmodels/darkmod/props/textures/torturechair
}

werebeast
{
    {
        blend diffusemap
        map models/md5/chars/monsters/werebeast/werebeast_diffuse
    }
}
"#;
        let aliases = parse_mtr_aliases(src);
        assert!(
            aliases.iter().any(|(m, p)| m == "tdm_oldchest_wood"
                && p.contains("chest_d")),
            "{aliases:?}"
        );
        assert!(
            aliases
                .iter()
                .any(|(m, p)| m == "torturechair" && p.contains("torturechair")),
            "{aliases:?}"
        );
        assert!(
            aliases
                .iter()
                .any(|(m, p)| m == "werebeast" && p.contains("werebeast_diffuse")),
            "{aliases:?}"
        );
        let tdm = r#"
undressed_old_man_01
{
flesh
diffusemapmodels/md5/chars/undressed/undressed_old_man_01
}

undressed_old_man_01_hair
{
flesh
{
blenddiffusemap
mapmodels/md5/chars/undressed/undressed_old_man_01_hair
alphatest 0.4
}
}
"#;
        let tdm_aliases = parse_mtr_aliases(tdm);
        assert!(
            tdm_aliases.iter().any(|(m, p)| m == "undressed_old_man_01"
                && p.ends_with("undressed_old_man_01")),
            "{tdm_aliases:?}"
        );
        assert!(
            tdm_aliases.iter().any(|(m, p)| m == "undressed_old_man_01_hair"
                && p.ends_with("undressed_old_man_01_hair")),
            "{tdm_aliases:?}"
        );
    }

    #[test]
    fn md5_uvs_keep_image_space_v() {
        let mesh = md5_to_mesh(minimal_md5().as_bytes()).expect("mesh");
        assert_eq!(mesh.parts[0].uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
    }

    fn minimal_md5anim() -> String {
        r#"MD5Version 10
numFrames 2
numJoints 1
frameRate 24
numAnimatedComponents 0
hierarchy {
"origin" -1 0 0
}
bounds {
( -1 -1 -1 ) ( 1 1 1 )
( -1 -1 -1 ) ( 1 1 1 )
}
baseframe {
( 0 0 0 ) ( 0 0 0 )
}
frame 0 {
}
frame 1 {
}
"#
        .into()
    }

    #[test]
    fn md5anim_parses_and_samples_bind_joints() {
        let anim = parse_md5anim(minimal_md5anim().as_bytes()).expect("anim");
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.hierarchy.len(), 1);
        let j = anim.world_joints(0);
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].pos, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn anim_state_names_map_doom_like_clips() {
        assert_eq!(anim_state_name("models/md5/chars/thief/walk.md5anim"), Some("walk"));
        assert_eq!(anim_state_name("models/md5/chars/thief/jump.md5anim"), Some("jump"));
        assert_eq!(anim_state_name("models/md5/chars/thief/death.md5anim"), Some("death"));
        assert_eq!(anim_state_name("models/md5/chars/thief/head_idle.md5anim"), None);
        assert_eq!(
            anim_state_name("models/md5/chars/monsters/werebeast/werebeast_walkcycle.md5anim"),
            Some("walk")
        );
    }

    #[test]
    fn sample_frames_spreads_across_clip() {
        assert_eq!(sample_frame_indices(37, 4), vec![0, 12, 24, 36]);
        assert_eq!(sample_frame_indices(1, 4), vec![0]);
    }

    #[test]
    fn utility_materials_are_skipped() {
        assert!(is_utility_material("textures/common/nodraw"));
        assert!(is_utility_material("textures/common/shadow"));
        assert!(is_utility_material("DkBlu"));
        assert!(!is_utility_material("tdm_oldchest_wood"));
    }

    #[test]
    fn real_wine_barrel_lwo_binds_dds_when_pack_present() {
        let pack = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/darkmod");
        let models = pack.join("tdm_models01.pk4");
        let textures = pack.join("tdm_models05.pk4");
        if !models.is_file() || !textures.is_file() {
            return;
        }
        let lwo = {
            let z = makepad_zip_file::zip_read_central_directory(
                &mut std::fs::File::open(&models).unwrap(),
            )
            .expect("cd");
            let mut file = std::fs::File::open(&models).unwrap();
            let header = z
                .file_headers
                .iter()
                .find(|h| h.file_name.replace('\\', "/") == "models/darkmod/containers/barrel_wine.lwo")
                .expect("barrel lwo");
            header.extract(&mut file).expect("extract lwo")
        };
        let dds = {
            let z = makepad_zip_file::zip_read_central_directory(
                &mut std::fs::File::open(&textures).unwrap(),
            )
            .expect("cd");
            let mut file = std::fs::File::open(&textures).unwrap();
            let header = z
                .file_headers
                .iter()
                .find(|h| {
                    h.file_name.replace('\\', "/")
                        == "dds/models/darkmod/props/textures/wine_barrel.dds"
                })
                .expect("barrel dds");
            header.extract(&mut file).expect("extract dds")
        };
        let (rgba, w, h) = crate::quake3_import::decode_dds(&dds).expect("decode dds");
        assert!(w >= 64 && h >= 64, "{w}x{h}");
        let staged = tmp_dir("barrel");
        let mut bank = Q3TexBank::default();
        let dest = staged.join("wine_barrel.dds");
        std::fs::write(&dest, &dds).unwrap();
        bank.files.insert(
            "models/darkmod/props/textures/wine_barrel".into(),
            dest,
        );
        bank.images.insert(
            "models/darkmod/props/textures/wine_barrel".into(),
            crate::quake3_import::Q3Image { w, h, rgba },
        );
        let asset = convert_lwo_with_bank(
            &lwo,
            "models/darkmod/containers/barrel_wine.lwo",
            &staged,
            "d3",
            &bank,
        )
        .expect("convert")
        .expect("some");
        assert!(asset.icon_rel.is_some());
        let icon = std::fs::read(staged.join(asset.icon_rel.as_ref().unwrap())).unwrap();
        assert!(icon.starts_with(b"\x89PNG"));
        assert!(icon.len() > 2000, "raster icon should not be a tiny gray tile");
        let _ = std::fs::remove_dir_all(&staged);
    }
}
