//! One attempt at a map. `lib::generate` runs this until the fairness check
//! in `verify` is happy (or the retries run out).
//!
//! The order the passes run in IS the design:
//! plateaus and rims, then water, then roads (which cut through both, so a
//! road is always a way ACROSS a river and THROUGH a cliff line), then the
//! resource fields (rotationally placed, so every house gets the same deal),
//! then scenery, and finally the start pockets — carved last so nothing can
//! land on top of a house's building room.

use crate::math::{atan2, hypot, sin_cos};
use crate::rng::{fbm, hash2, noise2, Rng};
use crate::verify::MapReport;
use crate::{HouseSlot, MapSpec, Prop, PropKind, ResourceCell, RtsMap, Start, Style, Terrain};

const TWO_PI: f32 = core::f32::consts::PI * 2.0;

/// The eight house colours, in the order slots are handed out. Chosen to
/// stay apart at a strategy camera's zoom, and to survive a colourblind
/// squint (no red/green pair adjacent).
const HOUSE_COLORS: [[u8; 3]; 8] = [
    [0xe8, 0xc0, 0x40],
    [0xd0, 0x20, 0x20],
    [0x30, 0x70, 0xd0],
    [0x30, 0xb0, 0x70],
    [0xd0, 0x60, 0xc0],
    [0xe0, 0x80, 0x20],
    [0x40, 0xc8, 0xd0],
    [0x90, 0x90, 0x98],
];

/// Per-style knobs. Everything a style IS lives in this table, so adding one
/// is a row rather than a branch scattered through the passes.
struct StyleRules {
    /// Value-noise cell size for the broken-ground field, and how much of the
    /// map it may claim at amount 1.0.
    rough_cell: f32,
    rough_max: f32,
    /// Plateau blobs at amount 1.0, and their radius band in cells.
    plateau_max: usize,
    plateau_radius: (f32, f32),
    /// Rivers at amount 1.0.
    river_max: usize,
    /// Trees and rocks per 1000 cells at amount 1.0.
    tree_rate: f32,
    rock_rate: f32,
    /// Ring roads between neighbouring starts, not just spokes to the middle.
    ring_roads: bool,
    /// Every feature is authored once and rotated onto every start, so the
    /// map is the same map from each house's chair.
    rotational: bool,
}

fn rules(style: Style) -> StyleRules {
    match style {
        Style::Temperate => StyleRules {
            rough_cell: 7.0,
            rough_max: 0.26,
            plateau_max: 4,
            plateau_radius: (4.0, 9.0),
            river_max: 2,
            tree_rate: 26.0,
            rock_rate: 6.0,
            ring_roads: true,
            rotational: false,
        },
        Style::Desert => StyleRules {
            rough_cell: 12.0,
            rough_max: 0.25,
            plateau_max: 6,
            plateau_radius: (3.0, 8.0),
            river_max: 0,
            tree_rate: 3.0,
            rock_rate: 20.0,
            ring_roads: false,
            rotational: false,
        },
        Style::Arena => StyleRules {
            rough_cell: 6.0,
            rough_max: 0.16,
            plateau_max: 3,
            plateau_radius: (3.0, 6.0),
            river_max: 0,
            tree_rate: 10.0,
            rock_rate: 10.0,
            ring_roads: true,
            rotational: true,
        },
    }
}

/// Working state: the passes write cells and collect placements, and the
/// finished struct is assembled once at the end.
struct Build<'a> {
    spec: &'a MapSpec,
    seed: u32,
    w: i32,
    h: i32,
    terrain: Vec<Terrain>,
    heights: Vec<u8>,
    props: Vec<Prop>,
    resources: Vec<ResourceCell>,
    anchors: Vec<(i32, i32)>,
    centre: (f32, f32),
    radius: f32,
    base_angle: f32,
    rng: Rng,
}

impl<'a> Build<'a> {
    #[inline]
    fn at(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            None
        } else {
            Some((y * self.w + x) as usize)
        }
    }

    #[inline]
    fn get(&self, x: i32, y: i32) -> Option<Terrain> {
        self.at(x, y).map(|at| self.terrain[at])
    }

    #[inline]
    fn set(&mut self, x: i32, y: i32, terrain: Terrain) {
        if let Some(at) = self.at(x, y) {
            self.terrain[at] = terrain;
        }
    }

    /// Rotate a point about the map centre — how a feature authored for one
    /// house lands in front of the next one.
    fn rotate(&self, x: f32, y: f32, angle: f32) -> (f32, f32) {
        let (s, c) = sin_cos(angle);
        let (dx, dy) = (x - self.centre.0, y - self.centre.1);
        (self.centre.0 + dx * c - dy * s, self.centre.1 + dx * s + dy * c)
    }

    fn slice_angle(&self) -> f32 {
        TWO_PI / self.spec.players.max(1) as f32
    }

    fn near_any_start(&self, x: i32, y: i32, distance: f32) -> bool {
        self.anchors
            .iter()
            .any(|&(sx, sy)| hypot((x - sx) as f32, (y - sy) as f32) < distance)
    }
}

pub fn build(spec: &MapSpec, seed: u32) -> RtsMap {
    let rules = rules(spec.style);
    let (w, h) = (spec.width as i32, spec.height as i32);
    let count = (w * h) as usize;
    let mut rng = Rng::new(seed ^ 0x51ed_270b);
    let centre = ((w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5);
    // The start ring: a regular polygon, so every house sees the same set of
    // distances to its rivals. The margin keeps a pocket, its field and a
    // little breathing room inside the map.
    let margin = crate::POCKET_RADIUS as f32 + 6.0;
    let radius = ((w.min(h) as f32) * 0.5 - margin).max(6.0).min(w.min(h) as f32 * 0.40);
    let base_angle = rng.unit() * TWO_PI;

    let mut build = Build {
        spec,
        seed,
        w,
        h,
        terrain: vec![Terrain::Clear; count],
        heights: vec![0u8; count],
        props: Vec::new(),
        resources: Vec::new(),
        anchors: Vec::new(),
        centre,
        radius,
        base_angle,
        rng,
    };

    build.anchors = (0..spec.players)
        .map(|index| {
            let angle = base_angle + build.slice_angle() * index as f32;
            let (s, c) = sin_cos(angle);
            let x = (centre.0 + c * radius).round() as i32;
            let y = (centre.1 + s * radius).round() as i32;
            (x.clamp(3, w - 4), y.clamp(3, h - 4))
        })
        .collect();

    rough_pass(&mut build, &rules);
    plateau_pass(&mut build, &rules);
    water_pass(&mut build, &rules);
    road_pass(&mut build, &rules);
    resource_pass(&mut build);
    shore_pass(&mut build);
    prop_pass(&mut build, &rules);
    pocket_pass(&mut build);

    finish(build)
}

/// Broken ground. Two octaves of value noise thresholded so the patches have
/// a shape rather than a per-cell speckle.
fn rough_pass(build: &mut Build, rules: &StyleRules) {
    let amount = match build.spec.style {
        // A desert's dunes ARE its terrain, so they do not read off `cliffs`
        // or `water` — they take the leftover of both.
        Style::Desert => 0.55 + build.spec.cliffs * 0.35,
        _ => build.spec.cliffs * 0.6 + 0.15,
    };
    let coverage = rules.rough_max * amount;
    if coverage <= 0.001 {
        return;
    }
    let (slice, rotational) = (build.slice_angle(), rules.rotational);
    let field: Vec<f32> = (0..build.h)
        .flat_map(|y| (0..build.w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (sx, sy) = if rotational {
                fold(build, x as f32, y as f32, slice)
            } else {
                (x as f32, y as f32)
            };
            fbm(build.seed ^ 0x6a31_f2c9, sx, sy, rules.rough_cell)
        })
        .collect();
    // The threshold is the field's own QUANTILE, not a guessed constant: two
    // octaves of value noise almost never reach +-1, so "everything above
    // 0.74" is not a quarter of the map, it is a speckle. Asking the field
    // where its top `coverage` starts makes the amount an author types mean
    // what it says on every map size.
    let threshold = {
        let mut sorted = field.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let index = ((1.0 - coverage) * (sorted.len() - 1) as f32).round() as usize;
        sorted[index.min(sorted.len() - 1)]
    };
    for y in 0..build.h {
        for x in 0..build.w {
            if field[(y * build.w + x) as usize] > threshold {
                build.set(x, y, Terrain::Rough);
            }
        }
    }
}

/// Fold a point into the first slice of an N-fold rotation — an arena's
/// terrain is authored once and seen the same way from every chair.
fn fold(build: &Build, x: f32, y: f32, slice: f32) -> (f32, f32) {
    let (dx, dy) = (x - build.centre.0, y - build.centre.1);
    let angle = atan2(dy, dx) - build.base_angle;
    let wrapped = angle - (angle / slice).floor() * slice;
    let r = hypot(dx, dy);
    let (s, c) = sin_cos(wrapped + build.base_angle);
    (build.centre.0 + r * c, build.centre.1 + r * s)
}

/// Raised ground with a matched rim. Each blob is a plateau whose interior
/// is walkable high ground and whose edge is cliff; one or two rim cells are
/// cut back to a ramp so the top is not a decorative island.
fn plateau_pass(build: &mut Build, rules: &StyleRules) {
    let wanted = (rules.plateau_max as f32 * build.spec.cliffs).round() as usize;
    if wanted == 0 {
        return;
    }
    let slice = build.slice_angle();
    let replicas: usize = if rules.rotational { build.spec.players as usize } else { 1 };
    let authored = if rules.rotational { wanted.max(1) } else { wanted };
    let mut placed: Vec<(f32, f32, f32)> = Vec::new();
    for index in 0..authored {
        // Somewhere that is not a start pocket and not another plateau.
        let (mut cx, mut cy, mut rx, mut ry) = (0.0, 0.0, 0.0, 0.0);
        let mut found = false;
        for _ in 0..64 {
            let x = build.rng.range(4, (build.w - 4).max(5) as usize) as f32;
            let y = build.rng.range(4, (build.h - 4).max(5) as usize) as f32;
            let (lo, hi) = rules.plateau_radius;
            let a = lo + build.rng.unit() * (hi - lo);
            let b = lo + build.rng.unit() * (hi - lo);
            let clearance = a.max(b) + crate::POCKET_RADIUS as f32 + 3.0;
            if build.near_any_start(x as i32, y as i32, clearance) {
                continue;
            }
            if placed
                .iter()
                .any(|&(px, py, pr)| hypot(x - px, y - py) < pr + a.max(b) + 2.0)
            {
                continue;
            }
            cx = x;
            cy = y;
            rx = a;
            ry = b;
            found = true;
            break;
        }
        if !found {
            continue;
        }
        placed.push((cx, cy, rx.max(ry)));
        let level = 1 + ((index as u32 + build.seed) & 1) as u8;
        for replica in 0..replicas {
            let (px, py) = if replica == 0 {
                (cx, cy)
            } else {
                build.rotate(cx, cy, slice * replica as f32)
            };
            stamp_plateau(build, px, py, rx, ry, level);
        }
    }
    rim_pass(build);
    ramp_pass(build);
}

fn stamp_plateau(build: &mut Build, cx: f32, cy: f32, rx: f32, ry: f32, level: u8) {
    let reach = rx.max(ry).ceil() as i32 + 2;
    for y in (cy as i32 - reach)..=(cy as i32 + reach) {
        for x in (cx as i32 - reach)..=(cx as i32 + reach) {
            // Leave a one-cell frame so a plateau never runs off the map with
            // no rim — an unrimmed edge is the "confetti" look.
            if x < 1 || y < 1 || x >= build.w - 1 || y >= build.h - 1 {
                continue;
            }
            let dx = (x as f32 - cx) / rx.max(0.5);
            let dy = (y as f32 - cy) / ry.max(0.5);
            let wobble = noise2(build.seed ^ 0x911c_37d1, x, y) * 0.16;
            if dx * dx + dy * dy < 1.0 + wobble {
                if let Some(at) = build.at(x, y) {
                    build.terrain[at] = Terrain::Plateau;
                    build.heights[at] = level;
                }
            }
        }
    }
}

/// Every plateau cell with a lower neighbour becomes its rim. Done as one
/// pass over the finished plateau set, so two plateaus that grew into each
/// other share a rim instead of each drawing their own through the middle.
fn rim_pass(build: &mut Build) {
    let mut rim = Vec::new();
    for y in 0..build.h {
        for x in 0..build.w {
            if build.get(x, y) != Some(Terrain::Plateau) {
                continue;
            }
            let level = build.at(x, y).map(|at| build.heights[at]).unwrap_or(0);
            let lower = [(0, -1), (1, 0), (0, 1), (-1, 0)].iter().any(|&(dx, dy)| {
                match build.at(x + dx, y + dy) {
                    None => true,
                    Some(at) => build.heights[at] < level,
                }
            });
            if lower {
                rim.push((x, y));
            }
        }
    }
    for (x, y) in rim {
        build.set(x, y, Terrain::Cliff);
    }
}

/// One ramp per connected plateau: the cliff rim cell nearest the map centre
/// is cut back to walkable, so high ground is ground and not scenery.
fn ramp_pass(build: &mut Build) {
    let count = build.terrain.len();
    let mut seen = vec![false; count];
    let mut ramps = Vec::new();
    for start in 0..count {
        if seen[start] || !matches!(build.terrain[start], Terrain::Plateau | Terrain::Cliff) {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let mut best: Option<(f32, i32, i32)> = None;
        while let Some(at) = stack.pop() {
            let (x, y) = ((at as i32) % build.w, (at as i32) / build.w);
            if build.terrain[at] == Terrain::Cliff {
                let d = hypot(x as f32 - build.centre.0, y as f32 - build.centre.1);
                if best.map(|(bd, _, _)| d < bd).unwrap_or(true) {
                    best = Some((d, x, y));
                }
            }
            for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                if let Some(next) = build.at(x + dx, y + dy) {
                    if !seen[next]
                        && matches!(build.terrain[next], Terrain::Plateau | Terrain::Cliff)
                    {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        if let Some((_, x, y)) = best {
            ramps.push((x, y));
        }
    }
    for (x, y) in ramps {
        // A two-cell mouth: one cell is a door a unit queues at.
        build.set(x, y, Terrain::Plateau);
        let side = if build.rng.next_u32() & 1 == 0 { (1, 0) } else { (0, 1) };
        if build.get(x + side.0, y + side.1) == Some(Terrain::Cliff) {
            build.set(x + side.0, y + side.1, Terrain::Plateau);
        }
    }
}

/// Rivers: edge to edge, so a river always goes somewhere. Roads cross them
/// later, which is where the bridges come from.
fn water_pass(build: &mut Build, rules: &StyleRules) {
    let wanted = (rules.river_max as f32 * build.spec.water).round() as usize;
    for river in 0..wanted {
        let horizontal = (build.seed as usize + river) & 1 == 0;
        let span = if horizontal { build.w } else { build.h };
        let across = if horizontal { build.h } else { build.w };
        // Start a third of the way in and wander; the wobble is a smooth
        // field, not a coin, so the bank is a bank and not a staircase.
        let base = build.rng.range((across / 4) as usize, (across * 3 / 4).max(across / 4 + 1) as usize) as f32;
        let width = 1 + build.rng.below(2) as i32;
        let phase = build.rng.next_u32();
        for step in 0..span {
            let drift = fbm(build.seed ^ phase, step as f32, river as f32 * 32.0, 9.0) * (across as f32 * 0.16);
            let line = (base + drift).round() as i32;
            for widen in 0..=width {
                let (x, y) = if horizontal { (step, line + widen) } else { (line + widen, step) };
                if x < 1 || y < 1 || x >= build.w - 1 || y >= build.h - 1 {
                    continue;
                }
                // A river never takes a house's pocket away from it.
                if build.near_any_start(x, y, crate::POCKET_RADIUS as f32 + 3.0) {
                    continue;
                }
                build.set(x, y, Terrain::Water);
                if let Some(at) = build.at(x, y) {
                    build.heights[at] = 0;
                }
            }
        }
    }
}

/// The band where land meets water reads as beach — and the engine slows a
/// unit on it, which is what makes a crossing a decision.
fn shore_pass(build: &mut Build) {
    let mut shore = Vec::new();
    for y in 0..build.h {
        for x in 0..build.w {
            if !matches!(build.get(x, y), Some(Terrain::Clear) | Some(Terrain::Rough)) {
                continue;
            }
            let touches = [(0, -1), (1, 0), (0, 1), (-1, 0), (1, 1), (-1, -1), (1, -1), (-1, 1)]
                .iter()
                .any(|&(dx, dy)| build.get(x + dx, y + dy) == Some(Terrain::Water));
            if touches {
                shore.push((x, y));
            }
        }
    }
    for (x, y) in shore {
        build.set(x, y, Terrain::Shore);
    }
}

/// Roads: a spoke from every start to the middle, and (where the style asks)
/// a ring linking neighbours. A road CUTS: through a cliff line it is a pass,
/// through a river it is a bridge. That is what makes the map connected by
/// construction rather than by luck.
fn road_pass(build: &mut Build, rules: &StyleRules) {
    if build.spec.roads <= 0.001 {
        return;
    }
    let centre = (build.centre.0.round() as i32, build.centre.1.round() as i32);
    let anchors = build.anchors.clone();
    for &(sx, sy) in &anchors {
        draw_road(build, sx, sy, centre.0, centre.1);
    }
    if rules.ring_roads && build.spec.roads > 0.5 && anchors.len() > 2 {
        let slice = build.slice_angle();
        for index in 0..anchors.len() {
            let next = (index + 1) % anchors.len();
            // Bend the link outward so a ring reads as a ring rather than as
            // a second set of spokes.
            let angle = build.base_angle + slice * (index as f32 + 0.5);
            let (s, c) = sin_cos(angle);
            let mx = (build.centre.0 + c * build.radius * 1.05).round() as i32;
            let my = (build.centre.1 + s * build.radius * 1.05).round() as i32;
            let (mx, my) = (mx.clamp(1, build.w - 2), my.clamp(1, build.h - 2));
            draw_road(build, anchors[index].0, anchors[index].1, mx, my);
            draw_road(build, mx, my, anchors[next].0, anchors[next].1);
        }
    }
}

fn draw_road(build: &mut Build, x0: i32, y0: i32, x1: i32, y1: i32) {
    let (mut x, mut y) = (x0, y0);
    let dx = (x1 - x).abs();
    let sx = if x < x1 { 1 } else { -1 };
    let dy = -(y1 - y).abs();
    let sy = if y < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        let crossing = matches!(build.get(x, y), Some(Terrain::Water) | Some(Terrain::Cliff));
        build.set(x, y, Terrain::Road);
        if let Some(at) = build.at(x, y) {
            build.heights[at] = 0;
        }
        if crossing {
            // A one-cell bridge is a bug report waiting to happen: widen the
            // deck across the direction of travel.
            let (ox, oy) = if dx > -dy { (0, 1) } else { (1, 0) };
            if matches!(build.get(x + ox, y + oy), Some(Terrain::Water) | Some(Terrain::Cliff)) {
                build.set(x + ox, y + oy, Terrain::Road);
                if let Some(at) = build.at(x + ox, y + oy) {
                    build.heights[at] = 0;
                }
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let twice = error * 2;
        if twice >= dy {
            error += dy;
            x += sx;
        }
        if twice <= dx {
            error += dx;
            y += sy;
        }
    }
}

/// Resource fields. Every house gets its own, at the same distance and of
/// the same size, placed by ROTATION so the map cannot favour a chair; the
/// neutral fields in the middle are placed the same way and are what the
/// round is actually fought over.
fn resource_pass(build: &mut Build) {
    if build.spec.resources <= 0.001 {
        return;
    }
    let slice = build.slice_angle();
    let home_radius = 2.5 + build.spec.resources * 3.5;
    let mid_radius = 2.0 + build.spec.resources * 4.0;
    // Beside the base, not on the road to the middle.
    let offset_angle = 1.05;
    let distance = crate::POCKET_RADIUS as f32 + 4.0;
    let mut field = 0u8;
    for index in 0..build.spec.players as usize {
        let (sx, sy) = build.anchors[index];
        let inward = atan2(build.centre.1 - sy as f32, build.centre.0 - sx as f32);
        let (s, c) = sin_cos(inward + offset_angle);
        let fx = sx as f32 + c * distance;
        let fy = sy as f32 + s * distance;
        stamp_field(build, fx, fy, home_radius, field);
        field = field.wrapping_add(1);
    }
    // Neutral fields: one per gap between neighbours, at half the ring
    // radius, plus the middle when the map is generous.
    let neutral = (build.spec.resources * 3.0).round() as usize;
    if neutral > 0 {
        for index in 0..build.spec.players as usize {
            let angle = build.base_angle + slice * (index as f32 + 0.5);
            let (s, c) = sin_cos(angle);
            let fx = build.centre.0 + c * build.radius * 0.55;
            let fy = build.centre.1 + s * build.radius * 0.55;
            stamp_field(build, fx, fy, mid_radius, field);
            field = field.wrapping_add(1);
        }
    }
    if build.spec.resources > 0.75 {
        stamp_field(build, build.centre.0, build.centre.1, mid_radius, field);
    }
}

fn stamp_field(build: &mut Build, cx: f32, cy: f32, radius: f32, field: u8) {
    let reach = radius.ceil() as i32 + 1;
    for y in (cy as i32 - reach)..=(cy as i32 + reach) {
        for x in (cx as i32 - reach)..=(cx as i32 + reach) {
            if x < 1 || y < 1 || x >= build.w - 1 || y >= build.h - 1 {
                continue;
            }
            let d = hypot(x as f32 - cx, y as f32 - cy);
            let edge = radius + noise2(build.seed ^ 0x1d3a_77c1, x, y) * 0.9;
            if d > edge {
                continue;
            }
            // A field never paves over the ways through the map, and never
            // grows on water or a cliff.
            if !matches!(build.get(x, y), Some(Terrain::Clear) | Some(Terrain::Rough) | Some(Terrain::Plateau)) {
                continue;
            }
            if build.near_any_start(x, y, crate::POCKET_RADIUS as f32 + 1.0) {
                continue;
            }
            build.set(x, y, Terrain::Resource);
            // Richest at the heart: `stage` is the amount a harvester can
            // still lift, so a full cell must be a HIGH number.
            let fall = (d / edge.max(0.001) * 7.0).round().clamp(0.0, 7.0) as u8;
            build.resources.push(ResourceCell {
                x: x as u16,
                y: y as u16,
                stage: 11 - fall,
                field,
            });
        }
    }
}

/// Trees, rocks, ruins. Blocking scenery is what gives a map cover; it is
/// kept off roads, fields, pockets and the ring the starts sit on.
fn prop_pass(build: &mut Build, rules: &StyleRules) {
    let area = (build.w * build.h) as f32 / 1000.0;
    let trees = (rules.tree_rate * area).round() as usize;
    let rocks = (rules.rock_rate * area).round() as usize;
    let ruins = if build.spec.style == Style::Arena { 0 } else { (area * 1.5).round() as usize };
    let slice = build.slice_angle();
    let replicas = if rules.rotational { build.spec.players as usize } else { 1 };
    let authored = |total: usize| if replicas > 1 { (total / replicas).max(1) } else { total };
    let plan = [
        (PropKind::Tree, authored(trees), 17u8),
        (PropKind::Rock, authored(rocks), 8u8),
        (PropKind::Ruin, authored(ruins), 4u8),
    ];
    for (kind, wanted, variants) in plan {
        for _ in 0..wanted {
            let mut chosen = None;
            for _ in 0..24 {
                let x = build.rng.range(2, (build.w - 2).max(3) as usize) as i32;
                let y = build.rng.range(2, (build.h - 2).max(3) as usize) as i32;
                if build.get(x, y) != Some(Terrain::Clear) && build.get(x, y) != Some(Terrain::Rough) {
                    continue;
                }
                if build.near_any_start(x, y, crate::POCKET_RADIUS as f32 + 3.0) {
                    continue;
                }
                chosen = Some((x, y));
                break;
            }
            let Some((x, y)) = chosen else { continue };
            let variant = (build.rng.below(variants.max(1) as usize) + 1) as u8;
            for replica in 0..replicas {
                let (px, py) = if replica == 0 {
                    (x as f32, y as f32)
                } else {
                    build.rotate(x as f32, y as f32, slice * replica as f32)
                };
                let (px, py) = (px.round() as i32, py.round() as i32);
                if !matches!(build.get(px, py), Some(Terrain::Clear) | Some(Terrain::Rough)) {
                    continue;
                }
                if build.props.iter().any(|p| p.x as i32 == px && p.y as i32 == py) {
                    continue;
                }
                build.props.push(Prop { x: px as u16, y: py as u16, kind, variant });
            }
        }
    }
    // Blooms are dressing on a resource field, not an obstacle: one per
    // field heart, only where the pack has something to draw.
    let hearts: Vec<(u16, u16)> = {
        let mut best: Vec<(u8, u16, (u16, u16))> = Vec::new();
        for cell in &build.resources {
            match best.iter_mut().find(|(field, _, _)| *field == cell.field) {
                Some(entry) => {
                    if cell.stage > entry.1 as u8 {
                        entry.1 = cell.stage as u16;
                        entry.2 = (cell.x, cell.y);
                    }
                }
                None => best.push((cell.field, cell.stage as u16, (cell.x, cell.y))),
            }
        }
        best.into_iter().map(|(_, _, at)| at).collect()
    };
    for (x, y) in hearts {
        build.props.push(Prop { x, y, kind: PropKind::Bloom, variant: 1 });
    }
}

/// The last word on every start: a clear, flat, prop-free, resource-free
/// pocket a house can actually put a base down in.
fn pocket_pass(build: &mut Build) {
    let radius = crate::POCKET_RADIUS;
    let anchors = build.anchors.clone();
    for &(sx, sy) in &anchors {
        for y in (sy - radius)..=(sy + radius) {
            for x in (sx - radius)..=(sx + radius) {
                if hypot((x - sx) as f32, (y - sy) as f32) > radius as f32 + 0.4 {
                    continue;
                }
                let Some(at) = build.at(x, y) else { continue };
                build.terrain[at] = Terrain::Clear;
                build.heights[at] = 0;
            }
        }
        let (sx, sy) = (sx, sy);
        build.props.retain(|p| {
            hypot(p.x as f32 - sx as f32, p.y as f32 - sy as f32) > radius as f32 + 0.4
        });
        build.resources.retain(|r| {
            hypot(r.x as f32 - sx as f32, r.y as f32 - sy as f32) > radius as f32 + 0.4
        });
    }
}

fn finish(build: Build) -> RtsMap {
    let Build { spec, seed, w, h, terrain, heights, mut props, mut resources, anchors, .. } = build;
    props.sort_by_key(|p| (p.y, p.x, p.kind));
    props.dedup_by_key(|p| (p.y, p.x));
    resources.sort_by_key(|r| (r.y, r.x));
    resources.dedup_by_key(|r| (r.y, r.x));

    let mut grid: Vec<u8> = terrain.iter().map(|t| t.grid_letter()).collect();
    for prop in &props {
        if !prop.kind.blocks() {
            continue;
        }
        let at = prop.y as usize * w as usize + prop.x as usize;
        if at < grid.len() {
            grid[at] = b'#';
        }
    }

    let houses = (0..spec.players)
        .map(|index| HouseSlot { index, color: HOUSE_COLORS[index as usize % HOUSE_COLORS.len()] })
        .collect();
    let starts = anchors
        .iter()
        .map(|&(x, y)| Start {
            x: x as u16,
            y: y as u16,
            pocket: 0,
            resource_cells: 0,
            resource_distance: u16::MAX,
        })
        .collect();

    let _ = hash2(seed, w, h);
    RtsMap {
        spec: spec.clone(),
        seed,
        width: w as u16,
        height: h as u16,
        terrain,
        heights,
        grid,
        resources,
        props,
        starts,
        houses,
        report: MapReport::default(),
    }
}
