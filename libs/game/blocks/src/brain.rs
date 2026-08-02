//! Brains — `game.wander`, `game.chase`, `game.patrol`.
//!
//! These absorb the per-actor AI that the sandbox3d fixture writes by hand:
//! ~80 lines of `wanderer_tick` / `hunt_step` plus an O(n²) threat scan inside
//! `on_tick`, re-run in script every frame for every actor. Engine-side they
//! cost nothing per script call and, being deterministic, replicate for free.

use makepad_game_math as gm;
use makepad_game_sim::{BodyKind, GameWorld, TICK_DT};
use makepad_math::*;

#[derive(Clone, Debug, PartialEq)]
pub enum BrainKind {
    /// Amble to a random point within `range` of home, pause, repeat.
    Wander {
        home: Vec3f,
        range: f32,
        speed: f32,
        pause: f32,
    },
    /// Hunt the nearest entity carrying `tag` (or a specific id).
    Chase {
        tag: String,
        target: u64,
        range: f32,
        catch: f32,
        speed: f32,
    },
    /// Walk a fixed route.
    Patrol {
        points: Vec<Vec3f>,
        speed: f32,
        looping: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Brain {
    pub entity: u64,
    pub kind: BrainKind,
    /// Countdown for the current phase (pause timer, retarget timer).
    pub timer: f32,
    /// Current destination (wander) or route index (patrol).
    pub goal: Vec3f,
    pub waypoint: usize,
    /// Set for one tick when a chase brain reaches its target — the script
    /// reads it through `game.caught(id)` and decides what "caught" means.
    pub caught: u64,
    /// Route direction for a non-looping patrol (+1 out, -1 back).
    forward: bool,
}

impl Brain {
    pub fn new(entity: u64, kind: BrainKind) -> Self {
        let goal = match &kind {
            BrainKind::Wander { home, .. } => *home,
            BrainKind::Patrol { points, .. } => points.first().copied().unwrap_or_default(),
            BrainKind::Chase { .. } => Vec3f::default(),
        };
        Self {
            entity,
            kind,
            timer: 0.0,
            goal,
            waypoint: 0,
            caught: 0,
            forward: true,
        }
    }

    pub fn tick(&mut self, world: &mut GameWorld) {
        self.caught = 0;
        let Some(me) = world.entity(self.entity) else {
            return;
        };
        if me.kind != BodyKind::Mover || me.attached_to != 0 {
            return;
        }
        let pos = me.pos;
        let speed_mult = me.speed_mult;

        let (goal, speed, stop_within) = match &self.kind {
            BrainKind::Wander {
                home,
                range,
                speed,
                pause,
            } => {
                let (home, range, speed, pause) = (*home, *range, *speed, *pause);
                self.timer -= TICK_DT;
                let arrived = planar_distance(pos, self.goal) < 1.0;
                if arrived && self.timer <= 0.0 {
                    // Pick a new spot; world.rand keeps it tape-repeatable.
                    let angle = world.rand() as f32 * std::f32::consts::TAU;
                    let radius = (world.rand() as f32).sqrt() * range;
                    let (s, c) = gm::sincos(angle);
                    self.goal = vec3f(home.x + c * radius, home.y, home.z + s * radius);
                    self.timer = pause;
                }
                if arrived {
                    (self.goal, 0.0, 1.0)
                } else {
                    (self.goal, speed, 1.0)
                }
            }
            BrainKind::Chase {
                tag,
                target,
                range,
                catch,
                speed,
            } => {
                let (target, range, catch, speed) = (*target, *range, *catch, *speed);
                let tag = tag.clone();
                // Explicit id wins; otherwise the nearest tagged entity in range.
                let prey = if target != 0 {
                    world.entity(target).map(|e| (e.id, e.pos))
                } else {
                    nearest_tagged(world, &tag, pos, range)
                };
                match prey {
                    Some((id, target_pos)) => {
                        let distance = planar_distance(pos, target_pos);
                        if distance <= catch {
                            self.caught = id;
                            (target_pos, 0.0, catch)
                        } else if distance <= range {
                            (target_pos, speed, catch)
                        } else {
                            (pos, 0.0, 1.0)
                        }
                    }
                    None => (pos, 0.0, 1.0),
                }
            }
            BrainKind::Patrol {
                points,
                speed,
                looping,
            } => {
                let (speed, looping) = (*speed, *looping);
                if points.is_empty() {
                    (pos, 0.0, 1.0)
                } else {
                    let goal = points[self.waypoint.min(points.len() - 1)];
                    if planar_distance(pos, goal) < 1.2 {
                        if looping {
                            self.waypoint = (self.waypoint + 1) % points.len();
                        } else if self.forward {
                            if self.waypoint + 1 >= points.len() {
                                self.forward = false;
                                self.waypoint = self.waypoint.saturating_sub(1);
                            } else {
                                self.waypoint += 1;
                            }
                        } else if self.waypoint == 0 {
                            self.forward = true;
                            self.waypoint = 1.min(points.len() - 1);
                        } else {
                            self.waypoint -= 1;
                        }
                    }
                    self.goal = points[self.waypoint.min(points.len() - 1)];
                    (self.goal, speed, 1.0)
                }
            }
        };

        let Some(me) = world.entity_mut(self.entity) else {
            return;
        };
        let to = vec3f(goal.x - pos.x, 0.0, goal.z - pos.z);
        let distance = to.length();
        if speed <= 0.0 || distance < stop_within * 0.5 {
            me.vel.x = 0.0;
            me.vel.z = 0.0;
            return;
        }
        let dir = to * (1.0 / distance);
        me.vel.x = dir.x * speed * speed_mult;
        me.vel.z = dir.z * speed * speed_mult;
    }
}

fn planar_distance(a: Vec3f, b: Vec3f) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

/// Nearest entity with this tag within `range` — the scan the fixture wrote by
/// hand for every actor, once per tick, in script.
fn nearest_tagged(world: &GameWorld, tag: &str, from: Vec3f, range: f32) -> Option<(u64, Vec3f)> {
    let mut best: Option<(u64, Vec3f, f32)> = None;
    for e in &world.entities {
        if e.tag != tag {
            continue;
        }
        let distance = planar_distance(from, e.pos);
        if distance > range {
            continue;
        }
        // Ties break on the lower id: iteration order is already deterministic
        // (entities are sorted), and this keeps it stable under equal distance.
        if best.map_or(true, |(_, _, d)| distance < d) {
            best = Some((e.id, e.pos, distance));
        }
    }
    best.map(|(id, pos, _)| (id, pos))
}
