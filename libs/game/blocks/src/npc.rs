//! `game.npc` — villagers with somewhere to be.
//!
//! The difference between an NPC and a wander brain is **intent a watcher can
//! read**. A random walk looks like a bug; a figure that crosses the street,
//! sits on a bench for a while, then heads for a door looks like a person. That
//! legibility is the whole feature, and it comes from three things:
//!
//! 1. **Points of interest** the world actually contains ([`PoiSet`]) — benches,
//!    doors, lamps, wells. NPCs go to *things*, not to coordinates.
//! 2. **Utility scoring** ([`Npc::decide`]) — each candidate action is scored
//!    from distance, personality, novelty and time of day, and the best wins.
//!    No state machine to hand-author per game.
//! 3. **Personality** ([`Personality`]) — seeded per NPC, so two villagers given
//!    identical orders behave differently. Without it a crowd moves as one
//!    organism, which reads worse than a single NPC.
//!
//! Movement stays the mover sweep in `step_world`: this block writes `vel` and
//! reads the result, exactly like [`crate::character`]. It never repositions an
//! entity, so props, terrain and the 0.55 step-up all behave for NPCs
//! identically to how they behave for the player.
//!
//! NPC-vs-NPC is **steering**, not physics: movers pass through each other by
//! the v1 sim contract, so crowding is avoided by a separation force rather
//! than by collision. That is also why a hundred villagers converging on one
//! bench spread out around it instead of stacking into a tower.
//!
//! Determinism: every decision comes from sim state plus a per-NPC RNG seeded
//! at construction. Deliberately NOT `world.rand()` — a shared stream couples
//! each NPC's choices to how many other NPCs drew before it, so adding one
//! villager would change what every other villager decided. Host-side (Shared
//! tier): decisions run on the host and only the resulting motion replicates.

use makepad_game_math as gm;
use makepad_game_sim::sense;
use makepad_game_sim::{BodyKind, GameWorld, TICK_DT};
use makepad_math::*;

/// Seconds of simulated time in one day/night cycle of NPC preference. Not a
/// lighting cycle — just the clock that stops every villager wanting the same
/// thing at once.
pub const DAY_SECONDS: f32 = 240.0;

/// A place worth going to. Registered by the host (or derived from entity tags
/// via [`PoiSet::from_tags`]), so a village built out of stock props gains
/// destinations without the game script listing coordinates.
#[derive(Clone, Debug)]
pub struct Poi {
    pub pos: Vec3f,
    /// What kind of place: "bench", "door", "lamp", "well", "market"…
    pub tag: String,
    /// Entity it belongs to, 0 for a bare point. Used to drop POIs whose prop
    /// was removed.
    pub entity: u64,
    /// How many NPCs may use it at once. A bench seats two; a plaza is open.
    pub capacity: u8,
    /// Currently claimed slots — the reason a third villager walks past a full
    /// bench instead of standing inside the two already sitting on it.
    pub taken: u8,
}

impl Poi {
    pub fn new(pos: Vec3f, tag: impl Into<String>) -> Self {
        Self {
            pos,
            tag: tag.into(),
            entity: 0,
            capacity: 2,
            taken: 0,
        }
    }

    pub fn with_capacity(mut self, capacity: u8) -> Self {
        self.capacity = capacity;
        self
    }

    pub fn with_entity(mut self, entity: u64) -> Self {
        self.entity = entity;
        self
    }

    pub fn full(&self) -> bool {
        self.taken >= self.capacity
    }
}

/// Every destination in the world. Cloned with [`crate::Blocks`] so eval
/// rollback restores NPCs and their destinations together.
#[derive(Clone, Debug, Default)]
pub struct PoiSet {
    pub list: Vec<Poi>,
}

impl PoiSet {
    pub fn clear(&mut self) {
        self.list.clear();
    }

    pub fn push(&mut self, poi: Poi) -> usize {
        self.list.push(poi);
        self.list.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Build destinations from entities carrying any of these tags — a village
    /// composed from stock props becomes navigable without listing coordinates.
    /// Positions are lifted to the prop's top-front rather than its centre, so
    /// an NPC heading for a bench walks to the bench, not into it.
    pub fn from_tags(world: &GameWorld, tags: &[(&str, u8)]) -> Self {
        let mut set = Self::default();
        for e in &world.entities {
            if let Some((tag, capacity)) = tags.iter().find(|(t, _)| *t == e.tag) {
                // Stand just outside the footprint on the +x side; any
                // deterministic offset beats standing inside the geometry.
                let stand = vec3f(
                    e.pos.x + e.half.x + 0.6,
                    e.pos.y - e.half.y,
                    e.pos.z,
                );
                set.push(
                    Poi::new(stand, *tag)
                        .with_capacity(*capacity)
                        .with_entity(e.id),
                );
            }
        }
        set
    }

    /// Forget destinations whose prop is gone.
    pub fn reconcile(&mut self, world: &GameWorld) {
        self.list
            .retain(|p| p.entity == 0 || world.entity(p.entity).is_some());
    }
}

/// Per-NPC temperament, derived from its seed. These are the knobs that make
/// two villagers with identical config behave unlike each other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Personality {
    /// Walk speed multiplier, ~0.8–1.25.
    pub haste: f32,
    /// How long it dwells once it arrives somewhere, ~0.6–1.6.
    pub patience: f32,
    /// Weight on loitering near other NPCs, 0–1.
    pub sociability: f32,
    /// Weight on wandering somewhere new rather than a known POI, 0–1.
    pub curiosity: f32,
    /// Pull back toward home, 0–1.
    pub homebody: f32,
    /// Phase offset into the day cycle, so preferences don't align.
    pub clock_offset: f32,
}

/// What an NPC is currently doing. The set is small on purpose: legible
/// routines come from *sequencing* these, not from having twenty verbs.
#[derive(Clone, Debug, PartialEq)]
pub enum Activity {
    /// Standing still, looking around.
    Idle { left: f32 },
    /// Walking to a place. `poi` is the claimed destination, if any.
    Travel { goal: Vec3f, poi: Option<usize> },
    /// Arrived and staying a while (sitting on the bench, waiting at the door).
    Dwell { left: f32, poi: Option<usize> },
    /// Hanging around another NPC.
    Follow { who: u64, left: f32 },
}

impl Activity {
    pub fn name(&self) -> &'static str {
        match self {
            Activity::Idle { .. } => "idle",
            Activity::Travel { .. } => "travel",
            Activity::Dwell { .. } => "dwell",
            Activity::Follow { .. } => "follow",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NpcConfig {
    /// Base walk speed before personality haste.
    pub speed: f32,
    pub jump: f32,
    /// Steps up to this tall are walked over (matches the sweep's 0.55).
    pub step_up: f32,
    /// Obstacles up to this tall are jumped rather than walked around.
    pub jump_reach: f32,
    /// How close counts as arrived.
    pub arrive: f32,
    /// Personal space: NPCs inside this push apart.
    pub spacing: f32,
    /// How far it looks ahead for obstacles.
    pub look_ahead: f32,
    pub turn_rate: f32,
}

impl Default for NpcConfig {
    fn default() -> Self {
        Self {
            speed: 2.6,
            jump: 9.0,
            step_up: 0.55,
            jump_reach: 1.3,
            arrive: 1.1,
            spacing: 1.4,
            look_ahead: 1.6,
            turn_rate: 7.0,
        }
    }
}

/// One villager.
#[derive(Clone, Debug)]
pub struct Npc {
    pub entity: u64,
    pub config: NpcConfig,
    pub personality: Personality,
    /// Where it returns to when it has nothing better to do.
    pub home: Vec3f,
    pub activity: Activity,
    /// Private deterministic stream (see the module note on why not world.rand).
    rng: u64,
    /// Countdown to the next decision, so scoring runs a few times a second
    /// rather than every tick.
    decide_in: f32,
    /// Seconds spent blocked on the current goal; gives up past a threshold.
    stuck: f32,
    /// Remaining sidestep time, and which way.
    avoid_left: f32,
    avoid_side: f32,
    /// Recently visited POIs, so a villager doesn't ping-pong between two.
    recent: [usize; 4],
    recent_at: usize,
    /// Time since it last set out for anything, feeding the home urge.
    away: f32,
    /// Blocks picking `Follow` again straight away. Without it two sociable
    /// villagers standing near each other each choose to loiter near the
    /// other, forever — a mutual lock where both stand still for the rest of
    /// the game. Caught by the village scenario test, where one NPC moved
    /// exactly 0.0 units in ninety seconds.
    social_cooldown: f32,
}

impl Npc {
    pub fn new(entity: u64, config: NpcConfig, home: Vec3f, seed: u64) -> Self {
        // Mix the seed so adjacent entity ids don't produce near-identical
        // personalities — villagers spawned in a loop are the common case.
        let mut rng = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(entity.wrapping_mul(0xBF58_476D_1CE4_E5B9))
            | 1;
        let personality = Personality {
            haste: 0.8 + draw(&mut rng) * 0.45,
            patience: 0.6 + draw(&mut rng) * 1.0,
            sociability: draw(&mut rng),
            curiosity: draw(&mut rng),
            homebody: draw(&mut rng),
            clock_offset: draw(&mut rng),
        };
        Self {
            entity,
            config,
            personality,
            home,
            activity: Activity::Idle {
                left: draw(&mut rng) * 2.0,
            },
            decide_in: draw(&mut rng) * 0.5,
            rng,
            stuck: 0.0,
            avoid_left: 0.0,
            avoid_side: 1.0,
            recent: [usize::MAX; 4],
            recent_at: 0,
            away: 0.0,
            social_cooldown: 0.0,
        }
    }

    fn unit(&mut self) -> f32 {
        draw(&mut self.rng)
    }

    fn remember(&mut self, poi: usize) {
        self.recent[self.recent_at % self.recent.len()] = poi;
        self.recent_at = self.recent_at.wrapping_add(1);
    }

    fn visited_recently(&self, poi: usize) -> bool {
        self.recent.contains(&poi)
    }

    /// Release whatever POI slot this NPC holds.
    fn release(&mut self, pois: &mut PoiSet) {
        let claimed = match self.activity {
            Activity::Travel { poi, .. } | Activity::Dwell { poi, .. } => poi,
            _ => None,
        };
        if let Some(i) = claimed {
            if let Some(p) = pois.list.get_mut(i) {
                p.taken = p.taken.saturating_sub(1);
            }
        }
    }

    /// Score candidate actions and commit to the best. The scores are
    /// deliberately simple and additive — legibility comes from POIs and
    /// personality, not from an elaborate planner.
    fn decide(&mut self, world: &GameWorld, pois: &mut PoiSet, pos: Vec3f, day: f32) {
        self.release(pois);
        let p = self.personality;
        let clock = (day + p.clock_offset).fract();

        let mut best_score = 0.0f32;
        let mut best: Option<(Vec3f, Option<usize>)> = None;

        for (i, poi) in pois.list.iter().enumerate() {
            if poi.full() {
                continue;
            }
            let d = planar_distance(pos, poi.pos);
            // Near things are more attractive, but not overwhelmingly so, or
            // every villager parks at whatever is closest and never moves.
            let near = 1.0 / (1.0 + d * 0.08);
            let novelty = if self.visited_recently(i) { 0.25 } else { 1.0 };
            let taste = tag_appeal(&poi.tag, clock, &p);
            let jitter = 0.85 + self.unit() * 0.3;
            let score = taste * near * novelty * jitter;
            if score > best_score {
                best_score = score;
                best = Some((poi.pos, Some(i)));
            }
        }

        // Loiter near somebody — only worth considering for sociable types,
        // and never twice running (see `social_cooldown`).
        if p.sociability > 0.35 && self.social_cooldown <= 0.0 {
            if let Some((who, wpos)) = nearest_other_npc(world, self.entity, pos, 22.0) {
                let d = planar_distance(pos, wpos);
                let score = p.sociability * (1.0 / (1.0 + d * 0.1)) * (0.8 + self.unit() * 0.4);
                if score > best_score {
                    self.activity = Activity::Follow {
                        who,
                        left: 3.0 + p.patience * 5.0,
                    };
                    // Long enough that the next choice is something else, so a
                    // pair of villagers breaks up instead of standing together
                    // permanently.
                    self.social_cooldown = 14.0 + p.patience * 10.0;
                    self.away += 1.0;
                    return;
                }
            }
        }

        // Head home — grows the longer it has been out.
        let home_score = p.homebody * (self.away / 45.0).clamp(0.0, 1.2) * (0.9 + self.unit() * 0.2);
        if home_score > best_score {
            self.away = 0.0;
            self.activity = Activity::Travel {
                goal: self.home,
                poi: None,
            };
            return;
        }

        // Wander: the fallback, and the reason a village without POIs still
        // has movement in it.
        let wander_score = (0.3 + p.curiosity * 0.55) * (0.8 + self.unit() * 0.4);
        if best.is_none() || wander_score > best_score {
            let angle = self.unit() * std::f32::consts::TAU;
            let radius = 3.0 + self.unit() * 9.0;
            let (s, c) = gm::sincos(angle);
            let mut goal = vec3f(pos.x + c * radius, pos.y, pos.z + s * radius);
            // Leash to the village. An unbiased random walk has no centre, so
            // over a few minutes villagers drift off the map — the trace had
            // one 43 units out with every destination inside 18. Past the
            // leash the step is aimed home instead of outward, which reads as
            // "heading back" rather than as a wall.
            const LEASH: f32 = 26.0;
            let from_home = planar_distance(pos, self.home);
            if from_home > LEASH {
                let back = normalize_planar(vec3f(
                    self.home.x - pos.x,
                    0.0,
                    self.home.z - pos.z,
                ));
                goal = vec3f(
                    pos.x + back.x * radius,
                    pos.y,
                    pos.z + back.z * radius,
                );
            }
            self.activity = Activity::Travel { goal, poi: None };
            self.away += 1.0;
            return;
        }

        if let Some((goal, poi)) = best {
            if let Some(i) = poi {
                if let Some(slot) = pois.list.get_mut(i) {
                    slot.taken = slot.taken.saturating_add(1);
                }
                self.remember(i);
            }
            self.away += 1.0;
            self.activity = Activity::Travel { goal, poi };
        }
    }

    /// Drive phase: decide if it is time to, then steer. Runs before the sweep.
    pub fn tick(&mut self, world: &mut GameWorld, pois: &mut PoiSet) {
        let Some(me) = world.entity(self.entity) else {
            return;
        };
        if me.kind != BodyKind::Mover || me.attached_to != 0 {
            return;
        }
        let (pos, half, on_floor, blocked_by, speed_mult) =
            (me.pos, me.half, me.on_floor, me.hit_wall, me.speed_mult);
        let day = (world.tick as f32 * TICK_DT / DAY_SECONDS).fract();

        self.decide_in -= TICK_DT;
        self.social_cooldown = (self.social_cooldown - TICK_DT).max(0.0);
        if self.decide_in <= 0.0 {
            // Stagger so a crowd doesn't re-plan on the same tick.
            self.decide_in = 0.45 + self.unit() * 0.35;
            let idle_or_done = match &mut self.activity {
                Activity::Idle { left } => {
                    *left -= 0.45;
                    *left <= 0.0
                }
                Activity::Dwell { left, .. } => {
                    *left -= 0.45;
                    *left <= 0.0
                }
                Activity::Follow { who, left } => {
                    *left -= 0.45;
                    *left <= 0.0 || world.entity(*who).is_none()
                }
                Activity::Travel { .. } => false,
            };
            if idle_or_done {
                self.decide(world, pois, pos, day);
            }
        }

        // Arrival + give-up handling for the travelling case.
        if let Activity::Travel { goal, poi } = self.activity {
            let d = planar_distance(pos, goal);
            if d <= self.config.arrive {
                let dwell = 2.5 + self.personality.patience * 7.0;
                self.stuck = 0.0;
                self.avoid_left = 0.0;
                self.activity = Activity::Dwell { left: dwell, poi };
            } else if blocked_by != 0 {
                self.stuck += TICK_DT;
                if self.stuck > 3.5 {
                    // Truly wedged: drop the goal rather than grind into it
                    // forever. This is what stops the "NPC vibrating against a
                    // wall" failure that makes a scene look broken.
                    self.stuck = 0.0;
                    self.release(pois);
                    self.activity = Activity::Idle { left: 0.3 };
                }
            } else {
                self.stuck = (self.stuck - TICK_DT * 0.5).max(0.0);
            }
        }

        // Desired direction for this tick.
        let (mut dir, mut want_speed) = match &self.activity {
            Activity::Idle { .. } | Activity::Dwell { .. } => (vec3f(0.0, 0.0, 0.0), 0.0),
            Activity::Travel { goal, .. } => (
                vec3f(goal.x - pos.x, 0.0, goal.z - pos.z),
                self.config.speed * self.personality.haste,
            ),
            Activity::Follow { who, .. } => match world.entity(*who) {
                Some(target) => {
                    let to = vec3f(target.pos.x - pos.x, 0.0, target.pos.z - pos.z);
                    // Stand *near* them, not on them.
                    if to.length() < 2.2 {
                        (vec3f(0.0, 0.0, 0.0), 0.0)
                    } else {
                        (to, self.config.speed * self.personality.haste * 0.9)
                    }
                }
                None => (vec3f(0.0, 0.0, 0.0), 0.0),
            },
        };

        let mut jump = false;
        if want_speed > 0.0 {
            let len = dir.length();
            if len > 1.0e-4 {
                dir = dir * (1.0 / len);
            }

            // Obstacle handling: step / jump / go around / give up.
            if self.avoid_left > 0.0 {
                self.avoid_left -= TICK_DT;
                let (px, pz) = (-dir.z * self.avoid_side, dir.x * self.avoid_side);
                // Blend sidestep with the goal so it curves around rather than
                // walking a right angle and losing the thread.
                dir = normalize_planar(vec3f(dir.x + px * 1.5, 0.0, dir.z + pz * 1.5));
            } else if let Some(ob) = sense::obstacle_ahead(
                world,
                self.entity,
                pos,
                half,
                dir,
                self.config.look_ahead,
            ) {
                let feet = pos.y - half.y;
                let rise = ob.top - feet;
                // NOTE the step-up is a TERRAIN contract (step.rs CLIMB): a
                // static box blocks the sweep whatever its height, so there is
                // no "low enough to walk over" case here. Terrain steps need no
                // probe at all — the sweep already climbs them. Believing
                // otherwise is precisely the perception/physics disagreement
                // this module is built to avoid.
                if rise <= self.config.jump_reach
                    && on_floor
                    && sense::spot_clear(
                        world,
                        self.entity,
                        vec3f(
                            pos.x + dir.x * (ob.dist + half.x * 2.0),
                            ob.top + half.y + 0.1,
                            pos.z + dir.z * (ob.dist + half.x * 2.0),
                        ),
                        half,
                    )
                {
                    jump = true;
                } else {
                    // Prefer the side that is actually open; if both are,
                    // stay with the last choice so it doesn't dither.
                    let left = sense::side_clearance(world, self.entity, pos, half, dir, 1.0, 1.6);
                    let right = sense::side_clearance(world, self.entity, pos, half, dir, -1.0, 1.6);
                    self.avoid_side = match (left, right) {
                        (true, false) => 1.0,
                        (false, true) => -1.0,
                        (true, true) => self.avoid_side,
                        (false, false) => {
                            // Boxed in. Let the stuck timer end this goal.
                            self.stuck += TICK_DT * 4.0;
                            self.avoid_side
                        }
                    };
                    self.avoid_left = 0.6;
                }
            }

            // Don't walk off a ledge on the way somewhere.
            if let Some(ground) = sense::ground_ahead(world, pos, half, dir, 1.2) {
                if ground < pos.y - half.y - 1.6 {
                    self.avoid_side = -self.avoid_side;
                    self.avoid_left = 0.5;
                    want_speed *= 0.4;
                }
            }

            // Personal space. Movers pass through each other in v1, so this is
            // the only thing keeping a crowd from occupying one point.
            let push = separation(world, self.entity, pos, self.config.spacing);
            if push.length() > 1.0e-4 {
                dir = normalize_planar(vec3f(
                    dir.x + push.x * 1.2,
                    0.0,
                    dir.z + push.z * 1.2,
                ));
            }
        }

        let Some(me) = world.entity_mut(self.entity) else {
            return;
        };
        if want_speed <= 0.0 {
            me.vel.x = 0.0;
            me.vel.z = 0.0;
        } else {
            me.vel.x = dir.x * want_speed * speed_mult;
            me.vel.z = dir.z * want_speed * speed_mult;
            me.turn_rate = self.config.turn_rate;
        }
        if jump && me.on_floor {
            me.vel.y = self.config.jump;
        }
    }

    /// One line describing what this NPC is doing and why — the only way to
    /// inspect behaviour without a screen, and what the scenario tests assert
    /// against.
    pub fn trace(&self, world: &GameWorld) -> String {
        let pos = world
            .entity(self.entity)
            .map(|e| e.pos)
            .unwrap_or_default();
        let speed = world
            .entity(self.entity)
            .map(|e| (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt())
            .unwrap_or(0.0);
        format!(
            "npc {:>3} @({:6.2},{:5.2},{:6.2}) {:<6} spd={:4.2} stuck={:4.2} haste={:.2} soc={:.2}",
            self.entity,
            pos.x,
            pos.y,
            pos.z,
            self.activity.name(),
            speed,
            self.stuck,
            self.personality.haste,
            self.personality.sociability,
        )
    }
}

/// xorshift64* — one draw in [0,1). A free function so both the constructor
/// (before `self` exists) and the running NPC share exactly one generator.
fn draw(rng: &mut u64) -> f32 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 7;
    *rng ^= *rng << 17;
    (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
}

fn planar_distance(a: Vec3f, b: Vec3f) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

fn normalize_planar(v: Vec3f) -> Vec3f {
    let len = (v.x * v.x + v.z * v.z).sqrt();
    if len <= 1.0e-4 {
        vec3f(0.0, 0.0, 0.0)
    } else {
        vec3f(v.x / len, 0.0, v.z / len)
    }
}

/// How much this kind of place appeals right now. The clock is what stops a
/// village doing one thing in unison: benches read as an afternoon activity,
/// doors as an evening one, work as a morning one.
fn tag_appeal(tag: &str, clock: f32, p: &Personality) -> f32 {
    // Smooth bump centred on `at`, width ~0.35 of a day.
    let bump = |at: f32| {
        let mut d = (clock - at).abs();
        if d > 0.5 {
            d = 1.0 - d;
        }
        (1.0 - d * 2.4).max(0.15)
    };
    match tag {
        "bench" | "seat" => 0.9 * bump(0.55) * p.patience.min(1.2),
        "door" | "home" => 0.85 * bump(0.85) * (0.5 + p.homebody),
        "well" | "market" | "shop" => 1.0 * bump(0.25),
        "lamp" | "sign" => 0.5 * bump(0.9) * (0.4 + p.curiosity),
        "work" | "field" => 0.95 * bump(0.15),
        _ => 0.6 * (0.5 + p.curiosity),
    }
}

/// Nearest other mover carrying an npc-ish role. Movers only, so villagers
/// gather around each other and the player rather than around crates.
fn nearest_other_npc(
    world: &GameWorld,
    self_id: u64,
    from: Vec3f,
    range: f32,
) -> Option<(u64, Vec3f)> {
    let mut best: Option<(u64, Vec3f, f32)> = None;
    for e in &world.entities {
        if e.id == self_id || e.kind != BodyKind::Mover || e.attached_to != 0 {
            continue;
        }
        let d = planar_distance(from, e.pos);
        if d > range {
            continue;
        }
        if best.map_or(true, |(_, _, bd)| d < bd) {
            best = Some((e.id, e.pos, d));
        }
    }
    best.map(|(id, pos, _)| (id, pos))
}

/// Push away from movers inside `spacing`, weighted by how close they are.
fn separation(world: &GameWorld, self_id: u64, pos: Vec3f, spacing: f32) -> Vec3f {
    let mut push = vec3f(0.0, 0.0, 0.0);
    for e in &world.entities {
        if e.id == self_id || e.kind != BodyKind::Mover || e.attached_to != 0 {
            continue;
        }
        let dx = pos.x - e.pos.x;
        let dz = pos.z - e.pos.z;
        let d2 = dx * dx + dz * dz;
        if d2 >= spacing * spacing || d2 <= 1.0e-6 {
            continue;
        }
        let d = d2.sqrt();
        let strength = (spacing - d) / spacing;
        push.x += dx / d * strength;
        push.z += dz / d * strength;
    }
    push
}
