use crate::{mesh, AnimationPath, Document, Error, Head, Result, Texture};
use makepad_gltf::{
    augment_glb_skin, replace_glb_node_animations, write_glb_mesh_textured_parts,
    write_glb_mesh_textured_parts_linear, GlbAnimPath, GlbNodeAnimChannel, GlbNodeAnimClip,
    GlbPrimitiveSkin, GlbSkinJoint, GlbTexturedPart,
};
use std::collections::BTreeMap;

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
    pub head: Head,
    pub glb: Vec<u8>,
    pub vertices: usize,
    pub triangles: usize,
    pub bounds: [[f32; 3]; 2],
    pub primitives: Vec<PrimitiveSource>,
}
#[derive(Default)]
struct Part {
    source_vertices: Vec<mesh::VertexId>,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    faces: Vec<mesh::FaceId>,
    weights: Vec<f64>,
    colors: Vec<[f32; 4]>,
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
    /// vertex must then have one to four known influences. Greater influence
    /// counts are refused rather than silently pruned. Editable rest TRS is
    /// applied by the final authoring pass; cubic controls bake at 60 Hz.
    /// Bounds are bind-pose bounds, not a conservative animated-motion envelope.
    /// Existing GLB augmentation stages are bounded and cancellation is checked
    /// between them; their inner serialization loops do not expose cancellation.
    /// Object/face provenance accompanies the result. The source document, not
    /// the render mesh, preserves editing identities and separate objects.
    pub(crate) fn compile_evaluated(
        &self,
        cancelled: Option<&dyn Fn() -> bool>,
    ) -> Result<CompiledModel> {
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
        if predicted_triangles > self.limits().mesh.max_triangles {
            return Err(Error::Budget("compiled triangles"));
        }
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
        let mut parts: BTreeMap<(String, u32), Part> = BTreeMap::new();
        let mut triangles = 0usize;
        for (name, mesh) in self.objects() {
            ctx.checkpoint(1)?;
            if joint_count == 0 && mesh.vertices().iter().any(|v| !v.weights.is_empty()) {
                return Err(Error::Invalid(
                    "weighted mesh requires a skeleton; static compile cannot discard skin weights",
                ));
            }
            let triangulated = mesh.triangulate(&mut ctx)?;
            triangles = triangles
                .checked_add(triangulated.triangles.len())
                .ok_or(Error::Budget("compiled triangles"))?;
            if triangles > self.limits().mesh.max_triangles {
                return Err(Error::Budget("compiled triangles"));
            }
            // Conservative bound includes temporary writer copies and accessors.
            if triangles.saturating_mul(384) > self.limits().mesh.max_bytes {
                return Err(Error::Budget("compile working bytes"));
            }
            for tri in &triangulated.triangles {
                ctx.checkpoint(1)?;
                let part = parts.entry((name.to_owned(), tri.material)).or_default();
                part.faces.push(tri.source_face);
                for index in tri.indices {
                    let v = &triangulated.vertices[index as usize];
                    part.indices.push(
                        part.positions
                            .len()
                            .try_into()
                            .map_err(|_| Error::Budget("compiled vertices"))?,
                    );
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
                        if v.weights.len() > 4 {
                            return Err(Error::Invalid("skinned compile supports at most four influences; reduce weights explicitly"));
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
        }
        if triangles == 0 {
            return Err(Error::Invalid("model has no surface to compile"));
        }
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
            let atlas = crate::atlas::bake(self.materials(), &ranges, &atlas_limits, &mut ctx)?;
            for ((_, material), part) in &mut parts {
                for uv in &mut part.uvs {
                    ctx.checkpoint(1)?;
                    *uv = atlas.remap(*material, *uv);
                }
            }
            Some(atlas)
        } else {
            None
        };
        let white = Texture::solid(1, 1, [255; 3], self.limits())?.to_png(self.limits())?;
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
                source_vertices: part.source_vertices.clone(),
                positions: part.positions.clone(),
                normals: part.normals.clone(),
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
        let mut glb = if atlas.is_some() {
            write_glb_mesh_textured_parts_linear(&views, false)
        } else {
            write_glb_mesh_textured_parts(&views, false)
        };
        drop(views);
        if rich_surface {
            glb = export_surface(self, &glb, &parts, &mut ctx)?;
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
                    weights: part.weights,
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
                            if steps.saturating_add(1).saturating_mul(clip.channels.len())>self.limits().max_keyframes{return Err(Error::Budget("baked cubic clip frames"));}
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
        glb = crate::authoring_export::finish(self, glb, &primitives, &mut ctx)?;
        if glb.len() > self.limits().max_source_bytes {
            return Err(Error::Budget("compiled GLB"));
        }
        ctx.checkpoint(glb.len() as u64)?;
        Ok(CompiledModel {
            head: self.head(),
            glb,
            vertices,
            triangles,
            bounds,
            primitives,
        })
    }
}

fn export_surface(
    doc: &Document,
    glb: &[u8],
    parts: &BTreeMap<(String, u32), Part>,
    ctx: &mut mesh::Context<'_>,
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
                fallback = SurfaceMaterial::from_legacy(
                    doc.materials()
                        .get(id)
                        .ok_or(Error::Invalid("surface export material"))?,
                    doc.limits(),
                )?;
                &fallback
            };
            source.validate(doc.limits(), ctx)?;
            let mut map = |channel: SurfaceChannel| -> Result<Option<GlbPbrTexture>> {
                let Some(image) = source.flatten(channel, doc.limits(), ctx)? else {
                    return Ok(None);
                };
                let mips = image.mip_chain(channel.encoding(), doc.limits(), ctx)?;
                let png = image.to_png(doc.limits())?;
                let mut mip_pngs = Vec::new();
                derived_bytes = derived_bytes.saturating_add(png.len());
                for mip in mips {
                    ctx.checkpoint(mip.pixels.len() as u64)?;
                    let png = mip.to_png(doc.limits())?;
                    derived_bytes = derived_bytes.saturating_add(png.len());
                    mip_pngs.push(png);
                }
                if derived_bytes
                    .saturating_mul(4)
                    .saturating_add(doc.surface().memory_bytes())
                    .saturating_add(glb.len() * 3)
                    > doc.limits().mesh.max_bytes
                {
                    return Err(Error::Budget("PBR export images"));
                }
                Ok(Some(GlbPbrTexture { png, mip_pngs }))
            };
            let out = GlbPbrMaterial {
                fur: source.fur,
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
        let tangents = part_tangents(part, ctx)?;
        assignments.push(GlbPbrPrimitive {
            mesh: 0,
            primitive,
            material,
            colors: part.colors.clone(),
            tangents,
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
