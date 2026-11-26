use crate::build_detail::{CompilePhase, CompileTimer};
use crate::{mesh, AnimationPath, Document, Error, Head, Result, Texture};
use makepad_gltf::{
    augment_glb_skin, replace_glb_node_animations, write_glb_mesh_textured_parts,
    write_glb_mesh_textured_parts_linear, GlbAnimPath, GlbNodeAnimChannel, GlbNodeAnimClip,
    GlbPrimitiveSkin, GlbSkinJoint, GlbTexturedPart,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Worker-owned export intermediates for a sequence of related compiles.
/// Entries compare their source inputs; changing an object or a texture layer
/// rebuilds that entry. Drop this with the build, rather than retaining model
/// geometry and image layers in a process-wide cache.
#[derive(Default)]
pub struct CompileCache {
    objects: BTreeMap<String, CachedObject>,
    textures: BTreeMap<(u32, crate::SurfaceChannel), CachedTexture>,
}

struct CachedObject {
    mesh: mesh::Mesh,
    colors: Vec<(mesh::VertexId, [u64; 4])>,
    rich_surface: bool,
    parts: Vec<(u32, Arc<Part>)>,
}

struct CachedTexture {
    msdf: bool,
    layers: Vec<crate::SurfaceLayer>,
    limits: (u32, usize),
    texture: Option<makepad_gltf::GlbPbrTexture>,
}

/// The SHA-256 of everything an export texture is made from: its channel, whether it is a
/// distance (MSDF) plane, the limits and the channel's layers. Equal digests make equal
/// textures, so [`shared_texture`] keeps only the digest. Every input struct is destructured
/// without `..`: a field added later does not compile here until it is hashed, or skipped with
/// the reason. Floats go in by their bits, so -0 and +0 are two inputs, as for the buffers.
fn texture_key(channel: crate::SurfaceChannel, msdf: bool, limits: &crate::Limits, layers: &[crate::SurfaceLayer]) -> [u8; 32] {
    use makepad_core_util::sha256::Sha256;
    fn word(h: &mut Sha256, v: u64) { h.update(&v.to_le_bytes()); }
    fn bytes(h: &mut Sha256, b: &[u8]) { word(h, b.len() as u64); h.update(b); }
    fn image(h: &mut Sha256, image: &crate::RgbaImage) {
        let crate::RgbaImage { width, height, pixels } = image;
        word(h, *width as u64);
        word(h, *height as u64);
        bytes(h, pixels);
    }
    let mut h = Sha256::new();
    bytes(&mut h, b"export texture 1");
    word(&mut h, channel as u64);
    word(&mut h, msdf as u64);
    // Undo and receipt retention shape the document's history, never its export.
    let crate::Limits { mesh: mesh::Limits { max_bytes }, max_history: _, max_receipts: _, max_texture_dimension } = limits;
    word(&mut h, *max_bytes as u64);
    word(&mut h, *max_texture_dimension as u64);
    word(&mut h, layers.len() as u64);
    for layer in layers {
        let crate::SurfaceLayer { id, image: pixels, mask, opacity, blend, visible, pattern, derived, mips } = layer;
        bytes(&mut h, id.as_bytes());
        image(&mut h, pixels);
        match mask {
            None => word(&mut h, 0),
            Some(mask) => { word(&mut h, 1); bytes(&mut h, mask); }
        }
        word(&mut h, opacity.to_bits());
        word(&mut h, *blend as u64);
        word(&mut h, *visible as u64);
        match pattern {
            None => word(&mut h, 0),
            Some(crate::SurfacePattern { kind, color_a, color_b, scale, seed }) => {
                word(&mut h, 1);
                word(&mut h, *kind as u64);
                for v in color_a.iter().chain(color_b).chain(scale) { word(&mut h, v.to_bits()); }
                word(&mut h, *seed);
            }
        }
        match derived {
            None => word(&mut h, 0),
            Some(crate::SurfaceDerivation { source_channel, source_layer, strength, roughness_min, roughness_max, metallic, wrap }) => {
                word(&mut h, 1);
                word(&mut h, *source_channel as u64);
                match source_layer {
                    None => word(&mut h, 0),
                    Some(layer) => { word(&mut h, 1); bytes(&mut h, layer.as_bytes()); }
                }
                for v in [strength, roughness_min, roughness_max, metallic] { word(&mut h, v.to_bits()); }
                word(&mut h, *wrap as u64);
            }
        }
        word(&mut h, mips.len() as u64);
        for mip in mips { image(&mut h, mip); }
    }
    h.finalize()
}

/// An export texture, built once per process for its inputs ([`texture_key`]). Models built
/// from one recipe share their materials' layers, and each export flattened, mipped and
/// PNG-encoded them again; a hit returns the bytes that work makes. The first ask of a key
/// builds it; asks that come while it does wait for it off the CPU, their permits lent
/// ([`ComputeOnce`]). A UI thread never waits for another thread's build: it takes a finished
/// one, or builds on its own. A failed build (a budget, a cancel) is not remembered, and an ask
/// that waited on one builds for itself. Only the digest and the PNGs are kept, never layers.
/// Bounded: past 96 MiB of PNGs the memo starts over, which is logged once.
///
/// The step before libs/render's load-ready encode memo: this one skips the export work, and models
/// that share a texture still embed equal PNGs, which the encode memo then encodes once.
///
/// [`ComputeOnce`]: makepad_core_util::once::ComputeOnce
fn shared_texture(key: [u8; 32], build: &mut dyn FnMut() -> Result<Option<makepad_gltf::GlbPbrTexture>>) -> Result<Option<makepad_gltf::GlbPbrTexture>> {
    use makepad_core_util::once::ComputeOnce;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    // `Err(())`: the build failed; its asker has the error, the others build for themselves.
    type Entry = Arc<ComputeOnce<std::result::Result<Option<makepad_gltf::GlbPbrTexture>, ()>>>;
    struct Memo { entries: std::collections::HashMap<[u8; 32], Entry>, bytes: usize }
    static MEMO: Mutex<Option<Memo>> = Mutex::new(None);
    static BOUND_LOGGED: AtomicBool = AtomicBool::new(false);
    const BOUND: usize = 96 << 20;
    #[cfg(test)]
    let build = &mut || { tests::BUILDS.with(|b| b.set(b.get() + 1)); build() };
    let ours = |memo: &Memo, entry: &Entry| memo.entries.get(&key).is_some_and(|e| Arc::ptr_eq(e, entry));
    if makepad_core_util::ui_thread::is_ui_thread() {
        let done = MEMO.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            .and_then(|memo| memo.entries.get(&key).and_then(|entry| entry.get().cloned()));
        return match done { Some(Ok(texture)) => Ok(texture), _ => build() };
    }
    loop {
        let entry = MEMO.lock().unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(|| Memo { entries: Default::default(), bytes: 0 })
            .entries.entry(key).or_default().clone();
        let mut built = None;
        let shared = entry.get_or_init(|| {
            let out = build();
            let shared = out.as_ref().map(Clone::clone).map_err(|_| ());
            built = Some(out);
            shared
        }).clone();
        let mut memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
        let Some(memo) = memo.as_mut().filter(|memo| ours(memo, &entry)) else {
            // (The memo started over meanwhile: the entry is no longer its own.)
            match (built, shared) {
                (Some(out), _) => return out,
                (None, Ok(texture)) => return Ok(texture),
                (None, Err(())) => continue,
            }
        };
        match (built, shared) {
            (Some(Err(error)), _) => { memo.entries.remove(&key); return Err(error); }
            (Some(Ok(texture)), _) => {
                let cost = texture.as_ref().map_or(0, |t| t.png.len() + t.mip_pngs.iter().map(Vec::len).sum::<usize>()) + key.len();
                if memo.bytes + cost > BOUND {
                    if !BOUND_LOGGED.swap(true, Ordering::Relaxed) {
                        eprintln!("model export: the texture memo passed {} MiB and starts over (logged once)", BOUND >> 20);
                    }
                    memo.entries.retain(|_, e| Arc::ptr_eq(e, &entry));
                    memo.bytes = 0;
                }
                memo.bytes += cost;
                return Ok(texture);
            }
            (None, Ok(texture)) => return Ok(texture),
            // Another ask's build failed: this one builds for itself.
            (None, Err(())) => { memo.entries.remove(&key); }
        }
    }
}

// PartialEq alone identifies -0 and +0, but the exported f32 buffers retain
// their bits. Check the floating-point mesh inputs as well as the topology.
fn same_mesh(a: &mesh::Mesh, b: &mesh::Mesh) -> bool {
    a == b
        && a.vertices().iter().zip(b.vertices()).all(|(a, b)| {
            a.position.map(f64::to_bits) == b.position.map(f64::to_bits)
                && a.weights.iter().zip(&b.weights).all(|(a, b)| a.weight.to_bits() == b.weight.to_bits())
        })
        && a.corners().iter().zip(b.corners()).all(|(a, b)| {
            a.uv.map(f64::to_bits) == b.uv.map(f64::to_bits)
                && a.normal.map(|n| n.map(f64::to_bits)) == b.normal.map(|n| n.map(f64::to_bits))
        })
        && a.edge_attributes().values().zip(b.edge_attributes().values())
            .all(|(a, b)| a.attributes.crease.to_bits() == b.attributes.crease.to_bits())
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrimitiveSource {
    pub object: String,
    pub material: u32,
    pub faces: Vec<mesh::FaceId>,
    pub source_vertices: Vec<mesh::VertexId>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}
#[derive(Clone, Debug)]
pub struct CompiledModel {
    /// Per-part delivery diagnostics; the remaining geometry still compiles.
    pub warnings: Vec<String>,
    pub head: Head,
    pub glb: Vec<u8>,
    pub vertices: usize,
    pub triangles: usize,
    pub bounds: [[f32; 3]; 2],
    pub primitives: Vec<PrimitiveSource>,
}
#[derive(Clone, Default)]
struct Part {
    source_vertices: Vec<mesh::VertexId>,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    faces: Vec<mesh::FaceId>,
    weights: Vec<f64>,
    colors: Vec<[f32; 4]>,
    tangents: Vec<[f32; 4]>,
}
fn f32_value(v: f64) -> Result<f32> {
    let v = v as f32;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(Error::Invalid("coordinate exceeds render precision"))
    }
}
impl Document {
    /// Compile opaque material primitives, preserving source corner UVs and normals.
    /// Skinned output bakes a shared image and remaps derived UVs so consumers
    /// using the first embedded image render every source material correctly.
    /// A document skeleton adds skinning to every primitive; every rendered
    /// vertex must then have known influences. Export keeps the four strongest
    /// influences and renormalizes them, with a diagnostic per affected mesh. Editable rest TRS is
    /// applied by the final authoring pass; cubic controls bake at 60 Hz.
    /// Bounds are bind-pose bounds, not a conservative animated-motion envelope.
    /// Existing GLB augmentation stages are bounded and cancellation is checked
    /// between them; their inner serialization loops do not expose cancellation.
    /// Object/face provenance accompanies the result. The source document, not
    /// the render mesh, preserves editing identities and separate objects.
    pub(crate) fn compile_evaluated(
        &self,
        cancelled: Option<&dyn Fn() -> bool>,
        mut cache: Option<&mut CompileCache>,
        delivery: Option<&crate::delivery::LodDelivery>,
    ) -> Result<CompiledModel> {
        let packing_timer = CompileTimer::new(CompilePhase::MeshPacking);
        let mut ctx = mesh::Context::new(self.limits().mesh.clone(), cancelled);
        let rich_surface =
            !self.surface().materials.is_empty() || !self.surface().vertex_colors.is_empty();
        let joint_count = self.skeleton().map_or(0, |s| s.joints.len());
        if let Some(skeleton) = self.skeleton() {
            skeleton.validate(self.limits(), &mut ctx)?;
        }
        // Admission includes dense weights required by the existing augmenter,
        // compact skin buffers, writer copies and source material resources.
        let predicted_triangles = self.objects().try_fold(0usize, |n, (_, m)| {
            n.checked_add(
                m.faces()
                    .iter()
                    .map(|f| f.corner_count as usize - 2)
                    .sum::<usize>(),
            )
            .ok_or(Error::Budget("compiled triangles"))
        })?;
        let texture_bytes = self
            .materials()
            .values()
            .map(|m| m.base_color_png.len())
            .sum::<usize>();
        let surface_bytes = self.surface().memory_bytes();
        let working = predicted_triangles
            .saturating_mul(512usize.saturating_add(joint_count.saturating_mul(32)))
            .saturating_add(texture_bytes.saturating_mul(4))
            .saturating_add(surface_bytes.saturating_mul(4));
        if working > self.limits().mesh.max_bytes {
            return Err(Error::Budget("skin/export working bytes"));
        }
        ctx.checkpoint(1)?;
        let mut parts: BTreeMap<(String, u32), Arc<Part>> = BTreeMap::new();
        let mut weld: std::collections::HashMap<(String, u32), std::collections::HashMap<(mesh::VertexId, [u32; 3], [u32; 2]), u32>> = std::collections::HashMap::new();
        // Shape keys carry per-triangle normal deltas, so a document with
        // morphs keeps one render vertex per triangle corner.
        let weld_corners = self.rig().morphs.is_empty();
        let mut triangles = 0usize;
        if let Some(cache) = cache.as_deref_mut() {
            cache.objects.retain(|name, _| joint_count == 0 && weld_corners && self.state.objects.contains_key(name));
            if !rich_surface { cache.textures.clear(); }
        }
        for (name, mesh) in self.objects() {
            ctx.checkpoint(1)?;
            // augment_glb_skin packs JOINTS_0/WEIGHTS_0 as four lanes, keeping
            // the strongest influences and renormalizing them. Editable weights stay intact.
            if joint_count > 0 && mesh.vertices().iter().any(|v| v.weights.len() > 4) {
                eprintln!("model mesh {name}: exported four strongest joint influences per vertex, renormalized");
            }
            // Skinned export remaps UVs and weights, and morphs keep distinct
            // corners. Cache only the static, welded representation.
            let reusable = cache.is_some() && joint_count == 0 && weld_corners;
            let colors = if reusable {
                self.surface().vertex_colors.range((name.to_owned(), mesh::VertexId(0))..)
                    .take_while(|((object, _), _)| object == name)
                    .map(|((_, vertex), color)| (*vertex, color.map(f64::to_bits))).collect::<Vec<_>>()
            } else { Vec::new() };
            if let Some(hit) = cache.as_deref().and_then(|c| c.objects.get(name))
                .filter(|hit| reusable && hit.rich_surface == rich_surface && hit.colors == colors && same_mesh(&hit.mesh, mesh)) {
                for (material, part) in &hit.parts {
                    triangles += part.indices.len() / 3;
                    parts.insert((name.to_owned(), *material), part.clone());
                }
                continue;
            }
            if joint_count == 0 && mesh.vertices().iter().any(|v| !v.weights.is_empty()) {
                return Err(Error::Invalid(
                    "weighted mesh requires a skeleton; static compile cannot discard skin weights",
                ));
            }
            // A closed shell wound inward would draw its inside under back-face
            // culling; the product faces every closed shell out (drafts too).
            let inward = mesh.inward_closed_faces(&mut ctx)?;
            let oriented;
            let mesh = if inward.is_empty() { mesh } else {
                let mut copy = mesh.clone(); copy.flip_faces(&inward, &mut ctx)?; oriented = copy; &oriented
            };
            let triangulated = mesh.triangulate(&mut ctx)?;
            triangles = triangles
                .checked_add(triangulated.triangles.len())
                .ok_or(Error::Budget("compiled triangles"))?;
            // Conservative bound includes temporary writer copies and accessors.
            if triangles.saturating_mul(384) > self.limits().mesh.max_bytes {
                return Err(Error::Budget("compile working bytes"));
            }
            for tri in &triangulated.triangles {
                ctx.checkpoint(1)?;
                let part = Arc::make_mut(parts.entry((name.to_owned(), tri.material)).or_default());
                let welded = weld.entry((name.to_owned(), tri.material)).or_default();
                part.faces.push(tri.source_face);
                for index in tri.indices {
                    let v = &triangulated.vertices[index as usize];
                    // Corners sharing source vertex, normal and UV share one
                    // render vertex (weights and colour follow the source).
                    let key = (v.source_vertex, v.normal.map(|x| (x as f32).to_bits()), v.uv.map(|x| (x as f32).to_bits()));
                    if let Some(&existing) = welded.get(&key).filter(|_| weld_corners) {
                        part.indices.push(existing);
                        continue;
                    }
                    let next: u32 = part.positions.len().try_into().map_err(|_| Error::Budget("compiled vertices"))?;
                    welded.insert(key, next);
                    part.indices.push(next);
                    part.source_vertices.push(v.source_vertex);
                    part.positions.push([
                        f32_value(v.position[0])?,
                        f32_value(v.position[1])?,
                        f32_value(v.position[2])?,
                    ]);
                    part.normals.push([
                        f32_value(v.normal[0])?,
                        f32_value(v.normal[1])?,
                        f32_value(v.normal[2])?,
                    ]);
                    part.uvs.push([f32_value(v.uv[0])?, f32_value(v.uv[1])?]);
                    if rich_surface {
                        let color = self
                            .surface()
                            .vertex_colors
                            .get(&(name.to_owned(), v.source_vertex))
                            .copied()
                            .unwrap_or([1.; 4]);
                        part.colors.push([
                            f32_value(color[0])?,
                            f32_value(color[1])?,
                            f32_value(color[2])?,
                            f32_value(color[3])?,
                        ]);
                    }
                    if joint_count > 0 {
                        if v.weights.is_empty() {
                            return Err(Error::Invalid(
                                "skinned compile requires weights on every rendered vertex",
                            ));
                        }
                        let start = part.weights.len();
                        part.weights.resize(start + joint_count, 0.);
                        for weight in &v.weights {
                            if weight.joint as usize >= joint_count || weight.weight as f32 <= 0. {
                                return Err(Error::Invalid(
                                    "skin weight cannot be represented without loss",
                                ));
                            }
                            part.weights[start + weight.joint as usize] = weight.weight;
                        }
                    }
                }
            }
            if rich_surface {
                let _tangent_timer = CompileTimer::new(CompilePhase::Tangents);
                for (_, part) in parts.range_mut((name.to_owned(), 0)..).take_while(|((object, _), _)| object == name) {
                    let part = Arc::make_mut(part);
                    part.tangents = part_tangents(part, &mut ctx)?;
                }
            }
            if reusable {
                let object_parts = parts.range((name.to_owned(), 0)..).take_while(|((object, _), _)| object == name)
                    .map(|((_, material), part)| (*material, part.clone())).collect();
                cache.as_deref_mut().unwrap().objects.insert(name.to_owned(), CachedObject {
                    mesh: self.object(name).unwrap().clone(), colors, rich_surface, parts: object_parts,
                });
            }
        }
        if triangles == 0 {
            return Err(Error::Invalid("model has no surface to compile"));
        }
        if let Some(cache) = cache.as_deref_mut() {
            let used = parts.keys().map(|(_, material)| *material).collect::<std::collections::BTreeSet<_>>();
            cache.textures.retain(|(id, _), _| used.contains(id));
        }
        drop(packing_timer);
        let atlas = if joint_count > 0 && !rich_surface {
            let mut ranges = crate::atlas::UvBounds::new();
            for ((_, material), part) in &parts {
                let range = ranges
                    .entry(*material)
                    .or_insert([[f32::INFINITY; 2], [f32::NEG_INFINITY; 2]]);
                for uv in &part.uvs {
                    for axis in 0..2 {
                        range[0][axis] = range[0][axis].min(uv[axis]);
                        range[1][axis] = range[1][axis].max(uv[axis]);
                    }
                }
            }
            let mut atlas_limits = self.limits().clone();
            atlas_limits.mesh.max_bytes = atlas_limits.mesh.max_bytes.saturating_sub(working);
            let started = crate::program::ProgramInstant::now();
            let texture_timer = CompileTimer::new(CompilePhase::Textures);
            let png_before = crate::build_detail::png_time();
            let atlas = crate::atlas::bake(self.materials(), &ranges, &atlas_limits, &mut ctx)?;
            crate::program::record_texture_time(started.elapsed());
            crate::build_detail::record_texture(None, crate::SurfaceChannel::BaseColor, "atlas", atlas.size[0], atlas.size[1],
                started.elapsed().saturating_sub(crate::build_detail::png_time() - png_before));
            drop(texture_timer);
            for ((_, material), part) in &mut parts {
                for uv in &mut Arc::make_mut(part).uvs {
                    ctx.checkpoint(1)?;
                    *uv = atlas.remap(*material, *uv);
                }
            }
            Some(atlas)
        } else {
            None
        };
        let started = crate::program::ProgramInstant::now();
        let png_before = crate::build_detail::png_time();
        let white = Texture::solid(1, 1, [255; 3], self.limits())?.to_png(self.limits())?;
        crate::program::record_texture_time(started.elapsed());
        crate::build_detail::record_texture(None, crate::SurfaceChannel::BaseColor, "white", 1, 1,
            started.elapsed().saturating_sub(crate::build_detail::png_time() - png_before));
        let packing_timer = CompileTimer::new(CompilePhase::MeshPacking);
        let mut bounds = [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]];
        let mut vertices = 0;
        let mut primitives = Vec::new();
        let mut views = Vec::new();
        for ((object, id), part) in &parts {
            ctx.checkpoint(1)?;
            let material = self
                .materials()
                .get(id)
                .ok_or(Error::Invalid("unknown compiled material"))?;
            for p in &part.positions {
                for d in 0..3 {
                    bounds[0][d] = bounds[0][d].min(p[d]);
                    bounds[1][d] = bounds[1][d].max(p[d]);
                }
            }
            vertices += part.positions.len();
            primitives.push(PrimitiveSource {
                object: object.clone(),
                material: *id,
                faces: part.faces.clone(),
                // The GLB is welded; this record stays one entry per triangle corner.
                source_vertices: part.indices.iter().map(|&i| part.source_vertices[i as usize]).collect(),
                positions: part.indices.iter().map(|&i| part.positions[i as usize]).collect(),
                normals: part.indices.iter().map(|&i| part.normals[i as usize]).collect(),
            });
            views.push(GlbTexturedPart {
                positions: &part.positions,
                normals: Some(&part.normals),
                uvs: &part.uvs,
                indices: &part.indices,
                base_color_png: if let Some(atlas) = &atlas {
                    &atlas.png
                } else if material.base_color_png.is_empty() {
                    &white
                } else {
                    &material.base_color_png
                },
                base_color_factor: if atlas.is_some() {
                    Some([1.0; 4])
                } else {
                    Some([
                        material.color[0] as f32,
                        material.color[1] as f32,
                        material.color[2] as f32,
                        1.0,
                    ])
                },
                colors: None,
                lightmap_png: None,
                lightmap_uvs: None,
                detail_png: None,
                detail_scale: [0.0; 2],
            });
        }
        ctx.checkpoint(1)?;
        drop(packing_timer);
        let _assembly_timer = CompileTimer::new(CompilePhase::GlbAssembly);
        let mut glb = if atlas.is_some() {
            write_glb_mesh_textured_parts_linear(&views, false)
        } else {
            write_glb_mesh_textured_parts(&views, false)
        };
        drop(views);
        if rich_surface {
            glb = export_surface(self, &glb, &parts, &mut ctx, cache.as_deref_mut())?;
        }
        if let Some(skeleton) = self.skeleton() {
            let parsed = makepad_gltf::parse_glb_bytes(&glb)
                .map_err(|_| Error::Invalid("compiled GLB parse"))?;
            let joint_node_start = parsed.document.nodes_slice().len();
            let target_node = parsed
                .document
                .nodes_slice()
                .iter()
                .position(|n| n.mesh.is_some())
                .ok_or(Error::Invalid("compiled GLB mesh node"))?;
            let heads = skeleton.global_translations()?;
            let joints = skeleton
                .joints
                .iter()
                .zip(heads)
                .map(|(joint, position)| {
                    Ok(GlbSkinJoint {
                        name: joint.name.clone(),
                        parent: joint.parent.map(|p| p as usize),
                        global_translation: [
                            f32_value(position[0])?,
                            f32_value(position[1])?,
                            f32_value(position[2])?,
                        ],
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let skins = parts
                .into_iter()
                .enumerate()
                .map(|(primitive, (_, part))| GlbPrimitiveSkin {
                    node: target_node,
                    primitive,
                    weights: Arc::unwrap_or_clone(part).weights,
                })
                .collect::<Vec<_>>();
            ctx.checkpoint((vertices as u64).saturating_mul(joint_count as u64))?;
            glb = augment_glb_skin(&glb, &joints, &skins)
                .map_err(|_| Error::Invalid("GLB skin augmentation"))?;
            ctx.checkpoint(1)?;
            if !self.clips().is_empty() {
                let mut clips = Vec::with_capacity(self.clips().len());
                for clip in self.clips().values() {
                    clip.validate(skeleton, self.limits(), &mut ctx)?;
                    let mut channels = Vec::with_capacity(clip.channels.len());
                    for channel in &clip.channels {
                        let path = match channel.path {
                            AnimationPath::Translation => GlbAnimPath::Translation,
                            AnimationPath::Rotation => GlbAnimPath::Rotation,
                            AnimationPath::Scale => GlbAnimPath::Scale,
                        };
                        let lanes = if channel.path == AnimationPath::Rotation {
                            4
                        } else {
                            3
                        };
                        let mut times = Vec::with_capacity(channel.keys.len());
                        let mut values = Vec::with_capacity(channel.keys.len() * lanes);
                        if self.rig().clip_options.get(&clip.name).is_some_and(|o|o.interpolation==crate::Interpolation::Cubic) {
                            let steps=(clip.duration()*60.).ceil()as usize;
                            for i in 0..=steps {ctx.checkpoint(1)?;let time=(i as f64/60.).min(clip.duration());let pose=self.rig().sample_clip(skeleton,clip,time)?;
                                let t=pose[channel.joint as usize];let value=match channel.path{AnimationPath::Translation=>[t.translation[0],t.translation[1],t.translation[2],0.],AnimationPath::Rotation=>t.rotation,AnimationPath::Scale=>[t.scale[0],t.scale[1],t.scale[2],0.]};
                                times.push(f32_value(time)?);for &v in &value[..lanes]{values.push(f32_value(v)?);}
                            }
                        } else {
                            for key in &channel.keys {ctx.checkpoint(1)?;times.push(f32_value(key.time)?);for &v in &key.value[..lanes]{values.push(f32_value(v)?);}}
                        }
                        channels.push(GlbNodeAnimChannel {
                            node: joint_node_start + channel.joint as usize,
                            path,
                            times,
                            values,
                        });
                    }
                    clips.push(GlbNodeAnimClip {
                        name: clip.name.clone(),
                        channels,
                    });
                }
                glb = replace_glb_node_animations(&glb, &clips)
                    .map_err(|_| Error::Invalid("GLB animation augmentation"))?;
            }
        }
        glb = crate::authoring_export::finish(self, glb, &primitives, delivery, &mut ctx)?;
        ctx.checkpoint(glb.len() as u64)?;
        Ok(CompiledModel {
            warnings: delivery.map_or_else(Vec::new, |d| d.warnings.clone()),
            head: self.head(),
            glb,
            vertices,
            triangles,
            bounds,
            primitives,
        })
    }
}

/// Whether the material exports a UV-dependent texture. Surface materials
/// override legacy slots; hidden or zero-opacity layers do not flatten to a
/// texture. The exporter's neutral white image is not a UV constraint.
pub(crate) fn material_samples_texture(doc: &Document, material: u32) -> bool {
    if let Some(surface) = doc.surface().materials.get(&material) {
        surface
            .channels
            .values()
            .flatten()
            .any(|layer| layer.visible && layer.opacity > 0.)
    } else {
        doc.materials()
            .get(&material)
            .is_some_and(|m| !m.base_color_png.is_empty())
    }
}

fn export_surface(
    doc: &Document,
    glb: &[u8],
    parts: &BTreeMap<(String, u32), Arc<Part>>,
    ctx: &mut mesh::Context<'_>,
    mut cache: Option<&mut CompileCache>,
) -> Result<Vec<u8>> {
    use crate::{SurfaceAlpha, SurfaceChannel, SurfaceMaterial};
    use makepad_gltf::{augment_glb_pbr, GlbPbrMaterial, GlbPbrPrimitive, GlbPbrTexture};
    let mut materials = Vec::new();
    let mut ids = BTreeMap::new();
    let mut assignments = Vec::new();
    let mut derived_bytes = 0usize;
    for (primitive, ((_, id), part)) in parts.iter().enumerate() {
        ctx.checkpoint(1)?;
        let material = if let Some(index) = ids.get(id) {
            *index
        } else {
            let fallback;
            let source = if let Some(source) = doc.surface().materials.get(id) {
                source
            } else {
                let _texture_timer = CompileTimer::new(CompilePhase::Textures);
                let started = crate::program::ProgramInstant::now();
                fallback = SurfaceMaterial::from_legacy(
                    doc.materials()
                        .get(id)
                        .ok_or(Error::Invalid("surface export material"))?,
                    doc.limits(),
                )?;
                crate::program::record_texture_time(started.elapsed());
                &fallback
            };
            source.validate(doc.limits(), ctx)?;
            let samples_texture = material_samples_texture(doc, *id);
            let mut map = |channel: SurfaceChannel| -> Result<Option<GlbPbrTexture>> {
                if !samples_texture {
                    return Ok(None);
                }
                let layers = source.channels.get(&channel).map(Vec::as_slice).unwrap_or(&[]);
                let msdf = channel == SurfaceChannel::BaseColor && source.shading.is_some_and(|s| s.msdf > 0.0);
                let limits = (doc.limits().max_texture_dimension, doc.limits().mesh.max_bytes);
                let hit = cache.as_deref().and_then(|c| c.textures.get(&(*id, channel)))
                    .filter(|c| c.msdf == msdf && c.limits == limits && c.layers == layers
                        && c.layers.iter().zip(layers).all(|(a, b)| a.opacity.to_bits() == b.opacity.to_bits()));
                let texture = if let Some(hit) = hit {
                    hit.texture.clone()
                } else {
                    let started = crate::program::ProgramInstant::now();
                    let _texture_timer = CompileTimer::new(CompilePhase::Textures);
                    // Shared across the process's models by content ([`shared_texture`]). A hit, like a
                    // CompileCache hit, skips the checks inside the work it saves (flatten's material
                    // size budget, the work checkpoints); the image budget below holds every texture.
                    let build = &mut || -> Result<Option<GlbPbrTexture>> {
                        let Some(image) = source.flatten(channel, doc.limits(), ctx)? else {
                            return Ok(None);
                        };
                        let png_before = crate::build_detail::png_time();
                        let mips = image.mip_chain(if msdf { crate::ImageEncoding::Linear } else { channel.encoding() }, doc.limits(), ctx)?;
                        let png = image.to_png(doc.limits())?;
                        let mut mip_pngs = Vec::new();
                        for mip in mips {
                            ctx.checkpoint(mip.pixels.len() as u64)?;
                            let png = mip.to_png(doc.limits())?;
                            mip_pngs.push(png);
                        }
                        let kind = layers.iter().find(|layer| layer.visible && layer.opacity > 0.)
                            .map_or("image", |layer| crate::build_detail::texture_kind(channel, layer));
                        crate::build_detail::record_texture(Some(*id), channel, kind, image.width, image.height,
                            started.elapsed().saturating_sub(crate::build_detail::png_time() - png_before));
                        Ok(Some(GlbPbrTexture { png, mip_pngs }))
                    };
                    // A channel with no visible layer flattens to nothing at once: no work to share.
                    let texture = if layers.iter().any(|l| l.visible && l.opacity > 0.) {
                        shared_texture(texture_key(channel, msdf, doc.limits(), layers), build)
                    } else {
                        build()
                    };
                    crate::program::record_texture_time(started.elapsed());
                    let texture = texture?;
                    if let Some(cache) = cache.as_deref_mut() {
                        cache.textures.insert((*id, channel), CachedTexture { msdf, layers: layers.to_vec(), limits, texture: texture.clone() });
                    }
                    texture
                };
                if let Some(texture) = &texture {
                    derived_bytes = derived_bytes.saturating_add(texture.png.len());
                    for png in &texture.mip_pngs { derived_bytes = derived_bytes.saturating_add(png.len()); }
                    if derived_bytes
                        .saturating_mul(4)
                        .saturating_add(doc.surface().memory_bytes())
                        .saturating_add(glb.len() * 3)
                        > doc.limits().mesh.max_bytes
                    {
                        return Err(Error::Budget("PBR export images"));
                    }
                }
                Ok(texture)
            };
            let out = GlbPbrMaterial {
                fur: source.fur,
                shading: source.shading,
                base_color: source.base_color,
                metallic: source.metallic,
                roughness: source.roughness,
                normal_scale: source.normal_scale,
                occlusion_strength: source.occlusion_strength,
                emissive: source.emissive,
                emissive_strength: source.emissive_strength,
                alpha_mode: match source.alpha {
                    SurfaceAlpha::Opaque => "OPAQUE",
                    SurfaceAlpha::Mask => "MASK",
                    SurfaceAlpha::Blend => "BLEND",
                }
                .into(),
                alpha_cutoff: source.alpha_cutoff,
                double_sided: source.double_sided,
                base_color_texture: map(SurfaceChannel::BaseColor)?,
                metallic_roughness_texture: map(SurfaceChannel::MetallicRoughness)?,
                normal_texture: map(SurfaceChannel::Normal)?,
                occlusion_texture: map(SurfaceChannel::Occlusion)?,
                emissive_texture: map(SurfaceChannel::Emissive)?,
            };
            let index = materials.len();
            materials.push(out);
            ids.insert(*id, index);
            index
        };
        assignments.push(GlbPbrPrimitive {
            mesh: 0,
            primitive,
            material,
            colors: part.colors.clone(),
            tangents: part.tangents.clone(),
        });
    }
    ctx.checkpoint((derived_bytes + glb.len()) as u64)?;
    let output = augment_glb_pbr(glb, &materials, &assignments)
        .map_err(|_| Error::Invalid("GLB PBR augmentation"))?;
    ctx.checkpoint(output.len() as u64)?;
    Ok(output)
}
fn part_tangents(part: &Part, ctx: &mut mesh::Context<'_>) -> Result<Vec<[f32; 4]>> {
    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| a[i] - b[i])
    }
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a.iter().zip(b).map(|(a, b)| a * b).sum()
    }
    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }
    fn norm(a: [f32; 3]) -> [f32; 3] {
        let len = dot(a, a).sqrt();
        if len > 1e-20 {
            a.map(|v| v / len)
        } else {
            [1., 0., 0.]
        }
    }
    let mut out = vec![[0.; 4]; part.positions.len()];
    for ids in part.indices.chunks_exact(3) {
        ctx.checkpoint(3)?;
        let [a, b, c] = [ids[0] as usize, ids[1] as usize, ids[2] as usize];
        let p1 = sub(part.positions[b], part.positions[a]);
        let p2 = sub(part.positions[c], part.positions[a]);
        let uv = |i: usize| {
            [
                part.uvs[i][0] - part.uvs[a][0],
                part.uvs[i][1] - part.uvs[a][1],
            ]
        };
        let u1 = uv(b);
        let u2 = uv(c);
        let det = u1[0] * u2[1] - u1[1] * u2[0];
        for id in [a, b, c] {
            let n = norm(part.normals[id]);
            let (t, b) = if det.abs() > 1e-16 {
                (
                    std::array::from_fn(|i| (p1[i] * u2[1] - p2[i] * u1[1]) / det),
                    std::array::from_fn(|i| (p2[i] * u1[0] - p1[i] * u2[0]) / det),
                )
            } else {
                let axis = if n[2].abs() < 0.9 {
                    [0., 0., 1.]
                } else {
                    [0., 1., 0.]
                };
                let t = norm(cross(axis, n));
                (t, cross(n, t))
            };
            let t = norm(std::array::from_fn(|i| t[i] - n[i] * dot(n, t)));
            let sign = if dot(cross(n, t), b) < 0. { -1. } else { 1. };
            out[id] = [t[0], t[1], t[2], sign];
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use makepad_gltf::GlbPbrTexture;
    thread_local! {
        /// Texture builds [`shared_texture`] ran on this thread.
        pub(super) static BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    fn bytes(t: &Option<GlbPbrTexture>) -> Option<(Vec<u8>, Vec<Vec<u8>>)> { t.as_ref().map(|t| (t.png.clone(), t.mip_pngs.clone())) }

    #[test]
    fn texture_key_covers_every_input() {
        let limits = Limits::default();
        let image = |w, h, v| RgbaImage::new(w, h, [v, 20, 30, 255], &limits).unwrap();
        let mut layer = SurfaceLayer::new("paint", image(4, 4, 10));
        layer.mask = Some(vec![200; 16]);
        layer.pattern = Some(SurfacePattern { kind: PatternKind::Checker, color_a: [0.1, 0.2, 0.3, 1.], color_b: [0.4, 0.5, 0.6, 1.], scale: [2., 3.], seed: 7 });
        layer.derived = Some(SurfaceDerivation { source_channel: SurfaceChannel::BaseColor, source_layer: Some("height".into()), strength: 1., roughness_min: 0.2, roughness_max: 0.8, metallic: 0.1, wrap: true });
        layer.mips = vec![image(2, 2, 11)];
        let base = (SurfaceChannel::BaseColor, false, limits.clone(), vec![layer, SurfaceLayer::new("top", image(4, 4, 40))]);
        type Input = (SurfaceChannel, bool, Limits, Vec<SurfaceLayer>);
        let edits: Vec<(&str, Box<dyn Fn(&mut Input)>)> = vec![
            ("channel", Box::new(|i| i.0 = SurfaceChannel::Emissive)),
            ("msdf", Box::new(|i| i.1 = true)),
            ("texture dimension limit", Box::new(|i| i.2.max_texture_dimension -= 1)),
            ("byte limit", Box::new(|i| i.2.mesh.max_bytes -= 1)),
            ("layer order", Box::new(|i| i.3.swap(0, 1))),
            ("layer count", Box::new(|i| { i.3.pop(); })),
            ("id", Box::new(|i| i.3[0].id = "paint2".into())),
            ("width", Box::new(|i| { let l = &mut i.3[0]; l.image.width = 8; l.image.height = 2; })),
            ("height", Box::new(|i| { let l = &mut i.3[0]; l.image.height = 8; l.image.width = 2; })),
            ("pixels", Box::new(|i| i.3[0].image.pixels[5] ^= 1)),
            ("no mask", Box::new(|i| i.3[0].mask = None)),
            ("mask", Box::new(|i| i.3[0].mask.as_mut().unwrap()[3] = 1)),
            ("opacity", Box::new(|i| i.3[0].opacity = 0.5)),
            ("blend", Box::new(|i| i.3[0].blend = SurfaceBlend::Multiply)),
            ("visible", Box::new(|i| i.3[0].visible = false)),
            ("no pattern", Box::new(|i| i.3[0].pattern = None)),
            ("pattern kind", Box::new(|i| i.3[0].pattern.as_mut().unwrap().kind = PatternKind::Stripes)),
            ("pattern colour a", Box::new(|i| i.3[0].pattern.as_mut().unwrap().color_a[3] = 0.5)),
            ("pattern colour b", Box::new(|i| i.3[0].pattern.as_mut().unwrap().color_b[0] = -0.)),
            ("pattern scale", Box::new(|i| i.3[0].pattern.as_mut().unwrap().scale[1] = 4.)),
            ("pattern seed", Box::new(|i| i.3[0].pattern.as_mut().unwrap().seed = 8)),
            ("no derivation", Box::new(|i| i.3[0].derived = None)),
            ("derivation source", Box::new(|i| i.3[0].derived.as_mut().unwrap().source_channel = SurfaceChannel::Normal)),
            ("derivation layer", Box::new(|i| i.3[0].derived.as_mut().unwrap().source_layer = None)),
            ("derivation strength", Box::new(|i| i.3[0].derived.as_mut().unwrap().strength = 2.)),
            ("derivation roughness min", Box::new(|i| i.3[0].derived.as_mut().unwrap().roughness_min = 0.3)),
            ("derivation roughness max", Box::new(|i| i.3[0].derived.as_mut().unwrap().roughness_max = 0.9)),
            ("derivation metallic", Box::new(|i| i.3[0].derived.as_mut().unwrap().metallic = 0.2)),
            ("derivation wrap", Box::new(|i| i.3[0].derived.as_mut().unwrap().wrap = false)),
            ("no mips", Box::new(|i| i.3[0].mips.clear())),
            ("mip pixels", Box::new(|i| i.3[0].mips[0].pixels[0] ^= 1)),
            ("second layer", Box::new(|i| i.3[1].image.pixels[0] ^= 1)),
        ];
        let key = |i: &Input| texture_key(i.0, i.1, &i.2, &i.3);
        assert_eq!(key(&base), key(&base.clone()), "equal inputs give equal keys");
        let mut seen = std::collections::HashMap::from([(key(&base), "the base")]);
        for (name, edit) in &edits {
            let mut input = base.clone();
            edit(&mut input);
            if let Some(other) = seen.insert(key(&input), name) {
                panic!("changing the {name} leaves the key of {other}");
            }
        }
    }

    #[test]
    fn shared_texture_builds_once_shares_its_build_and_never_holds_a_ui_thread() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let texture = |v: u8| Some(GlbPbrTexture { png: vec![v; 4], mip_pngs: vec![vec![v; 2]] });
        let key = |label: &str| makepad_core_util::sha256::sha256(format!("shared_texture test: {label}").as_bytes());
        // A failure is not remembered: the next ask builds, and its result is.
        let failed = key("failure");
        assert!(shared_texture(failed, &mut || Err(Error::Budget("test"))).is_err());
        assert_eq!(bytes(&shared_texture(failed, &mut || Ok(texture(1))).unwrap()), bytes(&texture(1)), "a failed build is built again");
        assert_eq!(bytes(&shared_texture(failed, &mut || panic!("a remembered texture is not built")).unwrap()), bytes(&texture(1)));
        // Asks while the first build runs share it; a UI thread builds for itself instead of waiting;
        // an ask that waited on a build that failed builds for itself.
        for (label, first_fails) in [("shared", false), ("failed while shared", true)] {
            let key = key(label);
            let (other_builds, ui_builds) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let gate = std::sync::Mutex::new(());
            let (inside, first_inside) = std::sync::mpsc::channel();
            let (ui_done, ui_result) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                let held = gate.lock().unwrap();
                let first = scope.spawn(|| shared_texture(key, &mut || {
                    inside.send(()).unwrap();
                    drop(gate.lock().unwrap());
                    if first_fails { Err(Error::Budget("test")) } else { Ok(texture(2)) }
                }).map(|t| bytes(&t)));
                first_inside.recv().unwrap();
                let others: Vec<_> = (0..3).map(|_| scope.spawn(|| shared_texture(key, &mut || {
                    other_builds.fetch_add(1, SeqCst);
                    Ok(texture(2))
                }).map(|t| bytes(&t)))).collect();
                scope.spawn(|| {
                    makepad_core_util::ui_thread::mark_ui_thread();
                    ui_done.send(shared_texture(key, &mut || { ui_builds.fetch_add(1, SeqCst); Ok(texture(2)) }).map(|t| bytes(&t))).unwrap();
                });
                let ui = ui_result.recv_timeout(std::time::Duration::from_secs(60)).expect("a UI thread does not wait for another thread's build");
                assert_eq!(ui.unwrap(), bytes(&texture(2)));
                assert_eq!(ui_builds.load(SeqCst), 1, "with the first build still running, the UI thread builds it itself");
                drop(held);
                assert_eq!(first.join().unwrap().is_err(), first_fails, "the first ask gets its own build's result");
                for other in others { assert_eq!(other.join().unwrap().unwrap(), bytes(&texture(2)), "an ask that waited gets a texture"); }
            });
            if first_fails {
                assert!((1..=3).contains(&other_builds.load(SeqCst)), "asks that waited on a failed build build for themselves");
            } else {
                assert_eq!(other_builds.load(SeqCst), 0, "asks that came while the first built did not build it again");
            }
        }
    }

    #[test]
    fn models_that_share_a_material_build_its_texture_once() {
        let limits = Limits::default();
        let build = |shape: Operation| {
            let program = [
                ProgramOp::Op(shape),
                ProgramOp::Op(Operation::Surface(SurfaceOperation::Layer { material: 0, channel: SurfaceChannel::BaseColor,
                    layer: SurfaceLayer::new("paint", RgbaImage::new(32, 32, [255; 4], &limits).unwrap()) })),
                ProgramOp::Op(Operation::Surface(SurfaceOperation::Pattern { material: 0, channel: SurfaceChannel::BaseColor, layer: "paint".into(),
                    pattern: SurfacePattern { kind: PatternKind::Checker, color_a: [0.2, 0.3, 0.4, 1.], color_b: [0.9, 0.8, 0.1, 1.], scale: [3., 5.], seed: 0x7e47_3e30 } })),
                ProgramOp::Op(Operation::Surface(SurfaceOperation::Mips { material: 0, channel: SurfaceChannel::BaseColor, layer: "paint".into() })),
            ];
            let before = BUILDS.with(|b| b.get());
            let built = build_program(&program, limits.clone(), &mut |_| Err("no inputs".into()), None).unwrap();
            let loaded = makepad_gltf::load_gltf_from_bytes(&built.compiled.glb, None).unwrap();
            let pbr = loaded.document.materials_slice()[0].pbr_metallic_roughness.as_ref().unwrap();
            let image = loaded.document.textures_slice()[pbr.base_color_texture.as_ref().unwrap().index].source.unwrap();
            (BUILDS.with(|b| b.get()) - before, makepad_gltf::load_image_bytes(&loaded, image).unwrap())
        };
        let (first_builds, first) = build(Operation::Plane { object: "plate".into(), size: [2., 2.] });
        let (second_builds, second) = build(Operation::Cube { object: "block".into(), size: [1., 2., 3.] });
        assert_eq!(first_builds, 1, "the first model builds its texture");
        assert_eq!(second_builds, 0, "a model with another mesh and the same material takes the built texture");
        assert_eq!(first, second, "and embeds the same bytes");
    }
}
