//! Makepad Arcade simulation core (game.md §Engine design).
//!
//! Rules of this crate:
//! - No Cx, no draw types, no ScriptObjectRef — script callbacks are opaque
//!   u32 slots the host resolves ([`CallbackSlot`]).
//! - Bit-deterministic cross-arch: all transcendental math goes through
//!   makepad-game-math; no HashMap iteration in simulation paths; all
//!   randomness through [`GameRng`].
//! - Everything is Clone: worlds snapshot by value (rollback ring, replay,
//!   session save, join-state dump).
//!
//! M0 stage A: the complete gamemaker world (entities, terrain, step,
//! queries) moved here verbatim from game_view.rs — float expression order
//! preserved so input tapes replay bit-identically. The remaining lib.rs
//! scaffolding (SlotMap/GameRng/World) is the target shape for later stages;
//! GameWorld below is the live parity-oracle sim.

pub mod dynamics;
pub mod entity;
pub mod player;
pub mod queries;
pub mod step;
pub mod terrain;
pub mod world;

pub use dynamics::*;
pub use entity::*;
pub use player::*;
pub use queries::*;
pub use step::*;
pub use terrain::*;
pub use world::*;

pub use makepad_game_math as math;

pub const TICK_HZ: u32 = 60;
pub const TICK_DT: f32 = 1.0 / 60.0;

// ------------------------------------------------------------------ entities

/// Generational entity id. Packs to u64 for script (f64-safe: gen and index
/// both fit far below 2^26) and for the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EntityId {
    pub index: u32,
    pub gen: u32,
}

impl EntityId {
    pub fn to_bits(self) -> u64 {
        (self.gen as u64) << 32 | self.index as u64
    }
    pub fn from_bits(bits: u64) -> Self {
        Self {
            index: bits as u32,
            gen: (bits >> 32) as u32,
        }
    }
}

#[derive(Clone, Debug)]
struct Slot<T> {
    gen: u32,
    value: Option<T>,
}

/// Generational slot map: O(1) id lookup, ids never dangle silently, slot
/// reuse bumps the generation so stale ids miss. Iteration is index order —
/// deterministic, independent of alloc/free history interleaving.
#[derive(Clone, Debug)]
pub struct SlotMap<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: u32,
}

impl<T> Default for SlotMap<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }
}

impl<T> SlotMap<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn alloc(&mut self, value: T) -> EntityId {
        self.len += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(slot.value.is_none());
            slot.value = Some(value);
            EntityId {
                index,
                gen: slot.gen,
            }
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                gen: 0,
                value: Some(value),
            });
            EntityId { index, gen: 0 }
        }
    }

    pub fn free(&mut self, id: EntityId) -> Option<T> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.gen != id.gen || slot.value.is_none() {
            return None;
        }
        slot.gen = slot.gen.wrapping_add(1);
        self.len -= 1;
        let value = slot.value.take();
        self.free.push(id.index);
        value
    }

    pub fn get(&self, id: EntityId) -> Option<&T> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.gen != id.gen {
            return None;
        }
        slot.value.as_ref()
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut T> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.gen != id.gen {
            return None;
        }
        slot.value.as_mut()
    }

    pub fn contains(&self, id: EntityId) -> bool {
        self.get(id).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, slot)| {
            slot.value.as_ref().map(|v| {
                (
                    EntityId {
                        index: i as u32,
                        gen: slot.gen,
                    },
                    v,
                )
            })
        })
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EntityId, &mut T)> {
        self.slots.iter_mut().enumerate().filter_map(|(i, slot)| {
            let gen = slot.gen;
            slot.value.as_mut().map(move |v| {
                (
                    EntityId {
                        index: i as u32,
                        gen,
                    },
                    v,
                )
            })
        })
    }
}

// ----------------------------------------------------------------------- rng

/// xorshift64* — deterministic, seedable, Clone (snapshots carry the stream).
#[derive(Clone, Debug)]
pub struct GameRng {
    state: u64,
}

impl GameRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9E3779B97F4A7C15 } else { seed },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// Uniform in [0, 1) with 24 bits of precision — exact f32 arithmetic.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16777216.0)
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

// ---------------------------------------------------------- script callbacks

/// Opaque handle to a host-owned script closure (on_tick, on_touch, timers).
/// The sim never touches the script VM; the host maps slots to closures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CallbackSlot(pub u32);

// ------------------------------------------------------------------ tiering

/// Replication tier (game.md §Multiplayer model). Attached to subsystems and
/// verbs, enforced at the net layer: only Shared state ever hits the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Host-authoritative, replicated to clients.
    Shared,
    /// Recomputed client-side from Shared state + tick; never sent.
    Derived,
    /// Per-device (effects, camera, audio mix); never sent, may differ.
    Local,
}

// -------------------------------------------------------------------- world

/// The deterministic world. M0: skeleton — entity/part stores and per-tick
/// bookkeeping arrive with the extraction.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub tick: u64,
    pub rng_seed: u64,
}

impl World {
    /// Seconds since world start, derived from tick — never wall clock.
    pub fn time(&self) -> f64 {
        self.tick as f64 * (1.0 / TICK_HZ as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slotmap_alloc_get_free() {
        let mut map = SlotMap::new();
        let a = map.alloc("a");
        let b = map.alloc("b");
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(a), Some(&"a"));
        assert_eq!(map.get(b), Some(&"b"));
        assert_eq!(map.free(a), Some("a"));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(a), None);
    }

    #[test]
    fn stale_id_misses_after_reuse() {
        let mut map = SlotMap::new();
        let a = map.alloc(1);
        map.free(a);
        let b = map.alloc(2);
        // slot reused, generation bumped
        assert_eq!(a.index, b.index);
        assert_ne!(a.gen, b.gen);
        assert_eq!(map.get(a), None);
        assert_eq!(map.get(b), Some(&2));
        // double free is a no-op
        assert_eq!(map.free(a), None);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn iteration_is_index_order_regardless_of_history() {
        let mut map = SlotMap::new();
        let ids: Vec<_> = (0..10).map(|i| map.alloc(i)).collect();
        map.free(ids[3]);
        map.free(ids[7]);
        map.alloc(100); // reuses slot 7 (LIFO free list)
        let order: Vec<_> = map.iter().map(|(_, v)| *v).collect();
        assert_eq!(order, vec![0, 1, 2, 4, 5, 6, 100, 8, 9]);
    }

    #[test]
    fn entity_id_bits_roundtrip() {
        let id = EntityId {
            index: 12345,
            gen: 678,
        };
        assert_eq!(EntityId::from_bits(id.to_bits()), id);
    }

    #[test]
    fn rng_is_deterministic_and_seeded() {
        let mut a = GameRng::new(42);
        let mut b = GameRng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut c = GameRng::new(43);
        assert_ne!(a.next_u64(), c.next_u64());
        // zero seed must not lock the stream at zero
        let mut z = GameRng::new(0);
        assert_ne!(z.next_u64(), 0);
        for _ in 0..1000 {
            let v = z.next_f32();
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn world_time_derives_from_tick() {
        let mut w = World::default();
        assert_eq!(w.time(), 0.0);
        w.tick = 60;
        assert_eq!(w.time(), 1.0);
        // Clone = snapshot
        let snap = w.clone();
        w.tick = 120;
        assert_eq!(snap.tick, 60);
    }
}
