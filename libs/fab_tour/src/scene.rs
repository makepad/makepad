//! `TourScene` — the input contract of this crate.
//!
//! Deliberately *not* `fab::model::Scene`: the tour planner needs only
//! triangles, a class per element and the storey table, and keeping that
//! surface tiny means the synthetic property tests can build buildings without
//! dragging in the whole viewer scene layer, and a future non-Fab source
//! (IFC, glTF, a game level) can feed it just as well.
//!
//! Coordinate law, inherited from the scene layer: **right-handed, Z up,
//! meters**. Nothing in this crate ever converts.

use makepad_math::{vec3, Aabb, Vec3f};

/// What an element *is*, as far as camera work cares. This is a coarsening of
/// `fab::model::ElementClass`: the planner only distinguishes things that
/// block a body, things you move or look through, and things worth looking at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TourClass {
    Wall,
    Slab,
    Roof,
    Column,
    Beam,
    /// A door leaf. Blocks the *sealed* grid (so rooms segment) but not the
    /// *open* grid (so you can walk through the doorway).
    Door,
    Window,
    Skylight,
    /// A hole cut in something, with no leaf of its own.
    Opening,
    Stair,
    Railing,
    CurtainWall,
    Furniture,
    Lamp,
    /// An source application zone: a room *label* volume, not real geometry. Never
    /// occupies space; used to name the rooms the planner finds.
    Zone,
    /// Terrain / site mesh. Solid (you stand on it) but not part of the
    /// building envelope.
    Site,
    #[default]
    Other,
}

impl TourClass {
    /// Blocks a body in the navigation grid. Only doors, plain openings and
    /// zones do not — a door leaf modelled shut would otherwise wall off every
    /// room from the planner.
    ///
    /// Glazing *does* block: a floor-to-ceiling window is a wall as far as
    /// walking is concerned, and treating it as free space is how a camera
    /// walks out of a first-floor curtain wall into thin air. What you can see
    /// through is a separate question — see [`TourClass::is_transparent`].
    pub fn blocks_navigation(self) -> bool {
        !matches!(self, TourClass::Door | TourClass::Opening | TourClass::Zone)
    }

    /// You can see through it, so it does not stop a sight line. Doors count:
    /// a doorway the planner walks through is a doorway the camera sees
    /// through, and it is the *open* state we plan for.
    pub fn is_transparent(self) -> bool {
        matches!(
            self,
            TourClass::Window
                | TourClass::CurtainWall
                | TourClass::Skylight
                | TourClass::Door
                | TourClass::Opening
                | TourClass::Zone
        )
    }

    /// Blocks in the *sealed* grid used for room segmentation. Everything
    /// except zones — the door leaf is exactly what separates two rooms.
    pub fn seals_rooms(self) -> bool {
        self != TourClass::Zone
    }

    /// A hole in the envelope you can pass through on foot or in the air.
    pub fn is_portal(self) -> bool {
        matches!(
            self,
            TourClass::Door | TourClass::Opening | TourClass::Skylight | TourClass::Window
        )
    }

    /// Glass. Drives both the "worth looking at" score and the façade ranking.
    pub fn is_glazing(self) -> bool {
        matches!(
            self,
            TourClass::Window | TourClass::CurtainWall | TourClass::Skylight
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            TourClass::Wall => "wall",
            TourClass::Slab => "slab",
            TourClass::Roof => "roof",
            TourClass::Column => "column",
            TourClass::Beam => "beam",
            TourClass::Door => "door",
            TourClass::Window => "window",
            TourClass::Skylight => "skylight",
            TourClass::Opening => "opening",
            TourClass::Stair => "stair",
            TourClass::Railing => "railing",
            TourClass::CurtainWall => "curtain wall",
            TourClass::Furniture => "furniture",
            TourClass::Lamp => "lamp",
            TourClass::Zone => "zone",
            TourClass::Site => "site",
            TourClass::Other => "element",
        }
    }

    /// Infer a class from an source application element name when the file carries no
    /// type codes (legacy AC20). Token-aware on purpose: `"floor framing"` is
    /// timber, not a slab, and `"exterior wall"` is a wall.
    pub fn from_name(name: &str) -> TourClass {
        let n = name.trim().to_ascii_lowercase();
        if n.is_empty() {
            return TourClass::Other;
        }
        if n == "site" || n.starts_with("site ") {
            return TourClass::Site;
        }
        if n.contains("curtain") && n.contains("wall") {
            return TourClass::CurtainWall;
        }
        if n.contains("wall") {
            return TourClass::Wall;
        }
        if n.contains("slab") {
            return TourClass::Slab;
        }
        if n == "floor" || n.ends_with(" floor") || n.starts_with("floor slab") {
            return TourClass::Slab;
        }
        if n.contains("fram") {
            return TourClass::Beam;
        }
        if n.contains("roof") {
            return TourClass::Roof;
        }
        if n.contains("stair") {
            return TourClass::Stair;
        }
        if n == "post" || n.contains("column") || n.contains("pillar") {
            return TourClass::Column;
        }
        if n.contains("beam") {
            return TourClass::Beam;
        }
        if n.contains("rail") {
            return TourClass::Railing;
        }
        if n.contains("window") {
            return TourClass::Window;
        }
        if n.contains("skylight") {
            return TourClass::Skylight;
        }
        if n.contains("door") {
            return TourClass::Door;
        }
        match n.as_str() {
            "toilet" | "tub" | "sink" | "bath" | "sofa" | "chair" | "table" | "bed" => {
                TourClass::Furniture
            }
            _ => TourClass::Other,
        }
    }
}

/// One building element: a contiguous run of triangles plus its classification.
#[derive(Clone, Debug)]
pub struct TourElement {
    pub name: String,
    pub class: TourClass,
    /// Index into [`TourScene::storeys`], or `usize::MAX` when unassigned.
    pub storey: usize,
    /// First triangle (not index) of this element in [`TourScene::indices`].
    pub first_tri: u32,
    pub tri_count: u32,
    pub bounds: Aabb,
}

impl TourElement {
    pub fn has_geometry(&self) -> bool {
        self.tri_count > 0
    }

    pub fn center(&self) -> Vec3f {
        (self.bounds.min + self.bounds.max) * 0.5
    }

    pub fn size(&self) -> Vec3f {
        self.bounds.max - self.bounds.min
    }

    /// Area of the element's largest vertical face, a decent proxy for "how
    /// much glass is this" without unwrapping the mesh.
    pub fn facade_area(&self) -> f32 {
        let s = self.size();
        (s.x.max(s.y)) * s.z
    }
}

/// A building storey. `elevation` is the finished floor level, Z up, meters.
#[derive(Clone, Debug)]
pub struct TourStorey {
    pub name: String,
    pub elevation: f32,
    /// Floor-to-floor height. `0` means "unknown"; the analyser fills it in
    /// from the next storey up.
    pub height: f32,
}

/// A flat, immutable building. Triangles are world space already.
#[derive(Clone, Debug)]
pub struct TourScene {
    pub name: String,
    pub positions: Vec<Vec3f>,
    /// Triangle list, 3 per triangle.
    pub indices: Vec<u32>,
    /// One entry per triangle: the owning element index.
    pub tri_element: Vec<u32>,
    pub elements: Vec<TourElement>,
    pub storeys: Vec<TourStorey>,
    pub bounds: Aabb,
}

impl Default for TourScene {
    fn default() -> TourScene {
        TourScene {
            name: String::new(),
            positions: Vec::new(),
            indices: Vec::new(),
            tri_element: Vec::new(),
            elements: Vec::new(),
            storeys: Vec::new(),
            bounds: crate::geom::aabb_empty(),
        }
    }
}

impl TourScene {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn triangle(&self, tri: usize) -> [Vec3f; 3] {
        let i = tri * 3;
        [
            self.positions[self.indices[i] as usize],
            self.positions[self.indices[i + 1] as usize],
            self.positions[self.indices[i + 2] as usize],
        ]
    }

    pub fn element_of_triangle(&self, tri: usize) -> Option<&TourElement> {
        self.tri_element
            .get(tri)
            .and_then(|e| self.elements.get(*e as usize))
    }

    pub fn elements_of_class(&self, class: TourClass) -> impl Iterator<Item = (usize, &TourElement)> {
        self.elements
            .iter()
            .enumerate()
            .filter(move |(_, e)| e.class == class)
    }

    /// Bounds of the building proper — everything except the site mesh and
    /// zone volumes. The terrain plate is usually far bigger than the house
    /// and would otherwise set the voxel grid's size, the reveal radius and
    /// every framing decision.
    pub fn building_bounds(&self) -> Aabb {
        let mut b = crate::geom::aabb_empty();
        for e in &self.elements {
            if !e.has_geometry() || e.class == TourClass::Site || e.class == TourClass::Zone {
                continue;
            }
            // source application grids are 2-D annotation; some exports still mesh them
            // as enormous planes that would swallow the house.
            if e.name.to_ascii_lowercase().starts_with("grid") {
                continue;
            }
            b = crate::geom::aabb_union(&b, &e.bounds);
        }
        // When nothing is classified as site — legacy exports arrive with every
        // element typed `Other` — fall back to the geometry itself: walls are
        // vertical and ground is not, so the extent of the near-vertical
        // surfaces is the building. Without this a house on an 80 m site sizes
        // its voxel grid to the site, the cell coarsens past a doorway, and the
        // interior tour quietly has nothing to walk through.
        if !self.elements.iter().any(|e| e.class == TourClass::Site) {
            let vb = self.vertical_bounds();
            if !crate::geom::aabb_is_empty(&vb) {
                let s = vb.max - vb.min;
                if s.x > 2.0 && s.y > 2.0 && s.z > 2.0 {
                    return vb;
                }
            }
        }
        if crate::geom::aabb_is_empty(&b) {
            self.bounds
        } else {
            b
        }
    }

    /// Bounds of the near-vertical geometry: walls, not ground.
    ///
    /// Robust to outliers, which real exports have in quantity — a stray
    /// element 130 m below the site is enough to blow the voxel budget and
    /// coarsen the cell to three quarters of a metre. Uses area-weighted 1st
    /// and 99th percentiles per axis rather than min/max, so a few square
    /// metres of nonsense cannot decide the size of the grid.
    pub fn vertical_bounds(&self) -> Aabb {
        let mut pts: Vec<(Vec3f, f32)> = Vec::new();
        for t in 0..self.triangle_count() {
            if let Some(e) = self.element_of_triangle(t) {
                if e.class == TourClass::Zone {
                    continue;
                }
            }
            let [p0, p1, p2] = self.triangle(t);
            let n = Vec3f::cross(p1 - p0, p2 - p0);
            let len = n.length();
            if len < 1e-9 || (n.z / len).abs() > 0.5 {
                continue;
            }
            let area = len * 0.5;
            if area < 0.05 {
                continue;
            }
            pts.push(((p0 + p1 + p2) * (1.0 / 3.0), area));
        }
        if pts.len() < 8 {
            return crate::geom::aabb_empty();
        }
        let total: f32 = pts.iter().map(|(_, a)| *a).sum();
        let axis = |get: fn(&Vec3f) -> f32| -> (f32, f32) {
            let mut v: Vec<(f32, f32)> = pts.iter().map(|(p, a)| (get(p), *a)).collect();
            v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let pick = |frac: f32| -> f32 {
                let want = total * frac;
                let mut acc = 0.0;
                for (x, a) in &v {
                    acc += *a;
                    if acc >= want {
                        return *x;
                    }
                }
                v.last().map(|(x, _)| *x).unwrap_or(0.0)
            };
            (pick(0.01), pick(0.99))
        };
        let (x0, x1) = axis(|p| p.x);
        let (y0, y1) = axis(|p| p.y);
        let (z0, z1) = axis(|p| p.z);
        Aabb {
            min: vec3(x0, y0, z0),
            max: vec3(x1, y1, z1),
        }
    }

    /// Storey levels read out of the geometry, for files that declare none.
    ///
    /// Legacy Fab exports (and plenty of IFC) carry no story table at all, and
    /// without storeys there are no plan lattices, no rooms and no walkthrough.
    /// Floors are the one thing a building has a lot of: large, flat,
    /// upward-facing surface. Histogram that area by height and the peaks are
    /// the storeys — no element classification required, so it works on files
    /// where everything arrives as `Other`.
    pub fn infer_storeys(&self) -> Vec<TourStorey> {
        if self.triangle_count() == 0 {
            return Vec::new();
        }
        let b = self.building_bounds();
        let (z0, z1) = (b.min.z, b.max.z);
        if !(z1 > z0) {
            return Vec::new();
        }
        let has_slabs = self
            .elements
            .iter()
            .any(|e| e.class == TourClass::Slab && e.has_geometry());
        const BIN: f32 = 0.10;
        let nbins = (((z1 - z0) / BIN).ceil() as usize + 1).min(20_000);
        let mut hist = vec![0f32; nbins];
        for t in 0..self.triangle_count() {
            let Some(e) = self.element_of_triangle(t) else {
                continue;
            };
            // Zones are labels, not floors; roofs are not walked on. Site is
            // the hillside. Framing is timber in a floor, not the floor.
            if matches!(
                e.class,
                TourClass::Zone | TourClass::Roof | TourClass::Site | TourClass::Beam
            ) {
                continue;
            }
            // When slabs were classified, they *are* the floors. Histogram
            // them alone so joists, furniture and the odd landing do not
            // invent storeys.
            if has_slabs && e.class != TourClass::Slab {
                continue;
            }
            let [p0, p1, p2] = self.triangle(t);
            let n = Vec3f::cross(p1 - p0, p2 - p0);
            let len = n.length();
            if len < 1e-9 {
                continue;
            }
            // Upward-facing only: a slab's underside is a ceiling, not a floor.
            if n.z / len < 0.90 {
                continue;
            }
            let z = (p0.z + p1.z + p2.z) / 3.0;
            let k = (((z - z0) / BIN).floor() as isize).clamp(0, nbins as isize - 1) as usize;
            hist[k] += len * 0.5;
        }
        // Blur so a floor split across two bins counts once.
        let mut sm = hist.clone();
        for i in 0..nbins {
            let lo = i.saturating_sub(2);
            let hi = (i + 3).min(nbins);
            sm[i] = hist[lo..hi].iter().sum();
        }
        let peak = sm.iter().cloned().fold(0.0f32, f32::max);
        if peak <= 0.0 {
            return Vec::new();
        }
        // Strong, well-separated peaks, tallest first.
        let mut idx: Vec<usize> = (0..nbins).filter(|i| sm[*i] > peak * 0.12 && sm[*i] > 8.0).collect();
        idx.sort_by(|a, b| sm[*b].partial_cmp(&sm[*a]).unwrap_or(std::cmp::Ordering::Equal));
        let mut levels: Vec<f32> = Vec::new();
        for i in idx {
            let z = z0 + i as f32 * BIN;
            if levels.iter().any(|l| (l - z).abs() < 2.0) {
                continue;
            }
            levels.push(z);
        }
        levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        levels
            .iter()
            .enumerate()
            .map(|(i, z)| TourStorey {
                name: format!("Level {i}"),
                elevation: *z,
                height: 0.0,
            })
            .collect()
    }

    /// Storeys sorted bottom-up, with `height` filled in from the gap to the
    /// next storey (and from the scene bounds for the topmost).
    /// Legacy files ship door leaves as unnamed `Other` meshes. A vertical
    /// slab 0.7–1.5 m wide and ~2 m tall is a door; marking it so does not
    /// invent geometry, it just stops the leaf from walling off the opening.
    pub fn reclassify_door_shaped(&mut self) {
        for e in &mut self.elements {
            if e.class != TourClass::Other || !e.has_geometry() {
                continue;
            }
            let s = e.size();
            let thick = s.x.min(s.y);
            let wide = s.x.max(s.y);
            if s.z >= 1.70 && s.z <= 2.70 && thick <= 0.45 && (0.65..=1.55).contains(&wide) {
                e.class = TourClass::Door;
            }
        }
    }

    pub fn storeys_resolved(&self) -> Vec<TourStorey> {
        let mut s = if self.storeys.is_empty() {
            self.infer_storeys()
        } else {
            self.storeys.clone()
        };
        s.sort_by(|a, b| a.elevation.partial_cmp(&b.elevation).unwrap_or(std::cmp::Ordering::Equal));
        for i in 0..s.len() {
            if s[i].height > 0.01 {
                continue;
            }
            let top = if i + 1 < s.len() {
                s[i + 1].elevation
            } else {
                self.building_bounds().max.z
            };
            s[i].height = (top - s[i].elevation).max(0.0);
        }
        s
    }
}

/// Incremental builder. Every `push_element` call opens a new element and the
/// triangles pushed after it belong to it.
#[derive(Debug, Default)]
pub struct TourSceneBuilder {
    scene: TourScene,
}

impl TourSceneBuilder {
    pub fn new(name: &str) -> TourSceneBuilder {
        TourSceneBuilder {
            scene: TourScene {
                name: name.into(),
                bounds: crate::geom::aabb_empty(),
                ..Default::default()
            },
        }
    }

    pub fn storey(&mut self, name: &str, elevation: f32, height: f32) -> usize {
        self.scene.storeys.push(TourStorey {
            name: name.into(),
            elevation,
            height,
        });
        self.scene.storeys.len() - 1
    }

    /// Open a new element. Triangles pushed after this belong to it.
    pub fn element(&mut self, name: &str, class: TourClass, storey: usize) -> usize {
        let first_tri = self.scene.triangle_count() as u32;
        self.scene.elements.push(TourElement {
            name: name.into(),
            class,
            storey,
            first_tri,
            tri_count: 0,
            bounds: crate::geom::aabb_empty(),
        });
        self.scene.elements.len() - 1
    }

    pub fn triangle(&mut self, a: Vec3f, b: Vec3f, c: Vec3f) {
        if self.scene.elements.is_empty() {
            return;
        }
        let ei = (self.scene.elements.len() - 1) as u32;
        let base = self.scene.positions.len() as u32;
        let mut ebounds = self.scene.elements[ei as usize].bounds;
        for p in [a, b, c] {
            self.scene.positions.push(p);
            self.scene.bounds = crate::geom::aabb_union_point(&self.scene.bounds, p);
            ebounds = crate::geom::aabb_union_point(&ebounds, p);
        }
        self.scene.indices.extend_from_slice(&[base, base + 1, base + 2]);
        self.scene.tri_element.push(ei);
        let elem = &mut self.scene.elements[ei as usize];
        elem.bounds = ebounds;
        elem.tri_count += 1;
    }

    /// Axis-aligned box, 12 triangles, outward winding.
    pub fn box_solid(&mut self, min: Vec3f, max: Vec3f) {
        let c = [
            Vec3f { x: min.x, y: min.y, z: min.z },
            Vec3f { x: max.x, y: min.y, z: min.z },
            Vec3f { x: max.x, y: max.y, z: min.z },
            Vec3f { x: min.x, y: max.y, z: min.z },
            Vec3f { x: min.x, y: min.y, z: max.z },
            Vec3f { x: max.x, y: min.y, z: max.z },
            Vec3f { x: max.x, y: max.y, z: max.z },
            Vec3f { x: min.x, y: max.y, z: max.z },
        ];
        const QUADS: [[usize; 4]; 6] = [
            [0, 3, 2, 1], // bottom
            [4, 5, 6, 7], // top
            [0, 1, 5, 4], // -y
            [1, 2, 6, 5], // +x
            [2, 3, 7, 6], // +y
            [3, 0, 4, 7], // -x
        ];
        for q in QUADS {
            self.triangle(c[q[0]], c[q[1]], c[q[2]]);
            self.triangle(c[q[0]], c[q[2]], c[q[3]]);
        }
    }

    pub fn finish(self) -> TourScene {
        self.scene
    }

    pub fn scene_mut(&mut self) -> &mut TourScene {
        &mut self.scene
    }
}

#[cfg(test)]
mod tests {
    use super::TourClass;

    #[test]
    fn classifies_source_names() {
        assert_eq!(TourClass::from_name("exterior wall"), TourClass::Wall);
        assert_eq!(TourClass::from_name("interior wall"), TourClass::Wall);
        assert_eq!(TourClass::from_name("wall"), TourClass::Wall);
        assert_eq!(TourClass::from_name("concrete slab"), TourClass::Slab);
        assert_eq!(TourClass::from_name("floor"), TourClass::Slab);
        assert_eq!(TourClass::from_name("floor framing"), TourClass::Beam);
        assert_eq!(TourClass::from_name("roof framing"), TourClass::Beam);
        assert_eq!(TourClass::from_name("Site"), TourClass::Site);
        assert_eq!(TourClass::from_name("post"), TourClass::Column);
        assert_eq!(TourClass::from_name("toilet"), TourClass::Furniture);
        assert_eq!(TourClass::from_name("tub"), TourClass::Furniture);
        assert_eq!(TourClass::from_name("Grid1"), TourClass::Other);
        assert_eq!(TourClass::from_name("207"), TourClass::Other);
        assert_eq!(TourClass::from_name("DELETE ME"), TourClass::Other);
    }
}
