use super::geometry::*;
use crate::makepad_draw::*;
use std::collections::HashMap;

pub const LABEL_COLLISION_PADDING: f64 = 4.0;
pub const LABEL_VIEW_MARGIN: f64 = 72.0;
pub const LABEL_MIN_PATH_PIXELS: f64 = 24.0;
pub const LABEL_MAX_CANDIDATES_ZOOMED_OUT: usize = 520;
pub const LABEL_MAX_CANDIDATES_MID_ZOOM: usize = 700;
pub const LABEL_MAX_CANDIDATES_DEFAULT: usize = 1200;
pub const LABEL_MAX_SHAPE_ATTEMPTS_ZOOMED_OUT: usize = 520;
pub const LABEL_MAX_SHAPE_ATTEMPTS_MID_ZOOM: usize = 700;
pub const LABEL_MAX_SHAPE_ATTEMPTS_DEFAULT: usize = 1200;
pub const LABEL_GLYPH_ANGLE_BLEND: f32 = 0.35;
pub const LABEL_MAX_GLYPH_TURN_RADIANS: f32 = 0.70;
pub const LABEL_CURVE_RESAMPLE_SPACING: f64 = 6.0;
pub const LABEL_CURVE_MAX_SAMPLES: usize = 192;
pub const LABEL_CURVE_SMOOTH_PASSES: usize = 2;
pub const LABEL_BASELINE_SHIFT_FACTOR: f64 = 1.0;
pub const LABEL_LAYOUT_MAX_CURVATURE: f32 = 1.0;
pub const LABEL_VERTICAL_AXIS_EPSILON: f32 = 0.22;
// One overzoomed z14 tile can hold a whole city's addresses; house numbers
// are lowest-priority and must survive this per-tile cap to ever reach the
// viewport filter.
pub const MAX_TILE_LABELS: usize = 32768;
pub const POINT_LABEL_HALF_SPAN_PIXELS: f32 = 96.0;
pub const ADDRESS_LABEL_MIN_ZOOM: f64 = 16.5;
pub const POI_LABEL_MIN_ZOOM: f64 = 16.0;

// --- Types ---

/// Label draw-color classes (carto-style): 0 default text, 1 food/drink
/// amenity (orange), 2 shop (purple), 3 culture/tourism (brown), 4 muted
/// (house numbers).
pub const LABEL_CLASS_DEFAULT: u8 = 0;
pub const LABEL_CLASS_AMENITY: u8 = 1;
pub const LABEL_CLASS_SHOP: u8 = 2;
pub const LABEL_CLASS_CULTURE: u8 = 3;
pub const LABEL_CLASS_MUTED: u8 = 4;
pub const LABEL_CLASS_HEALTH: u8 = 5;
pub const LABEL_CLASS_GREEN: u8 = 6;
/// Transport-blue symbols (bicycle parking etc.), carto 0x0092da.
pub const LABEL_CLASS_TRANSPORT: u8 = 7;
/// Tree canopy green discs.
pub const LABEL_CLASS_TREE: u8 = 8;

#[derive(Clone, Debug)]
pub struct TileLabel {
    pub text: String,
    pub priority: u8,
    pub source_layer: String,
    pub road_kind: String,
    pub color_class: u8,
    pub path_points: Vec<(f32, f32)>,
    /// Precomputed at tile build (compact_tile_labels): normalized text key
    /// and tile-local path bbox, so the per-frame scan over thousands of
    /// labels does no allocation and can reject offscreen labels with two
    /// point transforms.
    pub name_key: String,
    pub bbox: (f32, f32, f32, f32),
}

#[derive(Clone, Debug)]
pub struct LabelCandidate {
    pub text: String,
    pub name_key: String,
    pub road_kind: String,
    pub color_class: u8,
    pub source_rank: u8,
    pub score: f64,
    pub path_length: f64,
    pub center: Vec2d,
    pub repeat_distance: f64,
    pub font_scale: f32,
    pub screen_path: Vec<Vec2d>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LabelPerfStats {
    pub draw_tiles: usize,
    pub tiles_with_labels: usize,
    pub labels_in_tiles: usize,
    pub labels_scanned: usize,
    pub candidates: usize,
    pub candidates_kept: usize,
    pub shape_budget: usize,
    pub shaped_attempts: usize,
    pub shaped_ok: usize,
    pub rejected_repeat: usize,
    pub rejected_plan_none: usize,
    pub rejected_pre_short: usize,
    pub rejected_outside: usize,
    pub rejected_collision: usize,
    pub rejected_budget: usize,
    pub drawn_labels: usize,
    pub drawn_glyphs: usize,
}

// --- Label extraction ---

pub fn extract_way_label(
    tags: &HashMap<String, String>,
    points: &[(f32, f32)],
) -> Option<TileLabel> {
    if points.len() < 2 {
        return None;
    }
    if !tags.contains_key("highway") {
        return None;
    }
    let source_layer = tags.get("layer").cloned().unwrap_or_default();
    if is_road_polygon_layer(&source_layer) {
        return None;
    }
    if label_source_rank(&source_layer).is_none() {
        return None;
    }
    let name = select_label_text(tags)?;
    let road_kind = tags
        .get("highway")
        .cloned()
        .unwrap_or_else(|| "residential".to_string());
    let priority = road_label_priority(&road_kind);
    let path_points = simplify_label_path(points);
    if path_points.len() < 2 {
        return None;
    }
    Some(TileLabel {
        text: name,
        priority,
        source_layer,
        road_kind,
        color_class: LABEL_CLASS_DEFAULT,
        path_points,
        name_key: String::new(),
        bbox: (0.0, 0.0, 0.0, 0.0),
    })
}

/// Solid label-class colors (light theme values), for geometry like icons
/// that bakes the color into vertices.
pub fn poi_class_hex(color_class: u8) -> u32 {
    match color_class {
        LABEL_CLASS_AMENITY => 0xc77400,
        LABEL_CLASS_SHOP => 0xac39ac,
        LABEL_CLASS_CULTURE => 0x734a08,
        LABEL_CLASS_MUTED => 0x66768d,
        LABEL_CLASS_HEALTH => 0xbf0000,
        LABEL_CLASS_GREEN => 0x267d3f,
        LABEL_CLASS_TRANSPORT => 0x0092da,
        LABEL_CLASS_TREE => 0x69a05f,
        _ => 0x444444,
    }
}

/// Carto-style POI color grouping from shortbread poi attributes.
pub fn poi_color_class(tags: &HashMap<String, String>) -> u8 {
    if tags.contains_key("shop") {
        return LABEL_CLASS_SHOP;
    }
    if let Some(amenity) = tags.get("amenity") {
        return match amenity.as_str() {
            "restaurant" | "cafe" | "fast_food" | "bar" | "pub" | "biergarten" | "food_court"
            | "ice_cream" | "nightclub" => LABEL_CLASS_AMENITY,
            "theatre" | "cinema" | "arts_centre" | "library" | "museum" | "place_of_worship" => {
                LABEL_CLASS_CULTURE
            }
            "pharmacy" | "doctors" | "dentist" | "clinic" | "hospital" | "veterinary" => {
                LABEL_CLASS_HEALTH
            }
            _ => LABEL_CLASS_DEFAULT,
        };
    }
    if tags.contains_key("tourism") || tags.contains_key("historic") {
        return LABEL_CLASS_CULTURE;
    }
    LABEL_CLASS_DEFAULT
}

/// Name label for a named green area (park/garden), placed at its centroid.
pub fn extract_area_label(
    tags: &HashMap<String, String>,
    centroid: (f32, f32),
) -> Option<TileLabel> {
    let is_green = tags.contains_key("leisure")
        || matches!(
            tags.get("landuse").map(|value| value.as_str()),
            Some("grass" | "forest" | "meadow" | "village_green" | "recreation_ground" | "cemetery")
        );
    if !is_green {
        return None;
    }
    let name = select_label_text(tags)?;
    Some(TileLabel {
        text: name,
        priority: 3,
        source_layer: "green_area".to_string(),
        road_kind: format!("area{:.0}x{:.0}", centroid.0 * 4.0, centroid.1 * 4.0),
        color_class: LABEL_CLASS_GREEN,
        path_points: point_label_path(centroid),
        name_key: String::new(),
        bbox: (0.0, 0.0, 0.0, 0.0),
    })
}

pub fn extract_point_label(tags: &HashMap<String, String>, point: (f32, f32)) -> Option<TileLabel> {
    let source_layer = tags.get("layer").cloned().unwrap_or_default();
    match source_layer.as_str() {
        "addresses" => {
            let number = tags
                .get("housenumber")
                .or_else(|| tags.get("housename"))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && value.len() <= 8)?;
            return Some(TileLabel {
                text: number,
                priority: 4,
                source_layer,
                // unique per point so per-tile compaction keeps every number
                road_kind: format!("addr{:.0}x{:.0}", point.0 * 4.0, point.1 * 4.0),
                color_class: LABEL_CLASS_MUTED,
                path_points: point_label_path(point),
                name_key: String::new(),
                bbox: (0.0, 0.0, 0.0, 0.0),
            });
        }
        "pois" => {
            let name = select_label_text(tags)?;
            let color_class = poi_color_class(tags);
            return Some(TileLabel {
                text: name,
                priority: 3,
                source_layer,
                road_kind: format!("poi{:.0}x{:.0}", point.0 * 4.0, point.1 * 4.0),
                color_class,
                path_points: point_label_path(point),
                name_key: String::new(),
                bbox: (0.0, 0.0, 0.0, 0.0),
            });
        }
        _ => {}
    }
    if !tags.contains_key("highway") {
        return None;
    }
    if !is_road_point_label_layer(&source_layer) {
        return None;
    }
    if is_road_polygon_layer(&source_layer) {
        return None;
    }
    if label_source_rank(&source_layer).is_none() {
        return None;
    }
    let name = select_label_text(tags)?;
    let road_kind = tags
        .get("highway")
        .cloned()
        .unwrap_or_else(|| "residential".to_string());
    let priority = road_label_priority(&road_kind);
    Some(TileLabel {
        text: name,
        priority,
        source_layer,
        road_kind,
        color_class: LABEL_CLASS_DEFAULT,
        path_points: point_label_path(point),
                name_key: String::new(),
                bbox: (0.0, 0.0, 0.0, 0.0),
    })
}

pub fn compact_tile_labels(labels: &mut Vec<TileLabel>) {
    let mut by_street = HashMap::<(String, String), (f32, TileLabel)>::new();
    for mut label in labels.drain(..) {
        if label.path_points.len() < 2 {
            continue;
        }
        let name_key = normalize_label_key(&label.text);
        // single-character texts are valid house numbers
        let min_len = if label.source_layer == "addresses" { 1 } else { 2 };
        if name_key.len() < min_len {
            continue;
        }
        label.name_key = name_key.clone();
        let mut bbox = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for point in &label.path_points {
            bbox.0 = bbox.0.min(point.0);
            bbox.1 = bbox.1.min(point.1);
            bbox.2 = bbox.2.max(point.0);
            bbox.3 = bbox.3.max(point.1);
        }
        label.bbox = bbox;
        let key = (name_key, label.road_kind.clone());
        let length = polyline_length_f32(&label.path_points);
        let replace = match by_street.get(&key) {
            None => true,
            Some((best_len, best_label)) => {
                if length > *best_len + 1.0 {
                    true
                } else if (length - *best_len).abs() <= 1.0 {
                    let rank = label_source_rank(&label.source_layer).unwrap_or(0);
                    let best_rank = label_source_rank(&best_label.source_layer).unwrap_or(0);
                    rank > best_rank
                } else {
                    false
                }
            }
        };
        if replace {
            by_street.insert(key, (length, label));
        }
    }

    let mut compacted = by_street.into_values().collect::<Vec<_>>();
    compacted.sort_unstable_by(|a, b| {
        let a_label = &a.1;
        let b_label = &b.1;
        let a_rank = label_source_rank(&a_label.source_layer).unwrap_or(0);
        let b_rank = label_source_rank(&b_label.source_layer).unwrap_or(0);
        a_label
            .priority
            .cmp(&b_label.priority)
            .then_with(|| b_rank.cmp(&a_rank))
            .then_with(|| b.0.total_cmp(&a.0))
            .then_with(|| a_label.text.cmp(&b_label.text))
    });
    labels.extend(
        compacted
            .into_iter()
            .take(MAX_TILE_LABELS)
            .map(|(_, label)| label),
    );
}

// --- Label placement algorithms ---

pub fn normalize_label_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_space = false;
        } else if ch.is_whitespace() && !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

pub fn label_source_rank(layer: &str) -> Option<u8> {
    if layer.is_empty() {
        return Some(4);
    }
    Some(match layer {
        "street_labels" | "street_labels_points" => 7,
        "streets_polygons_labels" => 6,
        "transportation_name" => 6,
        "pois" => 3,
        "green_area" => 3,
        "transportation" | "road" | "streets" | "bridges" | "aerialways" | "ferries"
        | "public_transport" => 2,
        "addresses" => 1,
        _ => return None,
    })
}

pub fn repeat_distance_for_label(priority: u8, source_rank: u8) -> f64 {
    let base = match priority {
        1 => 220.0,
        2 => 170.0,
        _ => 120.0,
    };
    base + (source_rank as f64 - 4.0) * 10.0
}

pub fn label_candidate_budget(view_zoom: f64) -> usize {
    if view_zoom < 14.25 {
        LABEL_MAX_CANDIDATES_ZOOMED_OUT
    } else if view_zoom < 15.0 {
        LABEL_MAX_CANDIDATES_MID_ZOOM
    } else if view_zoom < 16.5 {
        LABEL_MAX_CANDIDATES_DEFAULT
    } else {
        // house-number zooms: few labels survive the viewport filter, so a
        // large budget costs little and shows every number
        2400
    }
}

pub fn label_shape_attempt_budget(view_zoom: f64) -> usize {
    if view_zoom < 14.25 {
        LABEL_MAX_SHAPE_ATTEMPTS_ZOOMED_OUT
    } else if view_zoom < 15.0 {
        LABEL_MAX_SHAPE_ATTEMPTS_MID_ZOOM
    } else if view_zoom < 16.5 {
        LABEL_MAX_SHAPE_ATTEMPTS_DEFAULT
    } else {
        2400
    }
}

pub fn estimate_label_width_pixels(text: &str, font_scale: f32) -> f64 {
    let mut units = 0.0_f64;
    for ch in text.chars() {
        units += if ch.is_whitespace() {
            0.45
        } else if ch.is_ascii_uppercase() {
            1.08
        } else if ch.is_ascii_digit() {
            0.86
        } else {
            0.92
        };
    }
    (units * 8.0 * font_scale as f64 + 6.0).max(12.0)
}

pub fn road_label_priority(road_kind: &str) -> u8 {
    match road_kind {
        "motorway" | "trunk" | "primary" => 1,
        "secondary" | "tertiary" => 2,
        _ => 3,
    }
}

pub fn is_road_point_label_layer(layer: &str) -> bool {
    matches!(
        layer,
        "street_labels"
            | "street_labels_points"
            | "streets_polygons_labels"
            | "transportation_name"
    )
}

fn point_label_path(point: (f32, f32)) -> Vec<(f32, f32)> {
    vec![
        (point.0 - POINT_LABEL_HALF_SPAN_PIXELS, point.1),
        (point.0 + POINT_LABEL_HALF_SPAN_PIXELS, point.1),
    ]
}

// --- Curve smoothing ---

/// Smooths `points` into `buf_a`, using `buf_b` and `cum` as scratch space.
/// On return, `buf_a` contains the smoothed polyline.
pub fn smooth_label_curve_into(
    points: &[Vec2d],
    buf_a: &mut Vec<Vec2d>,
    buf_b: &mut Vec<Vec2d>,
    cum: &mut Vec<f64>,
) {
    buf_a.clear();
    if points.len() < 3 {
        buf_a.extend_from_slice(points);
        return;
    }
    cum.clear();
    polyline_cumulative_lengths_into(points, cum);
    let total = *cum.last().unwrap_or(&0.0);
    if total < 12.0 {
        buf_a.extend_from_slice(points);
        return;
    }

    resample_polyline_evenly_into(
        points,
        cum,
        LABEL_CURVE_RESAMPLE_SPACING,
        LABEL_CURVE_MAX_SAMPLES,
        buf_a,
    );

    for _ in 0..LABEL_CURVE_SMOOTH_PASSES {
        smooth_polyline_once_into(buf_a, buf_b);
        std::mem::swap(buf_a, buf_b);
    }
}

fn resample_polyline_evenly_into(
    points: &[Vec2d],
    cumulative: &[f64],
    spacing: f64,
    max_samples: usize,
    out: &mut Vec<Vec2d>,
) {
    out.clear();
    let total = *cumulative.last().unwrap_or(&0.0);
    if points.len() < 2 || total <= 1e-6 {
        out.extend_from_slice(points);
        return;
    }

    let spacing = spacing.max(1.0);
    let mut sample_count = (total / spacing).ceil() as usize + 1;
    sample_count = sample_count.clamp(2, max_samples.max(2));

    for i in 0..sample_count {
        let t = if sample_count <= 1 {
            0.0
        } else {
            i as f64 / (sample_count - 1) as f64
        };
        let d = total * t;
        if let Some(point) = sample_polyline_point_at_distance(points, cumulative, d) {
            let push = match out.last() {
                None => true,
                Some(last) => {
                    let dx = point.x - last.x;
                    let dy = point.y - last.y;
                    dx * dx + dy * dy > 1e-3
                }
            };
            if push {
                out.push(point);
            }
        }
    }
    if out.len() < 2 {
        out.clear();
        out.extend_from_slice(points);
    }
}

fn smooth_polyline_once_into(src: &[Vec2d], dst: &mut Vec<Vec2d>) {
    dst.clear();
    if src.len() < 3 {
        dst.extend_from_slice(src);
        return;
    }
    dst.push(src[0]);
    for i in 1..src.len() - 1 {
        let prev = src[i - 1];
        let cur = src[i];
        let next = src[i + 1];
        dst.push(dvec2(
            (prev.x + 2.0 * cur.x + next.x) * 0.25,
            (prev.y + 2.0 * cur.y + next.y) * 0.25,
        ));
    }
    dst.push(src[src.len() - 1]);
}

// --- Label layout ---

pub fn choose_label_start_distance(
    points: &[Vec2d],
    cumulative: &[f64],
    text_width: f64,
) -> Option<f64> {
    let total = *cumulative.last()?;
    if total < text_width + 4.0 {
        return None;
    }
    let max_start = (total - text_width).max(0.0);
    if max_start <= 1e-6 {
        return Some(0.0);
    }

    // Scan in normalized coordinates (fraction of path length) so the
    // chosen position stays stable as the screen path scales with zoom.
    let text_frac = text_width / total;
    let max_frac = 1.0 - text_frac;
    let scan_steps = 24_usize; // fixed number of probes, independent of scale
    let scan_step_frac = if scan_steps > 1 {
        max_frac / scan_steps as f64
    } else {
        max_frac
    };
    let angle_delta = (text_width * 0.20).clamp(8.0, 28.0);
    let mut best_score = f32::INFINITY;
    let mut best_frac: Option<f64> = None;

    for i in 0..=scan_steps {
        let frac = (i as f64 * scan_step_frac).min(max_frac);
        let start = frac * total;
        let q1 = start + text_width * 0.25;
        let mid = start + text_width * 0.5;
        let q3 = start + text_width * 0.75;
        let Some(a1_raw) = sample_polyline_tangent_angle_raw(points, cumulative, q1, angle_delta)
        else {
            continue;
        };
        let Some(a2_raw) = sample_polyline_tangent_angle_raw(points, cumulative, mid, angle_delta)
        else {
            continue;
        };
        let Some(a3_raw) = sample_polyline_tangent_angle_raw(points, cumulative, q3, angle_delta)
        else {
            continue;
        };

        let a2 = a2_raw;
        let a1 = nearest_equivalent_angle(a2, a1_raw);
        let a3 = nearest_equivalent_angle(a2, a3_raw);
        let curvature = wrap_angle_pi(a1 - a2).abs() + wrap_angle_pi(a3 - a2).abs();
        let mid_frac = frac + text_frac * 0.5;
        let mid_bias = ((mid_frac - 0.5).abs()) as f32 * 0.10;
        let score = curvature + mid_bias;
        if score < best_score {
            best_score = score;
            best_frac = Some(frac);
        }
    }

    let best_frac = best_frac?;
    if best_score > LABEL_LAYOUT_MAX_CURVATURE {
        return None;
    }
    Some((best_frac * total).min(max_start))
}

pub fn choose_label_reverse(mid_angle: f32) -> bool {
    let cos = mid_angle.cos();
    let sin = mid_angle.sin();
    if cos.abs() > LABEL_VERTICAL_AXIS_EPSILON {
        cos < 0.0
    } else {
        // Near vertical, keep a deterministic reading direction.
        // Screen-space y grows downward; prefer bottom-to-top labels.
        sin < 0.0
    }
}

// --- Angle utilities ---

pub fn wrap_angle_pi(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    while angle < -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    angle
}

pub fn nearest_equivalent_angle(reference: f32, angle: f32) -> f32 {
    let mut out = angle;
    while out - reference > std::f32::consts::PI {
        out -= std::f32::consts::TAU;
    }
    while out - reference < -std::f32::consts::PI {
        out += std::f32::consts::TAU;
    }
    out
}
