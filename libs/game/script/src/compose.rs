//! The bridge between the asset library and the layout generators.
//!
//! `libs/game/gen` composes levels out of an abstract [`Kit`] — tiles with
//! roles and connection masks, knowing no filenames. `libs/game/assets` holds
//! 23 real kits whose roles were parsed off Kenney's systematic filenames.
//! This module joins them, so `game.town({roads_kit: "kenney/city-kit-roads"})`
//! lays a street grid out of real artwork.
//!
//! Nothing here touches a GPU: a generated level is a list of (asset id,
//! position, yaw), which the host loads and draws through the same model queue
//! a hand-placed `game.model` uses.

use makepad_game_assets::{AssetEntry, AssetIndex};
use makepad_game_gen::{Kit, Level, TileDef, TileRole};
use makepad_widgets::makepad_math::*;

/// Height used when a model's GLB never exposed bounds. Tiles are placed on a
/// grid whose pitch we do know, so only the collider box suffers, and a
/// door-height guess beats a zero-height box that nothing can stand on.
const FALLBACK_TILE_HEIGHT: f32 = 2.0;

/// Map the index's filename-derived role vocabulary onto the closed set the
/// generators switch on.
///
/// The two vocabularies do not line up one-to-one, and one case genuinely
/// cannot be settled by role alone: the index folds crossroads and T-junctions
/// into a single `junction`, because both are "more than two ways meet". The
/// generators must tell them apart — a 4-degree cell needs four open edges —
/// so the id is consulted for the words Kenney actually uses. Getting this
/// wrong is not subtle: a T where a crossroad belongs leaves a road stub
/// pointing at nothing.
/// What a kit is being fetched FOR.
///
/// Kenney's packs disagree about what a "tile" is. `city-kit-roads` is
/// structural pieces with parsed roles; `city-kit-suburban` is 40 whole
/// buildings named `building-a`..`building-z` with no role vocabulary at all.
/// A role-less model is therefore ambiguous — a whole building on a lot, or a
/// cone at a kerb — and only the caller knows which it wants. Getting this
/// wrong is silent: `town()` looks for `Building` tiles, finds none, and lays
/// streets with nothing along them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KitUse {
    /// Roads, corridors, walls — pieces that connect.
    Structure,
    /// Whole buildings that occupy a lot.
    Buildings,
    /// Scenery: lamps, cones, benches.
    Props,
}

fn role_of(entry: &AssetEntry, usage: KitUse) -> TileRole {
    let id = entry.id.to_ascii_lowercase();
    match entry.role {
        Some("junction") => {
            if id.contains("cross") || id.contains("intersection") || id.contains("-4") {
                TileRole::Cross
            } else {
                TileRole::TJunction
            }
        }
        Some("straight") | Some("bridge") => TileRole::Straight,
        Some("corner") => TileRole::Corner,
        Some("corner-inner") | Some("corner-outer") => TileRole::WallCorner,
        Some("end") => TileRole::End,
        Some("floor") => TileRole::Floor,
        Some("wall") => TileRole::Wall,
        Some("door") => TileRole::Door,
        Some("stairs") | Some("ramp") => TileRole::Stairs,
        // Roofs, windows and pillars are dressing on a structure, not
        // structure: they carry no edge semantics, so they fit anywhere.
        Some(_) => TileRole::Prop,
        // Role-less: the ambiguous case KitUse exists to settle.
        None if usage == KitUse::Buildings => TileRole::Building,
        None => TileRole::Prop,
    }
}

/// Build a generator [`Kit`] from a pack in the library.
///
/// Returns `None` when the pack is absent or has no usable tiles, which the
/// caller reports as a script error — a silently empty kit produces a level
/// with no tiles, and "nothing appeared" is the hardest failure to diagnose.
pub fn kit_from_index(index: &AssetIndex, pack: &str, usage: KitUse) -> Option<Kit> {
    let pack = pack.strip_prefix("kenney/").unwrap_or(pack);
    let info = index.kits().into_iter().find(|k| k.pack == pack)?;
    let entries = index.kit_tiles(pack, None);
    if entries.is_empty() {
        return None;
    }
    let tile_size = info.tile_size.unwrap_or(1.0);
    let tiles: Vec<TileDef> = entries
        .iter()
        .map(|e| {
            let height = e.size.map(|s| s[1]).filter(|h| *h > 0.01).unwrap_or(FALLBACK_TILE_HEIGHT);
            TileDef::new(e.id.clone(), role_of(e, usage), height)
        })
        .collect();
    Some(Kit::new(format!("kenney/{pack}"), tile_size, tiles))
}

/// One tile of a generated level, flattened to what a placement needs: the
/// asset id to draw, where to put it, and the box that makes it solid.
pub struct PlacedTile {
    pub model: String,
    pub pos: Vec3f,
    pub yaw: f32,
    /// Collider centre — a tile's origin is its floor, so the box sits above.
    pub centre: Vec3f,
    /// Collider half-extents: footprint from the kit pitch, height from the
    /// tile. Floored, because a zero half-extent is a plane the sweep tunnels
    /// straight through.
    pub half: Vec3f,
}

/// Flatten a [`Level`] into placements, resolving each kit-relative tile index
/// back to its asset id.
///
/// Tile indices are kit-relative by design (a kit is one atlas, so one batch),
/// which means the kits used to generate must be the kits used to resolve —
/// hence they are passed back in rather than looked up again.
pub fn level_placements(level: &Level, kits: &[Kit]) -> Vec<PlacedTile> {
    let mut out = Vec::new();
    for layer in &level.layers {
        let Some(kit) = kits.iter().find(|k| k.id == layer.kit_id) else {
            continue;
        };
        for p in &layer.placements {
            let Some(tile) = kit.tiles.get(p.tile as usize) else {
                continue;
            };
            let (centre, half) = p.bounds(layer.tile_size, tile.height);
            out.push(PlacedTile {
                model: tile.id.clone(),
                pos: p.pos,
                yaw: p.yaw(),
                centre,
                half: vec3f(half.x.max(0.05), half.y.max(0.05), half.z.max(0.05)),
            });
        }
    }
    out
}

/// Spawn a hidden collider box for a placed model.
///
/// Every field that matters is set explicitly rather than left to
/// `Entity::default()`. That is not defensive style, it is scar tissue: this
/// codebase has shipped a zero-seed rng (xorshift is a fixed point at zero),
/// zero-gravity rigid bodies and frictionless ground, all because a
/// sim-API-built world inherited a default the DSL path always filled in.
pub fn spawn_collider(
    world: &mut makepad_game_sim::GameWorld,
    centre: Vec3f,
    half: Vec3f,
    tag: &str,
) -> u64 {
    use makepad_game_sim::{BodyKind, Entity, Shape};
    world.mark_render_dirty();
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind: BodyKind::Static,
        shape: Shape::Box,
        pos: centre,
        half: vec3f(half.x.max(0.05), half.y.max(0.05), half.z.max(0.05)),
        color: vec4(0.5, 0.5, 0.5, 1.0),
        tag: tag.to_string(),
        collide: true,
        sensor: false,
        // The mesh is the prop's appearance; this box is only its substance.
        hidden: true,
        gravity_scale: 1.0,
        speed_mult: 1.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.6,
        ..Default::default()
    });
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, role: Option<&'static str>) -> AssetEntry {
        let mut e = AssetEntry {
            id: id.to_string(),
            name: id.to_string(),
            kind: makepad_game_assets::AssetKind::Model,
            path: std::path::PathBuf::new(),
            source: makepad_game_assets::Source::Kenney,
            pack: "city-kit-roads".to_string(),
            license: "CC0",
            credit: "Kenney",
            categories: Vec::new(),
            keywords: Vec::new(),
            themes: Vec::new(),
            rigged: false,
            animated: false,
            clips: Vec::new(),
            joints: 0,
            role,
            kit: true,
            size: Some([1.0, 0.2, 1.0]),
            format: "glb".to_string(),
            duration: None,
            loops: false,
            decodable: true,
            bytes: 0,
        };
        e.role = role;
        e
    }

    /// The index folds crossroads and tees into one `junction` role, but a
    /// 4-way cell needs four open edges. A T standing in for a crossroad
    /// leaves a road stub pointing at nothing.
    #[test]
    fn junction_role_splits_cross_from_tee_by_name() {
        assert_eq!(
            role_of(&entry("kenney/city-kit-roads/road-crossroad", Some("junction")), KitUse::Structure),
            TileRole::Cross
        );
        assert_eq!(
            role_of(&entry("kenney/city-kit-roads/road-intersection", Some("junction")), KitUse::Structure),
            TileRole::Cross
        );
        assert_eq!(
            role_of(&entry("kenney/city-kit-roads/road-split", Some("junction")), KitUse::Structure),
            TileRole::TJunction
        );
    }

    /// `town()` places tiles with role `Building`; a role-less model mapped to
    /// `Prop` means it looks for buildings, finds none, and lays streets with
    /// nothing along them — silently. Kenney's `city-kit-suburban` is exactly
    /// this shape: 40 whole buildings, zero parsed roles.
    #[test]
    fn a_roleless_model_is_a_building_when_the_kit_is_used_for_buildings() {
        let e = entry("kenney/city-kit-suburban/building-type-a", None);
        assert_eq!(role_of(&e, KitUse::Buildings), TileRole::Building);
        assert_eq!(role_of(&e, KitUse::Props), TileRole::Prop);
    }

    #[test]
    fn structural_roles_map_and_dressing_becomes_prop() {
        assert_eq!(
            role_of(&entry("a/b/road-straight", Some("straight")), KitUse::Structure),
            TileRole::Straight
        );
        assert_eq!(role_of(&entry("a/b/wall", Some("wall")), KitUse::Structure), TileRole::Wall);
        assert_eq!(role_of(&entry("a/b/gate", Some("door")), KitUse::Structure), TileRole::Door);
        assert_eq!(
            role_of(&entry("a/b/corner-inner", Some("corner-inner")), KitUse::Structure),
            TileRole::WallCorner
        );
        // A roof connects to nothing: it must fit anywhere rather than
        // constraining its neighbours.
        assert_eq!(role_of(&entry("a/b/roof", Some("roof")), KitUse::Structure), TileRole::Prop);
        assert_eq!(role_of(&entry("a/b/barrel", None), KitUse::Structure), TileRole::Prop);
    }
}
