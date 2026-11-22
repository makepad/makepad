//! The grass lane (crate::grass): one instance per visible patch.

use super::*;

/// The paint lane's binding that carries the grass field's cover raster
/// (shaders/splat.rs `grass_cover_at`): the PBR lane's ORM slot, which the
/// paint lane never reads. The model shaders have no free texture slot.
const GRASS_COVER_SLOT: LiveId = live_id!(orm_map);
/// The grass lane's binding that carries the trampled-grass field
/// (crate::trample): the PBR lane's emissive slot, which the blades never
/// read (it held the neutral 1x1).
const TRAMPLE_SLOT: LiveId = live_id!(emissive_map);
/// A stamp presses grass where the field's cover is at least this (the
/// blades' own cut, shaders/grass.rs `cover.x < 0.03`).
const TRAMPLE_COVER_MIN: u8 = 8;

impl Renderer {
    /// The level's grass field; `None` removes it. The same field again (the
    /// same `Arc`) is a no-op, so a host may hand it over every frame. It
    /// stays across [`Self::enter_realm`]: a host whose new realm grows other
    /// grass, or none, replaces or removes it.
    /// Preparing it takes 10 ms for a 1000 x 1000 field: a host that can
    /// prepare it on a worker hands it over with [`Self::set_grass_prepared`].
    pub fn set_grass(&mut self, cx: &mut Cx, field: Option<std::sync::Arc<crate::grass::GrassField>>) {
        match field {
            None => self.grass = None,
            Some(field) => {
                if self.grass.as_ref().is_some_and(|g| std::sync::Arc::ptr_eq(&g.field, &field)) {
                    return;
                }
                self.set_grass_prepared(cx, crate::grass::GrassPrep::new(field));
            }
        }
    }

    /// The level's grass from a field prepared off this thread
    /// ([`crate::grass::GrassPrep::new`]): only its textures and geometries
    /// are made here. The field it draws already is a no-op.
    pub fn set_grass_prepared(&mut self, cx: &mut Cx, prep: crate::grass::GrassPrep) {
        if self.grass.as_ref().is_some_and(|g| std::sync::Arc::ptr_eq(&g.field, prep.field())) {
            return;
        }
        let gpu = crate::grass::GrassGpu::new(cx, prep);
        log!("grass: {}x{} field at {} m, {} of {} patches grow grass", gpu.field.nx, gpu.field.nz, gpu.field.cell,
            gpu.cull.patches.iter().filter(|p| p.0 >= 8).count(), gpu.cull.patches.len());
        self.grass = Some(gpu);
    }

    /// The field the grass draws now, if any.
    pub fn grass_field(&self) -> Option<&std::sync::Arc<crate::grass::GrassField>> {
        self.grass.as_ref().map(|g| &g.field)
    }

    pub fn has_grass(&self) -> bool {
        self.grass.is_some()
    }

    /// Where the ground is pressed this frame (crate::trample): the host's
    /// contacts (a vehicle's grounded wheels, a character's feet), the
    /// field's centre (where the view looks: the followed body) and the
    /// game seconds since the last call (0 while paused). The field runs at
    /// the next scene draw. A stamp where no grass grows (the field's cover
    /// at its middle and its four ends) presses nothing and is left out, so
    /// a field with no stamp on grass for crate::trample::RECOVERY seconds
    /// costs nothing.
    pub fn set_ground_stamps(&mut self, focus: Vec3f, dt: f32, stamps: &[crate::trample::GroundStamp]) {
        let grass = self.grass.as_ref().map(|g| &*g.field);
        self.trample.set_stamps(focus, dt, stamps, |s| {
            let Some(field) = grass else { return false };
            let (along, across) = (vec2(s.yaw.sin(), s.yaw.cos()), vec2(s.yaw.cos(), -s.yaw.sin()));
            let (w, l) = (across * s.half_extents.x, along * s.half_extents.y);
            [s.pos, s.pos + w, s.pos - w, s.pos + l, s.pos - l]
                .iter()
                .any(|p| field.cover_at(p.x, p.y).is_some_and(|c| c[0] >= TRAMPLE_COVER_MIN))
        });
    }

    /// The trampled-grass field's pass for this frame, before the scene
    /// draws it (a child of the current pass): only for the scene's own draw
    /// and only where grass grows.
    pub(super) fn record_trample(&mut self, cx: &mut Cx3d) {
        if grass_off() || self.grass.is_none() {
            return;
        }
        self.trample.record(cx.cx);
    }

    /// The grass's far carpet on the terrain's paint lane (shaders/splat.rs
    /// `grass_carpet`), for this frame: the field, the blades' look, the
    /// screen's reach and the trampled field's read as the lane's uniforms. No field, or MAKEPAD_GRASS=0:
    /// the carpet is off (`grass_lod.w` 0).
    pub(super) fn set_grass_carpet(&self, cx: &mut Cx, draw: &mut crate::shaders::DrawSceneSplat, focal_px: f32) {
        let vars = &mut draw.pbr.skinned.draw_vars;
        let Some(grass) = self.grass.as_ref().filter(|_| !grass_off()) else {
            vars.set_uniform(cx, live_id!(grass_lod), &[0.0; 4]);
            return;
        };
        let (f, b) = (&grass.field, &grass.field.blades);
        let lod = crate::grass::GrassLod::new(b, focal_px, self.grass_pixels);
        let flowers = crate::grass::GrassLod::flowers(f, focal_px, self.grass_pixels);
        vars.set_uniform(cx, live_id!(grass_map), &[f.origin[0], f.origin[1], 1.0 / (f.nx as f32 * f.cell), 1.0 / (f.nz as f32 * f.cell)]);
        vars.set_uniform(cx, live_id!(grass_lush), &[b.lush[0], b.lush[1], b.lush[2], b.wind]);
        vars.set_uniform(cx, live_id!(grass_dry), &[b.dry[0], b.dry[1], b.dry[2], 0.0]);
        vars.set_uniform(cx, live_id!(grass_lod), &[lod.full, lod.reach, flowers.map_or(0.0, |l| l.reach), 1.0]);
        vars.set_uniform(cx, live_id!(grass_bloom), &bloom_mean(&f.flowers));
        // the field the blades read (bound by `bind_grass_cover`), or off
        vars.set_uniform(cx, live_id!(grass_trample), &self.trample.grass_read().map_or([0.0; 4], |(_, u)| u));
        vars.set_uniform(cx, live_id!(wind_state), &self.wind_clock.state());
        vars.set_uniform(cx, live_id!(wind_scroll), &self.wind_clock.scroll());
    }

    /// The cover raster on GRASS_COVER_SLOT and, while any texel is
    /// pressed, the trampled field on TRAMPLE_SLOT (the carpet's read,
    /// `grass_trample` in `set_grass_carpet`), after each `set_material`,
    /// which binds the material's own ORM and emissive maps there.
    pub(super) fn bind_grass_cover(&self, cx: &Cx, draw: &mut ModelDraw) {
        let (ModelDraw::Splat(d), Some(grass)) = (draw, self.grass.as_ref()) else { return };
        let vars = &mut d.pbr.skinned.draw_vars;
        let Some(id) = vars.draw_shader_id else { return };
        let slots = &cx.draw_shaders[id.index].mapping.textures;
        if let Some(slot) = slots.iter().position(|t| t.id == GRASS_COVER_SLOT).filter(|slot| *slot < vars.texture_slots.len()) {
            vars.set_texture(slot, &grass.cover_tex);
        }
        if let Some((field, _)) = self.trample.grass_read() {
            if let Some(slot) = slots.iter().position(|t| t.id == TRAMPLE_SLOT).filter(|slot| *slot < vars.texture_slots.len()) {
                vars.set_texture(slot, field);
            }
        }
    }

    /// After the opaque models: the blades are opaque too, and drawing them
    /// after the terrain and props lets the depth test reject the ones
    /// behind a barrier before they shade.
    pub(super) fn draw_grass(
        &mut self,
        cx: &mut Cx3d,
        eye: Vec3f,
        focal_px: f32,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if grass_off() { return; }
        let Some(grass) = self.grass.as_ref() else { return };
        let lod = crate::grass::GrassLod::new(&grass.field.blades, focal_px, self.grass_pixels);
        let flod = crate::grass::GrassLod::flowers(&grass.field, focal_px, self.grass_pixels).filter(|_| grass.flower_levels.is_some());
        let mut rings = std::mem::take(&mut self.grass_rings);
        let mut flower_rings = std::mem::take(&mut self.grass_flower_rings);
        grass.cull.visible(&grass.field, eye, frustum, &lod, &mut rings, flod.as_ref().map(|l| (l, &mut flower_rings)));
        if flod.is_none() {
            for level in flower_rings.iter_mut() { level.clear(); }
        }
        if rings.iter().chain(&flower_rings).all(Vec::is_empty) {
            self.grass_rings = rings;
            self.grass_flower_rings = flower_rings;
            return;
        }
        if self.grass_draw.is_none() {
            self.grass_draw = cx.cx.try_with_vm(|vm| Box::new(crate::shaders::DrawSceneGrass::script_new_with_default(vm)));
        }
        let Some(mut draw) = self.grass_draw.take() else {
            self.grass_rings = rings;
            self.grass_flower_rings = flower_rings;
            return;
        };
        // Metal compiles pipelines asynchronously: no grass until it exists
        // (the terrain's own grass texture is there meanwhile).
        let ready = draw.pbr.skinned.draw_vars.draw_shader_id.is_some_and(|id| cx.draw_shader_ready_here(id));
        if ready {
            self.orm_neutral(cx.cx);
            self.detail_neutral(cx.cx);
            self.bind_model_lane(cx, &mut ModelDraw::Grass(&mut draw), eye, fog, sun);
            let grass = self.grass.as_ref().unwrap();
            let f = &grass.field;
            let b = &f.blades;
            let (w, d) = (f.nx as f32 * f.cell, f.nz as f32 * f.cell);
            draw.pbr.skinned.morph_weights0 = vec4(f.origin[0], f.origin[1], 1.0 / w, 1.0 / d);
            draw.pbr.skinned.morph_weights1 = vec4(f.nx as f32, f.nz as f32, f.cell, 0.0);
            draw.pbr.skinned.morph_weights2 = vec4(b.height, b.width, lod.reach, b.wind);
            draw.pbr.skinned.morph_weights3 = vec4(b.lush[0], b.lush[1], b.lush[2], 0.0);
            draw.pbr.skinned.morph_weights4 = vec4(b.dry[0], b.dry[1], b.dry[2], 0.0);
            draw.pbr.skinned.morph_weights5 = vec4(eye.x, eye.y, eye.z, 0.0);
            let vars = &mut draw.pbr.skinned.draw_vars;
            vars.set_uniform(cx.cx, live_id!(wind_state), &self.wind_clock.state());
            vars.set_uniform(cx.cx, live_id!(wind_scroll), &self.wind_clock.scroll());
            vars.set_uniform(cx.cx, live_id!(grass_lod), &[lod.full, lod.reach, crate::grass::WIDTH_CAP, 0.0]);
            set_flower_uniforms(cx.cx, vars, &grass.field.flowers, flod.as_ref());
            vars.set_texture(0, &grass.height_tex);
            vars.set_texture(1, &grass.cover_tex);
            // Every other material slot the PBR lane declares gets a neutral
            // 1x1: the blades sample none of them, but no slot may be empty.
            let neutral = self.orm_fallback.clone();
            let detail = self.detail_fallback.clone();
            let vars = &mut draw.pbr.skinned.draw_vars;
            if let (Some(neutral), Some(detail)) = (neutral, detail) {
                vars.set_texture(5, &detail);
                vars.set_texture(6, &neutral);
                if let Some(id) = vars.draw_shader_id {
                    for name in [live_id!(orm_map), live_id!(normal_map), live_id!(occlusion_map), live_id!(emissive_map)] {
                        if let Some(slot) = cx.cx.draw_shaders[id.index].mapping.textures.iter().position(|t| t.id == name).filter(|slot| *slot < vars.texture_slots.len()) {
                            vars.set_texture(slot, &neutral);
                        }
                    }
                }
            }
            // The trampled field on TRAMPLE_SLOT while any texel is pressed;
            // otherwise the read is off (`grass_trample.z` 0) and the slot
            // keeps the neutral 1x1.
            let read = self.trample.grass_read();
            let vars = &mut draw.pbr.skinned.draw_vars;
            vars.set_uniform(cx.cx, live_id!(grass_trample), &read.map_or([0.0; 4], |(_, u)| u));
            if let (Some((field, _)), Some(id)) = (read, vars.draw_shader_id) {
                if let Some(slot) = cx.cx.draw_shaders[id.index].mapping.textures.iter().position(|t| t.id == TRAMPLE_SLOT).filter(|slot| *slot < vars.texture_slots.len()) {
                    vars.set_texture(slot, field);
                }
            }
            draw.pbr.skinned.transform = Mat4f::identity();
            draw.pbr.skinned.lm_rect = Vec4f::default();
            draw.pbr.skinned.draw_vars.set_uniform(cx.cx, live_id!(morph_ctl), &[0.0, 0.0, 0.0, 0.0]);
            for (level, patches) in rings.iter().enumerate() {
                if patches.is_empty() { continue; }
                let geometry = &grass.levels[level];
                draw.pbr.skinned.draw_vars.geometry_id = Some(geometry.geometry_id());
                let blades = grass.cull.level_blades[level];
                for &(x, z) in patches {
                    draw.pbr.skinned.morph_weights6 = vec4(x, z, 0.0, crate::grass::PATCH);
                    if draw.pbr.skinned.draw_vars.can_instance() {
                        let area = cx.add_instance(&mut draw.pbr.skinned.draw_vars);
                        draw.pbr.skinned.draw_vars.area = cx.update_area_refs(draw.pbr.skinned.draw_vars.area, area);
                    }
                }
                stats.grass_patches += patches.len();
                stats.grass_blades += patches.len() * blades;
            }
            // The flowers: one more instance for each patch that blooms,
            // from the flower geometries (the shader tells them by their
            // vertices' mark).
            if let Some(levels) = grass.flower_levels.as_ref() {
                for (level, patches) in flower_rings.iter().enumerate() {
                    if patches.is_empty() { continue; }
                    draw.pbr.skinned.draw_vars.geometry_id = Some(levels[level].geometry_id());
                    for &(x, z) in patches {
                        draw.pbr.skinned.morph_weights6 = vec4(x, z, 0.0, crate::grass::PATCH);
                        if draw.pbr.skinned.draw_vars.can_instance() {
                            let area = cx.add_instance(&mut draw.pbr.skinned.draw_vars);
                            draw.pbr.skinned.draw_vars.area = cx.update_area_refs(draw.pbr.skinned.draw_vars.area, area);
                        }
                    }
                    stats.grass_patches += patches.len();
                    stats.grass_flowers += patches.len() * grass.cull.level_flowers[level];
                }
            }
        }
        self.grass_draw = Some(draw);
        self.grass_rings = rings;
        self.grass_flower_rings = flower_rings;
    }
}

/// The flower kinds on the grass draw (shaders/grass.rs): their screen
/// density, their cumulative shares of the flowers, stem heights, and per
/// kind the petals (sRGB, head size) and the eye (sRGB, cup). Unused slots
/// repeat the last kind.
fn set_flower_uniforms(cx: &mut Cx, vars: &mut DrawVars, kinds: &[crate::grass::FlowerKind], lod: Option<&crate::grass::GrassLod>) {
    let Some(lod) = lod.filter(|_| !kinds.is_empty()) else {
        vars.set_uniform(cx, live_id!(grass_flower_lod), &[1.0, 1.0, 1.0, 0.0]);
        return;
    };
    vars.set_uniform(cx, live_id!(grass_flower_lod), &[lod.full, lod.reach, crate::grass::HEAD_CAP, kinds.len() as f32]);
    let total: f32 = kinds.iter().map(|k| k.density.max(0.0)).sum::<f32>().max(1.0e-6);
    let kind = |i: usize| &kinds[i.min(kinds.len() - 1)];
    let mut cum = [1.0f32; 4];
    let mut sum = 0.0;
    for (i, c) in cum.iter_mut().enumerate().take(kinds.len()) {
        sum += kinds[i].density.max(0.0);
        *c = if i + 1 == kinds.len() { 1.0 } else { sum / total };
    }
    vars.set_uniform(cx, live_id!(grass_flower_cum), &cum);
    vars.set_uniform(cx, live_id!(grass_flower_h), &[kind(0).height, kind(1).height, kind(2).height, kind(3).height]);
    let petals = [live_id!(flower_petal0), live_id!(flower_petal1), live_id!(flower_petal2), live_id!(flower_petal3)];
    let eyes = [live_id!(flower_eye0), live_id!(flower_eye1), live_id!(flower_eye2), live_id!(flower_eye3)];
    for i in 0..4 {
        let k = kind(i);
        vars.set_uniform(cx, petals[i], &[k.color[0], k.color[1], k.color[2], k.size]);
        vars.set_uniform(cx, eyes[i], &[k.eye[0], k.eye[1], k.eye[2], k.cup]);
    }
}

/// How much of a meadow's view its flowers' heads take, over the ground
/// they cover from above: they tilt and cup and stand over the blades
/// (the meadow guard's flower field, sandbox tests/foliage_meadow.rs).
const HEADS_SEEN: f32 = 1.35;

/// What a far flowered meadow shows of its flowers (the carpet's
/// `grass_bloom`, shaders/splat.rs): the heads' mean sRGB, by the ground
/// each kind's heads cover (a sixth of each head is its eye), and how much
/// of the view their heads take at full bloom (six diamond petals fill
/// about 0.55 of their disc, seen HEADS_SEEN of that).
fn bloom_mean(kinds: &[crate::grass::FlowerKind]) -> [f32; 4] {
    let area = |k: &crate::grass::FlowerKind| k.density.max(0.0) * std::f32::consts::PI * (k.size * 0.5).powi(2) * 0.55 * HEADS_SEEN;
    let cover: f32 = kinds.iter().map(area).sum();
    if cover <= 0.0 { return [0.0; 4]; }
    let mut col = [0.0f32; 3];
    for k in kinds {
        let w = area(k) / cover;
        for c in 0..3 { col[c] += w * (k.color[c] * 5.0 / 6.0 + k.eye[c] / 6.0); }
    }
    [col[0], col[1], col[2], cover]
}

/// MAKEPAD_GRASS=0: the A/B switch (no blades and no carpet: the terrain's
/// grass texture only).
fn grass_off() -> bool {
    thread_local! { static OFF: bool = std::env::var("MAKEPAD_GRASS").is_ok_and(|v| v == "0"); }
    OFF.with(|o| *o)
}
