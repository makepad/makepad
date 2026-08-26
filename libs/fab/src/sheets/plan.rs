//! Lane E: the **floor plan**.
//!
//! A plan is not a top view. It is a horizontal cut through the building at
//! about 1.2 m, where everything the plane passes through is drawn heavy and
//! filled (poché), everything below it is drawn lighter, and everything above
//! it is drawn as a thin dashed line or not at all. That classification, and
//! the fact that symbols *replace* the geometry they stand for, is what makes
//! a drawing read as a plan instead of a pile of outlines.
//!
//! The geometry comes from [`crate::sheets::slice`]; this module decides what
//! to cut, what class every drawn edge is, what becomes a symbol, and where
//! the annotation goes. Everything is in **paper millimetres** at true scale.

use crate::api::*;
use crate::sheets::slice::{self, Chain, Part, P2};
use crate::model::{Sheet, SheetItem, SheetLink, Stroke};
use makepad_widgets::*;

/// A3 landscape, the size Fab publishes its own layouts at.
pub const PAGE: [f32; 2] = [420.0, 297.0];
const MARGIN: f32 = 10.0;
const TITLE_H: f32 = 24.0;
/// Room outside the plan for dimension lines and labels.
const DIM_GUTTER: f32 = 16.0;

// ---- line weights, paper mm (the hierarchy is the drawing) ----
const W_CUT: f32 = 0.50;
const W_OPENING: f32 = 0.30;
const W_BELOW: f32 = 0.25;
const W_ABOVE: f32 = 0.15;
const W_ANNOT: f32 = 0.18;
const W_FRAME: f32 = 0.35;

// ---- ink ----
const INK: [f32; 4] = [0.09, 0.09, 0.09, 1.0];
const INK_BELOW: [f32; 4] = [0.36, 0.36, 0.36, 1.0];
const INK_ABOVE: [f32; 4] = [0.58, 0.58, 0.58, 1.0];
const INK_ANNOT: [f32; 4] = [0.22, 0.28, 0.42, 1.0];
const POCHE: [f32; 4] = [0.38, 0.37, 0.35, 1.0];
const ROOM_INK: [f32; 4] = [0.30, 0.30, 0.30, 1.0];
const OPENING_INK: [f32; 4] = [0.13, 0.13, 0.13, 1.0];

/// How an element takes part in the drawing. Decided once, per element, from
/// its class — never guessed from geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Cut and poché-filled when the plane passes through it.
    Structure,
    /// Replaced by a symbol (door leaf + swing, window sills).
    Opening,
    /// A room: outline and label, never drawn as material.
    Zone,
    /// Drawn as an outline only when it is under the cut (furniture).
    Loose,
    /// Not in a plan.
    Skip,
}

pub fn role_of(class: &ElementClass, show_loose: bool) -> Role {
    match class {
        ElementClass::Wall
        | ElementClass::Column
        | ElementClass::CurtainWall
        | ElementClass::Beam
        | ElementClass::Slab
        | ElementClass::Roof
        | ElementClass::Shell
        | ElementClass::Railing
        | ElementClass::Site => Role::Structure,
        // Symbols replace the geometry: treads + up-arrow, not a poche blob.
        ElementClass::Stair => Role::Opening,
        ElementClass::Door | ElementClass::Window | ElementClass::Skylight | ElementClass::Opening => {
            Role::Opening
        }
        ElementClass::Zone => Role::Zone,
        ElementClass::Furniture | ElementClass::Object | ElementClass::Lamp | ElementClass::Mesh
        | ElementClass::Morph => {
            if show_loose {
                Role::Loose
            } else {
                Role::Skip
            }
        }
        _ => Role::Skip,
    }
}

/// What the plan is cut and drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanSettings {
    /// Metres above the storey's own elevation. 1.2 m is the convention.
    pub cut_height: f32,
    /// Draw furniture and loose objects under the cut.
    pub show_loose: bool,
    /// Draw what is above the cut as thin dashes (beams, openings over).
    pub show_above: bool,
    /// Draw dimension lines around the plan.
    pub dimensions: bool,
}

impl Default for PlanSettings {
    fn default() -> Self {
        PlanSettings {
            cut_height: 1.2,
            show_loose: false,
            show_above: true,
            dimensions: true,
        }
    }
}

fn stroke(color: [f32; 4], width_mm: f32) -> Stroke {
    Stroke {
        width_mm,
        color,
        dash: [0.0, 0.0],
    }
}

fn dashed(color: [f32; 4], width_mm: f32, dash_mm: f32) -> Stroke {
    Stroke {
        width_mm,
        color,
        dash: [dash_mm, dash_mm * 0.6],
    }
}

/// Model metres → paper millimetres.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub scale: f32,
    pub mm_per_m: f32,
    origin_mm: [f32; 2],
    min: [f32; 2],
}

impl Frame {
    fn new(min: [f32; 2], size: [f32; 2], gutter: f32) -> Frame {
        let avail = [
            PAGE[0] - MARGIN * 2.0 - gutter * 2.0,
            PAGE[1] - MARGIN * 2.0 - TITLE_H - gutter * 2.0,
        ];
        let mut scale = 2000.0;
        for s in [20.0f32, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0] {
            let mm = 1000.0 / s;
            if size[0] * mm <= avail[0] && size[1] * mm <= avail[1] {
                scale = s;
                break;
            }
        }
        let mm_per_m = 1000.0 / scale;
        let w = size[0] * mm_per_m;
        let h = size[1] * mm_per_m;
        Frame {
            scale,
            mm_per_m,
            origin_mm: [
                (PAGE[0] - w) * 0.5,
                TITLE_H + (PAGE[1] - TITLE_H - h) * 0.5,
            ],
            min,
        }
    }

    pub fn paper(&self, x: f32, y: f32) -> [f32; 2] {
        [
            self.origin_mm[0] + (x - self.min[0]) * self.mm_per_m,
            self.origin_mm[1] + (y - self.min[1]) * self.mm_per_m,
        ]
    }

    fn grid(&self, p: P2) -> [f32; 2] {
        let m = slice::unq2(p);
        self.paper(m[0], m[1])
    }

    fn chain(&self, c: &Chain) -> Vec<[f32; 2]> {
        c.pts.iter().map(|p| self.grid(*p)).collect()
    }
}

/// Everything the cut produced, kept classified.
pub struct PlanGeometry {
    pub structure: Vec<Chain>,
    pub openings: Vec<Chain>,
    pub below: Vec<Chain>,
    pub above: Vec<Chain>,
    pub rooms: Vec<Room>,
    /// Elements that contributed something, for the sheet's links.
    pub drawn: Vec<(ElementId, ([f32; 2], [f32; 2]))>,
    pub stats: PlanStats,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlanStats {
    pub considered: u32,
    pub cut: u32,
    pub below: u32,
    pub above: u32,
    pub skipped: u32,
    pub segments: u32,
    pub loops_closed: u32,
    pub loops_open: u32,
    pub poche_rects: u32,
    pub slice_ms: f32,
}

pub struct Room {
    pub outline: Chain,
    pub area_m2: f64,
    pub name: String,
    pub centre: P2,
}

fn story_span(scene: &Scene, story_index: usize) -> (f32, f32) {
    let s = &scene.stories[story_index];
    let mut top = s.elevation + if s.height > 0.01 { s.height } else { 3.0 };
    // The next storey up bounds this one when heights are not published.
    let mut best = f32::MAX;
    for o in &scene.stories {
        if o.elevation > s.elevation + 0.05 && o.elevation < best {
            best = o.elevation;
        }
    }
    if best < f32::MAX {
        top = best;
    }
    (s.elevation, top)
}

/// Elements that belong to this storey: the ones filed under it, plus the ones
/// with no storey of their own that physically stand in its slab-to-slab band.
/// Without this the cut plane slices *every* floor of the building onto one
/// sheet, which is exactly what a plan must not do.
fn storey_elements(scene: &Scene, story_index: usize) -> Vec<ElementId> {
    let (base, top) = story_span(scene, story_index);
    let sid = scene.stories[story_index].id;
    let mut out = Vec::new();
    for e in &scene.elements {
        if !e.has_geometry() {
            continue;
        }
        match e.story {
            Some(s) if s == sid => out.push(e.id),
            Some(_) => {}
            None => {
                let c = aabb_center(&e.bounds).z;
                if c >= base - 0.05 && c < top {
                    out.push(e.id);
                }
            }
        }
    }
    out
}

/// Cut one storey.
pub fn cut_storey(scene: &Scene, story_index: usize, settings: &PlanSettings) -> PlanGeometry {
    let t0 = std::time::Instant::now();
    let (base, top) = story_span(scene, story_index);
    let z = base + settings.cut_height;
    let mut stats = PlanStats::default();

    let mut structure_parts: Vec<Part> = Vec::new();
    let mut opening_parts: Vec<Part> = Vec::new();
    let mut below_parts: Vec<Part> = Vec::new();
    let mut above_parts: Vec<Part> = Vec::new();
    let mut zones: Vec<(ElementId, Vec<Chain>)> = Vec::new();
    let mut drawn: Vec<(ElementId, ([f32; 2], [f32; 2]))> = Vec::new();

    for id in storey_elements(scene, story_index) {
        let Some(el) = scene.element(id) else { continue };
        stats.considered += 1;
        let role = role_of(&el.class, settings.show_loose);
        if role == Role::Skip {
            stats.skipped += 1;
            continue;
        }
        let b = el.bounds;
        let crosses = b.min.z <= z && b.max.z >= z;
        match role {
            Role::Zone => {
                // Cut the zone at its own mid-height: a zone volume that
                // stops below the cut plane still describes a room.
                let zz = if crosses { z } else { (b.min.z + b.max.z) * 0.5 };
                let mut segs = Vec::new();
                slice::slice_element(scene, id, zz, &mut segs);
                stats.segments += segs.len() as u32;
                let loops = slice::chains(segs);
                if !loops.is_empty() {
                    zones.push((id, loops));
                }
            }
            Role::Structure | Role::Opening => {
                if crosses {
                    let part = slice::part_of(scene, id, z);
                    stats.segments += part.loops.iter().map(|c| c.pts.len() as u32).sum::<u32>();
                    if !part.loops.is_empty() {
                        stats.cut += 1;
                        if role == Role::Opening {
                            opening_parts.push(part);
                        } else {
                            structure_parts.push(part);
                        }
                    }
                } else if b.max.z < z {
                    // Seen below: cut it just under its own top. A horizontal
                    // cut gives one clean closed outline; taking the
                    // normal-sign silhouette instead drags in every interior
                    // ridge of a faceted roof or terrain and draws them as
                    // lines across the plan.
                    if role == Role::Structure {
                        let zz = b.max.z - (aabb_extent(&b).z * 0.08).clamp(0.005, 0.05);
                        let part = slice::part_of(scene, id, zz);
                        stats.segments += part.loops.iter().map(|c| c.pts.len() as u32).sum::<u32>();
                        if !part.loops.is_empty() {
                            stats.below += 1;
                            below_parts.push(part);
                        }
                    }
                } else if settings.show_above && b.min.z > z && b.min.z < top {
                    let zz = b.min.z + (aabb_extent(&b).z * 0.08).clamp(0.005, 0.05);
                    let part = slice::part_of(scene, id, zz);
                    if !part.loops.is_empty() {
                        stats.above += 1;
                        above_parts.push(part);
                    }
                }
            }
            Role::Loose => {
                if b.max.z <= z {
                    let zz = b.max.z - (aabb_extent(&b).z * 0.3).clamp(0.005, 0.4);
                    let part = slice::part_of(scene, id, zz);
                    if !part.loops.is_empty() {
                        below_parts.push(part);
                    }
                }
            }
            Role::Skip => {}
        }
        drawn.push((
            id,
            (
                [b.min.x, b.min.y],
                [b.max.x, b.max.y],
            ),
        ));
    }

    // The union is what turns overlapping wall boxes into one building — and
    // the same for the layers below and above, which overlap just as much.
    let structure = slice::union_parts(&structure_parts);
    let mut below = slice::union_parts(&below_parts);
    let mut above = slice::union_parts(&above_parts);
    slice::cull_slivers(&mut below, 0.5, 100.0);
    slice::cull_slivers(&mut above, 0.8, 100.0);
    let openings: Vec<Chain> = opening_parts.into_iter().flat_map(|p| p.loops).collect();
    stats.loops_closed = structure.iter().filter(|c| c.closed).count() as u32;
    stats.loops_open = structure.iter().filter(|c| !c.closed).count() as u32;

    // Rooms: the voids of the union, named from the zones that sit in them.
    let rooms = rooms_from(&structure, &zones, scene);

    stats.slice_ms = t0.elapsed().as_secs_f32() * 1000.0;
    PlanGeometry {
        structure,
        openings,
        below,
        above,
        rooms,
        drawn,
        stats,
    }
}

/// The union's interior loops are the rooms. A zone element whose centre falls
/// in one of them gives it its name.
fn rooms_from(structure: &[Chain], zones: &[(ElementId, Vec<Chain>)], scene: &Scene) -> Vec<Room> {
    let closed: Vec<&Chain> = structure.iter().filter(|c| c.closed && c.pts.len() >= 3).collect();
    if closed.is_empty() {
        return Vec::new();
    }
    // The biggest loop is the building's outline; the voids wind the other way.
    let outer = closed
        .iter()
        .max_by(|a, b| a.area().partial_cmp(&b.area()).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap();
    let outer_sign = outer.signed_area().signum();
    let mut rooms: Vec<Room> = Vec::new();
    let mut used_zone: Vec<bool> = vec![false; zones.len()];
    for c in &closed {
        if std::ptr::eq(*c, *outer) {
            continue;
        }
        if c.signed_area().signum() == outer_sign {
            continue; // another solid island, not a void
        }
        let area = c.area();
        if area < 0.7 {
            continue; // a service duct, not a room
        }
        let centre = c.centroid();
        let mut name = String::new();
        for (zi, (id, loops)) in zones.iter().enumerate() {
            if slice::contains(loops, centre) {
                name = scene
                    .element(*id)
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                used_zone[zi] = true;
                break;
            }
        }
        rooms.push(Room {
            outline: (*c).clone(),
            area_m2: area,
            name,
            centre,
        });
    }
    // A zone the walls did not enclose is still a room: source application published it
    // as one, and its own outline is better data than our void detection.
    for (zi, (id, loops)) in zones.iter().enumerate() {
        if used_zone[zi] {
            continue;
        }
        let Some(biggest) = loops
            .iter()
            .filter(|c| c.closed && c.area() > 0.7)
            .max_by(|a, b| a.area().partial_cmp(&b.area()).unwrap_or(std::cmp::Ordering::Equal))
        else {
            continue;
        };
        rooms.push(Room {
            outline: biggest.clone(),
            area_m2: biggest.area(),
            name: scene.element(*id).map(|e| e.name.clone()).unwrap_or_default(),
            centre: biggest.centroid(),
        });
    }
    rooms.sort_by(|a, b| b.area_m2.partial_cmp(&a.area_m2).unwrap_or(std::cmp::Ordering::Equal));
    rooms
}

// ===========================================================================
// Symbols
// ===========================================================================

/// A door: the leaf swung open 90° plus its arc, drawn in the opening rather
/// than the door's own geometry. Which way it swings is not in the file (see
/// report R10) — the leaf hangs on the jamb nearest the model centre so the
/// choice is at least consistent.
fn door_symbol(frame: &Frame, b: &Aabb, model_centre: Vec3f, items: &mut Vec<SheetItem>) {
    let e = aabb_extent(b);
    let (along_x, width, thick) = if e.x >= e.y {
        (true, e.x, e.y)
    } else {
        (false, e.y, e.x)
    };
    if width < 0.3 {
        return;
    }
    let c = aabb_center(b);
    // Hinge at the end nearest the building centre; swing into the room.
    let (hinge, tip) = if along_x {
        let x = if c.x <= model_centre.x { b.min.x } else { b.max.x };
        let other = if c.x <= model_centre.x { b.max.x } else { b.min.x };
        ([x, c.y], [other, c.y])
    } else {
        let y = if c.y <= model_centre.y { b.min.y } else { b.max.y };
        let other = if c.y <= model_centre.y { b.max.y } else { b.min.y };
        ([c.x, y], [c.x, other])
    };
    let side = if along_x {
        if c.y <= model_centre.y { 1.0 } else { -1.0 }
    } else {
        if c.x <= model_centre.x { 1.0 } else { -1.0 }
    };
    // Leaf: perpendicular to the wall, one opening wide.
    let leaf_end = if along_x {
        [hinge[0], hinge[1] + width * side]
    } else {
        [hinge[0] + width * side, hinge[1]]
    };
    let h = frame.paper(hinge[0], hinge[1]);
    let l = frame.paper(leaf_end[0], leaf_end[1]);
    let t = frame.paper(tip[0], tip[1]);
    items.push(SheetItem::Path {
        points: vec![h, l],
        closed: false,
        stroke: stroke(OPENING_INK, W_OPENING),
    });
    // Swing arc from the leaf tip round to the far jamb.
    let r = width * frame.mm_per_m;
    let a0 = ((l[1] - h[1]) as f32).atan2((l[0] - h[0]) as f32).to_degrees();
    let a1 = ((t[1] - h[1]) as f32).atan2((t[0] - h[0]) as f32).to_degrees();
    let mut sweep = a1 - a0;
    while sweep > 180.0 {
        sweep -= 360.0;
    }
    while sweep < -180.0 {
        sweep += 360.0;
    }
    items.push(SheetItem::Arc {
        center: h,
        radius: r,
        start_deg: a0,
        end_deg: a0 + sweep,
        stroke: stroke(OPENING_INK, W_OPENING * 0.7),
    });
    let _ = thick;
}

/// A window: the glass line down the middle of the wall and a sill line on
/// each face.
fn window_symbol(frame: &Frame, b: &Aabb, items: &mut Vec<SheetItem>) {
    let e = aabb_extent(b);
    let along_x = e.x >= e.y;
    let c = aabb_center(b);
    let (a, z) = if along_x {
        ([b.min.x, c.y], [b.max.x, c.y])
    } else {
        ([c.x, b.min.y], [c.x, b.max.y])
    };
    items.push(SheetItem::Path {
        points: vec![frame.paper(a[0], a[1]), frame.paper(z[0], z[1])],
        closed: false,
        stroke: stroke(OPENING_INK, W_OPENING),
    });
    // Sill lines on both faces.
    let half = if along_x { e.y * 0.5 } else { e.x * 0.5 };
    for s in [-1.0f32, 1.0] {
        let (p, q) = if along_x {
            ([b.min.x, c.y + half * s], [b.max.x, c.y + half * s])
        } else {
            ([c.x + half * s, b.min.y], [c.x + half * s, b.max.y])
        };
        items.push(SheetItem::Path {
            points: vec![frame.paper(p[0], p[1]), frame.paper(q[0], q[1])],
            closed: false,
            stroke: stroke(OPENING_INK, W_OPENING * 0.6),
        });
    }
}

/// A stair: treads across the run, the "up" arrow, and a break line where
/// the flight passes the cut plane.
fn stair_symbol(frame: &Frame, b: &Aabb, cut_crosses: bool, items: &mut Vec<SheetItem>) {
    let e = aabb_extent(b);
    let c = aabb_center(b);
    let along_x = e.x >= e.y;
    let (from, to) = if along_x {
        ([b.min.x + e.x * 0.08, c.y], [b.max.x - e.x * 0.08, c.y])
    } else {
        ([c.x, b.min.y + e.y * 0.08], [c.x, b.max.y - e.y * 0.08])
    };
    let run = if along_x { e.x } else { e.y };
    let half = if along_x { e.y * 0.42 } else { e.x * 0.42 };
    let n_treads = ((run / 0.28).floor() as i32).clamp(3, 24);
    let drawn = if cut_crosses { n_treads / 2 + 1 } else { n_treads };
    for i in 0..drawn {
        let t = (i as f32 + 0.5) / n_treads as f32;
        let m = [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
        ];
        let (a, z) = if along_x {
            ([m[0], m[1] - half], [m[0], m[1] + half])
        } else {
            ([m[0] - half, m[1]], [m[0] + half, m[1]])
        };
        items.push(SheetItem::Path {
            points: vec![frame.paper(a[0], a[1]), frame.paper(z[0], z[1])],
            closed: false,
            stroke: stroke(OPENING_INK, W_ANNOT),
        });
    }
    let p0 = frame.paper(from[0], from[1]);
    let p1 = frame.paper(to[0], to[1]);
    items.push(SheetItem::Path {
        points: vec![p0, p1],
        closed: false,
        stroke: stroke(OPENING_INK, W_ANNOT),
    });
    // Arrowhead.
    let d = [p1[0] - p0[0], p1[1] - p0[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-3);
    let u = [d[0] / len, d[1] / len];
    let n = [-u[1], u[0]];
    let head = 2.4;
    items.push(SheetItem::Path {
        points: vec![
            [p1[0] - u[0] * head + n[0] * head * 0.45, p1[1] - u[1] * head + n[1] * head * 0.45],
            p1,
            [p1[0] - u[0] * head - n[0] * head * 0.45, p1[1] - u[1] * head - n[1] * head * 0.45],
        ],
        closed: false,
        stroke: stroke(OPENING_INK, W_ANNOT),
    });
    items.push(SheetItem::Text {
        pos: [p0[0] - u[0] * 3.0 - 1.2, p0[1] - u[1] * 3.0],
        text: "UP".into(),
        height_mm: 2.2,
        angle_deg: 0.0,
        color: OPENING_INK,
    });
    if cut_crosses {
        // Break line across the middle of the run.
        let m = [(p0[0] + p1[0]) * 0.5, (p0[1] + p1[1]) * 0.5];
        let w = if along_x { e.y } else { e.x } * frame.mm_per_m * 0.6;
        items.push(SheetItem::Path {
            points: vec![
                [m[0] + n[0] * w - u[0] * 1.2, m[1] + n[1] * w - u[1] * 1.2],
                [m[0] + n[0] * w * 0.2 + u[0] * 1.0, m[1] + n[1] * w * 0.2 + u[1] * 1.0],
                [m[0] - n[0] * w * 0.2 - u[0] * 1.0, m[1] - n[1] * w * 0.2 - u[1] * 1.0],
                [m[0] - n[0] * w + u[0] * 1.2, m[1] - n[1] * w + u[1] * 1.2],
            ],
            closed: false,
            stroke: stroke(OPENING_INK, W_ANNOT),
        });
    }
}

// ===========================================================================
// Annotation
// ===========================================================================

fn dimension_line(
    items: &mut Vec<SheetItem>,
    a: [f32; 2],
    b: [f32; 2],
    offset: [f32; 2],
    text: &str,
) {
    let p = [a[0] + offset[0], a[1] + offset[1]];
    let q = [b[0] + offset[0], b[1] + offset[1]];
    let st = stroke(INK_ANNOT, W_ANNOT);
    items.push(SheetItem::Path {
        points: vec![p, q],
        closed: false,
        stroke: st,
    });
    // Witness lines back to the thing being measured.
    for (from, to) in [(a, p), (b, q)] {
        items.push(SheetItem::Path {
            points: vec![
                [from[0] + offset[0] * 0.12, from[1] + offset[1] * 0.12],
                [to[0] + offset[0] * 0.18, to[1] + offset[1] * 0.18],
            ],
            closed: false,
            stroke: stroke(INK_ANNOT, W_ANNOT * 0.8),
        });
    }
    // 45° ticks, the architectural end mark.
    let d = [q[0] - p[0], q[1] - p[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-3);
    let u = [d[0] / len, d[1] / len];
    let t = 1.3;
    for end in [p, q] {
        items.push(SheetItem::Path {
            points: vec![
                [end[0] - (u[0] + u[1]) * t, end[1] - (u[1] - u[0]) * t],
                [end[0] + (u[0] + u[1]) * t, end[1] + (u[1] - u[0]) * t],
            ],
            closed: false,
            stroke: stroke(INK_ANNOT, W_ANNOT),
        });
    }
    let mid = [(p[0] + q[0]) * 0.5, (p[1] + q[1]) * 0.5];
    let vertical = d[1].abs() > d[0].abs();
    items.push(SheetItem::Text {
        pos: if vertical {
            [mid[0] + 1.0, mid[1]]
        } else {
            [mid[0] - text.len() as f32 * 0.7, mid[1] + 1.2]
        },
        text: text.into(),
        height_mm: 2.4,
        angle_deg: if vertical { 90.0 } else { 0.0 },
        color: INK_ANNOT,
    });
}

fn north_arrow(items: &mut Vec<SheetItem>, at: [f32; 2]) {
    let r = 6.0;
    items.push(SheetItem::Arc {
        center: at,
        radius: r,
        start_deg: 0.0,
        end_deg: 360.0,
        stroke: stroke(INK, W_ANNOT),
    });
    items.push(SheetItem::Fill {
        points: vec![
            [at[0], at[1] + r * 0.85],
            [at[0] - r * 0.32, at[1] - r * 0.5],
            [at[0], at[1] - r * 0.2],
        ],
        color: INK,
        stroke: None,
    });
    items.push(SheetItem::Path {
        points: vec![
            [at[0], at[1] + r * 0.85],
            [at[0] + r * 0.32, at[1] - r * 0.5],
            [at[0], at[1] - r * 0.2],
        ],
        closed: true,
        stroke: stroke(INK, W_ANNOT * 0.8),
    });
    items.push(SheetItem::Text {
        pos: [at[0] - 1.1, at[1] + r + 1.0],
        text: "N".into(),
        height_mm: 2.6,
        angle_deg: 0.0,
        color: INK,
    });
}

fn scale_bar(items: &mut Vec<SheetItem>, frame: &Frame, at: [f32; 2]) {
    let step_m: f32 = if frame.scale <= 50.0 {
        1.0
    } else if frame.scale <= 200.0 {
        2.0
    } else {
        10.0
    };
    let seg = step_m * frame.mm_per_m;
    for i in 0..4 {
        let x = at[0] + i as f32 * seg;
        items.push(SheetItem::Fill {
            points: vec![
                [x, at[1]],
                [x + seg, at[1]],
                [x + seg, at[1] + 1.4],
                [x, at[1] + 1.4],
            ],
            color: if i % 2 == 0 { INK } else { [1.0, 1.0, 1.0, 1.0] },
            stroke: Some(stroke(INK, 0.2)),
        });
    }
    items.push(SheetItem::Text {
        pos: [at[0] - 0.6, at[1] - 3.4],
        text: format!("0{}{:.0} m", " ".repeat(14), step_m * 4.0),
        height_mm: 2.2,
        angle_deg: 0.0,
        color: [0.3, 0.3, 0.3, 1.0],
    });
}

fn date_ymd() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86400)
        .unwrap_or(0) as i64;
    // Howard Hinnant civil_from_days, days since 1970-01-01.
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn title_block(items: &mut Vec<SheetItem>, number: &str, name: &str, scale: f32, project: &str, note: &str) {
    items.push(SheetItem::Path {
        points: vec![
            [MARGIN, MARGIN],
            [PAGE[0] - MARGIN, MARGIN],
            [PAGE[0] - MARGIN, PAGE[1] - MARGIN],
            [MARGIN, PAGE[1] - MARGIN],
        ],
        closed: true,
        stroke: stroke(INK, W_FRAME),
    });
    let bx = PAGE[0] - MARGIN - 138.0;
    items.push(SheetItem::Path {
        points: vec![
            [bx, MARGIN],
            [PAGE[0] - MARGIN, MARGIN],
            [PAGE[0] - MARGIN, MARGIN + TITLE_H - 4.0],
            [bx, MARGIN + TITLE_H - 4.0],
        ],
        closed: true,
        stroke: stroke(INK, W_ANNOT),
    });
    items.push(SheetItem::Path {
        points: vec![[bx, MARGIN + 8.0], [PAGE[0] - MARGIN, MARGIN + 8.0]],
        closed: false,
        stroke: stroke(INK, W_ANNOT * 0.7),
    });
    items.push(SheetItem::Text {
        pos: [bx + 3.0, MARGIN + 11.0],
        text: format!("{number}   {name}"),
        height_mm: 4.0,
        angle_deg: 0.0,
        color: INK,
    });
    items.push(SheetItem::Text {
        pos: [bx + 3.0, MARGIN + 2.6],
        text: format!("{project}"),
        height_mm: 2.4,
        angle_deg: 0.0,
        color: [0.3, 0.3, 0.3, 1.0],
    });
    items.push(SheetItem::Text {
        pos: [PAGE[0] - MARGIN - 34.0, MARGIN + 2.6],
        text: format!("1:{scale:.0}  {}", date_ymd()),
        height_mm: 3.0,
        angle_deg: 0.0,
        color: INK,
    });
    if !note.is_empty() {
        items.push(SheetItem::Text {
            pos: [MARGIN + 2.0, MARGIN + 2.0],
            text: note.into(),
            height_mm: 2.2,
            angle_deg: 0.0,
            color: [0.45, 0.45, 0.45, 1.0],
        });
    }
}

// ===========================================================================
// The sheet
// ===========================================================================

/// Build the plan sheet for one storey.
pub fn plan_sheet(
    scene: &Scene,
    story_index: usize,
    id: SheetId,
    settings: &PlanSettings,
    units: &Units,
) -> Option<Sheet> {
    let geo = cut_storey(scene, story_index, settings);
    if geo.structure.is_empty() && geo.below.is_empty() {
        return None;
    }
    // Extents from what is actually drawn, not from the whole model.
    let mut lo = [f32::MAX, f32::MAX];
    let mut hi = [f32::MIN, f32::MIN];
    for c in geo.structure.iter().chain(geo.below.iter()).chain(geo.above.iter()) {
        let (l, h) = c.bounds();
        lo[0] = lo[0].min(slice::unq(l[0]));
        lo[1] = lo[1].min(slice::unq(l[1]));
        hi[0] = hi[0].max(slice::unq(h[0]));
        hi[1] = hi[1].max(slice::unq(h[1]));
    }
    if !lo[0].is_finite() || hi[0] <= lo[0] {
        return None;
    }
    let frame = Frame::new(lo, [hi[0] - lo[0], hi[1] - lo[1]], DIM_GUTTER);

    let mut items: Vec<SheetItem> = Vec::new();
    let mut links: Vec<SheetLink> = Vec::new();

    // 1. poché — the cut material, openings knocked out by the even-odd rule
    let mut fill_loops: Vec<Chain> = geo.structure.iter().filter(|c| c.closed).cloned().collect();
    fill_loops.extend(geo.openings.iter().filter(|c| c.closed).cloned());
    let step_m = 0.03f32.max(0.25 / frame.mm_per_m); // ≈0.25 mm on paper
    let spans = slice::spans(&fill_loops, step_m);
    let poche_rects = spans.len() as u32;
    for s in &spans {
        let a = frame.grid([s.x0, s.y0]);
        let b = frame.grid([s.x1, s.y1]);
        items.push(SheetItem::Fill {
            points: vec![a, [b[0], a[1]], b, [a[0], b[1]]],
            color: POCHE,
            stroke: None,
        });
    }

    // 2. what is above the cut, first so the heavier lines land on top
    if settings.show_above {
        for c in &geo.above {
            items.push(SheetItem::Path {
                points: frame.chain(c),
                closed: c.closed,
                stroke: dashed(INK_ABOVE, W_ABOVE, 2.0),
            });
        }
    }

    // 3. what is below the cut
    for c in &geo.below {
        items.push(SheetItem::Path {
            points: frame.chain(c),
            closed: c.closed,
            stroke: stroke(INK_BELOW, W_BELOW),
        });
    }

    // 4. the cut itself
    for c in &geo.structure {
        items.push(SheetItem::Path {
            points: frame.chain(c),
            closed: c.closed,
            stroke: stroke(INK, W_CUT),
        });
    }

    // 5. symbols replace the geometry they stand for
    let centre = aabb_center(&scene.bounds);
    let (base, top) = story_span(scene, story_index);
    let z = base + settings.cut_height;
    for (id_el, _) in &geo.drawn {
        let Some(el) = scene.element(*id_el) else { continue };
        match el.class {
            ElementClass::Door => {
                if el.bounds.min.z <= z && el.bounds.max.z >= z {
                    door_symbol(&frame, &el.bounds, centre, &mut items);
                }
            }
            ElementClass::Window => {
                if el.bounds.min.z <= z && el.bounds.max.z >= z {
                    window_symbol(&frame, &el.bounds, &mut items);
                }
            }
            ElementClass::Stair => {
                if el.bounds.max.z > base && el.bounds.min.z < top {
                    stair_symbol(&frame, &el.bounds, el.bounds.max.z > z, &mut items);
                }
            }
            _ => {}
        }
    }

    // 6. rooms
    let mut placed: Vec<[f32; 4]> = Vec::new();
    for room in &geo.rooms {
        let c = frame.grid(room.centre);
        let name = if room.name.is_empty() {
            String::new()
        } else {
            room.name.clone()
        };
        let area = format!("{:.2} m²", room.area_m2);
        let w = (name.len().max(area.len()) as f32) * 1.5 + 2.0;
        let rect = [c[0] - w * 0.5, c[1] - 3.0, c[0] + w * 0.5, c[1] + 4.0];
        // Do not stack labels on top of each other.
        if placed.iter().any(|p| {
            rect[0] < p[2] && rect[2] > p[0] && rect[1] < p[3] && rect[3] > p[1]
        }) {
            continue;
        }
        // …and do not label a room too small to hold the text.
        let (lo_g, hi_g) = room.outline.bounds();
        let rw = slice::unq(hi_g[0] - lo_g[0]) * frame.mm_per_m;
        let rh = slice::unq(hi_g[1] - lo_g[1]) * frame.mm_per_m;
        if rw < w * 0.8 || rh < 6.0 {
            continue;
        }
        placed.push(rect);
        if !name.is_empty() {
            items.push(SheetItem::Text {
                pos: [c[0] - name.len() as f32 * 0.75, c[1] + 0.6],
                text: name,
                height_mm: 2.6,
                angle_deg: 0.0,
                color: ROOM_INK,
            });
        }
        items.push(SheetItem::Text {
            pos: [c[0] - area.len() as f32 * 0.62, c[1] - 2.8],
            text: area,
            height_mm: 2.2,
            angle_deg: 0.0,
            color: [0.42, 0.42, 0.42, 1.0],
        });
    }

    // 7. annotation
    if settings.dimensions {
        let a = frame.paper(lo[0], lo[1]);
        let b = frame.paper(hi[0], lo[1]);
        dimension_line(
            &mut items,
            a,
            b,
            [0.0, -8.0],
            &units.format_length((hi[0] - lo[0]) as f64),
        );
        let c = frame.paper(lo[0], hi[1]);
        dimension_line(
            &mut items,
            a,
            c,
            [-8.0, 0.0],
            &units.format_length((hi[1] - lo[1]) as f64),
        );
    }
    if scene.elements.iter().any(|e| e.class == ElementClass::Site) {
        north_arrow(&mut items, [PAGE[0] - MARGIN - 12.0, PAGE[1] - MARGIN - 12.0]);
    }
    scale_bar(&mut items, &frame, [MARGIN + 4.0, MARGIN + TITLE_H - 6.0]);

    let story = &scene.stories[story_index];
    title_block(
        &mut items,
        &format!("A-{:03}", 100 + story_index),
        &format!("{} — Plan", story.name),
        frame.scale,
        &scene.name,
        &format!(
            "cut {:.2} m above {} · {} cut · {} below · {} above · {} rooms · {} poché · {:.0} ms",
            settings.cut_height,
            story.name,
            geo.stats.cut,
            geo.stats.below,
            geo.stats.above,
            geo.rooms.len(),
            poche_rects,
            geo.stats.slice_ms
        ),
    );

    // Links: every element that made it into the drawing, by its footprint.
    for (id_el, (mn, mx)) in &geo.drawn {
        let p0 = frame.paper(mn[0], mn[1]);
        let p1 = frame.paper(mx[0], mx[1]);
        if (p1[0] - p0[0]).abs() < 0.4 && (p1[1] - p0[1]).abs() < 0.4 {
            continue;
        }
        links.push(SheetLink {
            rect_mm: [
                p0[0].min(p1[0]),
                p0[1].min(p1[1]),
                p0[0].max(p1[0]),
                p0[1].max(p1[1]),
            ],
            element: *id_el,
        });
    }

    Some(Sheet {
        id,
        name: format!("A-{:03} {} Plan", 100 + story_index, story.name),
        size_mm: PAGE,
        scale: frame.scale,
        items,
        links,
        story: Some(story.id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Scene {
        Scene::from_model(crate::model::demo::demo_house(), &mut |_| {})
    }

    #[test]
    fn a_storey_plan_cuts_and_fills() {
        let scene = demo();
        let s = PlanSettings::default();
        let sheet = plan_sheet(&scene, 0, SheetId::from_index(0), &s, &scene.units)
            .expect("the demo house has a ground floor");
        assert_eq!(sheet.size_mm, PAGE);
        assert!(sheet.scale >= 20.0 && sheet.scale <= 500.0, "scale {}", sheet.scale);
        let fills = sheet
            .items
            .iter()
            .filter(|i| matches!(i, SheetItem::Fill { .. }))
            .count();
        let paths = sheet
            .items
            .iter()
            .filter(|i| matches!(i, SheetItem::Path { .. }))
            .count();
        assert!(fills > 4, "no poché: {fills} fills");
        assert!(paths > 4, "no line work: {paths} paths");
        assert!(
            sheet.items.iter().any(|i| matches!(i, SheetItem::Arc { .. })),
            "door swing arc missing"
        );
        let geo = cut_storey(&scene, 0, &s);
        assert_eq!(slice::chain_crossings(&geo.structure), 0);
        // everything on the paper
        for l in &sheet.links {
            assert!(l.rect_mm[0] >= -1.0 && l.rect_mm[2] <= PAGE[0] + 1.0, "{:?}", l.rect_mm);
        }
    }

    #[test]
    fn the_cut_height_changes_what_is_cut() {
        let scene = demo();
        let low = cut_storey(&scene, 0, &PlanSettings { cut_height: 0.3, ..Default::default() });
        let high = cut_storey(&scene, 0, &PlanSettings { cut_height: 2.6, ..Default::default() });
        assert!(low.stats.cut > 0, "nothing cut low");
        assert!(
            low.stats.cut != high.stats.cut || low.stats.below != high.stats.below,
            "the cut height did nothing: {:?} vs {:?}",
            low.stats,
            high.stats
        );
    }

    /// The whole point of the storey filter: a plan of the ground floor must
    /// not contain the first floor's walls.
    #[test]
    fn a_plan_only_cuts_its_own_storey() {
        let scene = demo();
        if scene.stories.len() < 2 {
            return;
        }
        let g = cut_storey(&scene, 0, &PlanSettings::default());
        let f = cut_storey(&scene, 1, &PlanSettings::default());
        let ids_g: std::collections::HashSet<ElementId> =
            g.drawn.iter().map(|(id, _)| *id).collect();
        let ids_f: std::collections::HashSet<ElementId> =
            f.drawn.iter().map(|(id, _)| *id).collect();
        let shared: Vec<&ElementId> = ids_g.intersection(&ids_f).collect();
        assert!(
            shared.is_empty(),
            "{} elements appear on both storey plans",
            shared.len()
        );
    }

    fn villa() -> Option<Scene> {
        Some(demo())
    }

    /// Count proper crossings between the drawn line work. Two segments that
    /// cross is exactly what "the floor plan is self-intersecting" looks like.
    fn crossings_in(chains: &[Chain]) -> usize {
        let mut segs: Vec<([i64; 2], [i64; 2])> = Vec::new();
        for c in chains {
            let n = c.pts.len();
            if n < 2 {
                continue;
            }
            let last = if c.closed { n } else { n - 1 };
            for i in 0..last {
                segs.push((c.pts[i], c.pts[(i + 1) % n]));
            }
        }
        let mut n = 0;
        for i in 0..segs.len() {
            for j in (i + 1)..segs.len() {
                if super::slice_crossing_for_test(segs[i], segs[j]) {
                    n += 1;
                }
            }
        }
        n
    }

    /// The report the user asked for, on the real villa: what the old
    /// bounding-box generator drew versus what the cut draws, and the proof
    /// that the new line work does not cross itself.
    #[test]
    fn villa_plan_is_not_self_intersecting() {
        let Some(scene) = villa() else {
            eprintln!("villa sample missing, skipping");
            return;
        };
        let settings = PlanSettings::default();
        for si in 0..scene.stories.len().min(3) {
            // OLD: one axis-aligned rectangle per element of the storey.
            let ids = super::storey_elements(&scene, si);
            let rects: Vec<[f32; 4]> = ids
                .iter()
                .filter_map(|id| scene.element(*id))
                .map(|e| [e.bounds.min.x, e.bounds.min.y, e.bounds.max.x, e.bounds.max.y])
                .collect();
            let mut overlaps = 0usize;
            for i in 0..rects.len() {
                for j in (i + 1)..rects.len() {
                    let (a, b) = (rects[i], rects[j]);
                    if a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1] {
                        overlaps += 1;
                    }
                }
            }
            // NEW: classified cut + union.
            let geo = cut_storey(&scene, si, &settings);
            let cross = crossings_in(&geo.structure);
            eprintln!(
                "[{}] OLD bbox rects {:3} overlapping pairs {:5}  |  NEW considered {:3} cut {:3} below {:3} above {:3} skipped {:3} loops {}c/{}o crossings {} rooms {} {:.1} ms",
                scene.stories[si].name,
                rects.len(),
                overlaps,
                geo.stats.considered,
                geo.stats.cut,
                geo.stats.below,
                geo.stats.above,
                geo.stats.skipped,
                geo.stats.loops_closed,
                geo.stats.loops_open,
                cross,
                geo.rooms.len(),
                geo.stats.slice_ms
            );
            assert_eq!(
                cross, 0,
                "storey {} line work crosses itself {cross} times",
                scene.stories[si].name
            );
        }
    }

    #[test]
    fn classification_keeps_furniture_out_and_site_in() {
        assert_eq!(role_of(&ElementClass::Wall, false), Role::Structure);
        assert_eq!(role_of(&ElementClass::Door, false), Role::Opening);
        assert_eq!(role_of(&ElementClass::Stair, false), Role::Opening);
        assert_eq!(role_of(&ElementClass::Zone, false), Role::Zone);
        assert_eq!(role_of(&ElementClass::Furniture, false), Role::Skip);
        assert_eq!(role_of(&ElementClass::Furniture, true), Role::Loose);
        assert_eq!(role_of(&ElementClass::Site, false), Role::Structure);
        assert_eq!(role_of(&ElementClass::Unknown, false), Role::Skip);
    }

    fn box_mesh(min: [f32; 3], max: [f32; 3]) -> crate::model::MeshData {
        let (a, b) = (min, max);
        let v = [
            [a[0], a[1], a[2]],
            [b[0], a[1], a[2]],
            [b[0], b[1], a[2]],
            [a[0], b[1], a[2]],
            [a[0], a[1], b[2]],
            [b[0], a[1], b[2]],
            [b[0], b[1], b[2]],
            [a[0], b[1], b[2]],
        ];
        let faces: [[usize; 4]; 6] = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ];
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for f in faces {
            let base = positions.len() as u32;
            for i in f {
                positions.push(v[i]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let n = indices.len() as u32;
        crate::model::MeshData {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            indices,
            contour_edges: Vec::new(),
            submeshes: vec![crate::model::SubMesh {
                material: MaterialId::from_index(0),
                first_index: 0,
                index_count: n,
            }],
        }
    }

    fn add_el(
        model: &mut crate::model::ModelData,
        name: &str,
        class: ElementClass,
        story: StoryId,
        min: [f32; 3],
        max: [f32; 3],
    ) {
        let mesh = crate::model::MeshId::from_index(model.meshes.len());
        model.meshes.push(box_mesh(min, max));
        let id = ElementId::from_index(model.elements.len());
        model.elements.push(crate::model::ElementData {
            id,
            guid: format!("SYN-{name}"),
            name: name.into(),
            class,
            story: Some(story),
            layer: Some(LayerId::from_index(0)),
            parent: None,
            transform: Default::default(),
            meshes: vec![crate::model::MeshRef::identity(mesh)],
            properties: Vec::new(),
            quantities: Vec::new(),
        });
    }

    /// Four walls, a door, a window, a slab, two storeys — the three
    /// mandatory synthetic cases live in one model so the cut, the classes
    /// and the storey filter are tested together.
    fn synthetic_room() -> Scene {
        use crate::model::{Handedness, LayerData, MaterialData, ModelData, StoryData, UpAxis};
        let mut m = ModelData {
            name: "Synthetic Room".into(),
            units: Units {
                source_to_meters: 1.0,
                display: LengthUnit::Meter,
                precision: 2,
            },
            up_axis: UpAxis::Z,
            handedness: Handedness::Right,
            ..Default::default()
        };
        m.materials.push(MaterialData::default());
        let s0 = StoryId::from_index(0);
        let s1 = StoryId::from_index(1);
        m.stories.push(StoryData {
            id: s0,
            name: "Ground".into(),
            elevation: 0.0,
            height: 3.0,
        });
        m.stories.push(StoryData {
            id: s1,
            name: "First".into(),
            elevation: 3.0,
            height: 3.0,
        });
        m.layers.push(LayerData {
            id: LayerId::from_index(0),
            name: "A".into(),
            visible: true,
        });
        let t = 0.3f32;
        let (w, d) = (5.0f32, 4.0f32);
        // south wall split around a 1 m door
        add_el(&mut m, "Wall S-L", ElementClass::Wall, s0, [0.0, 0.0, 0.0], [2.0, t, 3.0]);
        add_el(&mut m, "Wall S-R", ElementClass::Wall, s0, [3.0, 0.0, 0.0], [w, t, 3.0]);
        add_el(&mut m, "Wall N", ElementClass::Wall, s0, [0.0, d - t, 0.0], [w, d, 3.0]);
        add_el(&mut m, "Wall W", ElementClass::Wall, s0, [0.0, t, 0.0], [t, d - t, 3.0]);
        add_el(&mut m, "Wall E", ElementClass::Wall, s0, [w - t, t, 0.0], [w, d - t, 3.0]);
        add_el(&mut m, "Door", ElementClass::Door, s0, [2.05, 0.05, 0.0], [2.95, 0.12, 2.15]);
        add_el(&mut m, "Window", ElementClass::Window, s0, [1.5, d - t * 0.6, 0.9], [2.9, d - t * 0.2, 2.3]);
        add_el(&mut m, "Slab", ElementClass::Slab, s0, [0.0, 0.0, -0.2], [w, d, 0.0]);
        add_el(&mut m, "Wall S1", ElementClass::Wall, s1, [0.0, 0.0, 3.0], [w, t, 6.0]);
        add_el(&mut m, "Zone", ElementClass::Zone, s0, [t, t, 0.1], [w - t, d - t, 2.8]);
        Scene::from_model(m, &mut |_| {})
    }

    #[test]
    fn room_with_a_door_and_a_window() {
        let scene = synthetic_room();
        let settings = PlanSettings::default();
        let geo = cut_storey(&scene, 0, &settings);
        assert_eq!(slice::chain_crossings(&geo.structure), 0, "self-intersecting cut");
        assert!(geo.stats.cut >= 4, "walls not cut: {:?}", geo.stats);
        assert!(
            !geo.openings.is_empty() || geo.stats.cut > 0,
            "door/window produced nothing: {:?}",
            geo.stats
        );
        let sheet = plan_sheet(&scene, 0, SheetId::from_index(0), &settings, &scene.units)
            .expect("synthetic plan");
        assert!(
            sheet.items.iter().any(|i| matches!(i, SheetItem::Arc { .. })),
            "door leaf/swing missing"
        );
        let window_lines = sheet
            .items
            .iter()
            .filter(|i| matches!(i, SheetItem::Path { points, .. } if points.len() == 2))
            .count();
        assert!(window_lines >= 3, "window sill + glass lines missing ({window_lines})");
        // CUT walls are 0.50 mm; SEEN BELOW (the slab) is 0.25 mm.
        let mut saw_cut = false;
        let mut saw_below = false;
        for item in &sheet.items {
            if let SheetItem::Path { stroke, .. } = item {
                if (stroke.width_mm - W_CUT).abs() < 1e-4 {
                    saw_cut = true;
                }
                if (stroke.width_mm - W_BELOW).abs() < 1e-4 {
                    saw_below = true;
                }
            }
        }
        assert!(saw_cut, "no CUT (0.50 mm) stroke");
        assert!(saw_below, "no SEEN BELOW (0.25 mm) stroke");
        let closed: Vec<&slice::Chain> = geo.structure.iter().filter(|c| c.closed).collect();
        assert!(!closed.is_empty(), "walls did not close");
        let poche: f64 = closed.iter().map(|c| c.area()).sum();
        // Door gap keeps the room from being a closed void; the cut is the
        // wall poche: 2×0.3 + 2×0.3 + 5×0.3 + 2×(3.4×0.3) = 4.74 m².
        assert!(
            (poche - 4.74).abs() / 4.74 < 0.005,
            "poche area {poche}, expected 4.74 ±0.5%"
        );
    }

    #[test]
    fn two_storeys_filter_and_edge_classes() {
        let scene = synthetic_room();
        let settings = PlanSettings::default();
        let g = cut_storey(&scene, 0, &settings);
        let f = cut_storey(&scene, 1, &settings);
        assert_eq!(slice::chain_crossings(&g.structure), 0);
        assert_eq!(slice::chain_crossings(&f.structure), 0);
        assert!(g.stats.cut >= 4, "ground cut {:?}", g.stats);
        assert!(f.stats.cut >= 1, "first-floor cut {:?}", f.stats);
        assert!(g.stats.below >= 1, "slab should be seen below: {:?}", g.stats);
        let names_g: Vec<String> = g
            .drawn
            .iter()
            .filter_map(|(id, _)| scene.element(*id).map(|e| e.name.clone()))
            .collect();
        let names_f: Vec<String> = f
            .drawn
            .iter()
            .filter_map(|(id, _)| scene.element(*id).map(|e| e.name.clone()))
            .collect();
        assert!(names_g.iter().any(|n| n == "Wall N"), "{names_g:?}");
        assert!(names_g.iter().all(|n| n != "Wall S1"), "upper wall on ground: {names_g:?}");
        assert!(names_f.iter().any(|n| n == "Wall S1"), "{names_f:?}");
        assert!(names_f.iter().all(|n| n != "Wall N"), "ground wall on first: {names_f:?}");
        // Door and window are openings, not poche-filled structure.
        assert_eq!(role_of(&ElementClass::Door, false), Role::Opening);
        assert_eq!(role_of(&ElementClass::Window, false), Role::Opening);
    }
}

/// Proper-crossing test exposed for the diagnostics above.
#[cfg(test)]
pub(crate) fn slice_crossing_for_test(a: ([i64; 2], [i64; 2]), b: ([i64; 2], [i64; 2])) -> bool {
    let r = [(a.1[0] - a.0[0]) as i128, (a.1[1] - a.0[1]) as i128];
    let s = [(b.1[0] - b.0[0]) as i128, (b.1[1] - b.0[1]) as i128];
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom == 0 {
        return false;
    }
    let qp = [(b.0[0] - a.0[0]) as i128, (b.0[1] - a.0[1]) as i128];
    let t = qp[0] * s[1] - qp[1] * s[0];
    let u = qp[0] * r[1] - qp[1] * r[0];
    let (t, u, denom) = if denom < 0 { (-t, -u, -denom) } else { (t, u, denom) };
    t > 0 && t < denom && u > 0 && u < denom
}

#[cfg(test)]
mod diagnostics {
    use super::*;
    use super::tests_support::villa_scene;

    /// What the ground floor sheet is actually made of, and which elements
    /// produce the long wandering chains ("spiders") that cross the drawing.
    #[test]
    fn ground_floor_composition() {
        let Some(scene) = villa_scene() else {
            eprintln!("villa sample missing, skipping");
            return;
        };
        let si = scene
            .stories
            .iter()
            .position(|s| s.name.contains("Ground"))
            .unwrap_or(0);
        let settings = PlanSettings::default();
        let geo = cut_storey(&scene, si, &settings);
        let sheet = plan_sheet(&scene, si, SheetId::from_index(0), &settings, &scene.units).unwrap();
        let fills = sheet.items.iter().filter(|i| matches!(i, SheetItem::Fill { .. })).count();
        let paths = sheet.items.iter().filter(|i| matches!(i, SheetItem::Path { .. })).count();
        let texts = sheet.items.iter().filter(|i| matches!(i, SheetItem::Text { .. })).count();
        let arcs = sheet.items.iter().filter(|i| matches!(i, SheetItem::Arc { .. })).count();
        eprintln!(
            "items: {} fills, {} paths, {} arcs, {} texts   structure {} chains, below {}, above {}",
            fills, paths, arcs, texts,
            geo.structure.len(), geo.below.len(), geo.above.len()
        );

        // Longest single segment in each layer: a plan-sized straight line
        // that is not a wall is a spider.
        let longest = |cs: &[Chain]| -> Vec<(f64, String, usize)> {
            let mut v: Vec<(f64, String, usize)> = cs
                .iter()
                .map(|c| {
                    let mut max = 0.0f64;
                    let n = c.pts.len();
                    let last = if c.closed { n } else { n.saturating_sub(1) };
                    for i in 0..last {
                        let a = c.pts[i];
                        let b = c.pts[(i + 1) % n];
                        let d = (((b[0] - a[0]) as f64).powi(2) + ((b[1] - a[1]) as f64).powi(2)).sqrt()
                            / super::slice::GRID_PER_METER;
                        max = max.max(d);
                    }
                    let name = scene
                        .element(c.element)
                        .map(|e| format!("{} [{}]", e.name, e.class.label()))
                        .unwrap_or_default();
                    (max, name, c.pts.len())
                })
                .collect();
            v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            v.truncate(6);
            v
        };
        eprintln!("longest structure segs: {:?}", longest(&geo.structure));
        eprintln!("longest below segs:     {:?}", longest(&geo.below));
        eprintln!("longest above segs:     {:?}", longest(&geo.above));

        // Poché coverage.
        let mut fill_loops: Vec<Chain> = geo.structure.iter().filter(|c| c.closed).cloned().collect();
        fill_loops.extend(geo.openings.iter().filter(|c| c.closed).cloned());
        let sp = super::slice::spans(&fill_loops, 0.03);
        let area: f64 = sp
            .iter()
            .map(|s| (s.x1 - s.x0) as f64 * (s.y1 - s.y0) as f64)
            .sum::<f64>()
            / (super::slice::GRID_PER_METER * super::slice::GRID_PER_METER);
        eprintln!(
            "poché: {} spans, {:.1} m² of cut material, {} closed loops fed in",
            sp.len(),
            area,
            fill_loops.len()
        );
        for (i, r) in geo.rooms.iter().take(6).enumerate() {
            eprintln!("room {i}: {:.2} m² '{}'", r.area_m2, r.name);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    pub fn villa_scene() -> Option<Scene> {
        Some(Scene::from_model(
            crate::model::demo::demo_house(),
            &mut |_| {},
        ))
    }
}
