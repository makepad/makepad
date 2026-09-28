//! The scene pass's late layers: water sheets, dynamic shadow meshes and SDF quads,
//! the MR shadow catcher, fireworks, lamp flares, bullet decals and screens/sprites.

use super::*;

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_water(
        &mut self,
        cx: &mut Cx3d,
        draws: &mut SceneDraws,
        world: &World,
        sun: &SunLight,
        (fog_color, fog_density): (Vec3f, f32),
        frustum: Option<&Frustum>,
        shows_environment: bool,
        camera_pos: Vec3f,
    ) {
        // 3w. Water sheets (mix.md W1): one displaced grid per `game.water`
        // volume, drawn after every opaque pass (blending sees depth: a hull
        // below the surface tints, a hull above does not) and before the
        // alpha batches, so sensor ghosts and particles composite over the
        // water. The VERTEX shader displaces by the same wave sum the sim
        // steps — same coefficients, same expression (pin test below) —
        // visual only: physics never reads the GPU.
        self.ensure_water_tiles(cx.cx, world.water.as_deref());
        if !self.water_tiles.is_empty() && shows_environment {
            if let Some(water_draw) = draws.water.as_deref_mut() {
                water_draw.transform = Mat4f::identity();
                water_draw.depth_clip = 1.0;
                water_draw.fog_color = fog_color;
                water_draw.fog_density = fog_density;
                let lin = self.lin_ctl();
                water_draw.draw_vars.set_uniform(cx.cx, live_id!(lin_ctl), &lin);
                water_draw.draw_vars.set_uniform(cx.cx, live_id!(water_eye), &[camera_pos.x, camera_pos.y, camera_pos.z, 0.0]);
                sun.write_into(
                    &mut water_draw.light_dir,
                    &mut water_draw.sun_color,
                    &mut water_draw.sun_sky,
                    &mut water_draw.sun_ground,
                );
                // The sim's own f32 tick-time — the ONE time base both sides
                // of the wave expression consume.
                let t = world.water_time;
                const WAVE_A: [LiveId; 8] = [
                    live_id!(wave_a0), live_id!(wave_a1), live_id!(wave_a2), live_id!(wave_a3),
                    live_id!(wave_a4), live_id!(wave_a5), live_id!(wave_a6), live_id!(wave_a7),
                ];
                const WAVE_B: [LiveId; 8] = [
                    live_id!(wave_b0), live_id!(wave_b1), live_id!(wave_b2), live_id!(wave_b3),
                    live_id!(wave_b4), live_id!(wave_b5), live_id!(wave_b6), live_id!(wave_b7),
                ];
                for tile in &self.water_tiles {
                    if let Some(frustum) = frustum {
                        if !frustum.intersects_aabb(tile.min, tile.max) {
                            continue;
                        }
                    }
                    // Per-volume uniforms: a differing coefficient set starts
                    // its own draw item (the appendable check compares
                    // dyn_uniforms), so volumes never share stale waves.
                    for i in 0..MAX_WAVES {
                        water_draw
                            .draw_vars
                            .set_uniform(cx.cx, WAVE_A[i], &tile.waves_a[i]);
                        water_draw
                            .draw_vars
                            .set_uniform(cx.cx, WAVE_B[i], &tile.waves_b[i]);
                    }
                    water_draw
                        .draw_vars
                        .set_uniform(cx.cx, live_id!(water_params), &[0.0, t, 0.0, 0.0]);
                    water_draw.draw_vars.geometry_id = Some(tile.geometry.geometry_id());
                    if water_draw.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&water_draw.draw_vars);
                        water_draw.draw_vars.area =
                            cx.update_area_refs(water_draw.draw_vars.area, new_area);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_late_layers(
        &mut self,
        cx: &mut Cx3d,
        draws: &mut SceneDraws,
        world: &World,
        frustum: Option<&Frustum>,
        camera_pos: Vec3f,
        shows_environment: bool,
        stats: &mut RenderStats,
        smoke_light: (SunLight, (Vec3f, f32), (Vec3f, Vec3f)),
    ) {
        // 4.5 The dynamic shadow-mesh layer (entity hull drapes + the blob
        // fallbacks accumulated above), rebuilt and uploaded per frame —
        // small by construction: statics live in the baked lightmap and
        // characters + cars ride the SDF quads below. Drawn after the alpha
        // batches so it lies over the ground it darkens; depth test on /
        // depth write off means overlapping shadows can never fight for the
        // buffer.
        if let Some(shadow) = draws.shadow.as_deref_mut() {
            let t0 = Cx::monotonic_now();
            self.last_dynamic_shadow_tris = self.shadow_mesh.triangle_count();
            if !self.shadow_mesh.is_empty() {
                let geometry = self.shadow_geometry.get_or_insert_with(|| Geometry::new(cx.cx));
                geometry.update(
                    cx.cx,
                    std::mem::take(&mut self.shadow_mesh.indices),
                    std::mem::take(&mut self.shadow_mesh.vertices),
                );
                shadow.draw_vars.geometry_id = Some(geometry.geometry_id());
                if shadow.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&shadow.draw_vars);
                    shadow.draw_vars.area = cx.update_area_refs(shadow.draw_vars.area, new_area);
                }
            }
            stats.dyn_shadow_us += perf_us(t0);
        }

        // 4.6 SDF silhouette shadows — the dynamic casters (characters +
        // driven cars): one shared quad, one five-vec4 record per caster,
        // the silhouette morphed per PIXEL from the caster's SDF atlas.
        // Sorted by atlas so each rig's crowd (and each car model's fleet)
        // shares a draw item — the atlas bind is what splits items.
        if let Some(sd) = draws.shadow_sdf.as_deref_mut() {
            if !self.sdf_instances.is_empty() {
                let t0 = Cx::monotonic_now();
                let geometry_id = self.ensure_flare_geometry(cx.cx);
                sd.draw_vars.geometry_id = Some(geometry_id);
                sd.depth_clip = 1.0;
                self.sdf_instances.sort_by(|a, b| a.atlas.cmp(&b.atlas));
                let mut bound: Option<&SdfAtlasKey> = None;
                for inst in &self.sdf_instances {
                    if bound != Some(&inst.atlas) {
                        let tex = match &inst.atlas {
                            SdfAtlasKey::Rig(rig) => self
                                .sdf_atlas_tex
                                .iter()
                                .find(|(r, _)| r == rig)
                                .and_then(|(_, p)| p.as_ref()),
                            SdfAtlasKey::Model(key) => {
                                self.model_sdf_tex.get(key).and_then(|p| p.as_ref())
                            }
                        };
                        let Some((tex, _)) = tex else { continue };
                        sd.draw_vars.set_texture(0, tex);
                        bound = Some(&inst.atlas);
                    }
                    sd.sdf_a = inst.a;
                    sd.sdf_b = inst.b;
                    sd.sdf_c = inst.c;
                    sd.sdf_d = inst.d;
                    sd.sdf_e = inst.e;
                    if sd.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&sd.draw_vars);
                        sd.draw_vars.area =
                            cx.update_area_refs(sd.draw_vars.area, new_area);
                    }
                }
                stats.dyn_shadow_us += perf_us(t0);
            }
        }
        self.sdf_instances.clear();

        // 5. MR shadow catcher: a dark translucent slab just under the
        // diorama's footprint. Without it the world reads as floating
        // stickers over the passthrough feed; with it, planted on the floor.
        // Drawn last so it blends over everything it sits beneath.
        if !shows_environment {
            if let Some(footprint) = Self::world_footprint(world) {
                let (center, radius) = footprint;
                let mut transform = Mat4f::identity();
                transform.v[12] = center.x;
                transform.v[13] = center.y - 0.02 / self.stage.scale.max(1.0e-4);
                transform.v[14] = center.z;
                draws.alpha.cube.cube.transform = transform;
                draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                // Flat slab: thin in y, footprint-sized in x/z.
                let thickness = 0.04 / self.stage.scale.max(1.0e-4);
                draws.alpha.cube.cube.cube_size =
                    vec3(radius * 2.0, thickness, radius * 2.0);
                draws.alpha.cube.cube.color = vec4(0.0, 0.0, 0.0, 0.25);
                draws.alpha.cube.cube.depth_clip = 1.0;
                draws.alpha.cube.glow = 0.0;
                draws.alpha.cube.color_adjust_ctl = vec4(0.0, 1.0, 1.0, 0.0);
                draws.alpha.cube.cube.draw(cx);
                stats.dyn_instances += 1;
                stats.shadow_catcher_drawn = true;
            }
        }

        // 5.5 Volumetric smoke clouds: raymarched spheres, depth-tested
        // against the world, never depth-written.
        if !self.smoke_volumes.is_empty() {
            if self.smoke_draw.is_none() {
                self.smoke_draw = cx.cx.try_with_vm(|vm| Box::new(crate::smoke::DrawSceneSmoke::script_new_with_default(vm)));
            }
            if let Some(mut smoke) = self.smoke_draw.take() {
                let geometry_id = self.ensure_flare_geometry(cx.cx);
                smoke.draw_vars.geometry_id = Some(geometry_id);
                smoke.depth_clip = 1.0;
                let sun = &smoke_light.0;
                let dv = &mut smoke.draw_vars;
                dv.set_uniform(cx.cx, live_id!(eye), &[camera_pos.x, camera_pos.y, camera_pos.z, 0.0]);
                dv.set_uniform(cx.cx, live_id!(light_dir), &[sun.dir.x, sun.dir.y, sun.dir.z]);
                dv.set_uniform(cx.cx, live_id!(sun_color), &[sun.color.x, sun.color.y, sun.color.z]);
                dv.set_uniform(cx.cx, live_id!(sun_sky), &[sun.sky.x, sun.sky.y, sun.sky.z]);
                let (fog_color, fog_density) = smoke_light.1;
                dv.set_uniform(cx.cx, live_id!(fog_color), &[fog_color.x, fog_color.y, fog_color.z]);
                dv.set_uniform(cx.cx, live_id!(fog_density), &[fog_density]);
                let (right, up) = smoke_light.2;
                dv.set_uniform(cx.cx, live_id!(cam_right), &[right.x, right.y, right.z, 0.0]);
                dv.set_uniform(cx.cx, live_id!(cam_up), &[up.x, up.y, up.z, 0.0]);
                for v in std::mem::take(&mut self.smoke_volumes) {
                    if let Some(frustum) = frustum {
                        if !frustum.intersects_sphere(v.center, v.radius) {
                            continue;
                        }
                    }
                    smoke.smoke_a = vec4(v.center.x, v.center.y, v.center.z, v.radius.max(0.05));
                    smoke.smoke_b = vec4(v.density.clamp(0.0, 4.0), v.age, v.seed, 0.0);
                    if smoke.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&smoke.draw_vars);
                        smoke.draw_vars.area = cx.update_area_refs(smoke.draw_vars.area, new_area);
                    }
                }
                self.smoke_draw = Some(smoke);
            }
        }

        // 6. Fireworks, last of all: additive and depth-write-off, so they
        // must come after every opaque surface has laid down the depth they
        // test against. ONE instance per shell — the GPU expands each into
        // SPARKS_PER_SHELL sparks from a closed form (firework.rs).
        if let Some(fw) = draws.firework.as_deref_mut() {
            if !self.firework_instances.is_empty() && shows_environment {
                let geometry_id = self.ensure_spark_geometry(cx.cx);
                fw.draw_vars.geometry_id = Some(geometry_id);
                fw.depth_clip = 1.0;
                for f in &self.firework_instances {
                    fw.origin_age = vec4(f.origin.x, f.origin.y, f.origin.z, f.age);
                    fw.launch_life = vec4(f.launch.x, f.launch.y, f.launch.z, f.life);
                    // Roomy quad; the sprite core occupies only its middle fifth
                    // (see spark_pixel), so this is streak headroom, not dot size.
                    fw.params = vec4(f.speed, f.seed, 1.1, 0.0);
                    fw.color = f.color;
                    fw.color_tail = f.color_tail;
                    if fw.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&fw.draw_vars);
                        fw.draw_vars.area = cx.update_area_refs(fw.draw_vars.area, new_area);
                    }
                }
                stats.firework_shells = self.firework_instances.len() as u64;
            }
        }

        // 6.5 Old-school lamp flares: one additive camera-facing billboard
        // pin-glow per visible street lamp, positions straight off the
        // harvested lamp list. Depth-tested (a wall between you and the lamp
        // eats the glow — that IS the old-school behaviour) but never
        // depth-written, drawn with the other late transparents.
        if let Some(fl) = draws.flare.as_deref_mut() {
            if shows_environment && !self.lamp_cache.is_empty() {
                let geometry_id = self.ensure_flare_geometry(cx.cx);
                fl.draw_vars.geometry_id = Some(geometry_id);
                fl.depth_clip = 1.0;
                for l in &self.lamp_cache {
                    if let Some(frustum) = frustum {
                        if !frustum.intersects_sphere(l.pos, 1.5) {
                            continue;
                        }
                    }
                    // Nudge toward the camera so the fixture's own head — the
                    // bulb sits INSIDE it — cannot eclipse its flare.
                    let to_cam = camera_pos - l.pos;
                    let d = to_cam.length().max(1.0e-4);
                    let pos = l.pos + to_cam * (0.35 / d).min(0.5);
                    // ~0.8-1.4 units across, scaled by the lamp's intensity
                    // (lamp colours cap at 2.0 for the atlas encode).
                    let intensity = light_intensity(l) * 0.5;
                    let size = (1.4 * intensity).clamp(0.8, 1.4);
                    fl.flare_pos = vec4(pos.x, pos.y, pos.z, size);
                    fl.flare_col = vec4(
                        l.color.x * 0.5,
                        l.color.y * 0.5,
                        l.color.z * 0.5,
                        1.0,
                    );
                    if fl.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&fl.draw_vars);
                        fl.draw_vars.area = cx.update_area_refs(fl.draw_vars.area, new_area);
                    }
                    stats.flares += 1;
                }
            }
        }

        // 6.55 Engine-default bullet holes: one bounded, instanced procedural
        // quad per live mark. The producer has resolved every mark to its
        // current world-space pose, including the surface offset.
        // Nothing in this path keys the static slabs or allocates per frame.
        if !world.decals.is_empty() {
            if self.decal_draw.is_none() {
                self.decal_draw = cx
                    .cx
                    .try_with_vm(|vm| Box::new(DrawSceneDecal::script_new_with_default(vm)));
            }
            if let Some(mut decal) = self.decal_draw.take() {
                let geometry_id = self.ensure_flare_geometry(cx.cx);
                decal.draw_vars.geometry_id = Some(geometry_id);
                decal.depth_clip = 1.0;
                for mark in &world.decals {
                    let (pos, normal) = (mark.pos, mark.normal);
                    if let Some(frustum) = frustum {
                        if !frustum.intersects_sphere(pos, mark.size) {
                            continue;
                        }
                    }
                    decal.decal_pos = vec4(pos.x, pos.y, pos.z, mark.size);
                    let roll = (mark.serial as f32 * 2.399_963_1).rem_euclid(std::f32::consts::TAU);
                    decal.decal_normal = vec4(normal.x, normal.y, normal.z, roll);
                    decal.decal_color = vec4(
                        mark.color.x,
                        mark.color.y,
                        mark.color.z,
                        mark.kind.shader_id(),
                    );
                    if decal.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&decal.draw_vars);
                        decal.draw_vars.area = cx.update_area_refs(decal.draw_vars.area, new_area);
                        stats.bullet_decals += 1;
                    }
                }
                self.decal_draw = Some(decal);
            }
        }

        // 6.6 In-world video screen: one textured quad on the shared flare
        // geometry. The host owns placement and the per-frame texture; this
        // just issues the instance. Opaque + depth-written, so ordering
        // against the transparents above doesn't matter.
        if let Some(sc) = draws.screen.as_deref_mut() {
            let geometry_id = self.ensure_flare_geometry(cx.cx);
            sc.draw_vars.geometry_id = Some(geometry_id);
            let lin = self.lin_ctl();
            sc.draw_vars.set_uniform(cx.cx, live_id!(lin_ctl), &lin);
            // The sprite lane below BORROWS this draw — one shader serves both
            // the video screen and every billboard — so it overwrites the pose
            // and texture the host owns. Remember them here and hand them back
            // when the lane is done: otherwise the last sprite of the frame
            // leaves its pose behind, next frame the "is there a screen?" test
            // (a zero `screen_size` draws nothing) reads THAT and passes, and
            // the sprite's whole sheet is drawn as one opaque quad with full
            // 0..1 UVs — an atlas standing in the world under the unit that
            // happened to be drawn last, and still standing there after the
            // level that owned it is gone.
            let host_pos = sc.screen_pos;
            let host_size = sc.screen_size;
            let host_texture = sc.draw_vars.texture_slots[0].clone();
            sc.depth_clip = 1.0;
            sc.cutout = 0.0;
            sc.pixelated = 0.0;
            sc.uv_rect = vec4(0.0, 0.0, 1.0, 1.0);
            sc.tint = vec4(1.0, 1.0, 1.0, 1.0);
            sc.color_adjust_ctl = vec4(0.0, 1.0, 1.0, 0.0);
            if sc.screen_size.x.abs() > 0.0
                && sc.screen_size.y > 0.0
                && sc.draw_vars.can_instance()
            {
                let new_area = cx.add_instance(&sc.draw_vars);
                sc.draw_vars.area = cx.update_area_refs(sc.draw_vars.area, new_area);
            }
            // Sprite billboards are cut-out artwork, always: the quad is a
            // bounding box around a figure and everything outside it is
            // transparent. Without the alpha test every actor is a black
            // rectangle. The video screen above keeps cutout 0.
            sc.cutout = 1.0;
            // Every ScreenInstance in this lane is sprite-sheet artwork:
            // monsters, props/map items, and one-shot impact/burst clips all
            // take the same exact level-0 nearest path. The shared video
            // screen above deliberately keeps ordinary smooth sampling.
            sc.pixelated = 1.0;
            for inst in draws.screen_instances {
                if inst.size.x <= 0.0 || inst.size.y <= 0.0 {
                    continue;
                }
                sc.draw_vars.set_texture(0, &inst.texture);
                sc.screen_pos = inst.pos;
                sc.screen_size = inst.size;
                sc.uv_rect = inst.uv;
                sc.tint = inst.tint;
                sc.color_adjust_ctl = inst.color_adjust;
                if sc.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&sc.draw_vars);
                    sc.draw_vars.area = cx.update_area_refs(sc.draw_vars.area, new_area);
                }
            }
            // The borrow ends here: the host's screen is exactly as it left it.
            sc.screen_pos = host_pos;
            sc.screen_size = host_size;
            sc.draw_vars.texture_slots[0] = host_texture;
        }
    }
}
