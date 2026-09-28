//! Placed model instances, model-state targets and animation state.

use super::*;

/// Which placed geometry a model-state command addresses.
///
/// An imported level is ONE placed instance, so naming the model id is the
/// natural handle for its doors; a prop placed many times needs the slot.
/// `Instance` wins over `Model` when both are set for the same part.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModelTarget {
    /// Every placed copy of this model id.
    Model(String),
    /// One slot in the [`Renderer::set_models`] list.
    Instance(usize),
    Attachment(usize),
}

impl From<&str> for ModelTarget {
    fn from(id: &str) -> Self {
        ModelTarget::Model(id.to_string())
    }
}
impl From<&String> for ModelTarget {
    fn from(id: &String) -> Self {
        ModelTarget::Model(id.clone())
    }
}
impl From<String> for ModelTarget {
    fn from(id: String) -> Self {
        ModelTarget::Model(id)
    }
}
impl From<usize> for ModelTarget {
    fn from(slot: usize) -> Self {
        ModelTarget::Instance(slot)
    }
}

/// One triggered part's clock. `time` is where the part IS, `target` where it
/// is heading; `speed` is clip-seconds per real second, fixed when the
/// command landed so a mid-move reversal takes the same wall-clock time as
/// the move it interrupts.
#[derive(Clone, Copy)]
pub(super) struct AnimPartRuntime {
    pub(super) state: usize,
    pub(super) time: f32,
    pub(super) target: f32,
    pub(super) speed: f32,
}

/// Where every triggered part of every addressed model currently is.
///
/// Deliberately free of GPU state: a door's motion is a clock over a clip,
/// and keeping it separable is what lets the whole reversible state machine
/// be tested without a device.
#[derive(Clone)]
pub(super) struct ModelClipPlayback{pub(super) name:Option<String>,pub(super) time:f32,pub(super) looping:bool,pub(super) weight:f32}
#[derive(Default, Clone)]
pub(super) struct ModelStates {
    pub(super) clips:std::collections::BTreeMap<ModelTarget,ModelClipPlayback>,
    pub(super) map: std::collections::BTreeMap<(ModelTarget, String), AnimPartRuntime>,
    /// Engine presentation clock for model-authored `localgen-*` idle clips.
    /// It is independent from level script state: placing the GLB is enough
    /// to make its manifest animation run.
    pub(super) idle_time: f32,
}

impl ModelStates {
    pub(super) fn morph_weights(&self,target:&ModelTarget,id:&str,morph:&crate::asset_morph::AssetMorph)->[f32;32]{
        if let Some(playback)=self.clip(target,id){
            if let Some(name)=&playback.name{let mut sampled=morph.sample_playback(Some(name),playback.time,playback.looping);for(i,value)in sampled.iter_mut().enumerate(){*value=morph.defaults[i]+(*value-morph.defaults[i])*playback.weight;}sampled}else{morph.defaults}
        }else{morph.sample(None,self.idle_time)}
    }
    pub(super) fn clip(&self,target:&ModelTarget,id:&str)->Option<&ModelClipPlayback>{self.clips.get(target).or_else(||self.clips.get(&ModelTarget::Model(id.into())))}
    pub(super) fn transform(&self,target:&ModelTarget,id:&str,def:&crate::model::AnimPart)->Mat4f{
        if let (Some(playback),Some(hierarchy))=(self.clip(target,id),def.clip.hierarchy.as_ref()){
            return hierarchy.transform_named_weighted(playback.name.as_deref(),playback.name.as_ref().map(|_|playback.time),playback.looping,playback.weight);
        }
        let(_,time,_)=self.clock(target,id,def);def.transform_at(time)
    }
    /// Aim `def` at `state`. False (and no change) when the part has no such
    /// state. `blend_secs` times THIS move: the speed is fixed here, from the
    /// distance still to cover, so an interrupted move reverses at a
    /// comparable pace instead of snapping or crawling.
    pub(super) fn set(
        &mut self,
        target: ModelTarget,
        def: &crate::model::AnimPart,
        state: &str,
        blend_secs: f32,
    ) -> bool {
        let Some(index) = def.state_index(state) else {
            return false;
        };
        let goal = def.state_time(index);
        let key = (target, def.name.clone());
        let now = self
            .map
            .get(&key)
            .map(|r| r.time)
            .unwrap_or_else(|| def.state_time(def.default));
        let speed = if blend_secs > 0.0 {
            ((goal - now).abs() / blend_secs).max(1.0e-6)
        } else {
            f32::INFINITY
        };
        let time = if speed.is_finite() { now } else { goal };
        self.map
            .insert(key, AnimPartRuntime { state: index, time, target: goal, speed });
        true
    }

    /// Advance every clock by `dt` seconds, linearly, stopping on target.
    pub(super) fn tick(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        self.idle_time = (self.idle_time + dt).rem_euclid(86_400.0);
        for run in self.map.values_mut() {
            if !run.speed.is_finite() {
                run.time = run.target;
                continue;
            }
            let step = run.speed * dt;
            if (run.target - run.time).abs() <= step {
                run.time = run.target;
            } else if run.target > run.time {
                run.time += step;
            } else {
                run.time -= step;
            }
        }
    }

    /// Drop every clock addressed at `id`, plus the listed placed slots —
    /// what a model going away means for its doors.
    pub(super) fn forget_model(&mut self, id: &str, slots: &[usize]) {
        self.clips.retain(|target,_|match target{ModelTarget::Model(model)=>model!=id,ModelTarget::Instance(i)=>!slots.contains(i),ModelTarget::Attachment(_)=>false});
        self.map.retain(|(target, _), _| match target {
            ModelTarget::Model(m) => m != id,
            ModelTarget::Instance(i) => !slots.contains(i),
            ModelTarget::Attachment(_) => false,
        });
    }

    /// Drop every per-slot clock. Slot numbers only mean anything against one
    /// placed list, so a scene whose identity changed takes its per-slot
    /// commands with it; per-MODEL commands (how an imported level addresses
    /// its own doors) survive.
    pub(super) fn forget_slots(&mut self) {
        self.clips.retain(|target,_|matches!(target,ModelTarget::Model(_)));
        self.map
            .retain(|(target, _), _| matches!(target, ModelTarget::Model(_)));
    }

    /// (state index, clip time, target time) for one part. A per-slot command
    /// wins over a per-model one; with neither, the part sits in the pose the
    /// file authored as its default.
    pub(super) fn clock(
        &self,
        target: &ModelTarget,
        model_id: &str,
        def: &crate::model::AnimPart,
    ) -> (usize, f32, f32) {
        let by_slot = matches!(target, ModelTarget::Instance(_)|ModelTarget::Attachment(_))
            .then(|| self.map.get(&(target.clone(), def.name.clone())))
            .flatten();
        let run = by_slot.or_else(|| {
            self.map
                .get(&(ModelTarget::Model(model_id.to_string()), def.name.clone()))
        });
        match run {
            Some(r) => (r.state, r.time, r.target),
            None => {
                if def.kind.as_deref().is_some_and(|kind| kind.starts_with("localgen-")||kind=="asset-animation")
                    && def.duration() > 0.0
                {
                    let t = self.idle_time.rem_euclid(def.duration());
                    return (def.default, t, t);
                }
                let t = def.state_time(def.default);
                (def.default, t, t)
            }
        }
    }
}

/// A part's current state, as reported by [`Renderer::model_states`].
#[derive(Clone, Debug, PartialEq)]
pub struct ModelPartState {
    /// The glTF node name — the handle `set_model_state` takes.
    pub part: String,
    /// Index into the part's `states`, and its name.
    pub state: usize,
    pub state_name: String,
    /// Where the part is on its clip, and where it is heading.
    pub time: f32,
    pub target_time: f32,
    /// The move has finished: the part sits exactly on `state`.
    pub settled: bool,
}

/// One placed instance's anim part, resolved into WORLD space for this
/// moment — what a walker collides with. Returned by
/// [`Renderer::anim_part_boxes`].
#[derive(Clone, Debug)]
pub struct AnimPartBox {
    /// Slot in the placed-model list, and the model id in it.
    pub instance: usize,
    pub model: String,
    pub part: String,
    /// `extras.kind` (`door`), for a caller that treats kinds differently.
    pub kind: Option<String>,
    pub state: usize,
    pub state_name: String,
    /// World-space collider boxes, already moved to where the part is now.
    pub boxes: Vec<(Vec3f, Vec3f)>,
    /// World AABB over `boxes` — a cheap broad-phase reject.
    pub min: Vec3f,
    pub max: Vec3f,
}

/// One placed stock prop. `model` is the asset id it was loaded under, e.g.
/// `kenney/car-kit/ambulance`.
#[derive(Clone)]
pub struct ModelInstance {
    pub model: String,
    /// Visual-only opt-in; absent or failed custom shader uses the stock lane.
    pub custom_material: Option<CustomMaterialInstance>,
    pub transform: Mat4f,
    /// Per-copy albedo multiplier. White preserves the authored material.
    pub tint: Vec4f,
    /// Hue degrees, saturation multiplier, value multiplier, reserved.
    pub color_adjust: Vec4f,
    /// True for instances that follow a moving body (`on_body`). Dynamic
    /// instances are EXCLUDED from the baked static shadows — a driveable
    /// car's parked silhouette otherwise stays behind as a stain — and get a
    /// per-frame blob until their model's SDF silhouette atlas lands.
    pub dynamic: bool,
    /// Depth-tie order among coplanar stacked statics: pieces stay
    /// geometrically FLUSH and this feeds the shader's depth_bias
    /// (order * 1e-3 view-space scale) so the later-placed piece wins the
    /// z-tie. 0 for anything that never stacks (dynamics, lone props).
    pub depth_order: f32,
    /// Optional complete model-space poses for externally-driven rigid parts,
    /// keyed by their source-neutral connection name. Missing entries sit in
    /// the authored rest pose, so generic viewers need no special handling.
    pub part_poses: Vec<ModelPartPose>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CustomMaterialInstance {
    pub name: String,
    pub params: Vec4f,
}

#[derive(Clone)]
pub struct ModelPartPose {
    pub connection: String,
    pub transform: Mat4f,
}

/// Read-only connection metadata exposed to game object loaders. All values
/// are in authored model space; callers apply the same uniform scale and
/// model-to-body basis they use for the visible instance.
#[derive(Clone, Debug)]
pub struct DrivenPartInfo {
    pub connection: String,
    pub pivot: Vec3f,
    pub anchor: Vec3f,
    pub radius: f32,
    pub width: f32,
    /// Display-only motion limits; never use these to configure physics.
    pub visual: Option<makepad_gltf::VisualWheelMotion>,
    pub rest_transform: Mat4f,
}

/// Stable FNV-1a signature for the subset of a placed-model frame that can
/// invalidate static derived state. `DefaultHasher` is intentionally not
/// used: its algorithm is not a persistence/identity contract.
/// Longest world-space side, in metres, at which a static that owns no AO
/// layout stops being treated as a shadow caster in the bake.
///
/// A prop casts onto the world around it; a whole IMPORTED LEVEL is the world
/// around it, so feeding one into the sun-depth passes shadows its own
/// interior and every room goes black. 40 m is comfortably above any prop
/// (the biggest kit building measures ~20) and far below a map.
pub(super) const CASTER_ONLY_MAX_SPAN: f32 = 40.0;

/// Does a static WITHOUT its own AO layout join the bake's sun-depth passes?
///
/// `explicit` is the caller's own answer for this model id
/// ([`Renderer::set_model_casts_shadow`]) and always wins — a host that knows
/// it is loading a world says so, and no heuristic overrules it. Otherwise a
/// prelit model (its light is already in COLOR_0, so the sun does not reach
/// it) and anything level-sized stay out.
pub(super) fn casts_as_caster_only(explicit: Option<bool>, prelit: bool, min: Vec3f, max: Vec3f) -> bool {
    if let Some(answer) = explicit {
        return answer;
    }
    if prelit {
        return false;
    }
    let size = max - min;
    size.x.max(size.y).max(size.z) <= CASTER_ONLY_MAX_SPAN
}

/// Local collider boxes through a world matrix, plus the AABB over them.
///
/// Each box is re-fitted around its eight transformed corners rather than
/// rotated as a box: a level's parts are axis-aligned, where this is exact,
/// and a rotated one still gets a collider that fully contains it — a door
/// swinging on a hinge is never LESS solid than it looks.
pub(super) fn world_boxes(m: &Mat4f, boxes: &[(Vec3f, Vec3f)]) -> (Vec<(Vec3f, Vec3f)>, Vec3f, Vec3f) {
    let mut out = Vec::with_capacity(boxes.len());
    let mut lo = vec3f(f32::MAX, f32::MAX, f32::MAX);
    let mut hi = vec3f(f32::MIN, f32::MIN, f32::MIN);
    for (bmin, bmax) in boxes {
        let mut blo = vec3f(f32::MAX, f32::MAX, f32::MAX);
        let mut bhi = vec3f(f32::MIN, f32::MIN, f32::MIN);
        for x in [bmin.x, bmax.x] {
            for y in [bmin.y, bmax.y] {
                for z in [bmin.z, bmax.z] {
                    let p = m
                        .transform_vec4(Vec4f { x, y, z, w: 1.0 })
                        .to_vec3f();
                    blo.x = blo.x.min(p.x);
                    blo.y = blo.y.min(p.y);
                    blo.z = blo.z.min(p.z);
                    bhi.x = bhi.x.max(p.x);
                    bhi.y = bhi.y.max(p.y);
                    bhi.z = bhi.z.max(p.z);
                }
            }
        }
        lo.x = lo.x.min(blo.x);
        lo.y = lo.y.min(blo.y);
        lo.z = lo.z.min(blo.z);
        hi.x = hi.x.max(bhi.x);
        hi.y = hi.y.max(bhi.y);
        hi.z = hi.z.max(bhi.z);
        out.push((blo, bhi));
    }
    if out.is_empty() {
        (out, Vec3f::default(), Vec3f::default())
    } else {
        (out, lo, hi)
    }
}

/// What the packed static slabs are valid for: the static geometry, its
/// paint, and the light baked into its colours. A repaint (`paint_rev`)
/// repacks the slabs and nothing else.
pub(super) fn static_slab_key(world: &World, bake_generation: u64) -> (u64, u64, u64) {
    (world.render_rev, world.paint_rev, bake_generation)
}

/// What a GPU lightmap job is valid for: the static geometry — never its
/// paint — the placed models and the daylight quantum. A lamp turning red
/// must not re-bake a town (Crossroads, 2026-09-02: 68 bakes a minute).
pub(super) fn lightmap_world_key(world: &World, models_rev: u64, day_key: u32) -> (u64, u64, u32) {
    (world.render_rev, models_rev, day_key)
}

/// Only a baked sun-visibility consumer needs angle updates. Below the
/// horizon every region has the same no-direct-sun result: the clock moving
/// through midnight must not continually replace a town's lighting atlas.
pub(super) fn lightmap_sun_changed(previous: Option<Vec3f>, dir: Vec3f, mode: crate::gpu_lightmap::GpuLightmapMode) -> bool {
    if mode != crate::gpu_lightmap::GpuLightmapMode::OnChange { return false; }
    let Some(previous) = previous else { return true; };
    let was_up = previous.y > 0.02;
    let is_up = dir.y > 0.02;
    was_up != is_up || (is_up && previous.normalize().dot(dir.normalize()) < 0.03_f32.cos())
}

pub(super) fn placed_scene_signature(instances: &[ModelInstance]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    fn bytes(hash: &mut u64, input: &[u8]) {
        for byte in input {
            *hash ^= *byte as u64;
            *hash = hash.wrapping_mul(PRIME);
        }
    }

    let mut hash = OFFSET;
    bytes(&mut hash, &(instances.len() as u64).to_le_bytes());
    for instance in instances {
        bytes(&mut hash, &(instance.model.len() as u64).to_le_bytes());
        bytes(&mut hash, instance.model.as_bytes());
        bytes(&mut hash, &[u8::from(instance.dynamic)]);
        if !instance.dynamic {
            for value in instance.transform.v {
                bytes(&mut hash, &value.to_bits().to_le_bytes());
            }
            bytes(&mut hash, &instance.depth_order.to_bits().to_le_bytes());
        }
    }
    hash
}

impl ModelInstance {
    /// Explicit authored metre-space attachment. Unlike `on_body`, this
    /// never measures/recentres/fits the mesh: `origin` lands on body origin.
    /// +Z front becomes engine -Z and the full rigid frame carries bank/pitch.
    /// Callers validate finite bounded scale/origin at the authoring boundary.
    pub fn on_body_authored(model: String, scale: f32, origin: Vec3f, frame: &Mat4f) -> Self {
        let mut local = Mat4f::identity();
        local.v[0] = -scale;
        local.v[5] = scale;
        local.v[10] = -scale;
        local.v[12] = origin.x * scale;
        local.v[13] = -origin.y * scale;
        local.v[14] = origin.z * scale;
        Self {
            model, transform: Mat4f::mul(frame, &local),
            tint: vec4(1.0, 1.0, 1.0, 1.0), color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
            dynamic: true, depth_order: 0.0, part_poses: Vec::new(), custom_material: None,
        }
    }
    /// Hang a model off a moving body, anchored by the MODEL's own measured
    /// bounds rather than by the body's collision box.
    ///
    /// Kenney authors a model with its origin on the surface it stands on:
    /// every vehicle in `car-kit`, `racing`, `retro-urban-kit` and
    /// `toy-car-kit` measures `min.y == 0` with the tyres at zero, and a prop
    /// puts its feet there the same way. Some kits do not — track and road
    /// pieces carry a skirt below zero, ground tiles a slab — so the anchor
    /// is READ from `bounds`, never assumed. `min.y` is the model's floor
    /// wherever the exporter left it.
    ///
    /// `frame` is the body's world rotation and position with no scale (see
    /// [`Renderer::rigid_transform`]). `drop` is how far below the body's
    /// origin, **along the body's own down axis**, that floor should sit —
    /// i.e. where the ground is relative to the body. For anything resting
    /// directly on its box that is the box's half height; for a wheeled body
    /// it is not, because the box never touches the road (see
    /// `blocks::Car::contact_drop`). Applying it in body space rather than
    /// world Y is what keeps the mesh bolted on when the body pitches.
    ///
    /// # Facing: the anchor turns the model 180°
    ///
    /// glTF defines an asset's FRONT as **+Z** (the spec's own words), and
    /// the vehicle kits follow it — measured on `car-kit/ambulance`, whose
    /// windshield and headlights sit on the model's +Z face. The engine's
    /// forward is **−Z** (`sim::heading` — every fixture assumes it). Passing
    /// the model's axes straight through therefore bolts every vehicle on
    /// TAIL-FIRST: parked it is invisible (a parked car has no "supposed"
    /// heading), but the moment it drives, throttle pulls it out of its own
    /// visual rear and a correctly-yawing car seen driving backwards reads as
    /// mirrored steering — reported as "flippers reversed AND steering
    /// reversed", one root for both. So the anchor rotates the model half a
    /// turn about Y, once, here — the boundary where authored-model space
    /// meets body space — rather than as a sign negated at some call site.
    pub fn on_body(
        model: String,
        bounds: (Vec3f, Vec3f),
        scale: f32,
        drop: f32,
        frame: &Mat4f,
    ) -> Self {
        let (min, max) = bounds;
        // Model space → body space: yaw π (glTF +Z front → engine −Z
        // forward) times uniform scale — for a half turn about Y that is
        // exactly negated x/z columns — then move the model's own floor to
        // `drop` below the origin and its own horizontal centre onto the
        // body's axis. Floor and centre are measured, so a kit that authors
        // its origin in a corner of the scene grid still lands on the body.
        let mut local = Mat4f::identity();
        local.v[0] = -scale;
        local.v[5] = scale;
        local.v[10] = -scale;
        local.v[12] = (min.x + max.x) * 0.5 * scale;
        local.v[13] = -drop - min.y * scale;
        local.v[14] = (min.z + max.z) * 0.5 * scale;
        Self {
            model,
            transform: Mat4f::mul(frame, &local),
            tint: vec4(1.0, 1.0, 1.0, 1.0),
            color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
            dynamic: true,
            depth_order: 0.0,
            custom_material: None,
            part_poses: Vec::new(),
        }
    }

    pub fn with_tint(mut self, tint: Vec4f) -> Self {
        self.tint = tint;
        self
    }

    pub fn with_custom_material(mut self, material: Option<CustomMaterialInstance>) -> Self {
        self.custom_material = material;
        self
    }

    pub fn with_color_adjust(mut self, color_adjust: Vec4f) -> Self {
        self.color_adjust = color_adjust;
        self
    }
}
