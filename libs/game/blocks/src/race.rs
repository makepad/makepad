//! Race kit — `game.spawnpoint`, `game.checkpoint`, `game.race`, `game.score`.
//!
//! Everything here is **Shared tier** (game.md §Multiplayer model): laps,
//! checkpoint progress, standings and scores are gameplay state the host owns
//! and replicates. Nothing in this module reads device input or wall clock, so
//! a client can render standings from the replicated struct alone.
//!
//! Checkpoint ordering is enforced rather than advisory: a racer only banks
//! checkpoint N+1, so cutting the course backwards or skipping a gate scores
//! nothing. That single rule is what makes a track out of a pile of triggers.

use makepad_game_sim::GameWorld;
use makepad_math::*;

/// A start slot: where a racer is placed, and which way it faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnPoint {
    pub pos: Vec3f,
    pub yaw: f32,
    /// Entity currently assigned to this slot (0 = free). M2 assigns per player.
    pub owner: u64,
}

/// A gate on the course. `index` is the order the racer must cross them in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Checkpoint {
    pub pos: Vec3f,
    pub half: Vec3f,
    pub index: u32,
    /// Sensor entity drawn for it (0 = invisible trigger).
    pub entity: u64,
}

/// One racer's progress. Sorted by (laps, checkpoints, -distance to next gate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Standing {
    pub entity: u64,
    pub lap: u32,
    /// Checkpoints banked this lap.
    pub checkpoint: u32,
    /// Total gates crossed since the start — the primary sort key.
    pub progress: u32,
    pub finished: bool,
    /// Tick the racer finished on (0 while racing) — the finishing order.
    pub finish_tick: u64,
    pub score: i32,
}

#[derive(Clone, Debug, Default)]
pub struct RaceKit {
    pub spawns: Vec<SpawnPoint>,
    pub checkpoints: Vec<Checkpoint>,
    pub standings: Vec<Standing>,
    /// Laps needed to finish; 0 = no race running (checkpoints still track).
    pub laps: u32,
    pub running: bool,
    /// Set for one tick when the first racer finishes.
    pub winner: u64,
}

impl RaceKit {
    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty() && self.checkpoints.is_empty() && self.standings.is_empty()
    }

    pub fn add_spawn(&mut self, pos: Vec3f, yaw: f32) -> usize {
        self.spawns.push(SpawnPoint {
            pos,
            yaw,
            owner: 0,
        });
        self.spawns.len() - 1
    }

    pub fn add_checkpoint(&mut self, pos: Vec3f, half: Vec3f, entity: u64) -> u32 {
        // Gates are numbered in declaration order unless the script overrides;
        // that keeps a 100-line fixture free of bookkeeping.
        let index = self.checkpoints.len() as u32;
        self.checkpoints.push(Checkpoint {
            pos,
            half,
            index,
            entity,
        });
        index
    }

    /// Place an entity on a start slot and register it as a racer.
    pub fn place(&mut self, world: &mut GameWorld, slot: usize, entity: u64) -> bool {
        let Some(spawn) = self.spawns.get_mut(slot) else {
            return false;
        };
        spawn.owner = entity;
        let (pos, yaw) = (spawn.pos, spawn.yaw);
        let Some(e) = world.entity_mut(entity) else {
            return false;
        };
        e.pos = pos;
        e.yaw = yaw;
        e.vel = vec3f(0.0, 0.0, 0.0);
        self.enter(entity);
        true
    }

    /// Register a racer (idempotent).
    pub fn enter(&mut self, entity: u64) {
        if self.standings.iter().any(|s| s.entity == entity) {
            return;
        }
        self.standings.push(Standing {
            entity,
            lap: 0,
            checkpoint: 0,
            progress: 0,
            finished: false,
            finish_tick: 0,
            score: 0,
        });
    }

    pub fn start(&mut self, laps: u32) {
        self.laps = laps.max(1);
        self.running = true;
        self.winner = 0;
        for standing in self.standings.iter_mut() {
            standing.lap = 0;
            standing.checkpoint = 0;
            standing.progress = 0;
            standing.finished = false;
            standing.finish_tick = 0;
        }
    }

    pub fn standing_of(&self, entity: u64) -> Option<&Standing> {
        self.standings.iter().find(|s| s.entity == entity)
    }

    /// 1-based finishing position, or 0 if unknown.
    pub fn rank_of(&self, entity: u64) -> u32 {
        self.order()
            .iter()
            .position(|s| s.entity == entity)
            .map(|i| i as u32 + 1)
            .unwrap_or(0)
    }

    pub fn add_score(&mut self, entity: u64, points: i32) {
        if let Some(standing) = self.standings.iter_mut().find(|s| s.entity == entity) {
            standing.score += points;
        } else {
            self.enter(entity);
            if let Some(standing) = self.standings.last_mut() {
                standing.score += points;
            }
        }
    }

    /// Standings, leader first. Finishers rank by finish tick; everyone else by
    /// gates banked, then by proximity to the next gate.
    pub fn order(&self) -> Vec<Standing> {
        let mut order = self.standings.clone();
        order.sort_by(|a, b| {
            match (a.finished, b.finished) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                (true, true) => return a.finish_tick.cmp(&b.finish_tick),
                (false, false) => {}
            }
            b.progress
                .cmp(&a.progress)
                // Stable tiebreak: entity id. Never distance — that would make
                // the order depend on float compares across machines.
                .then(a.entity.cmp(&b.entity))
        });
        order
    }

    pub(crate) fn reconcile(&mut self, world: &GameWorld) {
        self.standings.retain(|s| world.entity(s.entity).is_some());
        for spawn in self.spawns.iter_mut() {
            if spawn.owner != 0 && world.entity(spawn.owner).is_none() {
                spawn.owner = 0;
            }
        }
    }

    /// Bank gates for every racer that is inside the one it owes.
    pub(crate) fn post_tick(&mut self, world: &mut GameWorld) {
        if self.checkpoints.is_empty() || self.standings.is_empty() {
            return;
        }
        self.winner = 0;
        let count = self.checkpoints.len() as u32;
        let tick = world.tick;
        for standing in self.standings.iter_mut() {
            if standing.finished {
                continue;
            }
            let Some(entity) = world.entity(standing.entity) else {
                continue;
            };
            // Only the NEXT gate counts — that's the anti-shortcut rule.
            let wanted = standing.checkpoint % count;
            let Some(gate) = self.checkpoints.iter().find(|c| c.index == wanted) else {
                continue;
            };
            let inside = (entity.pos.x - gate.pos.x).abs() < gate.half.x + entity.half.x
                && (entity.pos.y - gate.pos.y).abs() < gate.half.y + entity.half.y
                && (entity.pos.z - gate.pos.z).abs() < gate.half.z + entity.half.z;
            if !inside {
                continue;
            }
            standing.checkpoint += 1;
            standing.progress += 1;
            if standing.checkpoint >= count {
                standing.checkpoint = 0;
                standing.lap += 1;
                if self.running && standing.lap >= self.laps {
                    standing.finished = true;
                    standing.finish_tick = tick;
                    if self.winner == 0 {
                        self.winner = standing.entity;
                    }
                }
            }
        }
    }

    pub(crate) fn hash(&self) -> u64 {
        let mut h: u64 = 0x517c_c1b7_2722_0a95;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x100_0000_01b3);
        };
        for standing in &self.standings {
            mix(standing.entity);
            mix(standing.lap as u64);
            mix(standing.checkpoint as u64);
            mix(standing.progress as u64);
            mix(standing.finish_tick);
            mix(standing.score as i64 as u64);
        }
        h
    }
}
