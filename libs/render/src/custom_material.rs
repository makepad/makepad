//! Opt-in procedural model materials. Stock models pay no additional shader cost.
use makepad_draw::*;
use crate::shaders::DrawSceneSkinned;

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneCustom {
    #[deref] pub skinned: DrawSceneSkinned,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))] pub params: Vec4f,
}

// The engine owns vertex, lighting and fog. Only the albedo surface hook is
// taken from a game's spec; arbitrary pixel/vertex overrides are not applied.
script_mod! {
    use mod.prelude.widgets_internal.*
    mod.draw.DrawSceneCustom = mod.std.set_type_default() do #(DrawSceneCustom::script_shader(vm)) {
        ..mod.draw.DrawSceneSkinned
        surface: fn(base: vec4) -> vec4 { return base }
        pixel: fn() {
            // Atlas x vertex tint. Kenney ships both conventions — most packs
            // UV-map into one colormap (tint = white), nature-kit and friends
            // carry no texture and colour per material (atlas = white 1x1).
            // Multiplying serves both without a branch or a second shader.
            // REPEAT + raw UVs (not fract): fract() wraps in software but
            // explodes screen-space derivatives at every tile seam, so the
            // GPU picks the tiniest mip and distant walls turn to noise.
            let tex = self.tex.sample_as_bgra_repeat(self.v_uv)
            // BUILD punch-through: palette 255 / magenta is the overlay key.
            let magenta = (tex.x > 0.75) && (tex.z > 0.75) && (tex.y < 0.22)
            if tex.w < 0.5 || magenta {
                discard()
            }
            let base = self.to_scene(vec3(tex.x, tex.y, tex.z))
            var albedo = self.color_adjust(
                vec3(base.x * self.v_tint.x, base.y * self.v_tint.y, base.z * self.v_tint.z),
                self.tint,
                self.color_adjust_ctl
            )
            // Detail: blendFunc GL_DST_COLOR GL_SRC_COLOR = 2 * dest * src.
            // Mean-127 overlay is identity; far mips go gray and drop out.
            if self.detail_st.x > 0.001 {
                let det = self.detail_map.sample_as_bgra_repeat(self.v_uv * self.detail_st)
                albedo = vec3(albedo.x * det.x * 2.0, albedo.y * det.y * 2.0, albedo.z * det.z * 2.0)
            }
            let surface = self.surface(vec4(albedo, tex.w))
            if surface.w < 0.5 { discard() }
            albedo = surface.xyz
            // AO scales AMBIENT only. Ambient is light arriving from
            // everywhere, which is exactly what a crevice blocks; direct
            // sunlight is already zero where the surface faces away. Folding
            // it into both would darken a lit wall twice for the same reason.
            // Occlusion from the ATLAS when the pack has one, else from the
            // vertex lane. Both live in [AO_FLOOR, 1].
            let baked = self.ao_map.sample(self.v_ao_uv).x
            // Dithered: the atlas is 8-bit and magnified well past a texel per
            // pixel, so a shallow wall gradient otherwise lands as visible
            // bands of piecewise-linear bilinear. Hash noise anchored in WORLD
            // space (screen-anchored grain swims when the camera moves) at
            // ±1.5% breaks the bands without reading as dirt on flat colour.
            let hash = fract(
                sin(dot(self.world.xy + self.world.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            let ao = clamp(
                mix(self.v_tint.w, baked, self.ao_enabled) + (hash - 0.5) * 0.03,
                0.0, 1.0
            )
            // AO scales ambient FULLY and direct partially. Ambient-only is
            // the physically tidy answer and it is why the bake was invisible:
            // ambient is about a quarter of the light here, so even a properly
            // dark corner moved the pixel by a few percent. Letting occlusion
            // take some of the direct term too is what every stylised renderer
            // does, and it is what makes a crease read as a crease.
            let ao_direct = mix(1.0, ao, 0.75)
            // Screen-space AO (ssao.rs), composed with the baked term on the
            // AMBIENT fill only — same law as `ao` above, stricter split:
            // a crevice blocks sky light, not the sun, so the direct and
            // shadow-mapped terms never see this factor at all.
            var sao = 1.0
            if self.ssao_ctl.x > 0.001 {
                let sp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                let suv = vec2(sp.x * 0.5 + 0.5, 0.5 - sp.y * 0.5)
                sao = 1.0 - (1.0 - self.ssao_map.sample_nearest(suv).x) * self.ssao_ctl.x
            }
            // Baked light: A gates the analytic sun through a smoothstep over
            // the signed-distance field — the penumbra width is the decode
            // WINDOW ([`LM_SUN_SOFT`]), a runtime knob, not a bake product.
            // RGB adds the lamps (x2: half range stored for overbright).
            let lm = self.light_map.sample_as_bgra(self.v_lm_uv)
            let has_lm = step(0.000001, self.lm_rect.z)
            let sun_vis = mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm)
            // Dynamics gate their sun through the GROUND region instead
            // (statics have v_lmg.z = 0, dynamics have lm_rect = 0, so the
            // two gates never both engage). The shadow-top plane rejects
            // the ground's shadow for vertices ABOVE the blocker along the
            // sun ray: a fence rail shades shins, never the head over it.
            let lmg = self.light_map.sample_as_bgra(self.v_lmg.xy)
            let top_g = self.lm_top_decode.x
                + self.top_map.sample(self.v_lmg.xy).x * self.lm_top_decode.y
            let occ_g = 1.0 - smoothstep(top_g - 0.15, top_g + 0.15, self.v_lmg.w)
            let sun_vis_g = mix(1.0, smoothstep(0.2, 0.8, lmg.w), self.v_lmg.z * occ_g)
            // Realtime: the cascades replace BOTH baked gates (own chart
            // and ground projection) — one receive path for every family.
            let sun_all = mix(
                sun_vis * sun_vis_g,
                self.csm_vis(self.v_csm.xyz, self.v_csm_n, self.v_csm.w),
                self.csm_p.x
            )
            // 0.9 = lightmap::LM_LAMP_CEIL — the atlas RGB decode.
            let lamps = lm.xyz * (0.9 * has_lm)
            // Local light — baked pools plus the per-frame slots — reaches
            // this fragment WITHOUT the sun's shadow term, because a shadow
            // is the absence of SUN and of nothing else. Over its bright core
            // a pool additionally fills that shadow back in, so a lamp drowns
            // out the streak its own pole throws across its own pool:
            // lightmap::lamp_shadow_fill.
            let local = lamps + self.v_dl
            let sun_lit = self.sun_filled(sun_all, local)
            let analytic = self.v_ambient * (ao * sao)
                + self.v_direct * (ao_direct * sun_lit)
                + local * ao_direct
            // prelit: albedo already carries COLOR_0 = LM×4. Multiplying
            // the sun again zeros any face that looks inward or down.
            let lit = albedo * mix(analytic, vec3(1.0, 1.0, 1.0), self.prelit)
            // AO DEBUG: show baked occlusion alone, contrast-stretched. AO
            // lives in [AO_FLOOR, 1] (0.52..1), so raw it is a wash of pale
            // greys and judging whether a 90-degree corner actually darkens is
            // guesswork. Remapped to full black-to-white, a corner that works
            // is unmistakable and one that does not is equally so.
            if self.ao_debug > 0.5 {
                // HARD PINK, heavily accentuated. Greyscale AO on grey-white
                // Kenney walls is unreadable — the whole reason the last three
                // bakes looked "a bit smudgy" is that a real defect and a
                // correct result differ by a few percent of luminance. Hue
                // separates occlusion from albedo completely, and the cube
                // curve pushes even slight darkening to saturation, so where
                // AO does anything at all it is obvious.
                // LINEAR over AO's actual range. An earlier version cubed this
                // to make faint occlusion obvious and that made the view lie:
                // a barely-shaded wall at ao=0.9 came out 37% pink, so the
                // whole house read as heavily occluded while the atlas was in
                // fact 74% unoccluded. A debug view that exaggerates is worse
                // than none — it hides the very problem it is there to show.
                let occ = clamp((1.0 - ao) / 0.70, 0.0, 1.0)
                return vec4(mix(vec3(1.0, 1.0, 1.0), vec3(1.0, 0.0, 0.55), occ), 1.0)
            }
            // LM DEBUG: the ACTIVE sun tier alone — red in shadow, green in
            // sun, lamps added as their own colour on top. Albedo suppressed
            // so a faint lamp or a misplaced shadow reads instantly.
            if self.lm_debug > 0.5 {
                return vec4(
                    mix(vec3(0.6, 0.1, 0.1), vec3(0.1, 0.6, 0.1), sun_lit) + lamps,
                    1.0
                )
            }
            return vec4(mix(self.to_display(lit), self.fog_color, self.v_fog), 1.0)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}

/// Install shader declarations in the owning isolate. Registration grants
/// no IO or host-widget access. GameMaterials only copies its allowlisted hook.
pub fn register(vm: &mut ScriptVm) -> ScriptValue {
    if vm.bx.heap.type_default_for_id(DrawSceneCustom::script_type_id_static()).is_some() {
        return NIL;
    }
    if vm.bx.heap.type_default_for_id(DrawSceneSkinned::script_type_id_static()).is_none() {
        crate::shaders::script_mod(vm);
    }
    script_mod(vm)
}

impl DrawSceneCustom {
    /// Construct a fresh allowlisted surface override. Source edits reuse a
    /// ScriptIp (body + opcode offset), while the draw function cache hashes
    /// only those locations. Invalidate this object's function-hash shortcut
    /// so a new body is frontend-checked; generated-code caching still safely
    /// shares identical shaders. Call only when declarations change, not for
    /// per-frame parameter updates.
    pub fn from_surface(vm: &mut ScriptVm, surface: ScriptObject, params: Vec4f) -> Result<Self, String> {
        if !vm.bx.heap.is_fn(surface) { return Err("surface is not a shader function".into()); }
        register(vm);
        let base = vm.bx.heap.type_default_for_id(Self::script_type_id_static())
            .ok_or_else(|| "custom material shader registration failed".to_string())?;
        let obj = vm.bx.heap.new_with_proto_no_vec(base.into());
        let result = vm.bx.heap.set_value(obj, id!(surface).into(), surface.into(), NoTrap);
        if !result.is_nil() || vm.bx.heap.object_method(obj, id!(surface).into(), NoTrap).as_object() != Some(surface) {
            return Err("custom material surface override was not installed".into());
        }
        let hash = DrawVars::compute_shader_functions_hash(&vm.bx.heap, obj);
        vm.host.cx_mut().draw_shaders.cache_functions_to_shader.remove(&hash);
        // #[deref] construction applies DrawSceneSkinned's defaults and can
        // already have compiled its stock shader. A failed override leaves
        // an existing ID untouched, so clear it before testing this candidate.
        let mut draw = Self::script_new(vm);
        draw.draw_vars.draw_shader_id = None;
        draw.draw_vars.geometry_id = None;
        draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
        draw.params = params;
        if !draw.draw_vars.can_instance() {
            return Err("surface shader failed frontend compilation; see shader diagnostic in app log".into());
        }
        Ok(draw)
    }
}
