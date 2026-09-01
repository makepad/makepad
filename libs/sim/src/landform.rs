//! Macro landforms — the AI's big geometry verbs (`game.landform`).
//!
//! One unified destructible world means "raise a mountain" is one op, not a
//! thousand brush strokes. A landform is heightfield-scale terrain surgery:
//! a target-surface shape (mountain / hill / ridge / valley / crater /
//! plateau) with multi-octave noise detail so a mountain reads as one, not a
//! cone. Composition rule, same as the foundation press: the HEIGHTFIELD
//! takes the shape wherever no voxel chunk owns the surface; materialized
//! chunks compose the same shape as voxel min/max
//! ([`VoxelField::compose_surface_targets`]), so raising ground over a
//! dug-open pit fills it to the new surface rather than leaving a punched
//! hole with a mountain painted around it.
//!
//! Ops are idempotent (raise = max, lower = min, plateau = clamp) and ride
//! the voxel op stream ([`VoxelOp::Landform`]). They are recorded on the
//! field ([`VoxelField::land_ops`]) so that:
//! - a script re-eval, which rebuilds the authored heightfield, REPLAYS the
//!   list on top ([`replay_land_ops`]) — the AI's mountains survive reload;
//! - the structure snapshot carries the list to late joiners.
//! Replay is heightfield-only (`voxelize = false`): the chunks persist their
//! own history, and re-composing would refill a tunnel dug through the
//! mountain after it was raised.
//!
//! Determinism: integer-hash lattice noise (same construction as
//! makepad-game-gen's terrain noise), fixed expression order, sqrt only
//! (IEEE-exact) — same op → same heights on every device.

use makepad_math::*;

use crate::terrain::Terrain;
use crate::voxel::{VoxelField, VoxelOp};
use crate::world::GameWorld;

/// Landform ops kept for replay/wire; beyond this they still apply but no
/// longer survive a reload (logged by the host path).
pub const MAX_LAND_OPS: usize = 128;

/// What shape `game.landform` cuts or raises.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LandKind {
    Mountain,
    Hill,
    Ridge,
    Valley,
    Crater,
    Plateau,
}

impl LandKind {
    pub fn parse(name: &str) -> LandKind {
        match name {
            "mountain" | "peak" => LandKind::Mountain,
            "ridge" => LandKind::Ridge,
            "valley" | "basin" | "dip" => LandKind::Valley,
            "crater" => LandKind::Crater,
            "plateau" | "mesa" | "flat" => LandKind::Plateau,
            _ => LandKind::Hill,
        }
    }
    pub fn to_u8(self) -> u8 {
        match self {
            LandKind::Mountain => 0,
            LandKind::Hill => 1,
            LandKind::Ridge => 2,
            LandKind::Valley => 3,
            LandKind::Crater => 4,
            LandKind::Plateau => 5,
        }
    }
    pub fn from_u8(v: u8) -> LandKind {
        match v {
            0 => LandKind::Mountain,
            2 => LandKind::Ridge,
            3 => LandKind::Valley,
            4 => LandKind::Crater,
            5 => LandKind::Plateau,
            _ => LandKind::Hill,
        }
    }
}

// ── noise (same integer-hash construction as gen's terrain noise) ────────

/// One lattice value in 0..1 — integer avalanche, device-identical.
#[inline]
fn lattice(seed: u64, x: i64, z: i64) -> f32 {
    let mut h = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((x as u64).wrapping_mul(0x2545_F491_4F6C_DD1D))
        .wrapping_add((z as u64).wrapping_mul(0x27D4_EB2F_1656_67C5));
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h >> 11) as f32 / (1u64 << 53) as f32
}

#[inline]
fn smoothstep01(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// `floor` without the truncation-toward-zero mirror bug.
#[inline]
fn floor_i64(x: f32) -> i64 {
    let t = x as i64;
    if x < 0.0 && (x - t as f32) != 0.0 {
        t - 1
    } else {
        t
    }
}

/// Bilinear value noise in 0..1.
fn value_noise(seed: u64, x: f32, z: f32) -> f32 {
    let x0 = floor_i64(x);
    let z0 = floor_i64(z);
    let sx = smoothstep01(x - x0 as f32);
    let sz = smoothstep01(z - z0 as f32);
    let h00 = lattice(seed, x0, z0);
    let h10 = lattice(seed, x0 + 1, z0);
    let h01 = lattice(seed, x0, z0 + 1);
    let h11 = lattice(seed, x0 + 1, z0 + 1);
    h00 + (h10 - h00) * sx + (h01 - h00) * sz + (h00 - h10 - h01 + h11) * sx * sz
}

/// 4-octave fbm, normalized to ~0..1. `ridged` 0 = rolling, 1 = sharp
/// crests (the fold that makes a mountain read as rock, not pudding).
fn fbm(seed: u64, x: f32, z: f32, ridged: f32) -> f32 {
    let mut amplitude = 1.0f32;
    let mut total = 0.0f32;
    let mut fx = x;
    let mut fz = z;
    for octave in 0..4u64 {
        let n = value_noise(seed ^ octave.wrapping_mul(0x9E37_79B9), fx, fz);
        let ridge = 1.0 - (n * 2.0 - 1.0).abs();
        total += (n + (ridge - n) * ridged) * amplitude;
        amplitude *= 0.5;
        fx *= 2.0;
        fz *= 2.0;
    }
    total / 1.875
}

// ── the shape ────────────────────────────────────────────────────────────

/// The target-surface heights this landform asks for at world (x, z):
/// `(raise_to, lower_to)`. Raise means `h = max(h, t)`, lower means
/// `h = min(h, t)`, both Some clamps — all idempotent, which is the whole
/// replay/re-delivery story. `(None, None)` outside the shape's support.
pub fn shape_targets(
    kind: LandKind,
    pos: Vec3f,
    r: f32,
    height: f32,
    seed: u32,
    x: f32,
    z: f32,
) -> (Option<f32>, Option<f32>) {
    let seed = seed as u64;
    let dx = x - pos.x;
    let dz = z - pos.z;
    let d = crate::math::sqrt(dx * dx + dz * dz);
    match kind {
        LandKind::Mountain => {
            // Wobbled footprint + ridged detail: the peak wanders, spurs
            // grow, and the silhouette stops being a traffic cone.
            let wob = 0.8 + 0.4 * fbm(seed ^ 0xA5A5, dx * 2.0 / r + 13.7, dz * 2.0 / r - 7.3, 0.0);
            let dn = d / (r * wob);
            if dn >= 1.0 {
                return (None, None);
            }
            let fall = smoothstep01(1.0 - dn);
            let fall = fall * crate::math::sqrt(fall); // ^1.5: steep flanks
            let n = fbm(seed, dx * 4.0 / r, dz * 4.0 / r, 1.0);
            (Some(pos.y + height * fall * (0.55 + 0.45 * n)), None)
        }
        LandKind::Hill => {
            let wob = 0.85 + 0.3 * fbm(seed ^ 0xA5A5, dx * 1.6 / r + 3.1, dz * 1.6 / r + 9.4, 0.0);
            let dn = d / (r * wob);
            if dn >= 1.0 {
                return (None, None);
            }
            let fall = smoothstep01(1.0 - dn);
            let n = fbm(seed, dx * 3.0 / r, dz * 3.0 / r, 0.0);
            (Some(pos.y + height * fall * (0.8 + 0.2 * n)), None)
        }
        LandKind::Ridge => {
            // A crest along a seed-picked axis, tapering toward its ends.
            let (mut ax, mut az) = (lattice(seed, 11, 7) * 2.0 - 1.0, lattice(seed, 3, 29) * 2.0 - 1.0);
            let alen = crate::math::sqrt(ax * ax + az * az);
            if alen < 1.0e-3 {
                (ax, az) = (1.0, 0.0);
            } else {
                ax /= alen;
                az /= alen;
            }
            let half = r * 1.4;
            let along = dx * ax + dz * az;
            let clamped = along.clamp(-half, half);
            let (nx, nz) = (dx - clamped * ax, dz - clamped * az);
            let dseg = crate::math::sqrt(nx * nx + nz * nz);
            let wob = 0.8 + 0.4 * fbm(seed ^ 0x51DE, along * 2.0 / r, 0.0, 0.0);
            let dn = dseg / (r * 0.45 * wob);
            if dn >= 1.0 {
                return (None, None);
            }
            let taper = smoothstep01((1.0 - along.abs() / half).clamp(0.0, 1.0));
            let fall = smoothstep01(1.0 - dn) * taper;
            let n = fbm(seed, dx * 3.0 / r, dz * 3.0 / r, 1.0);
            (Some(pos.y + height * fall * (0.6 + 0.4 * n)), None)
        }
        LandKind::Valley => {
            let wob = 0.85 + 0.3 * fbm(seed ^ 0xA5A5, dx * 1.6 / r - 5.2, dz * 1.6 / r + 2.8, 0.0);
            let dn = d / (r * wob);
            if dn >= 1.0 {
                return (None, None);
            }
            let fall = smoothstep01(1.0 - dn);
            let n = fbm(seed, dx * 3.0 / r, dz * 3.0 / r, 0.0);
            (None, Some(pos.y - height * fall * (0.8 + 0.2 * n)))
        }
        LandKind::Crater => {
            let rb = 0.68 * r;
            if d < rb {
                let t = d / rb;
                (None, Some(pos.y - height * (1.0 - t * t)))
            } else {
                let t = (d - 0.85 * r) / (0.28 * r);
                if t.abs() < 1.0 {
                    let bump = (1.0 - t * t) * (1.0 - t * t);
                    let n = fbm(seed, dx * 5.0 / r, dz * 5.0 / r, 0.0);
                    (Some(pos.y + 0.32 * height * bump * (0.8 + 0.2 * n)), None)
                } else {
                    (None, None)
                }
            }
        }
        LandKind::Plateau => {
            let dn = d / r;
            if dn >= 1.0 {
                return (None, None);
            }
            let t = pos.y + height;
            // Flat core; the ring clamps surrounding ground into a widening
            // band around the top — a feathered edge, still idempotent.
            let allow = ((dn - 0.7).max(0.0) / 0.3) * (height.abs() + 10.0);
            (Some(t - allow), Some(t + allow))
        }
    }
}

/// Full x/z reach of a landform's support (all kinds fit inside this).
pub fn shape_reach(r: f32) -> f32 {
    r * 2.1
}

// ── application ──────────────────────────────────────────────────────────

/// Heightfield part: write the shape into the terrain (idempotent max/min/
/// clamp) and recolor the vertices it moved by slope — rock on the new
/// cliffs, snow on a mountain's crown. Returns whether anything moved.
fn apply_heightfield(
    t: &mut Terrain,
    pos: Vec3f,
    kind: LandKind,
    r: f32,
    height: f32,
    seed: u32,
) -> bool {
    let cells = t.cells;
    if cells < 2 || t.heights.len() < cells * cells {
        return false;
    }
    let cs = t.cell_size.max(1.0e-6);
    let reach = shape_reach(r);
    let gx0 = (((pos.x - reach - t.origin) / cs).floor().max(0.0)) as usize;
    let gz0 = (((pos.z - reach - t.origin) / cs).floor().max(0.0)) as usize;
    let gx1 = ((((pos.x + reach - t.origin) / cs).ceil()).max(0.0) as usize).min(cells - 1);
    let gz1 = ((((pos.z + reach - t.origin) / cs).ceil()).max(0.0) as usize).min(cells - 1);
    if gx0 > gx1 || gz0 > gz1 {
        return false;
    }
    let w = gx1 - gx0 + 1;
    let mut moved = vec![false; w * (gz1 - gz0 + 1)];
    let mut changed = false;
    for gz in gz0..=gz1 {
        for gx in gx0..=gx1 {
            let x = t.origin + gx as f32 * cs;
            let z = t.origin + gz as f32 * cs;
            let (raise, lower) = shape_targets(kind, pos, r, height, seed, x, z);
            let h = &mut t.heights[gz * cells + gx];
            let before = *h;
            if let Some(up) = raise {
                if *h < up {
                    *h = up;
                }
            }
            if let Some(down) = lower {
                if *h > down {
                    *h = down;
                }
            }
            if *h != before {
                changed = true;
                moved[(gz - gz0) * w + (gx - gx0)] = true;
            }
        }
    }
    // Recolor pass: pure function of the FINAL heights (and the seed), so a
    // replay or re-delivery repaints identically instead of drifting.
    if changed && t.colors.len() >= cells * cells {
        for gz in gz0..=gz1 {
            for gx in gx0..=gx1 {
                if !moved[(gz - gz0) * w + (gx - gx0)] {
                    continue;
                }
                let h = |ix: usize, iz: usize| t.heights[iz * cells + ix];
                let xm = gx.saturating_sub(1);
                let xp = (gx + 1).min(cells - 1);
                let zm = gz.saturating_sub(1);
                let zp = (gz + 1).min(cells - 1);
                let dhdx = (h(xp, gz) - h(xm, gz)) / ((xp - xm).max(1) as f32 * cs);
                let dhdz = (h(gx, zp) - h(gx, zm)) / ((zp - zm).max(1) as f32 * cs);
                let slope = crate::math::sqrt(dhdx * dhdx + dhdz * dhdz);
                let j = lattice(seed as u64 ^ 0x77C0, gx as i64, gz as i64) * 0.14 - 0.07;
                let here = t.heights[gz * cells + gx];
                let snowy = kind == LandKind::Mountain && height > 0.0
                    && here > pos.y + 0.72 * height;
                let rockness = ((slope - 0.65) / 0.5).clamp(0.0, 1.0);
                if snowy {
                    t.colors[gz * cells + gx] =
                        vec4f(0.86 + j * 0.5, 0.87 + j * 0.5, 0.92 + j * 0.5, 1.0);
                } else if rockness > 0.35 {
                    t.colors[gz * cells + gx] =
                        vec4f(0.47 + j, 0.44 + j, 0.42 + j, 1.0);
                }
            }
        }
    }
    changed
}

/// Apply one [`VoxelOp::Landform`] to the world: heightfield always,
/// voxel composition into materialized chunks only when `voxelize` (first
/// application — a replay must not refill tunnels dug after the landform).
pub fn apply_landform_op(world: &mut GameWorld, op: VoxelOp, voxelize: bool) {
    let VoxelOp::Landform { pos, kind, r, height, seed } = op else {
        return;
    };
    let kind = LandKind::from_u8(kind);
    let Some(terrain) = world.terrain.as_mut() else {
        world.log("game.landform: needs a game.terrain heightfield first".to_string());
        return;
    };
    if apply_heightfield(terrain, pos, kind, r, height, seed) {
        terrain.revision += 1;
        world.mark_render_dirty();
    }
    if voxelize {
        let GameWorld { voxel, terrain, log_pending, .. } = world;
        if let Some(field) = voxel.as_deref_mut() {
            if field.chunk_count() > 0 {
                let reach = shape_reach(r);
                let band = height.abs() * 1.05 + field.cell * 2.0;
                field.compose_surface_targets(
                    pos.x - reach,
                    pos.x + reach,
                    pos.z - reach,
                    pos.z + reach,
                    pos.y - band,
                    pos.y + band,
                    terrain.as_ref(),
                    log_pending,
                    &mut |x, z| shape_targets(kind, pos, r, height, seed, x, z),
                );
            }
        }
    }
}

/// Two landform ops are THE SAME LANDFORM when everything but the sampled
/// base height matches. The verb samples its base from the live composed
/// surface — which the landform itself raises — so a re-eval re-running the
/// same script line arrives with a HIGHER pos.y. Matching on it would
/// re-record the op and stack the mountain on its own peak, +height per
/// eval (the floating-curtain ratchet). Identity is (x, z, kind, r,
/// height, seed); the recorded op keeps its first-sample base forever.
fn same_landform(a: &VoxelOp, b: &VoxelOp) -> bool {
    match (a, b) {
        (
            VoxelOp::Landform { pos: pa, kind: ka, r: ra, height: ha, seed: sa },
            VoxelOp::Landform { pos: pb, kind: kb, r: rb, height: hb, seed: sb },
        ) => {
            pa.x.to_bits() == pb.x.to_bits()
                && pa.z.to_bits() == pb.z.to_bits()
                && ka == kb
                && ra.to_bits() == rb.to_bits()
                && ha.to_bits() == hb.to_bits()
                && sa == sb
        }
        _ => false,
    }
}

/// The authority path (verb layer / host): record for replay + replication,
/// then apply. A re-delivered identical op (a script re-eval re-running its
/// own landform line) applies heightfield-only — the chunks already carry
/// its voxel effect plus everything dug since.
pub fn host_apply_landform(world: &mut GameWorld, op: VoxelOp) {
    let mut overflow = false;
    let field = world
        .voxel
        .get_or_insert_with(|| Box::new(VoxelField::new(0.5)));
    let recorded = field.land_ops.iter().find(|k| same_landform(k, &op)).copied();
    let known = recorded.is_some();
    if !known {
        if field.land_ops.len() < MAX_LAND_OPS {
            field.land_ops.push(op);
            // The list rides the structure snapshot: bump so it rebroadcasts.
            field.structure_rev += 1;
        } else {
            overflow = true;
        }
        if field.pending_ops.len() < 65536 {
            field.pending_ops.push(op);
        }
        if field.persist_ops.len() < 8192 {
            field.persist_ops.push(op);
        } else {
            field.persist_overflow = true;
            field.persist_ops.clear();
        }
    }
    if overflow {
        world.log(format!(
            "game.landform: more than {MAX_LAND_OPS} landforms — this one applies \
             but will not survive a reload"
        ));
    }
    // A known landform re-applies as its RECORDED twin (first-sample base):
    // the incoming copy's base was sampled from ground the landform itself
    // already raised, and applying that would still ratchet the heights.
    apply_landform_op(world, recorded.unwrap_or(op), !known);
}

/// The replica path (wire op): apply with voxel composition (this device's
/// chunks mirror the host's op application) and remember for ITS reloads —
/// never re-replicate.
pub fn wire_apply_landform(world: &mut GameWorld, op: VoxelOp) {
    let field = world
        .voxel
        .get_or_insert_with(|| Box::new(VoxelField::new(0.5)));
    let recorded = field.land_ops.iter().find(|k| same_landform(k, &op)).copied();
    let known = recorded.is_some();
    if !known && field.land_ops.len() < MAX_LAND_OPS {
        field.land_ops.push(op);
    }
    apply_landform_op(world, recorded.unwrap_or(op), !known);
}

/// Re-apply the recorded landform list onto a freshly rebuilt heightfield
/// (heightfield-only — see module doc). Runs once per pending flag, waiting
/// until a terrain exists; called every tick from `update_world_voxel`.
pub fn replay_land_ops(world: &mut GameWorld) {
    let Some(field) = world.voxel.as_deref() else {
        return;
    };
    if !field.land_replay_pending {
        return;
    }
    if field.land_ops.is_empty() {
        if let Some(f) = world.voxel.as_deref_mut() {
            f.land_replay_pending = false;
        }
        return;
    }
    if world.terrain.is_none() {
        return; // eval not there yet — retry next tick
    }
    let ops: Vec<VoxelOp> = field.land_ops.clone();
    if let Some(f) = world.voxel.as_deref_mut() {
        f.land_replay_pending = false;
    }
    for op in ops {
        apply_landform_op(world, op, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_world() -> GameWorld {
        let cells = 65;
        let mut w = GameWorld::new();
        w.terrain = Some(Terrain {
            cells,
            cell_size: 2.0,
            origin: -64.0,
            heights: vec![0.0; cells * cells],
            colors: vec![vec4f(0.4, 0.6, 0.4, 1.0); cells * cells],
            revision: 1,
        });
        w
    }

    fn mountain_op() -> VoxelOp {
        VoxelOp::Landform {
            pos: vec3f(0.0, 0.0, 0.0),
            kind: LandKind::Mountain.to_u8(),
            r: 30.0,
            height: 18.0,
            seed: 7,
        }
    }

    #[test]
    fn mountain_raises_idempotently_and_survives_replay() {
        let mut w = flat_world();
        host_apply_landform(&mut w, mountain_op());
        let peak = w.terrain.as_ref().unwrap().height_at(0.0, 0.0).unwrap();
        assert!(peak > 8.0, "mountain peak only {peak}");
        let snapshot = w.terrain.as_ref().unwrap().heights.clone();
        // Re-running the same script line must not double-raise.
        host_apply_landform(&mut w, mountain_op());
        assert_eq!(w.terrain.as_ref().unwrap().heights, snapshot);
        // Reload: heightfield rebuilt flat, replay restores the mountain.
        let n = w.terrain.as_ref().unwrap().heights.len();
        w.terrain.as_mut().unwrap().heights = vec![0.0; n];
        w.voxel.as_deref_mut().unwrap().land_replay_pending = true;
        replay_land_ops(&mut w);
        assert_eq!(w.terrain.as_ref().unwrap().heights, snapshot);
    }

    #[test]
    fn a_reeval_resampling_its_own_mountain_does_not_ratchet() {
        // The verb samples its base from the live surface; on a re-eval the
        // same script line therefore arrives with pos.y on the mountain's
        // own peak. It must be recognized as THE SAME landform and re-apply
        // with its recorded base — not stack +height per eval.
        let mut w = flat_world();
        host_apply_landform(&mut w, mountain_op());
        let snapshot = w.terrain.as_ref().unwrap().heights.clone();
        let peak = w.surface_height_at(0.0, 0.0).unwrap();
        assert!(peak > 8.0);
        let VoxelOp::Landform { kind, r, height, seed, .. } = mountain_op() else {
            unreachable!()
        };
        host_apply_landform(
            &mut w,
            VoxelOp::Landform { pos: vec3f(0.0, peak, 0.0), kind, r, height, seed },
        );
        assert_eq!(
            w.terrain.as_ref().unwrap().heights,
            snapshot,
            "re-sampled base ratcheted the mountain"
        );
        assert_eq!(w.voxel.as_deref().unwrap().land_ops.len(), 1, "op re-recorded");
    }

    #[test]
    fn landform_is_deterministic() {
        let mut a = flat_world();
        let mut b = flat_world();
        host_apply_landform(&mut a, mountain_op());
        host_apply_landform(&mut b, mountain_op());
        let ha = &a.terrain.as_ref().unwrap().heights;
        let hb = &b.terrain.as_ref().unwrap().heights;
        assert!(ha.iter().zip(hb.iter()).all(|(x, y)| x.to_bits() == y.to_bits()));
    }

    #[test]
    fn crater_digs_a_bowl_with_a_rim() {
        let mut w = flat_world();
        host_apply_landform(
            &mut w,
            VoxelOp::Landform {
                pos: vec3f(0.0, 0.0, 0.0),
                kind: LandKind::Crater.to_u8(),
                r: 20.0,
                height: 6.0,
                seed: 3,
            },
        );
        let t = w.terrain.as_ref().unwrap();
        let center = t.height_at(0.0, 0.0).unwrap();
        let rim = t.height_at(17.0, 0.0).unwrap();
        let outside = t.height_at(50.0, 0.0).unwrap();
        assert!(center < -4.0, "bowl centre {center}");
        assert!(rim > 0.5, "rim {rim}");
        assert!(outside.abs() < 1.0e-6, "outside moved: {outside}");
    }

    #[test]
    fn landform_composes_into_materialized_chunks() {
        // Dig a pit first (chunks own the surface there), then raise a
        // mountain over it: the voxel surface must rise with the ground.
        let mut w = flat_world();
        w.apply_voxel_op(VoxelOp::Dig {
            pos: vec3f(0.0, 0.0, 0.0),
            r: 4.0,
            mode: crate::voxel::DigMode::Carve,
            material: 1,
        });
        let field = w.voxel.as_deref().unwrap();
        let pit = field.surface_at(0.0, 0.0, 0.0).expect("pit does not own surface");
        assert!(pit < -1.0, "pit floor {pit}");
        host_apply_landform(&mut w, mountain_op());
        let field = w.voxel.as_deref().unwrap();
        let composed = field
            .surface_at(0.0, 0.0, 0.0)
            .expect("voxel no longer owns the raised surface");
        assert!(
            composed > 6.0,
            "voxel surface did not rise with the mountain: {composed}"
        );
        // And the seam agrees.
        let seam = w.surface_height_at(0.0, 0.0).unwrap();
        assert!((seam - composed).abs() < 1.0e-4, "seam {seam} != voxel {composed}");
    }
}
