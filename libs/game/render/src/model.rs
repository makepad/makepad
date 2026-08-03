//! Static glTF/GLB props — the Kenney stock catalogue.
//!
//! A static mesh is a skinned one minus joints and weights, so this reuses
//! [`crate::skin`]'s container, JSON and accessor code rather than growing a
//! second parser that could drift from it. What differs is the baking: a prop
//! never animates, so each node's world transform is folded into its vertices
//! once at load and the whole model becomes one buffer with one draw.
//!
//! Kenney GLBs are NOT self-contained — every material points at an external
//! `Textures/colormap.png` shared by the entire pack. That is the reason a
//! whole pack draws in a single batch: same texture, so no state change.

use crate::skin::{mat4_mul_dir, mat4_mul_point, oct_encode, trs_to_mat4, Accessors, JsonParser, NodeTrs, Val};
use makepad_draw::makepad_math::{Mat4f, Quat, Vec3f};

/// Floats per packed vertex — matches `geom.GameMeshVertex` and the skinned
/// stream, so both paths feed the same shader.
pub const MODEL_VERTEX_FLOATS: usize = 6;

/// A loaded prop: packed vertices ready for upload, plus where its texture
/// lives relative to the GLB.
pub struct StaticModel {
    /// Packed `geom.GameMeshVertex` floats: pos.xyz, oct-normal, f16 uv, unorm8 colour.
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    /// glTF image URI, e.g. `Textures/colormap.png`. Relative to the GLB.
    pub texture_uri: Option<String>,
    /// Model-space bounds, for placing a prop on the ground without guessing.
    pub min: Vec3f,
    pub max: Vec3f,
    /// Per-primitive model-space bounds — the prop's own decomposition.
    ///
    /// Kenney authors a house as walls, roof, door frame and chimney, and a
    /// tree as trunk and canopy, so each primitive's box is a ready-made
    /// low-res collider part. One AABB round the whole prop would make a
    /// doorway solid and a canopy a wall you bump into from ten feet away —
    /// both feel worse than no collision at all.
    pub parts: Vec<(Vec3f, Vec3f)>,
}

/// Boxes below this fraction of the model's largest dimension are decoration
/// — door handles, window frames, chimney pots. Colliding with them is worse
/// than ignoring them: they add cost and snag a walker on nothing.
const PART_MIN_FRACTION: f32 = 0.10;
/// "Low-res" is the point. A handful of boxes captures a house; thirty
/// captures its trim and feels no different to walk into.
const PART_MAX: usize = 8;

impl StaticModel {
    /// A low-res multi-box collider derived from the prop's own primitives.
    ///
    /// Returned in model space, so the caller scales and offsets them exactly
    /// as it does the visual instance. Boxes are dropped if they are tiny
    /// relative to the model, merged when they nearly coincide, and capped —
    /// the aim is a collider that *feels* right, not one that is exact.
    pub fn collider_parts(&self) -> Vec<(Vec3f, Vec3f)> {
        let span = (self.max.x - self.min.x)
            .max(self.max.y - self.min.y)
            .max(self.max.z - self.min.z);
        if span <= 0.0 {
            return Vec::new();
        }
        let floor = span * PART_MIN_FRACTION;
        let mut kept: Vec<(Vec3f, Vec3f)> = Vec::new();
        for (a, b) in &self.parts {
            let (w, h, d) = (b.x - a.x, b.y - a.y, b.z - a.z);
            // A part must be substantial in at least two axes: a flat panel is
            // a wall and matters, a thin rod is trim and does not.
            let big = [w, h, d].iter().filter(|v| **v >= floor).count();
            if big < 2 {
                continue;
            }
            // Merge into an existing box when they nearly coincide, which is
            // what a wall split across several primitives looks like.
            let tol = span * 0.08;
            if let Some(e) = kept.iter_mut().find(|(ka, kb)| {
                (ka.x - a.x).abs() < tol
                    && (ka.z - a.z).abs() < tol
                    && (kb.x - b.x).abs() < tol
                    && (kb.z - b.z).abs() < tol
            }) {
                e.0.y = e.0.y.min(a.y);
                e.1.y = e.1.y.max(b.y);
                continue;
            }
            kept.push((*a, *b));
        }
        // Keep the biggest when over budget: the parts that carry the shape.
        if kept.len() > PART_MAX {
            kept.sort_by(|x, y| {
                let vol = |p: &(Vec3f, Vec3f)| {
                    (p.1.x - p.0.x) * (p.1.y - p.0.y) * (p.1.z - p.0.z)
                };
                vol(y).partial_cmp(&vol(x)).unwrap_or(std::cmp::Ordering::Equal)
            });
            kept.truncate(PART_MAX);
        }
        kept
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / MODEL_VERTEX_FLOATS
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Height of the model, so a caller can sit it on the ground.
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn parse_glb(bytes: &[u8]) -> Result<StaticModel, String> {
        if bytes.len() < 12 || &bytes[0..4] != b"glTF" {
            return Err("not a GLB (magic mismatch)".into());
        }
        let mut json_chunk: Option<&[u8]> = None;
        let mut bin_chunk: &[u8] = &[];
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let data = bytes
                .get(at + 8..at + 8 + len)
                .ok_or("GLB chunk out of range")?;
            match kind {
                b"JSON" => json_chunk = Some(data),
                b"BIN\0" => bin_chunk = data,
                _ => {}
            }
            at += 8 + len + (4 - len % 4) % 4;
        }
        let json = JsonParser::parse(json_chunk.ok_or("GLB has no JSON chunk")?)?;
        let acc = Accessors {
            json: &json,
            bin: bin_chunk,
        };

        // Node rest transforms, then parents from the children lists.
        let node_vals = json.get("nodes").map(|n| n.arr()).unwrap_or(&[]);
        let mut rests: Vec<NodeTrs> = Vec::with_capacity(node_vals.len());
        let mut parents: Vec<Option<usize>> = vec![None; node_vals.len()];
        for n in node_vals {
            let mut rest = NodeTrs::default();
            if let Some(t) = n.get("translation") {
                rest.t = Vec3f {
                    x: t.idx(0).and_then(Val::f64).unwrap_or(0.0) as f32,
                    y: t.idx(1).and_then(Val::f64).unwrap_or(0.0) as f32,
                    z: t.idx(2).and_then(Val::f64).unwrap_or(0.0) as f32,
                };
            }
            if let Some(r) = n.get("rotation") {
                rest.r = Quat {
                    x: r.idx(0).and_then(Val::f64).unwrap_or(0.0) as f32,
                    y: r.idx(1).and_then(Val::f64).unwrap_or(0.0) as f32,
                    z: r.idx(2).and_then(Val::f64).unwrap_or(0.0) as f32,
                    w: r.idx(3).and_then(Val::f64).unwrap_or(1.0) as f32,
                };
            }
            if let Some(s) = n.get("scale") {
                rest.s = Vec3f {
                    x: s.idx(0).and_then(Val::f64).unwrap_or(1.0) as f32,
                    y: s.idx(1).and_then(Val::f64).unwrap_or(1.0) as f32,
                    z: s.idx(2).and_then(Val::f64).unwrap_or(1.0) as f32,
                };
            }
            rests.push(rest);
        }
        for (parent_index, n) in node_vals.iter().enumerate() {
            if let Some(children) = n.get("children") {
                for c in children.arr() {
                    if let Some(ci) = c.usize() {
                        if ci < parents.len() {
                            parents[ci] = Some(parent_index);
                        }
                    }
                }
            }
        }
        // World transform per node: walk to the root and multiply down. Depth
        // is a handful for a prop, so recomputing per mesh node is cheaper
        // than caching and far easier to read.
        let world_of = |mut node: usize| -> Mat4f {
            let mut chain = vec![node];
            while let Some(p) = parents[node] {
                chain.push(p);
                node = p;
            }
            let mut m = Mat4f::identity();
            for idx in chain.iter().rev() {
                m = Mat4f::mul(&m, &trs_to_mat4(&rests[*idx]));
            }
            m
        };

        let mut vertices: Vec<f32> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut min = Vec3f {
            x: f32::MAX,
            y: f32::MAX,
            z: f32::MAX,
        };
        let mut max = Vec3f {
            x: f32::MIN,
            y: f32::MIN,
            z: f32::MIN,
        };
        let mut vert_total = 0usize;
        let mut parts: Vec<(Vec3f, Vec3f)> = Vec::new();

        for (node_index, n) in node_vals.iter().enumerate() {
            let Some(mesh_index) = n.get("mesh").and_then(Val::usize) else {
                continue;
            };
            let world = world_of(node_index);
            let mesh = json
                .get("meshes")
                .and_then(|m| m.idx(mesh_index))
                .ok_or("bad mesh index")?;
            for prim in mesh.get("primitives").map(|p| p.arr()).unwrap_or(&[]) {
                let attrs = prim
                    .get("attributes")
                    .ok_or("primitive without attributes")?;
                // Kenney ships two conventions: most packs UV-map everything
                // into one `colormap.png`, but some (nature-kit) carry no
                // texture at all and colour each primitive with a material
                // baseColorFactor. Baking that factor into the vertex colour
                // lets both render through one shader — atlas models simply
                // carry white.
                let tint = prim
                    .get("material")
                    .and_then(Val::usize)
                    .and_then(|mi| json.get("materials").and_then(|m| m.idx(mi)))
                    .and_then(|m| m.get("pbrMetallicRoughness"))
                    .and_then(|p| p.get("baseColorFactor"))
                    .map(|f| {
                        [
                            f.idx(0).and_then(Val::f64).unwrap_or(1.0) as f32,
                            f.idx(1).and_then(Val::f64).unwrap_or(1.0) as f32,
                            f.idx(2).and_then(Val::f64).unwrap_or(1.0) as f32,
                            f.idx(3).and_then(Val::f64).unwrap_or(1.0) as f32,
                        ]
                    })
                    .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                let pos_acc = attrs
                    .get("POSITION")
                    .and_then(Val::usize)
                    .ok_or("primitive without POSITION")?;
                let (pos, _) = acc.read_f32(pos_acc)?;
                let normal = attrs
                    .get("NORMAL")
                    .and_then(Val::usize)
                    .map(|a| acc.read_f32(a))
                    .transpose()?
                    .map(|(v, _)| v);
                let uv = attrs
                    .get("TEXCOORD_0")
                    .and_then(Val::usize)
                    .map(|a| acc.read_f32(a))
                    .transpose()?
                    .map(|(v, _)| v);

                let base = vert_total as u32;
                let count = pos.len() / 3;
                let mut pmin = Vec3f { x: f32::MAX, y: f32::MAX, z: f32::MAX };
                let mut pmax = Vec3f { x: f32::MIN, y: f32::MIN, z: f32::MIN };
                for i in 0..count {
                    let g = |src: &Option<Vec<f32>>, lanes: usize, lane: usize, dflt: f32| {
                        src.as_ref()
                            .and_then(|v| v.get(i * lanes + lane).copied())
                            .unwrap_or(dflt)
                    };
                    // Bake the node transform in: a prop never animates, so
                    // this is done once here instead of per frame on the GPU.
                    let p = mat4_mul_point(
                        &world,
                        Vec3f {
                            x: pos[i * 3],
                            y: pos[i * 3 + 1],
                            z: pos[i * 3 + 2],
                        },
                    );
                    let mut nrm = mat4_mul_dir(
                        &world,
                        Vec3f {
                            x: g(&normal, 3, 0, 0.0),
                            y: g(&normal, 3, 1, 1.0),
                            z: g(&normal, 3, 2, 0.0),
                        },
                    );
                    let len = (nrm.x * nrm.x + nrm.y * nrm.y + nrm.z * nrm.z).sqrt();
                    if len > 1.0e-8 {
                        nrm.x /= len;
                        nrm.y /= len;
                        nrm.z /= len;
                    }
                    min.x = min.x.min(p.x);
                    min.y = min.y.min(p.y);
                    min.z = min.z.min(p.z);
                    max.x = max.x.max(p.x);
                    max.y = max.y.max(p.y);
                    max.z = max.z.max(p.z);
                    pmin.x = pmin.x.min(p.x);
                    pmin.y = pmin.y.min(p.y);
                    pmin.z = pmin.z.min(p.z);
                    pmax.x = pmax.x.max(p.x);
                    pmax.y = pmax.y.max(p.y);
                    pmax.z = pmax.z.max(p.z);
                    let (ox, oy) = oct_encode(nrm);
                    vertices.extend_from_slice(&[
                        p.x,
                        p.y,
                        p.z,
                        makepad_draw::pack_pair_f16(ox, oy),
                        makepad_draw::pack_pair_f16(g(&uv, 2, 0, 0.0), g(&uv, 2, 1, 0.0)),
                        makepad_draw::pack_unorm8x4(tint[0], tint[1], tint[2], tint[3]),
                    ]);
                }
                if let Some(idx_acc) = prim.get("indices").and_then(Val::usize) {
                    let (idx, _) = acc.read_f32(idx_acc)?;
                    indices.extend(idx.iter().map(|v| base + *v as u32));
                } else {
                    indices.extend((0..count as u32).map(|i| base + i));
                }
                vert_total += count;
                if count > 0 {
                    parts.push((pmin, pmax));
                }
            }
        }
        if vertices.is_empty() {
            return Err("no mesh primitives found".into());
        }

        // First image URI: Kenney packs use exactly one atlas per pack, and a
        // model referencing several would batch badly anyway.
        let texture_uri = json
            .get("images")
            .and_then(|i| i.idx(0))
            .and_then(|i| i.get("uri"))
            .and_then(Val::str)
            .map(str::to_string);

        Ok(StaticModel {
            vertices,
            indices,
            texture_uri,
            min,
            max,
            parts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single-triangle GLB built in code, so the parser is covered without
    /// requiring the downloaded catalogue.
    fn tiny_glb(with_node_translation: bool) -> Vec<u8> {
        let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut bin: Vec<u8> = Vec::new();
        for f in positions {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let node = if with_node_translation {
            r#"{"mesh":0,"translation":[10.0,0.0,0.0]}"#
        } else {
            r#"{"mesh":0}"#
        };
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},
            "nodes":[{node}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}}],
            "buffers":[{{"byteLength":{}}}],
            "images":[{{"uri":"Textures/colormap.png"}}]}}"#,
            bin.len(),
            bin.len()
        );
        let mut json_bytes = json.into_bytes();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    #[test]
    fn parses_a_static_mesh_without_a_skin() {
        let m = StaticModel::parse_glb(&tiny_glb(false)).unwrap();
        assert_eq!(m.vertex_count(), 3);
        assert_eq!(m.triangle_count(), 1);
        assert_eq!(m.texture_uri.as_deref(), Some("Textures/colormap.png"));
        assert_eq!(m.vertices.len(), 3 * MODEL_VERTEX_FLOATS);
    }

    /// The node transform must be folded into the vertices, not dropped —
    /// dropping it is why a prop would silently render at the origin.
    #[test]
    fn node_transform_is_baked_into_vertices() {
        let m = StaticModel::parse_glb(&tiny_glb(true)).unwrap();
        assert!(
            (m.min.x - 10.0).abs() < 1.0e-5,
            "translation not baked: min.x = {}",
            m.min.x
        );
        assert!((m.max.x - 11.0).abs() < 1.0e-5);
    }

    #[test]
    fn rejects_non_glb_input() {
        assert!(StaticModel::parse_glb(b"not a gltf at all").is_err());
        assert!(StaticModel::parse_glb(&[]).is_err());
    }

    /// A house is walls + roof + door frame, so its collider must be several
    /// boxes with the doorway left as a GAP. One AABB would make the door
    /// solid, which is the difference between a building and a rock.
    #[test]
    fn collider_parts_keep_structure_and_drop_trim() {
        let model = StaticModel {
            vertices: Vec::new(),
            indices: Vec::new(),
            texture_uri: None,
            min: Vec3f { x: -2.0, y: 0.0, z: -2.0 },
            max: Vec3f { x: 2.0, y: 3.0, z: 2.0 },
            parts: vec![
                // Two wall slabs either side of a doorway.
                (Vec3f { x: -2.0, y: 0.0, z: -2.0 }, Vec3f { x: -0.5, y: 3.0, z: 2.0 }),
                (Vec3f { x: 0.5, y: 0.0, z: -2.0 }, Vec3f { x: 2.0, y: 3.0, z: 2.0 }),
                // A door handle: substantial in no axis, must be dropped.
                (Vec3f { x: -0.4, y: 1.2, z: 1.9 }, Vec3f { x: -0.3, y: 1.3, z: 2.0 }),
            ],
        };
        let parts = model.collider_parts();
        assert_eq!(parts.len(), 2, "expected two walls, got {parts:?}");
        // And the doorway between them really is open.
        let gap = parts.iter().all(|(a, b)| !(a.x < 0.0 && b.x > 0.0));
        assert!(gap, "a collider spans the doorway: {parts:?}");
    }

    /// Low-res by design: a prop with many primitives must not produce a
    /// collider per screw. The biggest boxes carry the shape.
    #[test]
    fn collider_parts_are_capped() {
        let mut parts = Vec::new();
        for i in 0..40 {
            let x = i as f32 * 0.5;
            parts.push((
                Vec3f { x, y: 0.0, z: 0.0 },
                Vec3f { x: x + 2.0, y: 2.0, z: 2.0 },
            ));
        }
        let model = StaticModel {
            vertices: Vec::new(),
            indices: Vec::new(),
            texture_uri: None,
            min: Vec3f { x: 0.0, y: 0.0, z: 0.0 },
            max: Vec3f { x: 22.0, y: 2.0, z: 2.0 },
            parts,
        };
        assert!(model.collider_parts().len() <= 8);
    }
}
