//! DECKS — the continuous walk surface of a generated corridor.
//!
//! A road's asphalt, a railway's ballast and a bridge deck are drawn as a
//! smooth graded ribbon, but their colliders are flat box slabs a few
//! metres long: on a 6 % grade every slab top sits centimetres above the
//! deck at its downhill end, and a walker climbed the deck slab by slab —
//! "the bridge snaps the character like a staircase" (user, Highway,
//! 2026-09-02). A [`DeckStrip`] is the deck itself: the corridor's
//! centreline at the height a foot stands on, with the ribbon's half width.
//! Movers stand on it EXACTLY, at every point along it; the slabs remain
//! for rigid bodies and vehicles, which the box3d mirror carries.
//!
//! The strip is also the seam through which a corridor that the press
//! could not follow (a heightfield cell wider than the ribbon, a shielded
//! bank) still carries feet: the deck is the floor wherever the ground is
//! within a step of it.

use makepad_math::*;

/// One corridor's walk surface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckStrip {
    /// The corridor feature this strip belongs to — what a retraction
    /// removes it by.
    pub feature: String,
    /// The centreline, every few metres, at the WALK height (the drawn top
    /// a foot stands on, not the profile line under the ballast).
    pub pts: Vec<Vec3f>,
    /// Half the ribbon's width: how far off the centreline the deck reaches.
    pub half_width: f32,
}

impl DeckStrip {
    /// The walk height under (x, z), when the point lies on the strip:
    /// the height interpolated along the nearest centreline segment.
    /// Ends overshoot by a half disc, like the slabs they replace.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let hw2 = self.half_width * self.half_width;
        let mut best: Option<(f32, f32)> = None;
        for w in self.pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (ex, ez) = (b.x - a.x, b.z - a.z);
            let len2 = ex * ex + ez * ez;
            if len2 < 1.0e-6 {
                continue;
            }
            let t = (((x - a.x) * ex + (z - a.z) * ez) / len2).clamp(0.0, 1.0);
            let (px, pz) = (a.x + ex * t, a.z + ez * t);
            let d2 = (x - px) * (x - px) + (z - pz) * (z - pz);
            if d2 <= hw2 && best.map_or(true, |(bd2, _)| d2 < bd2) {
                best = Some((d2, a.y + (b.y - a.y) * t));
            }
        }
        best.map(|(_, h)| h)
    }
}

/// Feet this far ABOVE the deck are still on the deck: the excess a flat
/// collider slab top or an unpressed heightfield vertex can stand over
/// the drawn surface. They are set DOWN onto it. Anything higher is
/// something standing on the deck — a crate, a kerb — and keeps its top.
pub const DECK_STEP: f32 = 0.16;

/// The highest deck under a mover's footprint (centre + corners) that is
/// THIS floor: within [`DECK_STEP`] above the feet's height, or within
/// `climb` below them (a raised ballast walked into from the verge, a dip
/// the press could not fill, the seam of a bridge deck). A deck a storey
/// away — the overpass above a walker, the road under a bridge — is not.
pub fn deck_floor_under(decks: &[DeckStrip], pos: Vec3f, half: Vec3f, feet: f32, climb: f32) -> Option<f32> {
    if decks.is_empty() {
        return None;
    }
    let probes = [
        (pos.x, pos.z),
        (pos.x - half.x, pos.z - half.z),
        (pos.x + half.x, pos.z - half.z),
        (pos.x - half.x, pos.z + half.z),
        (pos.x + half.x, pos.z + half.z),
    ];
    let mut best: Option<f32> = None;
    for strip in decks {
        for (x, z) in probes {
            if let Some(h) = strip.height_at(x, z) {
                // Only a deck the feet are NEAR is a candidate: the
                // overpass above a walker, or the road under a bridge
                // deck, is another storey, not this floor.
                if h < feet - DECK_STEP || h > feet + climb {
                    continue;
                }
                best = Some(best.map_or(h, |b: f32| b.max(h)));
            }
        }
    }
    best
}

/// The top of a static or kinematic BOX the mover's footprint stands in,
/// when that top is within step reach above the feet: a stage, a crate
/// lid, a doorstep a mover was spawned inside of or pushed into. The
/// sweeps land a mover on a box it FALLS onto and step it up onto one it
/// WALKS into; a mover that starts a tick with its feet inside a box met
/// neither, and stood waist-deep in the stage it was placed on (user,
/// Mocap Stage, 2026-09-02). Returns the top and the box's id.
pub fn box_floor_under(
    walls: &[crate::queries::Solid],
    pos: Vec3f,
    half: Vec3f,
    feet: f32,
    climb: f32,
) -> Option<(f32, u64)> {
    // A hair inside the footprint, so a kerb the mover merely abuts (the
    // sweeps leave a contact skin) never counts as ground under it.
    let inset = 0.05f32;
    let (hx, hz) = ((half.x - inset).max(0.01), (half.z - inset).max(0.01));
    let mut best: Option<(f32, u64)> = None;
    for w in walls {
        if w.shape != crate::entity::Shape::Box {
            continue;
        }
        if !matches!(w.kind, crate::entity::BodyKind::Static | crate::entity::BodyKind::Kinematic) {
            continue;
        }
        let top = w.pos.y + w.half.y;
        if top <= feet + 1.0e-3 || top - feet > climb {
            continue;
        }
        // Footprints overlap in the ground plane, and the box reaches down
        // into the step zone (a mover that has sunk a hair below a thin
        // stage's underside is still standing in it; a slab hung a full
        // step above the feet is a ledge overhead, not a floor).
        let over_x = (pos.x - w.pos.x).abs() < hx + w.half.x;
        let over_z = (pos.z - w.pos.z).abs() < hz + w.half.z;
        let bottom = w.pos.y - w.half.y;
        if over_x && over_z && bottom <= feet + climb && best.map_or(true, |(b, _)| top > b) {
            best = Some((top, w.id));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graded_strip() -> DeckStrip {
        // 60 m at 6 %: y climbs 3.6 m, every 3 m a point.
        let pts = (0..=20).map(|i| vec3f(i as f32 * 3.0, i as f32 * 3.0 * 0.06, 0.0)).collect();
        DeckStrip { feature: "road:0".into(), pts, half_width: 4.0 }
    }

    #[test]
    fn the_strip_interpolates_the_grade_between_its_points() {
        let s = graded_strip();
        for k in 0..120 {
            let x = k as f32 * 0.5;
            let h = s.height_at(x, 1.5).expect("on the deck");
            assert!((h - x * 0.06).abs() < 1.0e-4, "at {x}: {h} vs {}", x * 0.06);
        }
        assert!(s.height_at(30.0, 4.5).is_none(), "beyond the ribbon edge");
        assert!(s.height_at(30.0, 3.9).is_some(), "inside the ribbon edge");
    }

    #[test]
    fn a_deck_two_storeys_away_is_not_this_floor() {
        let s = graded_strip();
        // Feet 5 m under the deck (a road under an overpass): no floor.
        assert!(deck_floor_under(&[s.clone()], vec3f(30.0, 1.0, 0.0), vec3f(0.35, 0.9, 0.35), 1.8 - 5.0, 0.55).is_none());
        // Standing on a crate on the deck: the crate keeps the feet.
        let deck = 30.0 * 0.06;
        assert!(deck_floor_under(&[s.clone()], vec3f(30.0, deck + 0.4 + 0.9, 0.0), vec3f(0.35, 0.9, 0.35), deck + 0.4, 0.55).is_none());
        // A raised ballast walked into from the verge: a step up.
        assert!(deck_floor_under(&[s.clone()], vec3f(30.0, deck - 0.3 + 0.9, 0.0), vec3f(0.35, 0.9, 0.35), deck - 0.3, 0.55).is_some());
        // Feet a slab-step above it: the deck is the floor.
        let feet = deck + 0.1;
        let floor = deck_floor_under(&[s], vec3f(30.0, feet + 0.9, 0.0), vec3f(0.35, 0.9, 0.35), feet, 0.55).expect("the deck is the floor");
        // The highest probe wins (the uphill corner, 0.35 m along): the
        // footprint stands on the deck, never in it.
        assert!((floor - deck).abs() < 0.03, "floor {floor} vs deck {deck}");
    }

    fn mover(id: u64, pos: Vec3f) -> crate::entity::Entity {
        use crate::entity::{BodyKind, Entity, Shape};
        Entity {
            id,
            kind: BodyKind::Mover,
            shape: Shape::Box,
            pos,
            half: vec3f(0.35, 0.9, 0.35),
            collide: true,
            gravity_scale: 1.0,
            push_mass: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.6,
            ..Default::default()
        }
    }

    /// The user's Highway bridge: a walker crossing a 6 % graded deck at
    /// 3 m/s rises smoothly — under 3 cm a tick, never a step, always on
    /// the floor. The deck is the strip; there is no terrain under it.
    #[test]
    fn a_walker_crosses_a_graded_deck_without_a_step() {
        let mut w = crate::world::GameWorld::new();
        w.gravity = 30.0;
        w.decks.push(graded_strip());
        w.next_id = 10;
        w.push_entity(mover(10, vec3f(1.0, 0.9 + 0.06, 0.0)));
        let mut last_y = None::<f32>;
        let mut max_dy = 0.0f32;
        for _ in 0..(60 * 18) {
            if let Some(e) = w.entities.iter_mut().find(|e| e.id == 10) {
                e.vel.x = 3.0;
                e.vel.z = 0.0;
            }
            crate::step::step_world(&mut w);
            let e = w.entities.iter().find(|e| e.id == 10).unwrap();
            if e.pos.x > 3.0 && e.pos.x < 57.0 {
                assert!(e.on_floor, "off the floor at x {:.1}", e.pos.x);
                let feet = e.pos.y - e.half.y;
                let deck = e.pos.x * 0.06;
                assert!((feet - deck).abs() < 0.03, "feet {:.3} vs deck {:.3} at x {:.1}", feet, deck, e.pos.x);
                if let Some(ly) = last_y {
                    max_dy = max_dy.max((e.pos.y - ly).abs());
                }
                last_y = Some(e.pos.y);
            }
        }
        assert!(max_dy < 0.03, "a step of {max_dy:.3} m in one tick");
        assert!(max_dy > 0.001, "the walker never climbed");
    }

    /// The user's Mocap Stage: a character spawned with its feet inside a
    /// 0.3 m stage box stands ON the stage, not waist-deep in it.
    #[test]
    fn a_character_spawned_inside_a_stage_stands_on_it() {
        use crate::entity::{BodyKind, Entity, Shape};
        let mut w = crate::world::GameWorld::new();
        w.gravity = 30.0;
        w.next_id = 2;
        w.push_entity(Entity {
            id: 1,
            kind: BodyKind::Static,
            shape: Shape::Box,
            pos: vec3f(0.0, 0.15, 0.0),
            half: vec3f(7.0, 0.15, 4.5),
            collide: true,
            push_mass: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.6,
            ..Default::default()
        });
        // Feet at the ground plane, 0.3 m inside the stage (a spawn that
        // measured its height from the ground, not the stage).
        w.push_entity(mover(2, vec3f(-1.6, 0.9, 0.0)));
        for _ in 0..30 {
            crate::step::step_world(&mut w);
        }
        let e = w.entities.iter().find(|e| e.id == 2).unwrap();
        let feet = e.pos.y - e.half.y;
        assert!((feet - 0.3).abs() < 0.01, "feet at {feet:.3}, the stage top is 0.3");
        assert!(e.on_floor && e.floor_id == 1, "standing on the stage: on_floor {} floor {}", e.on_floor, e.floor_id);
    }

    #[test]
    fn a_box_the_feet_stand_inside_is_a_floor_and_a_ledge_overhead_is_not() {
        use crate::entity::{BodyKind, Shape};
        use crate::queries::Solid;
        let stage = Solid {
            id: 7,
            kind: BodyKind::Static,
            pos: vec3f(0.0, 0.15, 0.0),
            half: vec3f(7.0, 0.15, 4.5),
            vel: vec3f(0.0, 0.0, 0.0),
            shape: Shape::Box,
        };
        // Feet at 0, inside the 0.3 m stage: its top is the floor.
        assert_eq!(box_floor_under(&[stage], vec3f(-1.6, 0.9, 0.0), vec3f(0.35, 0.9, 0.35), 0.0, 0.55), Some((0.3, 7)));
        // Standing on it already: nothing to lift.
        assert_eq!(box_floor_under(&[stage], vec3f(-1.6, 1.2, 0.0), vec3f(0.35, 0.9, 0.35), 0.3, 0.55), None);
        // Beside it, abutting its edge: not under the feet.
        assert_eq!(box_floor_under(&[stage], vec3f(7.35, 0.9, 0.0), vec3f(0.35, 0.9, 0.35), 0.0, 0.55), None);
        // A shelf hung above step reach is overhead, not a floor.
        let shelf = Solid { pos: vec3f(0.0, 0.7, 0.0), half: vec3f(7.0, 0.1, 4.5), ..stage };
        assert_eq!(box_floor_under(&[shelf], vec3f(0.0, 0.9, 0.0), vec3f(0.35, 0.9, 0.35), 0.0, 0.55), None);
    }
}
