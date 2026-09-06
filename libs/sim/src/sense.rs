//! Perception probes — what an NPC can find out about the world around it.
//!
//! Every probe reads the SAME solid set the mover sweep collides against
//! (`!sensor && collide && Static|Kinematic|Rigid`), so what an NPC believes it
//! can walk through and what actually stops it cannot disagree. A second,
//! subtly different notion of "solid" is exactly how AI ends up walking into
//! walls it was told were clear.
//!
//! These are deliberately cheap and stateless: a linear scan with an early-out,
//! called a few times per NPC per decision rather than every tick for every
//! NPC. A nav grid would beat them once a level needs real pathfinding through
//! rooms and doorways — see the note in `blocks::npc`.

use makepad_math::*;

use crate::entity::{BodyKind, Entity, Shape};
use crate::queries::overlaps;
use crate::world::GameWorld;

/// A solid a mover would run into.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Obstacle {
    pub id: u64,
    /// World-space top of the blocking box. Compare against the mover's feet:
    /// within step-up it is a kerb, within jump reach it is a crate, above
    /// that it is a wall and the only options are around or give up.
    pub top: f32,
    /// How far ahead the block starts.
    pub dist: f32,
}

/// The sweep's own filter, so perception and collision agree by construction.
#[inline]
fn blocks_movement(e: &Entity) -> bool {
    !e.sensor
        && e.collide
        && matches!(
            e.kind,
            BodyKind::Static | BodyKind::Kinematic | BodyKind::Rigid
        )
}

/// Highest solid within `reach` along `dir`, or None if the way is clear.
///
/// Samples the mover's own box forward rather than casting a ray: a ray
/// through a doorway says "clear" for a body twice its width, and an NPC that
/// trusts it wedges itself in the frame.
pub fn obstacle_ahead(
    world: &GameWorld,
    self_id: u64,
    pos: Vec3f,
    half: Vec3f,
    dir: Vec3f,
    reach: f32,
) -> Option<Obstacle> {
    let len = crate::math::sqrt(dir.x * dir.x + dir.z * dir.z);
    if len <= 1.0e-6 || reach <= 0.0 {
        return None;
    }
    let (dx, dz) = (dir.x / len, dir.z / len);
    const SAMPLES: usize = 3;
    for i in 1..=SAMPLES {
        let t = reach * i as f32 / SAMPLES as f32;
        let probe = vec3f(pos.x + dx * t, pos.y, pos.z + dz * t);
        let mut best: Option<Obstacle> = None;
        for e in &world.entities {
            if e.id == self_id || !blocks_movement(e) {
                continue;
            }
            if overlaps(probe, half, e.pos, e.half) {
                let top = e.pos.y + e.half.y;
                // Tallest wins: a doorstep in front of a wall must report the
                // wall, or the NPC steps up and heads straight into it.
                if best.map_or(true, |b: Obstacle| top > b.top) {
                    best = Some(Obstacle { id: e.id, top, dist: t });
                }
            }
        }
        // Nearest sample that hits anything is the one that matters.
        if best.is_some() {
            return best;
        }
    }
    None
}

/// Height of the surface an NPC would land on `ahead` units along `dir`, or
/// None over a hole. Used to refuse to walk off a ledge.
///
/// Anything whose top is well above the mover's feet is a wall, not a floor,
/// and is ignored here — that case belongs to [`obstacle_ahead`].
pub fn ground_ahead(
    world: &GameWorld,
    pos: Vec3f,
    half: Vec3f,
    dir: Vec3f,
    ahead: f32,
) -> Option<f32> {
    let len = crate::math::sqrt(dir.x * dir.x + dir.z * dir.z);
    if len <= 1.0e-6 {
        return None;
    }
    let probe = vec3f(pos.x + dir.x / len * ahead, pos.y, pos.z + dir.z / len * ahead);
    let feet = pos.y - half.y;
    // Composed world surface: an NPC at a dug pit's edge sees the pit floor,
    // not the pre-dig heightfield it would happily walk out onto.
    let mut ground = world.surface_height_at(probe.x, probe.z);
    for e in &world.entities {
        if !blocks_movement(e) {
            continue;
        }
        let top = e.pos.y + e.half.y;
        if top > feet + 0.6 {
            continue;
        }
        if (probe.x - e.pos.x).abs() < e.half.x && (probe.z - e.pos.z).abs() < e.half.z {
            ground = Some(ground.map_or(top, |g: f32| g.max(top)));
        }
    }
    ground
}

/// Could a body of this size stand here without overlapping anything solid?
pub fn spot_clear(world: &GameWorld, self_id: u64, pos: Vec3f, half: Vec3f) -> bool {
    !world
        .entities
        .iter()
        .any(|e| e.id != self_id && blocks_movement(e) && overlaps(pos, half, e.pos, e.half))
}

/// A stable dry place for a body to stand near `feet`, including an authored
/// dock over water. Water's X/Z planning footprint does not erase an actual
/// supporting floor. The complete foot rectangle must fit, the landing must
/// be above every overlapping water sheet, and the body must have clearance.
///
/// Deliberately bounded to terrain and horizontal axis-aligned box decks:
/// a rail, post, sensor, tilted hull or overhead bridge is not a landing.
/// `up`/`down` bound vertical travel; callers cannot accidentally teleport a
/// shore walker onto a bridge several metres overhead.
pub fn dry_standing_height(
    world: &GameWorld, exclude: [u64; 2], feet: Vec3f, half: Vec3f, up: f32, down: f32,
) -> Option<f32> {
    if [feet.x,feet.y,feet.z,half.x,half.y,half.z,up,down].iter().any(|v|!v.is_finite())
        || half.x<=0.0 || half.y<=0.0 || half.z<=0.0 || up<0.0 || down<0.0 { return None; }
    let corners = [
        (feet.x-half.x, feet.z-half.z), (feet.x-half.x, feet.z+half.z),
        (feet.x+half.x, feet.z-half.z), (feet.x+half.x, feet.z+half.z),
        (feet.x, feet.z),
    ];
    let clear = |height: f32| {
        if !height.is_finite() || height > feet.y+up || height < feet.y-down { return false; }
        // Every overlapping volume, even a narrow strip between the foot
        // probes. The wave crest envelope is conservative: a safe landing
        // does not become submerged on the next animation tick.
        if world.water.as_ref().is_some_and(|water| water.volumes.iter().any(|v|
            feet.x+half.x>=v.min.x && feet.x-half.x<=v.max.x
                && feet.z+half.z>=v.min.z && feet.z-half.z<=v.max.z
                && v.level()+v.amp_sum()>height+0.02)) { return false; }
        let center=vec3f(feet.x,height+half.y+0.025,feet.z);
        !world.entities.iter().any(|e| !exclude.contains(&e.id) && blocks_movement(e)
            && overlaps(center,half,e.pos,e.half))
    };
    let mut best=world.surface_height_at(feet.x,feet.z).filter(|height|
        corners.iter().all(|&(x,z)| world.surface_height_at(x,z)
            .is_some_and(|h| (h-height).abs() <= 0.18)) && clear(*height));
    for e in &world.entities {
        if exclude.contains(&e.id) || !blocks_movement(e) || e.shape != Shape::Box
            || !matches!(e.kind,BodyKind::Static|BodyKind::Kinematic)
            || e.yaw.abs()>1e-5
            || (e.orient.rotate_vec3(&vec3f(0.0,1.0,0.0))-vec3f(0.0,1.0,0.0)).length_squared()>1e-8
            || (e.orient.rotate_vec3(&vec3f(0.0,0.0,1.0))-vec3f(0.0,0.0,1.0)).length_squared()>1e-8
        { continue; }
        if corners.iter().any(|&(x,z)| (x-e.pos.x).abs()>e.half.x-0.01
            || (z-e.pos.z).abs()>e.half.z-0.01) { continue; }
        let height=e.pos.y+e.half.y;
        if clear(height) && best.is_none_or(|h| height>h) { best=Some(height); }
    }
    best
}

/// Is there room to sidestep `offset` units perpendicular to `dir` and keep
/// going? Checks the sidestep AND the step after it, because a gap you can
/// stand in but not walk out of is not a way around.
pub fn side_clearance(
    world: &GameWorld,
    self_id: u64,
    pos: Vec3f,
    half: Vec3f,
    dir: Vec3f,
    side: f32,
    offset: f32,
) -> bool {
    let len = crate::math::sqrt(dir.x * dir.x + dir.z * dir.z);
    if len <= 1.0e-6 {
        return false;
    }
    let (dx, dz) = (dir.x / len, dir.z / len);
    // Perpendicular on the ground plane.
    let (px, pz) = (-dz * side, dx * side);
    let step = vec3f(pos.x + px * offset, pos.y, pos.z + pz * offset);
    if !spot_clear(world, self_id, step, half) {
        return false;
    }
    let onward = vec3f(step.x + dx * offset, step.y, step.z + dz * offset);
    spot_clear(world, self_id, onward, half)
}

#[cfg(test)]
mod standing_tests {
    use super::*;
    use crate::water::{WaterState,WaterVolume};

    #[test]
    fn conservative_water_coverage_does_not_erase_dry_terrain_above_its_surface() {
        let mut world=GameWorld::new();
        world.terrain=Some(crate::terrain::Terrain {
            cells:3,cell_size:4.0,origin:-4.0,heights:vec![0.5;9],
            colors:vec![Vec4f::default();9],revision:1,
        });
        world.water=Some(Box::new(WaterState {volumes:vec![WaterVolume {
            min:vec3f(-10.0,-5.0,-10.0),max:vec3f(10.0,0.0,10.0),
            density:1.0,current:Vec3f::default(),waves:Vec::new(),
            color:Vec4f::default(),entity:0,draw_sheet:false,
        }],..Default::default()}));
        let half=vec3f(0.4,0.9,0.4);
        assert_eq!(dry_standing_height(&world,[0,0],vec3f(0.0,0.5,0.0),half,0.1,0.1),Some(0.5));
        world.terrain.as_mut().unwrap().heights.fill(-0.5);
        assert_eq!(dry_standing_height(&world,[0,0],vec3f(0.0,-0.5,0.0),half,0.1,0.1),None);
    }

    #[test]
    fn dry_support_requires_a_real_whole_footprint_deck_at_foot_height() {
        let mut world=GameWorld::new();
        world.water=Some(Box::new(WaterState {volumes:vec![WaterVolume {
            min:vec3f(-20.0,-4.0,-20.0),max:vec3f(20.0,0.0,20.0),
            density:1.0,current:Vec3f::default(),waves:Vec::new(),
            color:Vec4f::default(),entity:0,draw_sheet:false,
        }],..Default::default()}));
        let feet=vec3f(0.0,0.625,0.0);let half=vec3f(0.4,0.9,0.4);
        let sample=|world:&GameWorld| dry_standing_height(world,[0,0],feet,half,0.1,0.2);
        assert_eq!(sample(&world),None,"open water is not a floor");
        world.push_entity(Entity {id:1,kind:BodyKind::Static,
            pos:vec3f(0.0,0.45,0.0),half:vec3f(2.0,0.15,2.0),collide:true,
            ..Default::default()});
        assert!((sample(&world).unwrap()-0.6).abs()<1e-5);
        world.entity_mut(1).unwrap().sensor=true;
        assert_eq!(sample(&world),None,"a trigger is not a dock");
        world.entity_mut(1).unwrap().sensor=false;
        world.entity_mut(1).unwrap().half.x=0.2;
        assert_eq!(sample(&world),None,"a narrow post cannot carry the whole foot box");
        world.entity_mut(1).unwrap().half.x=2.0;
        world.entity_mut(1).unwrap().pos.y=8.0;
        assert_eq!(sample(&world),None,"do not jump onto an overhead bridge");
        world.entity_mut(1).unwrap().pos.y=-1.0;
        assert_eq!(sample(&world),None,"a submerged deck is not dry support");
        world.entity_mut(1).unwrap().pos.y=0.45;
        world.push_entity(Entity {id:2,kind:BodyKind::Static,
            pos:vec3f(0.0,1.8,0.0),half:vec3f(1.0,0.2,1.0),collide:true,
            ..Default::default()});
        assert_eq!(sample(&world),None,"landing needs body/head clearance too");
    }
}
