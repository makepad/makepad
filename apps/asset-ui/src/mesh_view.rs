//! MeshView — GLB artifact viewer with three paths:
//!
//! - STATIC PBR: an unskinned GLB whose materials reference textures
//!   (Hunyuan/TRELLIS painted output) renders as REAL materials through the
//!   engine's existing glTF path — `makepad_xr::render::GltfRenderer` over
//!   makepad-draw's `DrawPbr` (GGX direct light, normal mapping, ORM,
//!   sRGB-decoded base color). Light/environment/exposure are controllable
//!   (API + focused-pane keys) and the HUD reports which material roles are
//!   loaded vs absent. See `pbr_preview.rs`.
//! - STATIC base-color: any other unskinned/clipless mesh renders as a
//!   statue on the ground slab (`Renderer::load_model` + `model_bounds`
//!   + `set_models`), height-normalized, feet at y=0. Drag orbits, the
//!   wheel zooms. Also the honest fallback when a PBR load fails.
//! - PLAYABLE CHARACTER: a skinned GLB with animation clips (the character
//!   chain's output: Trellis mesh → SkinTokens rig → HY-Motion clips) loads
//!   through the engine skin parser and becomes PLAYABLE — click the pane,
//!   then WASD/arrows walk (camera-relative), Shift+movement runs, and Space
//!   jumps. The locomotion
//!   state machine (`makepad_render::play`, unit-tested) maps input → clip through
//!   the motion domain's deterministic clip names (idle/walk/run/jump); clips
//!   are in-place, movement is transform-driven, the camera follows the
//!   character. Same GPU-skinning flow: `parse_glb` →
//!   `rest_gpu_flat` (generated meshes are too dense for a live AO bake) →
//!   `upload_skin_rig`; per tick loop-safe locomotion sampling / state
//!   `blend_pose` / `palette` → `SkinnedDraw` → `SkinnedBatch` →
//!   `draw_scene_full`.
//!
//! MESH_PLAY_CAPTURE_DIR=<dir>: deterministic evidence run — a scripted
//! input schedule (idle → walk → run/turn → jump) pinned to the tick counter,
//! PNG captures at fixed ticks, exit once all are on disk (the rig
//! example's RIG_CAPTURE_DIR pattern).

use makepad_render::play::{LocoState, Locomotion, PlayInput};
use makepad_render::skin::{PoseBuffer, SkinnedModel, SKIN_VERTEX_FLOATS};
use makepad_render::{
    preview_scene_state, set_pass_camera, DrawSceneAlpha, DrawSceneCube,
    DrawSceneShadow, DrawSceneShadowSdf, DrawSceneSkinned, DrawSceneSkinnedGpu, DrawSceneSky,
    DrawSceneScreen, DrawSceneTerrain, DrawSceneTexture, SceneDraws, Renderer, GpuLightmapMode,
    ModelInstance, PreviewLook, PreviewStage, ScreenInstance, SkinnedBatch, SkinnedDraw, TICK_DT,
};
use makepad_widgets::*;

// The static-PBR branch lives in its own file; declared from here (not
// main.rs, which another lane owns) — `#[path]` resolves the sibling in src/.
#[path = "pbr_preview.rs"]
pub(crate) mod pbr_preview;
use pbr_preview::{PbrDisplayControls, PbrPreview, PbrStatus};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MeshViewBase = #(MeshView::register_widget(vm))
    mod.widgets.MeshView = set_type_default() do mod.widgets.MeshViewBase{
        width: Fill
        height: Fill
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_alpha +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_terrain +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_models +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_skinned +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_sprite +: {
            alpha_blend: true
            backface_culling: false
            pixel: fn() {
                let color = self.tex.sample_nearest(self.v_uv)
                if color.w < 0.08 {
                    discard()
                }
                return vec4(color.x, color.y, color.z, color.w)
            }
        }
        draw_hud +: {
            text_style: theme.font_regular{font_size: 9}
            color: #xffffffcc
        }
    }
}

struct PendingModel {
    glb: Vec<u8>,
    png: Option<Vec<u8>>,
    aomesh: Option<Vec<u8>>,
    ao_png: Option<Vec<u8>>,
}

/// Crossfade length when the locomotion state switches clips, seconds.
const FADE_SECONDS: f32 = 0.2;
/// Tail-to-head repair window for cyclic idle/walk clips. Generated motion
/// does not promise a seamless loop; spreading its seam over this window
/// prevents a periodic single-frame snap in play mode.
const LOOP_BLEND_SECONDS: f32 = 0.2;
/// Legacy three-clip artifacts have no authored run. Reusing their walk at
/// the controller speed ratio keeps foot cadence tied to ground travel until
/// a newly generated four-clip artifact is installed.
const RUN_FALLBACK_RATE: f32 = makepad_render::play::RUN_SPEED / makepad_render::play::WALK_SPEED;
const LOCO_SLOT_COUNT: usize = 4;
/// A generated landing may need a modest vertical contact repair after its
/// authored skeleton-root translation is removed. Larger corrections mean
/// the selected action window is not a credible landing pose, so fail the
/// playable load instead of hiding a broken animation by moving the whole
/// character an arbitrary distance.
const MAX_LANDING_CONTACT_CORRECTION_FRACTION: f32 = 0.15;
/// Acceptance bound for the corrected lowest vertex during the landing
/// crossfade, normalized by character height.
const LANDING_CONTACT_TOLERANCE_FRACTION: f32 = 0.02;

#[derive(Clone, Copy, Debug, Default)]
struct LandingContactAdjustment {
    /// Model-space Y added to the character transform after scaling.
    offset_model_y: f32,
    correction_fraction: f32,
    corrected_gap_fraction: f32,
    corrected_penetration_fraction: f32,
}

impl LandingContactAdjustment {
    fn passes(self) -> bool {
        self.correction_fraction <= MAX_LANDING_CONTACT_CORRECTION_FRACTION
            && self.corrected_gap_fraction <= LANDING_CONTACT_TOLERANCE_FRACTION
            && self.corrected_penetration_fraction <= LANDING_CONTACT_TOLERANCE_FRACTION
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct LandingContactAudit {
    samples: usize,
    invalid_samples: usize,
    max_uncorrected_gap_fraction: f32,
    max_uncorrected_penetration_fraction: f32,
    max_correction_fraction: f32,
    max_corrected_gap_fraction: f32,
    max_corrected_penetration_fraction: f32,
}

impl LandingContactAudit {
    fn passes(self) -> bool {
        self.samples > 0
            && self.invalid_samples == 0
            && self.max_correction_fraction <= MAX_LANDING_CONTACT_CORRECTION_FRACTION
            && self.max_corrected_gap_fraction <= LANDING_CONTACT_TOLERANCE_FRACTION
            && self.max_corrected_penetration_fraction <= LANDING_CONTACT_TOLERANCE_FRACTION
    }
}

fn landing_contact_gate_passes(has_jump_clip: bool, audit: LandingContactAudit) -> bool {
    // Older playable artifacts may only provide idle/walk. Their jump slot
    // intentionally falls back to idle, so there is no authored landing to
    // repair or reject.
    !has_jump_clip || audit.passes()
}

fn packed_y_bounds(packed: &[f32]) -> Option<(f32, f32)> {
    if packed.is_empty() || packed.len() % SKIN_VERTEX_FLOATS != 0 {
        return None;
    }
    let mut bounds: Option<(f32, f32)> = None;
    for vertex in packed.chunks_exact(SKIN_VERTEX_FLOATS) {
        let y = vertex[1];
        if !y.is_finite() {
            return None;
        }
        bounds = Some(match bounds {
            Some((min_y, max_y)) => (min_y.min(y), max_y.max(y)),
            None => (y, y),
        });
    }
    bounds
}

/// Put the posed mesh's lowest vertex back on the rest-pose contact plane.
/// The result is model-space: callers scale it exactly once when composing
/// the host transform. This deliberately knows nothing about joint names or
/// indices, so split ankles and unfamiliar generated hierarchies behave the
/// same way.
fn landing_contact_adjustment(
    rest_min_y: f32,
    posed_min_y: f32,
    character_height: f32,
) -> Option<LandingContactAdjustment> {
    if !rest_min_y.is_finite()
        || !posed_min_y.is_finite()
        || !character_height.is_finite()
        || character_height <= f32::EPSILON
    {
        return None;
    }
    let uncorrected = posed_min_y - rest_min_y;
    let offset_model_y = -uncorrected;
    let correction_fraction = offset_model_y.abs() / character_height;
    // Keep the residual calculation explicit rather than assuming exact
    // cancellation; the gate then catches non-finite/precision regressions.
    let corrected = uncorrected + offset_model_y;
    Some(LandingContactAdjustment {
        offset_model_y,
        correction_fraction,
        corrected_gap_fraction: corrected.max(0.0) / character_height,
        corrected_penetration_fraction: (-corrected).max(0.0) / character_height,
    })
}

fn clip_playback_rate(clips: &[usize; LOCO_SLOT_COUNT], slot: usize) -> f32 {
    if slot == Character::slot_for(LocoState::Run)
        && clips[slot] == clips[Character::slot_for(LocoState::Walk)]
    {
        RUN_FALLBACK_RATE
    } else {
        1.0
    }
}

/// The loaded playable character: engine model + per-frame animation state +
/// the locomotion controller. The controller half is presentation-agnostic
/// (`makepad_render::play`) — a later billboard character reuses it unchanged.
struct Character {
    model: SkinnedModel,
    texture: Texture,
    /// Clip index per LocoState slot [idle, walk, run, jump], resolved by NAME
    /// through the engine's substring matcher; a state whose candidates all
    /// miss falls back to the idle pick (a rig with only a walk still plays).
    clips: [usize; LOCO_SLOT_COUNT],
    /// Current clip slot and its local time.
    slot: usize,
    clip_time: f32,
    /// Previous clip, still advancing, blended out over FADE_SECONDS.
    prev_slot: usize,
    prev_time: f32,
    fade: f32,
    pose: PoseBuffer,
    prev_pose: PoseBuffer,
    blended: PoseBuffer,
    /// Reused head-pose buffer for allocation-free loop seam blending.
    loop_scratch: PoseBuffer,
    palette: Vec<Mat4f>,
    /// Uniform scale to human height + lift putting rest feet on y=0,
    /// measured from the CPU-skinned rest pose.
    scale: f32,
    lift: f32,
    rest_min_y: f32,
    rest_height: f32,
    /// Extra WORLD-space lift used only while a grounded state is blending
    /// out of jump. It is separate from `loco.pos[1]`, so the host ballistic
    /// arc remains the sole owner of airborne/root travel.
    landing_contact_lift: f32,
    /// Reused CPU-skin buffer. GPU skinning remains the steady-state path;
    /// this is populated only for the eleven landing-fade frames.
    landing_contact_packed: Vec<f32>,
    /// False for legacy idle/walk rigs whose jump slot falls back to idle.
    /// Those rigs have no authored landing and bypass contact correction.
    has_jump_clip: bool,
    /// The playable state (position, facing, jump arc).
    loco: Locomotion,
    /// Ticks spent airborne — drives the jump clip phase-locked to the arc.
    air_ticks: u32,
    /// Showcase mode (C key): pin+cycle clips instead of playing.
    showcase: bool,
    /// Index into `model.clips` while showcasing, so extra states
    /// (attack, pain, death) play too — not just the four loco slots.
    showcase_clip: usize,
    /// Extra model yaw (F key flips): generated rigs' authored facing
    /// varies; PI when the asset walks backwards.
    asset_yaw: f32,
    /// Renderer rig key (the load generation).
    rig: u64,
}

impl Character {
    fn switch(&mut self, slot: usize) {
        if slot == self.slot {
            return;
        }
        self.prev_slot = self.slot;
        self.prev_time = self.clip_time;
        self.slot = slot;
        self.clip_time = 0.0;
        self.fade = 0.0;
    }

    fn slot_for(state: LocoState) -> usize {
        match state {
            LocoState::Idle => 0,
            LocoState::Walk => 1,
            LocoState::Run => 2,
            LocoState::Jump => 3,
        }
    }

    fn playback_rate(&self, slot: usize) -> f32 {
        clip_playback_rate(&self.clips, slot)
    }

    fn advance_fade(fade: f32) -> f32 {
        (fade + TICK_DT / FADE_SECONDS).min(1.0)
    }

    fn slot_loops(slot: usize) -> bool {
        slot != Self::slot_for(LocoState::Jump)
    }

    /// Advance one fixed tick: integrate the controller, map its state to a
    /// clip, advance the clip clocks. Returns true when showcase mode rolled
    /// to another clip (for HUD refresh only).
    fn tick(&mut self, input: &PlayInput, cam_yaw: f32) {
        if self.showcase {
            // Showcase: every authored clip, not just the loco slots.
            self.clip_time += TICK_DT;
            self.prev_time += TICK_DT;
            self.fade = Self::advance_fade(self.fade);
            let n = self.model.clips.len().max(1);
            self.showcase_clip %= n;
            let duration = self.model.clips[self.showcase_clip].duration;
            if self.clip_time >= duration {
                self.showcase_clip = (self.showcase_clip + 1) % n;
                self.clip_time = 0.0;
                self.fade = 1.0;
            }
            return;
        }
        let state = self.loco.update(TICK_DT, input, cam_yaw);
        self.switch(Self::slot_for(state));
        if state == LocoState::Jump {
            // Generated jump clips contain long preparation and recovery.
            // Phase-lock only their centered action window to the ballistic
            // arc; mapping the whole multi-second clip here used to play it
            // at more than five times its authored speed.
            self.air_ticks += 1;
            let airtime =
                2.0 * makepad_render::play::JUMP_SPEED / makepad_render::play::GRAVITY;
            let progress =
                (self.air_ticks as f32 * TICK_DT / airtime.max(0.01)).min(1.0);
            let duration = self.model.clips[self.clips[self.slot]].duration;
            self.clip_time = makepad_render::play::jump_clip_time(progress, duration);
            let prev_rate = self.playback_rate(self.prev_slot);
            self.prev_time += TICK_DT * prev_rate;
        } else {
            self.air_ticks = 0;
            let rate = self.playback_rate(self.slot);
            let prev_rate = self.playback_rate(self.prev_slot);
            self.clip_time += TICK_DT * rate;
            self.prev_time += TICK_DT * prev_rate;
        }
        self.fade = Self::advance_fade(self.fade);
    }

    fn current_clip_name(&self) -> &str {
        if self.showcase {
            self.model
                .clips
                .get(self.showcase_clip)
                .map(|c| c.name.as_str())
                .unwrap_or("?")
        } else {
            self.model
                .clips
                .get(self.clips[self.slot])
                .map(|c| c.name.as_str())
                .unwrap_or("?")
        }
    }

    /// Sample the current (and, mid-fade, the previous) clip into a palette.
    fn pose_palette(&mut self) {
        if self.showcase {
            let clip = self.showcase_clip.min(self.model.clips.len().saturating_sub(1));
            self.model.sample_clip_loop_blended(
                clip,
                self.clip_time,
                LOOP_BLEND_SECONDS,
                &mut self.pose,
                &mut self.loop_scratch,
            );
            self.model.palette(&self.pose, &mut self.palette);
            self.landing_contact_lift = 0.0;
            return;
        }
        Self::sample_slot(
            &self.model,
            &self.clips,
            self.slot,
            self.clip_time,
            &mut self.pose,
            &mut self.loop_scratch,
        );
        if self.fade < 1.0 {
            Self::sample_slot(
                &self.model,
                &self.clips,
                self.prev_slot,
                self.prev_time,
                &mut self.prev_pose,
                &mut self.loop_scratch,
            );
            SkinnedModel::blend_pose(&self.prev_pose, &self.pose, self.fade, &mut self.blended);
        } else {
            std::mem::swap(&mut self.blended, &mut self.pose);
        }
        self.model.palette(&self.blended, &mut self.palette);
        self.landing_contact_lift = 0.0;
        let jump_slot = Self::slot_for(LocoState::Jump);
        // Do not touch the airborne transform: contact repair starts only
        // after Locomotion has clamped its host-owned ballistic Y to zero.
        // Once the state fade completes, ordinary gait/root motion is left
        // untouched again.
        if self.loco.grounded
            && self.has_jump_clip
            && self.slot != jump_slot
            && self.prev_slot == jump_slot
            && self.fade < 1.0
        {
            self.model
                .skin_to_packed(&self.palette, &mut self.landing_contact_packed);
            if let Some((posed_min_y, _)) = packed_y_bounds(&self.landing_contact_packed) {
                if let Some(adjustment) = landing_contact_adjustment(
                    self.rest_min_y,
                    posed_min_y,
                    self.rest_height,
                ) {
                    // Load-time auditing already rejects implausibly large
                    // corrections. Applying the measured residual here makes
                    // both upward gaps and downward penetration exactly zero.
                    if adjustment.passes() {
                        self.landing_contact_lift = adjustment.offset_model_y * self.scale;
                    }
                }
            }
        }
    }

    fn sample_slot(
        model: &SkinnedModel,
        clips: &[usize; LOCO_SLOT_COUNT],
        slot: usize,
        time: f32,
        out: &mut PoseBuffer,
        loop_scratch: &mut PoseBuffer,
    ) {
        if !Self::slot_loops(slot) {
            // Jump is phase-driven and one-shot: holding its terminal key is
            // essential, especially while it is the outgoing state fade.
            // Strip authored skeleton-root travel after sampling because the
            // play controller owns the character transform, including Y.
            model.sample_clip_clamped(clips[slot], time, out);
            model.strip_skeleton_root_translation(out);
        } else {
            model.sample_clip_loop_blended(
                clips[slot],
                time,
                LOOP_BLEND_SECONDS,
                out,
                loop_scratch,
            );
        }
    }
}

/// Exercise the exact jump-out crossfade for every possible grounded target
/// slot. This is a load-time hard gate: a corrupt/non-finite skin or a jump
/// requiring more than 15% of character height in whole-model correction is
/// displayed as a statue instead of entering play mode.
fn audit_landing_contact(
    model: &SkinnedModel,
    clips: &[usize; LOCO_SLOT_COUNT],
    rest_min_y: f32,
    character_height: f32,
) -> LandingContactAudit {
    let mut audit = LandingContactAudit::default();
    let jump_slot = Character::slot_for(LocoState::Jump);
    let jump_duration = model.clips[clips[jump_slot]].duration;
    // Reproduce the fixed-step controller rather than assuming its
    // semi-implicit ballistic integration lands on an analytic one-second
    // endpoint. `jump_time_before_landing` is exactly the outgoing clock
    // Character::switch preserves on the real first grounded tick.
    let mut controller = Locomotion::default();
    let mut jump_time_before_landing = 0.0;
    let mut airborne_ticks = 0u32;
    for tick in 0..600 {
        let state = controller.update(
            TICK_DT,
            &PlayInput {
                jump: tick == 0,
                ..Default::default()
            },
            0.0,
        );
        if state == LocoState::Jump {
            airborne_ticks += 1;
            let airtime =
                2.0 * makepad_render::play::JUMP_SPEED / makepad_render::play::GRAVITY;
            let progress =
                (airborne_ticks as f32 * TICK_DT / airtime.max(0.01)).min(1.0);
            jump_time_before_landing =
                makepad_render::play::jump_clip_time(progress, jump_duration);
        } else if airborne_ticks > 0 {
            break;
        }
    }
    if airborne_ticks == 0 {
        audit.invalid_samples = 1;
        return audit;
    }
    let fade_frames = (FADE_SECONDS / TICK_DT).ceil().max(1.0) as usize;
    let mut jump_pose = PoseBuffer::new();
    let mut target_pose = PoseBuffer::new();
    let mut blended = PoseBuffer::new();
    let mut loop_scratch = PoseBuffer::new();
    let mut palette = Vec::new();
    let mut packed = Vec::new();

    for target_slot in 0..jump_slot {
        let target_rate = clip_playback_rate(clips, target_slot);
        for frame in 1..fade_frames {
            let elapsed = frame as f32 * TICK_DT;
            Character::sample_slot(
                model,
                clips,
                jump_slot,
                jump_time_before_landing + elapsed,
                &mut jump_pose,
                &mut loop_scratch,
            );
            Character::sample_slot(
                model,
                clips,
                target_slot,
                elapsed * target_rate,
                &mut target_pose,
                &mut loop_scratch,
            );
            let fade = (elapsed / FADE_SECONDS).clamp(0.0, 1.0);
            SkinnedModel::blend_pose(&jump_pose, &target_pose, fade, &mut blended);
            model.palette(&blended, &mut palette);
            model.skin_to_packed(&palette, &mut packed);
            audit.samples += 1;
            let Some((posed_min_y, _)) = packed_y_bounds(&packed) else {
                audit.invalid_samples += 1;
                continue;
            };
            let raw = (posed_min_y - rest_min_y) / character_height;
            audit.max_uncorrected_gap_fraction =
                audit.max_uncorrected_gap_fraction.max(raw.max(0.0));
            audit.max_uncorrected_penetration_fraction =
                audit.max_uncorrected_penetration_fraction.max((-raw).max(0.0));
            let Some(adjustment) =
                landing_contact_adjustment(rest_min_y, posed_min_y, character_height)
            else {
                audit.invalid_samples += 1;
                continue;
            };
            audit.max_correction_fraction = audit
                .max_correction_fraction
                .max(adjustment.correction_fraction);
            audit.max_corrected_gap_fraction = audit
                .max_corrected_gap_fraction
                .max(adjustment.corrected_gap_fraction);
            audit.max_corrected_penetration_fraction = audit
                .max_corrected_penetration_fraction
                .max(adjustment.corrected_penetration_fraction);
        }
    }
    audit
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct MeshView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawSceneTexture,
    #[live]
    draw_cube: DrawSceneCube,
    #[live]
    draw_alpha: DrawSceneAlpha,
    #[live]
    draw_sky: DrawSceneSky,
    #[live]
    draw_terrain: DrawSceneTerrain,
    #[live]
    draw_shadow: DrawSceneShadow,
    /// Game-scene SDF silhouette tier (OnChange). Realtime CSM is the
    /// preview default; this stays wired so we share the sandbox SceneDraws.
    #[live]
    draw_shadow_sdf: DrawSceneShadowSdf,
    /// Static prop shader — unskinned meshes render as statues.
    #[live]
    draw_models: DrawSceneSkinned,
    /// GPU-skinned character shader — the playable path.
    #[live]
    draw_skinned: DrawSceneSkinnedGpu,
    /// Camera-facing map sprites (catalog billboards).
    #[live]
    draw_sprite: DrawSceneScreen,
    /// glTF PBR shader for the static material-bearing branch — makepad-draw's
    /// DrawPbr, the same shader XR's Gltf widget drives. Lighting fields are
    /// written per frame from `pbr.controls`, so no script theming here.
    #[live]
    draw_pbr: DrawPbr,
    #[live]
    draw_hud: DrawText,
    #[live(vec4(0.03, 0.045, 0.075, 1.0))]
    clear_color: Vec4f,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust(false)]
    world_built: bool,
    #[rust]
    renderer: Renderer,
    #[rust]
    look: PreviewLook,
    #[rust(true)]
    show_ground: bool,
    /// Bytes handed in by the pipeline, loaded on the next draw (needs Cx).
    #[rust]
    pending: Option<PendingModel>,
    /// Unique per-load model id (the renderer caches by id).
    #[rust(0u64)]
    generation: u64,
    #[rust]
    instance: Option<ModelInstance>,
    #[rust]
    character: Option<Character>,
    /// Static PBR branch state (retained glTF renderer + fit + controls +
    /// honest role/decode status). Exactly one of `instance`, `character`
    /// and `pbr`'s renderer is populated per loaded artifact.
    #[rust]
    pbr: PbrPreview,
    #[rust]
    status: String,
    // Orbit camera (rig pattern).
    #[rust(0.6f32)]
    orbit_yaw: f32,
    #[rust(-0.22f32)]
    orbit_pitch: f32,
    #[rust]
    orbit_last_abs: Option<DVec2>,
    #[rust]
    view_rect: Rect,
    // Play-mode input: keys held while the pane has click-focus. KeyUp is
    // NOT focus-gated (the sandbox lesson: a key released after focus moved
    // must still clear).
    #[rust]
    keys: Vec<KeyCode>,
    #[rust(false)]
    focused: bool,
    // Fixed-step tick, 60Hz, decoupled from the frame rate.
    #[rust]
    next_frame: NextFrame,
    /// Redraw→draw handshake for the hidden test (see handle_event): a
    /// redraw-requesting step sets `awaiting_draw`; draw_walk sets
    /// `drawn_since_tick`. An expectation that was never met means the
    /// viewer is hidden and the chain parks until a visible draw re-arms.
    #[rust(false)]
    drawn_since_tick: bool,
    #[rust(false)]
    awaiting_draw: bool,
    #[rust(true)]
    pump_parked: bool,
    #[rust]
    time_accum: f64,
    #[rust]
    last_time: Option<f64>,
    #[rust]
    tick: u64,
    /// MESH_PLAY_CAPTURE_DIR: deterministic capture run (see module docs).
    #[rust(std::env::var("MESH_PLAY_CAPTURE_DIR").ok())]
    capture_dir: Option<String>,
    /// Capture readback is asynchronous (and a 6 MP PNG can take longer
    /// than the 12-tick seam/jump evidence spacing). Keep exactly one
    /// request in flight and hold its authored tick until the file lands.
    /// This is evidence-run state only; interactive play never enters it.
    #[rust]
    capture_pending: Option<String>,
    #[rust(false)]
    capture_initialized: bool,
    #[rust]
    captures_taken: Vec<u64>,
    /// First-person walk of a World GLB (spawn sidecar). Identity transform,
    /// no studio slab, camera at the player start.
    #[rust]
    walk_cam: Option<WalkCam>,
    #[rust]
    extra_instances: Vec<ModelInstance>,
    #[rust]
    placed_sprites: Vec<PlacedSprite>,
    #[rust]
    pending_placed_models: Vec<PlacedModel>,
}

struct PlacedModel {
    pos: Vec3f,
    yaw: f32,
    path: std::path::PathBuf,
}

struct PlacedSprite {
    frames: Vec<Texture>,
    fps: f32,
    pos: Vec3f,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, Debug)]
struct WalkCam {
    eye: Vec3f,
    yaw: f32,
    pitch: f32,
}

fn walk_forward(yaw: f32, pitch: f32) -> Vec3f {
    let cp = pitch.cos();
    vec3f(yaw.sin() * cp, pitch.sin(), -yaw.cos() * cp)
}

/// Place the orbit rig so the lens sits at `eye` looking along yaw/pitch.
/// scene_state puts the camera at `target - forward * distance` with a 0.5m
/// floor on distance, so the look-at point is 0.5m ahead of the eye.
fn walk_rig(eye: Vec3f, yaw: f32, pitch: f32) -> (Vec3f, f32) {
    (eye + walk_forward(yaw, pitch) * 0.5, 0.5)
}

/// yaw-rotation * uniform scale, translated (the sandbox villager idiom).
fn trs_yaw(pos: Vec3f, yaw: f32, scale: f32) -> Mat4f {
    let mut m = Mat4f::rotation(vec3f(0.0, yaw, 0.0));
    for k in [0usize, 1, 2, 4, 5, 6, 8, 9, 10] {
        m.v[k] *= scale;
    }
    m.v[12] = pos.x;
    m.v[13] = pos.y;
    m.v[14] = pos.z;
    m
}

/// Keep the visible viewer and the offscreen thumbnail renderer on the same
/// classification rule. `SkinnedModel::parse_glb` can successfully parse a
/// static GLB as a zero-joint, zero-clip skin-shaped value; that is still a
/// statue and must never enter the skinned batch (which would draw nothing).
fn is_playable_skin_shape(joints: usize, clips: usize) -> bool {
    joints > 0 && joints <= 256 && clips > 0
}

pub(crate) fn is_playable_skin(model: &SkinnedModel) -> bool {
    is_playable_skin_shape(model.joint_count(), model.clips.len())
}

/// Base-color image bytes out of a GLB, if it embeds one: material 0's
/// baseColorTexture source, else image 0 (skin.rs ignores materials by
/// design — the host binds the texture itself).
pub(crate) fn extract_base_color(glb: &[u8]) -> Option<Vec<u8>> {
    let loaded = makepad_gltf::load_gltf_from_bytes(glb, None).ok()?;
    let doc = &loaded.document;
    let image_index = doc
        .materials_slice()
        .first()
        .and_then(|m| m.pbr_metallic_roughness.as_ref())
        .and_then(|pbr| pbr.base_color_texture.as_ref())
        .and_then(|info| doc.textures_slice().get(info.index))
        .and_then(|tex| tex.source)
        .or(if doc.images_slice().is_empty() { None } else { Some(0) })?;
    makepad_gltf::load_image_bytes(&loaded, image_index).ok()
}

fn texture_from_png(cx: &mut Cx, png: &[u8]) -> Option<Texture> {
    Some(ImageBuffer::from_png(png).ok()?.into_new_texture(cx))
}

/// Same decode as the sprite viewer: `.billboard` preview frames, else one PNG.
fn load_sprite_frames(cx: &mut Cx, path: &std::path::Path) -> Vec<Texture> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("billboard") {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if let Ok(bb) = makepad_asset_importer::stateful_billboard::StatefulBillboard::parse(&text)
        {
            let mut out = Vec::new();
            for frame in bb.preview_frames() {
                let file = bb.resolve_frame(path, frame);
                if let Ok(png) = std::fs::read(&file) {
                    if let Some(tex) = texture_from_png(cx, &png) {
                        out.push(tex);
                    }
                }
            }
            if !out.is_empty() {
                return out;
            }
        }
    }
    std::fs::read(path)
        .ok()
        .and_then(|png| texture_from_png(cx, &png))
        .into_iter()
        .collect()
}

/// Decode PNG or JPEG bytes into a texture (Blender/SkinTokens exports carry
/// either), falling back to a 1x1 white so an untextured rig still draws.
pub(crate) fn image_texture(cx: &mut Cx, bytes: Option<Vec<u8>>) -> Texture {
    if let Some(bytes) = bytes {
        let decoded = if bytes.starts_with(&[0xff, 0xd8]) {
            ImageBuffer::from_jpg(&bytes).ok()
        } else {
            ImageBuffer::from_png(&bytes).ok()
        };
        if let Some(image) = decoded {
            return image.into_new_texture(cx);
        }
    }
    Texture::new_with_format(
        cx,
        TextureFormat::VecBGRAu8_32 {
            width: 1,
            height: 1,
            data: Some(vec![0xffff_ffff]),
            updated: TextureUpdated::Full,
        },
    )
}

/// The evidence-run input script: what the "player" does at each tick.
/// Walk pushes -Z (toward the default camera's far side) for five seconds —
/// deliberately longer than the generated four-second gait so acceptance
/// crosses its loop seam. Shift-run then continues side-on for more than two
/// complete four-second cycles before the scripted jump. The longer evidence
/// run is intentional: a single flattering run frame cannot certify a loop.
#[cfg(not(target_arch = "wasm32"))]
fn capture_input(tick: u64) -> PlayInput {
    match tick {
        0..=89 => PlayInput::default(),
        90..=389 => PlayInput { axis_z: 1.0, ..Default::default() },
        390..=899 => PlayInput { axis_x: 1.0, run: true, ..Default::default() },
        900..=909 => PlayInput {
            axis_x: 1.0,
            run: true,
            jump: true,
            ..Default::default()
        },
        _ => PlayInput { axis_x: 1.0, run: true, ..Default::default() },
    }
}

/// Capture schedule: tick -> filename. Walk brackets one wrap; run brackets
/// two successive wraps from a side-on view. Jump includes takeoff, apex and
/// the first grounded/contact-corrected pose. Numeric all-key audits remain
/// the hard gate, while these frames make gross visual regressions obvious.
const CAPTURES: [(u64, &str); 14] = [
    (60, "idle.png"),
    (150, "walk_a.png"),
    (250, "walk_b.png"),
    (326, "walk_pre_wrap.png"),
    (338, "walk_post_wrap.png"),
    (450, "run_a.png"),
    (540, "run_b.png"),
    (624, "run_pre_wrap_1.png"),
    (638, "run_post_wrap_1.png"),
    (864, "run_pre_wrap_2.png"),
    (878, "run_post_wrap_2.png"),
    (910, "jump_takeoff.png"),
    (940, "jump_apex.png"),
    (970, "jump_landing.png"),
];

impl MeshView {
    /// Queue a GLB (and optional base-color PNG) for display; parsed and
    /// uploaded during the next draw. Routing is automatic: playable rig →
    /// play mode, static material-bearing GLB → PBR, anything else → statue.
    pub fn set_model_bytes(&mut self, cx: &mut Cx, glb: Vec<u8>, png: Option<Vec<u8>>) {
        self.set_model_bytes_ao(cx, glb, png, None, None);
    }

    /// Same as [`set_model_bytes`], plus the offline AO pair the importer
    /// writes beside the library GLB. Without these the statue path is unlit
    /// even when thumbs already used the bake.
    pub fn set_model_bytes_ao(
        &mut self,
        cx: &mut Cx,
        glb: Vec<u8>,
        png: Option<Vec<u8>>,
        aomesh: Option<Vec<u8>>,
        ao_png: Option<Vec<u8>>,
    ) {
        let n = glb.len();
        self.pending = Some(PendingModel {
            glb,
            png,
            aomesh,
            ao_png,
        });
        self.walk_cam = None;
        self.extra_instances.clear();
        self.placed_sprites.clear();
        self.pending_placed_models.clear();
        self.pbr.controls = PbrDisplayControls::default();
        self.reset_studio_camera();
        self.status = format!("loading GLB ({n} bytes)…");
        self.area.redraw(cx);
    }

    /// World walk leaves `cam_distance` at 0.5 and a look-down pitch; a
    /// Kenney prop then sits off-frame (empty slab). Restore the statue rig.
    fn reset_studio_camera(&mut self) {
        self.orbit_yaw = 0.6;
        self.orbit_pitch = -0.22;
        self.look.target = vec3f(0.0, 0.9, 0.0);
        self.look.distance = 4.2;
        self.look.fov = 45.0;
        
    }

    /// Catalog billboards in a world walk. Same payload as BillboardView:
    /// a `.billboard` manifest (states + frames) or a single PNG.
    pub fn set_placed_sprites(
        &mut self,
        cx: &mut Cx,
        sprites: Vec<(Vec3f, f32, f32, std::path::PathBuf)>,
    ) {
        self.placed_sprites.clear();
        for (pos, width, height, path) in sprites {
            let frames = load_sprite_frames(cx, &path);
            if frames.is_empty() {
                continue;
            }
            self.placed_sprites.push(PlacedSprite {
                frames,
                fps: 8.0,
                pos,
                width: width.max(0.04),
                height: height.max(0.04),
            });
        }
        if !self.placed_sprites.is_empty() {
            self.area.redraw(cx);
        }
    }

    /// Catalog GLBs (Q3 items / misc_model, Quake .mdl) in a world walk.
    /// Loaded after the map statue so `load_statue` cannot wipe them.
    pub fn set_placed_models(
        &mut self,
        cx: &mut Cx,
        models: Vec<(Vec3f, f32, std::path::PathBuf)>,
    ) {
        self.pending_placed_models = models
            .into_iter()
            .map(|(pos, yaw, path)| PlacedModel { pos, yaw, path })
            .collect();
        if self.instance.is_some() {
            self.apply_placed_models(cx);
        }
        if !self.pending_placed_models.is_empty() {
            self.area.redraw(cx);
        }
    }

    fn apply_placed_models(&mut self, cx: &mut Cx) {
        self.extra_instances.clear();
        for (i, spec) in self.pending_placed_models.iter().enumerate() {
            let Ok(glb) = std::fs::read(&spec.path) else {
                continue;
            };
            if !glb.starts_with(b"glTF") {
                continue;
            }
            let id = format!("aiapp/place-{}-{i}", self.generation);
            let png = extract_base_color(&glb);
            if self
                .renderer
                .load_model(cx, &id, &glb, png.as_deref())
                .is_err()
            {
                continue;
            }
            self.extra_instances.push(ModelInstance {
                model: id,
                transform: trs_yaw(spec.pos, spec.yaw, 1.0),
                dynamic: true,
                depth_order: 0.0,
            });
        }
    }

    /// First-person World preview. Call AFTER [`set_model_bytes`] so the
    /// pending load sees the walk flag and skips AABB fit.
    pub fn enable_walk(&mut self, cx: &mut Cx, spawn: ([f32; 3], f32, f32)) {
        let pitch = spawn.2.clamp(-1.2, 1.2);
        self.walk_cam = Some(WalkCam {
            eye: vec3f(spawn.0[0], spawn.0[1], spawn.0[2]),
            yaw: spawn.1,
            pitch,
        });
        self.orbit_yaw = spawn.1;
        self.orbit_pitch = pitch;
        self.apply_walk_camera();
        self.pbr.controls.ambient = 0.55;
        self.pbr.controls.env_intensity = 0.9;
        self.pump_parked = false;
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    fn apply_walk_camera(&mut self) {
        let Some(w) = self.walk_cam else {
            return;
        };
        let (target, distance) = walk_rig(w.eye, w.yaw, w.pitch);
        self.look.target = target;
        self.look.distance = distance;
        self.look.fov = 75.0;
        self.orbit_yaw = w.yaw;
        self.orbit_pitch = w.pitch;
    }

    fn sync_stage(&mut self) {
        if self.walk_cam.is_some() {
            self.show_ground = false;
            self.apply_walk_camera();
        } else if !self.show_ground && self.world_built {
            self.world_built = false;
            self.ensure_world();
        }
    }

    fn run_walk_tick(&mut self) {
        let input = self.play_input();
        let Some(w) = self.walk_cam.as_mut() else {
            return;
        };
        let speed = if input.run { 8.0 } else { 3.4 };
        let fwd = vec3f(w.yaw.sin(), 0.0, -w.yaw.cos());
        let right = vec3f(w.yaw.cos(), 0.0, w.yaw.sin());
        w.eye = w.eye + (fwd * input.axis_z + right * input.axis_x) * speed * TICK_DT;
        let down = |k: KeyCode| self.keys.contains(&k);
        if down(KeyCode::Space) || down(KeyCode::KeyE) {
            w.eye.y += speed * TICK_DT;
        }
        if down(KeyCode::KeyQ) || down(KeyCode::KeyC) {
            w.eye.y -= speed * TICK_DT;
        }
        self.apply_walk_camera();
    }

    /// Replace the PBR branch's lighting rig (key light direction/color/
    /// intensity, ambient fill, environment strength, exposure). Persists
    /// across model reloads; ignored by the statue and playable paths.
    #[allow(dead_code)] // handoff API: the authoring host wires this next
    pub fn set_pbr_controls(&mut self, cx: &mut Cx, controls: PbrDisplayControls) {
        self.pbr.controls = controls;
        self.area.redraw(cx);
    }

    #[allow(dead_code)] // handoff API: the authoring host wires this next
    pub fn pbr_controls(&self) -> PbrDisplayControls {
        self.pbr.controls
    }

    /// Light the PBR model with an equirect environment image (PNG/JPG
    /// bytes, in-memory). Applied at the next draw; replaces the procedural
    /// gradient cube until the next call.
    #[allow(dead_code)] // handoff API: the authoring host wires this next
    pub fn set_pbr_env_equirect(&mut self, cx: &mut Cx, bytes: Vec<u8>) {
        self.pbr.set_env_equirect(bytes);
        self.area.redraw(cx);
    }

    /// The PBR branch's honest load report (declared material roles, decode
    /// progress, environment source), None while another path is displayed.
    #[allow(dead_code)] // handoff API: the authoring host wires this next
    pub fn pbr_status(&self) -> Option<&PbrStatus> {
        self.pbr.status.as_ref()
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
        // Same fast-GPU contract as the sandbox F8 / settings toggle:
        // per-frame cascaded shadows, every dynamic caster in the sun.
        self.renderer
            .set_gpu_lightmap_mode(GpuLightmapMode::Realtime);
        // The fixed-step chain is armed by draw_walk once a character is
        // actually visible (pump_parked starts true), never here.
    }

    /// Ground slab (top at y=0) + sky; the mesh stands on it.
    fn ensure_world(&mut self) {
        if self.world_built {
            return;
        }
        self.world_built = true;
        self.show_ground = true;
        self.look.target = vec3f(0.0, 0.9, 0.0);
        self.look.distance = 4.2;
        self.look.fov = 45.0;
        self.status = "no mesh yet — run an image → mesh pipeline or load the sample".into();
    }

    fn load_pending(&mut self, cx: &mut CxDraw) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.generation += 1;
        self.instance = None;
        self.character = None;
        self.pbr.clear(&mut self.draw_pbr);
        // Priority order: a skinned GLB WITH clips is ALWAYS the playable
        // character (PBR must never regress playback); a static GLB whose
        // materials reference textures renders as real materials; anything
        // else (bare mesh, factors-only, rig without clips) is a statue.
        match SkinnedModel::parse_glb(&pending.glb) {
            Ok(model) if is_playable_skin(&model) => {
                self.load_character(cx, model, &pending.glb, pending.png)
            }
            _ if self.walk_cam.is_some() => {
                // Walk maps are 100k–2M tris. DrawPbr of a single that-size
                // object draws nothing (sky only). The statue path uploads
                // the atlas as base color at authored metres.
                self.load_statue(cx, pending)
            }
            _ => {
                // DrawPbr in this pass currently composites as empty sky
                // (worlds AND small Kenney props). Renderer is what
                // the thumbnailer already proves; use it for the hero mesh.
                let mut pending = pending;
                if pending.png.is_none() {
                    pending.png = extract_base_color(&pending.glb);
                }
                self.load_statue(cx, pending)
            }
        }
        self.sync_stage();
        self.area.redraw(cx);
    }

    /// The static PBR path: make the classified GLB resident through the
    /// engine glTF renderer. Failure (external-URI textures, non-finite
    /// geometry, decode errors) falls back to the base-color statue with
    /// the reason in the status line — something always renders, and it
    /// never pretends to be what it is not.
    fn load_pbr(
        &mut self,
        cx: &mut CxDraw,
        gltf: pbr_preview::MaterialBearingGltf,
        glb: &[u8],
        png: Option<Vec<u8>>,
    ) {
        match self.pbr.load(&mut self.draw_pbr, cx, gltf, self.generation) {
            Ok(()) => {
                if self.walk_cam.is_some() {
                    self.pbr.set_fit(Mat4f::identity());
                    self.status = format!("{} · WASD walk, drag look", self.pbr.summary());
                } else {
                    self.status = self.pbr.summary();
                }
            }
            Err(e) => {
                self.load_statue(
                    cx,
                    PendingModel {
                        glb: glb.to_vec(),
                        png,
                        aomesh: None,
                        ao_png: None,
                    },
                );
                self.status = format!("PBR load failed ({e}); base-color statue — {}", self.status);
            }
        }
    }

    /// The playable path: resolve clips by name, extract the embedded base
    /// color, measure the rest pose, make the rig resident (flat AO — a
    /// generated 100k+-vert bake would take minutes), spawn the controller.
    fn load_character(
        &mut self,
        cx: &mut Cx,
        model: SkinnedModel,
        glb: &[u8],
        png: Option<Vec<u8>>,
    ) {
        // Clip per locomotion state; a miss falls back to the idle pick so a
        // partial rig still plays (and idle falls back to clip 0).
        let idle = model
            .clip_index_any(LocoState::Idle.clip_candidates())
            .unwrap_or(0);
        let walk = model
            .clip_index_any(LocoState::Walk.clip_candidates())
            .unwrap_or(idle);
        // `run` was added after the original idle/walk/jump contract. Older
        // artifacts remain playable by cadence-adjusting their walk clip.
        let run = model
            .clip_index_any(LocoState::Run.clip_candidates())
            .unwrap_or(walk);
        let jump = model.clip_index_any(LocoState::Jump.clip_candidates());
        let has_jump_clip = jump.is_some();
        let jump = jump.unwrap_or(idle);
        let clips = [idle, walk, run, jump];
        // Measure the rest pose the exact way the GPU will pose it.
        let Some((rest_min_y, rest_max_y)) = ({
            let rest = model.rest_pose();
            let mut palette = Vec::new();
            model.palette(&rest, &mut palette);
            let mut packed = Vec::new();
            model.skin_to_packed(&palette, &mut packed);
            packed_y_bounds(&packed)
        }) else {
            self.load_statue_mesh(cx, glb, png);
            self.status = "character rejected: non-finite or empty rest-pose skin".into();
            return;
        };
        let rest_height = rest_max_y - rest_min_y;
        if !rest_height.is_finite() || rest_height <= 0.01 {
            self.load_statue_mesh(cx, glb, png);
            self.status = format!(
                "character rejected: invalid rest-pose height {rest_height:?}"
            );
            return;
        }
        let landing_audit = if has_jump_clip {
            audit_landing_contact(&model, &clips, rest_min_y, rest_height)
        } else {
            LandingContactAudit::default()
        };
        if !landing_contact_gate_passes(has_jump_clip, landing_audit) {
            self.load_statue_mesh(cx, glb, png);
            self.status = format!(
                "character rejected: landing contact gate failed ({} samples, {} invalid, correction {:.1}%H, gap {:.2}%H, penetration {:.2}%H)",
                landing_audit.samples,
                landing_audit.invalid_samples,
                landing_audit.max_correction_fraction * 100.0,
                landing_audit.max_corrected_gap_fraction * 100.0,
                landing_audit.max_corrected_penetration_fraction * 100.0,
            );
            return;
        }
        if has_jump_clip {
            log!(
                "mesh_view: landing contact audit passed: {} samples, raw gap {:.2}%H, raw penetration {:.2}%H, correction {:.2}%H",
                landing_audit.samples,
                landing_audit.max_uncorrected_gap_fraction * 100.0,
                landing_audit.max_uncorrected_penetration_fraction * 100.0,
                landing_audit.max_correction_fraction * 100.0,
            );
        }
        let scale = 1.75 / rest_height;
        let lift = -rest_min_y * scale;
        let texture = image_texture(cx, png.or_else(|| extract_base_color(glb)));
        let rig = self.generation;
        if !self.renderer.skin_rig_loaded(rig) {
            self.renderer.upload_skin_rig(cx, rig, model.rest_gpu_flat());
        }
        let names: Vec<&str> = model.clips.iter().map(|c| c.name.as_str()).collect();
        self.status = format!(
            "character: {} joints, {} verts, {} states {:?}",
            model.joint_count(),
            model.vertex_count(),
            names.len(),
            names
        );
        self.character = Some(Character {
            model,
            texture,
            clips,
            slot: 0,
            clip_time: 0.0,
            prev_slot: 0,
            prev_time: 0.0,
            fade: 1.0,
            pose: PoseBuffer::new(),
            prev_pose: PoseBuffer::new(),
            blended: PoseBuffer::new(),
            loop_scratch: PoseBuffer::new(),
            palette: Vec::new(),
            scale,
            lift,
            rest_min_y,
            rest_height,
            landing_contact_lift: 0.0,
            landing_contact_packed: Vec::new(),
            has_jump_clip,
            loco: Locomotion::default(),
            air_ticks: 0,
            showcase: false,
            showcase_clip: 0,
            asset_yaw: 0.0,
            rig,
        });
    }

    fn load_statue_mesh(&mut self, cx: &mut Cx, glb: &[u8], png: Option<Vec<u8>>) {
        self.load_statue(
            cx,
            PendingModel {
                glb: glb.to_vec(),
                png,
                aomesh: None,
                ao_png: None,
            },
        );
    }

    fn load_statue(&mut self, cx: &mut Cx, pending: PendingModel) {
        let id = format!("aiapp/gen-{}", self.generation);
        let has_ao = pending.aomesh.is_some() && pending.ao_png.is_some();
        self.extra_instances.clear();
        // Same Renderer load as ThumbnailRenderer::prepare_statue.
        // Multi-tile worlds split inside parse_glb so one instance draws
        // every embedded PNG — do not re-split here.
        match self.renderer.load_model_with_ao(
            cx,
            &id,
            &pending.glb,
            pending.png.as_deref(),
            pending.aomesh.as_deref(),
            pending.ao_png.as_deref(),
        ) {
            Ok(triangles) => {
                let (min, max) = self
                    .renderer
                    .model_bounds(&id)
                    .unwrap_or((vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)));
                // Uniform fit by the LARGEST dimension: normalizing by height
                // alone blew wide/flat objects (a boat's height is its
                // smallest axis) up far past the view. Aspect is always 1:1 —
                // only the fit factor changes.
                let ao_note = if has_ao { " · AO" } else { "" };
                if self.walk_cam.is_some() {
                    // World metres as authored — do not shrink a map into a statue.
                    // Dynamic so Realtime CSM treats the map as a sun caster.
                    self.instance = Some(ModelInstance {
                        model: id,
                        transform: Mat4f::identity(),
                        dynamic: true,
                        depth_order: 0.0,
                    });
                    self.status = format!("{triangles} tris{ao_note} · CSM · WASD walk, drag look");
                } else {
                    let size = max - min;
                    let extent = size.x.max(size.y).max(size.z).max(0.01);
                    let scale = 1.75 / extent;
                    // Stand on the ground plane, centered in X/Z.
                    let center = (min + max) * 0.5;
                    self.instance = Some(ModelInstance {
                        model: id,
                        transform: trs_yaw(
                            vec3f(-center.x * scale, -min.y * scale, -center.z * scale),
                            0.35,
                            scale,
                        ),
                        // Realtime CSM only collects `dynamic` movers.
                        dynamic: true,
                        depth_order: 0.0,
                    });
                    self.status =
                        format!("{triangles} tris, fit scale {scale:.2}{ao_note} · CSM");
                }
            }
            Err(e) => {
                self.instance = None;
                self.status = format!("GLB failed to load: {e}");
            }
        }
        if self.walk_cam.is_some() && !self.pending_placed_models.is_empty() {
            self.apply_placed_models(cx);
        }
    }

    /// Fold the held-key set into the play input axes (sandbox idiom).
    fn play_input(&self) -> PlayInput {
        let down = |k: KeyCode| self.keys.contains(&k);
        let axis = |pos: bool, neg: bool| pos as i8 as f32 - neg as i8 as f32;
        PlayInput {
            axis_x: axis(
                down(KeyCode::ArrowRight) || down(KeyCode::KeyD),
                down(KeyCode::ArrowLeft) || down(KeyCode::KeyA),
            ),
            axis_z: axis(
                down(KeyCode::ArrowUp) || down(KeyCode::KeyW),
                down(KeyCode::ArrowDown) || down(KeyCode::KeyS),
            ),
            run: down(KeyCode::Shift),
            jump: down(KeyCode::Space),
        }
    }

    fn run_tick(&mut self, cx: &mut Cx) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = self.capture_dir.clone() {
            let dir = std::path::Path::new(&dir);
            let _ = std::fs::create_dir_all(dir);
            if !self.capture_initialized {
                // A reused evidence directory must not make the completion
                // gate accept stale frames from an older executable/run.
                for (_, name) in CAPTURES {
                    let _ = std::fs::remove_file(dir.join(name));
                }
                self.capture_initialized = true;
            }
            if let Some(name) = self.capture_pending.clone() {
                if dir.join(&name).exists() {
                    self.capture_pending = None;
                } else {
                    // Freeze controller + clip clocks on the requested pose
                    // until Metal's asynchronous readback/PNG write lands.
                    return;
                }
            }
        }

        self.tick += 1;
        let input = match &self.capture_dir {
            #[cfg(not(target_arch = "wasm32"))]
            Some(_) => capture_input(self.tick),
            _ => self.play_input(),
        };
        let cam_yaw = self.orbit_yaw;
        if let Some(c) = self.character.as_mut() {
            c.tick(&input, cam_yaw);
            // Follow camera: orbit stays mouse-driven, the target rides the
            // character (head height at its normalized scale).
            self.look.target = vec3f(
                c.loco.pos[0],
                c.loco.pos[1] + 0.9,
                c.loco.pos[2],
            );
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = self.capture_dir.clone() {
            if self.character.is_some() {
                if let Some((at, name)) = CAPTURES
                    .iter()
                    .copied()
                    .find(|(at, _)| self.tick >= *at && !self.captures_taken.contains(at))
                {
                    self.captures_taken.push(at);
                    self.capture_pending = Some(name.to_string());
                    cx.capture_next_frame_to_file(std::path::Path::new(&dir).join(name));
                }
                // Exit only once every PNG is really on disk (the capture
                // readback lands frames after the request).
                if self.tick >= 990 && self.capture_pending.is_none() {
                    let done = CAPTURES
                        .iter()
                        .all(|(_, name)| std::path::Path::new(&dir).join(name).exists());
                    if done {
                        log!("mesh_view: play captures written to {dir}, exiting");
                        std::process::exit(0);
                    }
                }
            }
        }
    }
}

impl WidgetNode for MeshView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for MeshView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // PBR texture decodes are asynchronous and commit through the event
        // stream; a newly arrived map must repaint the pane (and its HUD
        // decode counter).
        if self.pbr.handle_event(cx, event) {
            self.area.redraw(cx);
        }

        // Fixed-step tick (runs the play controller + capture schedule).
        if self.next_frame.is_event(event).is_some() {
            // Hidden test, as a redraw→draw handshake: a step that requested
            // a redraw expects the widget to actually draw before the next
            // NextFrame. If that draw never came, the viewer is hidden (page
            // flipped away / another surface) — park instead of re-arming,
            // and let draw_walk re-arm on the next visible frame. Only
            // redraw-requesting steps set the expectation, so displays
            // pacing faster than TICK_DT can never false-park a visible
            // viewer on their zero-step events.
            let hidden = self.awaiting_draw && !std::mem::take(&mut self.drawn_since_tick);
            self.awaiting_draw = false;
            if hidden || (self.character.is_none() && self.walk_cam.is_none()) {
                self.pump_parked = true;
                self.last_time = None;
                self.time_accum = 0.0;
            } else {
                let time = cx.seconds_since_app_start();
                let last = self.last_time.replace(time).unwrap_or(time);
                self.time_accum += (time - last).min(0.25);
                let mut ticked = false;
                while self.time_accum >= TICK_DT as f64 {
                    self.time_accum -= TICK_DT as f64;
                    if self.walk_cam.is_some() {
                        self.run_walk_tick();
                    } else {
                        self.run_tick(cx);
                    }
                    ticked = true;
                }
                if ticked {
                    self.area.redraw(cx);
                    self.awaiting_draw = true;
                }
                self.next_frame = cx.new_next_frame();
            }
        }

        // Play-mode keys: gated on click-focus (the app's prompt TextInputs
        // see the same raw KeyDown stream); KeyUp always clears.
        match event {
            Event::KeyDown(ke) if self.focused => {
                if !self.keys.contains(&ke.key_code) {
                    self.keys.push(ke.key_code);
                }
                // PBR viewing controls (arrows light, -/= exposure, [ ] env,
                // 0 reset). Only when no character is playable, so play-mode
                // input is never shadowed; repeats pass through for smooth
                // orbiting.
                if self.character.is_none()
                    && self.walk_cam.is_none()
                    && self.pbr.control_key(ke.key_code)
                {
                    self.area.redraw(cx);
                }
                if !ke.is_repeat {
                    if let Some(c) = self.character.as_mut() {
                        match ke.key_code {
                            KeyCode::KeyC => {
                                c.showcase = !c.showcase;
                                if c.showcase {
                                    c.showcase_clip = c.clips[c.slot];
                                    c.clip_time = 0.0;
                                } else {
                                    c.loco = Locomotion::default();
                                }
                            }
                            KeyCode::KeyF => {
                                c.asset_yaw = if c.asset_yaw == 0.0 {
                                    std::f32::consts::PI
                                } else {
                                    0.0
                                };
                            }
                            _ => {}
                        }
                    }
                }
            }
            Event::KeyUp(ke) => {
                self.keys.retain(|k| *k != ke.key_code);
            }
            _ => {}
        }

        // Raw mouse only while orbit-dragging: the pointer can leave the
        // pane and must keep moving the camera. Never listen to raw
        // MouseDown — that steals clicks from dropdowns and other widgets.
        if self.orbit_last_abs.is_some() {
            match event {
                Event::MouseMove(me) => {
                    if let Some(last) = self.orbit_last_abs {
                        let delta = me.abs - last;
                        self.orbit_yaw -= delta.x as f32 * 0.01;
                        self.orbit_pitch =
                            (self.orbit_pitch + delta.y as f32 * 0.01).clamp(-1.45, 1.45);
                        if let Some(w) = self.walk_cam.as_mut() {
                            w.yaw = self.orbit_yaw;
                            w.pitch = self.orbit_pitch;
                        }
                        self.apply_walk_camera();
                        self.orbit_last_abs = Some(me.abs);
                        self.area.redraw(cx);
                    }
                }
                Event::MouseUp(me) if me.button.is_primary() => {
                    self.orbit_last_abs = None;
                }
                _ => {}
            }
        }

        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.orbit_last_abs = Some(fe.abs);
                // Click claims key focus for play mode (and takes it away
                // from the prompt TextInput so WASD doesn't type).
                self.focused = true;
                cx.set_key_focus(self.area);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerScroll(se) if self.walk_cam.is_none() => {
                let axis = if se.scroll.y.abs() > f64::EPSILON {
                    se.scroll.y
                } else {
                    se.scroll.x
                };
                if axis.abs() > f64::EPSILON {
                    let factor = if axis > 0.0 { 1.0 / 0.92 } else { 0.92 };
                    self.look.distance =
                        (self.look.distance * factor).clamp(1.0, 30.0);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::KeyFocusLost(_) => {
                self.focused = false;
                self.keys.clear();
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }

        self.ensure_initialized(cx.cx);
        self.ensure_world();
        self.load_pending(cx.cx);
        self.sync_stage();
        // Being drawn is what keeps (or brings) the fixed-step chain alive;
        // a parked chain re-arms here the moment a character or walk map
        // is visible.
        self.drawn_since_tick = true;
        if self.pump_parked && (self.character.is_some() || self.walk_cam.is_some()) {
            self.pump_parked = false;
            self.next_frame = cx.cx.new_next_frame();
        }
        self.view_rect = rect;
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        self.look.yaw = self.orbit_yaw;
        self.look.pitch = self.orbit_pitch;
        // Orbit zoom must retighten cascade 0 around the look-at so
        // shadow texels stay ~1 screen pixel. Walk keeps the 80 m ladder.
        if self.walk_cam.is_some() {
            self.renderer.set_csm_focus_distance(None);
        } else {
            self.renderer
                .set_csm_focus_distance(Some(self.look.distance));
        }
        let scene_state = preview_scene_state(self.look, rect, cx.time());
        if let Some(scene_state) = scene_state {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let now = cx.time();
            let cx3d = &mut Cx3d::new(cx.cx);
            let mut statue = match &self.instance {
                Some(inst) => vec![inst.clone()],
                None => Vec::new(),
            };
            statue.extend(self.extra_instances.iter().cloned());
            self.renderer.set_models(statue);
            // Pose the playable character for this frame.
            let mut items = Vec::new();
            let mut textures: Vec<&Texture> = Vec::new();
            if let Some(c) = self.character.as_mut() {
                c.pose_palette();
                let gait_slot = if c.slot == Character::slot_for(LocoState::Run) {
                    Character::slot_for(LocoState::Run)
                } else {
                    Character::slot_for(LocoState::Walk)
                };
                let gait_dur = c.model.clips[c.clips[gait_slot]].duration.max(0.01);
                let gait_active = matches!(c.loco.state, LocoState::Walk | LocoState::Run);
                let transform = trs_yaw(
                    vec3f(
                        c.loco.pos[0],
                        c.loco.pos[1] + c.lift + c.landing_contact_lift,
                        c.loco.pos[2],
                    ),
                    c.loco.yaw + c.asset_yaw,
                    c.scale,
                );
                items.push(
                    SkinnedDraw::new(1, c.rig, transform)
                        .with_texture(0)
                        .with_bounds(c.model.posed_bounds(&c.palette))
                        .with_palette(c.palette.clone())
                        .with_gait(
                            (c.clip_time / gait_dur).fract(),
                            if gait_active { 1.0 } else { 0.0 },
                        ),
                );
                textures.push(&c.texture);
            }
            let batch = if items.is_empty() {
                None
            } else {
                Some(SkinnedBatch {
                    skinned: &mut self.draw_skinned,
                    textures,
                    items,
                })
            };
            let screen_instances: Vec<ScreenInstance> = self
                .placed_sprites
                .iter()
                .filter_map(|s| {
                    let n = s.frames.len();
                    if n == 0 {
                        return None;
                    }
                    let i = if n == 1 {
                        0
                    } else {
                        ((now * s.fps.max(1.0) as f64).floor() as usize) % n
                    };
                    Some(ScreenInstance {
                        texture: s.frames[i].clone(),
                        pos: vec4(s.pos.x, s.pos.y, s.pos.z, self.orbit_yaw),
                        size: vec4(s.width, s.height, 0.0, 0.0),
                    })
                })
                .collect();
            let mut draws = SceneDraws {
                cube: &mut self.draw_cube,
                alpha: &mut self.draw_alpha,
                sky: &mut self.draw_sky,
                sky_analytic: None,
                terrain: &mut self.draw_terrain,
                shadow: Some(&mut self.draw_shadow),
                shadow_sdf: Some(&mut self.draw_shadow_sdf),
                firework: None,
                flare: None,
                water: None,
                screen: Some(&mut self.draw_sprite),
                screen_instances: &screen_instances,
                view_model: None,
            };
            let stage = if self.show_ground {
                PreviewStage::statue()
            } else {
                PreviewStage::empty()
            };
            self.renderer.draw_preview(
                cx3d,
                &mut self.draw_list,
                &mut draws,
                self.look,
                stage,
                scene_state,
                batch,
                Some(&mut self.draw_models),
            );
            // The static PBR hero draws AFTER the world, opaque and
            // depth-tested, so it composes against slab and sky in the depth
            // buffer. A playable character never reaches here — the skinned
            // lane above stays the only character path.
            if self.character.is_none() {
                self.pbr.draw(&mut self.draw_pbr, cx3d);
            }
        }
        cx.end_pass(&self.pass);

        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);

        let help = match &self.character {
            Some(c) if c.showcase => {
                format!(
                    "clip: {} (showcase)   C play mode   drag orbit, wheel zoom",
                    c.current_clip_name()
                )
            }
            Some(c) => {
                let focus = if self.focused {
                    ""
                } else {
                    "click pane, then "
                };
                format!(
                    "{} [{}]   {focus}WASD/arrows move, Shift run, Space jump, C showcase, F flip facing",
                    self.status,
                    c.current_clip_name(),
                )
            }
            None if self.walk_cam.is_some() => {
                let focus = if self.focused {
                    ""
                } else {
                    "click pane, then "
                };
                format!(
                    "{}   {focus}WASD/arrows walk, Shift run, E/Space up, Q/C down, drag look",
                    self.status
                )
            }
            None => match self.pbr.hud_line() {
                Some(line) => format!("{}   {line}", self.status),
                None => format!("{}   drag orbit, wheel zoom", self.status),
            },
        };
        self.draw_hud.draw_abs(
            cx,
            dvec2(rect.pos.x + 10.0, rect.pos.y + rect.size.y - 22.0),
            &help,
        );
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_rig_puts_the_lens_at_the_eye() {
        let eye = vec3f(-6.5, 0.64, 4.0);
        let yaw = std::f32::consts::FRAC_PI_2;
        let (target, distance) = walk_rig(eye, yaw, 0.0);
        assert!((distance - 0.5).abs() < 1e-5);
        let fwd = walk_forward(yaw, 0.0);
        // yaw π/2 looks +X
        assert!((fwd.x - 1.0).abs() < 1e-5);
        let cam = target - fwd * distance;
        assert!((cam.x - eye.x).abs() < 1e-4);
        assert!((cam.y - eye.y).abs() < 1e-4);
        assert!((cam.z - eye.z).abs() < 1e-4);
    }

    #[test]
    fn thumbnail_classifier_routes_static_parse_success_to_statue() {
        assert!(!is_playable_skin_shape(0, 0));
        assert!(!is_playable_skin_shape(0, 1));
        assert!(!is_playable_skin_shape(32, 0));
        assert!(is_playable_skin_shape(32, 1));
        assert!(!is_playable_skin_shape(257, 1));
    }

    #[test]
    fn locomotion_slots_are_stable_and_every_gait_is_loop_sampled() {
        assert_eq!(Character::slot_for(LocoState::Idle), 0);
        assert_eq!(Character::slot_for(LocoState::Walk), 1);
        assert_eq!(Character::slot_for(LocoState::Run), 2);
        assert_eq!(Character::slot_for(LocoState::Jump), 3);
        assert!(Character::slot_loops(Character::slot_for(LocoState::Idle)));
        assert!(Character::slot_loops(Character::slot_for(LocoState::Walk)));
        assert!(Character::slot_loops(Character::slot_for(LocoState::Run)));
        assert!(!Character::slot_loops(Character::slot_for(LocoState::Jump)));
        assert!(LOOP_BLEND_SECONDS > 0.0);
    }

    #[test]
    fn state_crossfade_reaches_full_weight_on_schedule() {
        let ticks = (FADE_SECONDS / TICK_DT).ceil() as usize;
        let mut fade = 0.0;
        for _ in 0..ticks {
            fade = Character::advance_fade(fade);
        }
        assert!(fade >= 0.999, "{ticks} ticks only reached {fade}");
        assert_eq!(Character::advance_fade(fade), 1.0);
    }

    #[test]
    fn optional_run_clip_uses_walk_with_ground_speed_cadence() {
        // Same indices model a legacy idle/walk/jump artifact whose run slot
        // was resolved to walk. A distinct run clip stays at authored speed.
        let mut character_clips = [0, 1, 1, 2];
        assert!((clip_playback_rate(
            &character_clips,
            Character::slot_for(LocoState::Run),
        ) - makepad_render::play::RUN_SPEED
            / makepad_render::play::WALK_SPEED)
            .abs()
            < f32::EPSILON);
        character_clips[Character::slot_for(LocoState::Run)] = 3;
        assert_eq!(
            clip_playback_rate(&character_clips, Character::slot_for(LocoState::Run)),
            1.0
        );
    }

    #[test]
    fn landing_contact_closes_hover_and_penetration_without_double_scaling() {
        // Model a representative six-percent shoe gap in source coordinates
        // and prove the correction is applied once when the host composes
        // its scale.
        let rest_min_y = -0.5;
        let height = 1.0;
        let scale = 1.75;
        let hover = height * 0.06;
        let adjustment =
            landing_contact_adjustment(rest_min_y, rest_min_y + hover, height).unwrap();
        assert!(adjustment.passes());
        assert!((adjustment.offset_model_y * scale + hover * scale).abs() < 1.0e-6);
        let lift = -rest_min_y * scale;
        let posed_world_low =
            lift + adjustment.offset_model_y * scale + (rest_min_y + hover) * scale;
        assert!(posed_world_low.abs() < 1.0e-6, "world contact {posed_world_low}");
        assert!(adjustment.corrected_gap_fraction <= LANDING_CONTACT_TOLERANCE_FRACTION);
        assert!(
            adjustment.corrected_penetration_fraction
                <= LANDING_CONTACT_TOLERANCE_FRACTION
        );

        // The same generic operation raises a penetrating landing. Every
        // corrected contact plane remains at zero, so there is no one-frame
        // downward penetration spike anywhere in the fade.
        for posed_delta in [0.08, 0.04, 0.01, -0.01, -0.04] {
            let adjustment =
                landing_contact_adjustment(rest_min_y, rest_min_y + posed_delta, height)
                    .unwrap();
            assert!(adjustment.passes(), "delta {posed_delta}: {adjustment:?}");
            let corrected = posed_delta + adjustment.offset_model_y;
            assert!(corrected.abs() <= height * LANDING_CONTACT_TOLERANCE_FRACTION);
            assert!(corrected >= -height * LANDING_CONTACT_TOLERANCE_FRACTION);
        }
    }

    #[test]
    fn landing_contact_gate_rejects_implausible_whole_character_shift() {
        let adjustment = landing_contact_adjustment(-0.5, -0.25, 1.0).unwrap();
        assert!(adjustment.correction_fraction > MAX_LANDING_CONTACT_CORRECTION_FRACTION);
        assert!(!adjustment.passes());
    }

    #[test]
    fn legacy_rig_without_jump_bypasses_landing_only_gate() {
        // The empty audit is deliberately a failure when an authored jump
        // exists, but must not reject an older idle/walk-only playable rig.
        let empty = LandingContactAudit::default();
        assert!(!empty.passes());
        assert!(landing_contact_gate_passes(false, empty));
        assert!(!landing_contact_gate_passes(true, empty));
    }

    #[test]
    fn unrigged_sample_never_enters_playable_landing_gate() {
        let Ok(bytes) = std::fs::read(crate::repo_path(
            "apps/asset-ui/resources/test/character_retex.glb",
        )) else {
            return;
        };
        assert!(
            SkinnedModel::parse_glb(&bytes).is_err(),
            "load_pending must keep this asset off the playable branch"
        );
    }

    #[test]
    fn static_material_bearing_sample_routes_to_the_pbr_branch() {
        // The bundled retextured sample is exactly the shape the pipeline
        // produces: static, self-contained, materials referencing an
        // embedded texture. load_pending's route: not playable → material-
        // bearing → PBR.
        let Ok(bytes) = std::fs::read(crate::repo_path(
            "apps/asset-ui/resources/test/character_retex.glb",
        )) else {
            return;
        };
        assert!(SkinnedModel::parse_glb(&bytes).is_err());
        assert!(
            pbr_preview::parse_material_bearing_glb(&bytes).is_some(),
            "embedded-material GLB must classify for the PBR branch"
        );
    }

    #[test]
    fn playable_classification_outranks_pbr_classification() {
        // The animated sample carries materials TOO — both classifiers can
        // match. load_pending checks the playable shape first, so a rigged
        // character always keeps its play mode; this pins the sample really
        // exercising that precedence rather than passing vacuously.
        let Ok(bytes) = std::fs::read(crate::repo_path(
            "apps/asset-ui/resources/test/character_anim.glb",
        )) else {
            return;
        };
        let model = SkinnedModel::parse_glb(&bytes).expect("parse bundled generated character");
        assert!(is_playable_skin(&model), "playable branch must win");
    }

    #[test]
    fn bundled_generated_rig_passes_every_grounded_landing_fade() {
        let Ok(bytes) = std::fs::read(crate::repo_path(
            "apps/asset-ui/resources/test/character_anim.glb",
        )) else {
            return;
        };
        let model = SkinnedModel::parse_glb(&bytes).expect("parse bundled generated character");
        let idle = model
            .clip_index_any(LocoState::Idle.clip_candidates())
            .expect("idle clip");
        let walk = model
            .clip_index_any(LocoState::Walk.clip_candidates())
            .expect("walk clip");
        let run = model
            .clip_index_any(LocoState::Run.clip_candidates())
            .unwrap_or(walk);
        let jump = model
            .clip_index_any(LocoState::Jump.clip_candidates())
            .expect("jump clip");
        let clips = [idle, walk, run, jump];
        let rest = model.rest_pose();
        let mut palette = Vec::new();
        let mut packed = Vec::new();
        model.palette(&rest, &mut palette);
        model.skin_to_packed(&palette, &mut packed);
        let (min_y, max_y) = packed_y_bounds(&packed).expect("finite rest bounds");
        let audit = audit_landing_contact(&model, &clips, min_y, max_y - min_y);
        assert!(audit.samples > 0);
        assert_eq!(audit.invalid_samples, 0);
        assert!(
            audit.max_corrected_gap_fraction <= LANDING_CONTACT_TOLERANCE_FRACTION,
            "first-ground/fade hover: {audit:?}"
        );
        assert!(
            audit.max_corrected_penetration_fraction <= LANDING_CONTACT_TOLERANCE_FRACTION,
            "landing penetration spike: {audit:?}"
        );
        assert!(audit.passes(), "landing contact hard gate: {audit:?}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn evidence_script_exercises_run_then_sprinting_jump() {
        assert!(capture_input(420).run);
        assert!(!capture_input(420).jump);
        assert!(capture_input(878).run);
        assert!(!capture_input(878).jump);
        assert!(capture_input(905).run);
        assert!(capture_input(905).jump);
        assert!(!capture_input(970).jump);
    }
}
