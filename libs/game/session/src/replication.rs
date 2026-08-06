//! Replication tiers (game.md §Multiplayer model).
//!
//! Three tiers, enforced here rather than documented and hoped for:
//!
//! | tier | examples | on the wire |
//! |---|---|---|
//! | **Shared** | pos, vel, body kind, size, tag, life, score, race progress | host → clients, every tick |
//! | **Derived** | facing yaw, walk-cycle blend, part animation, scale/glow easing, blob shadows | never — recomputed from Shared + tick |
//! | **Local** | camera rig, audio mix, particles, MR stage transform | never — this device's business |
//!
//! Facing yaw is the tier system paying for itself. It is visual-only by the
//! engine's oldest contract (the collision AABB never rotates), so a client can
//! reconstruct it from velocity and be right, and a game with 200 moving props
//! spends zero bytes on rotation.

use makepad_game_net::protocol::{EntityDesc, EntityState};
use makepad_game_sim::{BodyKind, Entity, GameWorld, Shape};
use makepad_math::*;

/// Bits in [`EntityState::flags`] — volatile, per tick.
pub mod state_flags {
    pub const ON_FLOOR: u16 = 1 << 0;
    pub const ATTACHED: u16 = 1 << 1;
}

/// Bits in [`EntityDesc::flags`] — construction-time, rarely changes.
pub mod desc_flags {
    pub const SENSOR: u16 = 1 << 0;
    pub const COLLIDE: u16 = 1 << 1;
    pub const HITS: u16 = 1 << 2;
    pub const AUTO_FACE: u16 = 1 << 3;
}

fn kind_to_u8(kind: BodyKind) -> u8 {
    match kind {
        BodyKind::Static => 0,
        BodyKind::Kinematic => 1,
        BodyKind::Mover => 2,
        BodyKind::Rigid => 3,
    }
}

fn kind_from_u8(v: u8) -> BodyKind {
    match v {
        1 => BodyKind::Kinematic,
        2 => BodyKind::Mover,
        3 => BodyKind::Rigid,
        _ => BodyKind::Static,
    }
}

fn shape_from_u8(v: u8) -> Shape {
    match v {
        1 => Shape::Sphere,
        2 => Shape::Cylinder,
        3 => Shape::Cone,
        4 => Shape::Wedge,
        _ => Shape::Box,
    }
}

/// Does this entity's state need to travel at all? Statics never move, so
/// after their descriptor lands they cost nothing per tick — which is what
/// makes a 200-prop track affordable.
pub fn is_replicated(entity: &Entity) -> bool {
    !matches!(entity.kind, BodyKind::Static)
}

/// Pack the Shared tier for every moving entity.
pub fn collect_states(world: &GameWorld) -> Vec<EntityState> {
    world
        .entities
        .iter()
        .filter(|e| is_replicated(e))
        .map(|e| {
            let mut flags = 0u16;
            if e.on_floor {
                flags |= state_flags::ON_FLOOR;
            }
            if e.attached_to != 0 {
                flags |= state_flags::ATTACHED;
            }
            EntityState {
                id: e.id,
                seq: 0, // stamped by the host as it sends
                pos: [e.pos.x, e.pos.y, e.pos.z],
                vel: [e.vel.x, e.vel.y, e.vel.z],
                // Yaw is Derived and reconstructed client-side; sending zero
                // keeps the field honest rather than quietly authoritative.
                yaw: 0.0,
                flags,
            }
        })
        .collect()
}

/// Pack construction data for every entity, moving or not.
pub fn collect_descs(world: &GameWorld) -> Vec<EntityDesc> {
    world.entities.iter().map(desc_of).collect()
}

/// Does an already-sent descriptor still describe this entity?
///
/// Compared field-by-field rather than by rebuilding and comparing, because
/// building a descriptor clones the tag string — doing that for every entity
/// every tick would allocate its way through a 200-prop world at 60Hz.
pub fn desc_matches(desc: &EntityDesc, e: &Entity) -> bool {
    desc.id == e.id
        // Only meaningful for statics; movers carry position in their state,
        // and comparing it here would resend a descriptor every tick.
        && (is_replicated(e) || desc.pos == [e.pos.x, e.pos.y, e.pos.z])
        && desc.kind == kind_to_u8(e.kind)
        && desc.shape == e.shape as u8
        && desc.glow == e.glow
        && desc.half == [e.half.x, e.half.y, e.half.z]
        && desc.color == [e.color.x, e.color.y, e.color.z, e.color.w]
        && desc.scale == [e.scale.x, e.scale.y, e.scale.z]
        && desc.flags == desc_flag_bits(e)
        && desc.tag == e.tag
}

fn desc_flag_bits(e: &Entity) -> u16 {
    let mut flags = 0u16;
    if e.sensor {
        flags |= desc_flags::SENSOR;
    }
    if e.collide {
        flags |= desc_flags::COLLIDE;
    }
    if e.hits {
        flags |= desc_flags::HITS;
    }
    if e.auto_face {
        flags |= desc_flags::AUTO_FACE;
    }
    flags
}

pub fn desc_of(e: &Entity) -> EntityDesc {
    EntityDesc {
        id: e.id,
        pos: [e.pos.x, e.pos.y, e.pos.z],
        half: [e.half.x, e.half.y, e.half.z],
        color: [e.color.x, e.color.y, e.color.z, e.color.w],
        scale: [e.scale.x, e.scale.y, e.scale.z],
        kind: kind_to_u8(e.kind),
        shape: e.shape as u8,
        glow: e.glow,
        flags: desc_flag_bits(e),
        tag: e.tag.clone(),
    }
}

/// Build a client-side entity from its descriptor plus latest state.
pub fn entity_from_wire(desc: &EntityDesc, state: Option<&EntityState>) -> Entity {
    let mut e = Entity {
        id: desc.id,
        pos: vec3f(desc.pos[0], desc.pos[1], desc.pos[2]),
        kind: kind_from_u8(desc.kind),
        half: vec3f(desc.half[0], desc.half[1], desc.half[2]),
        color: vec4f(desc.color[0], desc.color[1], desc.color[2], desc.color[3]),
        scale: vec3f(desc.scale[0], desc.scale[1], desc.scale[2]),
        scale_target: vec3f(desc.scale[0], desc.scale[1], desc.scale[2]),
        shape: shape_from_u8(desc.shape),
        glow: desc.glow,
        tag: desc.tag.clone(),
        sensor: desc.flags & desc_flags::SENSOR != 0,
        collide: desc.flags & desc_flags::COLLIDE != 0,
        hits: desc.flags & desc_flags::HITS != 0,
        auto_face: desc.flags & desc_flags::AUTO_FACE != 0,
        ..Default::default()
    };
    if let Some(s) = state {
        e.pos = vec3f(s.pos[0], s.pos[1], s.pos[2]);
        e.vel = vec3f(s.vel[0], s.vel[1], s.vel[2]);
        e.on_floor = s.flags & state_flags::ON_FLOOR != 0;
    }
    e
}

/// Rebuild a client world from replicated descriptors and states.
///
/// Entities are rebuilt sorted by id, which satisfies the sim's
/// sorted-by-id invariant by construction — a client never pushes out of
/// order because it never pushes incrementally.
pub fn apply_world(
    world: &mut GameWorld,
    descs: impl Iterator<Item = EntityDesc>,
    states: &std::collections::HashMap<u64, EntityState>,
) {
    let mut descs: Vec<EntityDesc> = descs.collect();
    descs.sort_by_key(|d| d.id);

    // Preserve Derived state (facing, anim easing) across the rebuild: it is
    // the client's own continuous quantity, not something the wire replaces.
    let mut previous: Vec<(u64, f32, Vec3f)> = world
        .entities
        .iter()
        .map(|e| (e.id, e.yaw, e.scale))
        .collect();
    previous.sort_by_key(|(id, _, _)| *id);

    let mut rebuilt = Vec::with_capacity(descs.len());
    for desc in &descs {
        let mut entity = entity_from_wire(desc, states.get(&desc.id));
        if let Ok(i) = previous.binary_search_by_key(&desc.id, |(id, _, _)| *id) {
            entity.yaw = previous[i].1;
            entity.scale = previous[i].2;
        }
        rebuilt.push(entity);
    }
    world.entities = rebuilt;
    world.next_id = descs.last().map_or(0, |d| d.id);
    world.mark_render_dirty();
}

/// Recompute the Derived tier locally: facing follows movement, exactly as the
/// host does it, so a remote car points where it drives without a byte spent.
pub fn derive_local(world: &mut GameWorld, dt: f32) {
    for e in world.entities.iter_mut() {
        if !e.auto_face {
            continue;
        }
        let speed_sq = e.vel.x * e.vel.x + e.vel.z * e.vel.z;
        if speed_sq > 0.0001 {
            let target = makepad_game_math::atan2(-e.vel.x, -e.vel.z);
            let mut delta = target - e.yaw;
            let tau = std::f32::consts::TAU;
            while delta > std::f32::consts::PI {
                delta -= tau;
            }
            while delta < -std::f32::consts::PI {
                delta += tau;
            }
            let step = e.turn_rate.max(1.0) * dt;
            e.yaw += delta.clamp(-step, step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn world_with_entities() -> GameWorld {
        let mut w = GameWorld::new();
        for (i, kind) in [BodyKind::Static, BodyKind::Mover, BodyKind::Rigid]
            .into_iter()
            .enumerate()
        {
            w.next_id += 1;
            let id = w.next_id;
            w.push_entity(Entity {
                id,
                kind,
                pos: vec3f(i as f32, 2.0, 0.0),
                half: vec3f(0.5, 0.5, 0.5),
                color: vec4f(1.0, 0.0, 0.0, 1.0),
                scale: vec3f(1.0, 1.0, 1.0),
                tag: format!("thing{i}"),
                collide: true,
                ..Default::default()
            });
        }
        w
    }

    #[test]
    fn statics_cost_nothing_per_tick() {
        let w = world_with_entities();
        let states = collect_states(&w);
        assert_eq!(states.len(), 2, "only the mover and the rigid replicate");
        assert_eq!(collect_descs(&w).len(), 3, "but all three are describable");
    }

    #[test]
    fn client_rebuild_reproduces_shared_fields() {
        let host = world_with_entities();
        let descs = collect_descs(&host);
        let states: HashMap<u64, EntityState> =
            collect_states(&host).into_iter().map(|s| (s.id, s)).collect();

        let mut client = GameWorld::new();
        apply_world(&mut client, descs.into_iter(), &states);

        assert_eq!(client.entities.len(), host.entities.len());
        assert!(client.entities_sorted_by_id());
        for (a, b) in client.entities.iter().zip(host.entities.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.tag, b.tag);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.half, b.half);
            if is_replicated(b) {
                assert_eq!(a.pos, b.pos);
            }
        }
    }

    #[test]
    fn derived_yaw_is_reconstructed_not_transmitted() {
        let mut w = GameWorld::new();
        w.next_id += 1;
        w.push_entity(Entity {
            id: 1,
            kind: BodyKind::Mover,
            vel: vec3f(4.0, 0.0, 0.0),
            auto_face: true,
            turn_rate: 100.0,
            ..Default::default()
        });
        // Nothing about yaw crosses the wire...
        assert!(collect_states(&w).iter().all(|s| s.yaw == 0.0));
        // ...yet the client turns the model to face travel.
        let before = w.entities[0].yaw;
        for _ in 0..30 {
            derive_local(&mut w, 1.0 / 60.0);
        }
        assert_ne!(w.entities[0].yaw, before);
    }
}
