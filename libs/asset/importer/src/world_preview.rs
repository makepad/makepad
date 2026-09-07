//! First-person map icon from the player start. GPU studio thumbs of worlds
//! land as studio-clear slate (backfaces / AABB). This rasterises the
//! converted GLB from the spawn sidecar so the card is an actual room.

use crate::classic_import::{decode_png_stored, encode_png_rgba};
use makepad_render::model::{StaticDrawLayer, StaticModel, MODEL_VERTEX_FLOATS};
use std::path::Path;

pub const PREVIEW_SIZE: usize = 256;

pub fn write_spawn_preview(glb: &Path) -> Result<(), String> {
    let bytes = std::fs::read(glb).map_err(|e| e.to_string())?;
    let spawn = read_spawn(&glb.with_extension("spawn")).ok_or("no spawn sidecar")?;
    let png = raster_glb_from_spawn(&bytes, spawn)?;
    std::fs::write(glb.with_extension("png"), png).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn raster_glb_from_spawn(
    glb: &[u8],
    spawn: ([f32; 3], f32, f32),
) -> Result<Vec<u8>, String> {
    let parts = world_parts_from_glb(glb)?;
    let (eye, yaw, pitch) = (spawn.0, spawn.1, spawn.2);
    let pitch = pitch.clamp(-0.6, 0.4);
    let cy = yaw.cos();
    let sy = yaw.sin();
    let cp = pitch.cos();
    let sp = pitch.sin();
    // Same forward as game scene.rs.
    let forward = [sy * cp, sp, -cy * cp];
    let right = [cy, 0.0, sy];
    let up = [
        right[1] * forward[2] - right[2] * forward[1],
        right[2] * forward[0] - right[0] * forward[2],
        right[0] * forward[1] - right[1] * forward[0],
    ];
    let fov = 75.0f32.to_radians();
    let focal = (PREVIEW_SIZE as f32 * 0.5) / (fov * 0.5).tan();
    let mut color = vec![0u8; PREVIEW_SIZE * PREVIEW_SIZE * 4];
    for px in color.chunks_exact_mut(4) {
        px.copy_from_slice(&[26, 31, 41, 255]);
    }
    let mut depth = vec![f32::INFINITY; PREVIEW_SIZE * PREVIEW_SIZE];
    let to_cam = |p: [f32; 3]| -> [f32; 3] {
        let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
        [
            d[0] * right[0] + d[1] * right[1] + d[2] * right[2],
            d[0] * up[0] + d[1] * up[1] + d[2] * up[2],
            d[0] * forward[0] + d[1] * forward[1] + d[2] * forward[2],
        ]
    };

    let project = |c: [f32; 3]| -> Option<[f32; 3]> {
        if c[2] < 0.08 {
            return None;
        }
        let x = PREVIEW_SIZE as f32 * 0.5 + c[0] * focal / c[2];
        let y = PREVIEW_SIZE as f32 * 0.5 - c[1] * focal / c[2];
        Some([x, y, c[2]])
    };

    let sample = |tex: Option<&[u8]>, tw: usize, th: usize, u: f32, v: f32| -> [u8; 4] {
        let Some(tex) = tex else {
            return [255; 4];
        };
        if tw == 0 || th == 0 {
            return [255; 4];
        }
        let wrap = |t: f32| t - t.floor();
        let u = wrap(u);
        let v = wrap(v);
        let x = ((u * tw as f32) as usize).min(tw - 1);
        let y = ((v * th as f32) as usize).min(th - 1);
        let i = (y * tw + x) * 4;
        if i + 3 < tex.len() {
            [tex[i], tex[i + 1], tex[i + 2], tex[i + 3]]
        } else {
            [255; 4]
        }
    };

    for mesh in &parts {
        let tex = mesh.tex.as_deref();
        let (tw, th) = mesh.tex_size;
        let mut t = 0;
        while t + 2 < mesh.indices.len() {
            let ia = mesh.indices[t] as usize;
            let ib = mesh.indices[t + 1] as usize;
            let ic = mesh.indices[t + 2] as usize;
            t += 3;
            if ia >= mesh.pos.len() || ib >= mesh.pos.len() || ic >= mesh.pos.len() {
                continue;
            }
            let ca = to_cam(mesh.pos[ia]);
            let cb = to_cam(mesh.pos[ib]);
            let cc = to_cam(mesh.pos[ic]);
        // Skip triangles entirely behind the near plane.
        if ca[2] < 0.08 && cb[2] < 0.08 && cc[2] < 0.08 {
            continue;
        }
        let Some(pa) = project(ca) else { continue };
        let Some(pb) = project(cb) else { continue };
        let Some(pc) = project(cc) else { continue };
        let ua = mesh.uv.get(ia).copied().unwrap_or([0.0, 0.0]);
        let ub = mesh.uv.get(ib).copied().unwrap_or([0.0, 0.0]);
        let uc = mesh.uv.get(ic).copied().unwrap_or([0.0, 0.0]);
        let ta = mesh.colors.get(ia).copied().unwrap_or([1.0; 3]);
        let tb = mesh.colors.get(ib).copied().unwrap_or([1.0; 3]);
        let tc = mesh.colors.get(ic).copied().unwrap_or([1.0; 3]);
        let minx = pa[0].min(pb[0]).min(pc[0]).floor() as i32;
        let maxx = pa[0].max(pb[0]).max(pc[0]).ceil() as i32;
        let miny = pa[1].min(pb[1]).min(pc[1]).floor() as i32;
        let maxy = pa[1].max(pb[1]).max(pc[1]).ceil() as i32;
        let minx = minx.clamp(0, PREVIEW_SIZE as i32 - 1);
        let maxx = maxx.clamp(0, PREVIEW_SIZE as i32 - 1);
        let miny = miny.clamp(0, PREVIEW_SIZE as i32 - 1);
        let maxy = maxy.clamp(0, PREVIEW_SIZE as i32 - 1);
        let area = edge(pa, pb, pc);
        if area.abs() < 0.5 {
            continue;
        }
        // No backface cull — BSP/Doom interiors are one-sided and winding
        // is not trustworthy from the start camera.
        for y in miny..=maxy {
            for x in minx..=maxx {
                let p = [x as f32 + 0.5, y as f32 + 0.5, 0.0];
                let w0 = edge(pb, pc, p) / area;
                let w1 = edge(pc, pa, p) / area;
                let w2 = edge(pa, pb, p) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = pa[2] * w0 + pb[2] * w1 + pc[2] * w2;
                if z < 0.08 || z > 220.0 {
                    continue;
                }
                let di = y as usize * PREVIEW_SIZE + x as usize;
                if z >= depth[di] {
                    continue;
                }
                let u = ua[0] * w0 + ub[0] * w1 + uc[0] * w2;
                let v = ua[1] * w0 + ub[1] * w1 + uc[1] * w2;
                let mut rgba = sample(tex, tw, th, u, v);
                if rgba[3] < 8 {
                    continue;
                }
                // Cheap distance fade so far walls don't flatten.
                let shade = (1.0 / (1.0 + z * 0.012)).clamp(0.35, 1.0);
                for c in 0..3 {
                    // glTF's material factor and COLOR_0 are linear. The
                    // parser already multiplied them once; the texture is
                    // sRGB, so decode before applying the interpolated tint.
                    let tint = ta[c] * w0 + tb[c] * w1 + tc[c] * w2;
                    let linear = srgb_to_linear(rgba[c] as f32 / 255.0) * tint;
                    rgba[c] = (linear_to_srgb(linear) * 255.0 * shade) as u8;
                }
                depth[di] = z;
                let o = di * 4;
                color[o..o + 4].copy_from_slice(&rgba);
            }
        }
        }
    }
    encode_png_rgba(&color, PREVIEW_SIZE as u32, PREVIEW_SIZE as u32)
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn edge(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    (c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0])
}

fn read_spawn(path: &Path) -> Option<([f32; 3], f32, f32)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if !lines.next()?.starts_with("world-spawn") {
        return None;
    }
    let mut xyz = lines.next()?.split_whitespace();
    let pos = [
        xyz.next()?.parse().ok()?,
        xyz.next()?.parse().ok()?,
        xyz.next()?.parse().ok()?,
    ];
    let mut yp = lines.next()?.split_whitespace();
    Some((pos, yp.next()?.parse().ok()?, yp.next()?.parse().ok()?))
}

pub struct Extracted {
    pub pos: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    /// Linear material base color multiplied by COLOR_0. Empty means white.
    /// The packed renderer's fourth lane can contain AO, so it is not alpha.
    pub colors: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub tex: Option<Vec<u8>>,
    pub tex_size: (usize, usize),
}

/// Same parse as the walk viewer / GPU thumbnailer (`StaticModel::parse_glb`).
/// Falls back to the GLB string scan only when the engine parser cannot
/// produce drawable parts.
fn world_parts_from_glb(glb: &[u8]) -> Result<Vec<Extracted>, String> {
    if let Ok(model) = StaticModel::parse_glb(glb) {
        if !model.draw_layers.is_empty() {
            let parts: Vec<Extracted> = model
                .draw_layers
                .into_iter()
                .filter_map(layer_to_extracted)
                .collect();
            if !parts.is_empty() {
                return Ok(parts);
            }
        } else if let Some(part) = layer_to_extracted(StaticDrawLayer {
            vertices: model.vertices,
            indices: model.indices,
            uvs: Vec::new(),
            texture_png: model.texture_png,
            detail_png: model.detail_png,
            detail_scale: model.detail_scale,
            pbr: model.pbr,
        }) {
            return Ok(vec![part]);
        }
    }
    extract_glb_parts(glb)
}

fn layer_to_extracted(layer: StaticDrawLayer) -> Option<Extracted> {
    let n = layer.vertices.len() / MODEL_VERTEX_FLOATS;
    if n < 3 || layer.indices.len() < 3 {
        return None;
    }
    let mut pos = Vec::with_capacity(n);
    let mut uv = Vec::with_capacity(n);
    let mut colors = Vec::with_capacity(n);
    for i in 0..n {
        let o = i * MODEL_VERTEX_FLOATS;
        pos.push([
            layer.vertices[o],
            layer.vertices[o + 1],
            layer.vertices[o + 2],
        ]);
        uv.push(layer.uvs.get(i).copied().unwrap_or_else(|| {
            let packed = layer.vertices[o + 4].to_bits();
            [
                makepad_platform::f16_bits_to_f32(packed as u16),
                makepad_platform::f16_bits_to_f32((packed >> 16) as u16),
            ]
        }));
        let packed = layer.vertices[o + 5].to_bits();
        colors.push([
            (packed & 0xff) as f32 / 255.0,
            ((packed >> 8) & 0xff) as f32 / 255.0,
            ((packed >> 16) & 0xff) as f32 / 255.0,
        ]);
    }
    let (tex, tex_size) = layer
        .texture_png
        .as_deref()
        .and_then(|png| decode_png_stored(png).ok())
        .map(|(rgba, w, h)| (Some(rgba), (w as usize, h as usize)))
        .unwrap_or((None, (0, 0)));
    Some(Extracted {
        pos,
        uv,
        colors,
        indices: layer.indices,
        tex,
        tex_size,
    })
}

pub fn extract_glb_parts(bytes: &[u8]) -> Result<Vec<Extracted>, String> {
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return Err("not glb".into());
    }
    let mut off = 12usize;
    let mut json = None;
    let mut bin = None;
    while off + 8 <= bytes.len() {
        let n = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let kind = &bytes[off + 4..off + 8];
        if off + 8 + n > bytes.len() {
            break;
        }
        let data = &bytes[off + 8..off + 8 + n];
        if kind == b"JSON" {
            json = Some(data);
        } else if kind.starts_with(b"BIN") {
            bin = Some(data);
        }
        off += 8 + n;
    }
    let json = std::str::from_utf8(json.ok_or("glb missing json")?)
        .map_err(|_| "glb json utf8")?;
    let bin = bin.ok_or("glb missing bin")?;

    let counts = json_counts(json);
    let views = json_views(json);
    let acc_views = json_accessor_views(json);
    let img_views = json_image_views(json);
    let prims = json_primitives(json);
    if views.len() < 4 || counts.len() < 4 {
        return Err("glb missing accessors".into());
    }
    let mut pngs = Vec::new();
    for &vi in &img_views {
        let Some(&(off, len)) = views.get(vi) else {
            pngs.push(None);
            continue;
        };
        if off >= bin.len() {
            pngs.push(None);
            continue;
        }
        let end = (off + len).min(bin.len());
        pngs.push(crate::classic_import::decode_png_stored(&bin[off..end]).ok());
    }
    if pngs.is_empty() {
        let mut search = 0usize;
        while let Some(rel) = bin[search..]
            .windows(8)
            .position(|w| w == b"\x89PNG\r\n\x1a\n")
        {
            let at = search + rel;
            match crate::classic_import::decode_png_stored(&bin[at..]) {
                Ok(decoded) => {
                    pngs.push(Some(decoded));
                    search = at + 16;
                }
                Err(_) => break,
            }
        }
    }
    let mut parts = Vec::new();
    let groups = if prims.is_empty() {
        vec![(0usize, 1usize, 3usize, 0usize)]
    } else {
        prims
    };
    for (idx_acc, pos_acc, uv_acc, mat) in groups {
        let Some(&nidx) = counts.get(idx_acc) else {
            continue;
        };
        let Some(&npos) = counts.get(pos_acc) else {
            continue;
        };
        let nuv = counts.get(uv_acc).copied().unwrap_or(npos);
        let view_of = |acc: usize| -> Option<(usize, usize)> {
            let vi = *acc_views.get(acc)?;
            views.get(vi).copied()
        };
        let Some((ioff, _)) = view_of(idx_acc) else {
            continue;
        };
        let Some((poff, _)) = view_of(pos_acc) else {
            continue;
        };
        let Some((uoff, _)) = view_of(uv_acc) else {
            continue;
        };
        let mut indices = Vec::with_capacity(nidx);
        for i in 0..nidx {
            let o = ioff + i * 4;
            if o + 4 > bin.len() {
                break;
            }
            indices.push(u32::from_le_bytes(bin[o..o + 4].try_into().unwrap()));
        }
        let mut pos = Vec::with_capacity(npos);
        for i in 0..npos {
            let o = poff + i * 12;
            if o + 12 > bin.len() {
                break;
            }
            pos.push([
                f32::from_le_bytes(bin[o..o + 4].try_into().unwrap()),
                f32::from_le_bytes(bin[o + 4..o + 8].try_into().unwrap()),
                f32::from_le_bytes(bin[o + 8..o + 12].try_into().unwrap()),
            ]);
        }
        let mut uv = Vec::with_capacity(nuv);
        for i in 0..nuv {
            let o = uoff + i * 8;
            if o + 8 > bin.len() {
                break;
            }
            uv.push([
                f32::from_le_bytes(bin[o..o + 4].try_into().unwrap()),
                f32::from_le_bytes(bin[o + 4..o + 8].try_into().unwrap()),
            ]);
        }
        let (tex, tex_size) = pngs
            .get(mat)
            .and_then(|t| t.as_ref())
            .map(|(rgba, w, h)| (Some(rgba.clone()), (*w as usize, *h as usize)))
            .unwrap_or((None, (0, 0)));
        if pos.len() >= 3 && indices.len() >= 3 {
            parts.push(Extracted {
                pos,
                uv,
                colors: Vec::new(),
                indices,
                tex,
                tex_size,
            });
        }
    }
    if parts.is_empty() {
        return Err("glb mesh empty".into());
    }
    Ok(parts)
}

fn json_primitives(json: &str) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let Some(start) = json.find("\"primitives\"") else {
        return out;
    };
    let rest = &json[start..];
    let end = rest
        .find("\"materials\"")
        .or_else(|| rest.find("\"accessors\""))
        .unwrap_or(rest.len());
    let body = &rest[..end];
    for chunk in body.split("{\"attributes\"") {
        if !chunk.contains("POSITION") {
            continue;
        }
        let pos = json_key_usize(chunk, "\"POSITION\":").unwrap_or(1);
        let uv = json_key_usize(chunk, "\"TEXCOORD_0\":").unwrap_or(3);
        let idx = json_key_usize(chunk, "\"indices\":").unwrap_or(0);
        let mat = json_key_usize(chunk, "\"material\":").unwrap_or(0);
        out.push((idx, pos, uv, mat));
    }
    out
}

fn json_accessor_views(json: &str) -> Vec<usize> {
    let Some(start) = json.find("\"accessors\"") else {
        return Vec::new();
    };
    let rest = &json[start..];
    let end = rest.find("\"bufferViews\"").unwrap_or(rest.len());
    json_all_key_usize(&rest[..end], "\"bufferView\":")
}

fn json_image_views(json: &str) -> Vec<usize> {
    let Some(start) = json.find("\"images\"") else {
        return Vec::new();
    };
    json_all_key_usize(&json[start..], "\"bufferView\":")
}

fn json_all_key_usize(s: &str, key: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(at) = rest.find(key) {
        rest = &rest[at + key.len()..];
        let n: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(v) = n.parse() {
            out.push(v);
        }
    }
    out
}

fn json_key_usize(s: &str, key: &str) -> Option<usize> {
    let at = s.find(key)?;
    let rest = &s[at + key.len()..];
    let n: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    n.parse().ok()
}

fn json_counts(json: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = json;
    while let Some(at) = rest.find("\"count\":") {
        rest = &rest[at + 8..];
        let n: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(v) = n.parse() {
            out.push(v);
        }
    }
    out
}

fn json_views(json: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut rest = json;
    while let Some(at) = rest.find("\"byteOffset\":") {
        rest = &rest[at + 13..];
        let off: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let mut after = rest;
        let len = if let Some(p) = after.find("\"byteLength\":") {
            after = &after[p + 13..];
            after
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        } else {
            0
        };
        if let Ok(off) = off.parse() {
            out.push((off, len));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_gltf::{
        write_glb_mesh_textured, write_glb_mesh_textured_parts, write_glb_named_parts,
        GlbNamedPart, GlbTexturedMesh, GlbTexturedPart,
    };
    use std::path::Path;

    #[test]
    #[ignore]
    fn render_local_map_previews() {
        let roots = [
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../local/ai_content_app/import/librequake/work/source/worlds"),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../local/ai_content_app/import/freedoom/work/source/worlds"),
        ];
        let mut n = 0usize;
        for root in roots {
            if !root.is_dir() {
                continue;
            }
            let mut stack = vec![root];
            while let Some(dir) = stack.pop() {
                for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                        continue;
                    }
                    if p.extension().and_then(|x| x.to_str()) != Some("glb") {
                        continue;
                    }
                    if !p.with_extension("spawn").is_file() {
                        continue;
                    }
                    match write_spawn_preview(&p) {
                        Ok(()) => {
                            n += 1;
                            eprintln!("preview {}", p.display());
                        }
                        Err(e) => eprintln!("preview fail {}: {e}", p.display()),
                    }
                }
            }
        }
        eprintln!("wrote {n} spawn previews");
        assert!(n > 0);
    }

    #[test]
    fn spawn_view_sees_a_textured_quad() {
        let mut rgba = vec![0u8; 4 * 4 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[200, 40, 40, 255]);
        }
        let png = crate::classic_import::encode_png_rgba(&rgba, 4, 4).unwrap();
        // Yaw 0 looks −Z (engine convention).
        let positions = [
            [-1.0, -1.0, -4.0],
            [1.0, -1.0, -4.0],
            [1.0, 1.0, -4.0],
            [-1.0, 1.0, -4.0],
        ];
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let indices = [0u32, 1, 2, 0, 2, 3];
        let glb = write_glb_mesh_textured(&GlbTexturedMesh {
            positions: &positions,
            normals: None,
            uvs: &uvs,
            indices: &indices,
            base_color_png: &png,
            metallic_roughness_png: None,
            double_sided: true,
            colors: None,
        });
        let out = raster_glb_from_spawn(&glb, ([0.0, 0.0, 0.0], 0.0, 0.0)).unwrap();
        assert!(out.starts_with(b"\x89PNG"));
        let (px, w, h) = crate::classic_import::decode_png_stored(&out).unwrap();
        assert_eq!((w, h), (PREVIEW_SIZE as u32, PREVIEW_SIZE as u32));
        let reds = px.chunks_exact(4).filter(|p| p[0] > 120 && p[1] < 80).count();
        assert!(reds > 200, "expected the red quad in view, got {reds} red texels");
    }

    const QUAD_UVS: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

    fn quad(center_x: f32) -> [[f32; 3]; 4] {
        [
            [center_x - 0.8, -0.8, -4.0],
            [center_x + 0.8, -0.8, -4.0],
            [center_x + 0.8, 0.8, -4.0],
            [center_x - 0.8, 0.8, -4.0],
        ]
    }

    fn textured_part<'a>(positions: &'a [[f32; 3]], png: &'a [u8], factor: [f32; 4]) -> GlbTexturedPart<'a> {
        GlbTexturedPart {
            positions,
            uvs: &QUAD_UVS,
            indices: &QUAD_INDICES,
            base_color_png: png,
            normals: None,
            base_color_factor: Some(factor),
            colors: None,
            lightmap_png: None,
            lightmap_uvs: None,
            detail_png: None,
            detail_scale: [1.0; 2],
        }
    }

    fn preview_pixels(glb: &[u8]) -> Vec<u8> {
        let png = raster_glb_from_spawn(glb, ([0.0; 3], 0.0, 0.0)).unwrap();
        decode_png_stored(&png).unwrap().0
    }

    fn assert_pixel(pixels: &[u8], x: usize, y: usize, expected: [u8; 3]) {
        let pixel = &pixels[(y * PREVIEW_SIZE + x) * 4..][..3];
        for c in 0..3 {
            assert!(pixel[c].abs_diff(expected[c]) <= 2, "pixel ({x}, {y}) = {pixel:?}, expected {expected:?}");
        }
    }

    #[test]
    fn spawn_view_multiplies_texture_factor_and_vertex_color_once_in_linear_space() {
        // Left and right texels differ, so this also checks UV extraction
        // from the merged packed stream rather than silently using [0, 0].
        let png = encode_png_rgba(&[128, 192, 64, 255, 64, 128, 192, 255], 2, 1).unwrap();
        let positions = quad(0.0);
        let colors = [[0.5, 1.0, 0.5]; 4];
        let mut part = textured_part(&positions, &png, [0.5, 0.25, 1.0, 1.0]);
        part.colors = Some(&colors);
        let glb = write_glb_mesh_textured_parts(&[part], true);
        assert!(StaticModel::parse_glb(&glb).unwrap().draw_layers.is_empty());
        let extracted = world_parts_from_glb(&glb).unwrap();
        for tint in &extracted[0].colors {
            for (actual, expected) in tint.iter().zip([0.25, 0.25, 0.5]) {
                assert!((actual - expected).abs() <= 1.0 / 255.0);
            }
        }
        let pixels = preview_pixels(&glb);
        // Includes the existing distance fade at z=4. These values would
        // differ substantially if the factor were lost, applied twice, or
        // multiplied directly into the gamma-encoded texture bytes.
        assert_pixel(&pixels, 112, 128, [63, 97, 42]);
        assert_pixel(&pixels, 144, 128, [28, 63, 133]);
    }

    #[test]
    fn spawn_view_preserves_distinct_material_tints_in_split_texture_layers() {
        let left_png = encode_png_rgba(&[200, 120, 40, 255], 1, 1).unwrap();
        let right_png = encode_png_rgba(&[255; 4], 1, 1).unwrap();
        let left = quad(-1.0);
        let right = quad(1.0);
        let glb = write_glb_mesh_textured_parts(&[
            textured_part(&left, &left_png, [0.25, 0.0, 1.0, 1.0]),
            textured_part(&right, &right_png, [0.0, 0.5, 1.0, 1.0]),
        ], true);
        assert_eq!(StaticModel::parse_glb(&glb).unwrap().draw_layers.len(), 2);
        let pixels = preview_pixels(&glb);
        assert_pixel(&pixels, 86, 128, [101, 0, 38]);
        assert_pixel(&pixels, 170, 128, [0, 179, 243]);
    }

    #[test]
    fn spawn_view_keeps_untextured_iris_pupil_and_eye_white_colors() {
        let positions = [quad(-2.0), quad(0.0), quad(2.0)];
        let colors = [[0.0, 0.25, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0], [1.0; 4]];
        let parts: Vec<_> = positions.iter().zip(colors).map(|(positions, color)| GlbNamedPart {
            name: "eye part",
            positions,
            indices: &QUAD_INDICES,
            pivot: [0.0; 3],
            color,
            parent: None,
            animation: None,
        }).collect();
        let glb = write_glb_named_parts(&parts);
        let extracted = world_parts_from_glb(&glb).unwrap();
        assert!(extracted.iter().all(|part| part.tex.is_none()));
        let pixels = preview_pixels(&glb);
        // The named-part writer emits its color as both baseColorFactor
        // and COLOR_0, so the GLB's green value is 0.25 * 0.25.
        assert_pixel(&pixels, 44, 128, [0, 67, 243]);
        assert_pixel(&pixels, 128, 128, [0, 0, 0]);
        assert_pixel(&pixels, 211, 128, [243; 3]);
    }
}
