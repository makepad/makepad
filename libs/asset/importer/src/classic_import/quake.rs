//! Quake 1 shareware / LibreQuake conversion (BSP, MDL, SPR, WAD2).

use super::doom::{
    emit_tri_st_atlas, is_character_mdl, lookup_slot, pack_atlas, quake_bsp_nav, quake_bsp_place,
};
use super::shared::*;
use crate::vertex_skin;
use makepad_asset_data::AssetKind;
use makepad_gltf::{write_glb_mesh_textured, GlbTexturedMesh};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(crate) fn convert_bsp(
    path: &Path,
    rel: &str,
    staged: &Path,
    source: ClassicSource,
    wal_bank: &crate::quake2_import::WalBank,
    q3_bank: &crate::quake3_import::Q3TexBank,
) -> Result<Vec<ClassicAsset>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() >= 8 && &bytes[0..4] == b"IBSP" {
        let ver = i32_le(&bytes, 4);
        if ver == 38 {
            return crate::quake2_import::convert_bsp38(&bytes, rel, staged, wal_bank, source.id());
        }
        if ver == 46 {
            return crate::quake3_import::convert_bsp46(&bytes, rel, staged, q3_bank, source.id());
        }
        return Err(format!("unsupported IBSP version {ver}"));
    }
    let map = quake_bsp_to_map(&bytes)?;
    let glb = map.glb;
    let slug = stem_slug(rel);
    // `b_*` are inline brush models (ammo boxes, health), not walkable maps.
    // Treating them as worlds left empty cards: no spawn, no icon.
    if slug.starts_with("b_") {
        return convert_brush_prop(&glb, &slug, staged, source);
    }
    let key = format!("worlds/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &glb).map_err(|e| e.to_string())?;
    if let Some(mut nav) = quake_bsp_nav(&bytes) {
        // A Quake door slides along its own axis: the anchor says WHERE it
        // is and how far it travels; the direction is the GLB's clip.
        nav.doors = map
            .doors
            .iter()
            .map(|d| crate::world_nav::NavDoor {
                name: d.name.clone(),
                pos: d.centre,
                closed_y: d.centre[1],
                // A Quake door mostly slides SIDEWAYS: the Y pair is the
                // vertical part of the move and says nothing on its own,
                // which is what `offset` is for.
                open_y: d.centre[1] + d.travel[1],
                offset: d.travel,
            })
            .collect();
        // A plat travels straight down, so its anchor is the Y pair the
        // contract already has.
        nav.lifts = map
            .lifts
            .iter()
            .map(|l| {
                crate::world_nav::NavDoor::vertical(
                    l.name.clone(),
                    l.centre,
                    l.centre[1],
                    l.down_y,
                )
            })
            .collect();
        nav.teleports = map.teleports.clone();
        write_nav_sidecar(&dest, &nav);
    }
    let place = quake_bsp_place(&bytes, source.id(), &key);
    let _ = crate::world_place::write_place_sidecar(&dest, &place);
    let icon_rel = crate::world_preview::write_spawn_preview(&dest)
        .ok()
        .map(|_| format!("{key}.png"));
    Ok(vec![ClassicAsset {
        key,
        kind: AssetKind::World,
        rel_path,
        tags: tags_for(AssetKind::World, &[source.id(), "map", "bsp", "no-portals"]),
        icon_rel,
    }])
}

fn convert_brush_prop(
    glb: &[u8],
    slug: &str,
    staged: &Path,
    source: ClassicSource,
) -> Result<Vec<ClassicAsset>, String> {
    let key = format!("props/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, glb).map_err(|e| e.to_string())?;
    let mut icon_rel = None;
    if let Some(png) = raster_glb_icon(glb, 0.35, 256) {
        let icon_path = dest.with_extension("png");
        if std::fs::write(&icon_path, &png).is_ok() {
            icon_rel = Some(format!("{key}.png"));
        }
    }
    Ok(vec![ClassicAsset {
        key,
        kind: AssetKind::Prop,
        rel_path,
        tags: tags_for(AssetKind::Prop, &[source.id(), "bsp", "brush", slug]),
        icon_rel,
    }])
}

fn raster_glb_icon(glb: &[u8], yaw: f32, dim: usize) -> Option<Vec<u8>> {
    let parts = crate::world_preview::extract_glb_parts(glb).ok()?;
    let mut pos = Vec::new();
    let mut uv = Vec::new();
    let mut idx = Vec::new();
    let mut tex = vec![180u8, 170, 160, 255];
    let mut tw = 1usize;
    let mut th = 1usize;
    for p in &parts {
        let base = pos.len() as u32;
        pos.extend_from_slice(&p.pos);
        if p.uv.len() == p.pos.len() {
            uv.extend_from_slice(&p.uv);
        } else {
            uv.extend(std::iter::repeat_n([0.0, 0.0], p.pos.len()));
        }
        idx.extend(p.indices.iter().map(|i| i + base));
        if let Some(t) = &p.tex {
            if !t.is_empty() {
                tex = t.clone();
                tw = p.tex_size.0.max(1);
                th = p.tex_size.1.max(1);
            }
        }
    }
    if pos.is_empty() || idx.len() < 3 {
        return None;
    }
    let tile = crate::anim_icon::raster_mesh_icon(&pos, &idx, &uv, &tex, tw, th, yaw, dim);
    encode_png_rgba(&tile, dim as u32, dim as u32).ok()
}

/// A converted Quake map: the GLB plus what moves in it.
pub(crate) struct QuakeMap {
    pub glb: Vec<u8>,
    pub doors: Vec<QuakeDoor>,
    pub lifts: Vec<QuakeLift>,
    pub teleports: Vec<crate::world_nav::NavTeleport>,
}

pub(crate) fn quake_bsp_to_glb(bytes: &[u8]) -> Result<Vec<u8>, String> {
    quake_bsp_to_map(bytes).map(|m| m.glb)
}

pub(crate) fn quake_bsp_to_map(bytes: &[u8]) -> Result<QuakeMap, String> {
    if bytes.len() < 4 + 15 * 8 {
        return Err("BSP too small".into());
    }
    let version = i32_le(bytes, 0);
    if version != 29 {
        return Err(format!("unsupported BSP version {version}"));
    }
    // lump index: 0 entities, 1 planes, 2 textures, 3 vertices, 4 visibility,
    // 5 nodes, 6 texinfo, 7 faces, 8 lighting, 9 clipnodes, 10 leaves,
    // 11 marksurfaces, 12 edges, 13 surfedges, 14 models
    let lump = |i: usize| -> (usize, usize) {
        let o = 4 + i * 8;
        (u32_le(bytes, o) as usize, u32_le(bytes, o + 4) as usize)
    };
    let (voff, vlen) = lump(3);
    let (toff, tlen) = lump(2);
    let (fioff, filen) = lump(6);
    let (foff, flen) = lump(7);
    let (eoff, elen) = lump(12);
    let (seoff, selen) = lump(13);
    if voff + vlen > bytes.len()
        || foff + flen > bytes.len()
        || eoff + elen > bytes.len()
        || seoff + selen > bytes.len()
        || fioff + filen > bytes.len()
    {
        return Err("BSP lump truncated".into());
    }
    let n_verts = vlen / 12;
    let n_edges = elen / 4;
    let n_surfedges = selen / 4;
    let n_faces = flen / 20;
    let n_texinfo = filen / 40;

    let mut verts = Vec::with_capacity(n_verts);
    for i in 0..n_verts {
        let o = voff + i * 12;
        verts.push([f32_le(bytes, o), f32_le(bytes, o + 4), f32_le(bytes, o + 8)]);
    }
    let mut edges = Vec::with_capacity(n_edges);
    for i in 0..n_edges {
        let o = eoff + i * 4;
        edges.push((u16_le(bytes, o) as usize, u16_le(bytes, o + 2) as usize));
    }
    let mut surfedges = Vec::with_capacity(n_surfedges);
    for i in 0..n_surfedges {
        surfedges.push(i32_le(bytes, seoff + i * 4));
    }

    // Textures (miptex directory).
    let mut sky_layers: Option<(Vec<u8>, Vec<u8>)> = None;
    let mut tex_names: Vec<String> = Vec::new();
    let mut tex_images: BTreeMap<String, RgbaImage> = BTreeMap::new();
    tex_images.insert(
        "_default".into(),
        RgbaImage {
            w: 16,
            h: 16,
            rgba: vec![0x90; 16 * 16 * 4],
        },
    );
    if tlen >= 4 {
        let numtex = i32_le(bytes, toff) as usize;
        if numtex < 4096 && toff + 4 + numtex * 4 <= bytes.len() {
            for i in 0..numtex {
                let offset = i32_le(bytes, toff + 4 + i * 4);
                if offset < 0 {
                    tex_names.push(format!("tex{i}"));
                    continue;
                }
                let mo = toff + offset as usize;
                if mo + 40 > bytes.len() {
                    tex_names.push(format!("tex{i}"));
                    continue;
                }
                let name = lump_name(&bytes[mo..mo + 16]);
                let tw = u32_le(bytes, mo + 16) as usize;
                let th = u32_le(bytes, mo + 20) as usize;
                let data_off = u32_le(bytes, mo + 24) as usize;
                tex_names.push(name.clone());
                // The sky picture is not an atlas tile: it becomes the sky
                // node's two scrolling layers.
                if is_quake_sky(&name) && sky_layers.is_none() && data_off != 0 {
                    let pix_off = mo + data_off;
                    if tw >= 2 && th >= 1 && pix_off + tw * th <= bytes.len() {
                        sky_layers =
                            quake_sky_layers(&bytes[pix_off..pix_off + tw * th], tw as u32, th as u32);
                    }
                }
                if skip_quake_tex(&name) || is_quake_sky(&name) {
                    continue;
                }
                if tw == 0 || th == 0 || tw > 512 || th > 512 {
                    continue;
                }
                let pix_off = mo + data_off;
                let pix_len = tw * th;
                if data_off == 0 || pix_off + pix_len > bytes.len() {
                    continue;
                }
                let indices = &bytes[pix_off..pix_off + pix_len];
                let pal = quake_palette();
                // Color 255 is transparent on fence textures (`{` prefix).
                let transparent = name.starts_with('{');
                let mut rgba = Vec::with_capacity(pix_len * 4);
                for &idx in indices {
                    if transparent && idx == 255 {
                        rgba.extend_from_slice(&[0, 0, 0, 0]);
                    } else {
                        let c = pal[idx as usize];
                        rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
                    }
                }
                tex_images.insert(
                    name,
                    RgbaImage {
                        w: tw as u32,
                        h: th as u32,
                        rgba,
                    },
                );
            }
        }
    }

    let (atlas_png, uv_map) = pack_atlas(&tex_images);

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let mut liquid_planes: BTreeSet<(i16, String)> = BTreeSet::new();

    let scale = 1.0 / 32.0; // Quake units → rough meters

    // Faces that leave the level mesh: the sky, the liquids you swim in,
    // and every brush a `func_door` moves.
    let mut sky = crate::classic_import::doom::SkyFaces::default();
    let (moff, mlen) = lump(14);
    let models = quake_models(bytes, moff, mlen);
    let (entoff, entlen) = lump(0);
    let entities = if entoff + entlen <= bytes.len() {
        std::str::from_utf8(&bytes[entoff..entoff + entlen]).unwrap_or("")
    } else {
        ""
    };
    let ents = quake_entities(entities);
    let doors = quake_doors(&ents, &models, scale);
    let lifts = quake_plats(&ents, &models, scale);
    let teleports = quake_teleports(
        &ents,
        &models,
        scale,
        (super::doom::QUAKE_VIEW_OFFSET + super::doom::QUAKE_ORIGIN_ABOVE_FLOOR)
            * super::doom::QUAKE_UNIT,
    );
    let mut door_of_face: BTreeMap<usize, usize> = BTreeMap::new();
    for (di, door) in doors.iter().enumerate() {
        for f in door.first_face..door.first_face + door.num_faces {
            door_of_face.insert(f, di);
        }
    }
    // A plat's faces leave the level for `lift_N`, so a walker meets the
    // platform where the map drew it instead of a hole in the shaft.
    let mut lift_of_face: BTreeMap<usize, usize> = BTreeMap::new();
    for (li, lift) in lifts.iter().enumerate() {
        for f in lift.first_face..lift.first_face + lift.num_faces {
            lift_of_face.entry(f).or_insert(li);
        }
    }
    let mut door_geom: Vec<(Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>)> =
        vec![(Vec::new(), Vec::new(), Vec::new()); doors.len()];
    let mut lift_geom: Vec<(Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>)> =
        vec![(Vec::new(), Vec::new(), Vec::new()); lifts.len()];
    let mut liquid_geom: BTreeMap<String, (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>)> =
        BTreeMap::new();

    for fi in 0..n_faces {
        let o = foff + fi * 20;
        // face v29: short planenum; short side; int firstedge; short numedges; short texinfo; ...
        let planenum = i16_le(bytes, o);
        let side = i16_le(bytes, o + 2);
        let firstedge = i32_le(bytes, o + 4);
        let numedges = i16_le(bytes, o + 8);
        let texinfo = i16_le(bytes, o + 10) as usize;
        if firstedge < 0 || numedges < 3 {
            continue;
        }
        let firstedge = firstedge as usize;
        let numedges = numedges as usize;
        if firstedge + numedges > n_surfedges {
            continue;
        }
        let mut tex_name = "_default".to_string();
        let mut svec = [1.0f32, 0.0, 0.0, 0.0];
        let mut tvec = [0.0f32, 1.0, 0.0, 0.0];
        if texinfo < n_texinfo {
            let tio = fioff + texinfo * 40;
            if tio + 40 <= bytes.len() {
                for k in 0..4 {
                    svec[k] = f32_le(bytes, tio + k * 4);
                    tvec[k] = f32_le(bytes, tio + 16 + k * 4);
                }
                let miptex = i32_le(bytes, tio + 32) as usize;
                if miptex < tex_names.len() {
                    tex_name = tex_names[miptex].clone();
                }
            }
        }
        if skip_quake_tex(&tex_name) {
            continue;
        }
        let sky_face = is_quake_sky(&tex_name);
        // Water/slime/lava are two-sided in the BSP and the GLB is
        // double-sided — emitting both faces z-fights every liquid.
        if is_quake_liquid(&tex_name) {
            if side != 0 || !liquid_planes.insert((planenum, tex_name.clone())) {
                continue;
            }
        }
        let slot = if sky_face {
            crate::classic_import::doom::SKY_SLOT
        } else {
            lookup_slot(&uv_map, &tex_name)
        };
        let tw = slot.w.max(1) as f32;
        let th = slot.h.max(1) as f32;

        let mut face_verts: Vec<[f32; 3]> = Vec::new();
        let mut face_st: Vec<[f32; 2]> = Vec::new();
        for e in 0..numedges {
            let se = surfedges[firstedge + e];
            let (a, b) = if se >= 0 {
                let edge = edges.get(se as usize).copied().unwrap_or((0, 0));
                edge
            } else {
                let edge = edges.get((-se) as usize).copied().unwrap_or((0, 0));
                (edge.1, edge.0)
            };
            if a < n_verts {
                let v = verts[a];
                face_verts.push([v[0] * scale, v[2] * scale, -v[1] * scale]); // Z-up → Y-up
                let s = v[0] * svec[0] + v[1] * svec[1] + v[2] * svec[2] + svec[3];
                let t = v[0] * tvec[0] + v[1] * tvec[1] + v[2] * tvec[2] + tvec[3];
                face_st.push([s / tw, t / th]);
            }
            let _ = b;
        }
        if face_verts.len() < 3 {
            continue;
        }
        // Quake faces are already convex. Fan them and clip only when the
        // face crosses a texture tile — same as a Quake engine with
        // GL_REPEAT, but we pack an atlas so we cannot interpolate across
        // a tile boundary.
        // Where this face's triangles go: the sky node, a liquid node, the
        // door that moves it, or the level mesh.
        let (out_pos, out_uv, out_idx) = if sky_face {
            (&mut sky.positions, &mut sky.uvs, &mut sky.indices)
        } else if is_quake_liquid(&tex_name) {
            let entry = liquid_geom
                .entry(tex_name.clone())
                .or_insert_with(|| (Vec::new(), Vec::new(), Vec::new()));
            (&mut entry.0, &mut entry.1, &mut entry.2)
        } else if let Some(&di) = door_of_face.get(&fi) {
            let g = &mut door_geom[di];
            (&mut g.0, &mut g.1, &mut g.2)
        } else if let Some(&li) = lift_of_face.get(&fi) {
            let g = &mut lift_geom[li];
            (&mut g.0, &mut g.1, &mut g.2)
        } else {
            (&mut positions, &mut uvs, &mut indices)
        };
        for i in 1..face_verts.len() - 1 {
            emit_tri_st_atlas(
                out_pos,
                out_uv,
                out_idx,
                face_verts[0],
                face_verts[i],
                face_verts[i + 1],
                face_st[0],
                face_st[i],
                face_st[i + 1],
                slot,
            );
        }
    }

    if positions.is_empty() {
        positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        indices = vec![0, 1, 2, 0, 2, 3];
    }

    // Weld the T-junctions the BSP leaves behind. A Quake face is split by
    // the node tree wherever a neighbouring leaf ends, so the long face on
    // one side of a wall meets several short ones on the other — the same
    // hairline Doom subsectors produce, and the same fix. Doors, liquids
    // and the sky are separate meshes, so the grid is built from all of
    // them and every part is welded against it.
    {
        let mut parts: Vec<&[[f32; 3]]> = vec![&positions[..], &sky.positions[..]];
        parts.extend(liquid_geom.values().map(|g| &g.0[..]));
        parts.extend(door_geom.iter().map(|g| &g.0[..]));
        parts.extend(lift_geom.iter().map(|g| &g.0[..]));
        let weld = super::weld::Weld::from_parts(&parts);
        weld.split(super::weld::Soup {
            positions: &mut positions,
            uvs: &mut uvs,
            normals: None,
            colors: None,
            indices: &mut indices,
        });
        weld.split(super::weld::Soup {
            positions: &mut sky.positions,
            uvs: &mut sky.uvs,
            normals: None,
            colors: None,
            indices: &mut sky.indices,
        });
        for g in liquid_geom.values_mut() {
            weld.split(super::weld::Soup {
                positions: &mut g.0,
                uvs: &mut g.1,
                normals: None,
                colors: None,
                indices: &mut g.2,
            });
        }
        for g in door_geom.iter_mut().chain(lift_geom.iter_mut()) {
            weld.split(super::weld::Soup {
                positions: &mut g.0,
                uvs: &mut g.1,
                normals: None,
                colors: None,
                indices: &mut g.2,
            });
        }
    }

    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &positions,
        normals: None,
        uvs: &uvs,
        indices: &indices,
        base_color_png: &atlas_png,
        metallic_roughness_png: None,
        double_sided: true,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return Err("GLB writer failed".into());
    }

    let mut extra = Vec::new();
    for (di, door) in doors.iter().enumerate() {
        let (p, u, i) = &door_geom[di];
        if i.len() < 3 {
            continue;
        }
        extra.push(crate::glb_nodes::ExtraNode::door_vector(
            door.name.clone(),
            p.clone(),
            u.clone(),
            i.clone(),
            door.travel,
            door.axis,
        )
        .secret(door.secret));
    }
    for (li, lift) in lifts.iter().enumerate() {
        let (p, u, i) = &lift_geom[li];
        if i.len() < 3 {
            continue;
        }
        extra.push(crate::glb_nodes::ExtraNode::lift(
            lift.name.clone(),
            p.clone(),
            u.clone(),
            i.clone(),
            Vec::new(),
            lift.centre[1],
            lift.down_y,
        ));
    }
    for (n, (name, (p, u, i))) in liquid_geom.iter().enumerate() {
        if i.len() < 3 {
            continue;
        }
        extra.push(crate::glb_nodes::ExtraNode::hazard(
            format!("hazard_{}", n + 1),
            p.clone(),
            u.clone(),
            i.clone(),
            Vec::new(),
            quake_liquid_damage(name),
            name.trim_start_matches('*'),
            true,
            // Quake liquids are volumes you SWIM through, not floors you
            // stand on — the surface must not stop a walker.
            false,
        ));
    }
    if !sky.is_empty() {
        if let Some((back, front)) = sky_layers {
            extra.push(crate::glb_nodes::ExtraNode::sky(
                std::mem::take(&mut sky.positions),
                std::mem::take(&mut sky.uvs),
                std::mem::take(&mut sky.indices),
                vec![back, front],
                "quake_scroll",
                1.0,
                0.0,
                "sky",
                // Quake's own `R_DrawSkyChain`: the back layer slides at 8
                // texture units a second, the keyed front at 16.
                Some([8.0, 16.0]),
                None,
            ));
        }
    }
    let glb = crate::glb_nodes::inject_nodes(&glb, &extra).unwrap_or(glb);
    Ok(QuakeMap {
        glb,
        doors,
        lifts,
        teleports,
    })
}

/// Split a Quake sky picture into its two layers: the RIGHT half is the
/// solid back layer, the LEFT half the front layer whose palette index 0 is
/// transparent (`R_InitSky`).
pub(crate) fn quake_sky_layers(indices: &[u8], w: u32, h: u32) -> Option<(Vec<u8>, Vec<u8>)> {
    let half = (w / 2) as usize;
    if half == 0 || h == 0 || indices.len() < (w * h) as usize {
        return None;
    }
    let pal = quake_palette();
    let mut back = Vec::with_capacity(half * h as usize * 4);
    let mut front = Vec::with_capacity(half * h as usize * 4);
    for y in 0..h as usize {
        let row = y * w as usize;
        for x in 0..half {
            let b = pal[indices[row + half + x] as usize];
            back.extend_from_slice(&[b[0], b[1], b[2], 255]);
            let i = indices[row + x];
            let f = pal[i as usize];
            let alpha = if i == 0 { 0 } else { 255 };
            front.extend_from_slice(&[f[0], f[1], f[2], alpha]);
        }
    }
    Some((
        encode_png_rgba(&back, half as u32, h).ok()?,
        encode_png_rgba(&front, half as u32, h).ok()?,
    ))
}

/// A Quake sub-model that moves: `func_door` and friends, resolved from the
/// entity's `model "*N"` to the face range in lump 14.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuakeDoor {
    pub name: String,
    pub first_face: usize,
    pub num_faces: usize,
    /// Brush centre in GLB space (metres), at its CLOSED pose.
    pub centre: [f32; 3],
    /// Open pose in GLB space (metres): the geometry is authored CLOSED.
    pub travel: [f32; 3],
    /// Dominant axis of `travel`, for the node extras.
    pub axis: &'static str,
    /// `func_door_secret`: drawn as a wall, opens like a door.
    pub secret: bool,
}

/// Quake's default door `lip`: how much of the door stays showing.
pub(crate) const QUAKE_DOOR_LIP: f32 = 8.0;

/// Brush models (lump 14). Only the face range and bounds are needed here.
pub(crate) fn quake_models(bytes: &[u8], off: usize, len: usize) -> Vec<([f32; 6], usize, usize)> {
    let mut out = Vec::new();
    let n = len / 64;
    for i in 0..n {
        let o = off + i * 64;
        if o + 64 > bytes.len() {
            break;
        }
        let mut bounds = [0.0f32; 6];
        for (k, b) in bounds.iter_mut().enumerate() {
            *b = f32_le(bytes, o + k * 4);
        }
        let first = i32_le(bytes, o + 56).max(0) as usize;
        let count = i32_le(bytes, o + 60).max(0) as usize;
        out.push((bounds, first, count));
    }
    out
}

/// One entity block of the entity lump, as key/value pairs.
///
/// The lump is a flat list of `{ "key" "value" … }` blocks. Every reader in
/// this file used to re-scan it for the two or three keys it cared about;
/// there are enough of them now (doors, plats, teleport pads and their
/// destinations) that one parse is both shorter and the only way a
/// destination can be looked up by `targetname`.
pub(crate) fn quake_entities(entities: &str) -> Vec<BTreeMap<String, String>> {
    let mut out = Vec::new();
    for block in entities.split(|c| c == '{' || c == '}') {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let mut kv = BTreeMap::new();
        for line in block.lines() {
            let parts: Vec<&str> = line
                .trim()
                .split('"')
                .filter(|s| !s.trim().is_empty())
                .collect();
            if parts.len() >= 2 {
                kv.insert(parts[0].to_string(), parts[1].to_string());
            }
        }
        if !kv.is_empty() {
            out.push(kv);
        }
    }
    out
}

fn ent_f32(e: &BTreeMap<String, String>, key: &str) -> Option<f32> {
    e.get(key).and_then(|v| v.trim().parse().ok())
}

fn ent_origin(e: &BTreeMap<String, String>) -> Option<[f32; 3]> {
    let mut it = e.get("origin")?.split_whitespace();
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    let z = it.next()?.parse().ok()?;
    Some([x, y, z])
}

/// The brush model an entity's `model "*N"` names.
fn ent_model<'a>(
    e: &BTreeMap<String, String>,
    models: &'a [([f32; 6], usize, usize)],
) -> Option<&'a ([f32; 6], usize, usize)> {
    let index = e.get("model")?.strip_prefix('*')?.parse::<usize>().ok()?;
    models.get(index)
}

/// A `func_plat`: a floor authored at its TOP that drops out from under you.
///
/// `SP_func_plat` sets `pos2_z = origin_z - height`, or `- size_z + 8` when
/// no `height` is given. The brush in the BSP is the top pose, which is the
/// pose [`crate::glb_nodes::ExtraNode::lift`] wants — authored up, resting
/// up, travelling away.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuakeLift {
    pub name: String,
    pub first_face: usize,
    pub num_faces: usize,
    /// Brush centre in GLB space (metres) at the UP pose.
    pub centre: [f32; 3],
    /// Where it travels to, in GLB metres (below `centre[1]`).
    pub down_y: f32,
}

/// Quake's hard-coded plat headroom: a plat with no `height` drops its own
/// size less eight units.
pub(crate) const QUAKE_PLAT_LIP: f32 = 8.0;

pub(crate) fn quake_plats(
    entities: &[BTreeMap<String, String>],
    models: &[([f32; 6], usize, usize)],
    scale: f32,
) -> Vec<QuakeLift> {
    let mut out = Vec::new();
    for e in entities {
        if e.get("classname").map(String::as_str) != Some("func_plat") {
            continue;
        }
        let Some(&(bounds, first, count)) = ent_model(e, models) else {
            continue;
        };
        if count == 0 {
            continue;
        }
        let size_z = bounds[5] - bounds[2];
        let drop = ent_f32(e, "height").unwrap_or((size_z - QUAKE_PLAT_LIP).max(0.0));
        if drop <= 0.0 {
            continue;
        }
        // Z-up -> Y-up: the plat's floor is the TOP of its brush.
        let up_y = bounds[5] * scale;
        out.push(QuakeLift {
            name: format!("lift_{}", out.len() + 1),
            first_face: first,
            num_faces: count,
            centre: [
                (bounds[0] + bounds[3]) * 0.5 * scale,
                up_y,
                -(bounds[1] + bounds[4]) * 0.5 * scale,
            ],
            down_y: up_y - drop * scale,
        });
    }
    out
}

/// `trigger_teleport` pads and the `info_teleport_destination` each one
/// sends you to.
///
/// A Quake teleporter is the only way into some rooms, and its pad is a
/// brush with no drawn faces — so a converter that publishes only geometry
/// leaves a navigator walking into a wall forever.
pub(crate) fn quake_teleports(
    entities: &[BTreeMap<String, String>],
    models: &[([f32; 6], usize, usize)],
    scale: f32,
    eye_above_origin: f32,
) -> Vec<crate::world_nav::NavTeleport> {
    let mut dests: BTreeMap<&str, (&BTreeMap<String, String>, [f32; 3])> = BTreeMap::new();
    for e in entities {
        let class = e.get("classname").map(String::as_str).unwrap_or("");
        if class != "info_teleport_destination" && class != "misc_teleporter_dest" {
            continue;
        }
        let (Some(name), Some(origin)) = (e.get("targetname"), ent_origin(e)) else {
            continue;
        };
        dests.entry(name.as_str()).or_insert((e, origin));
    }
    let mut out = Vec::new();
    for e in entities {
        if e.get("classname").map(String::as_str) != Some("trigger_teleport") {
            continue;
        }
        let Some(target) = e.get("target") else { continue };
        let Some(&(dest, origin)) = dests.get(target.as_str()) else {
            continue;
        };
        let Some(&(bounds, _, _)) = ent_model(e, models) else {
            continue;
        };
        // Quake (x, y, z) -> GLB (x, z, -y): the pad's y bounds swap ends.
        out.push(crate::world_nav::NavTeleport {
            name: format!("teleport_{}", out.len() + 1),
            pad_min: [bounds[0] * scale, -bounds[4] * scale],
            pad_max: [bounds[3] * scale, -bounds[1] * scale],
            dst: [
                origin[0] * scale,
                origin[2] * scale + eye_above_origin,
                -origin[1] * scale,
            ],
            yaw: std::f32::consts::FRAC_PI_2
                - ent_f32(dest, "angle").unwrap_or(0.0).to_radians(),
        });
    }
    out
}

/// `func_door` entities, resolved to face ranges and their open offset.
///
/// Quake opens a door along `angle` (`-1` up, `-2` down, otherwise a compass
/// direction) by the door's own size on that axis minus `lip`
/// (`SP_func_door`/`LinkDoors`). The map authors the CLOSED pose.
pub(crate) fn quake_doors(
    entities: &[BTreeMap<String, String>],
    models: &[([f32; 6], usize, usize)],
    scale: f32,
) -> Vec<QuakeDoor> {
    let mut out = Vec::new();
    for e in entities {
        let class = e.get("classname").map(String::as_str).unwrap_or("");
        if !class.starts_with("func_door") {
            continue;
        }
        let angle = ent_f32(e, "angle").unwrap_or(0.0);
        let lip = ent_f32(e, "lip").unwrap_or(QUAKE_DOOR_LIP);
        // A `func_door_secret` is drawn as a wall and opens like one: a
        // walker that cannot find it is stuck in the room forever, which is
        // exactly what the contract's `secret` flag is for.
        let secret = class == "func_door_secret";
        let Some(&(bounds, first, count)) = ent_model(e, models) else {
            continue;
        };
        if count == 0 {
            continue;
        }
        // Quake movement direction, Z-up.
        let dir = if (angle - -1.0).abs() < 0.01 {
            [0.0, 0.0, 1.0]
        } else if (angle - -2.0).abs() < 0.01 {
            [0.0, 0.0, -1.0]
        } else {
            let r = angle.to_radians();
            [r.cos(), r.sin(), 0.0]
        };
        let size = [
            bounds[3] - bounds[0],
            bounds[4] - bounds[1],
            bounds[5] - bounds[2],
        ];
        let span = (dir[0] * size[0]).abs() + (dir[1] * size[1]).abs() + (dir[2] * size[2]).abs();
        let distance = (span - lip).max(0.0);
        if distance <= 0.0 {
            continue;
        }
        // Z-up -> Y-up, same mapping the geometry uses.
        let travel = [
            dir[0] * distance * scale,
            dir[2] * distance * scale,
            -dir[1] * distance * scale,
        ];
        let axis = if travel[1].abs() >= travel[0].abs() && travel[1].abs() >= travel[2].abs() {
            "y"
        } else if travel[0].abs() >= travel[2].abs() {
            "x"
        } else {
            "z"
        };
        let mid = [
            (bounds[0] + bounds[3]) * 0.5,
            (bounds[1] + bounds[4]) * 0.5,
            (bounds[2] + bounds[5]) * 0.5,
        ];
        out.push(QuakeDoor {
            name: format!("door_{}", out.len() + 1),
            first_face: first,
            num_faces: count,
            centre: [mid[0] * scale, mid[2] * scale, -mid[1] * scale],
            travel,
            axis,
            secret,
        });
    }
    out
}

/// What a Quake liquid does to a swimmer, as a percent-per-second figure on
/// the same scale the Doom hazards use (`T_Damage` in `misc.qc`: lava is
/// lethal, slime hurts, water is harmless).
pub(crate) fn quake_liquid_damage(name: &str) -> u8 {
    let n = name.trim_start_matches('*').to_ascii_lowercase();
    if n.starts_with("lava") {
        20
    } else if n.starts_with("slime") {
        10
    } else {
        0
    }
}

pub(crate) fn skip_quake_tex(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("clip") || n.starts_with("trigger") || n == "skip" || n == "hint" || n == "aaatrigger"
}

/// Quake's sky brushes: drawn as the sky, not as a wall.
pub(crate) fn is_quake_sky(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with("sky")
}

pub(crate) fn is_quake_liquid(name: &str) -> bool {
    name.starts_with('*')
}

/// Official Quake palette (gfx/palette.lmp from LibreQuake pak0).
pub(crate) fn quake_palette() -> [[u8; 3]; 256] {
    const P: [[u8; 3]; 256] = [
        [0, 0, 0], [15, 15, 15], [31, 31, 31], [47, 47, 47], [63, 63, 63], [75, 75, 75], [91, 91, 91], [107, 107, 107],
        [123, 123, 123], [139, 139, 139], [155, 155, 155], [171, 171, 171], [187, 187, 187], [203, 203, 203], [219, 219, 219], [235, 235, 235],
        [15, 11, 7], [23, 15, 11], [31, 23, 11], [39, 27, 15], [47, 35, 19], [55, 43, 23], [63, 47, 23], [75, 55, 27],
        [83, 59, 27], [91, 67, 31], [99, 75, 31], [107, 83, 31], [115, 87, 31], [123, 95, 35], [131, 103, 35], [143, 111, 35],
        [11, 11, 15], [19, 19, 27], [27, 27, 39], [39, 39, 51], [47, 47, 63], [55, 55, 75], [63, 63, 87], [71, 71, 103],
        [79, 79, 115], [91, 91, 127], [99, 99, 139], [107, 107, 151], [115, 115, 163], [123, 123, 175], [131, 131, 187], [139, 139, 203],
        [0, 0, 0], [7, 7, 0], [11, 11, 0], [19, 19, 0], [27, 27, 0], [35, 35, 0], [43, 43, 7], [47, 47, 7],
        [55, 55, 7], [63, 63, 7], [71, 71, 7], [75, 75, 11], [83, 83, 11], [91, 91, 11], [99, 99, 11], [107, 107, 15],
        [7, 0, 0], [15, 0, 0], [23, 0, 0], [31, 0, 0], [39, 0, 0], [47, 0, 0], [55, 0, 0], [63, 0, 0],
        [71, 0, 0], [79, 0, 0], [87, 0, 0], [95, 0, 0], [103, 0, 0], [111, 0, 0], [119, 0, 0], [127, 0, 0],
        [19, 19, 0], [27, 27, 0], [35, 35, 0], [47, 43, 0], [55, 47, 0], [67, 55, 0], [75, 59, 7], [87, 67, 7],
        [95, 71, 7], [107, 75, 11], [119, 83, 15], [131, 87, 19], [139, 91, 19], [151, 95, 27], [163, 99, 31], [175, 103, 35],
        [35, 19, 7], [47, 23, 11], [59, 31, 15], [75, 35, 19], [87, 43, 23], [99, 47, 31], [115, 55, 35], [127, 59, 43],
        [143, 67, 51], [159, 79, 51], [175, 99, 47], [191, 119, 47], [207, 143, 43], [223, 171, 39], [239, 203, 31], [255, 243, 27],
        [11, 7, 0], [27, 19, 0], [43, 35, 15], [55, 43, 19], [71, 51, 27], [83, 55, 35], [99, 63, 43], [111, 71, 51],
        [127, 83, 63], [139, 95, 71], [155, 107, 83], [167, 123, 95], [183, 135, 107], [195, 147, 123], [211, 163, 139], [227, 179, 151],
        [171, 139, 163], [159, 127, 151], [147, 115, 135], [139, 103, 123], [127, 91, 111], [119, 83, 99], [107, 75, 87], [95, 63, 75],
        [87, 55, 67], [75, 47, 55], [67, 39, 47], [55, 31, 35], [43, 23, 27], [35, 19, 19], [23, 11, 11], [15, 7, 7],
        [187, 115, 159], [175, 107, 143], [163, 95, 131], [151, 87, 119], [139, 79, 107], [127, 75, 95], [115, 67, 83], [107, 59, 75],
        [95, 51, 63], [83, 43, 55], [71, 35, 43], [59, 31, 35], [47, 23, 27], [35, 19, 19], [23, 11, 11], [15, 7, 7],
        [219, 195, 187], [203, 179, 167], [191, 163, 155], [175, 151, 139], [163, 135, 123], [151, 123, 111], [135, 111, 95], [123, 99, 83],
        [107, 87, 71], [95, 75, 59], [83, 63, 51], [67, 51, 39], [55, 43, 31], [39, 31, 23], [27, 19, 15], [15, 11, 7],
        [111, 131, 123], [103, 123, 111], [95, 115, 103], [87, 107, 95], [79, 99, 87], [71, 91, 79], [63, 83, 71], [55, 75, 63],
        [47, 67, 55], [43, 59, 47], [35, 51, 39], [31, 43, 31], [23, 35, 23], [15, 27, 19], [11, 19, 11], [7, 11, 7],
        [255, 243, 27], [239, 223, 23], [219, 203, 19], [203, 183, 15], [187, 167, 15], [171, 151, 11], [155, 131, 7], [139, 115, 7],
        [123, 99, 7], [107, 83, 0], [91, 71, 0], [75, 55, 0], [59, 43, 0], [43, 31, 0], [27, 15, 0], [11, 7, 0],
        [0, 0, 255], [11, 11, 239], [19, 19, 223], [27, 27, 207], [35, 35, 191], [43, 43, 175], [47, 47, 159], [47, 47, 143],
        [47, 47, 127], [47, 47, 111], [47, 47, 95], [43, 43, 79], [35, 35, 63], [27, 27, 47], [19, 19, 31], [11, 11, 15],
        [43, 0, 0], [59, 0, 0], [75, 7, 0], [95, 7, 0], [111, 15, 0], [127, 23, 7], [147, 31, 7], [163, 39, 11],
        [183, 51, 15], [195, 75, 27], [207, 99, 43], [219, 127, 59], [227, 151, 79], [231, 171, 95], [239, 191, 119], [247, 211, 139],
        [167, 123, 59], [183, 155, 55], [199, 195, 55], [231, 227, 87], [127, 191, 255], [171, 231, 255], [215, 255, 255], [103, 0, 0],
        [139, 0, 0], [179, 0, 0], [215, 0, 0], [255, 0, 0], [255, 243, 147], [255, 247, 199], [255, 255, 255], [159, 91, 83],
    ];
    P
}
// ---------------------------------------------------------------------------
// Quake MDL
// ---------------------------------------------------------------------------

pub(crate) fn convert_mdl(
    path: &Path,
    rel: &str,
    staged: &Path,
    source: ClassicSource,
) -> Result<ClassicAsset, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let slug = stem_slug(rel);
    let kind = mdl_kind(&slug, rel);
    let decoded = decode_quake_mdl_ex(&bytes, kind == AssetKind::Character)?;
    let folder = match kind {
        AssetKind::Weapon => "weapons",
        AssetKind::Character => "characters",
        AssetKind::Prop => "props",
        _ => "meshes",
    };
    let key = format!("{folder}/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &decoded.glb).map_err(|e| e.to_string())?;
    let mut icon_rel = None;
    if let Some(png) = decoded.icon_png.as_ref() {
        let icon_path = dest.with_extension("png");
        if std::fs::write(&icon_path, png).is_ok() {
            icon_rel = Some(format!("{key}.png"));
        }
    }
    let mut tags = tags_for(kind, &[source.id(), "mdl", &slug]);
    if decoded.frame_count > 1 {
        tags.push("vertex-anim".into());
        tags.push(format!("frames-{}", decoded.frame_count));
    }
    if !decoded.clip_names.is_empty() {
        tags.push("skinned".into());
        for name in &decoded.clip_names {
            tags.push(format!("clip-{name}"));
        }
    }
    Ok(ClassicAsset {
        key,
        kind,
        rel_path,
        tags,
        icon_rel,
    })
}

pub(crate) fn mdl_kind(slug: &str, rel: &str) -> AssetKind {
    let lower = rel.to_ascii_lowercase();
    if lower.contains("weapon")
        || slug.contains("weapon")
        || slug.starts_with("v_")
        || slug.starts_with("g_")
        || slug.starts_with("w_")
    {
        return AssetKind::Weapon;
    }
    if is_character_mdl(slug) {
        return AssetKind::Character;
    }
    if slug.starts_with("h_") || slug.starts_with("gib") {
        return AssetKind::Prop;
    }
    AssetKind::Prop
}

pub(crate) struct DecodedMdl {
    pub glb: Vec<u8>,
    pub icon_png: Option<Vec<u8>>,
    pub frame_count: usize,
    pub clip_names: Vec<String>,
}

/// Turn Quake +X (model forward) toward studio +Z (camera).
fn mdl_face_camera(p: [f32; 3]) -> [f32; 3] {
    let yaw = -std::f32::consts::FRAC_PI_2;
    let (s, c) = (yaw.sin(), yaw.cos());
    [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
}

pub(crate) struct MdlFrame {
    name: String,
    packed: Vec<[u8; 3]>,
}

pub(crate) fn decode_quake_mdl(bytes: &[u8]) -> Result<DecodedMdl, String> {
    decode_quake_mdl_ex(bytes, false)
}

pub(crate) fn decode_quake_mdl_ex(bytes: &[u8], as_character: bool) -> Result<DecodedMdl, String> {
    if bytes.len() < 84 || &bytes[0..4] != b"IDPO" {
        return Err("not a Quake MDL (IDPO)".into());
    }
    let version = i32_le(bytes, 4);
    if version != 6 {
        return Err(format!("unsupported MDL version {version}"));
    }
    // ident, version, scale[3], origin[3], boundingradius, eye[3],
    // num_skins, skinwidth, skinheight, num_verts, num_tris, num_frames
    let num_skins = i32_le(bytes, 48) as usize;
    let skinwidth = i32_le(bytes, 52) as usize;
    let skinheight = i32_le(bytes, 56) as usize;
    let num_verts = i32_le(bytes, 60) as usize;
    let num_tris = i32_le(bytes, 64) as usize;
    let num_frames = i32_le(bytes, 68) as usize;
    let scale = [f32_le(bytes, 8), f32_le(bytes, 12), f32_le(bytes, 16)];
    let origin = [f32_le(bytes, 20), f32_le(bytes, 24), f32_le(bytes, 28)];
    if num_verts == 0 || num_tris == 0 || num_verts > 50_000 || num_tris > 50_000 {
        return Err("MDL mesh bounds invalid".into());
    }
    // Negative i32 header fields sign-extend to huge usize — cap them.
    if num_skins > 1024 || num_frames > 100_000 || skinwidth > 16384 || skinheight > 16384 {
        return Err("MDL header bounds invalid".into());
    }
    let mut off = 84usize;
    let pal = quake_palette();
    let mut skin_rgba = vec![0x80u8; skinwidth.max(1) * skinheight.max(1) * 4];
    for _ in 0..num_skins {
        if off + 4 > bytes.len() {
            return Err("MDL skin truncated".into());
        }
        let group = i32_le(bytes, off);
        off += 4;
        if group == 0 {
            let n = skinwidth * skinheight;
            if off + n > bytes.len() {
                return Err("MDL skin pixels truncated".into());
            }
            skin_rgba = indexed_to_rgba(&bytes[off..off + n], &pal, 255);
            for px in skin_rgba.chunks_exact_mut(4) {
                if px[0] == 255 && px[1] == 0 && px[2] == 255 {
                    px[3] = 0;
                }
            }
            off += n;
        } else {
            if off + 4 > bytes.len() {
                return Err("MDL skin group truncated".into());
            }
            let nb = i32_le(bytes, off) as usize;
            if nb > 1024 {
                return Err("MDL skin group too large".into());
            }
            off += 4 + nb * 4;
            let n = skinwidth * skinheight;
            for gi in 0..nb {
                if off + n > bytes.len() {
                    return Err("MDL group skin truncated".into());
                }
                if gi == 0 {
                    skin_rgba = indexed_to_rgba(&bytes[off..off + n], &pal, 255);
                    for px in skin_rgba.chunks_exact_mut(4) {
                        if px[0] == 255 && px[1] == 0 && px[2] == 255 {
                            px[3] = 0;
                        }
                    }
                }
                off += n;
            }
        }
    }
    if off + num_verts * 12 > bytes.len() {
        return Err("MDL texcoords truncated".into());
    }
    let mut onseam = vec![false; num_verts];
    let mut st = vec![[0.0f32; 2]; num_verts];
    for i in 0..num_verts {
        let o = off + i * 12;
        onseam[i] = i32_le(bytes, o) != 0;
        st[i] = [i32_le(bytes, o + 4) as f32, i32_le(bytes, o + 8) as f32];
    }
    off += num_verts * 12;
    if off + num_tris * 16 > bytes.len() {
        return Err("MDL tris truncated".into());
    }
    let sw = skinwidth.max(1) as f32;
    let sh = skinheight.max(1) as f32;
    let mut corners: Vec<usize> = Vec::with_capacity(num_tris * 3);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(num_tris * 3);
    let mut indices: Vec<u32> = Vec::with_capacity(num_tris * 3);
    for i in 0..num_tris {
        let o = off + i * 16;
        let front = i32_le(bytes, o) != 0;
        for k in 0..3 {
            let vi = i32_le(bytes, o + 4 + k * 4) as usize;
            if vi >= num_verts {
                continue;
            }
            let mut s = st[vi][0];
            if !front && onseam[vi] {
                s += sw * 0.5;
            }
            corners.push(vi);
            uvs.push([s / sw, st[vi][1] / sh]);
            indices.push(indices.len() as u32);
        }
    }
    off += num_tris * 16;

    let mut frames: Vec<MdlFrame> = Vec::new();
    for _ in 0..num_frames {
        if off + 4 > bytes.len() {
            break;
        }
        let ftype = i32_le(bytes, off);
        off += 4;
        if ftype == 0 {
            frames.push(read_mdl_simple_frame(bytes, &mut off, num_verts)?);
        } else {
            if off + 4 > bytes.len() {
                return Err("MDL group frame truncated".into());
            }
            let n = i32_le(bytes, off) as usize;
            off += 4;
            off = off.saturating_add(8); // group bbox min/max
            off = off.saturating_add(n.saturating_mul(4)); // intervals
            for _ in 0..n {
                frames.push(read_mdl_simple_frame(bytes, &mut off, num_verts)?);
            }
        }
    }
    if frames.is_empty() {
        return Err("MDL frames missing".into());
    }

    let unpack = |packed: &[u8; 3]| -> [f32; 3] {
        let x = packed[0] as f32 * scale[0] + origin[0];
        let y = packed[1] as f32 * scale[1] + origin[1];
        let z = packed[2] as f32 * scale[2] + origin[2];
        [x / 32.0, z / 32.0, -y / 32.0]
    };
    let pose_positions = |frame: &MdlFrame| -> Vec<[f32; 3]> {
        corners
            .iter()
            .map(|&vi| {
                frame
                    .packed
                    .get(vi)
                    .map(unpack)
                    .unwrap_or([0.0, 0.0, 0.0])
            })
            .collect()
    };

    let names: Vec<String> = frames.iter().map(|f| f.name.clone()).collect();
    let loop_idx = crate::anim_icon::pick_loop_indices(&names);
    let rest_i = loop_idx.first().copied().unwrap_or(0);
    let rest_pos = pose_positions(&frames[rest_i.min(frames.len() - 1)]);
    // Quake MDLs face +X. Studio / MeshView look from +Z, so −90° puts the
    // chest toward the camera. The card raster looks the other way (−Z
    // closer), so it needs a further 180°.
    let rest_pos: Vec<[f32; 3]> = rest_pos.iter().copied().map(mdl_face_camera).collect();
    let unique_of = |frame: &MdlFrame| -> Vec<[f32; 3]> {
        frame
            .packed
            .iter()
            .map(unpack)
            .map(mdl_face_camera)
            .collect()
    };
    let skin_png = encode_png_rgba(
        &skin_rgba,
        skinwidth.max(1) as u32,
        skinheight.max(1) as u32,
    )?;
    let clips = if as_character && frames.len() > 1 {
        vertex_skin::alias_loco_clips(vertex_skin::group_named_clips(&names))
    } else {
        Vec::new()
    };
    let mut clip_names: Vec<String> = Vec::new();
    let mut glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &rest_pos,
        normals: None,
        uvs: &uvs,
        indices: &indices,
        base_color_png: &skin_png,
        metallic_roughness_png: None,
        double_sided: false,
        colors: None,
    });
    if !clips.is_empty() {
        let unique_rest = unique_of(&frames[rest_i.min(frames.len() - 1)]);
        let unique_frames: Vec<Vec<[f32; 3]>> = frames.iter().map(unique_of).collect();
        match vertex_skin::write_skinned_from_vertex_frames(
            &unique_rest,
            &unique_frames,
            &corners,
            &uvs,
            &indices,
            &clips,
            &skin_png,
        ) {
            Ok(skinned) => {
                glb = skinned;
                clip_names = clips.iter().map(|c| c.name.clone()).collect();
            }
            Err(e) => {
                // Keep the rest-pose statue rather than dropping the asset.
                eprintln!("MDL skin fit failed ({e}); writing rest pose");
            }
        }
    }
    if !glb.starts_with(b"glTF") {
        return Err("MDL GLB write failed".into());
    }

    let yaw = std::f32::consts::PI + 0.35;
    let mut tiles = Vec::new();
    let pick = if loop_idx.is_empty() { vec![rest_i] } else { loop_idx };
    for &fi in &pick {
        let Some(frame) = frames.get(fi) else {
            continue;
        };
        let pos: Vec<[f32; 3]> = pose_positions(frame)
            .into_iter()
            .map(mdl_face_camera)
            .collect();
        tiles.push(crate::anim_icon::raster_mesh_tile(
            &pos,
            &indices,
            &uvs,
            &skin_rgba,
            skinwidth.max(1),
            skinheight.max(1),
            yaw,
        ));
    }
    let icon_png = if tiles.len() >= 2 {
        crate::anim_icon::pack_sheet(&tiles).ok().map(|sheet| sheet.png)
    } else if let Some(tile) = tiles.first() {
        encode_png_rgba(tile, crate::anim_icon::TILE as u32, crate::anim_icon::TILE as u32).ok()
    } else {
        None
    };
    Ok(DecodedMdl {
        glb,
        icon_png,
        frame_count: frames.len(),
        clip_names,
    })
}

pub(crate) fn read_mdl_simple_frame(
    bytes: &[u8],
    off: &mut usize,
    num_verts: usize,
) -> Result<MdlFrame, String> {
    // bbox min (4) + max (4) + name[16] + verts (num_verts * 4)
    if *off + 8 + 16 + num_verts * 4 > bytes.len() {
        return Err("MDL simple frame truncated".into());
    }
    *off += 8;
    let raw = &bytes[*off..*off + 16];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(16);
    let name = String::from_utf8_lossy(&raw[..end]).to_string();
    *off += 16;
    let mut packed = Vec::with_capacity(num_verts);
    for i in 0..num_verts {
        let o = *off + i * 4;
        packed.push([bytes[o], bytes[o + 1], bytes[o + 2]]);
    }
    *off += num_verts * 4;
    Ok(MdlFrame { name, packed })
}

pub(crate) fn quake_mdl_to_glb(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Ok(decode_quake_mdl(bytes)?.glb)
}

// ---------------------------------------------------------------------------
// Quake SPR
// ---------------------------------------------------------------------------

pub(crate) fn convert_spr(
    path: &Path,
    rel: &str,
    staged: &Path,
    source: ClassicSource,
) -> Result<Vec<ClassicAsset>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 36 || &bytes[0..4] != b"IDSP" {
        return Err("not a Quake SPR".into());
    }
    // version, type, radius, maxwidth, maxheight, nframes, beamlength, synctype
    let nframes = i32_le(&bytes, 24) as usize;
    let mut off = 36usize;
    let pal = quake_palette();
    let slug = stem_slug(rel);
    let mut assets = Vec::new();
    for fi in 0..nframes.min(256) {
        if off + 8 > bytes.len() {
            break;
        }
        let group = i32_le(&bytes, off);
        off += 4;
        if group != 0 {
            // group: skip
            if off + 4 > bytes.len() {
                break;
            }
            let nb = i32_le(&bytes, off) as usize;
            off += 4 + nb * 4;
            for _ in 0..nb {
                if off + 16 > bytes.len() {
                    break;
                }
                let w = i32_le(&bytes, off + 8) as usize;
                let h = i32_le(&bytes, off + 12) as usize;
                off += 16 + w * h;
            }
            continue;
        }
        // origin[2], width, height, pixels
        if off + 16 > bytes.len() {
            break;
        }
        let w = i32_le(&bytes, off + 8) as usize;
        let h = i32_le(&bytes, off + 12) as usize;
        off += 16;
        if w == 0 || h == 0 || w > 512 || h > 512 || off + w * h > bytes.len() {
            break;
        }
        let indices = &bytes[off..off + w * h];
        off += w * h;
        let mut rgba = indexed_to_rgba(indices, &pal, 255);
        for px in rgba.chunks_exact_mut(4) {
            if (px[0] == 255 && px[1] == 0 && px[2] == 255)
                || (px[0] == 0 && px[1] == 255 && px[2] == 255)
            {
                px[3] = 0;
            }
        }
        let png = encode_png_rgba(&rgba, w as u32, h as u32)?;
        let key = format!("billboards/{slug}/frame-{fi:02}");
        let rel_path = format!("{key}.png");
        let dest = staged.join(&rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, &png).map_err(|e| e.to_string())?;
        assets.push(ClassicAsset {
            key,
            kind: AssetKind::Billboard,
            rel_path,
            tags: tags_for(
                AssetKind::Billboard,
                &[source.id(), "sprite", "spr", &slug],
            ),
            icon_rel: None,
        });
    }
    if assets.is_empty() {
        return Err("SPR produced no frames".into());
    }
    Ok(assets)
}

// ---------------------------------------------------------------------------
// WAD2 textures
// ---------------------------------------------------------------------------

pub(crate) fn convert_wad2(
    path: &Path,
    rel: &str,
    staged: &Path,
    source: ClassicSource,
) -> Result<Vec<ClassicAsset>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 12 || &bytes[0..4] != b"WAD2" {
        return Err("not WAD2".into());
    }
    let n = u32_le(&bytes, 4) as usize;
    let diroff = u32_le(&bytes, 8) as usize;
    if diroff + n * 32 > bytes.len() {
        return Err("WAD2 directory truncated".into());
    }
    let pal = quake_palette();
    let wad_slug = stem_slug(rel);
    let mut assets = Vec::new();
    for i in 0..n.min(4096) {
        let e = diroff + i * 32;
        let pos = u32_le(&bytes, e) as usize;
        let size = u32_le(&bytes, e + 4) as usize;
        // disksize at +8, type at +12, compression +13, pad +14, name[16] at +16
        let name = lump_name(&bytes[e + 16..e + 32]);
        if pos + size > bytes.len() || size < 40 {
            continue;
        }
        let data = &bytes[pos..pos + size];
        let tw = u32_le(data, 16) as usize;
        let th = u32_le(data, 20) as usize;
        let data_off = u32_le(data, 24) as usize;
        if tw == 0 || th == 0 || tw > 512 || th > 512 {
            continue;
        }
        if data_off + tw * th > data.len() {
            continue;
        }
        if skip_quake_tex(&name) {
            continue;
        }
        let indices = &data[data_off..data_off + tw * th];
        let transparent = name.starts_with('{');
        let mut rgba = Vec::with_capacity(tw * th * 4);
        for &idx in indices {
            if transparent && idx == 255 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let c = pal[idx as usize];
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        let png = encode_png_rgba(&rgba, tw as u32, th as u32)?;
        let key = format!("textures/{wad_slug}/{}", sanitize_slug(&name));
        let rel_path = format!("{key}.png");
        let dest = staged.join(&rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, &png).map_err(|e| e.to_string())?;
        assets.push(ClassicAsset {
            key,
            kind: AssetKind::Texture,
            rel_path,
            tags: tags_for(AssetKind::Texture, &[source.id(), "wad2", &name]),
            icon_rel: None,
        });
    }
    Ok(assets)
}

