//! Model loading: stock props, static previews, parsed GLB models.

use super::*;

impl Renderer {
    /// Load a stock prop onto the GPU under `id`, idempotent. `png` is the
    /// pack atlas, shared by every model in that pack.
    pub fn load_model(
        &mut self,
        cx: &mut Cx,
        id: &str,
        glb: &[u8],
        png: Option<&[u8]>,
    ) -> Result<usize, String> {
        self.load_model_with_ao(cx, id, glb, png, None, None)
    }

    /// [`load_model_with_ao`] plus the offline `.shadowsdf` bytes, for a
    /// model whose whole bake travelled with it (an asset-store manifest:
    /// render GLB + AoMesh + AoTexture + ShadowSdf roles). The SDF is kept
    /// per model id and resolved against the sun like the disk sidecar.
    pub fn load_model_with_bake(
        &mut self,
        cx: &mut Cx,
        id: &str,
        glb: &[u8],
        png: Option<&[u8]>,
        aomesh: Option<&[u8]>,
        ao_png: Option<&[u8]>,
        shadow_sdf: Option<&[u8]>,
    ) -> Result<usize, String> {
        if let Some(sdf) = shadow_sdf {
            if !self.model_sdf_bytes.contains_key(id) {
                self.model_sdf_bytes.insert(id.to_string(), sdf.to_vec());
            }
        }
        self.load_model_with_ao(cx, id, glb, png, aomesh, ao_png)
    }

    /// Same as [`load_model`], but the caller may hand the offline AO pair
    /// (`.aomesh` + `.ao.png` bytes) instead of relying on `models_root`.
    /// Import thumbnails use this so a Kenney kit looks like the baked game
    /// mesh, not a raw unlit GLB.
    pub fn load_model_with_ao(
        &mut self,
        cx: &mut Cx,
        id: &str,
        glb: &[u8],
        png: Option<&[u8]>,
        aomesh: Option<&[u8]>,
        ao_png: Option<&[u8]>,
    ) -> Result<usize, String> {
        if let Some(at) = self.static_models.iter().position(|(k, _)| k == id) {
            return Ok(self.static_models[at].1.triangles);
        }
        // An `.aomesh` sidecar carries the bake's mesh but never the atlas
        // ("by URI" — the pack's colormap beside a checkout GLB). A GLB with
        // its atlas EMBEDDED (every pack import, every generated model) has
        // no such file: without this the sidecar lane drew the model white
        // under its AO. The embedded base colour is the atlas, exactly as
        // parse_glb would have read it.
        let embedded_png = if png.is_none() && aomesh.is_some() {
            crate::model::embedded_base_color_png(glb)
        } else {
            None
        };
        let png = png.or(embedded_png.as_deref());
        // Prefer the prebaked sidecar. It carries the exact mesh the atlas was
        // baked against — including the ao_uv lane, which the runtime has no
        // way to reconstruct — so loading it is what makes the AO texture mean
        // anything. Absent or stale, the plain glb still renders, just unlit by
        // AO, which is the correct outcome for an unbaked library.
        let baked = aomesh
            .and_then(StaticModel::from_aomesh)
            .or_else(|| Self::load_aomesh(id));
        let model = match baked {
            Some(mut body) if glb.windows(b"vehicle_wheel".len()).any(|w| w == b"vehicle_wheel") => {
                // The AO sidecar intentionally serializes only the flattened
                // body stream. Driven parts stay in the original GLB because
                // their per-frame pose makes baked AO invalid. Reattach those
                // definitions without giving up the body's baked chart.
                let mut source = StaticModel::parse_glb(glb)?;
                body.driven_parts = std::mem::take(&mut source.driven_parts);
                body
            }
            Some(body) => body,
            None => StaticModel::parse_glb(glb)?,
        };
        self.load_model_parsed(cx, id, model, png, ao_png)
    }

    /// Install a complete generated preview. Geometry/colliders were prepared
    /// off-thread; this path does no parsing, decoding, disk lookup, lightmap
    /// preparation or mesh cloning. The old model stays visible until here.
    pub fn install_static_preview(
        &mut self, cx: &mut Cx, id: &str, prepared: PreparedStaticPreview, transient: bool,
    ) -> Option<usize> {
        let at = self.static_models.iter().position(|(key, _)| key == id);
        let uploaded = self.upload_static_preview(cx, prepared);
        let triangles = uploaded.triangles;
        self.model_sdf_tex.remove(id);
        let ao=uploaded.ao.clone();
        let model = Self::uploaded_static_model(uploaded);
        self.model_lod_chains_dirty = true;
        let previous = if let Some(at) = at { Some(std::mem::replace(&mut self.static_models[at].1, model)) }
            else { self.static_models.push((id.to_string(), model)); if transient { self.preview_new.insert(id.to_string()); } None };
        let old_ao_key = self.model_pack.iter().find(|(key, _)| key == id).map(|(_, key)| key.clone());
        self.model_pack.retain(|(key, _)| key != id);
        if let Some(ao)=ao{self.ao_textures.retain(|(key,_)|key!=id);self.ao_textures.push((id.to_string(),ao));self.model_pack.push((id.to_string(),id.to_string()));}
        if transient {
            if !self.preview_new.contains(id) {
                if let Some(previous) = previous { self.preview_originals.entry(id.to_string()).or_insert((previous, old_ao_key)); }
            }
        } else {
            self.preview_originals.remove(id);
        }
        self.rebuild_csm_static_casters();
        Some(triangles)
    }

    pub fn upload_static_preview(&mut self, cx:&mut Cx, prepared:PreparedStaticPreview)->UploadedStaticPreview {
        // Identical images (within this model, and across every model and
        // chunk already resident) share one GPU texture.
        let cache=&mut self.texture_cache;
        let mut shared=|cx:&mut Cx,prepared:std::sync::Arc<crate::material_surface::PreparedTexture>|->Texture{
            if let Some(texture)=cache.get(&prepared.hash){return texture.clone();}
            std::sync::Arc::try_unwrap(prepared).unwrap_or_else(|arc|(*arc).clone()).upload_cached(cx,cache)
        };
        let mut upload=|layer:PreparedStaticLayer| {
            let geometry=Geometry::new(cx);geometry.update(cx,layer.indices,layer.vertices);
            let shiny=layer.orm_on||layer.pbr.is_shiny();
            // Surface maps (normal, occlusion, emissive) share by content too:
            // terrain tiles carry the same material maps.
            let surface=layer.surface.map(|s|crate::material_surface::UploadedSurface{definition:std::sync::Arc::new(s.definition),
                normal:shared(cx,std::sync::Arc::new(s.normal)),occlusion:shared(cx,std::sync::Arc::new(s.occlusion)),emissive:shared(cx,std::sync::Arc::new(s.emissive))});
            let material=LayerMaterial{surface,metallic:if shiny{layer.pbr.metallic}else{0.0},roughness:layer.pbr.roughness,
                orm:shared(cx,layer.orm),orm_on:layer.orm_on,mag_nearest:layer.pbr.mag_nearest,cutout:layer.cutout};
            (std::rc::Rc::new(geometry),shared(cx,layer.texture),shared(cx,layer.detail),layer.detail_scale,material)
        };
        let wants_pbr=!prepared.prelit&&(prepared.main.orm_on||prepared.main.pbr.is_shiny()||prepared.extra.iter().any(|l|l.orm_on||l.pbr.is_shiny())||prepared.anim_parts.iter().flat_map(|p|&p.draws).chain(prepared.driven_parts.iter().flat_map(|p|&p.draws)).any(|l|l.orm_on||l.pbr.is_shiny()));
        let (geometry,texture,detail,detail_scale,material)=upload(prepared.main);
        let extra_draws=prepared.extra.into_iter().map(&mut upload).collect();
        let anim_parts=prepared.anim_parts.into_iter().map(|part|LoadedAnimPart{def:part.def,collider:part.collider,
            draws:part.draws.into_iter().map(&mut upload).collect()}).collect();
        let driven_parts=prepared.driven_parts.into_iter().map(|part|LoadedDrivenPart{def:part.def,
            draws:part.draws.into_iter().map(&mut upload).collect()}).collect();
        drop(upload);
        let sky=prepared.sky.map(|part|{let geometry=Geometry::new(cx);geometry.update(cx,part.indices,part.vertices);
            LoadedSky{part:part.part,geometry:std::rc::Rc::new(geometry),tex0:upload_generated_texture(cx,part.tex0),tex1:upload_generated_texture(cx,part.tex1),
                draw_trace:0,positions:part.positions,indices:part.mesh_indices}});
        let ao=prepared.ao.map(|image|upload_generated_texture(cx,image));
        let sdf=prepared.sdf.map(|atlas|Self::upload_sdf_atlas(cx,atlas));
        let bake_geometry=prepared.bake_stream.map(|(indices,vertices)|{let g=Geometry::new(cx);g.update(cx,indices,vertices);std::rc::Rc::new(g)});
        let morph=prepared.morph.map(|m|m.upload(cx));
        drop(shared);
        let lods=prepared.lods.into_iter().map(|(distance,model)|(distance,self.upload_static_preview(cx,model))).collect();
        UploadedStaticPreview{lods,morph,ao,sdf,bake_geometry,lm_source:prepared.lm_source,emitters:prepared.emitters,geometry,texture,detail,detail_scale,material,wants_pbr,
            prelit:prepared.prelit,triangles:prepared.mesh_indices.len()/3,min:prepared.min,max:prepared.max,
            authored_collisions:prepared.authored_collisions,collider_parts:prepared.collider_parts,occluder_parts:prepared.occluder_parts,positions:prepared.positions,indices:prepared.mesh_indices,
            extra_draws,anim_parts,driven_parts,sky}
    }
    pub(super) fn uploaded_static_model(uploaded:UploadedStaticPreview)->LoadedModel {
        LoadedModel{lods:uploaded.lods.into_iter().map(|(distance,model)|(distance,Self::uploaded_static_model(model))).collect(),morph:uploaded.morph,prepared_sdf:Some(uploaded.sdf),emitters:uploaded.emitters,geometry:uploaded.geometry,texture:uploaded.texture,detail:uploaded.detail,detail_scale:uploaded.detail_scale,
            extra_draws:uploaded.extra_draws,material:uploaded.material,wants_pbr:uploaded.wants_pbr,prelit:uploaded.prelit,triangles:uploaded.triangles,
            min:uploaded.min,max:uploaded.max,authored_collisions:uploaded.authored_collisions,collider_parts:uploaded.collider_parts,occluder_parts:uploaded.occluder_parts,
            anim_parts:uploaded.anim_parts,driven_parts:uploaded.driven_parts,sky:uploaded.sky,mesh_positions:uploaded.positions,mesh_indices:uploaded.indices,
            lm_source:uploaded.lm_source,bake_geometry:uploaded.bake_geometry}
    }

    /// Register pre-uploaded generated geometry. Payload clones contain only
    /// shared handles, immutable Arc buffers and the bounded part list.
    pub fn install_uploaded_static(&mut self, id: &str, uploaded: UploadedStaticPreview) -> usize {
        let triangles = uploaded.triangles;
        self.model_sdf_tex.remove(id);
        let ao=uploaded.ao.clone();
        let model = Self::uploaded_static_model(uploaded);
        self.model_lod_chains_dirty = true;
        if let Some((_, old)) = self.static_models.iter_mut().find(|(key, _)| key == id) { *old = model; }
        else { self.static_models.push((id.to_string(), model)); }
        self.model_pack.retain(|(key, _)| key != id);
        if let Some(ao)=ao{self.ao_textures.retain(|(key,_)|key!=id);self.ao_textures.push((id.to_string(),ao));self.model_pack.push((id.to_string(),id.to_string()));}
        self.preview_originals.remove(id);
        self.preview_new.remove(id);
        self.rebuild_csm_static_casters();
        self.prune_texture_cache();
        triangles
    }

    /// Drop cached textures no loaded model references any more (the cache's
    /// refcount is the set of live models; O(models) and only on replace or
    /// unload, never per frame).
    pub(super) fn prune_texture_cache(&mut self) {
        if self.texture_cache.is_empty() {
            return;
        }
        fn collect(m: &LoadedModel, out: &mut Vec<TextureId>) {
            let mut layer = |t: &Texture, d: &Texture, mat: &LayerMaterial| {
                out.push(t.texture_id());
                out.push(d.texture_id());
                out.push(mat.orm.texture_id());
            };
            layer(&m.texture, &m.detail, &m.material);
            for (_, t, d, _, mat) in m.extra_draws.iter()
                .chain(m.anim_parts.iter().flat_map(|p| p.draws.iter()))
                .chain(m.driven_parts.iter().flat_map(|p| p.draws.iter()))
            {
                layer(t, d, mat);
            }
            for (_, lod) in &m.lods {
                collect(lod, out);
            }
        }
        let mut live = Vec::new();
        for (_, m) in &self.static_models {
            collect(m, &mut live);
        }
        self.texture_cache.retain(|_, t| live.contains(&t.texture_id()));
    }

    pub fn restore_all_static_previews(&mut self) {
        let mut aliases: Vec<_> = self.preview_originals.keys().cloned().collect();
        aliases.extend(self.preview_new.iter().cloned());
        for alias in aliases { self.restore_static_preview(&alias); }
    }

    /// A cleared or unsupported draft restores its resident durable model
    /// without any synchronous network read or GLB/skin parse.
    pub fn restore_static_preview(&mut self, id: &str) {
        if self.preview_new.remove(id) { self.static_models.retain(|(key, _)| key != id); self.rebuild_csm_static_casters(); }
        if let Some((original, ao_key)) = self.preview_originals.remove(id) {
            if let Some((_, model)) = self.static_models.iter_mut().find(|(key, _)| key == id) {
                *model = original;
                if let Some(key) = ao_key { self.model_pack.push((id.to_string(), key)); }
                self.rebuild_csm_static_casters();
            }
        }
    }

    /// Legacy static installation over an already parsed model. Collider and
    /// lightmap preparation still run here; asynchronous generated previews
    /// must use `PreparedStaticPreview` and `install_static_preview` instead.
    ///
    /// Existing callers retain baked/textured/animated-world support here.
    /// Parsing elsewhere does not make this legacy method an upload-only
    /// API: collider derivation, texture decode and disk AO remain below.
    pub fn load_model_parsed(
        &mut self,
        cx: &mut Cx,
        id: &str,
        mut model: StaticModel,
        png: Option<&[u8]>,
        ao_png: Option<&[u8]>,
    ) -> Result<usize, String> {
        if let Some(at) = self.static_models.iter().position(|(k, _)| k == id) {
            return Ok(self.static_models[at].1.triangles);
        }
        // NO BAKE AT LOAD. AO is an offline product (tools/ao_bake); the game
        // loads it or goes without. Baking here cost every launch for an
        // answer that never changes.
        let pack: String = id.split('/').take(2).collect::<Vec<_>>().join("/");
        // Taken out first: the parts are uploaded on their own below, and
        // everything after this point sees the model exactly as it did
        // before doors and skies existed.
        let anim_defs = std::mem::take(&mut model.anim_parts);
        let driven_defs = std::mem::take(&mut model.driven_parts);
        let sky_def = model.sky.take();
        let triangles = model.triangle_count();
        let (min, max) = (model.min, model.max);
        // Triangle-derived voxel boxes are the collider truth: measured
        // against the real kits, per-primitive parts are usually ONE merged
        // blob (see model.rs real_asset_tests). Primitive curation only as
        // the degenerate-mesh fallback — and always as the OCCLUDER set,
        // where few clean boxes beat many exact ones.
        let occluder_parts = model.collider_parts();
        let collider_parts = {
            let v = model.voxel_collider_boxes();
            if v.is_empty() { occluder_parts.clone() } else { v }
        };
        let stride = crate::model::MODEL_VERTEX_FLOATS;
        // The light baker's raycaster triangles, captured BEFORE the GPU
        // upload consumes the packed stream. Only `lm_source` holders use
        // them, but whether this model holds one is not known until its AO
        // atlas resolves below.
        let lm_positions: Vec<Vec3f> = (0..model.vertices.len() / stride)
            .map(|i| {
                vec3f(
                    model.vertices[i * stride],
                    model.vertices[i * stride + 1],
                    model.vertices[i * stride + 2],
                )
            })
            .collect();
        let lm_indices = model.indices.clone();
        // The light baker's per-vertex lanes, pulled out of the packed stream
        // BEFORE the GPU upload consumes it. ao_uv is the chart layout the
        // lightmap region reuses; the tint bytes stand in for albedo in the
        // bounce pass.
        let lm_ao_uv: Vec<[f32; 2]> = (0..model.vertices.len() / stride)
            .map(|i| crate::model::unpack_ao_uv(model.vertices[i * stride + 6]))
            .collect();
        let lm_albedo: Vec<Vec3f> = (0..model.vertices.len() / stride)
            .map(|i| {
                // pack_unorm8x4 order: r | g<<8 | b<<16 | ao<<24.
                let bits = model.vertices[i * stride + 5].to_bits();
                vec3f(
                    (bits & 255) as f32 / 255.0,
                    ((bits >> 8) & 255) as f32 / 255.0,
                    ((bits >> 16) & 255) as f32 / 255.0,
                )
            })
            .collect();
        // The bake variant's vertex stream, captured BEFORE the render
        // upload consumes the model: per-triangle flat winding normals in
        // the nrm lane (see `LoadedModel::bake_geometry`).
        let bake_stream = {
            let mut verts = Vec::with_capacity(model.indices.len() * stride);
            let mut idx = Vec::with_capacity(model.indices.len());
            for tri in model.indices.chunks_exact(3) {
                let p = |i: u32| {
                    let o = i as usize * stride;
                    vec3f(
                        model.vertices[o],
                        model.vertices[o + 1],
                        model.vertices[o + 2],
                    )
                };
                let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
                let n = Vec3f::cross(b - a, c - a);
                let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
                let n = if l > 1.0e-12 {
                    vec3f(n.x / l, n.y / l, n.z / l)
                } else {
                    vec3f(0.0, 1.0, 0.0)
                };
                let (ox, oy) = crate::skin::oct_encode(n);
                let packed_n = makepad_draw::pack_pair_f16(ox, oy);
                for i in tri {
                    let o = *i as usize * stride;
                    idx.push(verts.len() as u32 / stride as u32);
                    verts.extend_from_slice(&model.vertices[o..o + stride]);
                    let at = verts.len() - stride + 3;
                    verts[at] = packed_n;
                }
            }
            (idx, verts)
        };
        let geometry = Geometry::new(cx);
        let multi = model.draw_layers.len() > 1;
        if multi {
            geometry.update(
                cx,
                model.draw_layers[0].indices.clone(),
                model.draw_layers[0].vertices.clone(),
            );
        } else {
            geometry.update(cx, model.indices, model.vertices);
        }
        // Two Kenney conventions, one path. A pack that UV-maps into an atlas
        // needs that atlas — missing it would render white, which reads as a
        // broken model, so it is an error. A pack that carries no texture at
        // all (nature-kit: flat per-material colours) is correct with a white
        // 1x1, because model.rs baked those colours into the vertex tint and
        // the shader multiplies the two. Self-contained GLBs (generated
        // meshes with a baked atlas) embed their base color in the BIN chunk
        // — used when the caller supplies no atlas.
        //
        // Multi-tile worlds ignore the caller's single PNG override: that is
        // always image 0, and using it for every layer is what made the walk
        // view and the GPU thumbnail smear one tile across the map.
        let main_png = if multi {
            model.draw_layers[0].texture_png.as_deref()
        } else {
            png.or(model.texture_png.as_deref())
        };
        let texture = match (main_png, model.texture_uri.as_deref()) {
            (Some(bytes), _) => crate::texture_pack::image_buffer(bytes)
                .map_err(|e| format!("{id}: atlas decode failed: {e:?}"))?
                .into_new_mip_repeat_texture(cx),
            (None, None) => {
                let mut white = ImageBuffer::default();
                white.width = 1;
                white.height = 1;
                white.data = vec![0xFFFF_FFFF];
                white.into_new_texture(cx)
            }
            (None, Some(uri)) => {
                return Err(format!(
                    "{id}: atlas {uri} missing — run apps/commercial/sandbox/download_assets.sh"
                ))
            }
        };
        // Which shader this model draws with, decided here and never again.
        // A prelit map keeps the diffuse lane unconditionally: its COLOR_0 IS
        // the light, and laying a sun-driven highlight over a baked Quake
        // lightmap would put a moving specular on a wall the file already
        // finished lighting.
        let wants_pbr = !model.prelit
            && (model.pbr.is_shiny() || model.draw_layers.iter().any(|l| l.pbr.is_shiny()));
        let extra_draws = if multi {
            let mut extra = Vec::with_capacity(model.draw_layers.len() - 1);
            for (i, layer) in model.draw_layers.iter().enumerate().skip(1) {
                if layer.indices.len() < 3 {
                    continue;
                }
                let mat = self.upload_material(cx, &layer.pbr);
                let tex = match layer.texture_png.as_deref() {
                    Some(bytes) => crate::texture_pack::image_buffer(bytes)
                        .map_err(|e| format!("{id}: layer {i} atlas decode failed: {e:?}"))?
                        .into_new_mip_repeat_texture(cx),
                    None => {
                        let mut white = ImageBuffer::default();
                        white.width = 1;
                        white.height = 1;
                        white.data = vec![0xFFFF_FFFF];
                        white.into_new_texture(cx)
                    }
                };
                let (det, dscale) = self.upload_detail(
                    cx,
                    layer.detail_png.as_deref(),
                    layer.detail_scale,
                );
                let g = Geometry::new(cx);
                g.update(cx, layer.indices.clone(), layer.vertices.clone());
                extra.push((std::rc::Rc::new(g), tex, det, dscale, mat));
            }
            extra
        } else {
            Vec::new()
        };
        let (main_detail, main_detail_scale) = if multi {
            let layer0 = &model.draw_layers[0];
            self.upload_detail(cx, layer0.detail_png.as_deref(), layer0.detail_scale)
        } else {
            self.upload_detail(cx, model.detail_png.as_deref(), model.detail_scale)
        };
        let main_material = if multi {
            self.upload_material(cx, &model.draw_layers[0].pbr)
        } else {
            self.upload_material(cx, &model.pbr)
        };
        // Anim parts (doors, lifts). Their layers go through the same upload
        // as the model's own, but a part usually re-uses a texture the level
        // already has — a door is skinned with one of the level's tiles — so
        // an identical PNG binds the SAME texture rather than a second copy.
        // That keeps a part in its parent's batch instead of splitting it.
        let anim_parts: Vec<LoadedAnimPart> = {
            let mut out = Vec::with_capacity(anim_defs.len());
            for def in anim_defs {
                let mut draws = Vec::with_capacity(def.layers.len());
                for (li, layer) in def.layers.iter().enumerate() {
                    if layer.indices.len() < 3 {
                        continue;
                    }
                    let resident = if multi {
                        model
                            .draw_layers
                            .iter()
                            .position(|l| l.texture_png == layer.texture_png)
                            .and_then(|k| match k {
                                0 => Some(texture.clone()),
                                k => extra_draws.get(k - 1).map(|(_, t, _, _, _)| t.clone()),
                            })
                    } else if main_png.is_some() && main_png == layer.texture_png.as_deref() {
                        Some(texture.clone())
                    } else {
                        None
                    };
                    let tex = match resident {
                        Some(t) => t,
                        None => match layer.texture_png.as_deref() {
                            Some(bytes) => crate::texture_pack::image_buffer(bytes)
                                .map_err(|e| {
                                    format!(
                                        "{id}: part {} layer {li} atlas decode failed: {e:?}",
                                        def.name
                                    )
                                })?
                                .into_new_mip_repeat_texture(cx),
                            None => {
                                let mut white = ImageBuffer::default();
                                white.width = 1;
                                white.height = 1;
                                white.data = vec![0xFFFF_FFFF];
                                white.into_new_texture(cx)
                            }
                        },
                    };
                    let (det, dscale) =
                        self.upload_detail(cx, layer.detail_png.as_deref(), layer.detail_scale);
                    let g = Geometry::new(cx);
                    g.update(cx, layer.indices.clone(), layer.vertices.clone());
                    let material=self.upload_material(cx,&layer.pbr);
                    draws.push((std::rc::Rc::new(g), tex, det, dscale,material));
                }
                if draws.is_empty() {
                    continue;
                }
                let collider = def.collider_boxes();
                out.push(LoadedAnimPart { def: std::sync::Arc::new(def), draws, collider: std::sync::Arc::new(collider) });
            }
            out
        };
        let driven_parts: Vec<LoadedDrivenPart> = {
            let mut out = Vec::with_capacity(driven_defs.len());
            for def in driven_defs {
                let mut draws = Vec::with_capacity(def.layers.len());
                for (li, layer) in def.layers.iter().enumerate() {
                    if layer.indices.len() < 3 {
                        continue;
                    }
                    let resident = if multi {
                        model
                            .draw_layers
                            .iter()
                            .position(|l| l.texture_png == layer.texture_png)
                            .and_then(|k| match k {
                                0 => Some(texture.clone()),
                                k => extra_draws.get(k - 1).map(|(_, t, _, _, _)| t.clone()),
                            })
                    } else if main_png.is_some() && main_png == layer.texture_png.as_deref() {
                        Some(texture.clone())
                    } else {
                        None
                    };
                    let tex = match resident {
                        Some(t) => t,
                        None => match layer.texture_png.as_deref() {
                            Some(bytes) => crate::texture_pack::image_buffer(bytes)
                                .map_err(|e| {
                                    format!(
                                        "{id}: driven part {} layer {li} atlas decode failed: {e:?}",
                                        def.connection
                                    )
                                })?
                                .into_new_mip_repeat_texture(cx),
                            None => {
                                let mut white = ImageBuffer::default();
                                white.width = 1;
                                white.height = 1;
                                white.data = vec![0xFFFF_FFFF];
                                white.into_new_texture(cx)
                            }
                        },
                    };
                    let (det, dscale) =
                        self.upload_detail(cx, layer.detail_png.as_deref(), layer.detail_scale);
                    let g = Geometry::new(cx);
                    g.update(cx, layer.indices.clone(), layer.vertices.clone());
                    let material=self.upload_material(cx,&layer.pbr);
                    draws.push((std::rc::Rc::new(g), tex, det, dscale,material));
                }
                if !draws.is_empty() {
                    out.push(LoadedDrivenPart { def:std::sync::Arc::new(def), draws });
                }
            }
            out
        };
        // The map's sky faces: one geometry, up to two layer images. A
        // missing or undecodable layer falls back to a 1x1 rather than
        // failing the whole map — a wrong sky is a wrong sky, but no map is
        // better than a wrong sky only in a unit test.
        let sky = match sky_def {
            Some(part) => {
                let stride = crate::model::MODEL_VERTEX_FLOATS;
                let positions: Vec<Vec3f> = (0..part.vertices.len() / stride)
                    .map(|i| {
                        vec3f(
                            part.vertices[i * stride],
                            part.vertices[i * stride + 1],
                            part.vertices[i * stride + 2],
                        )
                    })
                    .collect();
                let indices = part.indices.clone();
                let g = Geometry::new(cx);
                g.update(cx, part.indices.clone(), part.vertices.clone());
                let mips = sky_wants_mips(part.projection);
                let mut layer = |i: usize, fallback: u32| -> (Texture, String) {
                    let Some(png) = part.images.get(i) else {
                        let mut flat = ImageBuffer::default();
                        flat.width = 1;
                        flat.height = 1;
                        flat.data = vec![fallback];
                        return (flat.into_new_texture(cx), "absent (1x1 fallback)".into());
                    };
                    let img = match crate::texture_pack::image_buffer(png) {
                        Ok(img) => img,
                        Err(error) => {
                            let mut flat = ImageBuffer::default();
                            flat.width = 1;
                            flat.height = 1;
                            flat.data = vec![fallback];
                            return (
                                flat.into_new_texture(cx),
                                format!("decode failed ({error:?}; 1x1 fallback)"),
                            );
                        }
                    };
                    let (width, height) = (img.width, img.height);
                    let texture = if mips {
                        img.into_new_mip_repeat_texture(cx)
                    } else {
                        // Explicitly one level: a Doom cylinder is always
                        // magnified, and a mip chain turns atan2's longitude
                        // cut into a visible hairline.
                        Texture::new_with_format(
                            cx,
                            TextureFormat::VecBGRAu8_32 {
                                width,
                                height,
                                data: Some(img.data),
                                updated: TextureUpdated::Full,
                            },
                        )
                    };
                    (texture, format!("embedded {width}x{height}"))
                };
                let (tex0, tex0_source) = layer(0, 0xFFFF_FFFF);
                // Transparent: the front layer keys the back one through
                // its alpha, so an absent second layer must add nothing.
                let (tex1, tex1_source) = layer(1, 0x0000_0000);
                if sky_trace_enabled() {
                    log!(
                        "map sky upload: model='{id}' texture='{}' projection={} tris={} \
                         layer0={} id={:?}, layer1={} id={:?}",
                        part.texture.as_deref().unwrap_or("<unnamed>"),
                        part.projection.as_str(),
                        part.triangle_count(),
                        tex0_source,
                        tex0.texture_id(),
                        tex1_source,
                        tex1.texture_id(),
                    );
                }
                Some(LoadedSky {
                    tex0,
                    tex1,
                    draw_trace: 0,
                    geometry: std::rc::Rc::new(g),
                    part: std::sync::Arc::new(part),
                    positions: std::sync::Arc::new(positions),
                    indices: std::sync::Arc::new(indices),
                })
            }
            None => None,
        };
        // The pack's prebaked atlas, if tools/ao_bake has produced one. No
        // atlas simply means no AO for that pack, not an error: a game must
        // still run against a library that was never baked.
        // A PER-MODEL atlas wins over the pack's when one exists.
        //
        // Sharing one 1024x1024 atlas across a 40-model pack leaves each model
        // about 26k texels — a 162x162 square for a whole house. Measured, that
        // puts window-frame and door trim at a quarter of a texel across, and
        // sub-texel geometry reads its neighbour's occlusion instead of its
        // own. A model baked alone gets the entire texture, which is 40x the
        // density on exactly the small features where the AO was wrong.
        //
        // Costs a texture bind per model rather than per pack, so this is for
        // models that earn it, not a blanket default — absent a per-model file
        // the pack atlas is still used and batching is unchanged.
        let mut model_ao_dims: Option<(usize, usize)> = None;
        let ao_key = match ao_png
            .and_then(|png| Self::gray_png_texture(cx, png))
            .or_else(|| Self::load_model_ao(cx, id))
        {
            Some((t, w, h)) => {
                if !self.ao_textures.iter().any(|(k, _)| k == id) {
                    self.ao_textures.push((id.to_string(), t));
                }
                model_ao_dims = Some((w, h));
                id.to_string()
            }
            None => {
                if !self.ao_textures.iter().any(|(k, _)| *k == pack) {
                    if let Some(t) = Self::load_pack_ao(cx, &pack) {
                        self.ao_textures.push((pack.clone(), t));
                    }
                }
                pack.clone()
            }
        };
        self.model_pack.push((id.to_string(), ao_key));

        // Only a model with its OWN AO layout gets a lightmap source: a
        // pack-shared layout would make every instance's region the size of
        // the whole pack atlas, mostly empty.
        let lm_source = model_ao_dims.map(|(ao_w, ao_h)| {
            std::sync::Arc::new(crate::lightmap::LmMeshSource {
                caster: crate::ao::MeshRaycaster::new(
                    lm_positions.clone(),
                    lm_indices.clone(),
                    min,
                    max,
                ),
                ao_uv: lm_ao_uv,
                albedo: lm_albedo,
                ao_w,
                ao_h,
            })
        });
        // Only models the light baker will see pay for the flat-normal
        // variant (lm_source holders get regions; everything else casts via
        // positions alone, where the render geometry serves).
        let bake_geometry = lm_source.is_some().then(|| {
            let g = Geometry::new(cx);
            g.update(cx, bake_stream.0, bake_stream.1);
            std::rc::Rc::new(g)
        });

        self.static_models.push((
            id.to_string(),
            LoadedModel {
                lods:Vec::new(),
                morph:None,
                prepared_sdf:None,
                emitters: Default::default(),
                geometry: std::rc::Rc::new(geometry),
                texture,
                detail: main_detail,
                detail_scale: main_detail_scale,
                extra_draws,
                material: main_material,
                wants_pbr,
                prelit: model.prelit,
                triangles,
                min,
                max,
                authored_collisions: Default::default(),
                collider_parts: std::sync::Arc::new(collider_parts),
                occluder_parts: std::sync::Arc::new(occluder_parts),
                anim_parts,
                driven_parts,
                sky,
                mesh_positions: std::sync::Arc::new(lm_positions),
                mesh_indices: std::sync::Arc::new(lm_indices),
                lm_source,
                bake_geometry,
            },
        ));
        self.rebuild_csm_static_casters();
        Ok(triangles)
    }

    /// Drop a resident model's caches so the NEXT `load_model*` call for
    /// `id` re-parses its GLB and re-reads its bake sidecars instead of
    /// hitting the early-return at the top of `load_model_with_ao`. For a
    /// republished asset-store revision: the caller drops its own RAM byte
    /// memo FIRST (so the reload streams fresh bytes rather than reusing
    /// the stale ones), then this drops the GPU-resident geometry/AO/SDF —
    /// mirroring every per-id side table `load_model_with_ao` writes.
    ///
    /// `placed_models` is untouched: a still-placed instance of `id` simply
    /// stops drawing (its geometry lookup fails) until the caller's next
    /// `load_model*` call for the same id lands, typically the same frame.
    ///
    /// A per-model AO texture (`ao_textures` keyed by `id` itself) is
    /// dropped too; a PACK-shared atlas (keyed by the pack prefix) is left
    /// alone — other resident models from the same pack still bind it.
    ///
    /// Returns whether `id` was actually resident.
    pub fn unload_model(&mut self, id: &str) -> bool {
        self.preview_originals.remove(id);
        let had = if let Some(at) = self.static_models.iter().position(|(k, _)| k == id) {
            self.static_models.remove(at);
            true
        } else {
            false
        };
        // Its doors go with it: a reload re-parses the GLB, and a clock aimed
        // at a state index of the OLD parse has no meaning against the new
        // one.
        let slots: Vec<usize> = self
            .placed_models
            .iter()
            .enumerate()
            .filter(|(_, m)| m.model == id)
            .map(|(i, _)| i)
            .collect();
        self.model_anim_state.forget_model(id, &slots);
        self.model_pack.retain(|(k, _)| k != id);
        self.ao_textures.retain(|(k, _)| k != id);
        self.model_sdf_bytes.remove(id);
        self.model_sdf_tex.remove(id);
        if had {
            // Any placed instance of this id now points at a stale
            // lightmap chart region (a re-baked AO mesh may lay its
            // ao_uv out differently) — force the next scene-identity
            // check to re-kick the bake rather than trusting the old
            // remap, the same way `set_models` treats any other
            // placed-scene-relevant change.
            self.lm_remaps.clear();
            self.placed_scene_signature = None;
            self.models_rev = self.models_rev.wrapping_add(1);
            self.rebuild_csm_static_casters();
            self.prune_texture_cache();
        }
        had
    }
}
