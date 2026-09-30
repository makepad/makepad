//! The per-view scene pass: `draw_scene` / `draw_scene_full`.

use super::*;

/// HDR lane: the analytic dome's radiance relative to the lit world. The
/// Preetham luminance at the legacy 0.1 exposure sits near a sunlit white
/// wall; a clear sky is a few times dimmer than that, and keeping it there
/// is what keeps its blue through the tone map.
const HDR_SKY_GAIN: f32 = 0.4;
/// HDR lane height fog: the base height the game's fog density applies at,
/// and the scale height it thins over (e^-1 every this many metres up).
const HDR_FOG_BASE: f32 = 0.0;
/// Sky-derived fill: gain on the dome's mean radiance (the dome is drawn
/// at HDR_SKY_GAIN; the fill sees the whole unobstructed hemisphere, the
/// eye mostly the horizon) and the ground's bounce albedo.
const HDR_SKY_FILL_GAIN: f32 = 1.3;
const HDR_GROUND_ALBEDO: f32 = 0.18;
const HDR_FOG_SCALE_HEIGHT: f32 = 40.0;

impl Renderer {
    /// HDR lane: the fill comes from the sky that is actually drawn. Under
    /// the analytic dome (and no authored `ambient`) the up-hemisphere fill
    /// is the dome's own cosine-weighted radiance — blue at noon, amber at
    /// sunset, near-black at night — and the down-hemisphere fill is that
    /// light bounced off a mid-grey ground. The rig's night floor stays as
    /// the lower bound so a moonless town is dim, never pitch black.
    fn hdr_fill_from_sky(&self, world: &World, hdr: &mut SunLight) {
        if world.sun.ambient.is_some() {
            return;
        }
        let Some(frame) = analytic_sky_frame(world, hdr.dir, self.stage.shows_environment(), true, self.sky_clock) else {
            return;
        };
        let sky = crate::sky::sky_fill_linear(&frame) * frame.dome_tint * (HDR_SKY_GAIN * HDR_SKY_FILL_GAIN);
        let bounce = (hdr.color * hdr.dir.y.max(0.0) + sky) * HDR_GROUND_ALBEDO;
        let floor = hdr.sky;
        let lum = |c: Vec3f| c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722;
        let floor_mix = |c: Vec3f, f: Vec3f| if lum(c) < lum(f) * 0.35 { f * 0.35 } else { c };
        hdr.sky = floor_mix(sky, floor);
        hdr.ground = floor_mix(bounce, hdr.ground);
    }

    /// Encode the whole 3D scene for one view. `draw_list` is the host's
    /// scene draw list (begun/ended here, exactly as before the move).
    pub fn draw_scene(
        &mut self,
        cx: &mut Cx3d,
        draw_list: &mut DrawList,
        draws: &mut SceneDraws,
        world: &World,
        scene_state: SceneState3D,
    ) -> RenderStats {
        self.draw_scene_full(cx, draw_list, draws, world, scene_state, None, None)
    }

    /// [`draw_scene`] plus an optional skinned-character batch.
    pub fn draw_scene_full(
        &mut self,
        cx: &mut Cx3d,
        draw_list: &mut DrawList,
        draws: &mut SceneDraws,
        world: &World,
        scene_state: SceneState3D,
        skinned: Option<SkinnedBatch>,
        mut models_draw: Option<&mut DrawSceneSkinned>,
    ) -> RenderStats {
        let mut stats = RenderStats::default();
        let camera_pos = scene_state.camera_pos;
        let stage_matrix = self.stage.matrix();
        let cluster_view = (self.stage.mode == StageMode::Flat).then(|| (
            Mat4f::mul(&scene_state.view, &stage_matrix), scene_state.projection,
        ));
        // CPU frustum for per-frame instance culling AND the Realtime
        // bake's visible-region scheduling, in the same world units the
        // instance transforms use (the stage rides inside the clip matrix).
        // Flat stage only: in XR the runtime overwrites the pass camera
        // with its own eye matrices every frame (see stage.rs), so
        // scene_state is not what renders there and culling from it could
        // drop visible geometry. Only per-frame packing points consult
        // this — cached static slabs are never culled, because they outlive
        // the camera that built them.
        let frustum_val = (self.stage.mode == StageMode::Flat).then(|| {
            Frustum::from_clip_matrix(&Mat4f::mul(
                &scene_state.projection,
                &Mat4f::mul(&scene_state.view, &stage_matrix),
            ))
        });
        let frustum = frustum_val.as_ref();
        // Which tier serves the DYNAMIC casters this frame — the ONE
        // decision point (gpu_lightmap::dynamic_shadow_tiers) both the
        // draw gates below and the mover collection consume, so the
        // settings/F8 mode switch flips the complete contract atomically.
        let tiers = crate::gpu_lightmap::dynamic_shadow_tiers(self.gpu_baker.mode());
        // A sun hour that moves makes this a clock-driven sky (`sky_clock`).
        let hour = world.sun.time_of_day.filter(|_| world.sun.dir.is_none());
        if hour.is_none() {
            self.sky_clock = false;
        } else if self.sky_hour.is_some() && self.sky_hour != hour {
            self.sky_clock = true;
        }
        self.sky_hour = hour;
        let sun = crate::sun::resolve_sun(&world.sun);
        self.light_eye = camera_pos;
        self.stream_lights(camera_pos, sun.dir.y);
        self.build_frame_lights(&sun);
        crate::entity_lights::append_entity_lights_with_model_headlights(
            world, &mut self.frame_lights, &self.model_headlight_owners,
        );
        // HDR output: every light below (sun, fill, lamps, fog) switches to
        // linear scene-referred values here, once, so shaders, the cluster
        // list and the GI relight all see the same convention.
        let sun = if self.hdr_output {
            self.scale_frame_lights_hdr();
            let mut hdr = sun.to_hdr();
            self.hdr_fill_from_sky(world, &mut hdr);
            // Exposure is metered on the UNAUTHORED rig for this sun
            // position: a script that dims its sun or ambient (a moonlit
            // arena) means dark, and metering its own dim light would lift
            // it back to mid-grey. `game.sky` exposure_ev biases it.
            self.hdr_exposure = if world.sun.color.is_some() || world.sun.ambient.is_some() {
                let mut stock = world.sun.clone();
                stock.color = None;
                stock.ambient = None;
                let mut metered = crate::sun::resolve_sun(&stock).to_hdr();
                self.hdr_fill_from_sky(world, &mut metered);
                metered.hdr_exposure()
            } else {
                hdr.hdr_exposure()
            };
            if let Some(ev) = world.sky.as_ref().map(|s| s.exposure_ev).filter(|ev| ev.is_finite() && *ev != 0.0) {
                self.hdr_exposure *= 2.0f32.powf(ev.clamp(-8.0, 8.0));
            }
            if self.clustered_frames % 240 == 0 && std::env::var_os("MAKEPAD_HDR_STATS").is_some() {
                log!("hdr: exposure {:.3}, sun {:?} sky {:?} ground {:?} dir.y {:.3}", self.hdr_exposure, hdr.color, hdr.sky, hdr.ground, hdr.dir.y);
            }
            hdr
        } else {
            sun
        };
        self.clustered.lin_ctl = self.lin_ctl();
        let local_shadows = self.clustered_enabled && self.frame_lights.iter().any(|l| l.shadows);
        // Character palettes pack BEFORE the cascades encode: the skinned
        // depth passes bind the same texture the visible draw does.
        match &skinned {
            Some(batch) => self.pack_skin_palettes(cx.cx, &batch.items, &mut stats),
            None => self.skin_joint_bases.clear(),
        }
        let mut lm_movers = if tiers.csm || local_shadows {
            self.ensure_entity_caster_geometries(cx.cx);
            self.collect_lm_movers(world, camera_pos, skinned.as_ref().map(|b| b.items.as_slice()))
        } else {
            Vec::new()
        };
        if tiers.csm && !self.world_atlas_required() {
            self.append_unbaked_world_casters(cx.cx, world, &mut lm_movers);
        }
        // The camera slice the Realtime cascades fit to: the far-plane
        // corners of THIS view, unprojected (far = clip z +w in every
        // backend's convention). Flat stage only — in XR the runtime owns
        // the eye matrices, and the baker falls back to eye-centered rings.
        let csm_view = (self.stage.mode == StageMode::Flat)
            .then(|| {
                let clip = Mat4f::mul(
                    &scene_state.projection,
                    &Mat4f::mul(&scene_state.view, &stage_matrix),
                );
                let inv = clip.invert();
                let corner = |x: f32, y: f32| {
                    let p = inv.transform_vec4(vec4(x, y, 1.0, 1.0));
                    if p.w.abs() < 1.0e-9 {
                        None
                    } else {
                        Some(vec3f(p.x / p.w, p.y / p.w, p.z / p.w))
                    }
                };
                Some(crate::shadow_csm::CsmView {
                    cam: camera_pos,
                    far_corners: [
                        corner(-1.0, -1.0)?,
                        corner(1.0, -1.0)?,
                        corner(-1.0, 1.0)?,
                        corner(1.0, 1.0)?,
                    ],
                    focus_distance: self.csm_focus.unwrap_or(0.0),
                })
            })
            .flatten();
        // The GPU light bake + cascades: encode this frame's passes (they
        // render BEFORE this scene pass, so a delivered atlas / fresh
        // cascade set is never sampled stale — no readback, no upload).
        // The streamed world: uploads, residency, selection, occlusion and
        // its shadow casters — before the cascades that draw them.
        if self.stream.is_some() {
            let clip = (self.stage.mode == StageMode::Flat).then(|| Mat4f::mul(
                &scene_state.projection,
                &Mat4f::mul(&scene_state.view, &stage_matrix),
            ));
            self.stream_prepare(cx.cx, camera_pos, clip.as_ref(), frustum);
        }
        self.run_gpu_lightmap(cx.cx, world, &lm_movers, csm_view.as_ref(), camera_pos);
        let (csm_statics, csm_movers, csm_us) = self.gpu_baker.csm_frame_stats();
        stats.csm_static_casters = csm_statics as u64;
        stats.csm_movers = csm_movers as u64;
        stats.csm_encode_us = csm_us;
        stats.csm_draws = self.gpu_baker.csm_draws() as u64;
        stats.csm_gpu_ms = self.gpu_baker.csm_gpu_ms();
        if self.clustered_enabled {
            self.clustered.update(cx.cx, &self.frame_lights, cluster_view);
            let statics = if local_shadows {
                if !(tiers.csm && !self.world_atlas_required()) {
                    self.append_unbaked_world_casters(cx.cx, world, &mut lm_movers);
                }
                self.collect_local_static_casters(cx.cx, world)
            } else { Vec::new() };
            self.clustered.render_shadows(cx.cx, camera_pos, &statics, &lm_movers);
            stats.clustered = self.clustered.stats();
        }
        if self.gi.mode()!=crate::GiMode::Off && self.clustered_enabled && cx.gpu_info().float_color_targets {
            self.run_fast_gi(cx, world, camera_pos, &scene_state, skinned.as_ref().map(|b| b.items.as_slice()), &sun, &mut stats);
        }
        draw_list.begin_always(cx);
        cx.begin_scene_3d(scene_state);
        // Camera-relative rendering: world geometry shifts by the render
        // origin on the GPU (the pass view was rebuilt around it by the
        // host's `Renderer::set_pass_camera`); CPU-side culling, cascades
        // and clusters above keep working in true world space.
        let origin = self.render_origin_for(camera_pos);
        let scene_view_transform = if origin == Vec3f::default() { stage_matrix } else {
            let mut shift = Mat4f::identity();
            shift.v[12] = -origin.x;
            shift.v[13] = -origin.y;
            shift.v[14] = -origin.z;
            Mat4f::mul(&shift, &stage_matrix)
        };
        let previous_world = cx.set_scene_world_transform_3d(scene_view_transform);
        // The stage rides on the draw list's view transform: every game
        // shader computes `draw_list.view_transform * transform`, so one
        // uniform scales and anchors sky, terrain, cubes and characters
        // together. The instance transforms below stay in world units, which
        // is why the cached static slabs survive a stage change untouched.
        // (The view matrix cannot carry this — in XR the runtime overwrites
        // camera_view/_r with its own eye matrices every frame.)
        cx.cx.draw_lists[draw_list.id()]
            .draw_list_uniforms
            .view_transform = scene_view_transform;
        // MR puts the game on your real floor: the room supplies the
        // horizon, so the game's own environment would only paint over the
        // passthrough feed.
        let shows_environment = self.stage.shows_environment();

        // One sun for every shader this frame (sun.rs). Written before any
        // batch begins, because instance fields are snapshotted per draw and
        // uniforms are captured when the draw item opens.
        let sun = {
            let sun = crate::sun::resolve_sun(&world.sun);
            if self.hdr_output {
                let mut hdr = sun.to_hdr();
                self.hdr_fill_from_sky(world, &mut hdr);
                hdr
            } else {
                sun
            }
        };

        // SDF-atlas sun era: the sidecars bake against one sun elevation.
        // An EXPLICIT sun change (OnChange's only kind — the day cycle
        // forces Realtime, where the SDF tier is off) drops the caches and
        // re-tries the sidecars against the new sun; a caster whose sidecar
        // disagrees falls to the blob tier. No runtime re-bake exists.
        {
            let len = sun.shadow_len_per_unit();
            if (len - self.sdf_baked_sun_len).abs() > 1.0e-3 {
                self.sdf_baked_sun_len = len;
                self.sdf_atlas_tex.clear();
                self.model_sdf_tex.clear();
            }
        }

        // The analytic (Preetham) sky replaces the hand-painted gradient
        // wherever the script kept the DEFAULT dome colours — a script that
        // authored its own top/ground colours keeps its gradient. The
        // horizon colour stays customisable either way: it only ever tinted
        // the FOG, and in analytic mode the fog instead comes from the
        // model's own tone-mapped horizon so sky and haze agree.
        let sky_frame = analytic_sky_frame(world, sun.dir, shows_environment, self.hdr_output, self.sky_clock);

        // Fog only exists once the script asked for a sky.
        let (fog_color, fog_density) = match &world.sky {
            Some(sky) if shows_environment => {
                let c = if self.hdr_output {
                    // Linear horizon radiance; an authored gradient keeps
                    // its authored brightness through the exposure.
                    sky_frame.as_ref().map(|frame| frame.fog_linear * HDR_SKY_GAIN).unwrap_or_else(|| {
                        srgb_to_linear(vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z))
                            * (1.0 / self.hdr_exposure)
                    })
                } else {
                    sky_frame
                        .as_ref()
                        .map(|frame| frame.fog_rgb)
                        .unwrap_or(vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z))
                };
                (c, sky.fog)
            }
            _ => (vec3(0.75, 0.87, 0.96), 0.0),
        };
        // HDR lane: per-pixel exponential height fog (clustered.rs
        // scene_fog) with the game's density at the base height.
        let fog_on = self.hdr_output && fog_density > 0.0;
        self.clustered.fog_ctl = if fog_on { [HDR_FOG_BASE, 1.0 / HDR_FOG_SCALE_HEIGHT, 0.0, 1.0] } else { [0.0; 4] };
        self.clustered.fog_eye = [camera_pos.x, camera_pos.y, camera_pos.z, 0.0];
        apply_sun(cx.cx, draws, &sun, fog_color);
        // The ground light field for the cube family (statics AND dynamics —
        // a mover crossing a baked shadow should darken). Uniforms, not
        // instance lanes: the packed slab layout stays untouched.
        {
            let lm_tex = self.lightmap_texture(cx.cx);
            // The shadow-top plane: a raised slab or ramp top compares its
            // fragment height against the height the shadow was measured
            // at, instead of wearing the grass-level shadow under it.
            let (top_tex, top_base, top_range) = self.lm_top_binding(cx.cx);
            let (lm_rect, lm_world) = self.lm_ground.unwrap_or_default();
            let csm = self.gpu_baker.csm_binding();
            trace!(
                "csm",
                "bind rx0w={:.4} rz0w={:.4} tex={:?}",
                csm.as_ref().map_or(0.0, |(f, _, _)| f.cascades[0].rx.w),
                csm.as_ref().map_or(0.0, |(f, _, _)| f.cascades[0].rz.w),
                csm.as_ref().map(|(_, t, _)| t.texture_id())
            );
            for dv in [
                &mut draws.cube.cube.draw_vars,
                &mut draws.alpha.cube.cube.draw_vars,
            ] {
                dv.set_texture(0, &lm_tex);
                dv.set_texture(1, &top_tex);
                Self::write_csm_uniforms(cx.cx, dv, &csm, &lm_tex, 2, self.shadow_debug);
                dv.set_uniform(
                    cx.cx,
                    live_id!(lm_rect),
                    &[lm_rect.x, lm_rect.y, lm_rect.z, lm_rect.w],
                );
                dv.set_uniform(
                    cx.cx,
                    live_id!(lm_world),
                    &[lm_world.x, lm_world.y, lm_world.z, lm_world.w],
                );
                dv.set_uniform(
                    cx.cx,
                    live_id!(lm_top_decode),
                    &[top_base, top_range, 0.0, 0.0],
                );
            }
        }
        // This frame's dynamic light list (lamps + firework flashes + host
        // lights), then the TRANSIENT slice for the world-spanning shaders.
        // Cube slabs and terrain sum lights per PIXEL and receive only the
        // transients — their street-lamp light is already baked into the
        // atlas RGB, and adding it analytically would double-light every
        // static surface. Written before any of their draw items open.
        if self.clustered_enabled {
            self.clustered_frames += 1;
            if self.clustered_frames == 1 || (self.clustered_frames % 120 == 0 && std::env::var_os("MAKEPAD_CLUSTER_STATS").is_some()) {
                log!("clustered forward: {} lights, {} refs, {} occupied clusters, max {} lights/cluster, {} us build/upload, {} bytes; {} grid; world atlas {}",
                    stats.clustered.lights, stats.clustered.references, stats.clustered.occupied_clusters,
                    stats.clustered.max_lights_in_cluster, stats.clustered.build_us, stats.clustered.upload_bytes,
                    if stats.clustered.world_grid { "world/XR" } else { "frustum" }, self.world_atlas_required());
                let s = stats.clustered.shadows;
                if s.faces > 0 || s.omitted_lights > 0 {
                    log!("local shadows: {} lights, {} faces, {} omitted, {} caster draws, {} us CPU encode",
                        s.lights,s.faces,s.omitted_lights,s.caster_draws,s.encode_us);
                }
            }
        } else {
            let transients = self.frame_baked_count..self.frame_lights.len();
            select_lights_for_world(
                &self.frame_lights,
                transients,
                camera_pos,
                frustum,
                &mut self.light_rank,
                &mut self.light_sel,
            );
            for dv in [
                &mut draws.cube.cube.draw_vars,
                &mut draws.alpha.cube.cube.draw_vars,
                &mut draws.terrain.draw_vars,
            ] {
                write_light_uniforms(
                    cx.cx,
                    dv,
                    &self.frame_lights,
                    &self.light_sel,
                    self.light_sel.len(),
                );
            }
        }
        for dv in [&mut draws.cube.cube.draw_vars, &mut draws.alpha.cube.cube.draw_vars, &mut draws.terrain.draw_vars] {
            self.clustered.bind(cx.cx, dv, self.clustered_enabled);
            self.gi.bind(cx.cx, dv);
        }

        // 1. Sky dome around the camera (depth-tested at radius, drawn
        // first). Default-sky worlds and worlds on a running clock draw
        // the ANALYTIC dome (Preetham + setting sun + stars, tinted by an
        // authored palette); authored gradients under a fixed hour keep
        // DrawSceneSky.
        if let Some(sky) = world.sky.as_ref().filter(|_| shows_environment) {
            let mut transform = Mat4f::identity();
            transform.v[12] = camera_pos.x;
            transform.v[13] = camera_pos.y;
            transform.v[14] = camera_pos.z;
            match (&sky_frame, &mut draws.sky_analytic) {
                (Some(f), Some(sa)) => {
                    sa.cube.transform = transform;
                    sa.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    sa.cube.cube_size = vec3(800.0, 800.0, 800.0);
                    // rgb: the authored palette's tint of the day dome.
                    sa.cube.color = vec4(f.dome_tint.x, f.dome_tint.y, f.dome_tint.z, 1.0);
                    sa.cube.depth_clip = 1.0;
                    sa.pz_y = f.pz_y;
                    sa.pz_x = f.pz_x;
                    sa.pz_yc = f.pz_yc;
                    sa.pz_e = f.pz_e;
                    sa.pz_f0 = f.pz_f0;
                    sa.zenith = f.zenith;
                    sa.sun_e = f.sun;
                    sa.sun_true = f.sun_true;
                    // w > 0: the HDR lane's linear radiance dome at this gain.
                    sa.sun_true.w = if self.hdr_output { HDR_SKY_GAIN } else { 0.0 };
                    // The star dome: panorama texture (or the 1x1 black
                    // stand-in) plus the celestial rotation, derived from
                    // the SAME hour that placed the sun — no host has to
                    // drive it, so every path that sets a time of day gets
                    // a sky that wheels correctly. A world with no hour
                    // (an authored sun direction) keeps a fixed dome.
                    let (star_tex, gain) = self.star_binding(cx.cx);
                    sa.cube.draw_vars.set_texture(0, &star_tex);
                    let rows = match world.sun.time_of_day {
                        Some(hours) => crate::sun::star_rows(hours, world.sun.latitude),
                        None => [
                            vec4(1.0, 0.0, 0.0, 0.0),
                            vec4(0.0, 1.0, 0.0, 0.0),
                            vec4(0.0, 0.0, 1.0, 0.0),
                        ],
                    };
                    sa.star_r0 = vec4(rows[0].x, rows[0].y, rows[0].z, gain);
                    sa.star_r1 = rows[1];
                    sa.star_r2 = rows[2];
                    // The dome takes the world's height fog toward the
                    // horizon (HDR lane), so sky and haze meet without a seam.
                    let sky_fog = if fog_on {
                        fog_density * HDR_FOG_SCALE_HEIGHT
                            * ((HDR_FOG_BASE - camera_pos.y) / HDR_FOG_SCALE_HEIGHT).min(30.0).exp()
                    } else {
                        0.0
                    };
                    // y: one screen pixel's angle (radians), which sizes the
                    // point stars to a pixel at any resolution and lens.
                    let pixel_angle = 2.0
                        / (scene_state.projection.v[5].abs()
                            * (scene_state.viewport_rect.size.y * cx.current_dpi_factor()).max(1.0) as f32)
                            .max(1.0e-3);
                    sa.cube.draw_vars.set_uniform(cx.cx, live_id!(sky_fog), &[sky_fog, pixel_angle, 0.0, if fog_on { 1.0 } else { 0.0 }]);
                    sa.cube.draw_vars.set_uniform(cx.cx, live_id!(sky_fog_color), &[fog_color.x, fog_color.y, fog_color.z]);
                    sa.cube.draw(cx);
                }
                _ => {
                    draws.sky.cube.transform = transform;
                    draws.sky.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.sky.cube.cube_size = vec3(800.0, 800.0, 800.0);
                    draws.sky.cube.color = vec4(1.0, 1.0, 1.0, 1.0);
                    draws.sky.cube.depth_clip = 1.0;
                    // An authored dome is art: the HDR lane shows it at its
                    // authored brightness (linear, pre-divided by exposure).
                    let dome = |c: Vec3f| {
                        if self.hdr_output { srgb_to_linear(c) * (1.0 / self.hdr_exposure) } else { c }
                    };
                    draws.sky.sky_top = dome(vec3(sky.top.x, sky.top.y, sky.top.z));
                    draws.sky.sky_horizon =
                        dome(vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z));
                    draws.sky.sky_ground = dome(vec3(sky.ground.x, sky.ground.y, sky.ground.z));
                    draws.sky.sky_bottom =
                        dome(vec3(sky.ground_bottom.x, sky.ground_bottom.y, sky.ground_bottom.z));
                    draws.sky.cube.draw(cx);
                }
            }
            stats.sky_drawn = true;
        }

        // 2. The smooth terrain mesh, in CHUNK_SIZE tiles so the hills
        // behind the camera cost nothing. One draw item per visible tile —
        // the price of being able to skip the rest.
        // Terrain can be megabytes at 257². It is immutable for the whole
        // draw, so borrow it from the world; cloning here turned a steady
        // landscape into frame-rate-scaled memory bandwidth.
        if let Some(terrain) = world.terrain.as_deref().filter(|_| shows_environment) {
            self.ensure_terrain_tiles(cx.cx, terrain, world.terrain_materials.as_deref());
            draws.terrain.transform = Mat4f::identity();
            draws.terrain.depth_clip = 1.0;
            draws.terrain.fog_color = fog_color;
            draws.terrain.fog_density = fog_density;
            draws.terrain.lm_debug = self.lm_debug;
            // One planar lightmap region serves every tile: tile uvs derive
            // from world xz, so all tiles share one rect pair.
            let (lm_rect, lm_world) = self.lm_ground.unwrap_or_default();
            draws.terrain.lm_rect = lm_rect;
            draws.terrain.lm_world = lm_world;
            let lm_tex = self.lightmap_texture(cx.cx);
            draws.terrain.draw_vars.set_texture(0, &lm_tex);
            let csm = self.gpu_baker.csm_binding();
            Self::write_csm_uniforms(cx.cx, &mut draws.terrain.draw_vars, &csm, &lm_tex, 1, self.shadow_debug);
            for tile in &self.terrain_tiles {
                if let Some(frustum) = frustum {
                    if !frustum.intersects_aabb(tile.min, tile.max) {
                        stats.terrain_tiles_culled += 1;
                        continue;
                    }
                }
                draws.terrain.draw_vars.geometry_id = Some(tile.geometry.geometry_id());
                if draws.terrain.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draws.terrain.draw_vars);
                    draws.terrain.draw_vars.area =
                        cx.update_area_refs(draws.terrain.draw_vars.area, new_area);
                }
                stats.terrain_tiles_drawn += 1;
                stats.terrain_drawn = true;
            }
        }

        // 2b. Voxel terrain chunk meshes (mix.md T2/T3): the editable field's
        // surface, drawn through the SAME terrain shader and planar lightmap
        // region as the tiles — the render path the heightfield already
        // earned, reused rather than reinvented. One item per visible chunk.
        self.ensure_voxel_tiles(cx.cx, world.voxel.as_deref());
        if !self.voxel_tiles.is_empty() && shows_environment {
            draws.terrain.transform = Mat4f::identity();
            draws.terrain.depth_clip = 1.0;
            draws.terrain.fog_color = fog_color;
            draws.terrain.fog_density = fog_density;
            draws.terrain.lm_debug = self.lm_debug;
            let (lm_rect, lm_world) = self.lm_ground.unwrap_or_default();
            draws.terrain.lm_rect = lm_rect;
            draws.terrain.lm_world = lm_world;
            let lm_tex = self.lightmap_texture(cx.cx);
            draws.terrain.draw_vars.set_texture(0, &lm_tex);
            let csm = self.gpu_baker.csm_binding();
            Self::write_csm_uniforms(cx.cx, &mut draws.terrain.draw_vars, &csm, &lm_tex, 1, self.shadow_debug);
            for tile in &self.voxel_tiles {
                if let Some(frustum) = frustum {
                    if !frustum.intersects_aabb(tile.min, tile.max) {
                        stats.terrain_tiles_culled += 1;
                        continue;
                    }
                }
                draws.terrain.draw_vars.geometry_id = Some(tile.geometry.geometry_id());
                if draws.terrain.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draws.terrain.draw_vars);
                    draws.terrain.draw_vars.area =
                        cx.update_area_refs(draws.terrain.draw_vars.area, new_area);
                }
                stats.terrain_tiles_drawn += 1;
                stats.terrain_drawn = true;
            }
        }

        // PERF: sections 3+4 batch per shape through many_instances. Statics
        // come from packed slabs rebuilt only when world.render_rev moves
        // (bump it — mark_render_dirty — or your static edit won't show);
        // dynamics (movers, their parts, beams, blob shadows) re-pack every
        // frame. One draw call per shape per pass; empty batches are skipped.
        // fog_color is a uniform now (set in apply_sun); only the density
        // stays per-instance, because shadows switch it off individually.
        draws.cube.fog_density = fog_density;
        draws.alpha.cube.fog_density = fog_density;

        // CPU light bake (bake.rs). Geometry-dependent occlusion is baked
        // once per world edit; the sun term is one ray per target and
        // refreshes whenever the sun swings, so a day cycle moves the baked
        // shadows instead of freezing them at dawn. Both land in the colours
        // packed below — the GPU never learns this happened.
        if self.world_atlas_required() {
            self.bake.update(world, &sun);
        }
        stats.bake = self.bake.stats();

        let vars_ready = draws.cube.cube.draw_vars.can_instance()
            && draws.alpha.cube.cube.draw_vars.can_instance();
        let slab_key = static_slab_key(world, self.bake.generation());
        if vars_ready && self.slab_key != Some(slab_key) {
            let t0 = Cx::monotonic_now();
            self.rebuild_static_slabs(draws, world);
            stats.slab_us += perf_us(t0);
            stats.slab_rebuilds += 1;
            self.slab_key = Some(slab_key);
        }
        stats.static_instances = self.slab_instance_count;
        stats.dyn_instances = 0;
        stats.instance_floats = draws.cube.cube.draw_vars.as_slice().len() as u32;

        // Slab chunk visibility, once per frame — both passes below memcpy
        // only the chunks whose content bounds touch the frustum. The chunks
        // themselves are camera-independent, so this never invalidates them.
        self.chunk_visible.clear();
        for chunk in &self.static_chunks {
            let visible = match frustum {
                Some(frustum) => frustum.intersects_aabb(chunk.min, chunk.max),
                None => true,
            };
            self.chunk_visible.push(visible);
            if visible {
                stats.chunks_drawn += 1;
            } else {
                stats.chunks_culled += 1;
            }
        }

        // Adaptive quality, resolved once per frame so every consumer below
        // sees the same level even if the governor moves mid-frame.
        let quality = self.thermometer.quality();
        // The dynamic shadow-mesh layer (entity hull drapes + pre-sidecar
        // blobs), OnChange only: in Realtime every caster is in the tiles.
        // Dropping projected_shadows demotes hulls to blobs — grounded but
        // cheaper — under thermal pressure.
        let shadow_mesh_enabled =
            draws.shadow.is_some() && quality.projected_shadows && tiers.sdf_quads;
        let shadow_budget =
            ((self.shadow_budget as f32 * quality.shadow_caster_scale).round() as usize).max(1);
        self.shadow_mesh.clear();
        // World-settle work, debounced: an edit burst pays ONE receiver-box
        // refresh + lightmap kick at rest. GENUINE world edits only — this
        // key never moves from routine mover/replication traffic, which is
        // what keeps OnChange at zero bake passes in steady state (the
        // two-mode invariant; gpu_lightmap.rs pins it).
        if self.world_atlas_required() {
            // The DAYLIGHT quantum is in the key: a lamp's strength is a
            // function of the sky (the headroom rail), so a sun that moves
            // enough to change it has changed the atlas, not just the shade
            // term. Quantized, so a day cycle pays a bake per 3% of pool —
            // not one per frame.
            let day_key = self.lamp_daylight_key(&sun);
            let key = (
                world.render_rev,
                self.bake.generation(),
                self.models_rev,
                day_key,
            );
            if self
                .shadow_gate
                .should_rebuild(key, Cx::monotonic_now(), SHADOW_SETTLE)
            {
                self.refresh_shadow_receivers(world);
                // Same settle cadence: the light bake becomes GPU render
                // passes on the next frame (gpu_lightmap.rs). In Realtime
                // the baker re-bakes visible regions per frame on its own —
                // only a WORLD change (or a sun change that moves the lamps)
                // re-schedules the whole job; OnChange re-kicks on every
                // settle, sun changes included.
                let world_key = lightmap_world_key(world, self.models_rev, day_key);
                if self.world_atlas_required() && (self.lm_kick_key != Some(world_key)
                    || lightmap_sun_changed(self.lm_kick_sun, sun.dir, self.gpu_baker.mode()))
                {
                    // Name the cause in the bake's own log line: a blowout
                    // that pops in has to be attributable to the run that
                    // caused it, not guessed at from a screenshot.
                    let trigger = match self.lm_kick_key {
                        None => crate::gpu_lightmap::BakeTrigger::FirstBake,
                        Some((rev, models, _)) if (rev, models) != (world_key.0, world_key.1) => {
                            crate::gpu_lightmap::BakeTrigger::WorldEdit
                        }
                        Some(_) => crate::gpu_lightmap::BakeTrigger::SunChange,
                    };
                    self.lm_kick_key = Some(world_key);
                    self.lm_kick_sun = Some(sun.dir);
                    self.kick_lightmap_bake(world, &sun, trigger);
                }
                self.shadow_gate.mark_built(key);
            }
        }

        // PERF: resolve dynamic parts and shape membership ONCE per frame —
        // the per-shape loops below must not re-scan entities per part.
        let mut dyn_parts: Vec<(usize, usize)> = Vec::new();
        for (part_index, part) in world.parts.iter().enumerate() {
            let Some(owner_index) = entity_index_sorted(&world.entities, part.owner) else {
                continue;
            };
            if world.entities[owner_index].kind != BodyKind::Static
                || part.animated
            {
                dyn_parts.push((part_index, owner_index));
            }
        }
        let mut dyn_entity_shapes = [false; 5];

        let mut dyn_sensor_shapes = [false; 5];
        for e in world
            .entities
            .iter()
            .filter(|e| e.kind != BodyKind::Static && !e.hidden)
        {
            match primitive_bucket(e) {
                Some(PrimitiveBucket::Opaque) => dyn_entity_shapes[e.shape.index()] = true,
                Some(PrimitiveBucket::Alpha) => dyn_sensor_shapes[e.shape.index()] = true,
                None => {}
            }
        }
        let mut dyn_part_shapes = [false; 5];
        let mut deferred_alpha: [Vec<DeferredAlphaCube>; 5] = Default::default();
        for (part_index, _) in &dyn_parts {
            dyn_part_shapes[world.parts[*part_index].shape.index()] = true;
        }

        // 3. Opaque pass, one batch per shape. Statics stream in from the
        // VISIBLE chunks only — still the same single draw item per shape,
        // the culling just shrinks what gets memcpy'd into it.
        for shape in Shape::ALL {
            let shape_index = shape.index();
            let has_static = self
                .static_chunks
                .iter()
                .zip(&self.chunk_visible)
                .any(|(c, v)| *v && !c.slab[shape_index].is_empty());
            let has_dynamic_entity = dyn_entity_shapes[shape_index];
            let has_dynamic_part = dyn_part_shapes[shape_index];
            let has_beams = shape == Shape::Box && !world.beams.is_empty();
            if !has_static && !has_dynamic_entity && !has_dynamic_part && !has_beams {
                continue;
            }
            let geometry_id = self.ensure_shape_geometry(cx.cx, shape);
            draws.cube.cube.draw_vars.geometry_id = Some(geometry_id);
            draws.cube.cube.many_instances =
                cx.begin_many_instances(&draws.cube.cube.draw_vars);
            if has_static {
                if let Some(mi) = &mut draws.cube.cube.many_instances {
                    for (chunk, visible) in self.static_chunks.iter().zip(&self.chunk_visible) {
                        if *visible {
                            mi.instances.extend_from_slice(&chunk.slab[shape_index]);
                        }
                    }
                }
            }
            // Dynamic entities: movers/kinematics/projectiles of this shape.
            for e in world
                .entities
                .iter()
                .filter(|e| {
                    primitive_bucket(e) == Some(PrimitiveBucket::Opaque)
                        && !e.hidden
                        && e.kind != BodyKind::Static
                        && e.shape == shape
                })
            {
                // Every shape geometry spans [-0.5, 0.5] scaled by cube_size,
                // so |half*scale| bounds the visual under ANY rotation. The
                // entity's cast shadow is not tied to this instance — blobs
                // and silhouettes draw from their own loops below.
                if let Some(frustum) = frustum {
                    let r = vec3f(
                        e.half.x * e.scale.x,
                        e.half.y * e.scale.y,
                        e.half.z * e.scale.z,
                    )
                    .length();
                    if !frustum.intersects_sphere(e.pos, r) {
                        stats.dyn_culled += 1;
                        continue;
                    }
                }
                let mut transform = Self::entity_rotation(e);
                transform.v[12] = e.pos.x;
                transform.v[13] = e.pos.y;
                transform.v[14] = e.pos.z;
                let size = vec3(
                    e.half.x * 2.0 * e.scale.x,
                    e.half.y * 2.0 * e.scale.y,
                    e.half.z * 2.0 * e.scale.z,
                );
                // Movers sample the baked probe lattice, so a crate rolling
                // under a bridge darkens without a shadow map or a pass.
                let color = shade_color(e.color, self.bake.dynamic_shade(e.pos), e.glow);
                if color.w < 0.999 {
                    deferred_alpha[shape_index].push(DeferredAlphaCube {
                        transform,
                        size,
                        color,
                        glow: e.glow,
                        color_adjust_ctl: e.color_adjust.instance(),
                    });
                    continue;
                }
                draws.cube.cube.transform = transform;
                draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.cube.cube.cube_size = size;
                draws.cube.cube.color = color;
                draws.cube.cube.depth_clip = 1.0;
                draws.cube.glow = e.glow;
                draws.cube.color_adjust_ctl = e.color_adjust.instance();
                draws.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            // Parts that are NOT in the slab: dynamic owner, or mid-animation.
            for (part_index, owner_index) in dyn_parts.iter().copied() {
                let part = &world.parts[part_index];
                if part.shape != shape {
                    continue;
                }
                let owner = &world.entities[owner_index];
                let transform = Self::part_transform(owner, part);
                // Both frames are pure rotations, so the composed transform's
                // translation is the part's world centre and |half*scale|
                // bounds it, exactly as for the entity above.
                if let Some(frustum) = frustum {
                    let center = vec3f(transform.v[12], transform.v[13], transform.v[14]);
                    let r = vec3f(
                        part.half.x * owner.scale.x,
                        part.half.y * owner.scale.y,
                        part.half.z * owner.scale.z,
                    )
                    .length();
                    if !frustum.intersects_sphere(center, r) {
                        stats.dyn_culled += 1;
                        continue;
                    }
                }
                let size = vec3(
                    part.half.x * 2.0 * owner.scale.x,
                    part.half.y * 2.0 * owner.scale.y,
                    part.half.z * 2.0 * owner.scale.z,
                );
                let color = shade_color(part.color, self.bake.dynamic_shade(owner.pos), part.glow);
                if color.w < 0.999 {
                    deferred_alpha[shape_index].push(DeferredAlphaCube {
                        transform,
                        size,
                        color,
                        glow: part.glow,
                        color_adjust_ctl: owner.color_adjust.instance(),
                    });
                    continue;
                }
                draws.cube.cube.transform = transform;
                draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.cube.cube.cube_size = size;
                draws.cube.cube.color = color;
                draws.cube.cube.depth_clip = 1.0;
                draws.cube.glow = part.glow;
                draws.cube.color_adjust_ctl = owner.color_adjust.instance();
                draws.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            // Immediate-mode beams (box batch): a box stretched between two
            // points (grapple cables, lasers). Cable axis on local z.
            if has_beams {
                for beam in &world.beams {
                    let d = beam.to - beam.from;
                    let len = d.length();
                    if len < 1.0e-4 {
                        continue;
                    }
                    // Sphere around the beam's midpoint; the box is size x
                    // size x len, so half the length plus the full cross
                    // section covers it under any orientation.
                    if let Some(frustum) = frustum {
                        let mid = beam.from + d * 0.5;
                        if !frustum.intersects_sphere(mid, len * 0.5 + beam.size) {
                            stats.dyn_culled += 1;
                            continue;
                        }
                    }
                    let f = d * (1.0 / len);
                    let upv = if f.y.abs() > 0.99 {
                        vec3f(1.0, 0.0, 0.0)
                    } else {
                        vec3f(0.0, 1.0, 0.0)
                    };
                    let r = Vec3f::cross(upv, f).normalize();
                    let u = Vec3f::cross(f, r);
                    let mid = beam.from + d * 0.5;
                    let mut m = Mat4f::identity();
                    m.v[0] = r.x;
                    m.v[1] = r.y;
                    m.v[2] = r.z;
                    m.v[4] = u.x;
                    m.v[5] = u.y;
                    m.v[6] = u.z;
                    m.v[8] = f.x;
                    m.v[9] = f.y;
                    m.v[10] = f.z;
                    m.v[12] = mid.x;
                    m.v[13] = mid.y;
                    m.v[14] = mid.z;
                    if beam.color.w < 0.999 {
                        deferred_alpha[shape_index].push(DeferredAlphaCube {
                            transform: m,
                            size: vec3(beam.size, beam.size, len),
                            color: beam.color,
                            glow: beam.glow,
                            color_adjust_ctl: vec4(0.0, 1.0, 1.0, 0.0),
                        });
                        continue;
                    }
                    draws.cube.cube.transform = m;
                    draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.cube.cube.cube_size = vec3(beam.size, beam.size, len);
                    draws.cube.cube.color = beam.color;
                    draws.cube.cube.depth_clip = 1.0;
                    draws.cube.glow = beam.glow;
                    draws.cube.color_adjust_ctl = vec4(0.0, 1.0, 1.0, 0.0);
                    draws.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                }
            }
            if let Some(mi) = draws.cube.cube.many_instances.take() {
                cx.end_many_instances(mi);
            }
        }

        // 3.5 Skinned characters — after all opaque, before alpha blending.
        if let Some(batch) = skinned {
            // Sidecar resolution precedes the shadow loop below (it
            // consults the uploaded atlases). OnChange only: in Realtime
            // the SDF tier draws nothing, so there is nothing to resolve.
            if tiers.sdf_quads {
                self.seed_skinned_sdf(cx.cx, &batch.items, &sun);
            }
            // Character shadows: one SDF-silhouette quad each. Per
            // character per frame the CPU computes ONE anchor (ground
            // sample + lamp/height policy, character_shadow_anchor) and
            // pushes one five-vec4 instance record — the pixel stage
            // morphs the baked silhouette between the yaw/phase neighbour
            // cells by lerping DISTANCES (shadow_sdf.rs), so this loop's
            // cost is flat and tiny no matter how the crowd grows. The
            // anchor stays the single boss of placement: its landing point
            // positions the quad, its lean supplies the owning light's
            // azimuth (sun blended toward a dominant lamp), its size_mul
            // compresses the sprite under a near-overhead bulb, its alpha
            // darkens it. The blob survives only as the fallback for a rig
            // with no loadable sidecar (or a host with no SDF shader).
            // Realtime draws NONE of this — characters are in the tiles.
            if shadow_mesh_enabled {
                let t0 = Cx::monotonic_now();
                let ground = world
                    .terrain
                    .as_ref()
                    .and_then(|t| {
                        let p = batch.items.first()?;
                        t.height_at(p.transform.v[12], p.transform.v[14])
                    })
                    .unwrap_or(0.0);
                for item in batch.items.iter() {
                    let t = &item.transform;
                    // Footprint from the transform's own scale: characters
                    // are authored around a ~0.45-unit half footprint.
                    let sx = (t.v[0] * t.v[0] + t.v[1] * t.v[1] + t.v[2] * t.v[2]).sqrt();
                    let sz = (t.v[8] * t.v[8] + t.v[9] * t.v[9] + t.v[10] * t.v[10]).sqrt();
                    let receiver = Receiver {
                        base_y: ground,
                        terrain: world.terrain.as_deref(),
                        statics: &self.receiver_boxes,
                    };
                    let feet = vec3f(t.v[12], t.v[13], t.v[14]);
                    let Some(a) = self.character_shadow_anchor(feet, &receiver, &sun)
                    else {
                        continue;
                    };
                    if draws.shadow_sdf.is_some() {
                        if let Some((_, Some((_, meta)))) = self
                            .sdf_atlas_tex
                            .iter()
                            .find(|(r, _)| *r == item.rig)
                        {
                            let yaw = t.v[8].atan2(t.v[10]);
                            // Horizontal direction TOWARD the owning light:
                            // opposite the anchor's lean (the silhouette
                            // points down the lean); a lean too small to
                            // read (dead under a bulb, or a noon sun) falls
                            // back to the sun's azimuth.
                            let l = (a.lean.x * a.lean.x + a.lean.y * a.lean.y).sqrt();
                            let (gx, gz) = if l > 1.0e-3 {
                                (-a.lean.x / l, -a.lean.y / l)
                            } else {
                                let g = sun.dir_ground();
                                (g.x, g.y)
                            };
                            // The atlas is baked in a canonical light frame
                            // (azimuth +x); the cell index is the yaw
                            // RELATIVE to the light: world = M(alpha) *
                            // canonical(yaw - alpha), alpha from the axis.
                            let rel = yaw - (-gz).atan2(gx);
                            let band2 = 2.0 * meta.band_world.max(1.0e-4);
                            let scale = sx.max(sz) * a.size_mul;
                            // Sun-tolerant stretch: the window IS the sample
                            // map, so scaling its ALONG components by the
                            // current-vs-baked shadow-length ratio stretches
                            // the baked silhouette to today's sun.
                            let sun_len = sun.shadow_len_per_unit();
                            let stretch =
                                (sun_len / meta.len_per_unit.max(0.05)).clamp(0.2, 5.0);
                            // Ride the highest surface under the
                            // silhouette's run (see sdf_quad_ground) —
                            // rect.x is the window's down-sun edge.
                            let y_quad = sdf_quad_ground(
                                a.root,
                                &receiver,
                                gx,
                                gz,
                                (-meta.rect.x * stretch).max(0.0) * scale,
                            );
                            // The quad's window origin — the bake's ground
                            // anchor, the silhouette's FOOT end — sits at
                            // the pinned root: the shadow stays attached to
                            // the boots however hard a lamp leans its body.
                            self.sdf_instances.push(SdfInstance {
                                atlas: SdfAtlasKey::Rig(item.rig),
                                a: vec4(a.root.x, y_quad, a.root.z, a.lift),
                                b: vec4(gx, gz, scale, a.alpha),
                                c: vec4(
                                    rel,
                                    item.gait_phase,
                                    item.gait_blend,
                                    meta.rows as f32,
                                ),
                                d: vec4(
                                    meta.rect.x * stretch,
                                    meta.rect.y,
                                    meta.rect.z * stretch,
                                    meta.rect.w,
                                ),
                                e: vec4(
                                    SDF_SOFT_BASE / band2,
                                    SDF_SOFT_HARDEN / (sun_len.max(0.2) * band2),
                                    0.0,
                                    0.0,
                                ),
                            });
                            stats.shadows += 1;
                            stats.sdf_shadow_instances += 1;
                            continue;
                        }
                        // Atlas still baking (or the rig baked to
                        // nothing): fall through to the blob.
                    }
                    // Blob fallback. Same anchor policy; the lamp/height
                    // darkening rides a local sun copy the mesh builder
                    // reads its alpha from.
                    let mut shadow_sun = sun;
                    shadow_sun.shadow_alpha =
                        (sun.shadow_alpha * (1.0 + 0.7 * a.lamp_w)).min(0.6);
                    // A blob is a contact shadow — it sits at the pinned
                    // root (never displaced by a lamp lean).
                    let centre = vec3f(a.root.x, feet.y, a.root.z);
                    if crate::shadow_mesh::build_blob_shadow(
                        centre,
                        0.45 * sx * a.size_mul,
                        0.45 * sz * a.size_mul,
                        &shadow_sun,
                        &receiver,
                        &mut self.shadow_mesh,
                    ) {
                        stats.shadows += 1;
                    }
                }
                stats.dyn_shadow_us += perf_us(t0);
            }
            // Ground height under every character (one receiver sample each):
            // the shader projects its baked-shadow lookup along the sun ray
            // down to this plane, so the boundary slants across the body and
            // a jumping character exits shadow as they rise.
            self.char_ground.clear();
            for item in &batch.items {
                let (x, z) = (item.transform.v[12], item.transform.v[14]);
                let base = world
                    .terrain
                    .as_ref()
                    .and_then(|t| t.height_at(x, z))
                    .unwrap_or(0.0);
                let receiver = Receiver {
                    base_y: base,
                    terrain: world.terrain.as_deref(),
                    statics: &self.receiver_boxes,
                };
                self.char_ground.push(receiver.sample(x, z).0);
            }
            self.draw_skinned_inner(
                cx,
                batch,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
                camera_pos,
            );
        }

        // Dynamic prop instances — the driveable cars. Their shadow is the
        // same SDF-silhouette quad the characters draw, from a yaw-only
        // atlas loaded from the model's offline `.shadowsdf` sidecar
        // (shadow_sdf.rs, tools/ao_bake): per frame each car costs one
        // anchor + one five-vec4 instance record, gait inputs inert. The
        // same anchor policy owns placement — lean and compression under a
        // dominant lamp, height offset on ramps. The atlas bakes the car
        // FLAT, so a heavily tilted or airborne car falls back to the
        // plain blob, as does any instance whose model has no sidecar.
        // Realtime draws none of this — the cars are in the tiles.
        if shadow_mesh_enabled {
            let t0 = Cx::monotonic_now();
            let instances = std::mem::take(&mut self.placed_models);
            for inst in &instances {
                if !inst.dynamic {
                    continue;
                }
                let Some((mmin, mmax)) = self
                    .static_models
                    .iter()
                    .find(|(k, _)| *k == inst.model)
                    .map(|(_, m)| (m.min, m.max))
                else {
                    continue;
                };
                // First sight of this model loads its sidecar (one stat +
                // read); a miss caches a None so the frame never re-tries.
                self.seed_model_sdf(cx.cx, &inst.model, &sun);
                let t = &inst.transform;
                let mid = vec3f(
                    (mmin.x + mmax.x) * 0.5,
                    mmin.y,
                    (mmin.z + mmax.z) * 0.5,
                );
                let feet = vec3f(
                    t.v[0] * mid.x + t.v[4] * mid.y + t.v[8] * mid.z + t.v[12],
                    t.v[1] * mid.x + t.v[5] * mid.y + t.v[9] * mid.z + t.v[13],
                    t.v[2] * mid.x + t.v[6] * mid.y + t.v[10] * mid.z + t.v[14],
                );
                let sx = (t.v[0] * t.v[0] + t.v[1] * t.v[1] + t.v[2] * t.v[2]).sqrt();
                let sz = (t.v[8] * t.v[8] + t.v[9] * t.v[9] + t.v[10] * t.v[10]).sqrt();
                let ground = world
                    .terrain
                    .as_ref()
                    .and_then(|tr| tr.height_at(feet.x, feet.z))
                    .unwrap_or(0.0);
                let receiver = Receiver {
                    base_y: ground,
                    terrain: world.terrain.as_deref(),
                    statics: &self.receiver_boxes,
                };
                // Tilt/air gates: body-up vs world-up from the transform's
                // y basis, clear air from the receiver under the footprint.
                let up_len =
                    (t.v[4] * t.v[4] + t.v[5] * t.v[5] + t.v[6] * t.v[6]).sqrt().max(1.0e-6);
                let up_y = t.v[5] / up_len;
                let air = feet.y - receiver.sample(feet.x, feet.z).0;
                let mut drawn = false;
                if draws.shadow_sdf.is_some() && car_sprite_allowed(up_y, air) {
                    if let Some(Some((_, meta))) = self.model_sdf_tex.get(&inst.model) {
                        if let Some(a) = character_shadow_anchor(
                            feet,
                            &receiver,
                            &sun,
                            &self.frame_lights,
                        ) {
                            let yaw = t.v[8].atan2(t.v[10]);
                            // Same owning-light frame math as the
                            // characters: axis toward the light, opposite
                            // the anchor's lean, sun azimuth when the lean
                            // is too small to read.
                            let l = (a.lean.x * a.lean.x + a.lean.y * a.lean.y).sqrt();
                            let (gx, gz) = if l > 1.0e-3 {
                                (-a.lean.x / l, -a.lean.y / l)
                            } else {
                                let g = sun.dir_ground();
                                (g.x, g.y)
                            };
                            let rel = yaw - (-gz).atan2(gx);
                            let band2 = 2.0 * meta.band_world.max(1.0e-4);
                            let scale = sx.max(sz) * a.size_mul;
                            // Sun-tolerant stretch, exactly as for rigs.
                            let sun_len = sun.shadow_len_per_unit();
                            let stretch =
                                (sun_len / meta.len_per_unit.max(0.05)).clamp(0.2, 5.0);
                            // Same raised-receiver guard as the characters:
                            // a car parked on grass beside a proud road
                            // slab must not bury its silhouette under it.
                            let y_quad = sdf_quad_ground(
                                a.root,
                                &receiver,
                                gx,
                                gz,
                                (-meta.rect.x * stretch).max(0.0) * scale,
                            );
                            // Window origin pinned at the root, exactly as
                            // for characters: the wheels' contact line
                            // never leaves the car.
                            self.sdf_instances.push(SdfInstance {
                                atlas: SdfAtlasKey::Model(inst.model.clone()),
                                a: vec4(a.root.x, y_quad, a.root.z, a.lift),
                                b: vec4(gx, gz, scale, a.alpha),
                                c: vec4(rel, 0.0, 0.0, meta.rows as f32),
                                d: vec4(
                                    meta.rect.x * stretch,
                                    meta.rect.y,
                                    meta.rect.z * stretch,
                                    meta.rect.w,
                                ),
                                e: vec4(
                                    SDF_SOFT_BASE / band2,
                                    SDF_SOFT_HARDEN / (sun_len.max(0.2) * band2),
                                    0.0,
                                    0.0,
                                ),
                            });
                            stats.shadows += 1;
                            stats.sdf_shadow_instances += 1;
                            drawn = true;
                        }
                    }
                }
                if !drawn {
                    crate::shadow_mesh::build_blob_shadow(
                        feet,
                        (mmax.x - mmin.x) * 0.55 * sx,
                        (mmax.z - mmin.z) * 0.55 * sz,
                        &sun,
                        &receiver,
                        &mut self.shadow_mesh,
                    );
                }
            }
            self.placed_models = instances;
            stats.dyn_shadow_us += perf_us(t0);
        }

        // Stock props: the same shader as the skinned path (both are textured
        // packed meshes), but with geometry uploaded once instead of per frame.
        if let Some(draw) = models_draw.as_deref_mut() {
            // Ground plane per DYNAMIC instance (cars), for the sun-ray
            // projected baked-shadow sample; statics never read it.
            self.model_ground.clear();
            for inst in &self.placed_models {
                if !inst.dynamic {
                    self.model_ground.push(0.0);
                    continue;
                }
                let (x, z) = (inst.transform.v[12], inst.transform.v[14]);
                let base = world
                    .terrain
                    .as_ref()
                    .and_then(|t| t.height_at(x, z))
                    .unwrap_or(0.0);
                let receiver = Receiver {
                    base_y: base,
                    terrain: world.terrain.as_deref(),
                    statics: &self.receiver_boxes,
                };
                self.model_ground.push(receiver.sample(x, z).0);
            }
            let instances = std::mem::take(&mut self.placed_models);
            self.prepare_foliage_lane(cx.cx);
            self.draw_models_inner(
                cx,
                ModelDraw::Diffuse(draw),
                camera_pos,
                &instances,
                WorldModelLane::Placed,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
            self.draw_pbr_models(
                cx,
                camera_pos,
                &instances,
                WorldModelLane::Placed,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
            self.draw_foliage_models(cx, camera_pos, &instances, WorldModelLane::Placed,
                (fog_color, fog_density), &sun, frustum, &mut stats);
            self.draw_custom_models(cx, camera_pos, &instances, WorldModelLane::Placed,
                (fog_color, fog_density), &sun, frustum, &mut stats);
            self.placed_models = instances;
            self.draw_stream(cx, draw, camera_pos, (fog_color, fog_density), &sun);
            self.draw_grass(cx, camera_pos, (fog_color, fog_density), &sun, frustum, &mut stats);

            // Actor-attached props share the world material/depth pass, but
            // this is their ONLY renderer traversal. In particular they
            // were absent from the lightmap/CSM mover and shadow-building
            // stages above. A receiver sample gives their shader the same
            // projected baked-shadow lookup as other moving geometry
            // without registering any receiver geometry of their own.
            self.world_attachment_ground.clear();
            for inst in &self.world_attachments {
                let (x, z) = (inst.transform.v[12], inst.transform.v[14]);
                let base = world
                    .terrain
                    .as_ref()
                    .and_then(|t| t.height_at(x, z))
                    .unwrap_or(0.0);
                let receiver = Receiver {
                    base_y: base,
                    terrain: world.terrain.as_deref(),
                    statics: &self.receiver_boxes,
                };
                self.world_attachment_ground
                    .push(receiver.sample(x, z).0);
            }
            let attachments = std::mem::take(&mut self.world_attachments);
            self.draw_models_inner(
                cx,
                ModelDraw::Diffuse(draw),
                camera_pos,
                &attachments,
                WorldModelLane::Attachment,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
            self.draw_pbr_models(
                cx,
                camera_pos,
                &attachments,
                WorldModelLane::Attachment,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
            self.draw_custom_models(cx, camera_pos, &attachments, WorldModelLane::Attachment,
                (fog_color, fog_density), &sun, frustum, &mut stats);
            self.world_attachments = attachments;
        }

        // 3s. The map's own sky surfaces, after the opaque world so they are
        // depth-rejected behind it rather than shading over it. Drawn even
        // when a host lends no models_draw: the sky lane owns its shader.
        self.draw_sky_faces(cx, camera_pos, frustum, &mut stats);

        self.draw_water(cx, draws, world, &sun, (fog_color, fog_density), frustum, shows_environment, camera_pos);

        // 4. Alpha pass, one batch per shape: static sensors from the slab,
        // then blob shadows (box batch) and dynamic sensors — drawn after all
        // opaque geometry so blending sees depth.
        for shape in Shape::ALL {
            let shape_index = shape.index();
            let has_static = self
                .static_chunks
                .iter()
                .zip(&self.chunk_visible)
                .any(|(c, v)| *v && !c.slab_alpha[shape_index].is_empty());
            let has_dynamic_sensor = dyn_sensor_shapes[shape_index];
            // Entity casters (crates, movers): runtime-spawned primitive
            // bodies with no offline atlas, so in OnChange their ONLY tier
            // is the hull drape / blob quad below. In Realtime they are in
            // the tiles (collect_lm_movers) and this tier is off.
            let has_shadows = shape == Shape::Box
                && tiers.sdf_quads
                && world.entities.iter().any(|e| {
                    matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
                        && !e.alpha_primitive
                        && !e.hidden
                        && e.parent == 0
                });
            let has_deferred = !deferred_alpha[shape_index].is_empty();
            if !has_static && !has_dynamic_sensor && !has_shadows && !has_deferred {
                continue;
            }
            let geometry_id = self.ensure_shape_geometry(cx.cx, shape);
            draws.alpha.cube.cube.draw_vars.geometry_id = Some(geometry_id);
            draws.alpha.cube.cube.many_instances =
                cx.begin_many_instances(&draws.alpha.cube.cube.draw_vars);
            if has_static {
                if let Some(mi) = &mut draws.alpha.cube.cube.many_instances {
                    for (chunk, visible) in self.static_chunks.iter().zip(&self.chunk_visible) {
                        if *visible {
                            mi.instances.extend_from_slice(&chunk.slab_alpha[shape_index]);
                        }
                    }
                }
            }
            if has_shadows {
                // Tiered cast shadows: the nearest casters get a real
                // silhouette MESH (shadow_mesh.rs, accumulated below and
                // drawn as one geometry); everything else falls back to a
                // blob quad in this batch. So the budget buys fidelity, and
                // the whole shadow layer is at most two draw calls.
                for (e, ground, projected) in
                    Self::shadow_casters(world, camera_pos, shadow_budget)
                {
                    let half = vec3f(
                        e.half.x * e.scale.x,
                        e.half.y * e.scale.y,
                        e.half.z * e.scale.z,
                    );
                    if projected && shadow_mesh_enabled {
                        // Silhouette tier: hull of the caster's own points,
                        // draped over whatever it lands on.
                        let mut transform = Self::entity_rotation(e);
                        transform.v[12] = e.pos.x;
                        transform.v[13] = e.pos.y;
                        transform.v[14] = e.pos.z;
                        caster_points(
                            e.shape,
                            &transform,
                            vec3f(half.x * 2.0, half.y * 2.0, half.z * 2.0),
                            &mut self.shadow_points,
                        );
                        let receiver = Receiver {
                            base_y: ground,
                            terrain: world.terrain.as_deref(),
                            statics: &self.receiver_boxes,
                        };
                        if crate::shadow_mesh::build_caster_shadow(
                            &self.shadow_points,
                            &sun,
                            &receiver,
                            &mut self.shadow_mesh,
                        ) {
                            stats.shadows += 1;
                            stats.projected_shadows += 1;
                        }
                        continue;
                    }
                    // Blob tier: the soft round contact blob in the shadow
                    // mesh. The box-batch quad below is a hard dark SQUARE,
                    // so every caster past the budget (or all of them once
                    // the thermometer drops projected shadows) cast a cube —
                    // a marble included. The quad stays only for a host
                    // with no shadow-mesh draw.
                    if draws.shadow.is_some() {
                        let receiver = Receiver {
                            base_y: ground,
                            terrain: world.terrain.as_deref(),
                            statics: &self.receiver_boxes,
                        };
                        if crate::shadow_mesh::build_blob_shadow(
                            vec3f(e.pos.x, e.pos.y - half.y, e.pos.z),
                            half.x * 1.1,
                            half.z * 1.1,
                            &sun,
                            &receiver,
                            &mut self.shadow_mesh,
                        ) {
                            stats.shadows += 1;
                        }
                        continue;
                    }
                    let quad = crate::shadow::blob_shadow(e.pos, half, ground, &sun);
                    let Some(quad) = quad else { continue };
                    // No fog on a shadow. It lies ON ground that is already
                    // fogged, so fogging it again mixes its RGB toward the
                    // bright horizon colour and a distant shadow comes out
                    // LIGHTER than the surface it darkens.
                    draws.alpha.cube.fog_density = 0.0;
                    draws.alpha.cube.cube.transform = quad.transform();
                    draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.alpha.cube.cube.cube_size = quad.size();
                    // The pipeline blends PREMULTIPLIED (src*1 + dst*(1-a)),
                    // so a shadow must be premultiplied black: RGB 0 leaves
                    // exactly ground*(1-a) — a true multiplicative shadow.
                    // Unpremultiplied dark RGB adds light instead of removing
                    // it, which is why the old blob shadows read as pale.
                    draws.alpha.cube.cube.color = vec4(0.0, 0.0, 0.0, quad.alpha);
                    draws.alpha.cube.cube.depth_clip = 1.0;
                    draws.alpha.cube.glow = 0.0;
                    draws.alpha.cube.color_adjust_ctl = vec4(0.0, 1.0, 1.0, 0.0);
                    draws.alpha.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                    stats.shadows += 1;
                }
                draws.alpha.cube.fog_density = fog_density;
            }
            for e in world
                .entities
                .iter()
                .filter(|e| e.alpha_primitive && !e.hidden && e.kind != BodyKind::Static && e.shape == shape)
            {
                if let Some(frustum) = frustum {
                    let r = vec3f(
                        e.half.x * e.scale.x,
                        e.half.y * e.scale.y,
                        e.half.z * e.scale.z,
                    )
                    .length();
                    if !frustum.intersects_sphere(e.pos, r) {
                        stats.dyn_culled += 1;
                        continue;
                    }
                }
                let mut transform = Self::entity_rotation(e);
                transform.v[12] = e.pos.x;
                transform.v[13] = e.pos.y;
                transform.v[14] = e.pos.z;
                draws.alpha.cube.cube.transform = transform;
                draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.alpha.cube.cube.cube_size = vec3(
                    e.half.x * 2.0 * e.scale.x,
                    e.half.y * 2.0 * e.scale.y,
                    e.half.z * 2.0 * e.scale.z,
                );
                let mut color = e.color;
                if color.w >= 0.99 {
                    // Sensors are see-through by default; explicit alpha wins.
                    color.w = 0.35;
                }
                draws.alpha.cube.cube.color = color;
                draws.alpha.cube.cube.depth_clip = 1.0;
                draws.alpha.cube.glow = e.glow;
                draws.alpha.cube.color_adjust_ctl = e.color_adjust.instance();
                draws.alpha.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            // Dynamic entities, parts and beams with fractional alpha, held
            // back from the opaque batch above.
            for deferred in deferred_alpha[shape_index].drain(..) {
                draws.alpha.cube.cube.transform = deferred.transform;
                draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.alpha.cube.cube.cube_size = deferred.size;
                draws.alpha.cube.cube.color = deferred.color;
                draws.alpha.cube.cube.depth_clip = 1.0;
                draws.alpha.cube.glow = deferred.glow;
                draws.alpha.cube.color_adjust_ctl = deferred.color_adjust_ctl;
                draws.alpha.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            if let Some(mi) = draws.alpha.cube.cube.many_instances.take() {
                cx.end_many_instances(mi);
            }
        }

        let v = &scene_state.view.v;
        let cam_axes = (vec3f(v[0], v[4], v[8]), vec3f(v[1], v[5], v[9]));
        self.draw_late_layers(cx, draws, world, frustum, camera_pos, shows_environment, &mut stats, (sun, (fog_color, fog_density), cam_axes));
        // Particles and VFX decals, after every other transparent. Particles
        // are pure decoration and the first thing thermal pressure thins
        // (quality.particle_scale): each burst keeps a prefix, so an effect
        // gets sparser instead of going missing.
        let vfx_light = crate::vfx::VfxLight {
            dir: sun.dir,
            sun: sun.color,
            sky: sun.sky,
            fog: fog_color,
            fog_density,
            lin: self.lin_ctl(),
        };
        self.draw_vfx_layer(cx, &scene_state, stage_matrix, frustum, vfx_light, quality.particle_scale, &mut stats);

        // 7. View-local held meshes, after the complete world. The dedicated
        // shader maps them into a portable near-depth band, while their queue
        // never visited any world bake/caster path above.
        if let Some(draw) = draws.view_model.as_deref_mut() {
            self.draw_view_models_inner(cx, draw, &sun, fog_color, &mut stats);
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        draw_list.end(cx);
        stats
    }
}
