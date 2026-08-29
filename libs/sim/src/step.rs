//! The fixed-step world update + touch collection — moved verbatim from
//! gamemaker's game_view.rs (M0 stage A extraction). Float expression order
//! preserved exactly; tapes must replay bit-identically across the move.

use std::collections::HashSet;

use makepad_math::*;

use crate::entity::*;
use crate::queries::*;
use crate::surface::*;
use crate::terrain::TERRAIN_ID;
use crate::world::GameWorld;
use crate::TICK_DT;

/// The blocking solid's top, when it is a STEP (within `climb` of the feet)
/// rather than a wall. None = a real wall, clamp as ever.
fn step_up_top(walls: &[Solid], id: u64, feet: f32, climb: f32) -> Option<f32> {
    let w = walls.iter().find(|w| w.id == id)?;
    let top = w.pos.y + w.half.y;
    if top > feet - 1.0e-3 && top - feet <= climb {
        Some(top)
    } else {
        None
    }
}

pub fn step_world(world: &mut GameWorld) {
    // The sorted-by-id invariant backs every binary-search lookup (world +
    // renderer). push_entity asserts incrementally; this catches everything
    // else (retain/rollback) once per tick in debug builds.
    debug_assert!(world.entities_sorted_by_id());
    let gravity = world.gravity;
    // D4 (F8): last tick's capsule↔rigid contacts become impulses BEFORE the
    // sweeps, so a knocked walker leaves the ground in this very tick. A
    // world without rigid↔mover contact (every pre-mix scenario) has an
    // empty pending list and this is a no-op — the goldens cannot move.
    {
        let GameWorld {
            dynamics, entities, ..
        } = &mut *world;
        crate::dynamics::apply_mover_contacts(dynamics, entities);
    }
    // Voxel terrain maintenance (T2-T4): heightfield hole punches + budgeted
    // remeshing, BEFORE the sweeps so this tick's fresh meshes become
    // colliders in the end-of-tick reconcile and movers read current data.
    // A world without a field returns immediately — pre-voxel content never
    // enters this code.
    if world.voxel.is_some() {
        crate::voxel::update_world_voxel(world, true);
    }
    // Snapshotted BEFORE the kinematic integration below: movers sweep against
    // last tick's kinematic poses. That ordering is load-bearing for tape
    // parity, so this stays a copy — but a copy of the 48-byte `Solid` view,
    // not the whole 208-byte entity.
    let statics: Vec<Solid> = world
        .entities
        .iter()
        // Sensors report touches but never collide; `collide: false` decor
        // neither collides nor reports — both documented contracts.
        // Rigids are included: their pose was read back from box3d at the end
        // of last tick, so at snapshot time they hold a settled position just
        // like a kinematic. Without them a character walks straight through a
        // crate stack, which is the single most obvious "the world isn't real"
        // tell. Movers still do not collide with each other (see BodyKind).
        // Surface-carriers (prop roofs) are excluded here on purpose, so
        // they are in NEITHER sweep list nor the shove clamp: their box
        // would put an invisible flat lid at ridge height over the roof
        // their grid describes. Movers meet them only through the surface
        // queries below; rigid dynamics and raycasts still see the box.
        .filter(|e| {
            !e.sensor
                && e.collide
                && e.surface.is_none()
                && matches!(
                    e.kind,
                    BodyKind::Static | BodyKind::Kinematic | BodyKind::Rigid
                )
        })
        .map(Solid::from)
        .collect();
    let surfaces: Vec<SurfaceSolid> = world
        .entities
        .iter()
        .filter(|e| !e.sensor && e.collide && e.kind == BodyKind::Static)
        .filter_map(|e| {
            e.surface.as_ref().map(|grid| SurfaceSolid {
                id: e.id,
                pos: e.pos,
                grid: grid.clone(),
            })
        })
        .collect();
    // Wedges are RAMPS, not walls. They are excluded from the axis sweeps and
    // handled as floors below, the same way the terrain is: a surface you walk
    // up, blocked only where it rises faster than you can climb. Collided as
    // their bounding box (which is what happened before) the ramp built to be
    // walked and driven up was an invisible cube.
    let walls: Vec<Solid> = statics
        .iter()
        .filter(|s| s.shape != Shape::Wedge)
        .copied()
        .collect();
    /// A step this tall walks up for free (Godot floor snapping over the
    /// terraced 0.5 steps); anything taller is a cliff wall.
    const CLIMB: f32 = 0.55;

    // Terrain is read-only here and was previously cloned once per tick purely
    // to dodge the `&mut entities` borrow — 1.3 MB of memcpy per tick on a
    // 257² field. Splitting the struct borrow costs nothing and copies nothing.
    let GameWorld {
        entities: world_entities,
        terrain: world_terrain,
        terrain_materials: world_terrain_materials,
        voxel: world_voxel,
        dynamics,
        ..
    } = &mut *world;
    let terrain = world_terrain.as_ref();
    let voxel = world_voxel.as_deref();

    // F9: projectiles moving further per tick than their own smallest half
    // extent can step OVER thin geometry in the axis sweeps — the classic
    // tunnel. Those get an exact box3d shapecast along their motion below.
    // The box3d mirror normally reconciles at the END of the tick, so when a
    // cast is due this tick we reconcile up front too — entity state is
    // untouched by reconcile, and slow-projectile worlds (all pre-mix
    // content) never enter this branch, so their box3d call sequence is
    // byte-identical to before.
    let ccd_active = world_entities.iter().any(|e| {
        e.kind == BodyKind::Mover && e.hits && e.attached_to == 0 && {
            let min_half = e.half.x.min(e.half.y).min(e.half.z).max(1.0e-3);
            crate::vec3_len(e.vel * TICK_DT) > min_half
        }
    });
    // P3: capsule-flagged movers resolve against the box3d mirror instead of
    // the axis sweeps, so the mirror must exist before the mover loop — the
    // same up-front reconcile CCD needs. Worlds without either (all pre-mix
    // content) skip it and keep their box3d call sequence byte-identical.
    let capsule_active = world_entities
        .iter()
        .any(|e| e.kind == BodyKind::Mover && e.capsule_collider && e.attached_to == 0);
    if ccd_active || capsule_active {
        crate::dynamics::reconcile(
            dynamics,
            world_entities,
            terrain,
            world_terrain_materials.as_ref(),
            world_voxel.as_deref(),
            gravity,
        );
    }
    // Projectiles never stop each other (documented: `hits` entities pass
    // through their own kind — the overlap IS the hit). Sorted for the
    // binary search inside the cast callback; entity order is id order.
    let projectile_ids: Vec<u64> = if ccd_active {
        world_entities.iter().filter(|e| e.hits).map(|e| e.id).collect()
    } else {
        Vec::new()
    };

    // Kinematics move first (script set their velocity).
    for e in world_entities.iter_mut() {
        if e.kind == BodyKind::Kinematic {
            e.pos = e.pos + e.vel * TICK_DT;
        }
    }

    // D4 (F8), mover-pushes-rigid half: a mover whose x/z sweep clamps
    // against a RIGID records a push here; applied as impulses after the
    // loop (the pressed rigid is another entity in the slice this loop
    // borrows mutably). Empty in every pre-mix scenario that keeps walkers
    // and rigids apart — allocation-free until someone actually presses.
    let mut sweep_pushes: Vec<crate::dynamics::SweepPush> = Vec::new();

    for e in world_entities.iter_mut() {
        if e.kind != BodyKind::Mover {
            continue;
        }
        // An active articulation is authoritative for this entity. Its old
        // mover sweep/controller must not integrate a second body over it.
        if dynamics.ragdoll_active(e.id) {
            continue;
        }
        // Riders are pinned to their vehicle after this loop, not simulated.
        if e.attached_to != 0 {
            continue;
        }
        // Where this tick's motion started, for the projectile CCD clamp.
        let ccd_start = e.pos;
        // Carried by the platform we stand on.
        if e.on_floor && e.floor_id != 0 {
            if let Some(base) = statics.iter().find(|s| s.id == e.floor_id) {
                if base.kind == BodyKind::Kinematic {
                    e.pos = e.pos + base.vel * TICK_DT;
                }
            }
        }

        e.vel.y -= gravity * e.gravity_scale * TICK_DT;

        // P3: the opt-in capsule path replaces the axis sweeps wholesale —
        // box3d contact planes + solve_planes give real wall sliding and
        // smooth corners. It reports through the same contract fields
        // (on_floor/floor_id/hit_wall/normals), so everything downstream of
        // the sweep works unchanged. Default-off: the flag is the only gate.
        if e.capsule_collider {
            crate::dynamics::capsule_mover_step(dynamics, e, &mut sweep_pushes);
            continue;
        }

        // Axis-separated sweeps: x, z, then y (so walking into a wall while
        // falling doesn't stick, and floors resolve last for on_floor).
        e.hit_wall = 0;
        e.wall_normal = vec3f(0.0, 0.0, 0.0);
        let feet = e.pos.y - e.half.y;
        // Voxel terrain (T4): a mover whose feet stand in carved-open air of
        // a materialized chunk is legitimately under the heightfield surface
        // (a tunnel). The terrain floor/cliff rules are suppressed for it —
        // the voxel checks below take over. Always false without a field, so
        // pre-voxel worlds run the exact old expressions.
        let in_voxel_air = voxel.map_or(false, |v| {
            v.is_carved_air(vec3f(e.pos.x, feet + 0.1, e.pos.z))
        });
        let (nx, hx, hx_id) = sweep_axis(&walls, e.id, e.pos, e.half, 0, e.vel.x * TICK_DT);
        e.pos.x = nx;
        if hx != 0.0 && !e.hits {
            // Box step-up (the CLIMB contract, finally honoured for BOXES):
            // a solid whose top is within step reach of the feet is a kerb,
            // not a wall — retry the sweep with the body lifted onto it.
            // Terrain terraces and wedges always had this; box floors (a
            // generated dungeon's slabs, a doorstep, a crate lid) clamped
            // flat-footed movers forever and read as invisible walls.
            // NEVER for projectiles (`hits`): a bullet lifted over a crate
            // reads as a ricochet — bullets stop at the first solid.
            if let Some(step) = step_up_top(&walls, hx_id, feet, CLIMB) {
                let mut lifted = e.pos;
                lifted.y = step + e.half.y + 1.0e-3;
                let (nx2, hx2, _) =
                    sweep_axis(&walls, e.id, lifted, e.half, 0, e.vel.x * TICK_DT);
                if hx2 == 0.0 {
                    e.pos = lifted;
                    e.pos.x = nx2;
                }
            }
        }
        if hx != 0.0 && e.pos.x == nx {
            record_sweep_push(&mut sweep_pushes, &walls, hx_id, e, vec3f(hx, 0.0, 0.0));
            e.vel.x = 0.0;
            e.hit_wall = hx_id;
            // The wall faces back at the mover: hx carries the sweep's own
            // hit sign (+1 when moving +x), so the normal is its negation.
            e.wall_normal = vec3f(-hx, 0.0, 0.0);
        }
        // A ramp too steep to step onto blocks exactly like a terrain cliff —
        // which is what makes the wedge's vertical back face still a wall.
        if let Some((ramp, _)) = ramp_floor_under(&statics, e.pos, e.half) {
            if ramp > feet + CLIMB {
                e.pos.x = nx - e.vel.x * TICK_DT;
                e.vel.x = 0.0;
            }
        }
        // Terrain cliffs block sideways movement; steps ≤ CLIMB pass (the y
        // pass snaps the mover up onto them).
        if !in_voxel_air {
            if let Some(t) = terrain {
                if let Some(ground) = t.floor_under(e.pos, e.half) {
                    if ground > feet + CLIMB {
                        e.pos.x = nx - e.vel.x * TICK_DT;
                        let dir = e.vel.x;
                        e.vel.x = 0.0;
                        if e.hit_wall == 0 {
                            e.hit_wall = TERRAIN_ID;
                            if dir != 0.0 {
                                e.wall_normal =
                                    vec3f(if dir > 0.0 { -1.0 } else { 1.0 }, 0.0, 0.0);
                            }
                        }
                    }
                }
            }
        }
        // Solid voxels above step height are walls (a blocky tower's side, a
        // tunnel's wall) — same contract as terrain cliffs.
        if let Some(v) = voxel {
            if v.solid_in_box(
                vec3f(e.pos.x - e.half.x, feet + CLIMB, e.pos.z - e.half.z),
                vec3f(e.pos.x + e.half.x, e.pos.y + e.half.y - 0.02, e.pos.z + e.half.z),
            ) {
                e.pos.x = nx - e.vel.x * TICK_DT;
                e.vel.x = 0.0;
                if e.hit_wall == 0 {
                    e.hit_wall = TERRAIN_ID;
                }
            }
        }
        // A roof's grid rising past step height blocks like a terrain cliff —
        // but only for the mover already standing on that roof (floor_id
        // gate); from the ground its columns are overhead geometry and the
        // prop's wall boxes do the blocking.
        if surface_rise_blocks(&surfaces, e.floor_id, e.pos, e.half, feet + CLIMB) {
            e.pos.x = nx - e.vel.x * TICK_DT;
            e.vel.x = 0.0;
        }
        let (nz, hz, hz_id) = sweep_axis(&walls, e.id, e.pos, e.half, 2, e.vel.z * TICK_DT);
        e.pos.z = nz;
        if hz != 0.0 && !e.hits {
            // Box step-up, z pass — see the x pass (and the same projectile
            // exclusion: bullets stop, they don't climb).
            let feet_now = e.pos.y - e.half.y;
            if let Some(step) = step_up_top(&walls, hz_id, feet_now, CLIMB) {
                let mut lifted = e.pos;
                lifted.y = step + e.half.y + 1.0e-3;
                let (nz2, hz2, _) =
                    sweep_axis(&walls, e.id, lifted, e.half, 2, e.vel.z * TICK_DT);
                if hz2 == 0.0 {
                    e.pos = lifted;
                    e.pos.z = nz2;
                }
            }
        }
        if hz != 0.0 && e.pos.z == nz {
            record_sweep_push(&mut sweep_pushes, &walls, hz_id, e, vec3f(0.0, 0.0, hz));
            e.vel.z = 0.0;
            if e.hit_wall == 0 {
                e.hit_wall = hz_id;
                e.wall_normal = vec3f(0.0, 0.0, -hz);
            }
        }
        if !in_voxel_air {
            if let Some(t) = terrain {
                if let Some(ground) = t.floor_under(e.pos, e.half) {
                    if ground > feet + CLIMB {
                        e.pos.z = nz - e.vel.z * TICK_DT;
                        let dir = e.vel.z;
                        e.vel.z = 0.0;
                        if e.hit_wall == 0 {
                            e.hit_wall = TERRAIN_ID;
                            if dir != 0.0 {
                                e.wall_normal =
                                    vec3f(0.0, 0.0, if dir > 0.0 { -1.0 } else { 1.0 });
                            }
                        }
                    }
                }
            }
        }
        if let Some(v) = voxel {
            if v.solid_in_box(
                vec3f(e.pos.x - e.half.x, feet + CLIMB, e.pos.z - e.half.z),
                vec3f(e.pos.x + e.half.x, e.pos.y + e.half.y - 0.02, e.pos.z + e.half.z),
            ) {
                e.pos.z = nz - e.vel.z * TICK_DT;
                e.vel.z = 0.0;
                if e.hit_wall == 0 {
                    e.hit_wall = TERRAIN_ID;
                }
            }
        }
        if let Some((ramp, _)) = ramp_floor_under(&statics, e.pos, e.half) {
            if ramp > feet + CLIMB {
                e.pos.z = nz - e.vel.z * TICK_DT;
                e.vel.z = 0.0;
            }
        }
        if surface_rise_blocks(&surfaces, e.floor_id, e.pos, e.half, feet + CLIMB) {
            e.pos.z = nz - e.vel.z * TICK_DT;
            e.vel.z = 0.0;
        }
        let (ny, hy, hy_id) = sweep_axis(&walls, e.id, e.pos, e.half, 1, e.vel.y * TICK_DT);
        e.pos.y = ny;
        e.on_floor = false;
        e.floor_id = 0;
        e.floor_normal = vec3f(0.0, 0.0, 0.0);
        if hy != 0.0 {
            if e.vel.y < 0.0 {
                e.on_floor = true;
                e.floor_id = hy_id;
                // Landed on a box top: flat.
                e.floor_normal = vec3f(0.0, 1.0, 0.0);
            }
            e.vel.y = 0.0;
            if e.hit_wall == 0 && e.hits {
                // A lobbed projectile landing counts as a hit too.
                e.hit_wall = hy_id;
            }
        }
        // A ramp is a floor: standing on the slope is the point of it.
        if let Some((ramp, id)) = ramp_floor_under(&statics, e.pos, e.half) {
            let floor_y = ramp + e.half.y;
            if e.pos.y <= floor_y {
                e.pos.y = floor_y;
                if e.vel.y <= 0.0 {
                    e.on_floor = true;
                    e.floor_id = id;
                    // The slope's true normal (P2), from the wedge that won
                    // the footprint probe — additive: nothing below reads it.
                    if let Some(s) = statics.iter().find(|s| s.id == id) {
                        e.floor_normal = wedge_normal(s);
                    }
                    if e.hit_wall == 0 && e.hits {
                        // A projectile touching a slope has hit it — never
                        // snap-and-slide along the wedge.
                        e.hit_wall = id;
                    }
                    e.vel.y = 0.0;
                }
            }
        }
        // The terrain is a floor: feet never sink below the ground surface —
        // unless the mover stands in carved-open voxel air (a tunnel), where
        // snapping to the heightfield would teleport it up through the roof.
        if !in_voxel_air {
            if let Some(t) = terrain {
                if let Some(ground) = t.floor_under(e.pos, e.half) {
                    let floor_y = ground + e.half.y;
                    if e.pos.y <= floor_y {
                        e.pos.y = floor_y;
                        if e.vel.y <= 0.0 {
                            e.on_floor = true;
                            e.floor_id = 0;
                            // Triangle normal under the mover's centre — the
                            // same triangle height_at collides with (P2).
                            e.floor_normal = t
                                .normal_at(e.pos.x, e.pos.z)
                                .unwrap_or(vec3f(0.0, 1.0, 0.0));
                            if e.hit_wall == 0 && e.hits {
                                e.hit_wall = TERRAIN_ID;
                            }
                            e.vel.y = 0.0;
                        }
                    }
                }
            }
        }
        // Voxel ground is a floor exactly like the terrain: the highest
        // air→solid crossing under the footprint, scanned down from step
        // reach. This is what a blocky tower's top and a tunnel's floor are.
        if let Some(v) = voxel {
            if let Some(ground) = v.floor_under(e.pos, e.half, (e.pos.y - e.half.y) + CLIMB) {
                let floor_y = ground + e.half.y;
                if e.pos.y <= floor_y {
                    e.pos.y = floor_y;
                    if e.vel.y <= 0.0 {
                        e.on_floor = true;
                        e.floor_id = 0;
                        // Voxel floors report a flat normal until the voxel
                        // core grows a surface-normal query — blocky tops and
                        // tunnel floors are flat; sculpted slopes read flat.
                        e.floor_normal = vec3f(0.0, 1.0, 0.0);
                        if e.hit_wall == 0 && e.hits {
                            e.hit_wall = TERRAIN_ID;
                        }
                        e.vel.y = 0.0;
                    }
                }
            }
            // A tunnel roof stops a jump: rising into materialized solid
            // zeroes the climb instead of letting the head cross into the
            // ridge (where the heightfield snap would take over).
            if e.vel.y > 0.0 {
                let head = vec3f(e.pos.x, e.pos.y + e.half.y + 0.05, e.pos.z);
                if v.is_solid_at(head) {
                    e.vel.y = 0.0;
                }
            }
        }
        // A prop's top surface is a floor the way the terrain is — for feet
        // already within step reach of it. Columns further above never grab
        // a mover walking beneath them, which is what keeps a roofed doorway
        // open while the roof still catches whoever drops onto it.
        if let Some((surf, id)) = surface_floor_under(&surfaces, e.pos, e.half, feet + CLIMB) {
            let floor_y = surf + e.half.y;
            if e.pos.y <= floor_y {
                e.pos.y = floor_y;
                if e.vel.y <= 0.0 {
                    e.on_floor = true;
                    e.floor_id = id;
                    // Roof grids are stepped columns: flat.
                    e.floor_normal = vec3f(0.0, 1.0, 0.0);
                    if e.hit_wall == 0 && e.hits {
                        // Same hit contract as terrain/voxel floors: a
                        // projectile landing on a prop surface stops there.
                        e.hit_wall = id;
                    }
                    e.vel.y = 0.0;
                }
            }
        }
        // F9: the continuous sweep. Only for projectiles that outran their
        // own size this tick — anything slower already got exact treatment
        // from the axis sweeps above, and stays bit-identical to the old
        // path. The cast runs start→post-sweep-end, so it can only find
        // geometry the sweeps MISSED (a wall the sweep stopped at leaves the
        // end point short of that wall, and the cast comes up empty).
        if ccd_active && e.hits {
            let min_half = e.half.x.min(e.half.y).min(e.half.z).max(1.0e-3);
            if crate::vec3_len(e.pos - ccd_start) > min_half {
                if let Some((pos, hit_id)) = crate::dynamics::projectile_ccd(
                    dynamics,
                    &projectile_ids,
                    e.id,
                    ccd_start,
                    e.pos,
                    e.half,
                ) {
                    e.pos = pos;
                    e.vel = vec3f(0.0, 0.0, 0.0);
                    e.hit_wall = hit_id;
                }
            }
        }
    }

    // Walkers shove the light rigids they pressed into this tick (D4/F8).
    // Impulses go into the box3d bodies; the poses come back through the
    // normal read-back at the end of the tick.
    if !sweep_pushes.is_empty() {
        crate::dynamics::apply_sweep_pushes(dynamics, &sweep_pushes);
    }

    // Movers shove each other apart. After the whole integration loop, so the
    // outcome doesn't depend on who was stepped first, and before rider
    // pinning, which is authoritative over anything this could do to a rider.
    separate_movers(world_entities, &statics);

    // Pin riders to their owners (vehicle seats, latched headcrabs). One pass
    // after integration, same frame the owner moved — the Godot mount pattern.
    // Most worlds attach nothing, and this pose table used to be built over
    // every entity regardless; the guard skips both the allocation and the
    // scan when no rider exists. The loop below is a no-op in that case, so
    // behaviour is unchanged.
    let owner_pose: Vec<(u64, Vec3f, f32)> = if world.entities.iter().any(|e| e.attached_to != 0) {
        world.entities.iter().map(|e| (e.id, e.pos, e.yaw)).collect()
    } else {
        Vec::new()
    };
    for e in world.entities.iter_mut() {
        if e.attached_to == 0 {
            continue;
        }
        if let Some((_, base, owner_yaw)) =
            owner_pose.iter().find(|(id, _, _)| *id == e.attached_to)
        {
            e.pos = *base + e.attach_offset;
            e.vel = vec3f(0.0, 0.0, 0.0);
            if e.attach_ride {
                // A latched rider scrabbles: its model spins in place.
                e.yaw += e.attach_spin * TICK_DT;
            } else {
                // A seated passenger faces where the vehicle faces.
                e.yaw = *owner_yaw;
            }
        } else {
            // Owner despawned: let go rather than freezing in the air.
            e.attached_to = 0;
        }
    }

    // Visual animation: facing, model scale, part poses. Rendering-only
    // state, but stepped with physics so input tapes replay identically.
    for e in world.entities.iter_mut() {
        if e.auto_face && e.kind == BodyKind::Mover {
            let speed = crate::math::sqrt(e.vel.x * e.vel.x + e.vel.z * e.vel.z);
            if speed > 0.2 {
                // Godot's shared _drive(): face where you walk, turn-rate
                // clamped, fronts at -z.
                let want = crate::math::atan2(-e.vel.x, -e.vel.z);
                let mut diff = want - e.yaw;
                while diff > std::f32::consts::PI {
                    diff -= std::f32::consts::TAU;
                }
                while diff < -std::f32::consts::PI {
                    diff += std::f32::consts::TAU;
                }
                let max_turn = e.turn_rate * TICK_DT;
                e.yaw += diff.clamp(-max_turn, max_turn);
            }
        }
        let ease = (6.0 * TICK_DT).min(1.0);
        e.scale = e.scale + (e.scale_target - e.scale) * ease;
    }
    // Part easing runs only while a move_part animation is live; on arrival the
    // part snaps to its target and settles, making it slab-eligible again.
    let mut settled_owners: Vec<u64> = Vec::new();
    for part in world.parts.iter_mut() {
        if !part.anim_active {
            continue;
        }
        let ease = (part.rate * TICK_DT).min(1.0);
        part.offset = part.offset + (part.target_offset - part.offset) * ease;
        part.rot = part.rot + (part.target_rot - part.rot) * ease;
        part.half = part.half + (part.target_half - part.half) * ease;
        let remaining = crate::vec3_len(part.target_offset - part.offset)
            + crate::vec3_len(part.target_rot - part.rot)
            + crate::vec3_len(part.target_half - part.half);
        if remaining < 1.0e-3 {
            part.offset = part.target_offset;
            part.rot = part.target_rot;
            part.half = part.target_half;
            part.anim_active = false;
            settled_owners.push(part.owner);
        }
    }
    if !settled_owners.is_empty()
        && settled_owners.iter().any(|o| world.is_static_visual(*o))
    {
        // A static owner's decoration finished moving: it re-enters the slab.
        world.mark_render_dirty();
    }

    // Projectile lifetimes: `life` seconds, then gone. The knockdown timer
    // (D4/F8) counts down in the same pass — integer state, so worlds where
    // it never gets set (everything pre-mix) are untouched bit-for-bit.
    // Camera shake decays HERE, not in a host loop: gamemaker decayed it in
    // its own tick and the sandbox never did, so every gunshot's recoil
    // ratcheted cam_shake to the 1.5 clamp and the view shook forever.
    // ×0.88/tick ≈ gone in half a second — the documented feel. Cosmetic
    // but deterministic (pure f32 op, pinned to zero in tapes at read time).
    if world.cam_shake > 0.0 {
        world.cam_shake = (world.cam_shake * 0.88) - 1.0e-3;
        if world.cam_shake < 0.0 {
            world.cam_shake = 0.0;
        }
    }
    let mut expired = false;
    let mut expired_static = false;
    for e in world.entities.iter_mut() {
        if e.knocked_down > 0 {
            e.knocked_down -= 1;
        }
        if e.life > 0.0 {
            e.life -= TICK_DT;
            if e.life <= 0.0 {
                e.life = f32::NEG_INFINITY;
                expired = true;
                expired_static |= e.kind == BodyKind::Static;
            }
        }
    }
    if expired {
        world.entities.retain(|e| e.life != f32::NEG_INFINITY);
        // Dynamic entities never live in the cached static slabs, but a
        // delayed static removal must invalidate them just like game.remove
        // does immediately. Without this the authoritative entity vanished
        // while its old geometry and shadow remained drawn indefinitely.
        if expired_static {
            world.mark_render_dirty();
        }
    }

    // Decoration follows its owner out (lifetime, game.remove, whatever).
    if !world.parts.is_empty() || !world.labels.is_empty() {
        let ids: HashSet<u64> = world.entities.iter().map(|e| e.id).collect();
        world.parts.retain(|p| ids.contains(&p.owner));
        world.labels.retain(|l| ids.contains(&l.owner));
    }

    // box3d dynamics layer (M1a): reconcile the body mirror against the
    // post-retain entity set, step the solver, read rigid poses back. Runs
    // LAST so a rigid spawned/removed/rolled-back this tick is already
    // settled in the entity list. Since D2 the mirror includes movers as
    // kinematic capsules, so exact queries and rigid contacts can see them
    // — but box3d still never writes a mover back.
    {
        let GameWorld {
            dynamics,
            entities,
            terrain,
            terrain_materials,
            voxel,
            water,
            gravity,
            tick,
            ..
        } = world;
        crate::dynamics::reconcile(
            dynamics,
            entities,
            terrain.as_ref(),
            terrain_materials.as_ref(),
            voxel.as_deref(),
            *gravity,
        );
        // Water buoyancy (W2): the dynamics pre-step force pass. AFTER the
        // reconcile so a rigid spawned this tick already has its body, BEFORE
        // the solver step so the forces land in this tick's integration —
        // the same slot the car suspension effectively occupies (it applies
        // its forces in Blocks::pre_step, which accumulate into this same
        // world_step). A world without a water field never enters.
        if let Some(water) = water.as_deref_mut() {
            crate::water::apply_buoyancy(dynamics, entities, water, *gravity, *tick);
        }
        crate::dynamics::step_dynamics(dynamics, entities);
    }
}

/// If the solid a mover's sweep just clamped against is a RIGID, record the
/// push (D4: walkers shove light rigids). `dir` is the blocked axis with the
/// sweep's own hit sign — the direction the mover was moving. The speed is
/// read BEFORE the sweep zeroes the velocity, i.e. the mover's intended
/// motion into the rigid this tick.
fn record_sweep_push(
    pushes: &mut Vec<crate::dynamics::SweepPush>,
    walls: &[Solid],
    hit_id: u64,
    mover: &Entity,
    dir: Vec3f,
) {
    let Ok(at) = walls.binary_search_by_key(&hit_id, |s| s.id) else {
        return;
    };
    if walls[at].kind != BodyKind::Rigid {
        return;
    }
    let speed = (mover.vel.x * dir.x + mover.vel.z * dir.z).abs();
    if speed <= crate::dynamics::CONTACT_MIN_APPROACH {
        return;
    }
    pushes.push(crate::dynamics::SweepPush {
        rigid_id: hit_id,
        dir,
        speed,
        mover_mass: push_mass_of(mover),
    });
}

/// Which side of body `a` a touching body `b` sits on — the one fact the
/// classic platformer contact model needs ("from above" is a stomp, "from the
/// side" is a hit) and that a bare `(a, b)` pair cannot express.
///
/// Judged on geometry at the moment of contact, in `a`'s frame: `b`'s feet
/// above `a`'s centre is `Above`, `b`'s head below `a`'s centre is `Below`,
/// everything else is `Side`. Movers pass through each other spatially, so
/// there is no contact normal to read here; the box relation is the honest
/// answer and it is deterministic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ContactSide {
    /// `b` came from above `a` (a stomp on `a`).
    Above,
    /// `b` is under `a`.
    Below,
    /// A flank contact.
    #[default]
    Side,
}

impl ContactSide {
    pub fn name(self) -> &'static str {
        match self {
            ContactSide::Above => "above",
            ContactSide::Below => "below",
            ContactSide::Side => "side",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "above" => ContactSide::Above,
            "below" => ContactSide::Below,
            "side" => ContactSide::Side,
            _ => return None,
        })
    }

    /// The same contact seen from the other body: `b` above `a` is `a`
    /// below `b`.
    pub fn mirror(self) -> Self {
        match self {
            ContactSide::Above => ContactSide::Below,
            ContactSide::Below => ContactSide::Above,
            ContactSide::Side => ContactSide::Side,
        }
    }
}

/// Where `b` sits relative to `a` — see [`ContactSide`].
pub fn contact_side_of(a: &Entity, b: &Entity) -> ContactSide {
    let b_feet = b.pos.y - b.half.y;
    let b_head = b.pos.y + b.half.y;
    if b_feet >= a.pos.y {
        ContactSide::Above
    } else if b_head <= a.pos.y {
        ContactSide::Below
    } else {
        ContactSide::Side
    }
}

/// The touch pairs of this tick, each with the side of the first body the
/// second one is on. `collect_touches` is this without the side.
pub fn collect_touches_sided(world: &GameWorld) -> Vec<(u64, u64, ContactSide)> {
    let mut touches = Vec::new();
    for sensor in world
        .entities
        .iter()
        .filter(|e| e.sensor && !e.non_interactive && e.tag != "tracer")
    {
        // Rigid inclusion is additive: with no Rigid entities the pair set is
        // identical to the pre-M1a scan (tape parity).
        for other in world
            .entities
            .iter()
            .filter(|e| {
                !e.non_interactive && matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
            })
        {
            if overlaps(sensor.pos, sensor.half, other.pos, other.half) {
                touches.push((sensor.id, other.id, contact_side_of(sensor, other)));
            }
        }
    }
    // `hits` entities (projectiles) report movers/kinematics they overlap —
    // movers pass through each other spatially, so overlap IS the hit — plus
    // whatever solid the sweep stopped them against this tick.
    for hitter in world
        .entities
        .iter()
        .filter(|e| e.hits && !e.non_interactive)
    {
        if hitter.hit_wall != 0 {
            let side = world
                .entity(hitter.hit_wall)
                .map(|w| contact_side_of(hitter, w))
                .unwrap_or_default();
            touches.push((hitter.id, hitter.hit_wall, side));
        }
        for other in world.entities.iter() {
            if other.id == hitter.id
                || other.sensor
                || other.non_interactive
                || other.hits
                || !matches!(
                    other.kind,
                    BodyKind::Mover | BodyKind::Kinematic | BodyKind::Rigid
                )
            {
                continue;
            }
            if overlaps(hitter.pos, hitter.half, other.pos, other.half) {
                touches.push((hitter.id, other.id, contact_side_of(hitter, other)));
            }
        }
    }
    touches
}

pub fn collect_touches(world: &GameWorld) -> Vec<(u64, u64)> {
    collect_touches_sided(world)
        .into_iter()
        .map(|(a, b, _)| (a, b))
        .collect()
}

#[cfg(test)]
mod ramp_tests {
    use super::*;

    /// A ramp 12 wide, 2 tall, 7 deep, sitting on the ground: its surface runs
    /// from y=0 at z=-7 up to y=4 at z=+7.
    fn world_with_ramp() -> GameWorld {
        let mut w = GameWorld::new();
        w.gravity = 30.0;
        // Ground. Without it the walker falls past the ramp and meets it from
        // BELOW, where being blocked is correct — which is what the first
        // version of this test was actually measuring.
        w.next_id += 1;
        let ground = w.next_id;
        w.push_entity(Entity {
            id: ground,
            kind: BodyKind::Static,
            shape: Shape::Box,
            pos: vec3f(0.0, -0.5, 0.0),
            half: vec3f(60.0, 0.5, 60.0),
            collide: true,
            push_mass: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.6,
            ..Default::default()
        });
        w.next_id += 1;
        let ramp = w.next_id;
        w.push_entity(Entity {
            id: ramp,
            kind: BodyKind::Static,
            shape: Shape::Wedge,
            pos: vec3f(0.0, 2.0, 0.0),
            half: vec3f(6.0, 2.0, 7.0),
            collide: true,
            push_mass: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.6,
            ..Default::default()
        });
        w.next_id += 1;
        let walker = w.next_id;
        w.push_entity(Entity {
            id: walker,
            kind: BodyKind::Mover,
            shape: Shape::Box,
            // On the ground just in front of the ramp's low edge.
            pos: vec3f(0.0, 0.9, -9.0),
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
        });
        w
    }

    fn walker(w: &GameWorld) -> Entity {
        w.entities.iter().find(|e| e.kind == BodyKind::Mover).unwrap().clone()
    }

    /// The reported bug: "i dont seem to be able to walk the character up the
    /// slanted block towards the platform".
    ///
    /// A wedge was collided as the box that CONTAINS it, so the ramp built to
    /// be walked up presented a vertical wall at its low edge. Walking into it
    /// stopped you dead against nothing you could see.
    #[test]
    fn a_character_can_walk_up_a_ramp() {
        let mut w = world_with_ramp();
        let start = walker(&w).pos;
        for _ in 0..180 {
            if let Some(e) = w.entities.iter_mut().find(|e| e.kind == BodyKind::Mover) {
                e.vel.x = 0.0;
                e.vel.z = 4.0; // walk toward the ramp's low edge and up it
            }
            step_world(&mut w);
        }
        let end = walker(&w).pos;
        assert!(
            end.z > start.z + 6.0,
            "walker only advanced from z={:.2} to z={:.2} — it is stuck on the ramp's face",
            start.z,
            end.z
        );
        assert!(
            end.y > start.y + 1.5,
            "walker reached z={:.2} but only rose to y={:.2} from {:.2} — it walked THROUGH the \
             ramp instead of up it",
            end.z,
            end.y,
            start.y
        );
    }

    /// The slope is walkable; the vertical back face is not. Approaching from
    /// behind (+z, walking in -z) must still be blocked, or the "ramp" is just
    /// a hole in the world.
    #[test]
    fn the_ramps_tall_back_face_is_still_a_wall() {
        let mut w = world_with_ramp();
        if let Some(e) = w.entities.iter_mut().find(|e| e.kind == BodyKind::Mover) {
            e.pos = vec3f(0.0, 0.9, 9.0); // behind the tall edge, on the ground
        }
        for _ in 0..120 {
            if let Some(e) = w.entities.iter_mut().find(|e| e.kind == BodyKind::Mover) {
                e.vel.z = -4.0;
            }
            step_world(&mut w);
        }
        let end = walker(&w);
        assert!(
            end.pos.z > 6.5,
            "walker at z={:.2} pushed into the ramp's full-height back face",
            end.pos.z
        );
        assert!(
            end.pos.y < 2.0,
            "walker was lifted to y={:.2} by a face it should have been stopped by",
            end.pos.y
        );
    }

    /// Standing on the slope must report a floor, or the controller refuses to
    /// jump and the walk animation never plays.
    #[test]
    fn standing_on_the_slope_counts_as_being_on_the_floor() {
        let mut w = world_with_ramp();
        if let Some(e) = w.entities.iter_mut().find(|e| e.kind == BodyKind::Mover) {
            e.pos = vec3f(0.0, 4.0, 0.0); // above the middle of the ramp
        }
        for _ in 0..90 {
            step_world(&mut w);
        }
        let end = walker(&w);
        assert!(end.on_floor, "not on_floor while resting on the ramp");
        // Mid-ramp the surface is half its height: 0 + 2.0, plus half a body.
        assert!(
            (end.pos.y - (2.0 + 0.9)).abs() < 0.2,
            "resting at y={:.2}, expected ~2.9 on the middle of the slope",
            end.pos.y
        );
    }
}

#[cfg(test)]
mod tracer_presentation_tests {
    use super::*;

    #[test]
    fn tracer_advances_without_touch_or_physics_side_effects() {
        let mut world = GameWorld::new();
        world.gravity = 9.81;
        world.push_entity(Entity {
            id: 1,
            kind: BodyKind::Rigid,
            pos: vec3f(0.0, 1.0, -1.0),
            half: vec3f(0.5, 0.5, 0.5),
            collide: true,
            density: 1.0,
            ..Default::default()
        });
        let start = vec3f(0.0, 1.0, 0.0);
        let velocity = vec3f(0.0, 0.0, -90.0);
        world.push_entity(Entity {
            id: 2,
            kind: BodyKind::Kinematic,
            pos: start,
            vel: velocity,
            half: vec3f(0.014, 0.014, 0.55),
            tag: "tracer".to_string(),
            sensor: true,
            collide: false,
            gravity_scale: 1.0,
            life: 1.0,
            ..Default::default()
        });

        step_world(&mut world);

        let tracer = world.entity(2).expect("tracer expired too early");
        assert_eq!(tracer.pos, start + velocity * TICK_DT);
        assert_eq!(tracer.vel, velocity, "gravity or a wall changed visual velocity");
        assert_eq!(tracer.hit_wall, 0);
        assert!(collect_touches(&world).is_empty(), "visual tracer emitted gameplay touch");
    }
}

#[cfg(test)]
mod lifetime_presentation_tests {
    use super::*;

    #[test]
    fn expiring_static_invalidates_cached_geometry_but_dynamic_does_not() {
        let mut world = GameWorld::new();
        world.push_entity(Entity {
            id: 1,
            kind: BodyKind::Static,
            collide: true,
            life: TICK_DT,
            ..Default::default()
        });
        world.push_entity(Entity {
            id: 2,
            kind: BodyKind::Mover,
            collide: true,
            life: TICK_DT,
            ..Default::default()
        });
        let before = world.render_rev;
        step_world(&mut world);
        assert!(world.entities.is_empty());
        assert_ne!(world.render_rev, before, "expired static stayed in the render slab");

        world.push_entity(Entity {
            id: 3,
            kind: BodyKind::Mover,
            collide: true,
            life: TICK_DT,
            ..Default::default()
        });
        let before = world.render_rev;
        step_world(&mut world);
        assert_eq!(
            world.render_rev, before,
            "dynamic expiration unnecessarily invalidated static presentation"
        );
    }
}
