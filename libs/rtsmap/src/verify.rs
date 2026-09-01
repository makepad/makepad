//! The generator checks its own work.
//!
//! A map that is pretty and unfair is a worse map than a plain fair one, so
//! nothing leaves the generator until it has proved four things about itself:
//! every house can put a base down, every house has a field within reach,
//! every house sees the same set of distances to its rivals, and every house
//! can WALK to every other one. The last is the one a hand-drawn map gets
//! wrong: a river or a cliff line closes and the round is unplayable.

use crate::math::hypot;
use crate::{RtsMap, Start};

/// Buildable cells a start pocket must contain.
pub const POCKET_MIN: u16 = 28;
/// Resource cells a house must have inside `crate::RESOURCE_REACH`.
pub const RESOURCE_CELLS_MIN: u16 = 8;
/// How far apart two houses' distance profiles may be, as a fraction.
pub const START_SPREAD_MAX: f32 = 0.10;
/// Worst/best walking distance to the middle of the map.
pub const PATH_SPREAD_MAX: f32 = 1.5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapReport {
    pub ok: bool,
    /// How many attempts it took (1 = the first one was fine).
    pub attempts: u32,
    pub reachable: bool,
    pub pocket_min: u16,
    pub resource_cells_min: u16,
    pub resource_reach_max: u16,
    pub start_spread: f32,
    pub path_spread: f32,
    /// Why it is not `ok`, in the order the checks run.
    pub failures: Vec<String>,
}

impl MapReport {
    /// Bigger is better — used only to pick the least-bad attempt when every
    /// attempt failed, so the caller still gets a playable map and a reason.
    pub fn score(&self) -> f32 {
        let mut score = 0.0;
        if self.reachable {
            score += 1000.0;
        }
        score += self.pocket_min as f32;
        score += self.resource_cells_min as f32;
        score -= self.start_spread * 200.0;
        score -= self.path_spread * 20.0;
        score -= self.resource_reach_max as f32 * 0.5;
        score
    }

    pub fn summary(&self) -> String {
        format!(
            "{} attempts={} reach={} pocket>={} field>={} within={} spread={:.3} path={:.2}{}",
            if self.ok { "fair" } else { "UNFAIR" },
            self.attempts,
            self.reachable,
            self.pocket_min,
            self.resource_cells_min,
            self.resource_reach_max,
            self.start_spread,
            self.path_spread,
            if self.failures.is_empty() { String::new() } else { format!(" — {}", self.failures.join("; ")) },
        )
    }
}

/// Can a ground unit stand on this `world-grid` letter?
#[inline]
pub fn passable_letter(letter: u8) -> bool {
    matches!(letter, b'.' | b'r' | b'b' | b't')
}

/// Breadth-first distance in cells from `from`, over passable grid letters.
/// `u16::MAX` means unreachable.
pub fn distance_field(map: &RtsMap, from: (u16, u16)) -> Vec<u16> {
    let (w, h) = (map.width as usize, map.height as usize);
    let mut dist = vec![u16::MAX; w * h];
    let start = from.1 as usize * w + from.0 as usize;
    if start >= dist.len() {
        return dist;
    }
    let mut queue = std::collections::VecDeque::new();
    dist[start] = 0;
    queue.push_back(start);
    while let Some(at) = queue.pop_front() {
        let (x, y) = ((at % w) as i32, (at / w) as i32);
        let step = dist[at].saturating_add(1);
        for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let next = ny as usize * w + nx as usize;
            if dist[next] != u16::MAX || !passable_letter(map.grid[next]) {
                continue;
            }
            dist[next] = step;
            queue.push_back(next);
        }
    }
    dist
}

/// Measure the map, fill in what each start actually got, and say whether it
/// is fair. Mutating is the point: `Start::pocket` and friends are MEASURED
/// facts, and a producer that wants to show them should read the same
/// numbers the check used.
pub fn verify(map: &mut RtsMap) -> MapReport {
    let mut report = MapReport { attempts: 1, ..MapReport::default() };
    if map.starts.is_empty() {
        report.failures.push("no starts".into());
        return report;
    }
    let (w, h) = (map.width as i32, map.height as i32);
    let starts: Vec<Start> = map.starts.clone();

    // Pocket: buildable, unblocked cells inside the guaranteed disc.
    let mut pocket_min = u16::MAX;
    for (index, start) in starts.iter().enumerate() {
        let mut pocket = 0u16;
        let r = crate::POCKET_RADIUS;
        for y in (start.y as i32 - r)..=(start.y as i32 + r) {
            for x in (start.x as i32 - r)..=(start.x as i32 + r) {
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                if hypot((x - start.x as i32) as f32, (y - start.y as i32) as f32) > r as f32 + 0.4 {
                    continue;
                }
                let at = y as usize * w as usize + x as usize;
                if map.terrain[at].buildable() && map.grid[at] != b'#' {
                    pocket += 1;
                }
            }
        }
        map.starts[index].pocket = pocket;
        pocket_min = pocket_min.min(pocket);
    }
    report.pocket_min = pocket_min;

    // Reach: one distance field per start pays for reachability, resource
    // reach and the walking-distance spread all at once.
    let fields: Vec<Vec<u16>> = starts.iter().map(|s| distance_field(map, (s.x, s.y))).collect();
    let mut reachable = true;
    for (index, field) in fields.iter().enumerate() {
        for (other, start) in starts.iter().enumerate() {
            if other == index {
                continue;
            }
            let at = start.y as usize * map.width as usize + start.x as usize;
            if field.get(at).copied().unwrap_or(u16::MAX) == u16::MAX {
                reachable = false;
            }
        }
    }
    report.reachable = reachable;

    let mut resource_cells_min = u16::MAX;
    let mut resource_reach_max = 0u16;
    for (index, field) in fields.iter().enumerate() {
        let mut cells = 0u16;
        let mut nearest = u16::MAX;
        for cell in &map.resources {
            let at = cell.y as usize * map.width as usize + cell.x as usize;
            let d = field.get(at).copied().unwrap_or(u16::MAX);
            if d == u16::MAX {
                continue;
            }
            nearest = nearest.min(d);
            if d <= crate::RESOURCE_REACH {
                cells += 1;
            }
        }
        map.starts[index].resource_cells = cells;
        map.starts[index].resource_distance = nearest;
        resource_cells_min = resource_cells_min.min(cells);
        resource_reach_max = resource_reach_max.max(if nearest == u16::MAX { u16::MAX } else { nearest });
    }
    report.resource_cells_min = if resource_cells_min == u16::MAX { 0 } else { resource_cells_min };
    report.resource_reach_max = resource_reach_max;

    // Equidistance: each house's SORTED distances to the others must be the
    // same list. That is the honest reading of "fair" for 3+ players, where
    // a single number cannot be equal for everyone.
    report.start_spread = start_spread(&starts);

    // Walking distance to the middle: the ring is fair as the crow flies;
    // this is what says the terrain did not make one spoke twice as long.
    let centre = (map.width / 2, map.height / 2);
    let mut walk: Vec<f32> = Vec::new();
    for field in &fields {
        let at = centre.1 as usize * map.width as usize + centre.0 as usize;
        match field.get(at).copied().unwrap_or(u16::MAX) {
            u16::MAX => {}
            d => walk.push(d as f32),
        }
    }
    report.path_spread = if walk.len() == starts.len() && !walk.is_empty() {
        let lo = walk.iter().cloned().fold(f32::MAX, f32::min).max(1.0);
        let hi = walk.iter().cloned().fold(0.0, f32::max);
        hi / lo
    } else {
        f32::INFINITY
    };

    if !report.reachable {
        report.failures.push("a house cannot walk to another house".into());
    }
    if report.pocket_min < POCKET_MIN {
        report.failures.push(format!("start pocket {} < {POCKET_MIN}", report.pocket_min));
    }
    if report.resource_cells_min < RESOURCE_CELLS_MIN && map.spec.resources > 0.0 {
        report
            .failures
            .push(format!("field within reach {} < {RESOURCE_CELLS_MIN}", report.resource_cells_min));
    }
    if report.start_spread > START_SPREAD_MAX {
        report.failures.push(format!("start spread {:.3} > {START_SPREAD_MAX}", report.start_spread));
    }
    if !(report.path_spread <= PATH_SPREAD_MAX) {
        report.failures.push(format!("walk spread {:.2} > {PATH_SPREAD_MAX}", report.path_spread));
    }
    report.ok = report.failures.is_empty();
    report
}

/// The worst relative disagreement between two houses' sorted distance
/// profiles. 0 for a regular polygon.
pub fn start_spread(starts: &[Start]) -> f32 {
    if starts.len() < 2 {
        return 0.0;
    }
    let profiles: Vec<Vec<f32>> = starts
        .iter()
        .map(|a| {
            let mut list: Vec<f32> = starts
                .iter()
                .filter(|b| !(b.x == a.x && b.y == a.y))
                .map(|b| hypot(b.x as f32 - a.x as f32, b.y as f32 - a.y as f32))
                .collect();
            list.sort_by(|l, r| l.partial_cmp(r).unwrap_or(std::cmp::Ordering::Equal));
            list
        })
        .collect();
    let mut worst = 0.0f32;
    for slot in 0..profiles[0].len() {
        let values: Vec<f32> = profiles.iter().filter_map(|p| p.get(slot).copied()).collect();
        if values.len() != profiles.len() {
            return f32::INFINITY;
        }
        let lo = values.iter().cloned().fold(f32::MAX, f32::min).max(0.001);
        let hi = values.iter().cloned().fold(0.0, f32::max);
        worst = worst.max((hi - lo) / lo);
    }
    worst
}
