//! World-model lanes: diffuse/PBR/custom model draws, sky faces, view models.

use super::*;

impl Renderer {
    /// Draw one world-model lane. Instances are grouped by model, so N
    /// copies of one prop cost ONE draw item with N instances rather than N
    /// draws — which is what makes a scene full of stock trees affordable.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_models_inner(
        &mut self,
        cx: &mut Cx3d,
        mut draw: ModelDraw<'_>,
        eye: Vec3f,
        instances: &[ModelInstance],
        lane: WorldModelLane,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if instances.is_empty() {
            return;
        }
        let pbr_lane = draw.is_pbr();
        let custom_name = match &draw {
            ModelDraw::Custom(name, _) => Some((*name).to_string()),
            _ => None,
        };
        self.bind_model_lane(cx, &mut draw, eye, fog, sun);
        // Dynamic lights. STATICS share one transient-only block (their
        // lamp light is baked; only the dl_split prefix reaches them, so a
        // zeroed lamp region costs nothing and keeps their per-model
        // batching whole). DYNAMICS are per-INSTANCE: each looks up ITS OWN
        // precomputed grid cell (O(1), light_grid.rs) with a positional
        // dead-band, so a car parked under a lamp holds a byte-identical
        // block frame after frame — the no-flicker contract. Different
        // blocks split draw items on purpose (the append test compares
        // uniforms); a handful of extra items beats a light popping.
        let (mut anch_x, mut anch_z, mut anch_n) = (0.0f64, 0.0f64, 0u32);
        for inst in instances {
            anch_x += inst.transform.v[12] as f64;
            anch_z += inst.transform.v[14] as f64;
            anch_n += 1;
        }
        let anchor_all = vec3f(
            (anch_x / anch_n.max(1) as f64) as f32,
            1.0,
            (anch_z / anch_n.max(1) as f64) as f32,
        );
        let empty_block = LightBlock::default();
        let mut static_block = [0.0f32; LIGHT_BLOCK_FLOATS];
        let static_split = if self.clustered_enabled { 0 } else { merge_transients_into_block(
            &empty_block,
            &self.frame_lights,
            self.frame_baked_count..self.frame_lights.len(),
            anchor_all,
            &mut self.light_rank,
            &mut static_block,
        ) };
        // Which block the draw_vars currently carry; statics rewrite only
        // after a dynamic instance changed it.
        let mut dynamic_block_active = true;
        // Sort by model so equal geometry+texture land adjacent: consecutive
        // add_instance calls with unchanged geometry and texture accumulate
        // into a single draw item.
        let mut order: Vec<usize> = (0..instances.len()).collect();
        order.sort_by(|a, b| instances[*a].model.cmp(&instances[*b].model));
        // Model slot per instance, resolved once through a map: a linear
        // search per instance is O(instances x models), which a streamed or
        // prop-heavy world turns into milliseconds.
        let slots: Vec<Option<usize>> = {
            let index: std::collections::HashMap<&str, usize> =
                self.static_models.iter().enumerate().map(|(i, (k, _))| (k.as_str(), i)).collect();
            instances.iter().map(|inst| index.get(inst.model.as_str()).copied()).collect()
        };
        let mut last: Option<(String,usize)> = None;
        for i in order {
            let inst = &instances[i];
            let dynamic = lane.is_dynamic(inst);
            let Some(at) = slots[i] else {
                continue;
            };
            let root = &self.static_models[at].1;
            let distance=crate::asset_lod::instance_distance(&inst.transform,eye);
            let mut fur_budget = 12_000usize.min(96_000usize.saturating_sub(stats.fur_triangles));
            let lod_index=root.lods.partition_point(|(threshold,_)|*threshold<=distance);
            let loaded=if lod_index==0{root}else{&root.lods[lod_index-1].1};
            // Lane filter. Both passes walk the same instance list — indices
            // address `lm_remaps` / `model_ground` / the light-cell key, so
            // they must not be renumbered — and each takes only the models
            // its shader owns.
            let uses_pbr_lane = self.pbr_materials_enabled && loaded.wants_pbr;
            let wanted_custom = inst.custom_material.as_ref().filter(|m| {
                custom_name.as_deref() == Some(m.name.as_str())
                    || self.custom_draws.get(&m.name).is_some_and(|d| d.draw_vars.can_instance())
            });
            if let Some(name) = custom_name.as_deref() {
                if wanted_custom.map(|m| m.name.as_str()) != Some(name) { continue; }
                if let (ModelDraw::Custom(_, d), Some(material)) = (&mut draw, wanted_custom) {
                    d.params = material.params;
                }
            } else if wanted_custom.is_some() || uses_pbr_lane != pbr_lane {
                continue;
            }
            // Hoisted: `loaded` borrows self, and the per-instance light
            // block below needs `&mut self` (cell hysteresis).
            draw.base().morph_ctl=Vec4f::default();
            if let Some(morph)=&loaded.morph{
                let target=match lane{WorldModelLane::Placed=>ModelTarget::Instance(i),WorldModelLane::Attachment=>ModelTarget::Attachment(i)};
                let weights=self.model_anim_state.morph_weights(&target,&inst.model,&morph.source);
                draw.base().morph_ctl=vec4(morph.source.width as f32,morph.source.height as f32,morph.source.vertices as f32,morph.source.targets as f32);
                draw.base().morph_weights0=vec4(weights[0],weights[1],weights[2],weights[3]);
                draw.base().morph_weights1=vec4(weights[4],weights[5],weights[6],weights[7]);
                draw.base().morph_weights2=vec4(weights[8],weights[9],weights[10],weights[11]);
                draw.base().morph_weights3=vec4(weights[12],weights[13],weights[14],weights[15]);
                draw.base().morph_weights4=vec4(weights[16],weights[17],weights[18],weights[19]);
                draw.base().morph_weights5=vec4(weights[20],weights[21],weights[22],weights[23]);
                draw.base().morph_weights6=vec4(weights[24],weights[25],weights[26],weights[27]);
                draw.base().morph_weights7=vec4(weights[28],weights[29],weights[30],weights[31]);
                if let Some(shader)=draw.base().draw_vars.draw_shader_id{if let Some(slot)=cx.draw_shaders[shader.index].mapping.textures.iter().position(|t|t.id==live_id!(morph_map)){draw.base().draw_vars.set_texture(slot,&morph.texture);}}
            }
            let tri_count = loaded.triangles;
            // Offscreen copies are skipped BEFORE packing, and a model whose
            // copies all fall outside never opens a draw item at all. The
            // shadow a prop casts is not affected: prop shadows live in the
            // merged static shadow mesh, which is drawn whole regardless.
            if let Some(frustum) = frustum {
                // Generic node clips can leave the authored rest bounds.
                // Their small, bounded part sets remain visible until posed
                // bounds are available; a rest-only cull would hide motion.
                // Fur can extend beyond the authored base geometry. The
                // validated recipe is bounded to 5 cm in model space.
                let fur_margin = vec3f(0.05, 0.05, 0.05);
                if loaded.anim_parts.is_empty() && !frustum.intersects_obb(root.min-fur_margin, root.max+fur_margin, &inst.transform) {
                    match lane {
                        WorldModelLane::Placed => stats.model_culled += 1,
                        WorldModelLane::Attachment => stats.world_attachment_culled += 1,
                    }
                    continue;
                }
            }
            let (layer_draws, prelit) = {
                let m = loaded;
                let mut layers = Vec::with_capacity(1 + m.extra_draws.len());
                // A model that is ALL anim parts (a lone door asset) has an
                // empty static stream and nothing to draw for layer 0.
                if m.triangles > 0 {
                    layers.push((
                        m.geometry.geometry_id(),
                        m.texture.clone(),
                        m.detail.clone(),
                        m.detail_scale,
                        m.material.clone(),
                    ));
                }
                for (g, t, d, s, mat) in &m.extra_draws {
                    layers.push((g.geometry_id(), t.clone(), d.clone(), *s, mat.clone()));
                }
                (layers, m.prelit)
            };
            // Rigid parts ride the PARENT's material: a door is cut from the
            // level tile it sits in, so its metal/roughness is the model's.
            let has_lightmap_source = loaded.lm_source.is_some();
            // The pack's baked occlusion, on slot 1. Packs share atlases, so
            // this changes only when the pack does — the sort above keeps
            // models of a pack adjacent, so it does not break batching.
            let ao_tex = self
                .model_pack
                .iter()
                .find(|(m, _)| *m == inst.model)
                .and_then(|(_, pack)| self.ao_textures.iter().find(|(k, _)| k == pack))
                .map(|(_, t)| t);
            if let Some(t) = ao_tex.filter(|_|lod_index==0) {
                draw.base().draw_vars.set_texture(1, t);
                draw.base().ao_enabled = 1.0;
                stats.ao_bound += 1;
            } else {
                draw.base().ao_enabled = 0.0;
                stats.ao_missing += 1;
            }
            draw.base().transform = inst.transform;
            draw.base().tint = inst.tint;
            draw.base().color_adjust_ctl = inst.color_adjust;
            // This copy's window into the light atlas; zero disables — a
            // dynamic prop or an unbaked model lights analytically as before.
            draw.base().lm_rect = if dynamic || !has_lightmap_source {
                Vec4f::default()
            } else {
                self.lm_remaps.get(i).copied().unwrap_or_default()
            };
            // Dynamic-light gate: dynamics sum every uniform slot (lamps
            // included), statics only the transient prefix — their lamp
            // light is already in the atlas RGB.
            draw.base().dl_apply = if dynamic { 1.0 } else { 0.0 };
            draw.base().depth_bias = inst.depth_order * 1.0e-3;
            if dynamic && !self.clustered_enabled {
                // This instance's OWN cell block + transients, and its
                // ground plane for the sun-ray-projected shadow sample.
                let (x, z) = (inst.transform.v[12], inst.transform.v[14]);
                let cell = self.stable_light_cell(lane.light_key(i), x, z);
                let block = match cell {
                    Some(c) => self.light_grid.block_of(c),
                    None => &empty_block,
                };
                let split = merge_transients_into_block(
                    block,
                    &self.frame_lights,
                    self.frame_baked_count..self.frame_lights.len(),
                    vec3f(x, inst.transform.v[13], z),
                    &mut self.light_rank,
                    &mut self.light_block_scratch,
                );
                write_light_block(cx.cx, &mut draw.base().draw_vars, &self.light_block_scratch, split);
                draw.base().ground_y = match lane {
                    WorldModelLane::Placed => self.model_ground.get(i),
                    WorldModelLane::Attachment => self.world_attachment_ground.get(i),
                }
                .copied()
                .unwrap_or(0.0);
                dynamic_block_active = true;
            } else if dynamic_block_active {
                write_light_block(cx.cx, &mut draw.base().draw_vars, &static_block, static_split);
                draw.base().ground_y = 0.0;
                dynamic_block_active = false;
            }
            if dynamic && self.clustered_enabled {
                draw.base().ground_y = match lane {
                    WorldModelLane::Placed => self.model_ground.get(i),
                    WorldModelLane::Attachment => self.world_attachment_ground.get(i),
                }.copied().unwrap_or(0.0);
            }
            if last.as_ref().is_none_or(|(model,level)|model!=&inst.model||*level!=lod_index) {
                match lane {
                    WorldModelLane::Placed => stats.model_draws += 1,
                    WorldModelLane::Attachment => stats.world_attachment_draws += 1,
                }
                last = Some((inst.model.clone(),lod_index));
            }
            match lane {
                WorldModelLane::Placed => {
                    stats.model_instances += 1;
                    stats.model_triangles += tri_count;
                }
                WorldModelLane::Attachment => {
                    stats.world_attachment_instances += 1;
                    stats.world_attachment_triangles += tri_count;
                }
            }
            for (geometry_id, texture, detail, dscale, material) in &layer_draws {
                draw.base().draw_vars.geometry_id = Some(*geometry_id);
                draw.base().draw_vars.set_texture(0, texture);
                draw.base().draw_vars.set_texture(5, detail);
                draw.base().detail_st = vec2f(dscale[0], dscale[1]);
                draw.base().prelit = if prelit { 1.0 } else { 0.0 };
                draw.set_material(cx.cx, material);
                stats.fur_triangles += draw.submit(cx, distance, &mut fur_budget);
            }
            // Rigid parts (doors, lifts). Each is one extra draw on the
            // PARENT's material — same shader, same textures, usually the
            // level tile it was cut from — placed at the instance transform
            // times where the part's state machine has it right now.
            //
            // A part carries no baked chart (lm_rect zeroed): it moves, so
            // the static atlas never had a window for it. Its shadow comes
            // from the realtime cascades, which see it as a mover.
            let parts: Vec<(Mat4f, usize, Vec<(GeometryId, Texture, Texture, [f32; 2],LayerMaterial)>)> = {
                let root=&self.static_models[at].1;
                let m = if lod_index==0{root}else{&root.lods[lod_index-1].1};
                let mut parts = Vec::with_capacity(m.anim_parts.len() + m.driven_parts.len());
                if !m.anim_parts.is_empty() {
                    let key = match lane {
                        WorldModelLane::Placed => ModelTarget::Instance(i),
                        // Attachments are not placed slots — a slot index
                        // would address someone else's instance — so their
                        // parts follow the per-MODEL command only.
                        WorldModelLane::Attachment => ModelTarget::Attachment(i),
                    };
                    parts.extend(m.anim_parts.iter().map(|p| {
                            let (_, _, _) =
                                self.model_anim_state.clock(&key, &inst.model, &p.def);
                            (
                                self.model_anim_state.transform(&key,&inst.model,&p.def),
                                p.def.indices.len() / 3,
                                p.draws
                                    .iter()
                                    .map(|(g, t, d, s,m)| {
                                        (g.geometry_id(), t.clone(), d.clone(), *s,m.clone())
                                    })
                                    .collect(),
                            )
                        }));
                }
                parts.extend(m.driven_parts.iter().map(|part| {
                    let pose = inst
                        .part_poses
                        .iter()
                        .find(|pose| pose.connection == part.def.connection)
                        .map(|pose| pose.transform)
                        .unwrap_or_else(|| part.def.rest_transform());
                    (
                        pose,
                        part.def.indices.len() / 3,
                        part
                            .draws
                            .iter()
                            .map(|(g, t, d, s,m)| (g.geometry_id(), t.clone(), d.clone(), *s,m.clone()))
                            .collect(),
                    )
                }));
                parts
            };
            for (pose, part_tris, part_draws) in &parts {
                draw.base().transform = Mat4f::mul(&inst.transform, pose);
                draw.base().lm_rect = Vec4f::default();
                for (geometry_id, texture, detail, dscale,material) in part_draws {
                    draw.base().draw_vars.geometry_id = Some(*geometry_id);
                    draw.base().draw_vars.set_texture(0, texture);
                    draw.base().draw_vars.set_texture(5, detail);
                    draw.base().detail_st = vec2f(dscale[0], dscale[1]);
                    draw.base().prelit = if prelit { 1.0 } else { 0.0 };
                    draw.set_material(cx.cx, material);
                    stats.fur_triangles += draw.submit(cx, distance, &mut fur_budget);
                }
                match lane {
                    WorldModelLane::Placed => stats.model_triangles += part_tris,
                    WorldModelLane::Attachment => {
                        stats.world_attachment_triangles += part_tris
                    }
                }
            }
            // A part opened its own draw item; the next instance of the same
            // model must re-bind its own transform rather than accumulate
            // into the part's.
            if !parts.is_empty() {
                last = None;
            }
        }
    }

    /// Everything a world-model lane binds ONCE before its instances: the
    /// cluster and GI tables, sun and fog, shading space, the specular eye,
    /// screen-space AO, the lightmap atlas, the cascades and the ground
    /// light field. Shared by the placed/attachment lanes and the streamed
    /// world lane (stream_draw.rs), so they can never shade differently.
    pub(super) fn bind_model_lane(
        &mut self,
        cx: &mut Cx3d,
        draw: &mut ModelDraw<'_>,
        eye: Vec3f,
        fog: (Vec3f, f32),
        sun: &SunLight,
    ) {
        let pbr_lane = draw.is_pbr();
        self.clustered.bind(cx.cx, &mut draw.base().draw_vars, self.clustered_enabled);
        self.gi.bind(cx.cx, &mut draw.base().draw_vars);
        {
            let draw = draw.base();
            sun.write_into(
                &mut draw.light_dir,
                &mut draw.sun_color,
                &mut draw.sun_sky,
                &mut draw.sun_ground,
            );
            draw.fog_color = fog.0;
            draw.fog_density = fog.1;
            draw.depth_clip = 1.0;
            draw.lm_debug = self.lm_debug;
        }
        // Shading space, once for the whole lane: written before anything
        // binds, so every draw item of the frame reads the same value.
        {
            let display = self.display_transform;
            let vars = &mut draw.base().draw_vars;
            vars.set_uniform(cx.cx, live_id!(display), &[display, 0.0, 0.0, 0.0]);
        }
        // The specular lobe is the one term in this renderer that needs the
        // camera. TRUE world position, matching v_csm.xyz / v_csm_n.
        if pbr_lane {
            let vars = &mut draw.base().draw_vars;
            vars.set_uniform(cx.cx, live_id!(eye), &[eye.x, eye.y, eye.z, 0.0]);
        }
        // Screen-space AO, once for the whole lane (both lanes pass through
        // here). Ambient-only by shader law; strength 0 = the slot is never
        // sampled, which is every game host's path.
        {
            let ssao = self.ssao.clone();
            let vars = &mut draw.base().draw_vars;
            match &ssao {
                Some((tex, strength)) => {
                    vars.set_texture(7, tex);
                    vars.set_uniform(cx.cx, live_id!(ssao_ctl), &[*strength, 0.0, 0.0, 0.0]);
                }
                None => {
                    vars.set_uniform(cx.cx, live_id!(ssao_ctl), &[0.0, 0.0, 0.0, 0.0]);
                }
            }
        }
        // One atlas for the whole static scene, so binding it here costs
        // nothing per instance and never breaks batching.
        let lm_tex = self.lightmap_texture(cx.cx);
        draw.base().draw_vars.set_texture(2, &lm_tex);
        {
            let csm = self.gpu_baker.csm_binding();
            Self::write_csm_uniforms(cx.cx, &mut draw.base().draw_vars, &csm, &lm_tex, 4, self.shadow_debug);
        }
        // The ground region, for DYNAMIC instances' baked sun shadow (a
        // driven car crossing a house's shadow darkens). A channel only —
        // their lamp light is analytic. The shadow-top plane rides along so
        // a dynamic lifted above the blocker rejects the ground's shadow.
        {
            let (top_tex, top_base, top_range) = self.lm_top_binding(cx.cx);
            draw.base().draw_vars.set_texture(3, &top_tex);
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_top_decode),
                &[top_base, top_range, 0.0, 0.0],
            );
            let (g_rect, g_world) = self.lm_ground.unwrap_or_default();
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_ground_rect),
                &[g_rect.x, g_rect.y, g_rect.z, g_rect.w],
            );
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_ground_world),
                &[g_world.x, g_world.y, g_world.z, g_world.w],
            );
        }
    }

    /// The specular half of the model pass: the same instance list, walked
    /// again for the models whose material carries shininess.
    ///
    /// The shader is the renderer's own, built on first use rather than lent
    /// through `SceneDraws` (the `sky_draw` pattern). That is deliberate: a
    /// host does not opt into PBR, a MODEL does, and every existing host —
    /// VJ, sandbox, the thumbnailer — gets the lane the moment it loads a
    /// shiny GLB without touching its widget or its script.
    ///
    /// Costs nothing when the scene has no shiny models: `wants_pbr` is a
    /// bool on an already-resident struct, so the early-out below is one
    /// linear scan of the instance list and no GPU work at all.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_pbr_models(
        &mut self,
        cx: &mut Cx3d,
        eye: Vec3f,
        instances: &[ModelInstance],
        lane: WorldModelLane,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if !self.pbr_materials_enabled {
            return;
        }
        let any = {
            let shiny: std::collections::HashSet<&str> = self.static_models.iter()
                .filter(|(_, m)| m.wants_pbr).map(|(k, _)| k.as_str()).collect();
            !shiny.is_empty() && instances.iter().any(|inst| shiny.contains(inst.model.as_str()))
        };
        if !any {
            return;
        }
        if self.pbr_draw.is_none() {
            // Held VM (a script-driven draw is mid-apply): try again next
            // frame rather than drawing with no shader.
            self.pbr_draw = cx
                .cx
                .try_with_vm(|vm| Box::new(DrawScenePbr::script_new_with_default(vm)));
        }
        let Some(mut draw) = self.pbr_draw.take() else {
            return;
        };
        self.draw_models_inner(
            cx,
            ModelDraw::Pbr(&mut draw),
            eye,
            instances,
            lane,
            fog,
            sun,
            frustum,
            stats,
        );
        self.pbr_draw = Some(draw);
    }

    /// Draw every placed map's sky surfaces.
    ///
    /// One draw item per map: the faces are already one geometry, and the
    /// whole point of the lane is that a sky costs the same whether it is
    /// two brushes or two hundred. Depth is written normally — the faces sit
    /// where the level put them — but nothing else about the world reaches
    /// them: no lightmap window, no cascades, no fog, no sun.
    pub(super) fn draw_sky_faces(
        &mut self,
        cx: &mut Cx3d,
        eye: Vec3f,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if !self
            .placed_models
            .iter()
            .any(|inst| self.model_sky(&inst.model).is_some())
        {
            return;
        }
        if self.sky_draw.is_none() {
            // Held VM (a script-driven draw is mid-apply): try again next
            // frame rather than drawing a sky with no shader.
            self.sky_draw = cx
                .cx
                .try_with_vm(|vm| Box::new(DrawSceneSkyMap::script_new_with_default(vm)));
        }
        let Some(mut draw) = self.sky_draw.take() else {
            if sky_trace_enabled() && !self.sky_draw_wait_logged {
                log!("map sky draw: waiting for DrawSceneSkyMap shader construction");
                self.sky_draw_wait_logged = true;
            }
            return;
        };
        self.sky_draw_wait_logged = false;
        let time = self.sky_time;
        let lin = self.lin_ctl();
        let instances = std::mem::take(&mut self.placed_models);
        for inst in &instances {
            let Some((_, loaded)) = self.static_models.iter_mut().find(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            let Some(sky) = &mut loaded.sky else { continue };
            // A map whose sky is entirely behind the camera pays nothing.
            // Per-FACE culling is deliberately not attempted: the faces are
            // one geometry precisely so a sky costs one draw.
            if let Some(frustum) = frustum {
                if !frustum.intersects_obb(sky.part.min, sky.part.max, &inst.transform) {
                    if sky_trace_enabled() && sky.draw_trace & 1 == 0 {
                        log!(
                            "map sky draw: model='{}' frustum-culled bounds \
                             ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2})",
                            inst.model,
                            sky.part.min.x,
                            sky.part.min.y,
                            sky.part.min.z,
                            sky.part.max.x,
                            sky.part.max.y,
                            sky.part.max.z,
                        );
                        sky.draw_trace |= 1;
                    }
                    stats.model_culled += 1;
                    continue;
                }
            }
            draw.transform = inst.transform;
            draw.depth_clip = 1.0;
            draw.eye = vec4(eye.x, eye.y, eye.z, 0.0);
            draw.sky_p = vec4(
                sky.part.projection.code(),
                sky.part.repeat,
                sky.part.scroll(0, time),
                sky.part.scroll(1, time),
            );
            draw.sky_q = vec4(sky.part.v_span, 1.0, 0.0, 0.0);
            draw.draw_vars.geometry_id = Some(sky.geometry.geometry_id());
            draw.draw_vars.set_texture(0, &sky.tex0);
            draw.draw_vars.set_texture(1, &sky.tex1);
            draw.draw_vars.set_uniform(cx.cx, live_id!(lin_ctl), &lin);
            let shader_ready = draw.draw_vars.can_instance();
            if sky_trace_enabled() && sky.draw_trace & 2 == 0 {
                let resident0 = match sky.tex0.get_format(cx.cx) {
                    TextureFormat::VecBGRAu8_32 {
                        width,
                        height,
                        data,
                        updated,
                    } => format!(
                        "VecBGRAu8_32({width}x{height}, texels={}, first={:#010x}, {updated:?})",
                        data.as_ref().map_or(0, Vec::len),
                        data.as_ref().and_then(|pixels| pixels.first()).copied().unwrap_or(0),
                    ),
                    TextureFormat::VecMipBGRAu8_32 {
                        width,
                        height,
                        data,
                        max_level,
                        wrap,
                        updated,
                    } => format!(
                        "VecMipBGRAu8_32({width}x{height}, texels={}, first={:#010x}, \
                         max_level={max_level:?}, wrap={wrap:?}, {updated:?})",
                        data.as_ref().map_or(0, Vec::len),
                        data.as_ref()
                            .and_then(|pixels| pixels.first())
                            .copied()
                            .unwrap_or(0),
                    ),
                    other => format!("{other:?}"),
                };
                let shader_textures = draw
                    .draw_vars
                    .draw_shader_id
                    .map(|shader| {
                        cx.cx.draw_shaders[shader.index]
                            .mapping
                            .textures
                            .iter()
                            .map(|input| format!("{}", input.id))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                log!(
                    "map sky draw: model='{}' texture='{}' shader_ready={} cull={} brightness={:.3} \
                     layer0={:?} layer1={:?} bound0={:?} bound1={:?} inputs={:?} \
                     resident0={} tris={}",
                    inst.model,
                    sky.part.texture.as_deref().unwrap_or("<unnamed>"),
                    shader_ready,
                    draw.draw_vars.options.backface_culling,
                    draw.brightness,
                    sky.tex0.texture_id(),
                    sky.tex1.texture_id(),
                    draw.draw_vars.texture_slots[0]
                        .as_ref()
                        .map(Texture::texture_id),
                    draw.draw_vars.texture_slots[1]
                        .as_ref()
                        .map(Texture::texture_id),
                    shader_textures,
                    resident0,
                    sky.part.triangle_count(),
                );
                sky.draw_trace |= 2;
            }
            if shader_ready {
                let new_area = cx.add_instance(&draw.draw_vars);
                draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
            }
            stats.model_draws += 1;
            stats.model_triangles += sky.part.triangle_count();
        }
        self.placed_models = instances;
        self.sky_draw = Some(draw);
    }

    /// Draw the private FPS presentation list. This path deliberately does
    /// not share world-model setup: no AO/lightmap/top-map/CSM bindings, no
    /// dynamic-light selection, no frustum/caster/receiver work. A typical
    /// list is one 70-triangle pistol and therefore one draw item.
    pub(super) fn draw_view_models_inner(
        &mut self,
        cx: &mut Cx3d,
        draw: &mut DrawSceneViewModel,
        sun: &SunLight,
        stats: &mut RenderStats,
    ) {
        if self.view_models.is_empty() {
            return;
        }
        sun.write_into(
            &mut draw.light_dir,
            &mut draw.sun_color,
            &mut draw.sun_sky,
            &mut draw.sun_ground,
        );
        let stage_inv = self.stage.matrix().invert();
        for inst in &self.view_models {
            let Some((_, loaded)) = self.static_models.iter().find(|(id, _)| *id == inst.model)
            else {
                continue;
            };
            let layer_draws = {
                let mut layers = Vec::with_capacity(1 + loaded.extra_draws.len());
                layers.push((loaded.geometry.geometry_id(), loaded.texture.clone()));
                for (g, t, _, _, _) in &loaded.extra_draws {
                    layers.push((g.geometry_id(), t.clone()));
                }
                layers
            };
            // The scene draw-list carries Stage for every world draw. Cancel
            // it here: a held model is attached to the physical camera, not
            // scaled into an MR diorama or moved onto its floor anchor.
            draw.transform = Mat4f::mul(&stage_inv, &inst.transform);
            for (geometry_id, texture) in &layer_draws {
                draw.draw_vars.geometry_id = Some(*geometry_id);
                draw.draw_vars.set_texture(0, texture);
                if draw.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draw.draw_vars);
                    draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
                }
            }
            stats.view_model_instances += 1;
            stats.view_model_triangles += loaded.triangles;
        }
    }
}
