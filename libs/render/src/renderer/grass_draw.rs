//! The grass lane (crate::grass): one instance per visible patch.

use super::*;

impl Renderer {
    /// The level's grass field; `None` removes it. The same field again (the
    /// same `Arc`) is a no-op, so a host may hand it over every frame.
    pub fn set_grass(&mut self, cx: &mut Cx, field: Option<std::sync::Arc<crate::grass::GrassField>>) {
        match field {
            None => self.grass = None,
            Some(field) => {
                if self.grass.as_ref().is_some_and(|g| std::sync::Arc::ptr_eq(&g.field, &field)) {
                    return;
                }
                let gpu = crate::grass::GrassGpu::new(cx, field);
                log!("grass: {}x{} field at {} m, {} of {} patches grow grass", gpu.field.nx, gpu.field.nz, gpu.field.cell,
                    gpu.patches.iter().filter(|p| p.0 >= 8).count(), gpu.patches.len());
                self.grass = Some(gpu);
            }
        }
    }

    pub fn has_grass(&self) -> bool {
        self.grass.is_some()
    }

    /// After the opaque models: the blades are opaque too, and drawing them
    /// after the terrain and props lets the depth test reject the ones
    /// behind a barrier before they shade.
    pub(super) fn draw_grass(
        &mut self,
        cx: &mut Cx3d,
        eye: Vec3f,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        // MAKEPAD_GRASS=0: the A/B switch (the terrain's grass texture only).
        thread_local! { static OFF: bool = std::env::var("MAKEPAD_GRASS").is_ok_and(|v| v == "0"); }
        if OFF.with(|o| *o) { return; }
        let Some(grass) = self.grass.as_ref() else { return };
        let mut rings = std::mem::take(&mut self.grass_rings);
        grass.visible(eye, frustum, &mut rings);
        if rings.iter().all(Vec::is_empty) {
            self.grass_rings = rings;
            return;
        }
        if self.grass_draw.is_none() {
            self.grass_draw = cx.cx.try_with_vm(|vm| Box::new(crate::shaders::DrawSceneGrass::script_new_with_default(vm)));
        }
        let Some(mut draw) = self.grass_draw.take() else {
            self.grass_rings = rings;
            return;
        };
        // Metal compiles pipelines asynchronously: no grass until it exists
        // (the terrain's own grass texture is there meanwhile).
        let ready = draw.pbr.skinned.draw_vars.draw_shader_id.is_some_and(|id| cx.cx.draw_shader_ready(id, self.hdr_output));
        if ready {
            self.orm_neutral(cx.cx);
            self.detail_neutral(cx.cx);
            self.bind_model_lane(cx, &mut ModelDraw::Grass(&mut draw), eye, fog, sun);
            let grass = self.grass.as_ref().unwrap();
            let f = &grass.field;
            let b = &f.blades;
            let (w, d) = (f.nx as f32 * f.cell, f.nz as f32 * f.cell);
            draw.pbr.skinned.morph_weights0 = vec4(f.origin[0], f.origin[1], 1.0 / w, 1.0 / d);
            draw.pbr.skinned.morph_weights1 = vec4(f.nx as f32, f.nz as f32, f.cell, grass.height_lo);
            draw.pbr.skinned.morph_weights2 = vec4(b.height, b.width, b.radius, b.wind);
            draw.pbr.skinned.morph_weights3 = vec4(b.lush[0], b.lush[1], b.lush[2], 0.0);
            draw.pbr.skinned.morph_weights4 = vec4(b.dry[0], b.dry[1], b.dry[2], grass.height_range);
            draw.pbr.skinned.morph_weights5 = vec4(eye.x, eye.y, eye.z, self.sky_time);
            let vars = &mut draw.pbr.skinned.draw_vars;
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
            draw.pbr.skinned.transform = Mat4f::identity();
            draw.pbr.skinned.lm_rect = Vec4f::default();
            draw.pbr.skinned.morph_ctl = Vec4f::default();
            for (ring, patches) in rings.iter().enumerate() {
                if patches.is_empty() { continue; }
                let geometry = &grass.rings[ring];
                draw.pbr.skinned.draw_vars.geometry_id = Some(geometry.geometry_id());
                let blades = cx.cx.geometries[geometry.geometry_id()].indices.len() / (15 - ring * 6).max(3);
                for &(x, z) in patches {
                    draw.pbr.skinned.morph_weights6 = vec4(x, z, 0.0, crate::grass::PATCH);
                    if draw.pbr.skinned.draw_vars.can_instance() {
                        let area = cx.add_instance(&draw.pbr.skinned.draw_vars);
                        draw.pbr.skinned.draw_vars.area = cx.update_area_refs(draw.pbr.skinned.draw_vars.area, area);
                    }
                }
                stats.grass_patches += patches.len();
                stats.grass_blades += patches.len() * blades;
            }
        }
        self.grass_draw = Some(draw);
        self.grass_rings = rings;
    }
}
