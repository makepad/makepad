//! Reading a building: storeys, walkable ground, rooms, the graph that
//! connects them, and what is worth pointing a camera at.
//!
//! # How rooms are found
//!
//! Not by flood-filling free space — that gives you one blob per floor as soon
//! as a single door stands open, and open-plan buildings have no doors at all.
//! Instead a **watershed on the clearance field**, which is what an architect
//! would draw:
//!
//! 1. Take the plan-view clearance (metres to the nearest blocking column)
//!    computed with door leaves *shut*, so a closed door is a wall.
//! 2. Cells with more than [`AnalysisConfig::core_radius`] of room are *open
//!    ground*. Its connected components are room cores. A doorway is 0.9 m
//!    wide → 0.45 m of clearance → never a core. A corridor at 1.4 m is.
//! 3. Grow every core outward at once. The fronts meet in the middle of each
//!    doorway, and that meeting line is the room boundary — and, because the
//!    two rooms' cells now touch, it is also the portal between them.
//! 4. Walkable ground no front ever reached has no core anywhere along it:
//!    that is circulation, and its components become corridor rooms.
//! 5. Two rooms whose cells touch share a **portal**; its centre is the widest
//!    point of the shared boundary, which is the middle of the doorway.
//!
//! The result survives open doors, missing doors and open-plan floors, and it
//! splits a living/kitchen volume at the pinch where a person would say the
//! rooms divide.
//!
//! # What makes a shot worth taking
//!
//! [`Room::score`] is a weighted sum, tuned on the two sample buildings and
//! written out here so it can be argued with:
//!
//! | term | weight | why |
//! |---|---|---|
//! | `sqrt(area / 10)` | 1.0 | big rooms read better on camera; sublinear so a hall does not eclipse everything |
//! | `glazing / 6` | 1.2 | glass is light and a view out — the strongest single predictor of a good interior shot |
//! | `ceiling - 2.4` | 0.6 | double-height space is drama |
//! | stairs present | 0.8 | a staircase is the one piece of architecture that photographs itself |
//! | `portals / 3` | 0.5 | circulation hubs are where a building explains itself |
//! | `sightline / 12` | 0.4 | a long clear axis is a shot; a cupboard is not |
//!
//! Exterior-facing façades are ranked the same way over glazing area, triangle
//! density (detail) and how far the camera can back off.

use crate::geom::*;
use crate::scene::{TourClass, TourScene};
use crate::voxel::{edt_2d, VoxelConfig, VoxelGrid};
use makepad_math::{vec3, Aabb, Vec3f};
use std::collections::VecDeque;

/// The body the planner plans for.
///
/// `radius` is the **passability** radius — can this body get through at all —
/// and it is deliberately smaller than the clearance a camera *prefers*. A
/// person is about 0.4 m wide across the shoulders but walks through a 0.8 m
/// door every day by not being a cylinder, and a lattice quantised at 0.15 m
/// with a half-cell safety margin turns a real 0.95 m doorway into 0.40 m of
/// measured clearance. Planning at 0.4 m therefore declares every interior
/// door impassable and the building comes out as a set of disconnected rooms.
///
/// So there are three numbers, and they are different on purpose:
///
/// | number | value | means |
/// |---|---|---|
/// | [`Body::radius`] | 0.30 m | will the body fit through |
/// | [`crate::MotionProfile::clearance`] | 0.32–0.50 m | where the camera would *like* to sit |
/// | [`crate::QaLimits::min_clearance`] | 0.10 m | hard safety: never inside geometry |
#[derive(Clone, Copy, Debug)]
pub struct Body {
    /// Horizontal half-width for passability. Clearance never drops below this
    /// on walkable ground.
    pub radius: f32,
    /// Total height of the volume that must stay clear.
    pub height: f32,
    /// Eye height above the floor.
    pub eye: f32,
    /// Thresholds and sills up to this are stepped over, not walked into.
    pub step_up: f32,
}

impl Body {
    /// A person: 1.7 m tall, eyes at 1.6 m, 0.30 m passability radius.
    pub fn walker() -> Body {
        Body {
            radius: 0.30,
            height: 1.70,
            eye: 1.60,
            step_up: 0.20,
        }
    }

    /// A drone: a sphere with `clearance` metres of air around it.
    pub fn drone(clearance: f32) -> Body {
        Body {
            radius: clearance,
            height: clearance * 2.0,
            eye: clearance,
            step_up: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AnalysisConfig {
    pub voxel: VoxelConfig,
    pub body: Body,
    /// Clearance above which ground counts as an open room core. A 0.9 m
    /// doorway gives 0.45 m and is never a core, which is what makes doorways
    /// the watershed lines between rooms.
    pub core_radius: f32,
    /// A room whose most open point is still closer to a wall than this is
    /// circulation rather than a room, and is scored down accordingly.
    pub corridor_width: f32,
    /// Cores below this floor area are noise, not rooms.
    pub min_room_area: f32,
    /// Corridor components below this are noise.
    pub min_corridor_area: f32,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        AnalysisConfig {
            voxel: VoxelConfig::default(),
            body: Body::walker(),
            core_radius: 0.42,
            corridor_width: 1.05,
            min_room_area: 2.5,
            min_corridor_area: 1.0,
        }
    }
}

/// One habitable level, resolved into a plan-view lattice.
pub struct StoreyPlan {
    pub index: usize,
    pub name: String,
    pub elevation: f32,
    pub height: f32,
    /// World Z the body's feet sit at.
    pub floor_z: f32,
    pub eye_z: f32,
    pub nx: usize,
    pub ny: usize,
    /// A body of the configured radius fits, standing on real floor.
    /// Exactly `clearance[i] >= body.radius` — see [`SiteAnalysis::clearance`].
    pub walkable: Vec<bool>,
    /// **The** walking clearance: metres to the nearest thing that stops a
    /// body, whether that is a wall beside it or the edge of the floor under
    /// it. `min(distance to a blocked column, distance to a hole in the
    /// floor)`, so a balcony edge is as real an obstacle as a wall and the
    /// field stays continuous across both.
    pub clearance: Vec<f32>,
    /// Metres to the nearest column with doors *shut* — the watershed input.
    pub sealed_clearance: Vec<f32>,
    pub interior: Vec<bool>,
    /// Global room id per cell, or `u32::MAX`.
    pub room_of: Vec<u32>,
    pub rooms: Vec<usize>,
}

impl StoreyPlan {
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> usize {
        y * self.nx + x
    }

    pub fn walkable_area(&self, cell: f32) -> f32 {
        self.walkable.iter().filter(|w| **w).count() as f32 * cell * cell
    }
}

#[derive(Clone, Debug)]
pub struct Room {
    pub id: usize,
    pub storey: usize,
    pub name: String,
    pub area: f32,
    /// Cell indices into the storey lattice.
    pub cells: Vec<u32>,
    /// The most open standing point, at eye height. Where the camera pauses.
    pub center: Vec3f,
    pub bounds: Aabb,
    pub interior: bool,
    pub corridor: bool,
    /// Square metres of glazing in this room's walls.
    pub glazing: f32,
    pub ceiling: f32,
    /// Longest clear sight line from `center`, metres.
    pub sightline: f32,
    /// Where to look from `center` for the best shot.
    pub best_view: Vec3f,
    pub score: f32,
}

/// A doorway or opening between two rooms.
#[derive(Clone, Debug)]
pub struct Portal {
    pub a: usize,
    pub b: usize,
    /// Middle of the opening, at eye height. Paths are pinned through this.
    pub center: Vec3f,
    pub width: f32,
    /// The door/opening element, when one lines up with the gap.
    pub element: Option<usize>,
}

/// A door-sized hole in a wall, derived from free-space rather than from a
/// typed Door element. Legacy files have no door records: an opening is a
/// corridor of free voxels that crosses a wall's footprint, at least 1.9 m
/// high and 0.7–1.5 m wide, at floor level.
#[derive(Clone, Debug)]
pub struct Opening {
    pub center: Vec3f,
    pub width: f32,
    pub height: f32,
    pub storey: usize,
    /// Horizontal unit vector through the wall (the way you walk).
    pub through: Vec3f,
    /// Storey-lattice cells that make up the gap. Used to keep the watershed
    /// from treating a doorway as open ground (which would merge the rooms).
    pub cells: Vec<u32>,
}

/// A vertical connection.
#[derive(Clone, Debug)]
pub struct StairLink {
    pub lower_room: usize,
    pub upper_room: usize,
    /// Walkable interior ground at the foot, on the lower storey. Where the
    /// route *arrives* before climbing.
    pub bottom: Vec3f,
    /// Walkable interior ground at the head, on the upper storey.
    pub top: Vec3f,
    /// The climb itself, on the flight's own centre line. Going straight from
    /// `bottom` to `top` is a chord across the stairwell that happily crosses
    /// whatever partitions stand between them.
    pub run_low: Vec3f,
    pub run_high: Vec3f,
    pub element: usize,
}

/// A way in from outside.
#[derive(Clone, Debug)]
pub struct Entrance {
    pub element: usize,
    pub center: Vec3f,
    /// Unit vector pointing away from the building.
    pub outward: Vec3f,
    pub width: f32,
    pub room: Option<usize>,
}

/// First-person pose just outside the analysed main entrance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WalkEntryPose {
    pub eye: Vec3f,
    /// Horizontal unit direction pointing into the building.
    pub forward: Vec3f,
}

/// A side of the building, ranked for the approach and reveal shots.
#[derive(Clone, Debug)]
pub struct Facade {
    /// Outward horizontal normal.
    pub dir: Vec3f,
    pub center: Vec3f,
    pub glazing: f32,
    pub detail: f32,
    /// How far a camera can retreat along `dir` and still see.
    pub standoff: f32,
    pub score: f32,
}

/// Which body the clearance question is being asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearMode {
    /// A walker on storey `n`: clearance is the plan-view field, which already
    /// folds in the floor edge.
    Walk(usize),
    /// A drone: clearance is the volumetric field, distance in every direction.
    Fly,
}

/// The one clearance oracle. Everything that asks "is there room here" —
/// A\*, the string pull, the spline relaxer, the QA harness — goes through a
/// `ClearanceField`, and there is no other way to ask.
///
/// The Doom walker learned this the expensive way: its graph and its body used
/// two nearly-identical wall tests, and the sliver between them was a band of
/// ledges the planner promised and the walker then refused forever. Two
/// clearance functions is a bug with a delay fuse.
pub struct ClearanceField<'a> {
    pub site: &'a SiteAnalysis,
    pub mode: ClearMode,
}

impl<'a> ClearanceField<'a> {
    /// Metres of room at `p`. Bilinear (walk) or trilinear (fly) so the field
    /// is continuous and its gradient is usable.
    pub fn at(&self, p: Vec3f) -> f32 {
        match self.mode {
            ClearMode::Fly => self.site.grid.clearance_at(p),
            ClearMode::Walk(si) => {
                let Some(st) = self.site.storeys.get(si) else {
                    return 0.0;
                };
                let g = &self.site.grid;
                let r = (p - g.origin) * (1.0 / g.cell);
                if !(r.x >= 0.0 && r.y >= 0.0) {
                    return 0.0;
                }
                let (x0, y0) = (r.x.floor(), r.y.floor());
                let (fx, fy) = (r.x - x0, r.y - y0);
                let (x0, y0) = (x0 as usize, y0 as usize);
                if x0 + 1 >= st.nx || y0 + 1 >= st.ny {
                    return 0.0;
                }
                let c = &st.clearance;
                let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
                let a = lerp(c[st.at(x0, y0)], c[st.at(x0 + 1, y0)], fx);
                let b = lerp(c[st.at(x0, y0 + 1)], c[st.at(x0 + 1, y0 + 1)], fx);
                lerp(a, b, fy)
            }
        }
    }

    pub fn gradient(&self, p: Vec3f) -> Vec3f {
        let h = self.site.grid.cell;
        let dx = self.at(p + vec3(h, 0.0, 0.0)) - self.at(p - vec3(h, 0.0, 0.0));
        let dy = self.at(p + vec3(0.0, h, 0.0)) - self.at(p - vec3(0.0, h, 0.0));
        let dz = match self.mode {
            // A walker's clearance does not vary with height: its feet are on
            // the floor and pushing it upward is not a manoeuvre it has.
            ClearMode::Walk(_) => 0.0,
            ClearMode::Fly => self.at(p + vec3(0.0, 0.0, h)) - self.at(p - vec3(0.0, 0.0, h)),
        };
        vec3(dx, dy, dz) * (0.5 / h)
    }

    pub fn segment_clear(&self, a: Vec3f, b: Vec3f, radius: f32) -> bool {
        let d = b - a;
        let len = d.length();
        if len < 1e-6 {
            return self.at(a) >= radius;
        }
        let steps = ((len / (self.site.grid.cell * 0.5)).ceil() as usize).max(1);
        (0..=steps).all(|i| self.at(a + d * (i as f32 / steps as f32)) >= radius)
    }

    pub fn cell(&self) -> f32 {
        self.site.grid.cell
    }

    /// Pull a point inside the voxel volume. Outside it the oracle reports no
    /// clearance at all, which is correct — it knows nothing out there — so
    /// every path has to stay in.
    pub fn clamp_inside(&self, p: Vec3f) -> Vec3f {
        let b = self.site.grid.bounds();
        let m = self.site.grid.cell * 3.0;
        vec3(
            p.x.clamp(b.min.x + m, b.max.x - m),
            p.y.clamp(b.min.y + m, b.max.y - m),
            p.z.clamp(b.min.z + m, b.max.z - m),
        )
    }

    /// How far the camera can see along `dir`, and whether it ends outdoors.
    pub fn sight(&self, from: Vec3f, dir: Vec3f, max: f32) -> (f32, bool) {
        self.site.grid.sight_run(from, dir, max)
    }
}

pub struct SiteAnalysis {
    pub grid: VoxelGrid,
    pub config: AnalysisConfig,
    pub storeys: Vec<StoreyPlan>,
    pub rooms: Vec<Room>,
    pub portals: Vec<Portal>,
    pub stairs: Vec<StairLink>,
    pub entrance: Option<Entrance>,
    pub facades: Vec<Facade>,
    /// Door-sized wall gaps derived from free space. Empty when the file
    /// already has typed doors that seal the rooms.
    pub openings: Vec<Opening>,
    /// Bounds of the building proper (everything but the site mesh).
    pub building: Aabb,
    pub bounds: Aabb,
    /// Rooms that no path can reach from the entrance. Reported, never hidden.
    pub unreachable: Vec<usize>,
    /// Visit the largest room next rather than the highest POI score. Set when
    /// the file has no zones and no typed doors — there is nothing else to
    /// rank by.
    pub visit_by_size: bool,
    pub analyse_ms: f32,
}

impl SiteAnalysis {
    pub fn room(&self, id: usize) -> &Room {
        &self.rooms[id]
    }

    /// The clearance oracle for a body. Use this; never read the fields raw.
    pub fn clearance(&self, mode: ClearMode) -> ClearanceField<'_> {
        ClearanceField { site: self, mode }
    }

    /// Rank key for visit order: area when the file has no zones/doors, else
    /// the POI score.
    pub fn room_priority(&self, i: usize) -> f32 {
        if self.visit_by_size {
            self.rooms[i].area
        } else {
            self.rooms[i].score
        }
    }

    /// Rooms in descending order of how much they deserve a camera.
    pub fn rooms_by_rank(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.rooms.len()).filter(|i| self.rooms[*i].interior).collect();
        v.sort_by(|a, b| {
            self.room_priority(*b)
                .partial_cmp(&self.room_priority(*a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    pub fn portals_of(&self, room: usize) -> impl Iterator<Item = &Portal> {
        self.portals
            .iter()
            .filter(move |p| p.a == room || p.b == room)
    }

    /// Put a walk camera `standoff` metres outside the front door, facing in.
    /// The entrance is the room-graph result from [`SiteAnalysis::analyse`],
    /// not a heuristic over raw door bounds. With no exterior portal/opening,
    /// fall back to the building centre at its ground level, facing +Y.
    pub fn walk_entry_pose(&self, eye_height: f32, standoff: f32) -> WalkEntryPose {
        let eye_height = eye_height.max(0.1);
        if let Some(entrance) = &self.entrance {
            let mut outward = vec3(entrance.outward.x, entrance.outward.y, 0.0);
            let len = outward.length();
            if len > 1e-5 && outward.is_finite() {
                outward *= 1.0 / len;
                let floor_z = entrance
                    .room
                    .and_then(|room| self.rooms.get(room))
                    .and_then(|room| self.storeys.get(room.storey))
                    .map(|storey| storey.floor_z)
                    .unwrap_or(entrance.center.z - self.config.body.eye);
                return WalkEntryPose {
                    eye: vec3(
                        entrance.center.x + outward.x * standoff.max(0.0),
                        entrance.center.y + outward.y * standoff.max(0.0),
                        floor_z + eye_height,
                    ),
                    forward: -outward,
                };
            }
        }
        let fallback = if !aabb_is_empty(&self.building) {
            self.building
        } else {
            self.bounds
        };
        if aabb_is_empty(&fallback) {
            return WalkEntryPose {
                eye: vec3(0.0, 0.0, eye_height),
                forward: vec3(0.0, 1.0, 0.0),
            };
        }
        WalkEntryPose {
            eye: vec3(
                (fallback.min.x + fallback.max.x) * 0.5,
                (fallback.min.y + fallback.max.y) * 0.5,
                fallback.min.z + eye_height,
            ),
            forward: vec3(0.0, 1.0, 0.0),
        }
    }

    /// The storey a room lives on.
    pub fn storey_of(&self, room: usize) -> &StoreyPlan {
        &self.storeys[self.rooms[room].storey]
    }

    pub fn analyse(scene: &TourScene, config: &AnalysisConfig) -> SiteAnalysis {
        let t0 = std::time::Instant::now();
        let grid = VoxelGrid::build(scene, &config.voxel);

        let building = building_bounds(scene);
        let visit_by_size = !scene.elements.iter().any(|e| {
            matches!(e.class, crate::scene::TourClass::Zone | crate::scene::TourClass::Door)
        });
        let mut a = SiteAnalysis {
            grid,
            config: *config,
            storeys: Vec::new(),
            rooms: Vec::new(),
            portals: Vec::new(),
            stairs: Vec::new(),
            entrance: None,
            facades: Vec::new(),
            openings: Vec::new(),
            building,
            bounds: scene.bounds,
            unreachable: Vec::new(),
            visit_by_size,
            analyse_ms: 0.0,
        };
        let phase = std::env::var("TOUR_TIMING").is_ok();
        let mut t = std::time::Instant::now();
        let mark = |name: &str, t: &mut std::time::Instant| {
            if phase {
                eprintln!("      [tour] {name:<16} {:.0} ms", t.elapsed().as_secs_f32() * 1000.0);
                *t = std::time::Instant::now();
            }
        };
        if phase {
            eprintln!("      [tour] {:<16} {:.0} ms", "voxelise", t.elapsed().as_secs_f32() * 1000.0);
            t = std::time::Instant::now();
        }
        a.build_storeys(scene);
        mark("storeys", &mut t);
        a.detect_openings(scene);
        mark("openings", &mut t);
        a.seal_openings();
        a.punch_openings();
        a.build_rooms();
        mark("rooms", &mut t);
        a.build_portals(scene);
        a.portals_from_openings();
        mark("portals", &mut t);
        a.name_rooms(scene);
        mark("names", &mut t);
        a.score_rooms(scene);
        mark("scores", &mut t);
        a.build_stairs(scene);
        mark("stairs", &mut t);
        a.find_entrance(scene);
        mark("entrance", &mut t);
        a.rank_facades(scene);
        mark("facades", &mut t);
        a.find_unreachable();
        mark("reach", &mut t);
        a.analyse_ms = t0.elapsed().as_secs_f32() * 1000.0;
        a
    }

    // -- storeys -----------------------------------------------------------

    fn build_storeys(&mut self, scene: &TourScene) {
        let cell = self.grid.cell;
        let body = self.config.body;
        let (nx, ny) = (self.grid.nx, self.grid.ny);
        let resolved = scene.storeys_resolved();
        // A storey with no headroom is a footing level, not a floor to walk.
        for (index, st) in resolved.iter().enumerate() {
            if st.height < body.height * 0.85 {
                continue;
            }
            let floor_z = st.elevation;
            let z0 = self.grid.z_index(floor_z + body.step_up + cell * 0.5);
            let z1 = self.grid.z_index(floor_z + body.height);
            let zf0 = self.grid.z_index(floor_z - 0.75);
            let zf1 = self.grid.z_index(floor_z + body.step_up * 0.5);

            let mut blocked = vec![false; nx * ny];
            let mut sealed_blocked = vec![false; nx * ny];
            let mut has_floor = vec![false; nx * ny];
            for y in 0..ny {
                for x in 0..nx {
                    let i = y * nx + x;
                    blocked[i] = self.grid.column_blocked(x, y, z0, z1);
                    sealed_blocked[i] = self.grid.column_sealed(x, y, z0, z1);
                    has_floor[i] = (zf0..=zf1).any(|z| self.grid.solid_at(x, y, z));
                }
            }
            let wall_clearance = edt_2d(&blocked, nx, ny, cell);
            let sealed_clearance = edt_2d(&sealed_blocked, nx, ny, cell);
            // Distance to the nearest missing floor. A balcony edge stops a
            // walker exactly as hard as a wall does, and folding it into the
            // same field is what keeps "the graph offered it, the body refused
            // it" impossible.
            let no_floor: Vec<bool> = has_floor.iter().map(|f| !*f).collect();
            let floor_clearance = edt_2d(&no_floor, nx, ny, cell);

            let clearance: Vec<f32> = (0..nx * ny)
                .map(|i| {
                    if blocked[i] || !has_floor[i] {
                        0.0
                    } else {
                        wall_clearance[i].min(floor_clearance[i])
                    }
                })
                .collect();

            let eye_z = floor_z + body.eye;
            let ze = self.grid.z_index(eye_z);
            let mut walkable = vec![false; nx * ny];
            let mut interior = vec![false; nx * ny];
            for y in 0..ny {
                for x in 0..nx {
                    let i = y * nx + x;
                    // The outermost ring is never walkable: the clearance
                    // oracle cannot interpolate there (it needs a neighbour on
                    // every side) and reports no room at all. Letting the
                    // lattice offer ground the oracle refuses to measure is
                    // precisely the graph/body disagreement this crate exists
                    // to not have.
                    let edge = x == 0 || y == 0 || x + 1 >= nx || y + 1 >= ny;
                    walkable[i] = !edge && clearance[i] >= body.radius;
                    interior[i] = self.grid.is_interior(x, y, ze);
                }
            }

            self.storeys.push(StoreyPlan {
                index,
                name: st.name.clone(),
                elevation: st.elevation,
                height: st.height,
                floor_z,
                eye_z,
                nx,
                ny,
                walkable,
                clearance,
                sealed_clearance,
                interior,
                room_of: vec![u32::MAX; nx * ny],
                rooms: Vec::new(),
            });
        }
    }

    // -- openings (wall-gap doors) ----------------------------------------

    /// Door-sized holes in walls, found from free-space rather than from typed
    /// Door elements. Two cases, both measured in metres on the wall geometry
    /// so voxelisation eating a cell off each jamb cannot hide a 0.9 m door:
    ///
    /// 1. A 0.7–1.5 m gap between two colinear wall segments (the wall was
    ///    split at the door).
    /// 2. A corridor of free voxels inside one wall's plan rectangle (the
    ///    door was subtracted from a single wall).
    ///
    /// Height ≥ 1.9 m is required of the free column. Those cells are later
    /// removed from the watershed cores so a 1 m doorway does not glue two
    /// rooms into one.
    fn detect_openings(&mut self, scene: &TourScene) {
        let walls: Vec<(usize, Aabb)> = scene
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == TourClass::Wall && e.has_geometry())
            .map(|(i, e)| (i, e.bounds))
            .collect();
        if walls.is_empty() {
            return;
        }
        // Typed door leaves already seal rooms and open the nav grid. Running
        // the geometric detector on top of them invents extra corridors and
        // the room graph stops matching the door schedule.
        if scene
            .elements
            .iter()
            .any(|e| e.class == TourClass::Door)
        {
            return;
        }
        let cell = self.grid.cell;
        for si in 0..self.storeys.len() {
            let floor_z = self.storeys[si].floor_z;
            let z0 = self.grid.z_index(floor_z + 0.10);
            let z1 = self.grid.z_index(floor_z + 2.00);
            if z1 <= z0 {
                continue;
            }
            let on_storey = |bb: &Aabb| bb.max.z >= floor_z + 0.6 && bb.min.z <= floor_z + 2.2;

            // 1. Gaps between colinear wall segments.
            for i in 0..walls.len() {
                for j in i + 1..walls.len() {
                    let a = &walls[i].1;
                    let b = &walls[j].1;
                    if !on_storey(a) || !on_storey(b) {
                        continue;
                    }
                    let a_dx = a.max.x - a.min.x;
                    let a_dy = a.max.y - a.min.y;
                    let b_dx = b.max.x - b.min.x;
                    let b_dy = b.max.y - b.min.y;
                    // Vertical walls (thin in X, run along Y).
                    if a_dx <= a_dy && b_dx <= b_dy {
                        let ax = (a.min.x + a.max.x) * 0.5;
                        let bx = (b.min.x + b.max.x) * 0.5;
                        if (ax - bx).abs() <= 0.45 {
                            let (lo, hi) = if a.max.y <= b.min.y + 1e-3 {
                                (a.max.y, b.min.y)
                            } else if b.max.y <= a.min.y + 1e-3 {
                                (b.max.y, a.min.y)
                            } else {
                                continue;
                            };
                            let width = hi - lo;
                            if (0.70..=1.50).contains(&width) {
                                let x0 = a.min.x.min(b.min.x);
                                let x1 = a.max.x.max(b.max.x);
                                self.try_opening(si, x0, lo, x1, hi, width, vec3(1.0, 0.0, 0.0), z0, z1);
                            }
                        }
                    }
                    // Horizontal walls (thin in Y, run along X).
                    if a_dy <= a_dx && b_dy <= b_dx {
                        let ay = (a.min.y + a.max.y) * 0.5;
                        let by = (b.min.y + b.max.y) * 0.5;
                        if (ay - by).abs() <= 0.45 {
                            let (lo, hi) = if a.max.x <= b.min.x + 1e-3 {
                                (a.max.x, b.min.x)
                            } else if b.max.x <= a.min.x + 1e-3 {
                                (b.max.x, a.min.x)
                            } else {
                                continue;
                            };
                            let width = hi - lo;
                            if (0.70..=1.50).contains(&width) {
                                let y0 = a.min.y.min(b.min.y);
                                let y1 = a.max.y.max(b.max.y);
                                self.try_opening(si, lo, y0, hi, y1, width, vec3(0.0, 1.0, 0.0), z0, z1);
                            }
                        }
                    }
                }
            }

            // 2. Holes inside a single wall's plan rectangle.
            for (_, bb) in &walls {
                if !on_storey(bb) {
                    continue;
                }
                let dx = bb.max.x - bb.min.x;
                let dy = bb.max.y - bb.min.y;
                if dx.min(dy) > 0.85 || dx.max(dy) < 1.0 {
                    continue;
                }
                self.holes_in_wall(si, bb, z0, z1, cell);
            }
        }
    }

    fn cells_in_rect(&self, si: usize, x0: f32, y0: f32, x1: f32, y1: f32, free_only: bool, z0: usize, z1: usize) -> Vec<u32> {
        let st = &self.storeys[si];
        let cell = self.grid.cell;
        let mut cells = Vec::new();
        let gx0 = (((x0.min(x1) - self.grid.origin.x) / cell).floor() as i32).max(0) as usize;
        let gy0 = (((y0.min(y1) - self.grid.origin.y) / cell).floor() as i32).max(0) as usize;
        let gx1 = (((x0.max(x1) - self.grid.origin.x) / cell).ceil() as i32)
            .clamp(0, st.nx as i32 - 1) as usize;
        let gy1 = (((y0.max(y1) - self.grid.origin.y) / cell).ceil() as i32)
            .clamp(0, st.ny as i32 - 1) as usize;
        for y in gy0..=gy1 {
            for x in gx0..=gx1 {
                if free_only && self.grid.column_blocked(x, y, z0, z1) {
                    continue;
                }
                cells.push((y * st.nx + x) as u32);
            }
        }
        cells
    }

    fn try_opening(
        &mut self,
        si: usize,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        through: Vec3f,
        z0: usize,
        z1: usize,
    ) {
        // Inflate the through-axis so a 0.2 m wall still covers a lattice cell.
        let pad = 0.20;
        let (x0, x1, y0, y1) = if through.x.abs() > through.y.abs() {
            (x0 - pad, x1 + pad, y0, y1)
        } else {
            (x0, x1, y0 - pad, y1 + pad)
        };
        let free = self.cells_in_rect(si, x0, y0, x1, y1, true, z0, z1);
        let cells = if free.is_empty() {
            self.cells_in_rect(si, x0, y0, x1, y1, false, z0, z1)
        } else {
            free
        };
        if cells.is_empty() {
            return;
        }
        // Already recorded?
        let cx = (x0 + x1) * 0.5;
        let cy = (y0 + y1) * 0.5;
        if self.openings.iter().any(|o| {
            o.storey == si && (o.center.x - cx).abs() < 0.4 && (o.center.y - cy).abs() < 0.4
        }) {
            return;
        }
        self.openings.push(Opening {
            center: vec3(cx, cy, self.storeys[si].eye_z),
            width,
            height: 1.90,
            storey: si,
            through,
            cells,
        });
    }

    fn holes_in_wall(&mut self, si: usize, bb: &Aabb, z0: usize, z1: usize, cell: f32) {
        let (nx, ny) = (self.storeys[si].nx, self.storeys[si].ny);
        let n = nx * ny;
        let mut hole = vec![false; n];
        let cells = self.cells_in_rect(si, bb.min.x, bb.min.y, bb.max.x, bb.max.y, true, z0, z1);
        if cells.is_empty() {
            return;
        }
        for c in &cells {
            hole[*c as usize] = true;
        }
        let mut seen = vec![false; n];
        for start in &cells {
            let start = *start as usize;
            if seen[start] {
                continue;
            }
            let mut comp = Vec::new();
            let mut q = VecDeque::new();
            seen[start] = true;
            q.push_back(start as u32);
            while let Some(c) = q.pop_front() {
                comp.push(c);
                let (x, y) = ((c as usize) % nx, (c as usize) / nx);
                for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (jx, jy) = (x as i32 + dx, y as i32 + dy);
                    if jx < 0 || jy < 0 || jx >= nx as i32 || jy >= ny as i32 {
                        continue;
                    }
                    let j = jy as usize * nx + jx as usize;
                    if hole[j] && !seen[j] {
                        seen[j] = true;
                        q.push_back(j as u32);
                    }
                }
            }
            let mut min_x = usize::MAX;
            let mut max_x = 0usize;
            let mut min_y = usize::MAX;
            let mut max_y = 0usize;
            let mut sx = 0.0f32;
            let mut sy = 0.0f32;
            for c in &comp {
                let (x, y) = ((*c as usize) % nx, (*c as usize) / nx);
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
                let p = self.grid.world_of(x, y, 0);
                sx += p.x;
                sy += p.y;
            }
            let span_x = (max_x.saturating_sub(min_x)) as f32 * cell + cell;
            let span_y = (max_y.saturating_sub(min_y)) as f32 * cell + cell;
            let (width, thick) = if span_x >= span_y {
                (span_x, span_y)
            } else {
                (span_y, span_x)
            };
            if !(0.70..=1.50).contains(&width) || thick > 0.85 || comp.len() < 2 {
                continue;
            }
            let ncells = comp.len() as f32;
            let through = if span_x >= span_y {
                vec3(0.0, 1.0, 0.0)
            } else {
                vec3(1.0, 0.0, 0.0)
            };
            let cx = sx / ncells;
            let cy = sy / ncells;
            if self.openings.iter().any(|o| {
                o.storey == si && (o.center.x - cx).abs() < 0.4 && (o.center.y - cy).abs() < 0.4
            }) {
                continue;
            }
            self.openings.push(Opening {
                center: vec3(cx, cy, self.storeys[si].eye_z),
                width,
                height: 1.90,
                storey: si,
                through,
                cells: comp,
            });
        }
    }

    /// Knock doorway cells out of the watershed cores. The open grid is
    /// unchanged — the body still walks through — but a 1 m gap is no longer
    /// "open ground" that welds two rooms together.
    fn seal_openings(&mut self) {
        for o in &self.openings {
            let Some(st) = self.storeys.get_mut(o.storey) else {
                continue;
            };
            for &c in &o.cells {
                let i = c as usize;
                if i < st.sealed_clearance.len() {
                    st.sealed_clearance[i] = 0.0;
                }
            }
        }
    }

    /// An architectural door the voxels only half-resolved is still a door:
    /// keep a walkable slot through it so the room graph and the body agree.
    fn punch_openings(&mut self) {
        if self.openings.is_empty() {
            return;
        }
        let r = self.config.body.radius;
        for o in &self.openings {
            let Some(st) = self.storeys.get_mut(o.storey) else {
                continue;
            };
            let (nx, ny) = (st.nx, st.ny);
            for &c in &o.cells {
                let i = c as usize;
                if i >= st.walkable.len() {
                    continue;
                }
                let x = i % nx;
                let y = i / nx;
                if x == 0 || y == 0 || x + 1 >= nx || y + 1 >= ny {
                    continue;
                }
                let z = self.grid.z_index(st.eye_z);
                if self.grid.solid_at(x, y, z) {
                    continue;
                }
                if st.clearance[i] < r {
                    st.clearance[i] = r;
                }
                st.walkable[i] = true;
                st.sealed_clearance[i] = 0.0;
            }
        }
    }

    // -- rooms (the watershed) --------------------------------------------

    fn build_rooms(&mut self) {
        let cell = self.grid.cell;
        let cell_area = cell * cell;
        let core_r = self.config.core_radius;

        for si in 0..self.storeys.len() {
            let (nx, ny) = (self.storeys[si].nx, self.storeys[si].ny);
            let n = nx * ny;
            // A core cell: walkable ground with room to spare, doors shut.
            let core: Vec<bool> = (0..n)
                .map(|i| self.storeys[si].walkable[i] && self.storeys[si].sealed_clearance[i] >= core_r)
                .collect();

            // Connected components of cores → seeds.
            let mut seed_of = vec![u32::MAX; n];
            let mut seeds: Vec<Vec<u32>> = Vec::new();
            for start in 0..n {
                if !core[start] || seed_of[start] != u32::MAX {
                    continue;
                }
                let id = seeds.len() as u32;
                let mut cells = Vec::new();
                let mut q = VecDeque::new();
                seed_of[start] = id;
                q.push_back(start as u32);
                while let Some(c) = q.pop_front() {
                    cells.push(c);
                    let (x, y) = ((c as usize) % nx, (c as usize) / nx);
                    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                        let (jx, jy) = (x as i32 + dx, y as i32 + dy);
                        if jx < 0 || jy < 0 || jx >= nx as i32 || jy >= ny as i32 {
                            continue;
                        }
                        let j = jy as usize * nx + jx as usize;
                        if core[j] && seed_of[j] == u32::MAX {
                            seed_of[j] = id;
                            q.push_back(j as u32);
                        }
                    }
                }
                seeds.push(cells);
            }
            // Drop specks.
            let keep: Vec<bool> = seeds
                .iter()
                .map(|c| c.len() as f32 * cell_area >= self.config.min_room_area)
                .collect();

            // Multi-source BFS from every core at once. The fronts meet in the
            // middle of each doorway and that meeting line is the room
            // boundary — which is also what makes the two rooms *adjacent*,
            // and adjacency is where portals come from.
            //
            // The growth is deliberately unbounded. An earlier version capped
            // it, which left a ribbon of unassigned cells in every doorway too
            // small to survive the corridor filter; the rooms then touched
            // nothing and the building came out as a set of islands with no
            // doors at all.
            let mut owner = vec![u32::MAX; n];
            let mut dist = vec![u32::MAX; n];
            let mut q: VecDeque<u32> = VecDeque::new();
            let mut live = 0u32;
            let mut remap = vec![u32::MAX; seeds.len()];
            for (s, cells) in seeds.iter().enumerate() {
                if !keep[s] {
                    continue;
                }
                remap[s] = live;
                for c in cells {
                    owner[*c as usize] = live;
                    dist[*c as usize] = 0;
                    q.push_back(*c);
                }
                live += 1;
            }
            while let Some(c) = q.pop_front() {
                let ci = c as usize;
                let d = dist[ci];
                let (x, y) = (ci % nx, ci / nx);
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (jx, jy) = (x as i32 + dx, y as i32 + dy);
                    if jx < 0 || jy < 0 || jx >= nx as i32 || jy >= ny as i32 {
                        continue;
                    }
                    let j = jy as usize * nx + jx as usize;
                    if self.storeys[si].walkable[j] && owner[j] == u32::MAX {
                        owner[j] = owner[ci];
                        dist[j] = d + 1;
                        q.push_back(j as u32);
                    }
                }
            }

            // Whatever the fronts never reached has no core at all: a strip of
            // walkable ground too narrow anywhere along it to be a room. That
            // is circulation.
            let mut corridor_id = live;
            for start in 0..n {
                if !self.storeys[si].walkable[start] || owner[start] != u32::MAX {
                    continue;
                }
                let mut cells = Vec::new();
                let mut q = VecDeque::new();
                owner[start] = corridor_id;
                q.push_back(start as u32);
                while let Some(c) = q.pop_front() {
                    cells.push(c);
                    let (x, y) = ((c as usize) % nx, (c as usize) / nx);
                    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                        let (jx, jy) = (x as i32 + dx, y as i32 + dy);
                        if jx < 0 || jy < 0 || jx >= nx as i32 || jy >= ny as i32 {
                            continue;
                        }
                        let j = jy as usize * nx + jx as usize;
                        if self.storeys[si].walkable[j] && owner[j] == u32::MAX {
                            owner[j] = corridor_id;
                            q.push_back(j as u32);
                        }
                    }
                }
                if cells.len() as f32 * cell_area < self.config.min_corridor_area {
                    for c in &cells {
                        owner[*c as usize] = u32::MAX;
                    }
                } else {
                    corridor_id += 1;
                }
            }

            // Materialise rooms.
            let mut local: Vec<Vec<u32>> = vec![Vec::new(); corridor_id as usize];
            for i in 0..n {
                if owner[i] != u32::MAX {
                    local[owner[i] as usize].push(i as u32);
                }
            }
            for (li, cells) in local.into_iter().enumerate() {
                if cells.is_empty() {
                    continue;
                }
                let no_core = li as u32 >= live;
                let gid = self.rooms.len();
                for c in &cells {
                    self.storeys[si].room_of[*c as usize] = gid as u32;
                }
                // Most open cell = where a person would stand to take it in.
                let (mut best, mut best_c) = (-1.0f32, cells[0]);
                for c in &cells {
                    let v = self.storeys[si].clearance[*c as usize];
                    if v > best {
                        best = v;
                        best_c = *c;
                    }
                }
                // Circulation: nowhere in it can you stand well clear of a wall.
                let corridor = no_core || best < self.config.corridor_width;
                let mut bounds = aabb_empty();
                let mut interior_votes = 0usize;
                for c in &cells {
                    let (x, y) = ((*c as usize) % nx, (*c as usize) / nx);
                    let p = self.grid.world_of(x, y, 0);
                    bounds = aabb_union_point(
                        &bounds,
                        vec3(p.x, p.y, self.storeys[si].floor_z),
                    );
                    bounds = aabb_union_point(
                        &bounds,
                        vec3(p.x, p.y, self.storeys[si].floor_z + self.storeys[si].height),
                    );
                    if self.storeys[si].interior[*c as usize] {
                        interior_votes += 1;
                    }
                }
                let (bx, by) = ((best_c as usize) % nx, (best_c as usize) / nx);
                let bp = self.grid.world_of(bx, by, 0);
                let center = vec3(bp.x, bp.y, self.storeys[si].eye_z);
                let area = cells.len() as f32 * cell_area;
                self.rooms.push(Room {
                    id: gid,
                    storey: si,
                    name: String::new(),
                    area,
                    cells,
                    center,
                    bounds,
                    interior: interior_votes * 2 > 0 && interior_votes as f32 > 0.5 * area / cell_area,
                    corridor,
                    glazing: 0.0,
                    ceiling: self.storeys[si].height,
                    sightline: 0.0,
                    best_view: center + vec3(1.0, 0.0, 0.0),
                    score: 0.0,
                });
                self.storeys[si].rooms.push(gid);
            }
        }
    }

    // -- the room graph ----------------------------------------------------

    fn build_portals(&mut self, scene: &TourScene) {
        let mut found: std::collections::HashMap<(usize, usize), (f32, u32)> =
            std::collections::HashMap::new();
        for si in 0..self.storeys.len() {
            let (nx, ny) = (self.storeys[si].nx, self.storeys[si].ny);
            for y in 0..ny {
                for x in 0..nx {
                    let i = y * nx + x;
                    let ra = self.storeys[si].room_of[i];
                    if ra == u32::MAX {
                        continue;
                    }
                    for (dx, dy) in [(1usize, 0usize), (0, 1)] {
                        let (jx, jy) = (x + dx, y + dy);
                        if jx >= nx || jy >= ny {
                            continue;
                        }
                        let j = jy * nx + jx;
                        let rb = self.storeys[si].room_of[j];
                        if rb == u32::MAX || rb == ra {
                            continue;
                        }
                        let key = (ra.min(rb) as usize, ra.max(rb) as usize);
                        // The widest point of the shared boundary is the
                        // middle of the doorway.
                        let w = self.storeys[si].clearance[i].min(self.storeys[si].clearance[j]);
                        let e = found.entry(key).or_insert((-1.0, i as u32));
                        if w > e.0 {
                            *e = (w, i as u32);
                        }
                    }
                }
            }
        }
        let doors: Vec<(usize, Aabb)> = scene
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class.is_portal() && e.has_geometry())
            .map(|(i, e)| (i, e.bounds))
            .collect();

        for ((a, b), (w, cell_i)) in found {
            let si = self.rooms[a].storey;
            let nx = self.storeys[si].nx;
            let (x, y) = ((cell_i as usize) % nx, (cell_i as usize) / nx);
            let p = self.grid.world_of(x, y, 0);
            let center = vec3(p.x, p.y, self.storeys[si].eye_z);
            // Attribute the opening to a door element if one straddles it.
            let element = doors
                .iter()
                .filter(|(_, bb)| aabb_distance(bb, center) < 1.2)
                .min_by(|(_, x1), (_, x2)| {
                    aabb_distance(x1, center)
                        .partial_cmp(&aabb_distance(x2, center))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| *i);
            self.portals.push(Portal {
                a,
                b,
                center,
                width: (w * 2.0).max(self.grid.cell),
                element,
            });
        }
    }

    /// Typed-door files already have watershed portals at every doorway.
    /// Derived openings may sit between two rooms that never grew into each
    /// other (the gap was not walkable until we punched it). Link them now
    /// so the visit order can walk the house.
    fn portals_from_openings(&mut self) {
        for oi in 0..self.openings.len() {
            let o_storey = self.openings[oi].storey;
            let o_width = self.openings[oi].width;
            let o_through = self.openings[oi].through;
            let o_cells = self.openings[oi].cells.clone();
            let center = {
                let Some(st) = self.storeys.get(o_storey) else {
                    continue;
                };
                let (nx, eye_z) = (st.nx, st.eye_z);
                let mut stand = None;
                let mut stand_c = 0.0f32;
                for &c in &o_cells {
                    let i = c as usize;
                    if i >= st.walkable.len() || !st.walkable[i] {
                        continue;
                    }
                    if st.clearance[i] > stand_c {
                        stand_c = st.clearance[i];
                        let (x, y) = (i % nx, i / nx);
                        let w = self.grid.world_of(x, y, 0);
                        stand = Some(vec3(w.x, w.y, eye_z));
                    }
                }
                stand
            };
            let Some(center) = center else {
                continue;
            };
            let perp = vec3(-o_through.y, o_through.x, 0.0);
            for dir in [o_through, perp] {
                let pa = center - dir * 1.2;
                let pb = center + dir * 1.2;
                let ra = self.room_at(pa, o_storey);
                let rb = self.room_at(pb, o_storey);
                let (Some(a), Some(b)) = (ra, rb) else {
                    continue;
                };
                if a == b {
                    continue;
                }
                if !(self.rooms[a].interior && self.rooms[b].interior) {
                    continue;
                }
                if self
                    .portals
                    .iter()
                    .any(|p| (p.a == a && p.b == b) || (p.a == b && p.b == a))
                {
                    break;
                }
                self.portals.push(Portal {
                    a,
                    b,
                    center,
                    width: o_width,
                    element: None,
                });
                break;
            }
        }
    }

    /// source application zones are room labels the architect already wrote. If one sits
    /// over a room we found, use its name instead of inventing one.
    fn name_rooms(&mut self, scene: &TourScene) {
        let zones: Vec<(&str, Aabb)> = scene
            .elements
            .iter()
            .filter(|e| e.class == TourClass::Zone)
            .map(|e| (e.name.as_str(), e.bounds))
            .collect();
        for r in self.rooms.iter_mut() {
            let mut named = None;
            for (name, bb) in &zones {
                let plan_hit = r.center.x >= bb.min.x
                    && r.center.x <= bb.max.x
                    && r.center.y >= bb.min.y
                    && r.center.y <= bb.max.y;
                let z_hit = r.center.z >= bb.min.z - 1.0 && r.center.z <= bb.max.z + 1.5;
                if plan_hit && z_hit && !name.is_empty() {
                    named = Some(name.to_string());
                    break;
                }
            }
            r.name = named.unwrap_or_else(|| {
                if r.corridor {
                    format!("Circulation {}", r.id)
                } else {
                    format!("Room {}", r.id)
                }
            });
        }
    }

    fn score_rooms(&mut self, scene: &TourScene) {
        let glazing: Vec<(Vec3f, f32)> = scene
            .elements
            .iter()
            .filter(|e| e.class.is_glazing() && e.has_geometry())
            .map(|e| (e.center(), e.facade_area()))
            .collect();
        let stairs: Vec<Aabb> = scene
            .elements_of_class(TourClass::Stair)
            .map(|(_, e)| e.bounds)
            .collect();

        let n_rooms = self.rooms.len();
        for ri in 0..n_rooms {
            // Glazing is attributed to whichever room owns the ground just
            // inside it: probe the compass around the element.
            let mut glass = 0.0f32;
            for (gc, area) in &glazing {
                let mut hit = false;
                for k in 0..8 {
                    let a = k as f32 * std::f32::consts::TAU / 8.0;
                    for d in [0.5f32, 1.0, 1.6] {
                        let p = *gc + vec3(a.cos() * d, a.sin() * d, 0.0);
                        if self.room_at(p, self.rooms[ri].storey) == Some(ri) {
                            hit = true;
                            break;
                        }
                    }
                    if hit {
                        break;
                    }
                }
                if hit {
                    glass += *area;
                }
            }
            let has_stair = stairs.iter().any(|s| {
                let c = self.rooms[ri].center;
                aabb_distance(s, vec3(c.x, c.y, c.z)) < 2.0
            });
            let portals = self.portals_of(ri).count() as f32;

            // Best view: sweep the compass, take the longest sight line, and
            // reward one that ends outdoors — that is a window, not a wall.
            let c = self.rooms[ri].center;
            let (mut best_score, mut best_dir, mut best_len) = (-1.0f32, vec3(1.0, 0.0, 0.0), 0.0f32);
            for k in 0..24 {
                let yaw = k as f32 * std::f32::consts::TAU / 24.0;
                for pitch in [-0.12f32, 0.0, 0.10] {
                    let d = yaw_pitch_to_dir(yaw, pitch);
                    let (len, outside) = self.grid.sight_run(c, d, 40.0);
                    let s = (len / 12.0).min(2.5) + if outside { 1.5 } else { 0.0 };
                    if s > best_score {
                        best_score = s;
                        best_dir = d;
                        best_len = len;
                    }
                }
            }

            let r = &mut self.rooms[ri];
            r.glazing = glass;
            r.sightline = best_len;
            r.best_view = c + best_dir * best_len.max(2.0);
            r.score = 1.0 * (r.area / 10.0).max(0.0).sqrt()
                + 1.2 * (glass / 6.0)
                + 0.6 * (r.ceiling - 2.4).max(0.0)
                + if has_stair { 0.8 } else { 0.0 }
                + 0.5 * (portals / 3.0)
                + 0.4 * (best_len / 12.0).min(1.5);
            if r.corridor {
                r.score *= 0.55;
            }
        }
    }

    /// Which room, if any, owns the ground under `p` on storey `si`.
    pub fn room_at(&self, p: Vec3f, si: usize) -> Option<usize> {
        let st = self.storeys.get(si)?;
        let (x, y, _) = self.grid.cell_of(vec3(p.x, p.y, st.eye_z))?;
        let r = st.room_of[st.at(x, y)];
        (r != u32::MAX).then_some(r as usize)
    }

    /// The nearest walkable cell of `room` to `p`, as a world point at eye
    /// height. Used to snap arbitrary targets onto the navigable lattice.
    pub fn nearest_in_room(&self, room: usize, p: Vec3f) -> Vec3f {
        let r = &self.rooms[room];
        let st = &self.storeys[r.storey];
        let mut best = r.center;
        let mut bd = f32::INFINITY;
        for c in &r.cells {
            let (x, y) = ((*c as usize) % st.nx, (*c as usize) / st.nx);
            let w = self.grid.world_of(x, y, 0);
            let q = vec3(w.x, w.y, st.eye_z);
            let d = (q - p).length_squared();
            if d < bd {
                bd = d;
                best = q;
            }
        }
        best
    }

    fn build_stairs(&mut self, scene: &TourScene) {
        let stairs: Vec<(usize, Aabb)> = scene
            .elements_of_class(TourClass::Stair)
            .filter(|(_, e)| e.has_geometry())
            .map(|(i, e)| (i, e.bounds))
            .collect();
        for (ei, bb) in stairs {
            let c = aabb_center(&bb);
            // The storeys the stair spans.
            let mut touched: Vec<usize> = Vec::new();
            for (si, st) in self.storeys.iter().enumerate() {
                if st.floor_z >= bb.min.z - 0.6 && st.floor_z <= bb.max.z + 0.6 {
                    touched.push(si);
                }
            }
            for w in touched.windows(2) {
                let (lo, hi) = (w[0], w[1]);
                // Choose the foot and head by *testing the climb*, not by
                // guessing. A staircase is the tightest space a tour goes
                // through and the one place a straight chord between two
                // plausible-looking points reliably crosses a wall, a stringer
                // or the slab edge — so try the nearest candidates on each
                // storey and keep the pair whose climb is actually clear.
                // The two ends of the flight's run, just clear of the treads.
                let sz = aabb_size(&bb);
                let over = 0.30;
                let (end_a, end_b) = if sz.x >= sz.y {
                    (
                        vec3(bb.min.x - over, c.y, 0.0),
                        vec3(bb.max.x + over, c.y, 0.0),
                    )
                } else {
                    (
                        vec3(c.x, bb.min.y - over, 0.0),
                        vec3(c.x, bb.max.y + over, 0.0),
                    )
                };

                let fly = self.clearance(ClearMode::Fly);
                let ez_lo = self.storeys[lo].eye_z;
                let ez_hi = self.storeys[hi].eye_z;
                let mut best: Option<(f32, usize, Vec3f, usize, Vec3f, Vec3f, Vec3f)> = None;

                // Which end is the foot is also a guess, so test both.
                for (foot, head) in [(end_a, end_b), (end_b, end_a)] {
                    let run_low = vec3(foot.x, foot.y, ez_lo);
                    let run_high = vec3(head.x, head.y, ez_hi);
                    let foot_cands = self.walkable_candidates(lo, foot, 10);
                    let head_cands = self.walkable_candidates(hi, head, 10);
                    for (fr, fp) in &foot_cands {
                        for (hr, hp) in &head_cands {
                            // Sample the ramp the generator will actually fly,
                            // bow and all — verifying a straight chord and then
                            // flying a curve is checking the wrong shot.
                            let mut worst = f32::INFINITY;
                            for (a, b, bow) in [
                                (*fp, run_low, 0.0f32),
                                (run_low, run_high, 0.20),
                                (run_high, *hp, 0.0),
                            ] {
                                let n =
                                    (((b - a).length() / (self.grid.cell * 0.5)).ceil() as usize).max(1);
                                for k in 0..=n {
                                    let t = k as f32 / n as f32;
                                    let mut p = a + (b - a) * t;
                                    p.z += (t * std::f32::consts::PI).sin() * bow;
                                    worst = worst.min(fly.at(p));
                                }
                            }
                            if best.as_ref().map_or(true, |(bw, ..)| worst > *bw) {
                                best =
                                    Some((worst, *fr, *fp, *hr, *hp, run_low, run_high));
                            }
                        }
                    }
                }

                // No clear climb anywhere: say so by leaving the upper rooms
                // unreachable rather than inventing a route through the slab.
                let Some((worst, lr, foot_pt, ur, head_pt, run_low, run_high)) = best else {
                    continue;
                };
                // A staircase is the tightest space in a building; ask only
                // that the climb is not *inside* anything and let the relaxer
                // win back the rest.
                if worst < 0.12 || lr == ur {
                    continue;
                }
                let bottom = vec3(foot_pt.x, foot_pt.y, ez_lo);
                let top = vec3(head_pt.x, head_pt.y, ez_hi);
                self.stairs.push(StairLink {
                    lower_room: lr,
                    upper_room: ur,
                    bottom,
                    top,
                    run_low,
                    run_high,
                    element: ei,
                });
            }
        }
    }

    /// Closest point on a storey that is both walkable **and** assigned to a
    /// room, with the room. Stairs land wherever they land, and the single
    /// nearest walkable cell is often an unassigned sliver at the edge of the
    /// stairwell; insisting on that one cell loses the stair link entirely and
    /// maroons the whole floor above.
    pub fn nearest_room_point(
        &self,
        si: usize,
        p: Vec3f,
        interior_only: bool,
    ) -> Option<(usize, Vec3f)> {
        self.nearest_room_point_visible(si, p, interior_only, None)
    }

    /// As [`SiteAnalysis::nearest_room_point`], but optionally requiring a
    /// clear straight line from `visible_from`. Nearest-by-distance alone will
    /// happily return a point in the next room through a wall, and a stair
    /// whose foot is on the far side of a partition sends the climb through it.
    pub fn nearest_room_point_visible(
        &self,
        si: usize,
        p: Vec3f,
        interior_only: bool,
        visible_from: Option<Vec3f>,
    ) -> Option<(usize, Vec3f)> {
        let st = self.storeys.get(si)?;
        let field = self.clearance(ClearMode::Walk(si));
        let radius = self.config.body.radius * 0.7;
        let mut best: Option<(f32, usize, Vec3f)> = None;
        for y in 0..st.ny {
            for x in 0..st.nx {
                let i = st.at(x, y);
                if !st.walkable[i] || st.room_of[i] == u32::MAX {
                    continue;
                }
                if interior_only && !self.rooms[st.room_of[i] as usize].interior {
                    continue;
                }
                let w = self.grid.world_of(x, y, 0);
                let d = (w.x - p.x) * (w.x - p.x) + (w.y - p.y) * (w.y - p.y);
                if best.map_or(true, |(bd, _, _)| d < bd) {
                    let q = vec3(w.x, w.y, st.eye_z);
                    if let Some(v) = visible_from {
                        if !field.segment_clear(vec3(v.x, v.y, st.eye_z), q, radius) {
                            continue;
                        }
                    }
                    best = Some((d, st.room_of[i] as usize, q));
                }
            }
        }
        best.map(|(_, r, p)| (r, p))
    }

    /// The `n` nearest walkable interior cells to a plan position, as
    /// `(room, point)`, nearest first and at most one per room.
    pub fn walkable_candidates(&self, si: usize, p: Vec3f, n: usize) -> Vec<(usize, Vec3f)> {
        let Some(st) = self.storeys.get(si) else {
            return Vec::new();
        };
        let mut all: Vec<(f32, usize, Vec3f)> = Vec::new();
        for y in 0..st.ny {
            for x in 0..st.nx {
                let i = st.at(x, y);
                if !st.walkable[i] || st.room_of[i] == u32::MAX {
                    continue;
                }
                let r = st.room_of[i] as usize;
                if !self.rooms[r].interior {
                    continue;
                }
                let w = self.grid.world_of(x, y, 0);
                let d = (w.x - p.x) * (w.x - p.x) + (w.y - p.y) * (w.y - p.y);
                all.push((d, r, vec3(w.x, w.y, st.eye_z)));
            }
        }
        all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut seen: Vec<usize> = Vec::new();
        let mut out = Vec::new();
        for (_, r, q) in all {
            if seen.contains(&r) {
                continue;
            }
            seen.push(r);
            out.push((r, q));
            if out.len() >= n {
                break;
            }
        }
        out
    }

    /// Nearest cell of one specific room to a plan position, at eye height.
    pub fn nearest_in_room_to(&self, room: usize, p: Vec3f) -> Option<Vec3f> {
        let r = self.rooms.get(room)?;
        let st = &self.storeys[r.storey];
        let mut best: Option<(f32, Vec3f)> = None;
        for cidx in &r.cells {
            let (x, y) = ((*cidx as usize) % st.nx, (*cidx as usize) / st.nx);
            if !st.walkable[st.at(x, y)] {
                continue;
            }
            let w = self.grid.world_of(x, y, 0);
            let d = (w.x - p.x) * (w.x - p.x) + (w.y - p.y) * (w.y - p.y);
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, vec3(w.x, w.y, st.eye_z)));
            }
        }
        best.map(|(_, p)| p)
    }

    /// Closest walkable point on a storey to a plan position.
    pub fn nearest_walkable(&self, si: usize, p: Vec3f) -> Option<Vec3f> {
        let st = self.storeys.get(si)?;
        let mut best = None;
        let mut bd = f32::INFINITY;
        for y in 0..st.ny {
            for x in 0..st.nx {
                if !st.walkable[st.at(x, y)] {
                    continue;
                }
                let w = self.grid.world_of(x, y, 0);
                let d = (w.x - p.x) * (w.x - p.x) + (w.y - p.y) * (w.y - p.y);
                if d < bd {
                    bd = d;
                    best = Some(vec3(w.x, w.y, st.eye_z));
                }
            }
        }
        best
    }

    /// The front door: the widest portal joining an interior room to the
    /// outdoors, lowest storey first.
    ///
    /// Found from the room graph rather than by probing door elements. A
    /// portal already *is* a navigable opening between two regions — the
    /// planner found it by walking through it — so anything the analysis can
    /// route through, this can find. Probing each `Door` element instead meant
    /// guessing its facing, guessing how far in "inside" was, and returning
    /// nothing whenever the guess landed on a partition, which then sent every
    /// exterior shot to a fallback and cost the full tour its interior.
    fn find_entrance(&mut self, _scene: &TourScene) {
        let mut best: Option<(f32, Entrance)> = None;
        for p in &self.portals {
            let inside = match (self.rooms[p.a].interior, self.rooms[p.b].interior) {
                (true, false) => p.a,
                (false, true) => p.b,
                _ => continue,
            };
            let si = self.rooms[inside].storey;
            // Which way is out? Probe the compass from the opening.
            let mut outward = None;
            for k in 0..16 {
                let a = k as f32 * std::f32::consts::TAU / 16.0;
                let d = vec3(a.cos(), a.sin(), 0.0);
                let out_p = p.center + d * 1.6;
                let in_p = p.center - d * 1.6;
                let out_ext = self
                    .grid
                    .cell_of(out_p)
                    .map_or(false, |(x, y, z)| self.grid.exterior_at(x, y, z));
                let in_ext = self
                    .grid
                    .cell_of(in_p)
                    .map_or(true, |(x, y, z)| self.grid.exterior_at(x, y, z));
                if out_ext && !in_ext {
                    outward = Some(d);
                    break;
                }
            }
            let Some(outward) = outward else { continue };
            // Wide openings near ground level make the best front doors.
            // Scoring by elevation alone rewards the basement: a garage door
            // at -2.95 m beat the front door at 0, and the approach shot then
            // flew down into the terrain to reach it.
            //
            // Legacy files have no typed door and may put the main living
            // floor up the hillside: prefer the storey with more interior.
            let score = p.width * 3.0 + self.entrance_storey_bonus(si);
            if best.as_ref().map_or(true, |(bs, _)| score > *bs) {
                best = Some((
                    score,
                    Entrance {
                        element: p.element.unwrap_or(usize::MAX),
                        center: p.center,
                        outward,
                        width: p.width,
                        room: Some(inside),
                    },
                ));
            }
        }
        // Derived openings too, not only as a fallback: a hillside house
        // often has its living floor's door as a wall-gap with no portal to
        // a walkable garden, and that door is still the right way in.
        for o in &self.openings {
            for sign in [1.0f32, -1.0] {
                let outward = o.through * sign;
                let out_p = o.center + outward * 1.6;
                let in_p = o.center - outward * 1.6;
                let out_ext = self
                    .grid
                    .cell_of(out_p)
                    .map_or(false, |(x, y, z)| self.grid.exterior_at(x, y, z));
                let in_ext = self
                    .grid
                    .cell_of(in_p)
                    .map_or(true, |(x, y, z)| self.grid.exterior_at(x, y, z));
                if !out_ext || in_ext {
                    continue;
                }
                let room = self.room_at(in_p, o.storey).filter(|r| self.rooms[*r].interior);
                let score = o.width * 3.0 + self.entrance_storey_bonus(o.storey);
                if best.as_ref().map_or(true, |(bs, _)| score > *bs) {
                    best = Some((
                        score,
                        Entrance {
                            element: usize::MAX,
                            center: o.center,
                            outward,
                            width: o.width,
                            room,
                        },
                    ));
                }
            }
        }
        self.entrance = best.map(|(_, e)| e);
        self.retarget_entrance_to_main_room();
    }

    /// Legacy houses on a hillside often have the front door in a 1-room
    /// vestibule and the living floor as a disconnected blob. Prefer an
    /// exterior opening on the largest room's storey so the walkthrough and
    /// the approach actually meet.
    fn retarget_entrance_to_main_room(&mut self) {
        if !self.visit_by_size {
            return;
        }
        let Some(&main) = self.rooms_by_rank().first() else {
            return;
        };
        let mc = self.rooms[main].center;
        let si = self.rooms[main].storey;
        let mut best: Option<(f32, Entrance)> = None;
        for o in &self.openings {
            if o.storey != si {
                continue;
            }
            for sign in [1.0f32, -1.0] {
                let outward = o.through * sign;
                let out_p = o.center + outward * 1.6;
                let in_p = o.center - outward * 1.6;
                let out_ext = self
                    .grid
                    .cell_of(out_p)
                    .map_or(false, |(x, y, z)| self.grid.exterior_at(x, y, z));
                let in_ext = self
                    .grid
                    .cell_of(in_p)
                    .map_or(true, |(x, y, z)| self.grid.exterior_at(x, y, z));
                if !out_ext || in_ext {
                    continue;
                }
                let gap = o.center - outward * 0.25;
                let clear = self.grid.clearance_at(gap);
                if clear < 0.18 {
                    continue;
                }
                let d = (o.center.x - mc.x).hypot(o.center.y - mc.y);
                let score = o.width * 2.0 + clear * 4.0 - d * 0.12;
                if best.as_ref().map_or(true, |(bs, _)| score > *bs) {
                    best = Some((
                        score,
                        Entrance {
                            element: usize::MAX,
                            center: o.center,
                            outward,
                            width: o.width,
                            room: Some(main),
                        },
                    ));
                }
            }
        }
        if let Some((_, e)) = best {
            self.entrance = Some(e);
        }
    }

    fn entrance_storey_bonus(&self, si: usize) -> f32 {
        if self.visit_by_size {
            let area: f32 = self.storeys.get(si).map_or(0.0, |st| {
                st.rooms
                    .iter()
                    .filter(|r| self.rooms[**r].interior)
                    .map(|r| self.rooms[*r].area)
                    .sum()
            });
            area * 0.04
        } else {
            -self.storeys[si].elevation.abs() * 1.5
        }
    }

    /// Score the compass around the building. A façade is worth approaching
    /// when it has glass, has modelled detail, and lets the camera stand back.
    fn rank_facades(&mut self, scene: &TourScene) {
        let c = aabb_center(&self.building);
        let size = aabb_size(&self.building);
        let plan_r = (size.x.max(size.y)) * 0.5;
        let mid_z = self.building.min.z + size.z * 0.5;

        for k in 0..16 {
            let yaw = k as f32 * std::f32::consts::TAU / 16.0;
            let dir = vec3(yaw.cos(), yaw.sin(), 0.0);
            let mut glazing = 0.0f32;
            let mut detail = 0.0f32;
            for e in &scene.elements {
                if !e.has_geometry() || e.class == TourClass::Site || e.class == TourClass::Zone {
                    continue;
                }
                let to = e.center() - c;
                let plan = vec3(to.x, to.y, 0.0);
                if plan.length() < 1e-3 {
                    continue;
                }
                let facing = plan.normalize().dot(dir);
                if facing <= 0.25 {
                    continue;
                }
                let w = facing * (plan.length() / plan_r.max(1e-3)).min(1.5);
                if e.class.is_glazing() {
                    glazing += e.facade_area() * w;
                }
                detail += e.tri_count as f32 * w;
            }
            let eye = vec3(c.x, c.y, mid_z);
            let standoff = self
                .grid
                .free_run(eye + dir * plan_r * 1.05, dir, self.config.body.radius, 80.0);
            let score = 1.0 * (glazing / 10.0).min(4.0)
                + 0.5 * (detail / 20_000.0).min(3.0)
                + 0.6 * (standoff / 25.0).min(1.5);
            self.facades.push(Facade {
                dir,
                center: eye + dir * plan_r,
                glazing,
                detail,
                standoff,
                score,
            });
        }
        self.facades.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Interior rooms outside the building's **largest connected region**.
    ///
    /// Defined against the largest region rather than against the entrance,
    /// because that is what the tour actually visits: a building can be split
    /// in two by a bricked-up doorway, and when the front door opens into the
    /// smaller half the tour shows the larger one. Reporting "unreachable from
    /// the entrance" would then contradict the track, which visits several of
    /// them.
    ///
    /// These are *reported*, never quietly dropped: a tour that skips half a
    /// house without saying so is worse than one that fails.
    fn find_unreachable(&mut self) {
        let n = self.rooms.len();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for p in &self.portals {
            if self.rooms[p.a].interior && self.rooms[p.b].interior {
                adj[p.a].push(p.b);
                adj[p.b].push(p.a);
            }
        }
        for s in &self.stairs {
            if self.rooms[s.lower_room].interior && self.rooms[s.upper_room].interior {
                adj[s.lower_room].push(s.upper_room);
                adj[s.upper_room].push(s.lower_room);
            }
        }
        let mut comp = vec![usize::MAX; n];
        let mut sizes: Vec<usize> = Vec::new();
        for r in 0..n {
            if !self.rooms[r].interior || comp[r] != usize::MAX {
                continue;
            }
            let id = sizes.len();
            let mut count = 0usize;
            let mut q = VecDeque::new();
            comp[r] = id;
            q.push_back(r);
            while let Some(c) = q.pop_front() {
                count += 1;
                for nb in &adj[c] {
                    if comp[*nb] == usize::MAX {
                        comp[*nb] = id;
                        q.push_back(*nb);
                    }
                }
            }
            sizes.push(count);
        }
        let main = sizes
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| **n)
            .map(|(i, _)| i);
        self.unreachable = (0..n)
            .filter(|i| {
                self.rooms[*i].interior && Some(comp[*i]) != main
            })
            .collect();
    }
}

/// Bounds of the building without the terrain, which is usually a huge plate
/// that would otherwise swallow every framing decision.
fn building_bounds(scene: &TourScene) -> Aabb {
    scene.building_bounds()
}
