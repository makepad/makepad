//! Lane E: generated sheets.
//!
//! `Scene::sheets` is empty for both samples, and it will stay empty for real
//! Fab files until the format's raster sheets can be expressed in
//! `fab::model::Sheet` for the vector drawing area.
//! So that the Sheets editor is a real thing to look at and drive — and so
//! that sheet ↔ model cross-highlight is tested end to end — this module
//! *draws* sheets from the loaded model: a 1:N plan per storey, four exterior
//! elevations, and a building section, in paper millimetres, with a `SheetLink`
//! per element.
//!
//! They are labelled "generated" on the sheet itself. The moment
//! `Scene::sheets` is non-empty, `sheets_for` returns the file's own sheets
//! and none of this runs.

use crate::api::*;
use crate::model::{
    makepad_math::{vec3, Vec3f},
    Element, Sheet, SheetItem, SheetLink, Stroke,
};
use crate::sheets::{plan, slice};

/// A3 landscape, the size Fab publishes the demo layouts at.
const PAGE: [f32; 2] = [420.0, 297.0];
const MARGIN: f32 = 12.0;
/// Room left at the bottom for the title block.
const TITLE_H: f32 = 26.0;

const INK: [f32; 4] = [0.10, 0.10, 0.10, 1.0];
const WALL_FILL: [f32; 4] = [0.62, 0.62, 0.60, 1.0];
const SLAB_FILL: [f32; 4] = [0.88, 0.88, 0.86, 1.0];
const OPENING: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

fn hairline(color: [f32; 4], width_mm: f32) -> Stroke {
    Stroke {
        width_mm,
        color,
        dash: [0.0, 0.0],
    }
}

fn rect_points(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// Pick the nearest architectural scale that fits `size` (metres) on the page.
fn pick_scale(size: [f32; 2], page: [f32; 2]) -> f32 {
    let avail = [
        page[0] - MARGIN * 2.0,
        page[1] - MARGIN * 2.0 - TITLE_H,
    ];
    for scale in [20.0f32, 50.0, 100.0, 200.0, 500.0, 1000.0] {
        let mm_per_m = 1000.0 / scale;
        if size[0] * mm_per_m <= avail[0] && size[1] * mm_per_m <= avail[1] {
            return scale;
        }
    }
    2000.0
}

struct Frame {
    scale: f32,
    /// Paper mm per model metre.
    mm_per_m: f32,
    origin_mm: [f32; 2],
    min: [f32; 2],
}

impl Frame {
    fn new(min: [f32; 2], size: [f32; 2]) -> Frame {
        let scale = pick_scale(size, PAGE);
        let mm_per_m = 1000.0 / scale;
        let w = size[0] * mm_per_m;
        let h = size[1] * mm_per_m;
        let origin_mm = [
            (PAGE[0] - w) * 0.5,
            TITLE_H + (PAGE[1] - TITLE_H - h) * 0.5,
        ];
        Frame {
            scale,
            mm_per_m,
            origin_mm,
            min,
        }
    }

    fn to_paper(&self, x: f32, y: f32) -> [f32; 2] {
        [
            self.origin_mm[0] + (x - self.min[0]) * self.mm_per_m,
            self.origin_mm[1] + (y - self.min[1]) * self.mm_per_m,
        ]
    }
}

fn title_block(items: &mut Vec<SheetItem>, number: &str, name: &str, scale: f32, project: &str) {
    items.push(SheetItem::Path {
        points: rect_points(MARGIN, MARGIN, PAGE[0] - MARGIN, PAGE[1] - MARGIN),
        closed: true,
        stroke: hairline(INK, 0.5),
    });
    items.push(SheetItem::Path {
        points: rect_points(PAGE[0] - MARGIN - 130.0, MARGIN, PAGE[0] - MARGIN, MARGIN + TITLE_H - 6.0),
        closed: true,
        stroke: hairline(INK, 0.35),
    });
    items.push(SheetItem::Text {
        pos: [PAGE[0] - MARGIN - 125.0, MARGIN + TITLE_H - 13.0],
        text: format!("{number}  {name}"),
        height_mm: 4.5,
        angle_deg: 0.0,
        color: INK,
    });
    items.push(SheetItem::Text {
        pos: [PAGE[0] - MARGIN - 125.0, MARGIN + 4.0],
        text: format!("{project}   ·   1:{scale:.0}   ·   generated from the model"),
        height_mm: 2.6,
        angle_deg: 0.0,
        color: [0.35, 0.35, 0.35, 1.0],
    });
}

/// A scale bar, 0 to `metres`, in the bottom-left of the drawing area.
fn scale_bar(items: &mut Vec<SheetItem>, frame: &Frame) {
    let step_m = if frame.scale <= 50.0 {
        1.0
    } else if frame.scale <= 200.0 {
        5.0
    } else {
        10.0
    };
    let x0 = MARGIN + 6.0;
    let y0 = MARGIN + TITLE_H - 4.0;
    let seg = step_m * frame.mm_per_m;
    for i in 0..4 {
        let x = x0 + i as f32 * seg;
        let fill = if i % 2 == 0 {
            [0.1, 0.1, 0.1, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        items.push(SheetItem::Fill {
            points: rect_points(x, y0, x + seg, y0 + 1.6),
            color: fill,
            stroke: Some(hairline(INK, 0.25)),
        });
    }
    items.push(SheetItem::Text {
        pos: [x0, y0 - 4.0],
        text: format!("0        {:.0} m", step_m * 4.0),
        height_mm: 2.4,
        angle_deg: 0.0,
        color: [0.35, 0.35, 0.35, 1.0],
    });
}

fn elevation_fill(class: &ElementClass) -> [f32; 4] {
    match class {
        ElementClass::Wall | ElementClass::Column | ElementClass::CurtainWall => WALL_FILL,
        ElementClass::Slab | ElementClass::Stair | ElementClass::Railing => SLAB_FILL,
        ElementClass::Roof | ElementClass::Shell => [0.52, 0.52, 0.50, 1.0],
        ElementClass::Beam => [0.72, 0.71, 0.68, 1.0],
        _ => [0.78, 0.78, 0.76, 1.0],
    }
}

fn is_opening(class: &ElementClass) -> bool {
    matches!(
        class,
        ElementClass::Door
            | ElementClass::Window
            | ElementClass::Skylight
            | ElementClass::Opening
    )
}

fn belongs_in_elevation(class: &ElementClass) -> bool {
    matches!(
        class,
        ElementClass::Wall
            | ElementClass::Slab
            | ElementClass::Roof
            | ElementClass::Shell
            | ElementClass::Column
            | ElementClass::Beam
            | ElementClass::Door
            | ElementClass::Window
            | ElementClass::Skylight
            | ElementClass::Opening
            | ElementClass::Stair
            | ElementClass::Railing
            | ElementClass::CurtainWall
            | ElementClass::Morph
    )
}

#[derive(Clone, Copy)]
enum ElevationView {
    South,
    North,
    East,
    West,
}

impl ElevationView {
    fn name(self) -> &'static str {
        match self {
            ElevationView::South => "South",
            ElevationView::North => "North",
            ElevationView::East => "East",
            ElevationView::West => "West",
        }
    }

    fn number(self) -> &'static str {
        match self {
            ElevationView::South => "A-201",
            ElevationView::North => "A-202",
            ElevationView::East => "A-203",
            ElevationView::West => "A-204",
        }
    }

    /// Orthographic paper axes and a score that increases toward the viewer.
    fn project(self, point: Vec3f) -> ([f32; 2], f32) {
        match self {
            ElevationView::South => ([point.x, point.z], -point.y),
            ElevationView::North => ([-point.x, point.z], point.y),
            ElevationView::East => ([point.y, point.z], point.x),
            ElevationView::West => ([-point.y, point.z], -point.x),
        }
    }
}

fn corners(bounds: &Aabb) -> [Vec3f; 8] {
    [
        vec3(bounds.min.x, bounds.min.y, bounds.min.z),
        vec3(bounds.max.x, bounds.min.y, bounds.min.z),
        vec3(bounds.min.x, bounds.max.y, bounds.min.z),
        vec3(bounds.max.x, bounds.max.y, bounds.min.z),
        vec3(bounds.min.x, bounds.min.y, bounds.max.z),
        vec3(bounds.max.x, bounds.min.y, bounds.max.z),
        vec3(bounds.min.x, bounds.max.y, bounds.max.z),
        vec3(bounds.max.x, bounds.max.y, bounds.max.z),
    ]
}

fn projected_bounds(
    scene: &Scene,
    ids: &[ElementId],
    view: ElevationView,
) -> Option<([f32; 2], [f32; 2], [f32; 2])> {
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    let mut depth = [f32::MAX, f32::MIN];
    for id in ids {
        let element = scene.element(*id)?;
        for corner in corners(&element.bounds) {
            let (point, toward) = view.project(corner);
            for axis in 0..2 {
                lo[axis] = lo[axis].min(point[axis]);
                hi[axis] = hi[axis].max(point[axis]);
            }
            depth[0] = depth[0].min(toward);
            depth[1] = depth[1].max(toward);
        }
    }
    (lo[0].is_finite() && hi[0] > lo[0] && hi[1] > lo[1]).then_some((lo, hi, depth))
}

fn projected_element_rect(element: &Element, view: ElevationView) -> ([f32; 2], [f32; 2]) {
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for corner in corners(&element.bounds) {
        let (point, _) = view.project(corner);
        for axis in 0..2 {
            lo[axis] = lo[axis].min(point[axis]);
            hi[axis] = hi[axis].max(point[axis]);
        }
    }
    (lo, hi)
}

fn triangle_area(points: &[[f32; 2]; 3]) -> f32 {
    ((points[1][0] - points[0][0]) * (points[2][1] - points[0][1])
        - (points[1][1] - points[0][1]) * (points[2][0] - points[0][0]))
        .abs()
        * 0.5
}

/// A storey plan: a real horizontal cut, classified and unioned
/// ([`crate::sheets::plan`]). The bounding-box version this replaced drew one
/// axis-aligned rectangle per element — 388 of them on the villa's Ground
/// Floor with 1644 overlapping pairs, which is what "self-intersecting" looked
/// like on screen.
fn plan_for_story(
    scene: &Scene,
    story_index: usize,
    id: SheetId,
    settings: &plan::PlanSettings,
) -> Option<Sheet> {
    plan::plan_sheet(scene, story_index, id, settings, &scene.units)
}

/// A triangle-projected orthographic elevation. Site/terrain is deliberately
/// excluded from the framing: a deep terrain skirt must not shrink the house
/// to a vertical mark. Opaque faces are painted back-to-front; openings are
/// then knocked out in paper white, which makes windows read as holes.
fn elevation(scene: &Scene, id: SheetId, view: ElevationView) -> Option<Sheet> {
    let ids: Vec<ElementId> = scene
        .elements
        .iter()
        .filter(|element| element.has_geometry() && belongs_in_elevation(&element.class))
        .map(|e| e.id)
        .collect();
    let (lo, hi, depth) = projected_bounds(scene, &ids, view)?;
    let frame = Frame::new(lo, [(hi[0] - lo[0]).max(0.1), (hi[1] - lo[1]).max(0.1)]);

    let mut items = Vec::new();
    let mut links = Vec::new();
    let mut faces: Vec<(f32, [f32; 4], [[f32; 2]; 3])> = Vec::new();
    for eid in &ids {
        let element = scene.element(*eid)?;
        if !is_opening(&element.class) {
            for triangle in slice::element_triangles(scene, *eid) {
                let projected = triangle.map(|point| view.project(point));
                let model_points = [projected[0].0, projected[1].0, projected[2].0];
                if triangle_area(&model_points) < 1e-5 {
                    continue;
                }
                let paper = model_points.map(|point| frame.to_paper(point[0], point[1]));
                if triangle_area(&paper) < 0.01 {
                    continue;
                }
                faces.push((
                    (projected[0].1 + projected[1].1 + projected[2].1) / 3.0,
                    elevation_fill(&element.class),
                    paper,
                ));
            }
        }
        let (element_lo, element_hi) = projected_element_rect(element, view);
        let p0 = frame.to_paper(element_lo[0], element_lo[1]);
        let p1 = frame.to_paper(element_hi[0], element_hi[1]);
        links.push(SheetLink {
            rect_mm: [
                p0[0].min(p1[0]),
                p0[1].min(p1[1]),
                p0[0].max(p1[0]),
                p0[1].max(p1[1]),
            ],
            element: *eid,
        });
    }
    faces.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, color, points) in faces {
        items.push(SheetItem::Fill {
            points: points.to_vec(),
            color,
            stroke: None,
        });
    }
    if items.is_empty() {
        return None;
    }

    // Only openings on the viewer-facing half of the model punch the façade;
    // an opening on the far wall must not erase an unrelated near wall.
    let depth_mid = (depth[0] + depth[1]) * 0.5;
    for eid in &ids {
        let element = scene.element(*eid)?;
        if !is_opening(&element.class) {
            continue;
        }
        let (_, toward) = view.project(aabb_center(&element.bounds));
        if toward < depth_mid {
            continue;
        }
        let (opening_lo, opening_hi) = projected_element_rect(element, view);
        let p0 = frame.to_paper(opening_lo[0], opening_lo[1]);
        let p1 = frame.to_paper(opening_hi[0], opening_hi[1]);
        if (p1[0] - p0[0]).abs() < 0.4 || (p1[1] - p0[1]).abs() < 0.4 {
            continue;
        }
        items.push(SheetItem::Fill {
            points: rect_points(p0[0], p0[1], p1[0], p1[1]),
            color: OPENING,
            stroke: Some(hairline(INK, 0.3)),
        });
    }

    // Storey lines, the thing an elevation is actually for.
    for s in &scene.stories {
        let y = frame.to_paper(lo[0], s.elevation)[1];
        if y < MARGIN + TITLE_H || y > PAGE[1] - MARGIN {
            continue;
        }
        items.push(SheetItem::Path {
            points: vec![frame.to_paper(lo[0], s.elevation), frame.to_paper(hi[0], s.elevation)],
            closed: false,
            stroke: Stroke {
                width_mm: 0.25,
                color: [0.35, 0.45, 0.7, 1.0],
                dash: [3.0, 2.0],
            },
        });
        items.push(SheetItem::Text {
            pos: [frame.to_paper(lo[0], s.elevation)[0] + 1.0, y + 1.5],
            text: format!("{}  {:+.2} m", s.name, s.elevation),
            height_mm: 2.4,
            angle_deg: 0.0,
            color: [0.35, 0.45, 0.7, 1.0],
        });
    }
    scale_bar(&mut items, &frame);
    let title = format!("{} Elevation", view.name());
    title_block(&mut items, view.number(), &title, frame.scale, &scene.name);

    Some(Sheet {
        id,
        name: format!("{} {}", view.number(), title),
        size_mm: PAGE,
        scale: frame.scale,
        items,
        links,
        story: None,
    })
}

fn vertical_intersection(triangle: [Vec3f; 3], y: f32) -> Option<[[f32; 2]; 2]> {
    let distance = triangle.map(|point| point.y - y);
    if distance.iter().all(|value| *value > 0.0) || distance.iter().all(|value| *value < 0.0) {
        return None;
    }
    if distance.iter().all(|value| value.abs() < 1e-6) {
        return None;
    }
    let mut hits: Vec<[f32; 2]> = Vec::with_capacity(3);
    for edge in 0..3 {
        let next = (edge + 1) % 3;
        let from = triangle[edge];
        let to = triangle[next];
        let a = distance[edge];
        let b = distance[next];
        let point = if a.abs() < 1e-6 {
            Some([from.x, from.z])
        } else if (a < 0.0) != (b < 0.0) {
            let t = a / (a - b);
            Some([from.x + (to.x - from.x) * t, from.z + (to.z - from.z) * t])
        } else {
            None
        };
        if let Some(point) = point {
            if !hits.iter().any(|old| {
                (old[0] - point[0]).abs() < 1e-5 && (old[1] - point[1]).abs() < 1e-5
            }) {
                hits.push(point);
            }
        }
    }
    if hits.len() >= 2 {
        Some([hits[0], hits[1]])
    } else {
        None
    }
}

/// A transverse section through the centre of the architectural model.
fn building_section(scene: &Scene, id: SheetId) -> Option<Sheet> {
    let ids: Vec<ElementId> = scene
        .elements
        .iter()
        .filter(|element| {
            element.has_geometry()
                && belongs_in_elevation(&element.class)
                && !is_opening(&element.class)
        })
        .map(|element| element.id)
        .collect();
    let (lo, hi, _) = projected_bounds(scene, &ids, ElevationView::South)?;
    let frame = Frame::new(lo, [(hi[0] - lo[0]).max(0.1), (hi[1] - lo[1]).max(0.1)]);
    let cut_y = ids
        .iter()
        .filter_map(|id| scene.element(*id))
        .fold([f32::MAX, f32::MIN], |range, element| {
            [range[0].min(element.bounds.min.y), range[1].max(element.bounds.max.y)]
        });
    let cut_y = (cut_y[0] + cut_y[1]) * 0.5;
    let mut items = Vec::new();
    let mut links = Vec::new();
    for story in &scene.stories {
        let y = frame.to_paper(lo[0], story.elevation)[1];
        items.push(SheetItem::Path {
            points: vec![frame.to_paper(lo[0], story.elevation), frame.to_paper(hi[0], story.elevation)],
            closed: false,
            stroke: Stroke {
                width_mm: 0.2,
                color: [0.42, 0.48, 0.62, 1.0],
                dash: [3.0, 2.0],
            },
        });
        items.push(SheetItem::Text {
            pos: [frame.to_paper(lo[0], story.elevation)[0] + 1.0, y + 1.5],
            text: format!("{}  {:+.2} m", story.name, story.elevation),
            height_mm: 2.4,
            angle_deg: 0.0,
            color: [0.35, 0.45, 0.7, 1.0],
        });
    }
    for eid in ids {
        let element = scene.element(eid)?;
        let mut contributed = false;
        for triangle in slice::element_triangles(scene, eid) {
            let Some(segment) = vertical_intersection(triangle, cut_y) else {
                continue;
            };
            let points = segment.map(|point| frame.to_paper(point[0], point[1]));
            if (points[0][0] - points[1][0]).abs() + (points[0][1] - points[1][1]).abs() < 0.05 {
                continue;
            }
            items.push(SheetItem::Path {
                points: points.to_vec(),
                closed: false,
                stroke: hairline(INK, 0.45),
            });
            contributed = true;
        }
        if contributed {
            let p0 = frame.to_paper(element.bounds.min.x, element.bounds.min.z);
            let p1 = frame.to_paper(element.bounds.max.x, element.bounds.max.z);
            links.push(SheetLink {
                rect_mm: [
                    p0[0].min(p1[0]),
                    p0[1].min(p1[1]),
                    p0[0].max(p1[0]),
                    p0[1].max(p1[1]),
                ],
                element: eid,
            });
        }
    }
    if links.is_empty() {
        return None;
    }
    scale_bar(&mut items, &frame);
    title_block(&mut items, "A-301", "Building Section", frame.scale, &scene.name);
    Some(Sheet {
        id,
        name: "A-301 Building Section".into(),
        size_mm: PAGE,
        scale: frame.scale,
        items,
        links,
        story: None,
    })
}

/// The sheets to show: the file's own when it has them, generated ones
/// otherwise.
pub fn sheets_for(scene: &Scene, settings: &plan::PlanSettings) -> Vec<Sheet> {
    if !scene.sheets.is_empty() {
        return scene.sheets.clone();
    }
    if scene.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut order: Vec<usize> = (0..scene.stories.len()).collect();
    order.sort_by(|a, b| {
        scene.stories[*a]
            .elevation
            .partial_cmp(&scene.stories[*b].elevation)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for i in order {
        if let Some(s) = plan_for_story(scene, i, SheetId::from_index(out.len()), settings) {
            out.push(s);
        }
    }
    for view in [
        ElevationView::South,
        ElevationView::North,
        ElevationView::East,
        ElevationView::West,
    ] {
        if let Some(sheet) = elevation(scene, SheetId::from_index(out.len()), view) {
            out.push(sheet);
        }
    }
    if let Some(sheet) = building_section(scene, SheetId::from_index(out.len())) {
        out.push(sheet);
    }
    out
}

/// True when what we are showing was generated here, not published in the
/// file — the sheet view says so, and the report asks lane A for the real one.
pub fn is_generated(scene: &Scene) -> bool {
    scene.sheets.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_house_makes_plans_and_an_elevation() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let sheets = sheets_for(&scene, &plan::PlanSettings::default());
        assert!(sheets.len() >= 2, "got {} sheets", sheets.len());
        assert!(sheets.iter().any(|s| s.name.contains("Elevation")));
        for s in &sheets {
            assert_eq!(s.size_mm, PAGE);
            assert!(s.scale >= 20.0);
            assert!(!s.links.is_empty(), "{} has no links", s.name);
            // Everything must land on the paper.
            for l in &s.links {
                assert!(l.rect_mm[0] >= 0.0 && l.rect_mm[2] <= PAGE[0], "{:?}", l.rect_mm);
                assert!(l.rect_mm[1] >= 0.0 && l.rect_mm[3] <= PAGE[1], "{:?}", l.rect_mm);
            }
        }
    }

    #[test]
    fn a_published_sheet_wins_over_a_generated_one() {
        let mut scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        scene.sheets = vec![Sheet {
            id: SheetId::from_index(0),
            name: "A-999 From the file".into(),
            size_mm: [420.0, 297.0],
            scale: 100.0,
            items: Vec::new(),
            links: Vec::new(),
            story: None,
        }];
        let sheets = sheets_for(&scene, &plan::PlanSettings::default());
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].name, "A-999 From the file");
        assert!(!is_generated(&scene));
    }
}
