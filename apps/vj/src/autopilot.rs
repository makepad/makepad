//! DJ autopilot: decides WHEN the deck-to-deck transition happens.
//!
//! The autopilot owns timing intent only — the deck engine owns what a
//! command means for the decks, and the mixer performs it. Pure and
//! clock-free like the engine: the host feeds one observation per pump
//! tick (20 Hz) and executes the returned commands, so every decision here
//! is testable by replaying observations, and every scheduled moment lands
//! within one pump interval of its plan — that tolerance is the timing
//! contract, stated rather than hidden.
//!
//! One transition can fire per outgoing load generation, by construction:
//! the generation is latched the moment the fade (or its degenerate
//! no-fade twin) is issued, so a hand scrubbing back over the outro cannot
//! fire it twice. A hand anywhere (the host routes every operator touch
//! through `hands_on`) drops the plan wholesale and the next tick re-plans
//! against whatever the hand left behind — autopilot never argues.

use crate::blend::{self, BlendStep, Lane, Medium, MixBrain, Role, SungMap};
use crate::decks::{DeckGen, DeckId};
use crate::track_shape::TrackShape;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoLoad {
    Empty,
    Loading,
    Loaded,
    Failed,
}

#[derive(Clone, Copy, Debug)]
pub struct AutoDeckObs {
    pub load: AutoLoad,
    pub gen: DeckGen,
    pub playing: bool,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub rate: f64,
    pub loop_on: bool,
    pub scratching: bool,
    /// One bar in source seconds, when the grid is usable.
    pub bar_secs_src: Option<f64>,
    /// The four separated lanes are live on this deck.
    pub stems_ready: bool,
}

impl Default for AutoDeckObs {
    fn default() -> AutoDeckObs {
        AutoDeckObs {
            load: AutoLoad::Empty,
            gen: 0,
            playing: false,
            position_secs: 0.0,
            duration_secs: 0.0,
            rate: 1.0,
            loop_on: false,
            scratching: false,
            bar_secs_src: None,
            stems_ready: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AutoObs {
    pub decks: [AutoDeckObs; 2],
    /// The mixer's live fader position, mid-ramp included.
    pub fader: f32,
    pub queue_len: usize,
    /// The operator's fade slider. Read when a plan is BUILT, never while
    /// one runs: a knob move re-plans an armed transition and leaves a
    /// running fade alone.
    pub fade_secs_knob: f32,
    /// `sync_leader()`, for the both-playing role tie-break.
    pub leader_hint: Option<DeckId>,
}

impl AutoObs {
    fn deck(&self, deck: DeckId) -> &AutoDeckObs {
        &self.decks[deck.index()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AutoStyle {
    /// Classic: the incoming intro plays under the outgoing outro.
    #[default]
    Outro,
    /// Body-to-body: the incoming track starts past its intro and the fade
    /// lands before the outgoing outro plays.
    Body,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AutoCmd {
    /// Seek the incoming deck to its start point (no auto-sync re-lock).
    CueIn { deck: DeckId, secs: f64 },
    /// One explicit tempo/phase lock of the incoming deck to the live one.
    SyncIn { deck: DeckId },
    /// Start the incoming deck (idempotent play, never a toggle).
    PlayIn { deck: DeckId },
    /// Start the timed crossfade. The host also arms fade tracking and the
    /// engine's auto-fade hold.
    BeginFade { to: DeckId, secs: f32 },
    /// The transition is over: unload `retire`, requeue it when `requeue`
    /// and the repeat toggle agree, pump the queue once. `requeue: false`
    /// marks a deck that never sounded — a failed load is not recycled.
    HandBack { retire: DeckId, requeue: bool },
    /// Nothing audible: put the fader on `deck` and play it.
    StartSet { deck: DeckId },
    /// The incoming side is empty and the queue holds tracks: fill it.
    PumpQueue,
    /// One gain move on the mixer's blend overlay — the autopilot's own
    /// hands, never the operator's knobs.
    Blend { deck: DeckId, lane: Lane, gain: f32 },
    /// Let the overlay go on one deck (ramped to unity).
    ClearBlend { deck: DeckId },
}

/// Everything latched at arm time. The knob and style are frozen here so a
/// slider move cannot rewrite a ramp the mixer already owns.
#[derive(Clone, Debug)]
struct Plan {
    out: DeckId,
    incoming: DeckId,
    out_gen: DeckGen,
    style: AutoStyle,
    knob: f32,
    cue_secs: f64,
    /// OUT source position where the fade begins.
    fire_at_src: f64,
    /// OUT source position past which a still-loading IN abandons the plan.
    deadline_src: f64,
    /// Cue + sync issued.
    prepped: bool,
    /// The deadline passed: the track plays out and the hand-back rides on
    /// its end instead of a fade.
    abandoned: bool,
    /// Vocal-guard verdict: how far past the fire point (OUT source secs)
    /// the incoming vocals stay ducked. None = no clash.
    duck_offset_src: Option<f64>,
    /// The medium latched when prep pre-muted the incoming deck. Fire uses
    /// this, never a fresh reading — a deck pre-muted for EQ must not fire
    /// as Stems with an open bass lane.
    prep_medium: Option<Medium>,
}

/// A transition in flight: the fade the mixer runs plus the gain
/// choreography still to perform against it.
#[derive(Clone, Debug)]
struct Fade {
    out: DeckId,
    target: f32,
    /// Remaining choreography, ascending by `at_wall`.
    schedule: Vec<BlendStep>,
    /// Listening seconds since the fade fired, accumulated from OUT's
    /// playhead — the same arithmetic as the countdown.
    elapsed_wall: f64,
    last_out_pos: f64,
    status: &'static str,
}

#[derive(Clone, Debug)]
enum State {
    Idle,
    Planned(Plan),
    Fading(Fade),
}

/// Fader-landing tolerance: the mixer's ramp snaps exactly onto its
/// target, the epsilon just releases one tick earlier.
const LAND_EPS: f32 = 1e-3;
/// Lead/tail margin when a deck has no grid to measure a bar with.
const NO_GRID_MARGIN_SECS: f64 = 2.0;
/// "The deck ran off its end" tolerance on a stopped playhead.
const END_SLACK_SECS: f64 = 0.25;

pub struct AutoPilot {
    on: bool,
    style: AutoStyle,
    state: State,
    /// Start the set on the next tick (set by switch-on, and by a hand-back
    /// that leaves nothing playing — the set restarts itself).
    start_pending: bool,
    /// Shapes keyed by load generation. Generations are engine-global and
    /// unique, so a voice swap needs no bookkeeping here: lookups follow
    /// the gens the observation carries.
    shapes: Vec<(DeckGen, TrackShape)>,
    /// The operator's smartness ceiling and the two orthogonal brains.
    pub brain: MixBrain,
    /// RANDOM on the panel: every new plan rolls `brain` afresh, so each
    /// transition gets its own kind of mix. `brain` still holds the brain
    /// the CURRENT plan resolved to — prep and fire read one value.
    pub brain_random: bool,
    /// The roll's xorshift state. Seeded once, never zero.
    brain_seed: u64,
    pub vocal_guard: bool,
    pub phrase_snap: bool,
    /// Sung intervals per load generation, same lifecycle as `shapes`.
    vocals: Vec<(DeckGen, SungMap)>,
    /// Phrase boundaries per load generation, same lifecycle as `shapes`.
    changes: Vec<(DeckGen, Vec<f64>)>,
    /// The overlay may be holding something down (a pre-mute, a duck): the
    /// next Idle tick leads with ClearBlend for both decks.
    blend_dirty: bool,
    /// Outgoing generations whose one transition already fired (or was
    /// abandoned): never fire twice per load.
    fired: Vec<DeckGen>,
    /// Generations OBSERVED playing within reach of their own end. Only
    /// these may take the ended-deck hand-back: a paused deck the operator
    /// seeked or scratched to the end was never seen running out, and
    /// ejecting it would destroy a track the hand deliberately parked.
    ran_out: Vec<DeckGen>,
    status: String,
}

impl AutoPilot {
    pub fn new() -> AutoPilot {
        AutoPilot {
            on: false,
            style: AutoStyle::default(),
            state: State::Idle,
            start_pending: false,
            shapes: Vec::new(),
            brain: MixBrain::default(),
            brain_random: false,
            brain_seed: 0x9E37_79B9_7F4A_7C15,
            vocal_guard: true,
            phrase_snap: true,
            vocals: Vec::new(),
            changes: Vec::new(),
            blend_dirty: false,
            fired: Vec::new(),
            ran_out: Vec::new(),
            status: String::new(),
        }
    }

    pub fn on(&self) -> bool {
        self.on
    }

    pub fn style(&self) -> AutoStyle {
        self.style
    }

    /// The one status line, exactly as the AUTO DJ button shows it.
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn set_on(&mut self, on: bool) {
        self.on = on;
        self.state = State::Idle;
        self.start_pending = on;
        if !on {
            self.status.clear();
        }
    }

    /// A style change re-plans an armed transition; a running fade keeps
    /// its plan and the new style takes the next one.
    pub fn set_style(&mut self, style: AutoStyle) {
        self.style = style;
        self.replan();
    }

    pub fn set_brain(&mut self, brain: MixBrain) {
        self.brain = brain;
        self.brain_random = false;
        self.replan();
    }

    /// RANDOM on the panel: hold the current brain for any armed plan is
    /// wrong — a settings change re-plans, and the fresh plan rolls.
    pub fn set_brain_random(&mut self, on: bool) {
        self.brain_random = on;
        self.replan();
    }

    /// One roll per plan: xorshift64*, mapped over the three brains. The
    /// state advances every roll, so consecutive transitions vary even
    /// when their plans are otherwise identical.
    fn roll_brain(&mut self) -> MixBrain {
        let mut x = self.brain_seed;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.brain_seed = x;
        match x.wrapping_mul(0x2545_F491_4F6C_DD1D) % 3 {
            0 => MixBrain::Fade,
            1 => MixBrain::Eq,
            _ => MixBrain::Stems,
        }
    }

    pub fn set_vocal_guard(&mut self, on: bool) {
        self.vocal_guard = on;
        self.replan();
    }

    pub fn set_phrase_snap(&mut self, on: bool) {
        self.phrase_snap = on;
        self.replan();
    }

    /// A settings change re-plans an armed transition and leaves a running
    /// fade alone. A dropped plan may have pre-muted the incoming deck; the
    /// dirty latch has the next Idle tick let the overlay go.
    fn replan(&mut self) {
        if matches!(self.state, State::Planned(_)) {
            self.state = State::Idle;
        }
    }

    /// Sung intervals for a load, from lyrics words or the vocal envelope.
    /// A map landing after a plan armed re-plans it — vocal data usually
    /// arrives minutes before the fire, and a guard that ignored it would
    /// stack the very singers it exists to separate.
    pub fn vocals_ready(&mut self, gen: DeckGen, map: SungMap) {
        self.vocals.retain(|(g, _)| *g != gen);
        self.vocals.push((gen, map));
        while self.vocals.len() > 8 {
            self.vocals.remove(0);
        }
        if self.vocal_guard {
            self.replan();
        }
    }

    /// Phrase boundaries for a load, off the published analysis. Late
    /// arrivals re-plan for the same reason sung maps do.
    pub fn changes_ready(&mut self, gen: DeckGen, changes: Vec<f64>) {
        self.changes.retain(|(g, _)| *g != gen);
        self.changes.push((gen, changes));
        while self.changes.len() > 8 {
            self.changes.remove(0);
        }
        if self.phrase_snap {
            self.replan();
        }
    }

    fn sung(&self, gen: DeckGen) -> Option<&SungMap> {
        self.vocals.iter().find(|(g, _)| *g == gen).map(|(_, m)| m)
    }

    fn changes_of(&self, gen: DeckGen) -> &[f64] {
        self.changes
            .iter()
            .find(|(g, _)| *g == gen)
            .map(|(_, c)| c.as_slice())
            .unwrap_or(&[])
    }

    /// The hand always wins: drop the plan, keep the light on, re-plan next
    /// tick from wherever the hand left the decks. Never issues commands —
    /// the touching handler already did what the hand wanted.
    pub fn hands_on(&mut self) {
        if self.on {
            self.state = State::Idle;
            self.start_pending = false;
        }
    }

    pub fn shape_ready(&mut self, gen: DeckGen, shape: TrackShape) {
        self.shapes.retain(|(g, _)| *g != gen);
        self.shapes.push((gen, shape));
        // Analyses arrive whether or not the autopilot is on, and the
        // per-tick pruning only runs while it is: cap the vec so a long
        // session with AUTO DJ dark cannot grow it without bound. Only two
        // generations can ever be live at once.
        while self.shapes.len() > 8 {
            self.shapes.remove(0);
        }
    }

    fn shape(&self, gen: DeckGen) -> Option<&TrackShape> {
        self.shapes.iter().find(|(g, _)| *g == gen).map(|(_, s)| s)
    }

    pub fn tick(&mut self, obs: &AutoObs) -> Vec<AutoCmd> {
        if !self.on {
            return Vec::new();
        }
        // Shapes and latches for loads that no longer exist are dead.
        let gens = [obs.decks[0].gen, obs.decks[1].gen];
        self.shapes.retain(|(g, _)| gens.contains(g));
        self.vocals.retain(|(g, _)| gens.contains(g));
        self.changes.retain(|(g, _)| gens.contains(g));
        self.fired.retain(|g| gens.contains(g));
        self.ran_out.retain(|g| gens.contains(g));
        // Witness every load seen PLAYING within reach of its own end: only
        // such a load may later take the ended-deck hand-back.
        for d in &obs.decks {
            if d.load == AutoLoad::Loaded
                && d.playing
                && d.duration_secs > 0.0
                && d.position_secs >= d.duration_secs - END_SLACK_SECS
                && !self.ran_out.contains(&d.gen)
            {
                self.ran_out.push(d.gen);
            }
        }
        // A dropped plan or an aborted fade may have left the overlay
        // holding a pre-mute down: the first Idle tick lets it go.
        let mut sweep = Vec::new();
        if self.blend_dirty && matches!(self.state, State::Idle) {
            self.blend_dirty = false;
            sweep.push(AutoCmd::ClearBlend { deck: DeckId::A });
            sweep.push(AutoCmd::ClearBlend { deck: DeckId::B });
        }
        let mut cmds = match self.state.clone() {
            State::Idle => self.tick_idle(obs),
            State::Planned(plan) => self.tick_planned(obs, plan),
            State::Fading(fade) => self.tick_fading(obs, fade),
        };
        if !sweep.is_empty() {
            sweep.append(&mut cmds);
            return sweep;
        }
        cmds
    }

    fn tick_idle(&mut self, obs: &AutoObs) -> Vec<AutoCmd> {
        let loaded_playing =
            |d: DeckId| obs.deck(d).load == AutoLoad::Loaded && obs.deck(d).playing;
        let a = loaded_playing(DeckId::A);
        let b = loaded_playing(DeckId::B);
        if !a && !b {
            return self.tick_silent(obs);
        }
        self.start_pending = false;
        let out = match (a, b) {
            (true, false) => DeckId::A,
            (false, true) => DeckId::B,
            _ => {
                if obs.fader < 0.5 - LAND_EPS {
                    DeckId::A
                } else if obs.fader > 0.5 + LAND_EPS {
                    DeckId::B
                } else {
                    obs.leader_hint.unwrap_or(DeckId::A)
                }
            }
        };
        let o = *obs.deck(out);
        if o.loop_on {
            self.status = "loop held".to_string();
            return Vec::new();
        }
        if o.scratching {
            self.status = "hands on".to_string();
            return Vec::new();
        }
        if self.fired.contains(&o.gen) {
            // This load already had its one transition (a scrub brought it
            // back, or a hand cancelled the fade): ride to its end.
            self.status = "played, riding out".to_string();
            return Vec::new();
        }
        let Some(out_shape) = self.shape(o.gen).copied() else {
            self.status = "reading track".to_string();
            return Vec::new();
        };
        let incoming = out.other();
        let i = *obs.deck(incoming);
        match i.load {
            AutoLoad::Loaded => {}
            AutoLoad::Loading => {
                self.status = format!("waiting on {}", letter(incoming));
                return Vec::new();
            }
            AutoLoad::Empty => {
                if obs.queue_len > 0 {
                    self.status = "loading next".to_string();
                    return vec![AutoCmd::PumpQueue];
                }
                self.status = "queue empty".to_string();
                return Vec::new();
            }
            AutoLoad::Failed => {
                self.status = "load failed".to_string();
                return vec![AutoCmd::HandBack { retire: incoming, requeue: false }];
            }
        }
        let Some(in_shape) = self.shape(i.gen).copied() else {
            self.status = "reading track".to_string();
            return Vec::new();
        };
        // Build the plan. All shape times are SOURCE seconds; the fade
        // length converts to wall seconds at fire time from the rates then.
        let knob = obs.fade_secs_knob.max(0.05);
        let lead = o.bar_secs_src.unwrap_or(NO_GRID_MARGIN_SECS);
        let fire_raw = match self.style {
            AutoStyle::Outro => out_shape.outro_start_secs,
            AutoStyle::Body => {
                out_shape.outro_start_secs - knob as f64 * o.rate.max(0.05)
            }
        };
        // A trigger already behind the playhead fires one bar from now,
        // never on the spot — a beat of grace for the hand.
        let mut fire_at_src = if fire_raw <= o.position_secs {
            o.position_secs + lead
        } else {
            fire_raw
        };
        let cue_secs = match self.style {
            AutoStyle::Outro => 0.0,
            AutoStyle::Body => in_shape.intro_end_secs,
        };
        // Phrase snap first, the vocal guard last: a clash outranks a
        // boundary. Both stay inside the runway the grace and the tail
        // margin define.
        let lo = o.position_secs + lead;
        let hi = (o.duration_secs - lead).max(lo);
        if self.phrase_snap {
            fire_at_src = blend::snap_to_phrase(
                fire_at_src,
                self.changes_of(o.gen),
                o.bar_secs_src,
                (lo, hi),
            );
        }
        let mut duck_offset_src = None;
        if self.vocal_guard {
            // The guard runs with whatever exists. Only the OUTGOING map is
            // load-bearing: shifting the fire point moves OUT's window and
            // nothing else, so with only IN's map there is nothing to dodge
            // — the incoming timeline is anchored to its cue, not the fire.
            // With OUT known and IN unknown, the incoming track is treated
            // as singing throughout, so the fire dodges every OUT phrase.
            if let Some(out_map) = self.sung(o.gen) {
                let always = SungMap(vec![(0.0, f64::MAX)]);
                let in_map = self.sung(i.gen).unwrap_or(&always);
                let fade_src = knob as f64 * o.rate.max(0.05);
                let (guarded, duck) = blend::vocal_guard(
                    fire_at_src,
                    fade_src,
                    out_map,
                    in_map,
                    cue_secs,
                    o.bar_secs_src.unwrap_or(2.0),
                    (lo, hi),
                );
                fire_at_src = guarded;
                duck_offset_src = duck;
            }
        }
        let deadline_src = (o.duration_secs - lead).max(o.position_secs);
        // RANDOM resolves here, once per plan: everything downstream —
        // prep's pre-mutes, the fire's ramps — reads the one rolled value.
        if self.brain_random {
            self.brain = self.roll_brain();
        }
        self.state = State::Planned(Plan {
            out,
            incoming,
            out_gen: o.gen,
            style: self.style,
            knob,
            cue_secs,
            fire_at_src,
            deadline_src,
            prepped: false,
            abandoned: false,
            duck_offset_src,
            prep_medium: None,
        });
        self.set_countdown(obs, out, incoming, fire_at_src);
        Vec::new()
    }

    /// Nothing is playing: continue the set if a track ended under us,
    /// start it if switch-on asked, hold otherwise.
    fn tick_silent(&mut self, obs: &AutoObs) -> Vec<AutoCmd> {
        for deck in [DeckId::A, DeckId::B] {
            let d = obs.deck(deck);
            if d.load == AutoLoad::Loaded
                && !d.playing
                && !d.scratching
                && d.duration_secs > 0.0
                && d.position_secs >= d.duration_secs - END_SLACK_SECS
                && self.ran_out.contains(&d.gen)
            {
                // Ran off its end — witnessed: this load was OBSERVED
                // playing into its final stretch, so the stop is the file
                // running out, not a hand that paused, seeked or scratched
                // the record to the end. Hand it back and restart the set
                // on whatever the pump brings.
                self.start_pending = true;
                self.status = "next".to_string();
                return vec![AutoCmd::HandBack { retire: deck, requeue: true }];
            }
        }
        if self.start_pending {
            let loaded = |d: DeckId| obs.deck(d).load == AutoLoad::Loaded;
            let candidate = match (loaded(DeckId::A), loaded(DeckId::B)) {
                (true, true) => Some(if obs.fader > 0.5 + LAND_EPS {
                    DeckId::B
                } else {
                    DeckId::A
                }),
                (true, false) => Some(DeckId::A),
                (false, true) => Some(DeckId::B),
                (false, false) => None,
            };
            if let Some(deck) = candidate {
                self.start_pending = false;
                self.status = "starting".to_string();
                return vec![AutoCmd::StartSet { deck }];
            }
            if obs.queue_len > 0 {
                self.status = "loading next".to_string();
                return vec![AutoCmd::PumpQueue];
            }
            self.status = "no tracks".to_string();
            return Vec::new();
        }
        self.status = "holding".to_string();
        Vec::new()
    }

    /// Cue, phase-lock, and PRE-MUTE the incoming deck: it arrives at the
    /// fire point with its bass (and ducked vocals) already silent, so its
    /// start never leaks. The medium latches here — a deck pre-muted for
    /// EQ must not fire as Stems with an open bass lane.
    fn prep_in(
        &mut self,
        plan: &mut Plan,
        o: &AutoDeckObs,
        i: &AutoDeckObs,
        cmds: &mut Vec<AutoCmd>,
    ) {
        cmds.push(AutoCmd::CueIn { deck: plan.incoming, secs: plan.cue_secs });
        cmds.push(AutoCmd::SyncIn { deck: plan.incoming });
        let medium = blend::medium(self.brain, o.stems_ready, i.stems_ready);
        match medium {
            Medium::Fade => {}
            Medium::Eq => {
                cmds.push(AutoCmd::Blend {
                    deck: plan.incoming,
                    lane: Lane::Band(0),
                    gain: 0.0,
                });
                self.blend_dirty = true;
            }
            Medium::Stems => {
                cmds.push(AutoCmd::Blend {
                    deck: plan.incoming,
                    lane: Lane::Stem(blend::BASS),
                    gain: 0.0,
                });
                if plan.duck_offset_src.is_some() {
                    cmds.push(AutoCmd::Blend {
                        deck: plan.incoming,
                        lane: Lane::Stem(blend::VOCALS),
                        gain: 0.0,
                    });
                }
                self.blend_dirty = true;
            }
        }
        plan.prep_medium = Some(medium);
        plan.prepped = true;
    }

    fn tick_planned(&mut self, obs: &AutoObs, mut plan: Plan) -> Vec<AutoCmd> {
        let o = *obs.deck(plan.out);
        // The world moved under the plan — a new load, a knob move, a style
        // change: rebuild from scratch next tick.
        if o.gen != plan.out_gen
            || o.load != AutoLoad::Loaded
            || plan.style != self.style
            || (plan.knob - obs.fade_secs_knob.max(0.05)).abs() > 1e-3
        {
            self.state = State::Idle;
            return Vec::new();
        }
        if o.loop_on {
            self.status = "loop held".to_string();
            self.state = State::Planned(plan);
            return Vec::new();
        }
        if o.scratching {
            self.status = "hands on".to_string();
            self.state = State::Planned(plan);
            return Vec::new();
        }
        if !o.playing {
            if o.position_secs >= o.duration_secs - END_SLACK_SECS {
                // Ran out before the fade could fire (or while abandoned).
                self.fire_latch(plan.out_gen);
                self.state = State::Idle;
                self.start_pending = !obs.deck(plan.incoming).playing;
                self.status = "next".to_string();
                return vec![AutoCmd::HandBack { retire: plan.out, requeue: true }];
            }
            // Stopped short of the end without a hands_on: stand down.
            self.state = State::Idle;
            return Vec::new();
        }
        let i = *obs.deck(plan.incoming);
        match i.load {
            AutoLoad::Failed => {
                self.state = State::Idle;
                self.status = "load failed".to_string();
                return vec![AutoCmd::HandBack { retire: plan.incoming, requeue: false }];
            }
            AutoLoad::Empty => {
                self.state = State::Idle;
                return Vec::new();
            }
            _ => {}
        }
        if plan.abandoned {
            self.status = "missed, playing out".to_string();
            self.state = State::Planned(plan);
            return Vec::new();
        }
        let pos = o.position_secs;
        let lead = o.bar_secs_src.unwrap_or(NO_GRID_MARGIN_SECS);
        let mut cmds = Vec::new();
        if !plan.prepped && i.load == AutoLoad::Loaded && pos >= plan.fire_at_src - lead {
            // Cue first, then the one phase lock: the engine's Bar quantize
            // may trim the cue by up to half a bar, which is the accepted
            // price of landing in phase. Prep also pre-mutes (prep_in).
            self.prep_in(&mut plan, &o, &i, &mut cmds);
        }
        if pos < plan.fire_at_src {
            self.set_countdown(obs, plan.out, plan.incoming, plan.fire_at_src);
            self.state = State::Planned(plan);
            return cmds;
        }
        // At (or past) the trigger.
        if i.load != AutoLoad::Loaded {
            if pos >= plan.deadline_src {
                plan.abandoned = true;
                self.fire_latch(plan.out_gen);
                self.status = "missed, playing out".to_string();
            } else {
                self.status = format!("waiting on {}", letter(plan.incoming));
            }
            self.state = State::Planned(plan);
            return cmds;
        }
        // Wall fade length from the rates as they stand now. When prep
        // happened this same tick the observed IN rate predates the sync;
        // the error is bounded by sync's ±25% envelope and only stretches
        // or trims the blend, never the trigger.
        let in_rate = i.rate.max(0.05);
        let out_rate = o.rate.max(0.05);
        let mut fade = match plan.style {
            AutoStyle::Outro => {
                let intro = self
                    .shape(i.gen)
                    .map(|s| s.intro_end_secs)
                    .unwrap_or(f64::MAX);
                (plan.knob as f64).min(intro / in_rate)
            }
            AutoStyle::Body => plan.knob as f64,
        };
        let floor = i
            .bar_secs_src
            .map(|bar| bar / in_rate)
            .unwrap_or(1.0)
            .max(1.0);
        let runway = ((o.duration_secs - pos) / out_rate - 1.0).max(1.0);
        fade = fade.max(floor).min(runway).max(1.0);
        let target: f32 = match plan.incoming {
            DeckId::A => 0.0,
            DeckId::B => 1.0,
        };
        self.fire_latch(plan.out_gen);
        cmds.push(AutoCmd::PlayIn { deck: plan.incoming });
        if (obs.fader - target).abs() <= LAND_EPS {
            // The fader already sits on the incoming side (the hand faded
            // the old deck out beforehand): nothing to ramp — hand back on
            // the spot. Pre-mutes let go with it.
            self.state = State::Idle;
            self.status = "next".to_string();
            if self.blend_dirty {
                self.blend_dirty = false;
                cmds.push(AutoCmd::ClearBlend { deck: DeckId::A });
                cmds.push(AutoCmd::ClearBlend { deck: DeckId::B });
            }
            cmds.push(AutoCmd::HandBack { retire: plan.out, requeue: true });
            return cmds;
        }
        cmds.push(AutoCmd::BeginFade { to: plan.incoming, secs: fade as f32 });
        // The choreography against the fade the mixer now runs. The In-side
        // 0.0 steps were performed at prep; what remains rides the clock.
        let medium = plan.prep_medium.unwrap_or(Medium::Fade);
        let bar_wall = i.bar_secs_src.map(|bar| bar / in_rate).unwrap_or(2.0);
        let duck_wall = plan.duck_offset_src.map(|duck| duck / out_rate);
        let schedule: Vec<BlendStep> =
            blend::choreography(medium, fade, bar_wall, duck_wall)
                .into_iter()
                .filter(|step| step.at_wall > 0.0 || step.role == Role::Out)
                .collect();
        if schedule.is_empty() && self.blend_dirty {
            // The fade came out too short for the choreography: this runs
            // as a PLAIN fade, so the prep's pre-mutes must let go now —
            // not pop back at the landing after a bass-less blend.
            self.blend_dirty = false;
            cmds.push(AutoCmd::ClearBlend { deck: DeckId::A });
            cmds.push(AutoCmd::ClearBlend { deck: DeckId::B });
        }
        let status: &'static str = if schedule.is_empty() {
            "fading"
        } else {
            match medium {
                Medium::Stems => "fading (stems)",
                Medium::Eq if self.brain == MixBrain::Stems => {
                    "fading (eq — stems not ready)"
                }
                Medium::Eq => "fading (eq)",
                Medium::Fade => "fading",
            }
        };
        self.status = status.to_string();
        self.state = State::Fading(Fade {
            out: plan.out,
            target,
            schedule,
            elapsed_wall: 0.0,
            last_out_pos: pos,
            status,
        });
        cmds
    }

    fn tick_fading(&mut self, obs: &AutoObs, mut fade: Fade) -> Vec<AutoCmd> {
        let o = obs.deck(fade.out);
        if o.load != AutoLoad::Loaded {
            // The retiring deck vanished under us (a hand loaded something
            // new — hands_on normally catches this first): nothing left to
            // hand back. Any held overlay stays dirty and the sweep lets go.
            self.state = State::Idle;
            return Vec::new();
        }
        let landed = (obs.fader - fade.target).abs() <= LAND_EPS;
        let out_dead = !o.playing;
        if landed || out_dead {
            self.state = State::Idle;
            self.start_pending = !obs.deck(fade.out.other()).playing;
            self.status = "next".to_string();
            let mut cmds = Vec::new();
            if self.blend_dirty {
                self.blend_dirty = false;
                cmds.push(AutoCmd::ClearBlend { deck: DeckId::A });
                cmds.push(AutoCmd::ClearBlend { deck: DeckId::B });
            }
            cmds.push(AutoCmd::HandBack { retire: fade.out, requeue: true });
            return cmds;
        }
        // Listening time since the fire, off OUT's own playhead — the same
        // clock the countdown used. Emit every step whose moment passed.
        let delta = (o.position_secs - fade.last_out_pos).max(0.0) / o.rate.max(0.05);
        fade.elapsed_wall += delta;
        fade.last_out_pos = o.position_secs;
        let mut cmds = Vec::new();
        while let Some(step) = fade.schedule.first().copied() {
            if step.at_wall > fade.elapsed_wall {
                break;
            }
            fade.schedule.remove(0);
            let deck = match step.role {
                Role::Out => fade.out,
                Role::In => fade.out.other(),
            };
            cmds.push(AutoCmd::Blend { deck, lane: step.lane, gain: step.gain });
            self.blend_dirty = true;
        }
        self.status = fade.status.to_string();
        self.state = State::Fading(fade);
        cmds
    }

    fn fire_latch(&mut self, gen: DeckGen) {
        if !self.fired.contains(&gen) {
            self.fired.push(gen);
        }
    }

    fn set_countdown(
        &mut self,
        obs: &AutoObs,
        out: DeckId,
        incoming: DeckId,
        fire_at_src: f64,
    ) {
        let o = obs.deck(out);
        let wall = ((fire_at_src - o.position_secs) / o.rate.max(0.05)).max(0.0);
        let secs = wall.round() as u64;
        self.status = format!("→{} {}:{:02}", letter(incoming), secs / 60, secs % 60);
    }
}

fn letter(deck: DeckId) -> char {
    match deck {
        DeckId::A => 'A',
        DeckId::B => 'B',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-deck world the tests advance 50 ms at a time, with a fader
    /// that ramps linearly and snaps exactly onto its target — the mixer's
    /// Ramp semantics.
    struct World {
        obs: AutoObs,
        fade: Option<(f32, f32)>, // (target, step_per_tick)
    }

    const TICK: f64 = 0.05;

    fn deck(duration: f64) -> AutoDeckObs {
        AutoDeckObs {
            load: AutoLoad::Loaded,
            gen: 0,
            playing: false,
            position_secs: 0.0,
            duration_secs: duration,
            rate: 1.0,
            loop_on: false,
            scratching: false,
            bar_secs_src: Some(2.0), // 120 BPM bars
            stems_ready: false,
        }
    }

    fn world() -> World {
        World {
            obs: AutoObs {
                decks: [deck(300.0), deck(300.0)],
                fader: 0.0,
                queue_len: 0,
                fade_secs_knob: 8.0,
                leader_hint: None,
            },
            fade: None,
        }
    }

    impl World {
        fn tick(&mut self, pilot: &mut AutoPilot) -> Vec<AutoCmd> {
            for d in &mut self.obs.decks {
                if d.playing {
                    d.position_secs += TICK * d.rate;
                    if d.position_secs >= d.duration_secs {
                        d.position_secs = d.duration_secs;
                        d.playing = false;
                    }
                }
            }
            if let Some((target, step)) = self.fade {
                let delta = target - self.obs.fader;
                if delta.abs() <= step {
                    self.obs.fader = target;
                    self.fade = None;
                } else {
                    self.obs.fader += step * delta.signum();
                }
            }
            let cmds = pilot.tick(&self.obs);
            // Execute the world's share of the commands, as the host would.
            for cmd in &cmds {
                match cmd {
                    AutoCmd::PlayIn { deck } => {
                        self.obs.decks[deck.index()].playing = true;
                    }
                    AutoCmd::StartSet { deck } => {
                        self.obs.decks[deck.index()].playing = true;
                        self.obs.fader = if *deck == DeckId::B { 1.0 } else { 0.0 };
                    }
                    AutoCmd::CueIn { deck, secs } => {
                        self.obs.decks[deck.index()].position_secs = *secs;
                    }
                    AutoCmd::BeginFade { to, secs } => {
                        let target = if *to == DeckId::B { 1.0 } else { 0.0 };
                        let step = (target - self.obs.fader).abs()
                            / (*secs as f32 / TICK as f32);
                        self.fade = Some((target, step));
                    }
                    AutoCmd::HandBack { retire, .. } => {
                        self.obs.decks[retire.index()] =
                            AutoDeckObs { load: AutoLoad::Empty, ..Default::default() };
                    }
                    _ => {}
                }
            }
            cmds
        }

        /// Run until the pilot emits something, up to `max` ticks.
        fn run_until_cmds(
            &mut self,
            pilot: &mut AutoPilot,
            max: usize,
        ) -> (Vec<AutoCmd>, usize) {
            for n in 0..max {
                let cmds = self.tick(pilot);
                if !cmds.is_empty() {
                    return (cmds, n);
                }
            }
            (Vec::new(), max)
        }
    }

    fn shape(intro: f64, outro: f64) -> TrackShape {
        TrackShape { intro_end_secs: intro, outro_start_secs: outro, detected: true }
    }

    fn armed_pilot(w: &mut World) -> AutoPilot {
        let mut pilot = AutoPilot::new();
        // The transition-timing tests predate the smart media: pin them to
        // the plain fade so their exact command sequences stay exact.
        pilot.brain = MixBrain::Fade;
        pilot.set_on(true);
        w.obs.decks[0].gen = 1;
        w.obs.decks[1].gen = 2;
        w.obs.decks[0].playing = true;
        pilot.shape_ready(1, shape(20.0, 280.0));
        pilot.shape_ready(2, shape(16.0, 280.0));
        pilot
    }

    #[test]
    fn an_outro_transition_runs_cue_sync_play_fade_and_hands_back() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        // Park A just before the prep point (outro 280, lead one 2 s bar).
        w.obs.decks[0].position_secs = 277.0;
        let (cmds, _) = w.run_until_cmds(&mut pilot, 40);
        assert_eq!(
            cmds,
            vec![
                AutoCmd::CueIn { deck: DeckId::B, secs: 0.0 },
                AutoCmd::SyncIn { deck: DeckId::B },
            ],
            "prep lands one bar before the fire point"
        );
        let (cmds, ticks) = w.run_until_cmds(&mut pilot, 60);
        assert_eq!(cmds[0], AutoCmd::PlayIn { deck: DeckId::B });
        let AutoCmd::BeginFade { to: DeckId::B, secs } = cmds[1] else {
            panic!("expected a fade, got {cmds:?}");
        };
        // Fired within one tick of the outro point.
        let pos = w.obs.decks[0].position_secs;
        assert!(
            (pos - 280.0).abs() <= 0.05 + 1e-9,
            "fired at {pos}, one tick around 280"
        );
        assert!(ticks < 60);
        // Outro style: fade = min(knob 8, IN intro 16) = 8.
        assert!((secs - 8.0).abs() < 0.5, "fade of {secs}");
        // Ride the fade to its landing.
        let (cmds, _) = w.run_until_cmds(&mut pilot, 200);
        assert_eq!(
            cmds,
            vec![AutoCmd::HandBack { retire: DeckId::A, requeue: true }],
            "the landed fade retires the outgoing deck"
        );
        assert!(w.obs.decks[1].playing, "the set continues on B");
    }

    #[test]
    fn a_body_transition_cues_past_the_intro_and_lands_at_the_outro() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        pilot.set_style(AutoStyle::Body);
        // Fade must END at 280: with an 8 s knob it starts at 272.
        w.obs.decks[0].position_secs = 269.0;
        let (cmds, _) = w.run_until_cmds(&mut pilot, 40);
        assert_eq!(
            cmds[0],
            AutoCmd::CueIn { deck: DeckId::B, secs: 16.0 },
            "body style starts the incoming track past its intro"
        );
        let (cmds, _) = w.run_until_cmds(&mut pilot, 80);
        let AutoCmd::BeginFade { secs, .. } = cmds[1] else {
            panic!("expected a fade, got {cmds:?}");
        };
        let pos = w.obs.decks[0].position_secs;
        assert!((pos - 272.0).abs() <= 0.1, "fade started at {pos}");
        assert!((secs - 8.0).abs() < 0.5);
    }

    #[test]
    fn a_hand_mid_fade_drops_the_plan_and_the_load_never_refires() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[0].position_secs = 279.5;
        w.run_until_cmds(&mut pilot, 40); // prep
        w.run_until_cmds(&mut pilot, 40); // fire
        pilot.hands_on();
        // The hand pulled the fader back to A: no hand-back may fire, and
        // this load never transitions again.
        w.fade = None;
        w.obs.fader = 0.0;
        for _ in 0..100 {
            assert!(w.tick(&mut pilot).is_empty(), "one transition per load");
        }
        assert_eq!(pilot.status(), "played, riding out");
    }

    #[test]
    fn switching_off_mid_fade_never_hands_back() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[0].position_secs = 279.5;
        w.run_until_cmds(&mut pilot, 40);
        w.run_until_cmds(&mut pilot, 40);
        pilot.set_on(false);
        for _ in 0..300 {
            assert!(w.tick(&mut pilot).is_empty(), "off means off");
        }
    }

    #[test]
    fn a_loading_incoming_deck_holds_until_the_deadline_then_rides_the_end() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[1].load = AutoLoad::Loading;
        w.obs.decks[0].position_secs = 279.5;
        // Past the trigger with IN still loading: no fire.
        let (cmds, _) = w.run_until_cmds(&mut pilot, 100);
        assert!(cmds.is_empty(), "held: {cmds:?}");
        // The deck plays to its end; the hand-back arrives anyway.
        w.obs.decks[0].position_secs = 299.9;
        let (cmds, _) = w.run_until_cmds(&mut pilot, 40);
        assert_eq!(
            cmds,
            vec![AutoCmd::HandBack { retire: DeckId::A, requeue: true }]
        );
    }

    #[test]
    fn a_failed_incoming_load_is_retired_without_requeue() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[1].load = AutoLoad::Failed;
        w.obs.decks[0].position_secs = 100.0;
        let (cmds, _) = w.run_until_cmds(&mut pilot, 10);
        assert_eq!(
            cmds,
            vec![AutoCmd::HandBack { retire: DeckId::B, requeue: false }],
            "a track that never sounded is not recycled"
        );
    }

    #[test]
    fn an_empty_incoming_deck_pumps_the_queue_once_a_tick_at_most() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[1] = AutoDeckObs { load: AutoLoad::Empty, ..Default::default() };
        w.obs.queue_len = 2;
        let cmds = w.tick(&mut pilot);
        assert_eq!(cmds, vec![AutoCmd::PumpQueue]);
        // Queue empty instead: hold and say so.
        w.obs.queue_len = 0;
        let cmds = w.tick(&mut pilot);
        assert!(cmds.is_empty());
        assert_eq!(pilot.status(), "queue empty");
    }

    #[test]
    fn a_fader_already_on_the_incoming_side_skips_the_fade() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.fader = 1.0; // the hand faded A out long ago
        w.obs.decks[0].position_secs = 279.9;
        let (first, _) = w.run_until_cmds(&mut pilot, 80);
        let cmds = if first.iter().any(|c| matches!(c, AutoCmd::PlayIn { .. })) {
            first
        } else {
            w.run_until_cmds(&mut pilot, 80).0
        };
        assert!(cmds.contains(&AutoCmd::PlayIn { deck: DeckId::B }));
        assert!(
            cmds.contains(&AutoCmd::HandBack { retire: DeckId::A, requeue: true }),
            "no ramp to run: hand back on the spot ({cmds:?})"
        );
        assert!(!cmds.iter().any(|c| matches!(c, AutoCmd::BeginFade { .. })));
    }

    #[test]
    fn switch_on_with_nothing_playing_starts_the_favoured_loaded_deck() {
        let mut w = world();
        let mut pilot = AutoPilot::new();
        w.obs.decks[0].gen = 1;
        w.obs.decks[1].gen = 2;
        w.obs.fader = 1.0;
        pilot.set_on(true);
        let cmds = w.tick(&mut pilot);
        assert_eq!(cmds, vec![AutoCmd::StartSet { deck: DeckId::B }]);
        assert!(w.obs.decks[1].playing);
    }

    #[test]
    fn switch_on_with_empty_decks_pumps_then_starts() {
        let mut w = world();
        let mut pilot = AutoPilot::new();
        w.obs.decks = [
            AutoDeckObs { load: AutoLoad::Empty, ..Default::default() },
            AutoDeckObs { load: AutoLoad::Empty, ..Default::default() },
        ];
        w.obs.queue_len = 1;
        pilot.set_on(true);
        let cmds = w.tick(&mut pilot);
        assert_eq!(cmds, vec![AutoCmd::PumpQueue]);
        // The pump loaded deck A.
        w.obs.decks[0] = AutoDeckObs { gen: 9, ..deck(300.0) };
        w.obs.queue_len = 0;
        let cmds = w.tick(&mut pilot);
        assert_eq!(cmds, vec![AutoCmd::StartSet { deck: DeckId::A }]);
    }

    #[test]
    fn an_ended_deck_under_a_quiet_autopilot_continues_the_set() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        // Queue stays empty so no plan ever arms; A plays out.
        w.obs.decks[1] = AutoDeckObs { load: AutoLoad::Empty, ..Default::default() };
        w.obs.decks[0].position_secs = 299.8;
        let mut guard = 0;
        loop {
            let cmds = w.tick(&mut pilot);
            if !cmds.is_empty() {
                assert_eq!(
                    cmds,
                    vec![AutoCmd::HandBack { retire: DeckId::A, requeue: true }]
                );
                break;
            }
            guard += 1;
            assert!(guard < 40, "the ended deck was never handed back");
        }
    }

    #[test]
    fn a_parked_deck_seeked_to_the_end_is_never_ejected() {
        // The operator paused the deck (hands_on kept autopilot lit), then
        // clicked the far end of the overview to inspect the outro. The
        // deck now reads stopped-at-the-end — but it was never OBSERVED
        // playing there, so the ended-deck arm must hold, not eject.
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        pilot.hands_on();
        w.obs.decks[0].playing = false;
        w.obs.decks[0].position_secs = 299.9;
        for _ in 0..40 {
            assert!(w.tick(&mut pilot).is_empty(), "the parked track survives");
        }
        assert_eq!(pilot.status(), "holding");
    }

    #[test]
    fn a_scratch_holding_the_record_at_the_end_delays_the_hand_back() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        // Witness the deck genuinely playing into its final stretch...
        w.obs.decks[1] = AutoDeckObs { load: AutoLoad::Empty, ..Default::default() };
        w.obs.decks[0].position_secs = 299.8;
        w.tick(&mut pilot);
        // ...then it stops under a hand on the record: no eject while the
        // scratch lasts, hand-back the moment it lets go.
        w.obs.decks[0].playing = false;
        w.obs.decks[0].position_secs = 300.0;
        w.obs.decks[0].scratching = true;
        for _ in 0..10 {
            assert!(w.tick(&mut pilot).is_empty(), "a hand on the record wins");
        }
        w.obs.decks[0].scratching = false;
        let (cmds, _) = w.run_until_cmds(&mut pilot, 10);
        assert_eq!(
            cmds,
            vec![AutoCmd::HandBack { retire: DeckId::A, requeue: true }]
        );
    }

    #[test]
    fn a_knob_move_replans_an_armed_transition() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[0].position_secs = 100.0;
        w.tick(&mut pilot); // builds the plan
        assert!(pilot.status().starts_with("→B"), "{}", pilot.status());
        w.obs.fade_secs_knob = 16.0;
        w.tick(&mut pilot); // plan dropped
        w.tick(&mut pilot); // rebuilt with the new knob
        assert!(pilot.status().starts_with("→B"));
    }

    #[test]
    fn a_looping_outgoing_deck_is_held_not_fought() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[0].loop_on = true;
        w.obs.decks[0].position_secs = 290.0;
        for _ in 0..10 {
            assert!(w.tick(&mut pilot).is_empty());
        }
        assert_eq!(pilot.status(), "loop held");
    }
    #[test]
    fn a_stems_transition_pre_mutes_swaps_and_lets_go() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        pilot.brain = MixBrain::Stems;
        w.obs.decks[0].stems_ready = true;
        w.obs.decks[1].stems_ready = true;
        w.obs.decks[0].position_secs = 277.0;
        // Prep arrives with the incoming bass already silenced.
        let (cmds, _) = w.run_until_cmds(&mut pilot, 40);
        assert_eq!(
            cmds,
            vec![
                AutoCmd::CueIn { deck: DeckId::B, secs: 0.0 },
                AutoCmd::SyncIn { deck: DeckId::B },
                AutoCmd::Blend {
                    deck: DeckId::B,
                    lane: Lane::Stem(blend::BASS),
                    gain: 0.0
                },
            ]
        );
        // Fire.
        let (cmds, _) = w.run_until_cmds(&mut pilot, 60);
        assert_eq!(cmds[0], AutoCmd::PlayIn { deck: DeckId::B });
        assert!(matches!(cmds[1], AutoCmd::BeginFade { to: DeckId::B, .. }));
        assert_eq!(pilot.status(), "fading (stems)");
        // The basslines swap on the bar nearest the fade's middle (8 s
        // fade, 2 s bars: at 4 s).
        let (cmds, ticks) = w.run_until_cmds(&mut pilot, 200);
        assert_eq!(
            cmds,
            vec![
                AutoCmd::Blend {
                    deck: DeckId::A,
                    lane: Lane::Stem(blend::BASS),
                    gain: 0.0
                },
                AutoCmd::Blend {
                    deck: DeckId::B,
                    lane: Lane::Stem(blend::BASS),
                    gain: 1.0
                },
            ]
        );
        let swap_secs = ticks as f64 * TICK;
        assert!(
            (swap_secs - 4.0).abs() <= 0.2,
            "the swap landed {swap_secs} s into the fade"
        );
        // Landing lets the overlay go before the hand-back.
        let (cmds, _) = w.run_until_cmds(&mut pilot, 200);
        assert_eq!(
            cmds,
            vec![
                AutoCmd::ClearBlend { deck: DeckId::A },
                AutoCmd::ClearBlend { deck: DeckId::B },
                AutoCmd::HandBack { retire: DeckId::A, requeue: true },
            ]
        );
    }

    #[test]
    fn missing_stems_degrade_the_pair_to_eq_and_say_so() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        pilot.brain = MixBrain::Stems;
        w.obs.decks[0].stems_ready = true;
        // The incoming deck has no stems: the pair runs the EQ blend.
        w.obs.decks[1].stems_ready = false;
        w.obs.decks[0].position_secs = 277.0;
        let (cmds, _) = w.run_until_cmds(&mut pilot, 40);
        assert!(
            cmds.contains(&AutoCmd::Blend {
                deck: DeckId::B,
                lane: Lane::Band(0),
                gain: 0.0
            }),
            "the EQ low band carries the pre-mute: {cmds:?}"
        );
        w.run_until_cmds(&mut pilot, 60);
        assert_eq!(pilot.status(), "fading (eq — stems not ready)");
    }

    #[test]
    fn the_vocal_guard_moves_the_fire_off_a_sung_phrase() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        // OUT sings across the outro point; IN sings from the top. The only
        // clean window inside ±2 bars starts two bars late.
        pilot.vocals_ready(1, SungMap(vec![(279.0, 283.0)]));
        pilot.vocals_ready(2, SungMap(vec![(0.0, 30.0)]));
        w.obs.decks[0].position_secs = 270.0;
        w.run_until_cmds(&mut pilot, 300); // prep (two bars before 284)
        let (cmds, _) = w.run_until_cmds(&mut pilot, 100);
        assert!(matches!(cmds[0], AutoCmd::PlayIn { .. }), "{cmds:?}");
        let fired_at = w.obs.decks[0].position_secs;
        assert!(
            (fired_at - 284.0).abs() <= 0.1,
            "the guard fires at {fired_at}, two bars past the sung outro"
        );
    }

    #[test]
    fn phrase_snap_lands_the_fire_on_a_detected_change() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        pilot.changes_ready(1, vec![278.5]);
        w.obs.decks[0].position_secs = 270.0;
        w.run_until_cmds(&mut pilot, 200); // prep
        let (cmds, _) = w.run_until_cmds(&mut pilot, 300);
        assert!(matches!(cmds[0], AutoCmd::PlayIn { .. }), "{cmds:?}");
        let fired_at = w.obs.decks[0].position_secs;
        assert!(
            (fired_at - 278.5).abs() <= 0.1,
            "snapped fire at {fired_at}"
        );
    }

    #[test]
    fn a_hand_after_prep_lets_the_overlay_go() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        pilot.brain = MixBrain::Stems;
        w.obs.decks[0].stems_ready = true;
        w.obs.decks[1].stems_ready = true;
        w.obs.decks[0].position_secs = 278.5;
        let (cmds, _) = w.run_until_cmds(&mut pilot, 40); // prep pre-mutes
        assert!(cmds.iter().any(|c| matches!(c, AutoCmd::Blend { .. })));
        pilot.hands_on();
        let cmds = w.tick(&mut pilot);
        assert_eq!(
            &cmds[..2],
            &[
                AutoCmd::ClearBlend { deck: DeckId::A },
                AutoCmd::ClearBlend { deck: DeckId::B },
            ],
            "the dropped plan's pre-mute is released: {cmds:?}"
        );
    }

    #[test]
    fn a_brain_change_replans_an_armed_transition() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[0].position_secs = 100.0;
        w.tick(&mut pilot);
        assert!(pilot.status().starts_with("→B"));
        pilot.set_brain(MixBrain::Eq);
        w.tick(&mut pilot); // dropped
        w.tick(&mut pilot); // rebuilt under the new brain
        assert!(pilot.status().starts_with("→B"));
        assert_eq!(pilot.brain, MixBrain::Eq);
    }

    /// RANDOM is a roll per plan, not a fourth brain: the resolver must
    /// reach all three over a run of transitions, and picking a fixed
    /// brain on the panel ends the rolling.
    #[test]
    fn random_rolls_cover_every_brain_and_a_fixed_pick_ends_them() {
        let mut pilot = AutoPilot::new();
        pilot.set_brain_random(true);
        let mut seen = [false; 3];
        for _ in 0..64 {
            match pilot.roll_brain() {
                MixBrain::Fade => seen[0] = true,
                MixBrain::Eq => seen[1] = true,
                MixBrain::Stems => seen[2] = true,
            }
        }
        assert_eq!(seen, [true; 3], "64 rolls must visit all three brains");
        pilot.set_brain(MixBrain::Eq);
        assert!(!pilot.brain_random, "a fixed pick switches RANDOM off");
        assert_eq!(pilot.brain, MixBrain::Eq);
    }

    #[test]
    fn a_short_fade_degrades_to_plain_and_releases_the_premute() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        pilot.brain = MixBrain::Stems;
        w.obs.decks[0].stems_ready = true;
        w.obs.decks[1].stems_ready = true;
        // A knob under two bars: the choreography has no room and the
        // transition must run as a PLAIN fade — including letting the
        // prep's pre-mute go at fire, not at landing.
        w.obs.fade_secs_knob = 2.0;
        w.obs.decks[0].position_secs = 277.0;
        let (cmds, _) = w.run_until_cmds(&mut pilot, 40);
        assert!(
            cmds.contains(&AutoCmd::Blend {
                deck: DeckId::B,
                lane: Lane::Stem(blend::BASS),
                gain: 0.0
            }),
            "prep still pre-mutes before the fade length is known: {cmds:?}"
        );
        let (cmds, _) = w.run_until_cmds(&mut pilot, 60);
        assert!(matches!(cmds[0], AutoCmd::PlayIn { .. }), "{cmds:?}");
        assert!(
            cmds.contains(&AutoCmd::ClearBlend { deck: DeckId::B }),
            "a degraded-to-plain fade releases the pre-mute at fire: {cmds:?}"
        );
        // And the fade itself schedules no further blend moves.
        let (cmds, _) = w.run_until_cmds(&mut pilot, 200);
        assert!(
            !cmds.iter().any(|c| matches!(c, AutoCmd::Blend { .. })),
            "nothing choreographs a plain fade: {cmds:?}"
        );
    }

    #[test]
    fn a_sung_map_arriving_after_arming_replans_the_fire() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        w.obs.decks[0].position_secs = 270.0;
        w.tick(&mut pilot); // arms at the raw outro, 280
        assert!(pilot.status().starts_with("→B"), "{}", pilot.status());
        // The lyrics bake lands AFTER the plan armed — the usual order.
        // The guard must re-plan, not sleep on data it asked for.
        pilot.vocals_ready(1, SungMap(vec![(279.0, 283.0)]));
        pilot.vocals_ready(2, SungMap(vec![(0.0, 30.0)]));
        w.run_until_cmds(&mut pilot, 300); // prep
        let (cmds, _) = w.run_until_cmds(&mut pilot, 100);
        assert!(matches!(cmds[0], AutoCmd::PlayIn { .. }), "{cmds:?}");
        let fired_at = w.obs.decks[0].position_secs;
        assert!(
            (fired_at - 284.0).abs() <= 0.1,
            "the late map still moved the fire to {fired_at}"
        );
    }

    #[test]
    fn the_guard_runs_with_only_the_outgoing_map() {
        let mut w = world();
        let mut pilot = armed_pilot(&mut w);
        // Only the OUT deck has vocal data; the incoming track counts as
        // singing throughout, so the fire still dodges the sung outro.
        pilot.vocals_ready(1, SungMap(vec![(279.0, 283.0)]));
        w.obs.decks[0].position_secs = 270.0;
        w.run_until_cmds(&mut pilot, 300); // prep
        let (cmds, _) = w.run_until_cmds(&mut pilot, 100);
        assert!(matches!(cmds[0], AutoCmd::PlayIn { .. }), "{cmds:?}");
        let fired_at = w.obs.decks[0].position_secs;
        assert!(
            (fired_at - 284.0).abs() <= 0.1,
            "one map is enough to shift the fire: {fired_at}"
        );
    }

}
