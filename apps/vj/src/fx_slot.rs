//! EFFECT SLOTS — the center-console home of the vjeffect content category.
//!
//! Three slots sit above the crossfader: EFFECT A | TRANSITION | EFFECT B.
//! Each holds one `vjeffect` asset from the browse grid: click a slot to ARM
//! it and the next FX-tile click loads there; an unarmed FX-tile click lands
//! on the STANDBY deck's effect slot (content clicks keep cueing the decks —
//! an effect never displaces a playing clip; SHIFT-click is the explicit
//! effect-AS-CONTENT cue). The slot type law is strict: A/B take any
//! vjeffect, TRANSITION only transition-tagged ones, and a wrong-type click
//! while armed FLASHES a refusal instead of accepting or cueing.
//! A channel slot runs its effect as an EFFECT PASS over that
//! deck's content (deck texture → `input0`, effect output replaces the deck's
//! contribution to the program); on an empty deck the effect runs standalone,
//! which makes every generator engine playable content on its own. The
//! TRANSITION slot engages only while the crossfader travels (hand or
//! AUTOFADE): the A/B mix is pre-composited offscreen with the SAME
//! `DrawProgram` shader the program uses, handed to the effect as `input0`,
//! and the effect's output is dissolved over the program by
//! `triangle(program_mix)` — nothing at the fader's ends, everything
//! mid-fade, so engagement can never pop.
//!
//! Three widgets + one pure model:
//!   * [`FxSlots`] — the assignment/arm/knob state machine (tested below).
//!   * [`VjFxSlotHost`] — the offscreen render host (a 4x4 in the status bar,
//!     exactly the `VjFxThumbs`/mesh-slot idiom): owns a slot-mode
//!     [`VjFxView`], the transition premix pass, and samples its output into
//!     its tiny rect so the pass chain is a frame dependency.
//!   * [`VjFxSlotTile`] — the designed slot tile in the mixer column: live
//!     effect preview (the host's output texture, sampled for free), name,
//!     armed ring, bypass dim, engage meter, and an inviting empty state.
//!
//! An empty or bypassed slot costs exactly nothing: the host draws nothing,
//! the effect view's clock is stopped, and the program path is byte-for-byte
//! the pre-slot pipeline.

use crate::effects::shaders::DrawVjFxPresent;
use crate::effects::VjFxView;
use crate::mix::MixState;
use crate::views::DrawProgram;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The slot tile: SDF chrome + the live effect texture in one quad.
    // Instance fields come from the Rust struct (has_fx/armed/hover/down/
    // bypass/engage); the accent is the console's green.
    set_type_default() do #(DrawFxSlotTile::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)

        pixel: fn() {
            let w = self.rect_size.x
            let h = self.rect_size.y
            let p = self.pos * self.rect_size
            let sdf = Sdf2d.viewport(p)
            let acc = vec3(0.243, 0.878, 0.690)
            let r = 7.0
            sdf.box(1.0, 1.0, w - 2.0, h - 2.0, r)
            // Loaded: the live picture, dimmed under bypass, with a darkened
            // band at the bottom so the name always reads.
            let live = self.tex.sample_as_bgra(self.pos)
            let lit = mix(1.0, 0.30, self.bypass)
            let band = smoothstep(h - 26.0, h - 5.0, p.y) * 0.62
            let pic = vec3(live.x, live.y, live.z) * lit * (1.0 - band)
            // Empty: a machined well — vertical gradient, engraved plus that
            // warms toward the accent under the pointer or while armed.
            let well = mix(vec3(0.088, 0.108, 0.140), vec3(0.038, 0.050, 0.068), self.pos.y)
            let dx = abs(p.x - w * 0.5)
            let dy = abs(p.y - h * 0.5 + 5.0)
            let plus = max(step(dx, 1.4) * step(dy, 8.0), step(dy, 1.4) * step(dx, 8.0))
            let warm = 0.30 + 0.55 * max(self.hover, self.armed)
            let well2 = well + acc * plus * warm + acc * self.armed * 0.03
            let base = mix(well2, pic, self.has_fx)
            let col = base + vec3(1.0, 1.0, 1.0) * self.down * 0.05
            sdf.fill(vec4(col.x, col.y, col.z, 1.0))
            // Engage meter: an accent bar growing along the bottom while the
            // transition effect is live in the program.
            sdf.box(6.0, h - 7.0, max((w - 12.0) * self.engage, 0.001), 3.0, 1.5)
            sdf.fill(vec4(acc.x * self.engage, acc.y * self.engage, acc.z * self.engage, self.engage))
            // Border: chrome at rest, accent when armed or engaged, brighter
            // under the pointer. Armed also thickens the ring.
            sdf.box(1.0, 1.0, w - 2.0, h - 2.0, r)
            let on = max(self.armed, self.engage)
            let bc = mix(vec4(1.0, 1.0, 1.0, 0.16), vec4(acc.x, acc.y, acc.z, 0.9), on)
            let bc2 = bc.mix(vec4(0.9, 1.0, 0.97, 0.7), self.hover * 0.4)
            // Refusal: the ring burns amber over everything else.
            let bc3 = bc2.mix(vec4(1.0, 0.62, 0.24, 0.95), self.refuse)
            sdf.stroke(bc3, 1.0 + max(self.armed, self.refuse) * 0.8)
            return sdf.result
        }
    }

    mod.widgets.VjFxSlotHostBase = #(VjFxSlotHost::register_widget(vm))
    mod.widgets.VjFxSlotHost = set_type_default() do mod.widgets.VjFxSlotHostBase{
        width: 4
        height: 4
        fx: mod.widgets.VjFxView{ composite: false }
    }

    mod.widgets.VjFxSlotTileBase = #(VjFxSlotTile::register_widget(vm))
    mod.widgets.VjFxSlotTile = set_type_default() do mod.widgets.VjFxSlotTileBase{
        width: 150
        height: 86
        draw_title +: {
            color: #xf0f5f9
            text_style: theme.font_bold{font_size: 8}
        }
        draw_tag +: {
            color: #x8e9aa7
            text_style: theme.font_bold{font_size: 7}
        }
    }
}

// ---------------------------------------------------------------------------
// the pure slot model
// ---------------------------------------------------------------------------

/// Which slot. `EffectA`/`EffectB` run over their deck's content; the
/// `Transition` runs over the program mix during crossfades.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxSlotKind {
    EffectA,
    Transition,
    EffectB,
}

impl FxSlotKind {
    pub const ALL: [FxSlotKind; 3] =
        [FxSlotKind::EffectA, FxSlotKind::Transition, FxSlotKind::EffectB];

    pub fn index(self) -> usize {
        match self {
            FxSlotKind::EffectA => 0,
            FxSlotKind::Transition => 1,
            FxSlotKind::EffectB => 2,
        }
    }

    /// Stable single-letter key for persistence file names.
    pub fn key(self) -> char {
        match self {
            FxSlotKind::EffectA => 'a',
            FxSlotKind::Transition => 't',
            FxSlotKind::EffectB => 'b',
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            FxSlotKind::EffectA => "FX A",
            FxSlotKind::Transition => "TRANS",
            FxSlotKind::EffectB => "FX B",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            FxSlotKind::EffectA => "EFFECT A",
            FxSlotKind::Transition => "TRANSITION",
            FxSlotKind::EffectB => "EFFECT B",
        }
    }
}

/// One slot's assignment + controls. SPD scales the effect's own clock
/// (0.5 = 1x); the two dial knobs drive the doc's first two DECLARED dials
/// (`dials:` in the splash, or the engine's default set) and override the
/// bound user param only once touched — an untouched knob leaves the
/// document's own (possibly music-bound) value in charge.
#[derive(Clone, Debug, PartialEq)]
pub struct FxSlotState {
    /// The loaded effect's display name; `None` = empty slot.
    pub title: Option<String>,
    /// The loaded revision id (string form) — the slot tile shows THAT
    /// effect's catalog thumbnail by it (identity over monitoring: the
    /// deck monitors show the live result, the tile says WHICH effect).
    pub rev: Option<String>,
    pub bypass: bool,
    /// SPD knob position 0..1 (see [`FxSlots::speed_scale`]).
    pub speed: f32,
    /// The THREE fixed dial knobs' overrides for user params p0..p2 (p3 is
    /// reserved — the transition engage triangle rides it); `None` until
    /// touched. FIXED COUNT: a MIDI binding on "slot A dial 2" keeps
    /// meaning p2 whatever effect loads; the doc's declarations only LABEL
    /// the dials (an undeclared one shows dimmed and inert).
    pub p: [Option<f32>; 3],
    /// Transient status ("loading…", a load error). Never persisted.
    pub note: Option<String>,
    /// A refusal flash: (message, host-seconds it expires). Transient.
    pub flash: Option<(String, f64)>,
}

impl Default for FxSlotState {
    fn default() -> Self {
        FxSlotState {
            title: None,
            rev: None,
            bypass: false,
            speed: 0.5,
            p: [None; 3],
            note: None,
            flash: None,
        }
    }
}

impl FxSlotState {
    /// Loaded and switched on: this slot participates in the program.
    pub fn running(&self) -> bool {
        self.title.is_some() && !self.bypass
    }
}

/// The click-slot-then-click-tile assignment state machine.
#[derive(Clone, Debug, PartialEq)]
pub struct FxSlots {
    /// The armed slot: the next FX-tile click loads here. Arming is a
    /// toggle; it survives a load so effects can be auditioned in place.
    pub armed: Option<FxSlotKind>,
    pub slots: [FxSlotState; 3],
    /// The AUTOFADE toggle: an EFFECT tile click (which always lands on
    /// the most-faded-out side's slot) also sweeps the crossfader to bring
    /// that side up. Off = load only; the operator rides the fader.
    pub click_autofade: bool,
}

impl Default for FxSlots {
    fn default() -> Self {
        FxSlots { armed: None, slots: Default::default(), click_autofade: true }
    }
}

impl FxSlots {
    pub fn slot(&self, kind: FxSlotKind) -> &FxSlotState {
        &self.slots[kind.index()]
    }

    pub fn slot_mut(&mut self, kind: FxSlotKind) -> &mut FxSlotState {
        &mut self.slots[kind.index()]
    }

    /// Click on a slot tile: arm it, switch the arm, or disarm.
    /// ONE-SHOT consumption: an ACCEPTED load on `kind` spends the arm —
    /// returns true when it was armed. A refusal never calls this, so a
    /// wrong-type click keeps the arm for the right tile. (A latched arm
    /// that survived its load silently owned every later effect click —
    /// the "auto-drop stopped working" wedge.)
    pub fn consume_armed(&mut self, kind: FxSlotKind) -> bool {
        if self.armed == Some(kind) {
            self.armed = None;
            true
        } else {
            false
        }
    }

    /// Returns whether that slot is armed afterwards.
    pub fn toggle_arm(&mut self, kind: FxSlotKind) -> bool {
        if self.armed == Some(kind) {
            self.armed = None;
            false
        } else {
            self.armed = Some(kind);
            true
        }
    }

    /// An effect landed in `kind`: name it, switch it on, clear stale notes.
    /// The knobs keep their positions — swapping effects mid-set must not
    /// yank the levers.
    pub fn loaded(&mut self, kind: FxSlotKind, title: String, rev: Option<String>) {
        let slot = self.slot_mut(kind);
        slot.title = Some(title);
        slot.rev = rev;
        slot.bypass = false;
        slot.note = None;
        // A NEW doc lands UNTOUCHED: the previous occupant's dial values
        // must never bleed onto this one's dials (the "every wipe dips
        // dark" report — the old effect's third knob kept applying to the
        // freshly loaded wipe's DIP). The host layers the effect's OWN
        // sticky profile on top afterwards, if one exists.
        slot.p = [None; 3];
        slot.speed = FxSlotState::default().speed;
    }

    /// CLEAR: back to the factory-default empty slot (arming untouched).
    pub fn clear(&mut self, kind: FxSlotKind) {
        self.slots[kind.index()] = FxSlotState::default();
    }

    /// SLOT TYPE LAW: EFFECT A/B accept any vjeffect document — never
    /// videos or other content; the TRANSITION slot accepts ONLY
    /// transition-tagged vjeffects. `Err` is the refusal message.
    pub fn accepts(
        kind: FxSlotKind,
        is_effect: bool,
        transition_tagged: bool,
    ) -> Result<(), &'static str> {
        if !is_effect {
            return Err("FX docs only");
        }
        if kind == FxSlotKind::Transition && !transition_tagged {
            return Err("TRANSITION FX only");
        }
        Ok(())
    }

    /// How long a refusal flash stays on the tile.
    pub const FLASH_SECS: f64 = 2.2;

    /// A wrong-type assignment: flash the slot with the reason.
    pub fn refuse(&mut self, kind: FxSlotKind, msg: &str, now: f64) {
        self.slot_mut(kind).flash = Some((msg.to_string(), now + Self::FLASH_SECS));
    }

    /// Expire finished flashes; true when something changed (UI resync).
    pub fn tick_flashes(&mut self, now: f64) -> bool {
        let mut changed = false;
        for slot in self.slots.iter_mut() {
            if slot.flash.as_ref().is_some_and(|(_, until)| now >= *until) {
                slot.flash = None;
                changed = true;
            }
        }
        changed
    }

    pub fn any_flash(&self) -> bool {
        self.slots.iter().any(|slot| slot.flash.is_some())
    }

    /// Any slot that wants render/pump time.
    pub fn any_running(&self) -> bool {
        self.slots.iter().any(FxSlotState::running)
    }

    /// SPD knob position → time multiplier: 0.25x .. 4x, centre = 1x.
    pub fn speed_scale(knob: f32) -> f32 {
        2.0f32.powf((knob.clamp(0.0, 1.0) - 0.5) * 4.0)
    }

    /// gen-panel.txt-style persistence body (arming is a live gesture and is
    /// not persisted; notes are transient).
    pub fn encode(&self) -> String {
        let mut out = format!("v2\naf {}\n", u8::from(self.click_autofade));
        for slot in &self.slots {
            let dial = |v: Option<f32>| v.map(|v| format!("{v:.4}")).unwrap_or_else(|| "-".into());
            out.push_str(&format!(
                "{} {} {:.4} {} {} {} {} {}\n",
                u8::from(slot.title.is_some()),
                u8::from(slot.bypass),
                slot.speed,
                dial(slot.p[0]),
                dial(slot.p[1]),
                dial(slot.p[2]),
                slot.rev.as_deref().unwrap_or("-"),
                slot.title.as_deref().unwrap_or(""),
            ));
        }
        out
    }

    /// Tolerant decode of [`FxSlots::encode`]'s output; anything malformed
    /// falls back to that slot's default. v1 files (two dials) still read.
    pub fn decode(body: &str) -> FxSlots {
        let mut slots = FxSlots::default();
        let mut lines = body.lines().peekable();
        let dial_count = match lines.next() {
            Some("v2") => 3usize,
            Some("v1") => 2,
            _ => return slots,
        };
        // Optional settings line ("af 0|1"); older files go straight to
        // the slot lines.
        if lines.peek().is_some_and(|line| line.starts_with("af ")) {
            slots.click_autofade = lines.next() == Some("af 1");
        }
        for slot in slots.slots.iter_mut() {
            let Some(line) = lines.next() else { break };
            let head_fields = if dial_count == 3 { 5 } else { 4 };
            let mut it = line.splitn(head_fields + dial_count, ' ');
            let loaded = it.next() == Some("1");
            let bypass = it.next() == Some("1");
            let speed = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.5f32);
            let mut p = [None; 3];
            for value in p.iter_mut().take(dial_count) {
                *value = it.next().and_then(|v| v.parse().ok());
            }
            // v2 carries the revision id before the title; v1 has neither.
            let rev = if dial_count == 3 {
                it.next().filter(|v| *v != "-").map(str::to_string)
            } else {
                None
            };
            let title = it.next().unwrap_or("").trim().to_string();
            if loaded && !title.is_empty() {
                slot.title = Some(title);
                slot.rev = rev;
            }
            slot.bypass = bypass && slot.title.is_some();
            slot.speed = speed.clamp(0.0, 1.0);
            slot.p = p;
        }
        slots
    }
}

// ---------------------------------------------------------------------------
// the offscreen render host
// ---------------------------------------------------------------------------

/// This frame's transition premix: the two program sources and the
/// downstream mix, composited exactly as the program would.
#[derive(Clone)]
pub struct PremixJob {
    pub a: Option<(Texture, f32)>,
    pub b: Option<(Texture, f32)>,
    pub mix: f32,
    pub state: MixState,
}

/// The premix render target (post.rs `SubPass` idiom).
struct PremixRt {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
}

impl PremixRt {
    fn new(cx: &mut Cx) -> Self {
        let pass = DrawPass::new_with_name(cx, "vjfx_slot_premix");
        let draw_list = DrawList2d::new(cx);
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
        );
        pass.set_color_texture(
            cx,
            &texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );
        Self { pass, draw_list, texture }
    }
}

/// Offscreen resolution of the transition premix — the effect pass renders
/// at the same size, so the program never upsamples.
const PREMIX_SIZE: DVec2 = dvec2(1280.0, 720.0);

/// The offscreen effect-pass host: a 4x4 widget in the always-drawn status
/// bar (the mesh/splat/flow-slot convention). While enabled it renders its
/// [`VjFxView`] every frame and samples the output into its tiny rect — the
/// frame dependency that keeps the offscreen chain rendering. Disabled, it
/// draws nothing at all.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VjFxSlotHost {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The slot-mode effect view (composite off; program-sized pass).
    #[live]
    fx: VjFxView,
    #[live]
    draw_present: DrawVjFxPresent,
    /// The transition premix quad — the SAME `DrawProgram` shader the
    /// program widget composites with, so the effect sees exactly what the
    /// operator would.
    #[live]
    draw_premix: DrawProgram,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    enabled: bool,
    #[rust]
    premix: Option<PremixJob>,
    #[rust]
    premix_rt: Option<PremixRt>,
    #[rust]
    premix_drawn: bool,
}

impl VjFxSlotHost {
    /// Evaluate + load an effect document. Errors come back to the caller —
    /// the slot tile wears them.
    pub fn load(&mut self, cx: &mut Cx, key: &str, source: &str) -> Result<String, String> {
        let name = self.fx.set_effect_source(cx, key, source)?;
        self.fx.set_live(cx, self.enabled);
        self.area.redraw(cx);
        Ok(name)
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.fx.clear_effect(cx);
        self.premix = None;
        self.premix_drawn = false;
        self.area.redraw(cx);
    }

    pub fn has_effect(&self) -> bool {
        self.fx.has_effect()
    }

    /// The loaded doc's declared dials (see `effects::doc::DialDecl`).
    pub fn dials(&self) -> Vec<crate::effects::doc::DialDecl> {
        self.fx.dials().to_vec()
    }

    /// Run/park the effect. Parked costs nothing; the last frame stays in
    /// the output texture for the tile preview.
    pub fn set_enabled(&mut self, cx: &mut Cx, on: bool) {
        if self.enabled == on {
            return;
        }
        self.enabled = on;
        self.fx.set_live(cx, on);
        if on {
            self.next_frame = cx.new_next_frame();
        }
        self.area.redraw(cx);
    }

    pub fn set_beat(&mut self, beat_pos: f64, bpm: f64) {
        self.fx.set_beat(beat_pos, bpm);
    }

    pub fn set_signals(&mut self, audio: [f32; 4]) {
        self.fx.set_signals(audio);
    }

    /// THE AUDIO PICTURE (effects/audio_tex.rs): the live spectrogram +
    /// waveform texture every engine's shaders sample, plus the four
    /// binding levels derived from the same analysis.
    pub fn set_audio(&mut self, binding: Option<crate::effects::audio_tex::AudioBinding>) {
        self.fx.set_audio(binding);
    }

    pub fn set_speed(&mut self, scale: f32) {
        self.fx.set_speed_scale(scale);
    }

    pub fn set_user(&mut self, over: [Option<f32>; 4]) {
        self.fx.set_user_override(over);
    }

    /// Channel mode: the deck's content becomes the effect's `input0`.
    pub fn set_channel_input(&mut self, texture: Option<Texture>) {
        self.premix = None;
        self.fx.set_input_texture(0, texture);
    }

    /// Transition mode: composite `a`/`b` under the downstream mix into the
    /// premix texture next draw, and feed THAT to the effect as `input0`.
    pub fn set_premix(&mut self, job: PremixJob) {
        self.premix = Some(job);
    }

    /// True when the loaded doc is the two-deck `transition` engine: feed it
    /// with [`Self::set_deck_inputs`] and sweep p3 with the CROSSFADER.
    pub fn wants_deck_inputs(&self) -> bool {
        self.fx.wants_deck_inputs()
    }

    /// The loaded doc declared `engage: "ramp"` (overlay/key semantics).
    pub fn engage_ramp(&self) -> bool {
        self.fx.engage_ramp()
    }

    /// Duo mode: deck A into input 0, deck B into input 1 (no premix pass).
    pub fn set_deck_inputs(&mut self, a: Option<Texture>, b: Option<Texture>) {
        self.premix = None;
        self.fx.set_input_texture(0, a);
        self.fx.set_input_texture(1, b);
    }

    /// The effect's output — only while it is actually running. Re-fetched
    /// per frame by the pump (feedback chains ping-pong the identity).
    pub fn output(&self) -> Option<Texture> {
        if self.enabled && self.fx.has_effect() {
            self.fx.output_texture()
        } else {
            None
        }
    }

    /// The last rendered frame regardless of run state — the slot tile's
    /// preview (a bypassed effect shows its frozen frame, dimmed).
    pub fn preview_output(&self) -> Option<Texture> {
        if self.fx.has_effect() {
            self.fx.output_texture()
        } else {
            None
        }
    }

    /// The premix texture, once it has actually been rendered.
    pub fn premix_output(&self) -> Option<Texture> {
        if self.premix_drawn {
            self.premix_rt.as_ref().map(|rt| rt.texture.clone())
        } else {
            None
        }
    }

    pub fn status(&self) -> &str {
        &self.fx.status
    }

    fn render_premix(&mut self, cx: &mut Cx2d) {
        let Some(job) = self.premix.clone() else { return };
        if self.premix_rt.is_none() {
            self.premix_rt = Some(PremixRt::new(cx.cx));
        }
        let d = &mut self.draw_premix;
        d.has_a = if job.a.is_some() { 1.0 } else { 0.0 };
        d.has_b = if job.b.is_some() { 1.0 } else { 0.0 };
        match &job.a {
            Some((tex, aspect)) => {
                d.aspect_a = aspect.max(0.05);
                d.draw_vars.set_texture(0, tex);
            }
            None => d.draw_vars.empty_texture(0),
        }
        match &job.b {
            Some((tex, aspect)) => {
                d.aspect_b = aspect.max(0.05);
                d.draw_vars.set_texture(1, tex);
            }
            None => d.draw_vars.empty_texture(1),
        }
        d.mix_ab = job.mix.clamp(0.0, 1.0);
        d.mix_mode = job.state.mode.as_f32();
        d.mix_p1 = job.state.p1.clamp(0.0, 1.0);
        d.mix_p2 = job.state.p2.clamp(0.0, 1.0);
        // Downstream mix only: the premix is exactly the program compose
        // the operator dialed in, at the fader's position.
        let Some(rt) = self.premix_rt.as_mut() else { return };
        rt.pass.set_size(cx.cx, PREMIX_SIZE);
        cx.make_child_pass(&rt.pass);
        cx.begin_pass(&rt.pass, Some(1.0));
        rt.draw_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        self.draw_premix
            .draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: pass_size });
        cx.end_pass_sized_turtle();
        let Some(rt) = self.premix_rt.as_mut() else { return };
        rt.draw_list.end(cx);
        cx.end_pass(&rt.pass);
        self.premix_drawn = true;
    }
}

impl WidgetNode for VjFxSlotHost {
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

impl Widget for VjFxSlotHost {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The effect view runs its own NextFrame clock while live;
        // forwarding every event is what keeps it ticking.
        self.fx.handle_event(cx, event, scope);
        if self.next_frame.is_event(event).is_some() && self.enabled && self.fx.has_effect() {
            self.area.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        if self.enabled && self.fx.has_effect() {
            // Transition premix renders BEFORE the effect pass samples it —
            // the sim-fields-before-scene ordering from effects/view.rs.
            if self.premix.is_some() {
                self.render_premix(cx);
                if let Some(tex) = self.premix_output() {
                    self.fx.set_input_texture(0, Some(tex));
                }
            }
            let _ = self.fx.draw_walk(cx, scope, Walk::fill());
            // Sample the output into the 4x4: the frame dependency that
            // makes the offscreen chain actually render.
            if let Some(texture) = self.fx.output_texture() {
                self.draw_present.draw_vars.set_texture(0, &texture);
                self.draw_present.draw_abs(cx, rect);
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        if self.enabled && self.fx.has_effect() {
            self.area.redraw(cx.cx);
            self.next_frame = cx.cx.new_next_frame();
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// the slot tile
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum FxSlotTileAction {
    #[default]
    None,
    Pressed,
}

/// Instance layout law: only `#[live]` shader-input fields after the deref.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFxSlotTile {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub has_fx: f32,
    #[live]
    pub armed: f32,
    #[live]
    pub hover: f32,
    #[live]
    pub down: f32,
    #[live]
    pub bypass: f32,
    #[live]
    pub engage: f32,
    /// Refusal flash 0/1 — the border burns amber while a wrong-type
    /// assignment message is up.
    #[live]
    pub refuse: f32,
}

/// What the tile is showing this frame — pushed by the app, compared to
/// decide whether a redraw is owed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FxSlotTileState {
    pub title: Option<String>,
    /// Small status word (bottom-right): "BYP", "ON FADE", an error.
    pub note: String,
    pub armed: bool,
    pub bypass: bool,
    /// Transition liveness 0..1 → the engage meter.
    pub engage: f32,
    /// A refusal flash is up: the note is the refusal, the ring burns.
    pub flash: bool,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VjFxSlotTile {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_tile: DrawFxSlotTile,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_tag: DrawText,
    #[rust]
    area: Area,
    /// "EFFECT A" / "TRANSITION" / "EFFECT B" — set once by the app.
    #[rust]
    hint: String,
    #[rust]
    tag: String,
    #[rust]
    state: FxSlotTileState,
    #[rust]
    preview: Option<Texture>,
    #[rust]
    hover: bool,
    #[rust]
    down: bool,
}

impl VjFxSlotTile {
    pub fn set_labels(&mut self, tag: &str, hint: &str) {
        self.tag = tag.to_string();
        self.hint = hint.to_string();
    }

    pub fn set_state(&mut self, cx: &mut Cx, state: FxSlotTileState) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.area.redraw(cx);
    }

    /// Bind the live output texture (or drop back to the empty art). Only an
    /// identity change redraws — the picture itself animates because the
    /// enclosing pass re-renders while the program pumps.
    pub fn set_preview(&mut self, cx: &mut Cx, texture: Option<Texture>) {
        let changed = match (&self.preview, &texture) {
            (None, None) => false,
            (Some(a), Some(b)) => a.texture_id() != b.texture_id(),
            _ => true,
        };
        if changed {
            self.preview = texture;
            self.area.redraw(cx);
        }
    }

    fn measure(&self, cx: &mut Cx2d, text: &str) -> f64 {
        let laid = self
            .draw_title
            .layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        (laid.size_in_lpxs.width * self.draw_title.font_scale) as f64
    }
}

impl WidgetNode for VjFxSlotTile {
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

impl Widget for VjFxSlotTile {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.down = true;
                cx.widget_action(self.uid, FxSlotTileAction::Pressed);
                self.area.redraw(cx);
            }
            Hit::FingerUp(_) => {
                self.down = false;
                self.area.redraw(cx);
            }
            Hit::FingerHoverIn(_) => {
                self.hover = true;
                cx.set_cursor(MouseCursor::Hand);
                self.area.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hover = false;
                self.down = false;
                self.area.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        let has_fx = self.state.title.is_some() && self.preview.is_some();
        match &self.preview {
            Some(tex) if self.state.title.is_some() => {
                self.draw_tile.draw_vars.set_texture(0, tex)
            }
            _ => self.draw_tile.draw_vars.empty_texture(0),
        }
        self.draw_tile.has_fx = if has_fx { 1.0 } else { 0.0 };
        self.draw_tile.armed = if self.state.armed { 1.0 } else { 0.0 };
        self.draw_tile.hover = if self.hover { 1.0 } else { 0.0 };
        self.draw_tile.down = if self.down { 1.0 } else { 0.0 };
        self.draw_tile.bypass = if self.state.bypass { 1.0 } else { 0.0 };
        self.draw_tile.engage = self.state.engage.clamp(0.0, 1.0);
        self.draw_tile.refuse = if self.state.flash { 1.0 } else { 0.0 };
        self.draw_tile.draw_abs(cx, rect);
        self.area = self.draw_tile.area();

        // Tag, top-left — accent while armed.
        self.draw_tag.color = if self.state.armed {
            vec4(0.243, 0.878, 0.690, 1.0)
        } else {
            vec4(0.557, 0.604, 0.655, 0.9)
        };
        self.draw_tag
            .draw_abs(cx, dvec2(rect.pos.x + 7.0, rect.pos.y + 5.0), &self.tag);

        match self.state.title.clone() {
            Some(mut title) => {
                // Name, bottom-left, clipped to the tile with an ellipsis.
                let max_w = rect.size.x - 16.0;
                while title.chars().count() > 4 && self.measure(cx, &title) > max_w {
                    title.pop();
                    while !title.is_char_boundary(title.len()) {
                        title.pop();
                    }
                    title.pop();
                    title.push('…');
                }
                self.draw_title.color = if self.state.bypass {
                    vec4(0.62, 0.67, 0.72, 0.9)
                } else {
                    vec4(0.94, 0.96, 0.98, 1.0)
                };
                self.draw_title.draw_abs(
                    cx,
                    dvec2(rect.pos.x + 7.0, rect.pos.y + rect.size.y - 15.0),
                    &title,
                );
            }
            None => {
                // Inviting empty state: the hint sits under the engraved +.
                self.draw_title.color = if self.state.armed {
                    vec4(0.243, 0.878, 0.690, 1.0)
                } else {
                    vec4(0.49, 0.54, 0.59, 1.0)
                };
                let w = self.measure(cx, &self.hint);
                self.draw_title.draw_abs(
                    cx,
                    dvec2(
                        rect.pos.x + (rect.size.x - w) * 0.5,
                        rect.pos.y + rect.size.y * 0.5 + 9.0,
                    ),
                    &self.hint,
                );
            }
        }
        if !self.state.note.is_empty() {
            // Status word, bottom-right (top-centred when empty). A
            // refusal reads in the same amber as the burning ring, and
            // moves to the top-right so it never collides with the name.
            self.draw_tag.color = if self.state.flash {
                vec4(1.0, 0.62, 0.24, 1.0)
            } else {
                vec4(0.243, 0.878, 0.690, 0.95)
            };
            let w = self.measure(cx, &self.state.note);
            let pos = if self.state.flash {
                dvec2(rect.pos.x + rect.size.x - w - 7.0, rect.pos.y + 5.0)
            } else if self.state.title.is_some() {
                dvec2(
                    rect.pos.x + rect.size.x - w - 7.0,
                    rect.pos.y + rect.size.y - 14.0,
                )
            } else {
                dvec2(
                    rect.pos.x + (rect.size.x - w) * 0.5,
                    rect.pos.y + rect.size.y * 0.5 + 21.0,
                )
            };
            self.draw_tag.draw_abs(cx, pos, &self.state.note);
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// tests — the slot-assignment state machine
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accepted_load_consumes_the_arm_and_a_refusal_keeps_it() {
        let mut slots = FxSlots::default();
        assert!(slots.toggle_arm(FxSlotKind::EffectA));
        assert!(slots.consume_armed(FxSlotKind::EffectA));
        assert_eq!(slots.armed, None, "one-shot: the arm is spent");
        assert!(!slots.consume_armed(FxSlotKind::EffectA), "nothing left to spend");
        // A refusal path never consumes: arming stays for the right tile.
        assert!(slots.toggle_arm(FxSlotKind::Transition));
        assert!(!slots.consume_armed(FxSlotKind::EffectB));
        assert_eq!(slots.armed, Some(FxSlotKind::Transition));
    }

    #[test]
    fn arming_is_a_toggle_and_a_switch() {
        let mut m = FxSlots::default();
        // No arm: an FX-tile click is a CONTENT cue, not a slot load.
        assert_eq!(m.armed, None);
        assert!(m.toggle_arm(FxSlotKind::Transition));
        assert_eq!(m.armed, Some(FxSlotKind::Transition));
        // Arming another slot SWITCHES the arm, it does not stack.
        assert!(m.toggle_arm(FxSlotKind::EffectB));
        assert_eq!(m.armed, Some(FxSlotKind::EffectB));
        // Clicking the armed slot again disarms.
        assert!(!m.toggle_arm(FxSlotKind::EffectB));
        assert_eq!(m.armed, None);
    }

    #[test]
    fn loading_switches_on_and_resets_the_previous_docs_knobs() {
        let mut m = FxSlots::default();
        let slot = m.slot_mut(FxSlotKind::EffectA);
        slot.bypass = true;
        slot.note = Some("load failed".into());
        slot.speed = 0.8;
        slot.p[1] = Some(0.9);
        m.loaded(FxSlotKind::EffectA, "Neon Growth".into(), Some("rev:abc".into()));
        let slot = m.slot(FxSlotKind::EffectA);
        assert_eq!(slot.title.as_deref(), Some("Neon Growth"));
        assert!(!slot.bypass, "a fresh load is ON");
        assert_eq!(slot.note, None, "stale errors do not survive a load");
        // THE DIAL LAW REVERSED (user): the previous occupant's dial values
        // must never bleed onto the new doc — "when i switch transitions
        // dont keep the dial values of the previous one". A new load lands
        // UNTOUCHED; the host layers the effect's OWN sticky profile after.
        assert_eq!(
            slot.speed,
            FxSlotState::default().speed,
            "the old doc's clock never rides the new doc"
        );
        assert_eq!(slot.p, [None, None, None], "dials land untouched");
        assert!(m.any_running());
        m.clear(FxSlotKind::EffectA);
        assert_eq!(*m.slot(FxSlotKind::EffectA), FxSlotState::default());
        assert!(!m.any_running());
    }

    #[test]
    fn bypass_parks_the_slot() {
        let mut m = FxSlots::default();
        m.loaded(FxSlotKind::EffectB, "Tunnel".into(), None);
        assert!(m.slot(FxSlotKind::EffectB).running());
        m.slot_mut(FxSlotKind::EffectB).bypass = true;
        assert!(!m.slot(FxSlotKind::EffectB).running());
        assert!(!m.any_running());
    }

    #[test]
    fn persistence_round_trips_including_untouched_knobs() {
        let mut m = FxSlots::default();
        m.loaded(FxSlotKind::EffectA, "Neon Growth".into(), Some("rev:abc".into()));
        m.slot_mut(FxSlotKind::EffectA).p = [Some(0.25), None, Some(0.8)];
        m.slot_mut(FxSlotKind::EffectA).speed = 0.75;
        m.loaded(FxSlotKind::Transition, "Beat Lens".into(), None);
        m.slot_mut(FxSlotKind::Transition).bypass = true;
        m.toggle_arm(FxSlotKind::EffectB);
        m.click_autofade = false;
        let decoded = FxSlots::decode(&m.encode());
        assert_eq!(decoded.armed, None, "arming is a live gesture, not state");
        assert!(!decoded.click_autofade, "the AUTOFADE latch persists");
        // An older file without the settings line keeps the default ON.
        assert!(FxSlots::decode("v1\n1 0 0.5 - - X\n").click_autofade);
        // A v1 (two-dial) file still restores its two dials.
        let legacy = FxSlots::decode("v1\n1 0 0.6000 0.2500 - Old Effect\n");
        assert_eq!(legacy.slot(FxSlotKind::EffectA).p, [Some(0.25), None, None]);
        assert_eq!(legacy.slot(FxSlotKind::EffectA).title.as_deref(), Some("Old Effect"));
        let mut expect = m.clone();
        expect.armed = None;
        assert_eq!(decoded, expect);
        // Junk decodes to the default panel instead of exploding.
        assert_eq!(FxSlots::decode("nonsense\n1 1"), FxSlots::default());
        assert_eq!(FxSlots::decode(""), FxSlots::default());
    }

    #[test]
    fn titles_with_spaces_survive_the_line_format() {
        let mut m = FxSlots::default();
        m.loaded(FxSlotKind::EffectB, "Domino Liturgy — gold".into(), Some("sha256:aa".into()));
        let decoded = FxSlots::decode(&m.encode());
        assert_eq!(
            decoded.slot(FxSlotKind::EffectB).title.as_deref(),
            Some("Domino Liturgy — gold")
        );
    }

    #[test]
    fn slot_type_law_is_strict() {
        // A/B: any vjeffect, never content.
        for kind in [FxSlotKind::EffectA, FxSlotKind::EffectB] {
            assert_eq!(FxSlots::accepts(kind, true, false), Ok(()));
            assert_eq!(FxSlots::accepts(kind, true, true), Ok(()));
            assert_eq!(FxSlots::accepts(kind, false, false), Err("FX docs only"));
            // Even a "transition-tagged" non-effect is content: refused.
            assert_eq!(FxSlots::accepts(kind, false, true), Err("FX docs only"));
        }
        // TRANSITION: only transition-tagged effects.
        assert_eq!(FxSlots::accepts(FxSlotKind::Transition, true, true), Ok(()));
        assert_eq!(
            FxSlots::accepts(FxSlotKind::Transition, true, false),
            Err("TRANSITION FX only")
        );
        assert_eq!(
            FxSlots::accepts(FxSlotKind::Transition, false, false),
            Err("FX docs only")
        );
    }

    #[test]
    fn refusal_flashes_expire_and_never_persist() {
        let mut m = FxSlots::default();
        m.refuse(FxSlotKind::Transition, "TRANSITION FX only", 10.0);
        assert!(m.any_flash());
        assert!(!m.tick_flashes(10.0 + FxSlots::FLASH_SECS - 0.1), "still up");
        assert!(m.any_flash());
        assert!(m.tick_flashes(10.0 + FxSlots::FLASH_SECS + 0.1), "expired");
        assert!(!m.any_flash());
        // A flash never survives the persistence round trip.
        m.refuse(FxSlotKind::EffectA, "FX docs only", 5.0);
        let decoded = FxSlots::decode(&m.encode());
        assert!(!decoded.any_flash());
    }

    #[test]
    fn speed_knob_centre_is_unity_and_the_range_is_symmetric() {
        assert!((FxSlots::speed_scale(0.5) - 1.0).abs() < 1e-6);
        assert!((FxSlots::speed_scale(0.0) - 0.25).abs() < 1e-6);
        assert!((FxSlots::speed_scale(1.0) - 4.0).abs() < 1e-6);
    }
}
