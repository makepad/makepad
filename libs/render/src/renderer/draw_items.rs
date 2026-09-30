//! Per-frame draw payloads: GPU-skinned characters, SDF shadow quads, layer materials.

use super::*;

/// One GPU-skinned character instance for [`Renderer::draw_scene_full`].
/// The per-frame payload is the joint `palette`; the rig's rest mesh is
/// resident on the GPU ([`Renderer::upload_skin_rig`]).
#[derive(Clone)]
pub struct SkinnedDraw {
    /// Stable per-character id — the rotational-shadow keyframe cache key.
    pub key: u64,
    /// Which resident rest mesh this character wears. Characters of one rig
    /// share the geometry and batch into one draw item.
    pub rig: u64,
    pub transform: Mat4f,
    /// Per-character wash over the model's own colours, so one rig can furnish
    /// a village without every villager being the same figure.
    pub tint: Vec4f,
    /// Hue degrees, saturation multiplier, value multiplier, reserved.
    pub color_adjust: Vec4f,
    /// Index into [`SkinnedBatch::textures`]. Characters from different packs
    /// carry different atlases, and binding one character's atlas to another
    /// does not fail — it renders, wrongly, looking like a shading bug.
    pub texture: usize,
    /// This frame's joint palette ([`crate::skin::SkinnedModel::palette`]).
    pub palette: Vec<Mat4f>,
    /// Posed model-space bounds for frustum culling
    /// ([`crate::skin::SkinnedModel::posed_bounds`]) — no posed vertices
    /// exist on the CPU to measure any more.
    pub bounds: Option<(Vec3f, Vec3f)>,
    /// Walk-cycle phase, 0..1 through the clip — picks the pose-phase atlas
    /// row pair so the shadow breathes with the stride.
    pub gait_phase: f32,
    pub morph_clip:Option<String>,
    pub morph_time:f32,
    pub morph_looping:bool,
    /// 0 = idle stance, 1 = full walk; mixes the idle row toward the
    /// phase rows exactly the way the pose blend does.
    pub gait_blend: f32,
    /// Where this rig's offline `.shadowsdf` sidecar lives: the rig's
    /// source GLB path plus its [`crate::skin::SkinnedModel::rest_hash`]
    /// (the `.skinao` keying scheme). The ONLY source of a rig's SDF
    /// shadow atlas — tools/ao_bake writes `<glb>.shadowsdf`, the runtime
    /// loads it or the rig's characters blob.
    pub sdf_sidecar: Option<(String, u64)>,
}

impl SkinnedDraw {
    /// Untinted: the model's own colours, unchanged.
    pub fn new(key: u64, rig: u64, transform: Mat4f) -> Self {
        Self {
            key,
            rig,
            transform,
            tint: vec4(1.0, 1.0, 1.0, 1.0),
            color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
            texture: 0,
            palette: Vec::new(),
            bounds: None,
            gait_phase: 0.0,
            morph_clip:None,morph_time:0.0,morph_looping:true,
            gait_blend: 0.0,
            sdf_sidecar: None,
        }
    }

    pub fn with_morph_time(mut self,clip:Option<String>,time:f32,looping:bool)->Self{self.morph_clip=clip;self.morph_time=time;self.morph_looping=looping;self}
    pub fn with_tint(mut self, tint: Vec4f) -> Self {
        self.tint = tint;
        self
    }

    pub fn with_color_adjust(mut self, color_adjust: Vec4f) -> Self {
        self.color_adjust = color_adjust;
        self
    }

    /// Which atlas this character samples.
    pub fn with_texture(mut self, texture: usize) -> Self {
        self.texture = texture;
        self
    }

    pub fn with_palette(mut self, palette: Vec<Mat4f>) -> Self {
        self.palette = palette;
        self
    }

    pub fn with_bounds(mut self, bounds: Option<(Vec3f, Vec3f)>) -> Self {
        self.bounds = bounds;
        self
    }

    /// Where the rig's offline `.shadowsdf` sidecar lives (see
    /// [`Self::sdf_sidecar`]): the source GLB path — the sidecar is
    /// `<glb>.shadowsdf` beside it — and the rig's rest hash to key it.
    pub fn with_sdf_sidecar(mut self, glb_path: &str, source_hash: u64) -> Self {
        self.sdf_sidecar = Some((glb_path.to_string(), source_hash));
        self
    }

    /// This frame's walk-cycle phase (0..1) and idle-to-walk blend, driving
    /// the pose-phase shadow atlas rows.
    pub fn with_gait(mut self, phase: f32, blend: f32) -> Self {
        self.gait_phase = phase;
        self.gait_blend = blend;
        self
    }
}

/// The skinned characters for one frame, drawn between the opaque and alpha
/// passes (so blob shadows and sensor ghosts blend over them correctly).
pub struct SkinnedBatch<'a> {
    pub skinned: &'a mut DrawSceneSkinnedGpu,
    /// One atlas per distinct character pack; items index into it. A village
    /// of Kenney civilians shares a single entry (one `colormap.png` per
    /// pack), so the common case stays one texture and one bind.
    pub textures: Vec<&'a Texture>,
    pub items: Vec<SkinnedDraw>,
}

/// One caster's SDF-silhouette shadow record, mirroring
/// [`crate::shaders::DrawSceneShadowSdf`]'s instance fields. `atlas` picks
/// the texture at draw: a rig id for characters, a model index for cars —
/// both mapped through [`SdfAtlasKey`].
pub(super) struct SdfInstance {
    pub(super) atlas: SdfAtlasKey,
    pub(super) a: Vec4f,
    pub(super) b: Vec4f,
    pub(super) c: Vec4f,
    pub(super) d: Vec4f,
    pub(super) e: Vec4f,
}

/// Which uploaded atlas an [`SdfInstance`] draws with. Ord so the draw loop
/// can sort the frame's instances and share one draw item per atlas.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SdfAtlasKey {
    Rig(u64),
    Model(String),
}

/// What the runtime needs to address a rig's uploaded SDF atlas: the shared
/// window every cell maps (shadow_sdf.rs `rect`), the pose rows, and the
/// encode band for converting edge softness into encoded-d units.
#[derive(Clone, Copy)]
pub(super) struct SdfMeta {
    pub(super) rect: Vec4f,
    pub(super) rows: usize,
    pub(super) band_world: f32,
    pub(super) len_per_unit: f32,
}

/// SDF shadow edge softness, sprite units: the base half-width at the feet
/// and the extra per unit of source height (contact hardening — the tip of
/// a shadow is cast by the head and blurs wider than the feet's contact).
pub(super) const SDF_SOFT_BASE: f32 = 0.05;
pub(super) const SDF_SOFT_HARDEN: f32 = 0.10;

/// Car-tilt gate for the SDF quad: the atlas bakes the car FLAT, and the
/// quad follows the local ground plane, which reads fine up to moderate
/// tilt. Beyond this cosine of body-up vs world-up (~37 degrees) the flat
/// sprite lies — a rolled or launched car falls back to the plain blob.
pub(super) const SDF_CAR_TILT_MIN_UP: f32 = 0.8;

/// Airborne gate: above this many units of clear air under the wheels the
/// flat silhouette stops being the car's shadow (mid-jump, off a ramp
/// lip) — blob until it lands. Characters keep their sprite when jumping
/// (limbs read even in the air); a car's roof-line does not.
pub(super) const SDF_CAR_AIR_MAX: f32 = 1.5;

/// May a car draw its flat SDF sprite, or must it fall back to the blob?
/// `up_y` = cosine of body-up against world-up (the transform's normalised
/// y basis), `air` = clear units between the wheels and the receiver.
pub(super) fn car_sprite_allowed(up_y: f32, air: f32) -> bool {
    up_y >= SDF_CAR_TILT_MIN_UP && air <= SDF_CAR_AIR_MAX
}

/// A character shadow's resolved landing (see
/// [`Renderer::character_shadow_anchor`]).
pub(super) struct ShadowAnchor {
    /// FOOT-END landing point, y at the receiver surface: the caster's
    /// ground contact, moved only by the HEIGHT-driven part of the offset
    /// policy (a jump slides the whole shadow off the feet — that is the
    /// point of a jump shadow). A lamp lean NEVER moves this end: a shadow
    /// roots at the feet and its body sweeps away from the light, so a
    /// grounded caster's root is always their own contact point. The
    /// silhouette quad pins its window origin (the bake's ground anchor)
    /// here.
    pub(super) root: Vec3f,
    /// The owning light's full lean, world xz: the direction the
    /// silhouette POINTS (away from the light), magnitude the light's
    /// mid-body projection. Supplies the quad's long-axis direction and
    /// nothing else — leaning translates the BODY of the shadow, not its
    /// contact.
    pub(super) lean: Vec2f,
    /// Normal + slope bias to clear the receiver.
    pub(super) lift: f32,
    /// Final shadow alpha: drop fade × height fade × lamp boost.
    pub(super) alpha: f32,
    /// Sprite scale multiplier (swells with height).
    pub(super) size_mul: f32,
    /// 0..1 — how much the dominant lamp owns this shadow.
    pub(super) lamp_w: f32,
}

/// The whole character-shadow anchor policy as a free function, so the GPU
/// fan, the CPU fallback AND the unit tests all read the same numbers —
/// see [`Renderer::character_shadow_anchor`] for the policy prose.
pub(super) fn character_shadow_anchor(
    feet: Vec3f,
    receiver: &Receiver,
    sun: &SunLight,
    lights: &[crate::lightmap::LmLight],
) -> Option<ShadowAnchor> {
    let (g0, _) = receiver.sample(feet.x, feet.z);
    let h = (feet.y - g0).max(0.0);
    let sy = sun.dir.y.max(0.2);
    // Strongest lamp at the feet — same attenuation/cone math as the
    // shader term, so what lights the character is what owns its shadow.
    let sun_i = sun.color.x.max(sun.color.y).max(sun.color.z)
        * (sun.dir.y * 4.0).clamp(0.0, 1.0);
    let mut best: Option<(f32, &crate::lightmap::LmLight)> = None;
    for l in lights {
        if l.radius <= 0.0 {
            continue;
        }
        let (dx, dy, dz) = (feet.x - l.pos.x, feet.y - l.pos.y, feet.z - l.pos.z);
        let d2 = dx * dx + dy * dy + dz * dz;
        if d2 >= l.radius * l.radius {
            continue;
        }
        let d = d2.sqrt().max(1.0e-4);
        let att = 1.0 - d / l.radius;
        let cone = (((l.pos.y - feet.y) / d + 0.35) / 1.35).clamp(0.0, 1.0);
        let s = light_intensity(l) * att * att * (cone * cone * l.spot + (1.0 - l.spot));
        if best.as_ref().is_none_or(|(bs, _)| s > *bs) {
            best = Some((s, l));
        }
    }
    let mut lamp_w = 0.0;
    if let Some((lamp_i, _)) = best {
        let ratio = (lamp_i * 2.0) / (lamp_i * 2.0 + sun_i + 1.0e-4);
        lamp_w = ((ratio - 0.35) / 0.4).clamp(0.0, 1.0);
    }
    // The lean policy, as a function of caster height `hh`: sun projection
    // blended toward the dominant lamp's true mid-body projection.
    let lean_at = |hh: f32| -> Vec2f {
        let mut off = vec2f(-sun.dir.x / sy * hh, -sun.dir.z / sy * hh);
        if lamp_w > 0.0 {
            let (_, l) = best.expect("lamp_w > 0 implies a best lamp");
            let (hx, hz) = (feet.x - l.pos.x, feet.z - l.pos.z);
            let rho = (hx * hx + hz * hz).sqrt();
            // The LEAN saturates faster than the dominance itself: sqrt
            // eases the blend toward the lamp's TRUE projection while a
            // meaningful share of the light is the lamp's. Linear was
            // measured too timid in the day village — dominance ~0.7 beside
            // a lamp halved a ~0.7-unit true offset into a sub-half-unit
            // nudge that read as nothing. Darkening keeps the LINEAR weight:
            // alpha is about energy, the lean is about where the silhouette
            // POINTS, and only the second one was invisible.
            let off_w = lamp_w.sqrt();
            if rho > 0.15 {
                // Shadow of a mid-body occluder thrown by the bulb.
                const MID: f32 = 0.9;
                let denom = (l.pos.y - g0 - MID).max(0.5);
                let m = (rho * (MID + hh) / denom).min(2.5);
                let (lx, lz) = (hx / rho * m, hz / rho * m);
                off = vec2f(off.x + (lx - off.x) * off_w, off.y + (lz - off.y) * off_w);
            } else {
                // Dead under the bulb: no direction — it only darkens.
                off = vec2f(off.x * (1.0 - off_w), off.y * (1.0 - off_w));
            }
        }
        off
    };
    let lean = lean_at(h);
    // The ROOT moves only with the height-driven part of the lean: the
    // full lean minus what the policy would say for the same caster
    // standing on the ground. Grounded (h = 0) the two cancel exactly and
    // the root IS the contact point — a lamp lean redirects the body of
    // the shadow but never detaches it from the boots (the floating-
    // sideways-silhouette report this parameterisation replaces). Airborne
    // the difference is the true projection growth with height, sun and
    // lamp alike, so a jump still slides the whole shadow.
    let ground_lean = lean_at(0.0);
    let root_x = feet.x + lean.x - ground_lean.x;
    let root_z = feet.z + lean.y - ground_lean.y;
    let probe = vec3f(root_x, feet.y, root_z);
    let (ground, normal, alpha, _) = shadow_drop_params(probe, receiver, sun)?;
    let slope = (1.0 - normal.y.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    Some(ShadowAnchor {
        root: vec3f(root_x, ground, root_z),
        lean,
        lift: SHADOW_NORMAL_BIAS + SHADOW_SLOPE_BIAS * slope,
        alpha: (alpha * (1.0 + 0.7 * lamp_w)).min(0.6)
            * (1.0 - h / 4.0).clamp(0.0, 1.0),
        // Height swells the sprite (occluder distance); a dominant lamp
        // COMPRESSES it toward the root instead. The silhouette is baked
        // against the SUN, so under a bulb it still carries the sun's
        // elongation — at dusk a whole body-length of it — and a near-
        // vertical light must pin the shadow small and dark at the feet.
        // The quad scales about its window origin — the pinned foot end —
        // (the SDF quad's sdf_b.z is this same number), so scaling shortens
        // the FAR end toward the feet, which is exactly "compressed toward
        // the feet". 0.4x at full dominance.
        size_mul: (1.0 + 0.25 * h) * (1.0 - 0.6 * lamp_w),
        lamp_w,
    })
}

/// The plane an SDF shadow quad should lie on: the anchor's own landing
/// height, RAISED to the highest receiver surface under the silhouette's
/// down-sun run. The quad is one flat plane; when the silhouette crosses
/// onto a HIGHER receiver top — a road slab 8 cm proud of the grass its
/// caster stands on — a plane anchored at the caster's ground sits UNDER
/// that top and the whole silhouette depth-buries: the shadow vanishes
/// entirely at exactly the headings that lay it across the slab (and
/// flickers with camera angle at the slab's silhouette edge). A shadow
/// floating a few centimetres above the lower surface is invisible; a
/// buried one is a missing shadow, so raising is always the right trade.
/// `(gx, gz)` is the unit axis TOWARD the light, `len` the silhouette's
/// world-unit reach away from it (the atlas window's down-sun extent x
/// the sprite scale).
pub(super) fn sdf_quad_ground(anchor: Vec3f, receiver: &Receiver, gx: f32, gz: f32, len: f32) -> f32 {
    let mut y = anchor.y;
    for f in [0.5f32, 1.0] {
        let (sy, _) = receiver.sample(anchor.x - gx * len * f, anchor.z - gz * len * f);
        if sy > y {
            y = sy;
        }
    }
    y
}

/// One draw layer's metallic-roughness material, resident.
///
/// Neutral (`metallic 0, roughness 1, orm_on false`) is exactly the shading
/// the diffuse lane gives, which is what a layer gets when its own material
/// carries no shininess — including inside a model that IS on the PBR lane
/// because some OTHER layer is shiny. That matters: the glTF default is
/// `metallic 1`, and a layer taken literally at that would lose its whole
/// diffuse lobe and render black.
#[derive(Clone)]
pub(super) struct LayerMaterial {
    pub(super) surface: Option<crate::material_surface::UploadedSurface>,
    pub(super) metallic: f32,
    pub(super) roughness: f32,
    /// The metallicRoughness map, or a white 1x1 when the material is
    /// factors-only (`orm_on` false folds it out of both products anyway).
    pub(super) orm: Texture,
    pub(super) orm_on: bool,
    /// Magnified texels draw nearest (the glTF sampler's `magFilter`).
    pub(super) mag_nearest: bool,
    /// The layer's base texture can fail the alpha test, so the layer
    /// needs the shader that can discard (opaque.rs).
    pub(super) cutout: bool,
}

/// Which shader a model pass drives.
///
/// The two lanes share every line of [`Renderer::draw_models_inner`]:
/// [`DrawScenePbr`] derefs [`DrawSceneSkinned`], so all the per-instance
/// state — transform, lightmap window, dynamic-light gate, AO binding — is
/// written through one type, and the lanes differ only in the material the
/// PBR shader additionally reads. A model picks its lane at LOAD time
/// (`LoadedModel::wants_pbr`), so a frame is at most two passes over the
/// instance list regardless of how the scene is mixed.
pub(super) enum ModelDraw<'a> {
    Diffuse(&'a mut DrawSceneSkinned),
    Pbr(&'a mut DrawScenePbr),
    Custom(&'a str, &'a mut crate::custom_material::CustomMaterial),
    /// The streamed world's surfaces (stream_draw.rs).
    City(&'a mut crate::shaders::DrawSceneCity),
    /// Grass blades (grass_draw.rs): the lane binding only.
    Grass(&'a mut crate::shaders::DrawSceneGrass),
    /// Swaying trees and bushes (shaders/foliage.rs).
    Foliage(&'a mut crate::shaders::DrawSceneFoliageLit),
}

impl ModelDraw<'_> {
    pub(super) fn base(&mut self) -> &mut DrawSceneSkinned {
        match self {
            ModelDraw::Diffuse(d) => d,
            ModelDraw::Pbr(d) => &mut d.skinned,
            ModelDraw::Custom(_, d) => &mut d.draw.pbr.skinned,
            ModelDraw::City(d) => &mut d.pbr.skinned,
            ModelDraw::Grass(d) => &mut d.pbr.skinned,
            ModelDraw::Foliage(d) => &mut d.pbr.skinned,
        }
    }

    /// The PBR shader and its game.material variant: both bind the eye and
    /// each layer's metallic-roughness.
    pub(super) fn is_pbr(&self) -> bool {
        matches!(self, ModelDraw::Pbr(_) | ModelDraw::Custom(..) | ModelDraw::City(_) | ModelDraw::Grass(_) | ModelDraw::Foliage(_))
    }

    fn pbr(&mut self) -> Option<&mut DrawScenePbr> {
        match self {
            ModelDraw::Diffuse(_) => None,
            ModelDraw::Pbr(d) => Some(d),
            ModelDraw::Custom(_, d) => Some(&mut d.draw.pbr),
            ModelDraw::City(d) => Some(&mut d.pbr),
            ModelDraw::Grass(d) => Some(&mut d.pbr),
            ModelDraw::Foliage(d) => Some(&mut d.pbr),
        }
    }

    /// Bind one layer's metallic-roughness. A no-op on the diffuse lane,
    /// which has no such lanes to bind — that is the whole reason the two
    /// shaders are siblings.
    pub(super) fn set_material(&mut self, cx: &Cx, m: &LayerMaterial) {
        self.base().fur = Default::default();
        self.base().fur_layer.x = 0.0;
        // Wind rides the morph lanes (morph_ctl.w = -1, draw_models sets it
        // after this call for foliage layers); every other draw sways nothing.
        if self.base().morph_ctl.w < 0.0 { self.base().morph_ctl = Vec4f::default(); }
        self.base().tex_mag = vec2f(if m.mag_nearest { 1.0 } else { 0.0 }, 0.0);
        if let Some(d) = self.pbr() {
            d.metallic = m.metallic;
            d.roughness = m.roughness;
            d.orm_on = if m.orm_on { 1.0 } else { 0.0 };
            d.surface_on=if m.surface.is_some(){1.0}else{0.0};
            d.material_alpha=1.0;d.alpha_mode=0.0;d.alpha_cutoff=0.5;d.normal_scale=0.0;d.occlusion_strength=0.0;d.emissive=vec3f(0.0,0.0,0.0);d.double_sided=0.0;d.triplanar=0.0;
            d.skinned.draw_vars.options.alpha_blend=false;d.skinned.draw_vars.options.depth_write=true;d.skinned.draw_vars.options.backface_culling=true;
            if let Some(surface)=&m.surface {
                let definition=&surface.definition;
                d.skinned.fur = crate::material_surface::fur_params(definition.fur);
                d.material_alpha=definition.base_alpha;d.alpha_mode=definition.alpha_mode as f32;d.alpha_cutoff=definition.alpha_cutoff;
                d.triplanar=definition.triplanar;
                // Race paint in the spare tex_mag lane (shaders.rs DrawSceneSkinned).
                d.skinned.tex_mag.y=definition.packed_shading();
                // A number plate: tex_mag.x = 2 selects the per-copy digit remap.
                if definition.plate { d.skinned.tex_mag.x = 2.0; }d.normal_scale=definition.normal_scale;d.occlusion_strength=definition.occlusion_strength;d.emissive=vec3f(definition.emissive[0],definition.emissive[1],definition.emissive[2]);d.double_sided=if definition.double_sided{1.0}else{0.0};
                d.skinned.draw_vars.options.alpha_blend=definition.alpha_mode==2;d.skinned.draw_vars.options.depth_write=definition.alpha_mode!=2;d.skinned.draw_vars.options.backface_culling=!definition.double_sided;
            }
            if let Some(id)=d.skinned.draw_vars.draw_shader_id {
                for (name,texture) in [(live_id!(normal_map),m.surface.as_ref().map(|s|&s.normal)),(live_id!(occlusion_map),m.surface.as_ref().map(|s|&s.occlusion)),(live_id!(emissive_map),m.surface.as_ref().map(|s|&s.emissive))] {
                    if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==name).filter(|slot|*slot<d.skinned.draw_vars.texture_slots.len()){d.skinned.draw_vars.set_texture(slot,texture.unwrap_or(&m.orm));}
                }
            }

            // Derived materials follow the shared lighting texture set.
            // Resolve their own binding by name, not a fragile slot number.
            if let Some(id) = d.skinned.draw_vars.draw_shader_id {
                // A shader past the draw-call slot budget degrades (the map
                // stays unbound) instead of panicking the whole scene draw.
                if let Some(slot) = cx.draw_shaders[id.index].mapping.textures.iter().position(|t| t.id == live_id!(orm_map)).filter(|slot| *slot < d.skinned.draw_vars.texture_slots.len()) {
                    d.skinned.draw_vars.set_texture(slot, &m.orm);
                }
            }
        }
    }

    pub(super) fn submit(&mut self, cx: &mut Cx3d, distance: f32, fur_budget: &mut usize) -> usize {
        self.submit_as(cx, distance, fur_budget, None)
    }

    /// `submit`, with the base layer drawn through `opaque` (the lane's
    /// no-discard variant, opaque.rs) when the caller knows it cuts no pixel.
    /// Fur shells always keep the stock shader: they discard by design.
    pub(super) fn submit_as(&mut self, cx: &mut Cx3d, distance: f32, fur_budget: &mut usize, opaque: Option<DrawShaderId>) -> usize {
        let draw = self.base();
        if !draw.draw_vars.can_instance() { return 0; }
        let triangles = draw.draw_vars.geometry_id.map_or(0, |id| cx.cx.geometries[id].indices.len() / 3);
        let shells = crate::material_surface::fur_shell_count(draw.fur.x, &draw.transform, distance, triangles, fur_budget);
        let stock = draw.draw_vars.draw_shader_id;
        for layer in 0..=shells {
            draw.fur_layer.x = layer as f32 / shells.max(1) as f32;
            draw.draw_vars.draw_shader_id = if layer == 0 { opaque.or(stock) } else { stock };
            let area = cx.add_instance(&draw.draw_vars);
            draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, area);
        }
        draw.draw_vars.draw_shader_id = stock;
        draw.fur_layer.x = 0.0;
        shells * triangles
    }
}
