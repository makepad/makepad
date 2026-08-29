//! Minimal GLB writer.
//!
//! Produces a valid glTF 2.0 binary container with a single mesh primitive
//! (positions + smooth vertex normals + u32 indices, TRIANGLES). No
//! materials, UVs or textures — intended for geometry pipelines (e.g.
//! makepad-remesh / trellis mesh output). Normals are computed here
//! (area-weighted from the faces) so every consumer shades the mesh
//! correctly without its own fallback path.
//!
//! [`write_glb_mesh_skinned`] extends the container with a skeleton (node
//! hierarchy + skin + inverse bind matrices) and any number of named
//! animation clips (translation/rotation/scale channels, LINEAR samplers) —
//! the character-chain output format. The acceptance contract is our own
//! engine parser (`libs/render/src/skin.rs`, round-trip tested there)
//! plus standard glTF viewers, so the writer emits everything the spec
//! validator requires (POSITION and animation-input min/max, scene graph
//! reachability for every joint, VEC4 u8/u16 joints, f32 weights).

/// Area-weighted smooth vertex normals from triangle faces. Degenerate
/// vertices (no face contribution) get +Y so the data stays normalized.
pub fn compute_vertex_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for tri in indices.chunks_exact(3) {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if a >= positions.len() || b >= positions.len() || c >= positions.len() {
            continue;
        }
        let (pa, pb, pc) = (positions[a], positions[b], positions[c]);
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        // cross(u, v): magnitude = 2x face area, so summing unnormalized
        // cross products area-weights the smooth normal for free.
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for &i in &[a, b, c] {
            normals[i][0] += n[0];
            normals[i][1] += n[1];
            normals[i][2] += n[2];
        }
    }
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-20 {
            n[0] /= len;
            n[1] /= len;
            n[2] /= len;
        } else {
            *n = [0.0, 1.0, 0.0];
        }
    }
    normals
}

/// Serialize positions + triangle indices into a GLB byte vector with
/// computed smooth vertex normals.
pub fn write_glb_mesh(positions: &[[f32; 3]], indices: &[u32]) -> Vec<u8> {
    write_glb_mesh_colored(positions, indices, None)
}

/// [`write_glb_mesh`] with optional per-vertex COLOR_0 (linear RGB 0..=1,
/// written as FLOAT VEC3 — universally readable, no normalized-int decode
/// required of consumers). `colors` must match `positions` in length.
pub fn write_glb_mesh_colored(
    positions: &[[f32; 3]],
    indices: &[u32],
    colors: Option<&[[f32; 3]]>,
) -> Vec<u8> {
    assert!(indices.len() % 3 == 0, "indices must be triangles");
    if let Some(colors) = colors {
        assert!(
            colors.len() == positions.len(),
            "colors must match positions"
        );
    }
    let normals = compute_vertex_normals(positions, indices);

    // ---- BIN chunk: indices, positions, normals[, colors] (4-byte aligned) ----
    let idx_bytes = indices.len() * 4;
    let pos_bytes = positions.len() * 12;
    let nrm_bytes = normals.len() * 12;
    let col_bytes = colors.map_or(0, |c| c.len() * 12);
    let mut bin = Vec::with_capacity(idx_bytes + pos_bytes + nrm_bytes + col_bytes);
    for &i in indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    for p in positions {
        for &c in p {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    for n in &normals {
        for &c in n {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    if let Some(colors) = colors {
        for col in colors {
            for &c in col {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }

    // POSITION accessor min/max are required by the spec
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in positions {
        for d in 0..3 {
            mn[d] = mn[d].min(p[d]);
            mx[d] = mx[d].max(p[d]);
        }
    }
    if positions.is_empty() {
        mn = [0.0; 3];
        mx = [0.0; 3];
    }

    let fmt_f = |v: f32| {
        // shortest roundtrip formatting keeps the JSON exact and compact
        let mut s = format!("{v}");
        if !s.contains('.') && !s.contains('e') && !s.contains("inf") && !s.contains("NaN") {
            s.push_str(".0");
        }
        s
    };
    let color_attr = if colors.is_some() { ",\"COLOR_0\":3" } else { "" };
    let color_accessor = if let Some(colors) = colors {
        format!(
            ",{{\"bufferView\":3,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\"}}",
            colors.len()
        )
    } else {
        String::new()
    };
    let color_view = if colors.is_some() {
        format!(
            ",{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}}",
            idx_bytes + pos_bytes + nrm_bytes,
            col_bytes
        )
    } else {
        String::new()
    };
    let json = format!(
        concat!(
            "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"makepad-gltf-writer\"}},",
            "\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"mesh\":0}}],",
            "\"meshes\":[{{\"primitives\":[{{\"attributes\":{{\"POSITION\":1,\"NORMAL\":2{}}},\"indices\":0,\"mode\":4}}]}}],",
            "\"accessors\":[",
            "{{\"bufferView\":0,\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}},",
            "{{\"bufferView\":1,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\",",
            "\"min\":[{},{},{}],\"max\":[{},{},{}]}},",
            "{{\"bufferView\":2,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\"}}{}],",
            "\"bufferViews\":[",
            "{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":{},\"target\":34963}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}}{}],",
            "\"buffers\":[{{\"byteLength\":{}}}]}}"
        ),
        color_attr,
        indices.len(),
        positions.len(),
        fmt_f(mn[0]),
        fmt_f(mn[1]),
        fmt_f(mn[2]),
        fmt_f(mx[0]),
        fmt_f(mx[1]),
        fmt_f(mx[2]),
        normals.len(),
        color_accessor,
        idx_bytes,
        idx_bytes,
        pos_bytes,
        idx_bytes + pos_bytes,
        nrm_bytes,
        color_view,
        bin.len(),
    );
    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }

    // ---- GLB container ----
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
    out.extend_from_slice(&bin);
    out
}

/// POSITION + NORMAL + TEXCOORD_0, no embedded images. Hunyuan paint
/// consumes this as the retexture atlas.
pub fn write_glb_mesh_unwrapped(
    positions: &[[f32; 3]],
    normals: Option<&[[f32; 3]]>,
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Vec<u8> {
    assert!(indices.len() % 3 == 0, "indices must be triangles");
    assert!(uvs.len() == positions.len(), "uvs must match positions");
    let normals: Vec<[f32; 3]> = match normals {
        Some(n) => {
            assert!(n.len() == positions.len(), "normals must match positions");
            n.to_vec()
        }
        None => compute_vertex_normals(positions, indices),
    };
    let idx_bytes = indices.len() * 4;
    let pos_bytes = positions.len() * 12;
    let nrm_bytes = normals.len() * 12;
    let uv_bytes = uvs.len() * 8;
    let mut bin = Vec::with_capacity(idx_bytes + pos_bytes + nrm_bytes + uv_bytes);
    for &i in indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    for p in positions {
        for &c in p {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    for n in &normals {
        for &c in n {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    for uv in uvs {
        for &c in uv {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in positions {
        for d in 0..3 {
            mn[d] = mn[d].min(p[d]);
            mx[d] = mx[d].max(p[d]);
        }
    }
    if positions.is_empty() {
        mn = [0.0; 3];
        mx = [0.0; 3];
    }
    let fmt_f = |v: f32| {
        let mut s = format!("{v}");
        if !s.contains('.') && !s.contains('e') && !s.contains("inf") && !s.contains("NaN") {
            s.push_str(".0");
        }
        s
    };
    let json = format!(
        concat!(
            "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"makepad-gltf-writer\"}},",
            "\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"mesh\":0}}],",
            "\"meshes\":[{{\"primitives\":[{{\"attributes\":{{\"POSITION\":1,\"NORMAL\":2,\"TEXCOORD_0\":3}},\"indices\":0,\"mode\":4}}]}}],",
            "\"accessors\":[",
            "{{\"bufferView\":0,\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}},",
            "{{\"bufferView\":1,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\",",
            "\"min\":[{},{},{}],\"max\":[{},{},{}]}},",
            "{{\"bufferView\":2,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\"}},",
            "{{\"bufferView\":3,\"componentType\":5126,\"count\":{},\"type\":\"VEC2\"}}],",
            "\"bufferViews\":[",
            "{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":{},\"target\":34963}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}}],",
            "\"buffers\":[{{\"byteLength\":{}}}]}}"
        ),
        indices.len(),
        positions.len(),
        fmt_f(mn[0]),
        fmt_f(mn[1]),
        fmt_f(mn[2]),
        fmt_f(mx[0]),
        fmt_f(mx[1]),
        fmt_f(mx[2]),
        normals.len(),
        uvs.len(),
        idx_bytes,
        idx_bytes,
        pos_bytes,
        idx_bytes + pos_bytes,
        nrm_bytes,
        idx_bytes + pos_bytes + nrm_bytes,
        uv_bytes,
        bin.len(),
    );
    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

/// Input for [`write_glb_mesh_textured`]: a UV-unwrapped mesh with baked
/// PBR textures (pre-encoded PNG bytes — the writer stays codec-free).
pub struct GlbTexturedMesh<'a> {
    pub positions: &'a [[f32; 3]],
    /// Per-vertex normals; None = computed area-weighted from the faces.
    /// Chart-unwrapped meshes should pass pre-split normals mapped through
    /// the vertex split, or shading seams appear along chart borders.
    pub normals: Option<&'a [[f32; 3]]>,
    /// TEXCOORD_0 per vertex, [0,1], glTF convention (v down, matching a
    /// top-to-bottom encoded atlas).
    pub uvs: &'a [[f32; 2]],
    pub indices: &'a [u32],
    /// RGBA base color atlas, PNG bytes.
    pub base_color_png: &'a [u8],
    /// glTF metallic-roughness atlas (G=roughness, B=metallic), PNG bytes.
    pub metallic_roughness_png: Option<&'a [u8]>,
    pub double_sided: bool,
    /// Optional linear RGB vertex colors (COLOR_0). Used to carry Quake II
    /// face lightmaps without a second texture.
    pub colors: Option<&'a [[f32; 3]]>,
}

/// Serialize a UV-mapped mesh with embedded PNG textures into a GLB:
/// POSITION + computed smooth NORMAL + TEXCOORD_0, one PBR material with
/// baseColorTexture (+ optional metallicRoughnessTexture), images embedded
/// in the BIN chunk.
pub fn write_glb_mesh_textured(mesh: &GlbTexturedMesh) -> Vec<u8> {
    assert!(mesh.indices.len() % 3 == 0, "indices must be triangles");
    assert!(
        mesh.uvs.len() == mesh.positions.len(),
        "uvs must match positions"
    );
    let normals: Vec<[f32; 3]> = match mesh.normals {
        Some(n) => {
            assert!(n.len() == mesh.positions.len(), "normals must match positions");
            n.to_vec()
        }
        None => compute_vertex_normals(mesh.positions, mesh.indices),
    };

    // ---- BIN: indices, positions, normals, uvs, base png[, mr png] ----
    if let Some(c) = mesh.colors {
        assert!(c.len() == mesh.positions.len(), "colors must match positions");
    }
    let idx_bytes = mesh.indices.len() * 4;
    let pos_bytes = mesh.positions.len() * 12;
    let nrm_bytes = normals.len() * 12;
    let uv_bytes = mesh.uvs.len() * 8;
    let color_bytes = if mesh.colors.is_some() {
        mesh.positions.len() * 12
    } else {
        0
    };
    let mut bin = Vec::with_capacity(
        idx_bytes
            + pos_bytes
            + nrm_bytes
            + uv_bytes
            + color_bytes
            + mesh.base_color_png.len()
            + 64,
    );
    for &i in mesh.indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    for p in mesh.positions {
        for &c in p {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    for n in &normals {
        for &c in n {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    for uv in mesh.uvs {
        for &c in uv {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    if let Some(cols) = mesh.colors {
        for c in cols {
            for &ch in c {
                let v = if ch.is_finite() { ch.clamp(0.0, 1.0) } else { 1.0 };
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let base_png_offset = bin.len();
    bin.extend_from_slice(mesh.base_color_png);
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let mr_png_offset = bin.len();
    if let Some(mr) = mesh.metallic_roughness_png {
        bin.extend_from_slice(mr);
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
    }

    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in mesh.positions {
        for d in 0..3 {
            mn[d] = mn[d].min(p[d]);
            mx[d] = mx[d].max(p[d]);
        }
    }
    if mesh.positions.is_empty() {
        mn = [0.0; 3];
        mx = [0.0; 3];
    }
    let fmt_f = |v: f32| {
        let mut s = format!("{v}");
        if !s.contains('.') && !s.contains('e') && !s.contains("inf") && !s.contains("NaN") {
            s.push_str(".0");
        }
        s
    };

    let has_mr = mesh.metallic_roughness_png.is_some();
    let has_color = mesh.colors.is_some();
    let color_view = if has_color { 4 } else { 0 };
    let png_view = if has_color { 5 } else { 4 };
    let mr_view = if has_color { 6 } else { 5 };
    let color_attr = if has_color {
        format!(",\"COLOR_0\":{color_view}")
    } else {
        String::new()
    };
    let color_acc = if has_color {
        format!(
            ",{{\"bufferView\":{color_view},\"componentType\":5126,\"count\":{},\"type\":\"VEC3\"}}",
            mesh.positions.len()
        )
    } else {
        String::new()
    };
    let color_view_json = if has_color {
        format!(
            ",{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}}",
            idx_bytes + pos_bytes + nrm_bytes + uv_bytes,
            color_bytes
        )
    } else {
        String::new()
    };
    let mr_texture_json = if has_mr {
        ",\"metallicRoughnessTexture\":{\"index\":1}"
    } else {
        ""
    };
    let mr_image_json = if has_mr {
        format!(
            ",{{\"bufferView\":{mr_view},\"mimeType\":\"image/png\"}}"
        )
    } else {
        String::new()
    };
    let mr_texture_list = if has_mr { ",{\"source\":1,\"sampler\":0}" } else { "" };
    let mr_view_json = if has_mr {
        format!(
            ",{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{}}}",
            mr_png_offset,
            mesh.metallic_roughness_png.map_or(0, |m| m.len())
        )
    } else {
        String::new()
    };
    let json = format!(
        concat!(
            "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"makepad-gltf-writer\"}},",
            "\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"mesh\":0}}],",
            "\"meshes\":[{{\"primitives\":[{{\"attributes\":{{\"POSITION\":1,\"NORMAL\":2,\"TEXCOORD_0\":3{}}}",
            ",\"indices\":0,\"material\":0,\"mode\":4}}]}}],",
            "\"materials\":[{{\"pbrMetallicRoughness\":{{\"baseColorTexture\":{{\"index\":0}}{}",
            ",\"metallicFactor\":{},\"roughnessFactor\":1.0}},\"doubleSided\":{}}}],",
            "\"textures\":[{{\"source\":0,\"sampler\":0}}{}],",
            "\"samplers\":[{{\"magFilter\":9729,\"minFilter\":9729,\"wrapS\":33071,\"wrapT\":33071}}],",
            "\"images\":[{{\"bufferView\":{},\"mimeType\":\"image/png\"}}{}],",
            "\"accessors\":[",
            "{{\"bufferView\":0,\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}},",
            "{{\"bufferView\":1,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\",",
            "\"min\":[{},{},{}],\"max\":[{},{},{}]}},",
            "{{\"bufferView\":2,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\"}},",
            "{{\"bufferView\":3,\"componentType\":5126,\"count\":{},\"type\":\"VEC2\"}}{}],",
            "\"bufferViews\":[",
            "{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":{},\"target\":34963}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}}{}",
            ",{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{}}}{}],",
            "\"buffers\":[{{\"byteLength\":{}}}]}}"
        ),
        color_attr,
        mr_texture_json,
        if has_mr { "1.0" } else { "0.0" },
        mesh.double_sided,
        mr_texture_list,
        png_view,
        mr_image_json,
        mesh.indices.len(),
        mesh.positions.len(),
        fmt_f(mn[0]),
        fmt_f(mn[1]),
        fmt_f(mn[2]),
        fmt_f(mx[0]),
        fmt_f(mx[1]),
        fmt_f(mx[2]),
        normals.len(),
        mesh.uvs.len(),
        color_acc,
        idx_bytes,
        idx_bytes,
        pos_bytes,
        idx_bytes + pos_bytes,
        nrm_bytes,
        idx_bytes + pos_bytes + nrm_bytes,
        uv_bytes,
        color_view_json,
        base_png_offset,
        mesh.base_color_png.len(),
        mr_view_json,
        bin.len(),
    );
    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
    out.extend_from_slice(&bin);
    out
}

/// One textured triangle set with its own base-color PNG. UVs may go
/// outside 0..1; the shared sampler repeats.
pub struct GlbTexturedPart<'a> {
    pub positions: &'a [[f32; 3]],
    pub uvs: &'a [[f32; 2]],
    pub indices: &'a [u32],
    pub base_color_png: &'a [u8],
    /// When set, used as-is (one normal per position). Otherwise computed
    /// from triangle windings.
    pub normals: Option<&'a [[f32; 3]]>,
    /// Optional material baseColorFactor. Lets many parts share one image
    /// and vary only by tint (e.g. BUILD shade levels).
    pub base_color_factor: Option<[f32; 4]>,
    /// Optional linear RGB vertex colors (COLOR_0). Tiling albedo stays on
    /// TEXCOORD_0; this carries per-vertex lightmaps / baked light.
    pub colors: Option<&'a [[f32; 3]]>,
    /// Optional shipped lightmap (Q3). Paired with [`lightmap_uvs`]
    /// (`TEXCOORD_1`). The viewer applies it on load; albedo keeps tiling.
    pub lightmap_png: Option<&'a [u8]>,
    pub lightmap_uvs: Option<&'a [[f32; 2]]>,
    /// Optional Q3 / Unreal detail overlay. Sampled at `TEXCOORD_0 * scale`
    /// and multiplied `2 * dest * src` (identity at mean 0.5).
    pub detail_png: Option<&'a [u8]>,
    pub detail_scale: [f32; 2],
}

/// One authored rigid part for [`write_glb_named_parts`]. Positions are in
/// model space; the writer stores them relative to `pivot` and puts that
/// pivot on the matching glTF node. This preserves the author's useful
/// articulation origin without changing the rendered model-space geometry.
pub struct GlbNamedPart<'a> {
    pub name: &'a str,
    pub positions: &'a [[f32; 3]],
    pub indices: &'a [u32],
    pub pivot: [f32; 3],
    pub color: [f32; 4],
    /// Index in this slice. A parent must precede its child.
    pub parent: Option<usize>,
    /// Optional idle-motion manifest. The writer also emits a standards-based
    /// glTF clip so the engine and ordinary viewers share one asset truth.
    pub animation: Option<GlbPartAnimation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlbPartAnimationKind { Swing, Spin, Bob }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlbPartAnimation {
    pub kind: GlbPartAnimationKind,
    /// 0=x, 1=y, 2=z.
    pub axis: usize,
    pub degrees: f32,
    pub hz: f32,
    pub amp: f32,
}

/// Serialize rigid named parts as one glTF node + mesh + material per part.
///
/// This is deliberately a structural writer: it does not merge authored
/// parts into primitives of one mesh, because node names and pivots are part
/// of the model-program asset contract.
pub fn write_glb_named_parts(parts: &[GlbNamedPart]) -> Vec<u8> {
    let mut b = GlbBinBuilder::new();
    let mut nodes = Vec::new();
    let mut meshes = Vec::new();
    let mut materials = Vec::new();
    let mut animations = Vec::new();
    let mut children = vec![Vec::<usize>::new(); parts.len()];
    for (index, part) in parts.iter().enumerate() {
        if let Some(parent) = part.parent {
            assert!(parent < index, "part parent must precede child");
            children[parent].push(index);
        }
    }

    for part in parts {
        if part.positions.is_empty() || part.indices.len() < 3 {
            continue;
        }
        assert!(part.indices.len() % 3 == 0, "indices must be triangles");
        assert!(
            part.indices.iter().all(|&index| index < part.positions.len() as u32),
            "part index out of range"
        );

        let local_positions: Vec<[f32; 3]> = part
            .positions
            .iter()
            .map(|p| {
                [
                    p[0] - part.pivot[0],
                    p[1] - part.pivot[1],
                    p[2] - part.pivot[2],
                ]
            })
            .collect();
        let normals = compute_vertex_normals(&local_positions, part.indices);

        let mut index_bytes = Vec::with_capacity(part.indices.len() * 4);
        for &index in part.indices {
            index_bytes.extend_from_slice(&index.to_le_bytes());
        }
        let index_accessor = b.accessor(
            &index_bytes,
            5125,
            part.indices.len(),
            "SCALAR",
            None,
            Some(34963),
        );
        let mut position_values = Vec::with_capacity(local_positions.len() * 3);
        for position in &local_positions {
            position_values.extend_from_slice(position);
        }
        let position_accessor =
            b.f32_accessor(&position_values, 3, "VEC3", true, Some(34962));
        let mut normal_values = Vec::with_capacity(normals.len() * 3);
        for normal in &normals {
            normal_values.extend_from_slice(normal);
        }
        let normal_accessor =
            b.f32_accessor(&normal_values, 3, "VEC3", false, Some(34962));
        let mut color_values = Vec::with_capacity(local_positions.len() * 3);
        for _ in &local_positions {
            color_values.extend_from_slice(&part.color[..3]);
        }
        let color_accessor =
            b.f32_accessor(&color_values, 3, "VEC3", false, Some(34962));

        let mesh_index = meshes.len();
        let material_index = materials.len();
        meshes.push(format!(
            "{{\"name\":\"{}\",\"primitives\":[{{\"attributes\":{{\"POSITION\":{position_accessor},\"NORMAL\":{normal_accessor},\"COLOR_0\":{color_accessor}}},\"indices\":{index_accessor},\"material\":{material_index},\"mode\":4}}]}}",
            json_escape(part.name)
        ));
        let [r, g, blue, a] = part.color.map(|v| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                1.0
            }
        });
        materials.push(format!(
            "{{\"name\":\"{}\",\"pbrMetallicRoughness\":{{\"baseColorFactor\":[{r},{g},{blue},{a}],\"metallicFactor\":0.0,\"roughnessFactor\":0.72}}}}",
            json_escape(part.name)
        ));
        let translation = match part.parent {
            Some(parent) => [
                part.pivot[0] - parts[parent].pivot[0],
                part.pivot[1] - parts[parent].pivot[1],
                part.pivot[2] - parts[parent].pivot[2],
            ],
            None => part.pivot,
        };
        let children_json = if children[mesh_index].is_empty() {
            String::new()
        } else {
            format!(
                ",\"children\":[{}]",
                children[mesh_index].iter().map(usize::to_string).collect::<Vec<_>>().join(",")
            )
        };
        let extras_json = part.animation.map_or_else(String::new, |animation| {
            let kind = match animation.kind {
                GlbPartAnimationKind::Swing => "swing",
                GlbPartAnimationKind::Spin => "spin",
                GlbPartAnimationKind::Bob => "bob",
            };
            let axis = ["x", "y", "z"][animation.axis.min(2)];
            format!(
                ",\"extras\":{{\"states\":[\"idle-a\",\"idle-b\"],\"default\":\"idle-a\",\"kind\":\"localgen-{kind}\",\"axis\":\"{axis}\",\"degrees\":{},\"hz\":{},\"amp\":{}}}",
                fmt_f32(animation.degrees), fmt_f32(animation.hz), fmt_f32(animation.amp)
            )
        });
        nodes.push(format!(
            "{{\"name\":\"{}\",\"mesh\":{mesh_index},\"translation\":[{},{},{}]{children_json}{extras_json}}}",
            json_escape(part.name),
            fmt_f32(translation[0]),
            fmt_f32(translation[1]),
            fmt_f32(translation[2]),
        ));

        if let Some(animation) = part.animation {
            let hz = animation.hz.max(0.001);
            let (times, values, path, width) = match animation.kind {
                GlbPartAnimationKind::Swing => {
                    let times = vec![0.0, 0.25 / hz, 0.75 / hz, 1.0 / hz];
                    let quaternion = |degrees: f32| {
                        let half = degrees.to_radians() * 0.5;
                        let mut value = [0.0, 0.0, 0.0, half.cos()];
                        value[animation.axis.min(2)] = half.sin();
                        value
                    };
                    let mut values = Vec::new();
                    for degrees in [0.0, animation.degrees, -animation.degrees, 0.0] {
                        values.extend_from_slice(&quaternion(degrees));
                    }
                    (times, values, "rotation", 4)
                }
                GlbPartAnimationKind::Spin => {
                    let times = vec![0.0, 0.5 / hz, 1.0 / hz];
                    let mut values = Vec::new();
                    for degrees in [0.0f32, 180.0, 360.0] {
                        let half = degrees.to_radians() * 0.5;
                        let mut value = [0.0, 0.0, 0.0, half.cos()];
                        value[animation.axis.min(2)] = half.sin();
                        values.extend_from_slice(&value);
                    }
                    (times, values, "rotation", 4)
                }
                GlbPartAnimationKind::Bob => {
                    let times = vec![0.0, 0.25 / hz, 0.75 / hz, 1.0 / hz];
                    let mut values = Vec::new();
                    for offset in [0.0, animation.amp, -animation.amp, 0.0] {
                        let mut value = translation;
                        value[animation.axis.min(2)] += offset;
                        values.extend_from_slice(&value);
                    }
                    (times, values, "translation", 3)
                }
            };
            let input = b.f32_accessor(&times, 1, "SCALAR", true, None);
            let output = b.f32_accessor(
                &values,
                width,
                if width == 4 { "VEC4" } else { "VEC3" },
                false,
                None,
            );
            animations.push(format!(
                "{{\"name\":\"{}\",\"samplers\":[{{\"input\":{input},\"output\":{output},\"interpolation\":\"LINEAR\"}}],\"channels\":[{{\"sampler\":0,\"target\":{{\"node\":{mesh_index},\"path\":\"{path}\"}}}}]}}",
                json_escape(part.name)
            ));
        }
    }

    if nodes.is_empty() {
        return write_glb_mesh(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            &[0, 1, 2],
        );
    }
    let scene_nodes = parts.iter().enumerate()
        .filter(|(_, part)| part.parent.is_none())
        .map(|(index, _)| index.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let animations_json = if animations.is_empty() {
        String::new()
    } else {
        format!("\"animations\":[{}],", animations.join(","))
    };
    let json = format!(
        concat!(
            "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"makepad-gltf-writer\"}},",
            "\"scene\":0,\"scenes\":[{{\"nodes\":[{}]}}],",
            "\"nodes\":[{}],\"meshes\":[{}],\"materials\":[{}],{}",
            "\"accessors\":[{}],\"bufferViews\":[{}],",
            "\"buffers\":[{{\"byteLength\":{}}}]}}"
        ),
        scene_nodes,
        nodes.join(","),
        meshes.join(","),
        materials.join(","),
        animations_json,
        b.accessors.join(","),
        b.views.join(","),
        b.bin.len(),
    );
    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let mut bin = b.bin;
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

/// Several textured parts as one mesh (one primitive + material each).
/// Identical image bytes are stored once and shared. Sampler is linear +
/// repeat so wall/floor UVs can tile.
pub fn write_glb_mesh_textured_parts(parts: &[GlbTexturedPart], double_sided: bool) -> Vec<u8> {
    if parts.is_empty() {
        return write_glb_mesh(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            &[0, 1, 2],
        );
    }
    let mut b = GlbBinBuilder::new();
    let mut prims = Vec::new();
    let mut materials = Vec::new();
    let mut textures = Vec::new();
    let mut images = Vec::new();
    let mut image_by_bytes: std::collections::HashMap<&[u8], usize> =
        std::collections::HashMap::new();
    for part in parts {
        if part.indices.len() < 3 || part.positions.is_empty() {
            continue;
        }
        let computed;
        let normals: &[[f32; 3]] = if let Some(n) = part.normals {
            if n.len() == part.positions.len() {
                n
            } else {
                computed = compute_vertex_normals(part.positions, part.indices);
                &computed
            }
        } else {
            computed = compute_vertex_normals(part.positions, part.indices);
            &computed
        };
        let mut idx_bytes = Vec::with_capacity(part.indices.len() * 4);
        for &i in part.indices {
            idx_bytes.extend_from_slice(&i.to_le_bytes());
        }
        let idx_acc = b.accessor(&idx_bytes, 5125, part.indices.len(), "SCALAR", None, Some(34963));
        let mut pos = Vec::with_capacity(part.positions.len() * 3);
        for p in part.positions {
            pos.extend_from_slice(p);
        }
        let pos_acc = b.f32_accessor(&pos, 3, "VEC3", true, Some(34962));
        let mut nrm = Vec::with_capacity(normals.len() * 3);
        for n in normals {
            nrm.extend_from_slice(n);
        }
        let nrm_acc = b.f32_accessor(&nrm, 3, "VEC3", false, Some(34962));
        let mut uvs = Vec::with_capacity(part.uvs.len() * 2);
        for uv in part.uvs {
            uvs.extend_from_slice(uv);
        }
        let uv_acc = b.f32_accessor(&uvs, 2, "VEC2", false, Some(34962));
        let color_attr = match part.colors {
            Some(cols) if cols.len() == part.positions.len() => {
                let mut cvals = Vec::with_capacity(cols.len() * 3);
                for c in cols {
                    for &ch in c {
                        cvals.push(if ch.is_finite() { ch.clamp(0.0, 1.0) } else { 1.0 });
                    }
                }
                let acc = b.f32_accessor(&cvals, 3, "VEC3", false, Some(34962));
                format!(",\"COLOR_0\":{acc}")
            }
            _ => String::new(),
        };
        let lm_attr = match (part.lightmap_png, part.lightmap_uvs) {
            (Some(_png), Some(uvs)) if uvs.len() == part.positions.len() => {
                let mut vals = Vec::with_capacity(uvs.len() * 2);
                for uv in uvs {
                    vals.extend_from_slice(uv);
                }
                let acc = b.f32_accessor(&vals, 2, "VEC2", false, Some(34962));
                format!(",\"TEXCOORD_1\":{acc}")
            }
            _ => String::new(),
        };
        let ti = *image_by_bytes.entry(part.base_color_png).or_insert_with(|| {
            let img_view = b.view(part.base_color_png, None);
            let ii = images.len();
            images.push(format!("{{\"bufferView\":{img_view},\"mimeType\":\"image/png\"}}"));
            textures.push(format!("{{\"source\":{ii},\"sampler\":0}}"));
            ii
        });
        let lmi = part.lightmap_png.map(|png| {
            *image_by_bytes.entry(png).or_insert_with(|| {
                let img_view = b.view(png, None);
                let ii = images.len();
                images.push(format!("{{\"bufferView\":{img_view},\"mimeType\":\"image/png\"}}"));
                textures.push(format!("{{\"source\":{ii},\"sampler\":0}}"));
                ii
            })
        });
        let dti = match part.detail_png {
            Some(png) if part.detail_scale[0].abs() > 1e-4 || part.detail_scale[1].abs() > 1e-4 => {
                Some(*image_by_bytes.entry(png).or_insert_with(|| {
                    let img_view = b.view(png, None);
                    let ii = images.len();
                    images.push(format!("{{\"bufferView\":{img_view},\"mimeType\":\"image/png\"}}"));
                    textures.push(format!("{{\"source\":{ii},\"sampler\":0}}"));
                    ii
                }))
            }
            _ => None,
        };
        let mi = materials.len();
        let factor = match part.base_color_factor {
            Some([r, g, bl, a]) if part.base_color_factor != Some([1.0, 1.0, 1.0, 1.0]) => {
                format!("\"baseColorFactor\":[{r},{g},{bl},{a}],")
            }
            _ => String::new(),
        };
        let extras = {
            let mut bits = Vec::new();
            if let Some(i) = lmi {
                bits.push(format!("\"lightmapTexture\":{{\"index\":{i},\"texCoord\":1}}"));
            }
            if let Some(i) = dti {
                bits.push(format!(
                    "\"detailTexture\":{{\"index\":{i},\"texCoord\":0,\"scale\":[{},{}]}}",
                    part.detail_scale[0], part.detail_scale[1]
                ));
            }
            if bits.is_empty() {
                String::new()
            } else {
                format!(",\"extras\":{{{}}}", bits.join(","))
            }
        };
        materials.push(format!(
            "{{\"pbrMetallicRoughness\":{{{factor}\"baseColorTexture\":{{\"index\":{ti}}},\"metallicFactor\":0.0,\"roughnessFactor\":1.0}},\"doubleSided\":{double_sided}{extras}}}"
        ));
        prims.push(format!(
            "{{\"attributes\":{{\"POSITION\":{pos_acc},\"NORMAL\":{nrm_acc},\"TEXCOORD_0\":{uv_acc}{color_attr}{lm_attr}}},\"indices\":{idx_acc},\"material\":{mi},\"mode\":4}}"
        ));
    }
    if prims.is_empty() {
        return write_glb_mesh(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            &[0, 1, 2],
        );
    }
    let json = format!(
        concat!(
            "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"makepad-gltf-writer\"}},",
            "\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"mesh\":0}}],",
            "\"meshes\":[{{\"primitives\":[{}]}}],",
            "\"materials\":[{}],",
            "\"textures\":[{}],",
            "\"samplers\":[{{\"magFilter\":9729,\"minFilter\":9987,\"wrapS\":10497,\"wrapT\":10497}}],",
            "\"images\":[{}],",
            "\"accessors\":[{}],",
            "\"bufferViews\":[{}],",
            "\"buffers\":[{{\"byteLength\":{}}}]}}"
        ),
        prims.join(","),
        materials.join(","),
        textures.join(","),
        images.join(","),
        b.accessors.join(","),
        b.views.join(","),
        b.bin.len(),
    );
    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    // The BIN chunk must be zero-padded to 4 bytes (the trailing image blob
    // has arbitrary length); the JSON buffer byteLength stays unpadded,
    // which the GLB spec allows.
    let mut bin = b.bin;
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

// ------------------------------------------------------------ skinned+anim

/// One skeleton joint for [`write_glb_mesh_skinned`]. Joints are written as glTF
/// nodes 1..=N (node 0 is the mesh node); `parent` indexes into the same
/// joints slice (`None` = skeleton root). The rest pose is the node TRS; the
/// inverse bind matrix is column-major, mesh space → joint space at bind.
pub struct GlbJoint {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    /// Quaternion, glTF order XYZW.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Column-major 4x4.
    pub inverse_bind: [f32; 16],
}

impl GlbJoint {
    /// A joint at a rest translation with identity rotation/scale and an
    /// inverse bind that undoes its GLOBAL bind translation (the common case
    /// for translation-only rest skeletons).
    pub fn at(name: &str, parent: Option<usize>, translation: [f32; 3], global_bind: [f32; 3]) -> Self {
        let mut inverse_bind = [0.0f32; 16];
        inverse_bind[0] = 1.0;
        inverse_bind[5] = 1.0;
        inverse_bind[10] = 1.0;
        inverse_bind[15] = 1.0;
        inverse_bind[12] = -global_bind[0];
        inverse_bind[13] = -global_bind[1];
        inverse_bind[14] = -global_bind[2];
        GlbJoint {
            name: name.to_string(),
            parent,
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlbAnimPath {
    Translation,
    Rotation,
    Scale,
}

/// One animation channel: keyframes for one TRS path of one joint.
pub struct GlbAnimChannel {
    /// Index into the joints slice (NOT the glTF node index).
    pub joint: usize,
    pub path: GlbAnimPath,
    /// Key times, seconds, ascending.
    pub times: Vec<f32>,
    /// 3 floats per key (translation/scale) or 4 (rotation, XYZW).
    pub values: Vec<f32>,
}

/// A named clip. Channels sharing an identical `times` vector share one
/// input accessor in the output (the writer dedupes by content).
pub struct GlbAnimClip {
    pub name: String,
    pub channels: Vec<GlbAnimChannel>,
}

/// Input for [`write_glb_mesh_skinned`].
pub struct GlbSkinnedMesh<'a> {
    pub positions: &'a [[f32; 3]],
    /// None = computed area-weighted from the faces.
    pub normals: Option<&'a [[f32; 3]]>,
    /// Optional TEXCOORD_0 (required if `base_color_png` is set).
    pub uvs: Option<&'a [[f32; 2]]>,
    pub indices: &'a [u32],
    /// Per-vertex joint indices (into `joints`), 4 influences.
    pub joints_0: &'a [[u16; 4]],
    /// Per-vertex weights; should sum to 1 (consumers renormalize).
    pub weights_0: &'a [[f32; 4]],
    pub joints: &'a [GlbJoint],
    pub clips: &'a [GlbAnimClip],
    /// Optional RGBA base color texture, pre-encoded PNG bytes.
    pub base_color_png: Option<&'a [u8]>,
}

/// Format an f32 for JSON (finite shortest-roundtrip, integer values get
/// a `.0` so the value stays a JSON number readers treat as float-ish).
fn fmt_f32(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let mut s = format!("{v}");
    if !s.contains('.') && !s.contains('e') {
        s.push_str(".0");
    }
    s
}

fn fmt_f32_list(vals: &[f32]) -> String {
    let mut s = String::new();
    for (i, v) in vals.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&fmt_f32(*v));
    }
    s
}

/// JSON-escape a name (control chars are dropped — glTF names are labels).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// Accumulates the BIN chunk plus the bufferViews/accessors JSON fragments.
struct GlbBinBuilder {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl GlbBinBuilder {
    fn new() -> Self {
        GlbBinBuilder { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() }
    }
    fn align4(&mut self) {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
    }
    /// Add a bufferView over `bytes` (4-aligned), returns the view index.
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align4();
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let target = target.map_or(String::new(), |t| format!(",\"target\":{t}"));
        self.views.push(format!(
            "{{\"buffer\":0,\"byteOffset\":{offset},\"byteLength\":{}{target}}}",
            bytes.len()
        ));
        self.views.len() - 1
    }
    /// Add an accessor over its own tightly-packed view, returns accessor index.
    fn accessor(
        &mut self,
        bytes: &[u8],
        component_type: u32,
        count: usize,
        ty: &str,
        min_max: Option<(String, String)>,
        target: Option<u32>,
    ) -> usize {
        let view = self.view(bytes, target);
        let mm = min_max.map_or(String::new(), |(mn, mx)| {
            format!(",\"min\":[{mn}],\"max\":[{mx}]")
        });
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":{component_type},\"count\":{count},\"type\":\"{ty}\"{mm}}}"
        ));
        self.accessors.len() - 1
    }
    fn f32_accessor(
        &mut self,
        vals: &[f32],
        lanes: usize,
        ty: &str,
        with_min_max: bool,
        target: Option<u32>,
    ) -> usize {
        let mut bytes = Vec::with_capacity(vals.len() * 4);
        for v in vals {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let count = vals.len() / lanes;
        let mm = with_min_max.then(|| {
            let mut mn = vec![f32::INFINITY; lanes];
            let mut mx = vec![f32::NEG_INFINITY; lanes];
            for chunk in vals.chunks_exact(lanes) {
                for (d, v) in chunk.iter().enumerate() {
                    mn[d] = mn[d].min(*v);
                    mx[d] = mx[d].max(*v);
                }
            }
            if count == 0 {
                mn = vec![0.0; lanes];
                mx = vec![0.0; lanes];
            }
            (fmt_f32_list(&mn), fmt_f32_list(&mx))
        });
        self.accessor(&bytes, 5126, count, ty, mm, target)
    }
}

/// Serialize a skinned mesh + skeleton + named animation clips into a GLB.
///
/// Node layout: node 0 = mesh node (mesh 0 + skin 0, identity transform),
/// nodes 1..=J = joints in input order. The scene lists the mesh node and
/// every root joint, so all joints are reachable (spec requirement).
/// Samplers are LINEAR; channels sharing a times vector share an input
/// accessor. Panics on inconsistent input lengths (programmer error).
pub fn write_glb_mesh_skinned(mesh: &GlbSkinnedMesh) -> Vec<u8> {
    let vcount = mesh.positions.len();
    assert!(mesh.indices.len() % 3 == 0, "indices must be triangles");
    assert!(mesh.joints_0.len() == vcount, "joints_0 must match positions");
    assert!(mesh.weights_0.len() == vcount, "weights_0 must match positions");
    if let Some(n) = mesh.normals {
        assert!(n.len() == vcount, "normals must match positions");
    }
    if let Some(uv) = mesh.uvs {
        assert!(uv.len() == vcount, "uvs must match positions");
    }
    assert!(
        mesh.base_color_png.is_none() || mesh.uvs.is_some(),
        "base_color_png requires uvs"
    );
    assert!(!mesh.joints.is_empty(), "skin needs at least one joint");
    for j in mesh.joints {
        if let Some(p) = j.parent {
            assert!(p < mesh.joints.len(), "joint parent out of range");
        }
    }
    for clip in mesh.clips {
        for ch in &clip.channels {
            assert!(ch.joint < mesh.joints.len(), "channel joint out of range");
            let lanes = if ch.path == GlbAnimPath::Rotation { 4 } else { 3 };
            assert!(
                ch.values.len() == ch.times.len() * lanes,
                "channel values must be times*{lanes}"
            );
            assert!(!ch.times.is_empty(), "channel needs at least one key");
        }
    }

    let normals: Vec<[f32; 3]> = match mesh.normals {
        Some(n) => n.to_vec(),
        None => compute_vertex_normals(mesh.positions, mesh.indices),
    };

    let mut b = GlbBinBuilder::new();

    // Geometry accessors.
    let mut idx_bytes = Vec::with_capacity(mesh.indices.len() * 4);
    for i in mesh.indices {
        idx_bytes.extend_from_slice(&i.to_le_bytes());
    }
    let acc_indices = b.accessor(&idx_bytes, 5125, mesh.indices.len(), "SCALAR", None, Some(34963));
    let flat3 = |v: &[[f32; 3]]| -> Vec<f32> { v.iter().flatten().copied().collect() };
    let acc_pos = b.f32_accessor(&flat3(mesh.positions), 3, "VEC3", true, Some(34962));
    let acc_nrm = b.f32_accessor(&flat3(&normals), 3, "VEC3", false, Some(34962));
    let acc_uv = mesh.uvs.map(|uvs| {
        let flat: Vec<f32> = uvs.iter().flatten().copied().collect();
        b.f32_accessor(&flat, 2, "VEC2", false, Some(34962))
    });
    // JOINTS_0: u8 lanes when the rig fits (engine palette limit is 256).
    let acc_joints = if mesh.joints.len() <= 256 {
        let mut bytes = Vec::with_capacity(vcount * 4);
        for j4 in mesh.joints_0 {
            for j in j4 {
                debug_assert!((*j as usize) < mesh.joints.len(), "vertex joint out of range");
                bytes.push(*j as u8);
            }
        }
        b.accessor(&bytes, 5121, vcount, "VEC4", None, Some(34962))
    } else {
        let mut bytes = Vec::with_capacity(vcount * 8);
        for j4 in mesh.joints_0 {
            for j in j4 {
                bytes.extend_from_slice(&j.to_le_bytes());
            }
        }
        b.accessor(&bytes, 5123, vcount, "VEC4", None, Some(34962))
    };
    let weights_flat: Vec<f32> = mesh.weights_0.iter().flatten().copied().collect();
    let acc_weights = b.f32_accessor(&weights_flat, 4, "VEC4", false, Some(34962));
    let ibm_flat: Vec<f32> = mesh.joints.iter().flat_map(|j| j.inverse_bind).collect();
    let acc_ibm = b.f32_accessor(&ibm_flat, 16, "MAT4", false, None);

    // Animations: dedupe input (times) accessors per clip by content.
    let mut animations_json = Vec::with_capacity(mesh.clips.len());
    for clip in mesh.clips {
        let mut time_accs: Vec<(&[f32], usize)> = Vec::new();
        let mut samplers = Vec::with_capacity(clip.channels.len());
        let mut channels = Vec::with_capacity(clip.channels.len());
        for (ci, ch) in clip.channels.iter().enumerate() {
            let input = match time_accs.iter().find(|(t, _)| *t == ch.times.as_slice()) {
                Some((_, acc)) => *acc,
                None => {
                    // Spec requires min/max on animation input accessors.
                    let acc = b.f32_accessor(&ch.times, 1, "SCALAR", true, None);
                    time_accs.push((&ch.times, acc));
                    acc
                }
            };
            let (ty, path) = match ch.path {
                GlbAnimPath::Rotation => ("VEC4", "rotation"),
                GlbAnimPath::Translation => ("VEC3", "translation"),
                GlbAnimPath::Scale => ("VEC3", "scale"),
            };
            let output = b.f32_accessor(&ch.values, if ch.path == GlbAnimPath::Rotation { 4 } else { 3 }, ty, false, None);
            samplers.push(format!(
                "{{\"input\":{input},\"output\":{output},\"interpolation\":\"LINEAR\"}}"
            ));
            channels.push(format!(
                "{{\"sampler\":{ci},\"target\":{{\"node\":{},\"path\":\"{path}\"}}}}",
                ch.joint + 1
            ));
        }
        animations_json.push(format!(
            "{{\"name\":\"{}\",\"samplers\":[{}],\"channels\":[{}]}}",
            json_escape(&clip.name),
            samplers.join(","),
            channels.join(",")
        ));
    }

    // Optional texture (image bytes live at the end of BIN).
    let material_blocks = mesh.base_color_png.map(|png| {
        let img_view = b.view(png, None);
        (
            "\"materials\":[{\"pbrMetallicRoughness\":{\"baseColorTexture\":{\"index\":0},\"metallicFactor\":0.0,\"roughnessFactor\":1.0}}],\
             \"textures\":[{\"source\":0,\"sampler\":0}],\
             \"samplers\":[{\"magFilter\":9729,\"minFilter\":9729,\"wrapS\":33071,\"wrapT\":33071}],"
                .to_string(),
            format!("\"images\":[{{\"bufferView\":{img_view},\"mimeType\":\"image/png\"}}],"),
        )
    });
    b.align4();

    // Nodes: 0 = mesh node, 1..=J = joints.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); mesh.joints.len()];
    let mut roots = Vec::new();
    for (ji, j) in mesh.joints.iter().enumerate() {
        match j.parent {
            Some(p) => children[p].push(ji + 1),
            None => roots.push(ji + 1),
        }
    }
    let mut nodes_json = Vec::with_capacity(mesh.joints.len() + 1);
    nodes_json.push("{\"name\":\"skinned_mesh\",\"mesh\":0,\"skin\":0}".to_string());
    for (ji, j) in mesh.joints.iter().enumerate() {
        let kids = if children[ji].is_empty() {
            String::new()
        } else {
            format!(
                ",\"children\":[{}]",
                children[ji].iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")
            )
        };
        nodes_json.push(format!(
            "{{\"name\":\"{}\",\"translation\":[{}],\"rotation\":[{}],\"scale\":[{}]{kids}}}",
            json_escape(&j.name),
            fmt_f32_list(&j.translation),
            fmt_f32_list(&j.rotation),
            fmt_f32_list(&j.scale),
        ));
    }
    let scene_nodes: Vec<String> = std::iter::once(0usize)
        .chain(roots.iter().copied())
        .map(|n| n.to_string())
        .collect();
    let joint_list: Vec<String> = (1..=mesh.joints.len()).map(|n| n.to_string()).collect();

    let mut attrs = format!("\"POSITION\":{acc_pos},\"NORMAL\":{acc_nrm}");
    if let Some(uv) = acc_uv {
        attrs.push_str(&format!(",\"TEXCOORD_0\":{uv}"));
    }
    attrs.push_str(&format!(",\"JOINTS_0\":{acc_joints},\"WEIGHTS_0\":{acc_weights}"));
    let material_ref = if material_blocks.is_some() { ",\"material\":0" } else { "" };
    let (mat_json, img_json) = material_blocks.unwrap_or((String::new(), String::new()));
    let animations = if animations_json.is_empty() {
        String::new()
    } else {
        format!("\"animations\":[{}],", animations_json.join(","))
    };
    // skeleton = first root joint (optional per spec, helps some viewers).
    let json = format!(
        "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"makepad-gltf-writer\"}},\
         \"scene\":0,\"scenes\":[{{\"nodes\":[{scene}]}}],\
         \"nodes\":[{nodes}],\
         \"skins\":[{{\"joints\":[{joints}],\"inverseBindMatrices\":{acc_ibm},\"skeleton\":{skeleton}}}],\
         \"meshes\":[{{\"primitives\":[{{\"attributes\":{{{attrs}}},\"indices\":{acc_indices}{material_ref},\"mode\":4}}]}}],\
         {animations}{mat_json}{img_json}\
         \"accessors\":[{accessors}],\
         \"bufferViews\":[{views}],\
         \"buffers\":[{{\"byteLength\":{bin_len}}}]}}",
        scene = scene_nodes.join(","),
        nodes = nodes_json.join(","),
        joints = joint_list.join(","),
        skeleton = roots.first().copied().unwrap_or(1),
        accessors = b.accessors.join(","),
        views = b.views.join(","),
        bin_len = b.bin.len(),
    );

    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + b.bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(b.bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
    out.extend_from_slice(&b.bin);
    out
}

#[cfg(test)]
mod tests {
    use super::{write_glb_mesh, write_glb_mesh_colored};
    use crate::buffer::decode_mesh_primitive;
    use crate::loader::load_gltf_from_bytes;

    #[test]
    fn roundtrip() {
        let positions = vec![[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.5], [0.0, 1.0, -0.25]];
        let indices = vec![0u32, 1, 2];
        let glb = write_glb_mesh(&positions, &indices);
        let loaded = load_gltf_from_bytes(&glb, None).expect("parse own output");
        let prim = decode_mesh_primitive(&loaded, 0, 0).expect("decode own output");
        assert_eq!(prim.positions, positions);
        assert_eq!(prim.indices, indices);
        assert!(prim.colors0.is_none());
    }

    #[test]
    fn textured_parts_use_trilinear_repeat_sampler() {
        use super::{write_glb_mesh_textured_parts, GlbTexturedPart};
        let positions = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let uvs = [[0.0f32, 0.0], [2.0, 0.0], [0.0, 2.0]];
        let indices = [0u32, 1, 2];
        let glb = write_glb_mesh_textured_parts(
            &[GlbTexturedPart {
                positions: &positions,
                uvs: &uvs,
                indices: &indices,
                base_color_png: b"PNGBYTES_TILE",
                normals: None,
                base_color_factor: None,
                colors: None,
                lightmap_png: None,
                lightmap_uvs: None,
                detail_png: None,
                detail_scale: [0.0, 0.0],
            }],
            true,
        );
        let json = String::from_utf8_lossy(&glb);
        assert!(
            json.contains("\"minFilter\":9987"),
            "tiling world parts need LINEAR_MIPMAP_LINEAR, got a slice of: {}",
            &json[..json.len().min(800)]
        );
        assert!(json.contains("\"wrapS\":10497"), "tiling world parts need REPEAT");
        assert!(json.contains("\"wrapT\":10497"), "tiling world parts need REPEAT");
    }

    #[test]
    fn roundtrip_textured() {
        use super::{write_glb_mesh_textured, GlbTexturedMesh};
        let positions = vec![[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let uvs = vec![[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let indices = vec![0u32, 1, 2];
        // Not a real PNG — the writer must not care (codec-free container).
        let base_png = b"PNGBYTES_BASE".to_vec();
        let mr_png = b"PNGBYTES_MR".to_vec();
        let glb = write_glb_mesh_textured(&GlbTexturedMesh {
            positions: &positions,
            normals: None,
            uvs: &uvs,
            indices: &indices,
            base_color_png: &base_png,
            metallic_roughness_png: Some(&mr_png),
            double_sided: true,
            colors: None,
        });
        let loaded = load_gltf_from_bytes(&glb, None).expect("parse own output");
        let prim = decode_mesh_primitive(&loaded, 0, 0).expect("decode own output");
        assert_eq!(prim.positions, positions);
        assert_eq!(prim.texcoords0.as_deref(), Some(&uvs[..]));
        assert_eq!(prim.indices, indices);
        assert_eq!(prim.material, Some(0));
        // Both images decode back byte-exact from their bufferViews.
        let img0 = crate::image::load_image_bytes(&loaded, 0).expect("base image");
        assert_eq!(img0, base_png.as_slice());
        let img1 = crate::image::load_image_bytes(&loaded, 1).expect("mr image");
        assert_eq!(img1, mr_png.as_slice());
    }

    #[test]
    fn textured_parts_write_detail_extras() {
        use super::{write_glb_mesh_textured_parts, GlbTexturedPart};
        let positions = vec![[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let uvs = vec![[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let indices = vec![0u32, 1, 2];
        let glb = write_glb_mesh_textured_parts(
            &[GlbTexturedPart {
                positions: &positions,
                uvs: &uvs,
                indices: &indices,
                base_color_png: b"PNGBYTES_BASE",
                normals: None,
                base_color_factor: None,
                colors: None,
                lightmap_png: None,
                lightmap_uvs: None,
                detail_png: Some(b"PNGBYTES_DETAIL"),
                detail_scale: [2.9, 2.234],
            }],
            true,
        );
        let json = {
            let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
            std::str::from_utf8(&glb[20..20 + json_len]).unwrap()
        };
        assert!(
            json.contains("detailTexture"),
            "material extras must name the overlay"
        );
        assert!(json.contains("2.9"), "scale x: {json}");
        assert!(json.contains("2.234"), "scale y: {json}");
        let loaded = load_gltf_from_bytes(&glb, None).expect("parse");
        let img1 = crate::image::load_image_bytes(&loaded, 1).expect("detail image");
        assert_eq!(img1, b"PNGBYTES_DETAIL");
    }

    #[test]
    fn roundtrip_skinned() {
        use super::{
            write_glb_mesh_skinned, GlbAnimChannel, GlbAnimClip, GlbAnimPath, GlbJoint,
            GlbSkinnedMesh,
        };
        // Two-joint chain, four vertices, two named clips. The full skinning
        // acceptance test (sampling + palettes through the engine parser)
        // lives in libs/render; this pins the container structure via
        // our own reader.
        let positions = vec![
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        let indices = vec![0u32, 1, 2, 2, 1, 3];
        let joints_0 = vec![[0u16, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0]];
        let weights_0 = vec![
            [1.0f32, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.75, 0.25, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
        ];
        let joints = vec![
            GlbJoint::at("root", None, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
            GlbJoint::at("tip", Some(0), [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        ];
        let clips = vec![
            GlbAnimClip {
                name: "walk".into(),
                channels: vec![
                    GlbAnimChannel {
                        joint: 1,
                        path: GlbAnimPath::Rotation,
                        times: vec![0.0, 0.5, 1.0],
                        values: vec![
                            0.0, 0.0, 0.0, 1.0, //
                            0.0, 0.0, 0.7071, 0.7071, //
                            0.0, 0.0, 0.0, 1.0,
                        ],
                    },
                    GlbAnimChannel {
                        joint: 0,
                        path: GlbAnimPath::Translation,
                        times: vec![0.0, 0.5, 1.0],
                        values: vec![0.0, 0.0, 0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 0.0],
                    },
                ],
            },
            GlbAnimClip {
                name: "idle".into(),
                channels: vec![GlbAnimChannel {
                    joint: 1,
                    path: GlbAnimPath::Scale,
                    times: vec![0.0, 2.0],
                    values: vec![1.0, 1.0, 1.0, 1.0, 1.1, 1.0],
                }],
            },
        ];
        let glb = write_glb_mesh_skinned(&GlbSkinnedMesh {
            positions: &positions,
            normals: None,
            uvs: None,
            indices: &indices,
            joints_0: &joints_0,
            weights_0: &weights_0,
            joints: &joints,
            clips: &clips,
            base_color_png: None,
        });
        // Container + geometry survive our own reader.
        let loaded = load_gltf_from_bytes(&glb, None).expect("parse own output");
        let prim = decode_mesh_primitive(&loaded, 0, 0).expect("decode own output");
        assert_eq!(prim.positions, positions);
        assert_eq!(prim.indices, indices);
        // The JSON chunk carries the skin + both named clips (the reader
        // keeps skins/animations raw, so check the source text).
        let json_len =
            u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).expect("json utf8");
        for needle in [
            "\"skins\":[{\"joints\":[1,2]",
            "\"JOINTS_0\":",
            "\"WEIGHTS_0\":",
            "\"inverseBindMatrices\":",
            "\"name\":\"walk\"",
            "\"name\":\"idle\"",
            "\"path\":\"rotation\"",
            "\"interpolation\":\"LINEAR\"",
        ] {
            assert!(json.contains(needle), "missing {needle} in {json}");
        }
    }

    #[test]
    fn roundtrip_colors() {
        let positions = vec![[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.5], [0.0, 1.0, -0.25]];
        let indices = vec![0u32, 1, 2];
        let colors = vec![[1.0f32, 0.0, 0.25], [0.0, 0.5, 1.0], [0.125, 1.0, 0.0]];
        let glb = write_glb_mesh_colored(&positions, &indices, Some(&colors));
        let loaded = load_gltf_from_bytes(&glb, None).expect("parse own output");
        let prim = decode_mesh_primitive(&loaded, 0, 0).expect("decode own output");
        assert_eq!(prim.positions, positions);
        assert_eq!(prim.indices, indices);
        let got = prim.colors0.expect("colors survive the roundtrip");
        for (rgba, rgb) in got.iter().zip(&colors) {
            assert_eq!(&rgba[..3], rgb);
            assert_eq!(rgba[3], 1.0);
        }
    }
}
