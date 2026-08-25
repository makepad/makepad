//! Procedural buildings, so the property tests have something to be right
//! about without shipping a 76 MB `.fab` into the test suite.
//!
//! A BSP split of a rectangle gives the rooms; then every pair of rooms that
//! share an edge gets a wall with a doorway in it. That construction makes the
//! room graph **connected**, so "every room is reachable" is true of the
//! fixture and any room the planner misses is the planner's fault — except
//! when [`Plan::seal_rooms`] deliberately bricks a doorway up, which is how
//! the unreachable-room reporting gets tested.
//!
//! Doors are placed from leaf *adjacency*, not per BSP split, and the
//! difference matters: a split line between two halves is subdivided by later
//! splits, so one door per split leaves most neighbouring pairs with a solid
//! wall between them and most of the house unreachable.

use crate::scene::{TourClass, TourScene, TourSceneBuilder};
use makepad_math::{vec3, Vec3f};

const WALL_T: f32 = 0.20;
const DOOR_W: f32 = 0.95;
const DOOR_H: f32 = 2.05;
const STOREY_H: f32 = 2.90;
const SLAB_T: f32 = 0.30;

/// Deterministic xorshift64*, so a failing seed reproduces exactly.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E3779B97F4A7C15).max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn f32(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    fn h(&self) -> f32 {
        self.y1 - self.y0
    }
    fn area(&self) -> f32 {
        self.w() * self.h()
    }
}

/// What to build.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub seed: u64,
    pub width: f32,
    pub depth: f32,
    pub storeys: usize,
    /// Stop splitting below this floor area.
    pub min_room_area: f32,
    /// Brick up this many doorways, creating unreachable rooms on purpose.
    pub seal_rooms: usize,
}

impl Default for Plan {
    fn default() -> Self {
        Plan {
            seed: 1,
            width: 14.0,
            depth: 10.0,
            storeys: 2,
            min_room_area: 11.0,
            seal_rooms: 0,
        }
    }
}

struct Build<'a> {
    b: TourSceneBuilder,
    rng: Rng,
    plan: &'a Plan,
    n_walls: usize,
    n_doors: usize,
    sealed: usize,
}

impl Build<'_> {
    fn wall_box(&mut self, min: Vec3f, max: Vec3f, storey: usize) {
        if max.x - min.x <= 1e-3 || max.y - min.y <= 1e-3 || max.z - min.z <= 1e-3 {
            return;
        }
        self.n_walls += 1;
        let name = format!("WAL-{:03}", self.n_walls);
        self.b.element(&name, TourClass::Wall, storey);
        self.b.box_solid(min, max);
    }

    /// A wall segment with one doorway in it. `along` is the axis the wall
    /// runs along (0 = x, 1 = y); `at` is the other coordinate.
    fn wall_with_door(
        &mut self,
        along: usize,
        at: f32,
        a: f32,
        b: f32,
        base: f32,
        storey: usize,
        seal: bool,
    ) {
        let len = b - a;
        if len < DOOR_W + 0.6 {
            // Too short for a door: solid.
            self.seg(along, at, a, b, base, base + STOREY_H, storey);
            return;
        }
        let d0 = self.rng.range(a + 0.3, b - DOOR_W - 0.3);
        let d1 = d0 + DOOR_W;
        self.seg(along, at, a, d0, base, base + STOREY_H, storey);
        self.seg(along, at, d1, b, base, base + STOREY_H, storey);
        // Lintel over the opening.
        self.seg(along, at, d0, d1, base + DOOR_H, base + STOREY_H, storey);
        if seal {
            // A bricked-up doorway: no door element, solid wall instead.
            self.seg(along, at, d0, d1, base, base + DOOR_H, storey);
            self.sealed += 1;
            return;
        }
        // The door leaf itself: thin, so it seals the room graph without
        // blocking navigation (see `TourClass::blocks_navigation`).
        self.n_doors += 1;
        let name = format!("DOR-{:03}", self.n_doors);
        self.b.element(&name, TourClass::Door, storey);
        let t = 0.03;
        let (min, max) = if along == 0 {
            (vec3(d0, at - t, base), vec3(d1, at + t, base + DOOR_H))
        } else {
            (vec3(at - t, d0, base), vec3(at + t, d1, base + DOOR_H))
        };
        self.b.box_solid(min, max);
    }

    fn seg(&mut self, along: usize, at: f32, a: f32, b: f32, z0: f32, z1: f32, storey: usize) {
        if b - a <= 1e-3 {
            return;
        }
        let h = WALL_T * 0.5;
        let (min, max) = if along == 0 {
            (vec3(a, at - h, z0), vec3(b, at + h, z1))
        } else {
            (vec3(at - h, a, z0), vec3(at + h, b, z1))
        };
        self.wall_box(min, max, storey);
    }

    /// One wall, with one doorway, for every pair of leaves that share an
    /// edge. This is what makes the room graph connected: every pair of
    /// neighbouring rooms gets its own door, so a room can only be isolated
    /// when `Plan::seal_rooms` deliberately bricks one up.
    fn walls_from_adjacency(
        &mut self,
        leaves: &[Rect],
        base: f32,
        storey: usize,
        hole: Option<Rect>,
    ) {
        let n = leaves.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let (a, b) = (leaves[i], leaves[j]);
                // Shared vertical edge?
                let vert = if (a.x1 - b.x0).abs() < 1e-3 {
                    Some(a.x1)
                } else if (b.x1 - a.x0).abs() < 1e-3 {
                    Some(b.x1)
                } else {
                    None
                };
                if let Some(x) = vert {
                    let lo = a.y0.max(b.y0);
                    let hi = a.y1.min(b.y1);
                    if hi - lo > 0.4 {
                        for (a2, b2) in clip_out(lo, hi, hole.map(|h| (h.x0, h.x1, h.y0, h.y1)), x, true) {
                            let seal = self.sealed < self.plan.seal_rooms;
                            self.wall_with_door(1, x, a2, b2, base, storey, seal);
                        }
                    }
                    continue;
                }
                let horiz = if (a.y1 - b.y0).abs() < 1e-3 {
                    Some(a.y1)
                } else if (b.y1 - a.y0).abs() < 1e-3 {
                    Some(b.y1)
                } else {
                    None
                };
                if let Some(y) = horiz {
                    let lo = a.x0.max(b.x0);
                    let hi = a.x1.min(b.x1);
                    if hi - lo > 0.4 {
                        for (a2, b2) in clip_out(lo, hi, hole.map(|h| (h.x0, h.x1, h.y0, h.y1)), y, false) {
                            let seal = self.sealed < self.plan.seal_rooms;
                            self.wall_with_door(0, y, a2, b2, base, storey, seal);
                        }
                    }
                }
            }
        }
    }

    /// Recursive BSP producing the leaf rectangles. Walls are *not* emitted
    /// here: a split line between two halves gets subdivided by later splits,
    /// so putting one door per split leaves most room pairs with a solid wall
    /// between them and the "graph is a tree" promise is a lie. Walls come
    /// afterwards, from leaf adjacency — see [`Build::walls_from_adjacency`].
    fn split(&mut self, r: Rect, base: f32, storey: usize, depth: usize, out: &mut Vec<Rect>) {
        let can = r.area() > self.plan.min_room_area * 2.0 && depth < 5;
        if !can {
            out.push(r);
            return;
        }
        // Split the long way, with a jittered position.
        let vertical = if r.w() > r.h() * 1.25 {
            true
        } else if r.h() > r.w() * 1.25 {
            false
        } else {
            self.rng.f32() < 0.5
        };
        let f = self.rng.range(0.38, 0.62);
        if vertical {
            let xs = r.x0 + r.w() * f;
            if (xs - r.x0).min(r.x1 - xs) < 2.0 {
                out.push(r);
                return;
            }
            self.split(
                Rect { x1: xs, ..r },
                base,
                storey,
                depth + 1,
                out,
            );
            self.split(
                Rect { x0: xs, ..r },
                base,
                storey,
                depth + 1,
                out,
            );
        } else {
            let ys = r.y0 + r.h() * f;
            if (ys - r.y0).min(r.y1 - ys) < 2.0 {
                out.push(r);
                return;
            }
            self.split(
                Rect { y1: ys, ..r },
                base,
                storey,
                depth + 1,
                out,
            );
            self.split(
                Rect { y0: ys, ..r },
                base,
                storey,
                depth + 1,
                out,
            );
        }
    }
}

/// Split `[a, b]` around a rectangular hole, returning the parts that survive.
/// `at` is the wall's fixed coordinate; `vertical` means the wall runs along y.
fn clip_out(
    a: f32,
    b: f32,
    hole: Option<(f32, f32, f32, f32)>,
    at: f32,
    vertical: bool,
) -> Vec<(f32, f32)> {
    let Some((hx0, hx1, hy0, hy1)) = hole else {
        return vec![(a, b)];
    };
    // Does this wall cross the hole at all?
    let (cross, h0, h1) = if vertical {
        (at > hx0 && at < hx1, hy0, hy1)
    } else {
        (at > hy0 && at < hy1, hx0, hx1)
    };
    if !cross || h1 <= a || h0 >= b {
        return vec![(a, b)];
    }
    let mut out = Vec::new();
    if h0 - a > 0.4 {
        out.push((a, h0));
    }
    if b - h1 > 0.4 {
        out.push((h1, b));
    }
    out
}

/// Build a house. Deterministic in `plan.seed`.
pub fn building(plan: &Plan) -> TourScene {
    let mut bd = Build {
        b: TourSceneBuilder::new("Synthetic house"),
        rng: Rng::new(plan.seed),
        plan,
        n_walls: 0,
        n_doors: 0,
        sealed: 0,
    };
    let (w, d) = (plan.width, plan.depth);
    for s in 0..plan.storeys {
        bd.b.storey(&format!("Level {s}"), s as f32 * STOREY_H, STOREY_H);
    }
    // Ground under everything.
    bd.b.element("SITE", TourClass::Site, 0);
    bd.b.box_solid(
        vec3(-12.0, -12.0, -0.6 - SLAB_T),
        vec3(w + 12.0, d + 12.0, -0.6),
    );

    // Stairwell footprint, reused on every floor.
    let stair = Rect {
        x0: w - 3.6,
        y0: 0.4,
        x1: w - 0.6,
        y1: 3.4,
    };

    for s in 0..plan.storeys {
        let base = s as f32 * STOREY_H;

        // Floor slab, with a hole over the stair for every floor above the
        // ground one.
        bd.b.element(&format!("SLB-{s}"), TourClass::Slab, s);
        if s == 0 {
            bd.b
                .box_solid(vec3(0.0, 0.0, base - SLAB_T), vec3(w, d, base));
        } else {
            // Four bands around the stairwell opening.
            let (z0, z1) = (base - SLAB_T, base);
            bd.b.box_solid(vec3(0.0, 0.0, z0), vec3(w, stair.y0, z1));
            bd.b.box_solid(vec3(0.0, stair.y1, z0), vec3(w, d, z1));
            bd.b
                .box_solid(vec3(0.0, stair.y0, z0), vec3(stair.x0, stair.y1, z1));
            bd.b
                .box_solid(vec3(stair.x1, stair.y0, z0), vec3(w, stair.y1, z1));
        }

        // Partition the plan first: the front door has to be placed where
        // there is actually floor behind it. A door opening onto a wall 50 mm
        // away is not an entrance, and the analyser is right to refuse it.
        let mut leaves = Vec::new();
        let usable = Rect {
            x0: 0.2,
            y0: 0.2,
            x1: w - 0.2,
            y1: d - 0.2,
        };
        bd.split(usable, base, s, 0, &mut leaves);

        // Exterior envelope.
        let top = base + STOREY_H;
        bd.b.element(&format!("EXT-N-{s}"), TourClass::Wall, s);
        bd.b.box_solid(vec3(-WALL_T, d, base), vec3(w + WALL_T, d + WALL_T, top));
        bd.b.element(&format!("EXT-E-{s}"), TourClass::Wall, s);
        bd.b.box_solid(vec3(w, -WALL_T, base), vec3(w + WALL_T, d + WALL_T, top));
        bd.b.element(&format!("EXT-W-{s}"), TourClass::Wall, s);
        bd.b.box_solid(vec3(-WALL_T, -WALL_T, base), vec3(0.0, d + WALL_T, top));

        // South wall carries the front door on the ground floor.
        if s == 0 {
            // Widest room on the south edge that is not the stairwell: a
            // front door opening straight onto a flight of stairs is not a
            // front door, and the analyser correctly finds no way in.
            let clear_of_stair = |r: &Rect| {
                plan.storeys < 2 || r.x1 <= stair.x0 + 0.1 || r.x0 >= stair.x1 - 0.1
            };
            let front = leaves
                .iter()
                .filter(|r| r.y0 < 0.5 && clear_of_stair(r))
                .max_by(|a, b| a.w().partial_cmp(&b.w()).unwrap_or(std::cmp::Ordering::Equal))
                .or_else(|| {
                    leaves
                        .iter()
                        .filter(|r| r.y0 < 0.5)
                        .max_by(|a, b| a.w().partial_cmp(&b.w()).unwrap_or(std::cmp::Ordering::Equal))
                })
                .copied()
                .unwrap_or(Rect { x0: 0.0, y0: 0.0, x1: w, y1: d });
            let dx = ((front.x0 + front.x1) * 0.5 - DOOR_W * 0.5)
                .clamp(0.4, w - DOOR_W - 0.4);
            bd.b.element("EXT-S-0a", TourClass::Wall, s);
            bd.b.box_solid(vec3(-WALL_T, -WALL_T, base), vec3(dx, 0.0, top));
            bd.b.element("EXT-S-0b", TourClass::Wall, s);
            bd.b
                .box_solid(vec3(dx + DOOR_W, -WALL_T, base), vec3(w + WALL_T, 0.0, top));
            bd.b.element("EXT-S-0c", TourClass::Wall, s);
            bd.b.box_solid(
                vec3(dx, -WALL_T, base + DOOR_H),
                vec3(dx + DOOR_W, 0.0, top),
            );
            bd.b.element("DOR-FRONT", TourClass::Door, s);
            bd.b.box_solid(
                vec3(dx, -0.03, base),
                vec3(dx + DOOR_W, 0.03, base + DOOR_H),
            );
        } else {
            bd.b.element(&format!("EXT-S-{s}"), TourClass::Wall, s);
            bd.b.box_solid(vec3(-WALL_T, -WALL_T, base), vec3(w + WALL_T, 0.0, top));
        }

        // A window per façade, so the POI scorer has glass to find.
        for (k, (ax, at, a0, a1)) in [
            (0usize, d, w * 0.2, w * 0.2 + 1.8),
            (0, d, w * 0.65, w * 0.65 + 1.8),
            (1, w, d * 0.3, d * 0.3 + 1.6),
            (1, 0.0, d * 0.55, d * 0.55 + 1.6),
        ]
        .iter()
        .enumerate()
        {
            bd.b.element(&format!("WDW-{s}-{k}"), TourClass::Window, s);
            let (z0, z1) = (base + 0.9, base + 2.2);
            let (min, max) = if *ax == 0 {
                (vec3(*a0, *at - 0.12, z0), vec3(*a1, *at + 0.12, z1))
            } else {
                (vec3(*at - 0.12, *a0, z0), vec3(*at + 0.12, *a1, z1))
            };
            bd.b.box_solid(min, max);
            // Cut the hole: the envelope above was solid, so punch a lintel
            // gap by adding nothing — the window box sits proud of the wall
            // and the wall behind it is what blocks. For the tour this is
            // enough: glazing area is what the scorer reads.
        }

        // Above the ground floor the stairwell is a void; partitions must not
        // span it. (A wall hanging in mid-air over the stairs is nonsense the
        // planner would rightly refuse to fly through.)
        bd.walls_from_adjacency(&leaves, base, s, if s > 0 { Some(stair) } else { None });

        // Stairs, and the guard wall around the well.
        if plan.storeys > 1 && s + 1 < plan.storeys {
            bd.b.element(&format!("STR-{s}"), TourClass::Stair, s);
            let steps = 16;
            for i in 0..steps {
                let f0 = i as f32 / steps as f32;
                let f1 = (i + 1) as f32 / steps as f32;
                // The last tread runs past the well edge and under the slab
                // above, so the top of the flight and the floor it arrives on
                // are one connected piece of walkable ground. Leave even a
                // single cell of gap and the whole upper storey is marooned.
                let y0 = stair.y0 + 0.15 + (stair.y1 - stair.y0 - 0.3) * f0;
                let mut y1 = stair.y0 + 0.15 + (stair.y1 - stair.y0 - 0.3) * f1;
                if i + 1 == steps {
                    y1 = stair.y1 + 0.9;
                }
                bd.b.box_solid(
                    vec3(stair.x0 + 0.15, y0, base),
                    vec3(stair.x1 - 0.15, y1, base + STOREY_H * f1),
                );
            }
        }

        // Ceiling for the top storey.
        if s + 1 == plan.storeys {
            bd.b.element("ROOF", TourClass::Roof, s);
            bd.b.box_solid(
                vec3(-0.4, -0.4, top),
                vec3(w + 0.4, d + 0.4, top + SLAB_T),
            );
        }

        // Name the rooms with zones, the way source application would.
        for (i, r) in leaves.iter().enumerate() {
            bd.b.element(&format!("Room {}-{}", s, i + 1), TourClass::Zone, s);
            bd.b.box_solid(
                vec3(r.x0 + 0.3, r.y0 + 0.3, base + 0.05),
                vec3(r.x1 - 0.3, r.y1 - 0.3, base + 0.15),
            );
        }
    }

    bd.b.finish()
}

/// The standard fixture: a two-storey house with a stair.
pub fn villa() -> TourScene {
    building(&Plan::default())
}

/// A single-storey building with `seal` doorways bricked up.
pub fn with_unreachable(seed: u64, seal: usize) -> TourScene {
    building(&Plan {
        seed,
        storeys: 1,
        seal_rooms: seal,
        ..Default::default()
    })
}
