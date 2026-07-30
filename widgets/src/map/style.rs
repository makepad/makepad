use super::geometry::{is_road_polygon_layer, tag_is, tag_is_truthy};
use crate::makepad_draw::*;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MapFillRule = #(MapFillRule::script_api(vm))
    mod.widgets.MapRoadRule = #(MapRoadRule::script_api(vm))
    mod.widgets.MapWaterwayRule = #(MapWaterwayRule::script_api(vm))
    mod.widgets.MapRailRule = #(MapRailRule::script_api(vm))
    mod.widgets.MapShinyStyle = #(MapShinyStyle::script_api(vm))
    mod.widgets.MapThemeStyle = #(MapThemeStyle::script_component(vm))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokePassStyle {
    pub color: u32,
    pub width: f32,
    pub shape_id: f32,
    /// Width-growth class for GPU stroke re-expansion: which zoom curve the
    /// baked width follows, so stale-bucket tiles can be corrected to the
    /// width the current view zoom calls for.
    pub expand_class: f32,
    /// Tilt-mode micro-depth (sort-rank scaled): separates overlapping
    /// strokes (rail over road) by more than the depth-buffer quantum while
    /// staying far below one ground pixel of view depth.
    pub depth_micro: f32,
    /// 3D bridge deck height in meters (0 = grounded). Tapered to zero at
    /// the segment ends by the stroke appender so approaches read as ramps.
    pub deck_m: f32,
}

/// Micro-depth per unit of sort rank (rank 710 rail → 0.0014).
pub const DEPTH_MICRO_PER_RANK: f32 = 2e-4;
/// Per-feature micro ladder for SAME-rank overlapping fills (park over
/// grass): keeps bake order deterministic where ranks tie, wraps well
/// below one rank step.
pub const DEPTH_MICRO_PER_FEATURE: f32 = 1e-5;

/// Regular roads: widths follow `zoom_width_mult` directly.
pub const EXPAND_CLASS_ROAD: f32 = 0.0;
/// Thin uncased paths + rails: grow as `max(1, mult)^0.35`.
pub const EXPAND_CLASS_THIN: f32 = 1.0;
/// Waterway centerlines: shrink as `mult^1.6` below z14, linear above.
pub const EXPAND_CLASS_WATER: f32 = 2.0;
/// Constant screen-px strokes (building outlines).
pub const EXPAND_CLASS_CONST_PX: f32 = 3.0;

/// Per-class width correction for drawing geometry baked at `bucket` while
/// the camera sits at (fractional) `view_zoom`: multiplying the baked stroke
/// offsets by this factor yields the width a fresh restyle at `view_zoom`
/// would bake. x=road, y=thin, z=water, w=constant-px.
pub fn stroke_width_correction(bucket: u32, view_zoom: f64) -> [f32; 4] {
    let geom_scale = 2.0_f64.powf(view_zoom - bucket as f64);
    let s_view = zoom_width_mult_continuous(view_zoom);
    let s_bucket = zoom_width_mult(bucket) as f64;
    let thin = |s: f64| s.max(1.0).powf(0.35);
    let water = |s: f64| if s < 1.0 { s.powf(1.6) } else { s };
    [
        (s_view / (s_bucket * geom_scale)) as f32,
        (thin(s_view) / (thin(s_bucket) * geom_scale)) as f32,
        (water(s_view) / (water(s_bucket) * geom_scale)) as f32,
        (1.0 / geom_scale) as f32,
    ]
}

/// `zoom_width_mult` interpolated between integer buckets so corrections
/// stay smooth through a zoom gesture.
fn zoom_width_mult_continuous(view_zoom: f64) -> f64 {
    let floor = view_zoom.floor().max(0.0);
    let frac = (view_zoom - floor).clamp(0.0, 1.0);
    let a = zoom_width_mult(floor as u32) as f64;
    let b = zoom_width_mult(floor as u32 + 1) as f64;
    a + (b - a) * frac
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeStyle {
    pub sort_rank: i16,
    pub casing: Option<StrokePassStyle>,
    pub center: StrokePassStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct StrokeTemplate {
    sort_rank: i16,
    casing: Option<StrokePassStyle>,
    center: StrokePassStyle,
    /// Lowest render bucket this rule draws at (carto hides paths and small
    /// waterways well before roads).
    min_zoom: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledMapTheme {
    pub background: Vec4f,
    pub status_text: Vec4f,
    pub label: Vec4f,
    pub label_halo: Vec4f,
    building_fill: Option<u32>,
    pub building_outline: Option<u32>,
    street_area_fill: Option<u32>,
    bridge_area_fill: Option<u32>,
    water_fill: Option<u32>,
    landuse_fills: HashMap<String, u32>,
    landuse_default: Option<u32>,
    leisure_fills: HashMap<String, u32>,
    leisure_default: Option<u32>,
    road_rules: HashMap<String, StrokeTemplate>,
    road_default: Option<StrokeTemplate>,
    waterway_rules: HashMap<String, StrokeTemplate>,
    waterway_default: Option<StrokeTemplate>,
    railway_rule: Option<StrokeTemplate>,
    /// shiny.md lighting config: bake flags + the one SceneSun. Lives in
    /// the compiled theme so flipping any bake flag rides the existing
    /// style-epoch restyle (stale tiles stay drawable while rebaking).
    pub shiny: ShinyConfig,
}

impl Default for CompiledMapTheme {
    fn default() -> Self {
        Self {
            background: Vec4f::from_u32(0xddd7ccff),
            status_text: Vec4f::from_u32(0xdee9f4ff),
            label: Vec4f::from_u32(0x000000ff),
            label_halo: Vec4f::from_u32(0xffffffff),
            building_fill: None,
            building_outline: None,
            street_area_fill: None,
            bridge_area_fill: None,
            water_fill: None,
            landuse_fills: HashMap::new(),
            landuse_default: None,
            leisure_fills: HashMap::new(),
            leisure_default: None,
            road_rules: HashMap::new(),
            road_default: None,
            waterway_rules: HashMap::new(),
            waterway_default: None,
            railway_rule: None,
            shiny: ShinyConfig::default(),
        }
    }
}

/// DSL-facing shiny.md switches, one per theme (a dark preset can run
/// building sheen + route glow while the day theme keeps them off).
/// Compiles into the plain `ShinyConfig` POD carried by the compiled theme.
#[derive(Script, ScriptHook, Clone)]
pub struct MapShinyStyle {
    #[source]
    source: ScriptObjectRef,
    #[live(false)]
    pub bake_ao: bool,
    #[live(false)]
    pub bake_bounce: bool,
    #[live(false)]
    pub bake_shadows: bool,
    #[live(false)]
    pub terrain_shadows: bool,
    #[live(false)]
    pub dynamic_sun: bool,
    #[live(false)]
    pub water_fx: bool,
    #[live(false)]
    pub building_sheen: bool,
    #[live(false)]
    pub foliage_fx: bool,
    #[live(false)]
    pub route_glow: bool,
    #[live(false)]
    pub bloom: bool,
    #[live(false)]
    pub tilt_shift: bool,
    /// Local solar time driving the sun position; negative keeps the
    /// legacy fixed NW sun (today's exact look).
    #[live(-1.0)]
    pub sun_hours: f32,
    #[live(52.0)]
    pub sun_latitude: f32,
    /// Overrides the sun's shadow alpha when >= 0.
    #[live(-1.0)]
    pub shadow_alpha: f32,
}

impl Default for MapShinyStyle {
    fn default() -> Self {
        Self {
            source: Default::default(),
            bake_ao: false,
            bake_bounce: false,
            bake_shadows: false,
            terrain_shadows: false,
            dynamic_sun: false,
            water_fx: false,
            building_sheen: false,
            foliage_fx: false,
            route_glow: false,
            bloom: false,
            tilt_shift: false,
            sun_hours: -1.0,
            sun_latitude: 52.0,
            shadow_alpha: -1.0,
        }
    }
}

impl MapShinyStyle {
    pub fn compile(&self) -> ShinyConfig {
        let mut sun = if self.sun_hours >= 0.0 {
            SceneSun::from_time_of_day(self.sun_hours, self.sun_latitude)
        } else {
            SceneSun::default()
        };
        if self.shadow_alpha >= 0.0 {
            sun.shadow_alpha = self.shadow_alpha;
        }
        ShinyConfig {
            bake_ao: self.bake_ao,
            bake_bounce: self.bake_bounce,
            bake_shadows: self.bake_shadows,
            terrain_shadows: self.terrain_shadows,
            dynamic_sun: self.dynamic_sun,
            water_fx: self.water_fx,
            building_sheen: self.building_sheen,
            foliage_fx: self.foliage_fx,
            route_glow: self.route_glow,
            bloom: self.bloom,
            tilt_shift: self.tilt_shift,
            xr_shadow_map: false,
            sun,
        }
    }
}

#[derive(Script, ScriptHook, Clone, Default)]
pub struct MapFillRule {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub group: String,
    #[live]
    pub value: String,
    #[live]
    pub color: Vec4f,
}

#[derive(Script, ScriptHook, Clone, Default)]
pub struct MapRoadRule {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub kind: String,
    #[live]
    pub sort_rank: u32,
    #[live]
    pub casing_color: Vec4f,
    #[live]
    pub casing_width: f32,
    #[live]
    pub casing_shape_id: f32,
    #[live]
    pub center_color: Vec4f,
    #[live]
    pub center_width: f32,
    #[live]
    pub center_shape_id: f32,
    /// Hidden below this render zoom (0 = always visible).
    #[live]
    pub min_zoom: f32,
}

#[derive(Script, ScriptHook, Clone, Default)]
pub struct MapWaterwayRule {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub kind: String,
    #[live]
    pub sort_rank: u32,
    #[live]
    pub casing_color: Vec4f,
    #[live]
    pub casing_width: f32,
    #[live]
    pub casing_shape_id: f32,
    #[live]
    pub center_color: Vec4f,
    #[live]
    pub center_width: f32,
    #[live]
    pub center_shape_id: f32,
    /// Hidden below this render zoom (0 = always visible).
    #[live]
    pub min_zoom: f32,
}

#[derive(Script, ScriptHook, Clone, Default)]
pub struct MapRailRule {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub sort_rank: u32,
    #[live]
    pub casing_color: Vec4f,
    #[live]
    pub casing_width: f32,
    #[live]
    pub casing_shape_id: f32,
    #[live]
    pub center_color: Vec4f,
    #[live]
    pub center_width: f32,
    #[live]
    pub center_shape_id: f32,
}

#[derive(Script, Clone)]
pub struct MapThemeStyle {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub background: Vec4f,
    #[live]
    pub status_text: Vec4f,
    #[live]
    pub label: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub label_halo: Vec4f,
    #[live]
    pub shiny: MapShinyStyle,
    #[rust]
    fill_rules: Vec<MapFillRule>,
    #[rust]
    road_rules: Vec<MapRoadRule>,
    #[rust]
    waterway_rules: Vec<MapWaterwayRule>,
    #[rust]
    railway_rule: Option<MapRailRule>,
}

impl Default for MapThemeStyle {
    fn default() -> Self {
        Self {
            source: Default::default(),
            background: Vec4f::from_u32(0xddd7ccff),
            status_text: Vec4f::from_u32(0xdee9f4ff),
            label: Vec4f::from_u32(0x000000ff),
            label_halo: Vec4f::from_u32(0xffffffff),
            shiny: MapShinyStyle::default(),
            fill_rules: Vec::new(),
            road_rules: Vec::new(),
            waterway_rules: Vec::new(),
            railway_rule: None,
        }
    }
}

impl ScriptHook for MapThemeStyle {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        self.fill_rules.clear();
        self.road_rules.clear();
        self.waterway_rules.clear();
        self.railway_rule = None;

        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    let Some(obj) = kv.value.as_object() else {
                        continue;
                    };
                    if vm
                        .bx
                        .heap
                        .type_matches_id(obj, MapFillRule::script_type_id_static())
                    {
                        self.fill_rules
                            .push(MapFillRule::script_from_value(vm, kv.value));
                    } else if vm
                        .bx
                        .heap
                        .type_matches_id(obj, MapRoadRule::script_type_id_static())
                    {
                        self.road_rules
                            .push(MapRoadRule::script_from_value(vm, kv.value));
                    } else if vm
                        .bx
                        .heap
                        .type_matches_id(obj, MapWaterwayRule::script_type_id_static())
                    {
                        self.waterway_rules
                            .push(MapWaterwayRule::script_from_value(vm, kv.value));
                    } else if vm
                        .bx
                        .heap
                        .type_matches_id(obj, MapRailRule::script_type_id_static())
                    {
                        self.railway_rule = Some(MapRailRule::script_from_value(vm, kv.value));
                    }
                }
            });
        }
    }
}

impl MapThemeStyle {
    pub fn compile(&self) -> CompiledMapTheme {
        let mut compiled = CompiledMapTheme {
            background: self.background,
            status_text: self.status_text,
            label: self.label,
            label_halo: self.label_halo,
            shiny: self.shiny.compile(),
            ..CompiledMapTheme::default()
        };

        for rule in &self.fill_rules {
            let group = rule.group.trim().to_ascii_lowercase();
            if group.is_empty() {
                continue;
            }
            let value = rule.value.trim().to_ascii_lowercase();
            let color = vec4_to_rgb_hex(rule.color);

            match group.as_str() {
                "building" => compiled.building_fill = Some(color),
                "building_outline" => compiled.building_outline = Some(color),
                "street_area" => compiled.street_area_fill = Some(color),
                "bridge_area" => compiled.bridge_area_fill = Some(color),
                "water" => compiled.water_fill = Some(color),
                "landuse" => {
                    if is_default_key(value.as_str()) {
                        compiled.landuse_default = Some(color);
                    } else {
                        compiled.landuse_fills.insert(value, color);
                    }
                }
                "leisure" => {
                    if is_default_key(value.as_str()) {
                        compiled.leisure_default = Some(color);
                    } else {
                        compiled.leisure_fills.insert(value, color);
                    }
                }
                _ => {}
            }
        }

        for rule in &self.road_rules {
            let kind = rule.kind.trim().to_ascii_lowercase();
            let template = stroke_template_from_road_rule(rule);
            if is_default_key(kind.as_str()) {
                compiled.road_default = Some(template);
            } else {
                compiled.road_rules.insert(kind, template);
            }
        }

        for rule in &self.waterway_rules {
            let kind = rule.kind.trim().to_ascii_lowercase();
            let template = stroke_template_from_waterway_rule(rule);
            if is_default_key(kind.as_str()) {
                compiled.waterway_default = Some(template);
            } else {
                compiled.waterway_rules.insert(kind, template);
            }
        }

        if let Some(rule) = &self.railway_rule {
            compiled.railway_rule = Some(stroke_template_from_rail_rule(rule));
        }

        compiled
    }
}

fn stroke_template_from_road_rule(rule: &MapRoadRule) -> StrokeTemplate {
    StrokeTemplate {
        sort_rank: clamp_u32_to_i16(rule.sort_rank),
        casing: if rule.casing_width > 0.0 {
            Some(StrokePassStyle { deck_m: 0.0,
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
                expand_class: EXPAND_CLASS_ROAD,
                depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK,
            })
        } else {
            None
        },
        center: StrokePassStyle { deck_m: 0.0,
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
            expand_class: EXPAND_CLASS_ROAD,
            depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK,
        },
        min_zoom: rule.min_zoom,
    }
}

fn stroke_template_from_waterway_rule(rule: &MapWaterwayRule) -> StrokeTemplate {
    StrokeTemplate {
        sort_rank: clamp_u32_to_i16(rule.sort_rank),
        casing: if rule.casing_width > 0.0 {
            Some(StrokePassStyle { deck_m: 0.0,
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
                expand_class: EXPAND_CLASS_WATER,
                depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK,
            })
        } else {
            None
        },
        center: StrokePassStyle { deck_m: 0.0,
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
            expand_class: EXPAND_CLASS_WATER,
            depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK,
        },
        min_zoom: rule.min_zoom,
    }
}

fn stroke_template_from_rail_rule(rule: &MapRailRule) -> StrokeTemplate {
    StrokeTemplate {
        sort_rank: clamp_u32_to_i16(rule.sort_rank),
        casing: if rule.casing_width > 0.0 {
            Some(StrokePassStyle { deck_m: 0.0,
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
                expand_class: EXPAND_CLASS_THIN,
                depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK,
            })
        } else {
            None
        },
        center: StrokePassStyle { deck_m: 0.0,
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
            expand_class: EXPAND_CLASS_THIN,
            depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK,
        },
        min_zoom: 0.0,
    }
}

fn is_default_key(value: &str) -> bool {
    matches!(value, "" | "*" | "default")
}

fn clamp_u32_to_i16(value: u32) -> i16 {
    value.min(i16::MAX as u32) as i16
}

fn vec4_to_rgb_hex(color: Vec4f) -> u32 {
    color.to_u32() >> 8
}

impl CompiledMapTheme {
    pub fn building_fill_color(&self) -> Option<u32> {
        self.building_fill
    }
}

/// Fill opacity: overlay tints are translucent so the base map reads
/// through; everything else stays opaque.
pub fn fill_alpha_for_tags(tags: &HashMap<String, String>) -> f32 {
    match tags.get("layer").map(|value| value.as_str()) {
        Some("natura2000" | "wetlands") => 0.22,
        Some("vk100" | "vk500") => 0.45,
        Some("gemeenten" | "wijken" | "buurten") => 0.32,
        Some("bag") => 0.85,
        _ => 1.0,
    }
}

/// Procedural fill texture, carto-style: 30 = staggered dot stipple
/// (courtyard gardens), 31 = diagonal hatch (playgrounds), 32 = staggered
/// open circles (woods/forests/cemeteries — tree rings). 0 = solid.
pub fn fill_pattern_shape(tags: &HashMap<String, String>) -> f32 {
    match tags.get("leisure").map(|value| value.as_str()) {
        Some("garden") => return 30.0,
        Some("playground") => return 31.0,
        _ => {}
    }
    if matches!(
        tags.get("landuse").map(|value| value.as_str()),
        Some("cemetery" | "forest")
    ) || tags.get("natural").map(|value| value.as_str()) == Some("wood")
    {
        return 32.0;
    }
    0.0
}

/// Building-age color from BAG bouwjaar — shared by the flat choropleth
/// fill and the 3D building tint.
pub fn bag_year_color(tags: &HashMap<String, String>) -> Option<u32> {
    let bouwjaar = tags
        .get("bouwjaar")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0) as i32;
    Some(match bouwjaar {
        0 => 0xbdbdbd,
        year if year < 1800 => 0x8c2d04,
        year if year < 1900 => 0xcc4c02,
        year if year < 1930 => 0xec7014,
        year if year < 1960 => 0xfe9929,
        year if year < 1980 => 0xfec44f,
        year if year < 2000 => 0x78c679,
        year if year < 2010 => 0x41b6c4,
        _ => 0x225ea8,
    })
}

pub fn fill_color_for_tags(
    theme: &CompiledMapTheme,
    tags: &HashMap<String, String>,
    closed: bool,
    render_zoom: u32,
) -> Option<u32> {
    if !closed {
        return None;
    }

    if tags.contains_key("building") {
        return theme.building_fill;
    }
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) {
        return theme.street_area_fill;
    }
    if matches!(layer, "bridges" | "pier_polygons" | "dam_polygons") {
        return theme.bridge_area_fill;
    }
    // Station platforms: carto draws them as calm gray slabs.
    if layer == "platforms" {
        return theme.bridge_area_fill;
    }
    // Nature reserves tint translucent green (alpha via fill_alpha_for_tags).
    if matches!(layer, "natura2000" | "wetlands") {
        return Some(0x74b787);
    }
    // Building-age choropleth (BAG bouwjaar): rust = old, blue = new.
    if layer == "bag" {
        return bag_year_color(tags);
    }
    // Districts tint as translucent AREA shapes, one tier per zoom band so
    // gemeente/wijk/buurt tints never stack into mud; stable per-district
    // hue from the CBS code.
    if matches!(layer, "gemeenten" | "wijken" | "buurten") {
        let tier_active = match layer {
            "gemeenten" => render_zoom < 11,
            "wijken" => (11..13).contains(&render_zoom),
            _ => render_zoom >= 13,
        };
        if !tier_active {
            return None;
        }
        const DISTRICT_PALETTE: [u32; 8] = [
            0xe57373, 0x64b5f6, 0x81c784, 0xffb74d, 0xba68c8, 0x4db6ac,
            0xf06292, 0xa1887f,
        ];
        let code = tags
            .get("buurtcode")
            .or_else(|| tags.get("wijkcode"))
            .or_else(|| tags.get("gemeentecode"))
            .map(|value| value.as_str())
            .unwrap_or("");
        let mut h: u32 = 5381;
        for b in code.bytes() {
            h = h.wrapping_mul(33) ^ b as u32;
        }
        return Some(DISTRICT_PALETTE[(h % DISTRICT_PALETTE.len() as u32) as usize]);
    }
    // Population choropleth (CBS grid cells), yellow -> deep blue.
    if matches!(layer, "vk100" | "vk500") {
        let population = tags
            .get("aantal_inwoners")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0);
        if population <= 0.0 {
            return None;
        }
        return Some(match population as i64 {
            1..=25 => 0xffffcc,
            26..=100 => 0xc7e9b4,
            101..=250 => 0x7fcdbb,
            251..=500 => 0x41b6c4,
            501..=1000 => 0x2c7fb8,
            _ => 0x253494,
        });
    }
    if tag_is(tags, "natural", "water") || tag_is(tags, "waterway", "riverbank") {
        return theme.water_fill;
    }
    if matches!(
        tags.get("natural").map(|value| value.as_str()),
        Some("scrub" | "heath" | "shrubbery")
    ) {
        // carto scrub green; the detail archive routes these as natural=*.
        return theme.landuse_fills.get("grass").copied().or(theme.landuse_default);
    }
    if matches!(
        tags.get("natural").map(|value| value.as_str()),
        Some("sand" | "beach" | "shingle")
    ) {
        // zoo enclosures, dunes, riverbanks — carto's pale sand tan.
        return theme.landuse_fills.get("sand").copied().or(theme.landuse_default);
    }
    if let Some(landuse) = tags.get("landuse") {
        let key = landuse.trim().to_ascii_lowercase();
        if let Some(color) = theme.landuse_fills.get(&key) {
            return Some(*color);
        }
        return theme.landuse_default;
    }
    if let Some(leisure) = tags.get("leisure") {
        let key = leisure.trim().to_ascii_lowercase();
        if let Some(color) = theme.leisure_fills.get(&key) {
            return Some(*color);
        }
        return theme.leisure_default;
    }
    None
}

/// shiny.md material id (carried in param3 of shape-0 fills) for a fill's
/// tags: water and green areas get per-pixel effects behind uniform gates;
/// everything else stays 0 = the untouched legacy path.
pub fn fill_material_for_tags(tags: &HashMap<String, String>) -> f32 {
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if tag_is(tags, "natural", "water") || tag_is(tags, "waterway", "riverbank") || layer == "ocean"
    {
        return MAT_WATER;
    }
    if matches!(
        tags.get("natural").map(|value| value.as_str()),
        Some("scrub" | "heath" | "shrubbery")
    ) {
        return MAT_GREEN;
    }
    let is_green = matches!(
        tags.get("leisure").map(|value| value.as_str()),
        Some("park" | "nature_reserve" | "garden" | "golf_course" | "pitch" | "village_green")
    ) || matches!(
        tags.get("landuse").map(|value| value.as_str()),
        Some(
            "grass"
                | "forest"
                | "meadow"
                | "farmland"
                | "allotments"
                | "village_green"
                | "recreation_ground"
                | "cemetery"
        )
    );
    if is_green {
        return MAT_GREEN;
    }
    MAT_NONE
}

/// Semantic paint order for the fill pass (carto-like): land/landcover as the
/// base, sites above land, water above both, then buildings, then road areas.
/// Raw MVT layer order painted `land`/`sites` over the buildings.
pub fn fill_layer_rank(tags: &HashMap<String, String>) -> u8 {
    if tags.contains_key("building") {
        return 40;
    }
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) {
        // above base land, but below sites/parks — pedestrian plazas like
        // Bellamyplein must not paint over the park inside them
        return 12;
    }
    if layer == "platforms" {
        return 13;
    }
    // Sand-floored enclosures paint over the zoo's grass.
    if layer == "attraction_area" {
        return 20;
    }
    // Overlay choropleths: population under buildings, building-age above.
    if matches!(layer, "vk100" | "vk500") {
        return 21;
    }
    if layer == "bag" {
        return 41;
    }
    // District tints paint OVER everything ground-level (roads included)
    // so the area reads as one marked shape.
    if matches!(layer, "gemeenten" | "wijken" | "buurten") {
        return 60;
    }
    // Green areas (parks/gardens/grass) rank above generic landuse and sites:
    // they share the `land` layer with huge residential polygons and would
    // otherwise lose to protobuf feature order (Bellamyplein rendered gray).
    let is_green = tags.contains_key("leisure")
        || matches!(
            tags.get("landuse").map(|value| value.as_str()),
            Some(
                "grass"
                    | "forest"
                    | "meadow"
                    | "farmland"
                    | "allotments"
                    | "village_green"
                    | "recreation_ground"
                    | "cemetery"
            )
        );
    match layer {
        "ocean" => 5,
        "land" | "landuse" | "landcover" | "detail_land" => {
            if is_green {
                // Distinct sub-ranks: nested greens (grass patches or a
                // playground inside a park) must never tie — equal ranks
                // shimmer in the tilt-mode micro-depth.
                if let Some(leisure) = tags.get("leisure") {
                    match leisure.as_str() {
                        "park" | "nature_reserve" => 16,
                        "garden" | "golf_course" => 17,
                        "pitch" | "playground" => 19,
                        _ => 16,
                    }
                } else {
                    match tags.get("landuse").map(|value| value.as_str()) {
                        Some("grass") => 18,
                        _ => 17,
                    }
                }
            } else {
                10
            }
        }
        "sites" | "park" | "pois" => 15,
        "water" | "water_polygons" | "water_polygons_labels" => 20,
        "dam_polygons" | "pier_polygons" => 25,
        "bridges" => 45,
        _ => {
            if tag_is(tags, "natural", "water") || tag_is(tags, "waterway", "riverbank") {
                20
            } else if is_green {
                16
            } else {
                10
            }
        }
    }
}

/// Carto-derived road width multiplier per render zoom, relative to the z14
/// baseline widths in the theme (openstreetmap-carto: residential 3px@z14,
/// 5@z15, 6@z16, 12@z17; majors 6@z14 -> 18@z17).
pub fn zoom_width_mult(render_zoom: u32) -> f32 {
    match render_zoom {
        0..=11 => 0.35,
        12 => 0.45,
        13 => 0.7,
        14 => 1.0,
        15 => 1.7,
        16 => 2.1,
        17 => 3.6,
        18 => 4.2,
        _ => 5.0,
    }
}

fn scaled_style(template: StrokeTemplate, rank_bias: i16, width_scale: f32) -> StrokeStyle {
    let rank = (template.sort_rank as i32 + rank_bias as i32)
        .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    StrokeStyle {
        sort_rank: rank,
        casing: template.casing.map(|casing| StrokePassStyle {
            width: casing.width * width_scale,
            ..casing
        }),
        center: StrokePassStyle {
            width: template.center.width * width_scale,
            ..template.center
        },
    }
}

fn is_pedestrian_bridge_member(highway: &str) -> bool {
    matches!(highway, "footway" | "path" | "steps" | "cycleway")
}

pub fn stroke_style_for_tags(
    theme: &CompiledMapTheme,
    tags: &HashMap<String, String>,
    _tile_zoom: u32,
    render_zoom: u32,
    zoom_mult: f32,
    px_to_units: f32,
) -> Option<StrokeStyle> {
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) || layer == "bridges" {
        // Pedestrian squares / wide path areas get carto's thin gray edge
        // from street level; the fill itself comes from the fill pass.
        if is_road_polygon_layer(layer) && render_zoom >= 15 {
            let edge = theme
                .street_area_fill
                .map(contrast_edge)
                .unwrap_or(0xc2bfba);
            return Some(StrokeStyle {
                sort_rank: 135,
                casing: None,
                center: StrokePassStyle { deck_m: 0.0,
                    color: edge,
                    width: 0.8 * px_to_units,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_CONST_PX,
                    depth_micro: 135.0 * DEPTH_MICRO_PER_RANK,
                },
            });
        }
        return None;
    }

    // Label-source geometry exists only to place text. In particular,
    // `street_labels` duplicates/simplifies the physical `streets` ways
    // and does not preserve their bridge segmentation, so admitting it as
    // a stroke creates a second ground-level road beneath elevated decks.
    if matches!(
        layer,
        "street_labels"
            | "street_labels_points"
            | "streets_polygons_labels"
            | "transportation_name"
            | "water_lines_labels"
            | "water_polygons_labels"
            | "boundary_labels"
            | "place_labels"
    ) {
        return None;
    }
    // Walls/fences dark thin, hedges green (detail archive).
    if layer == "barrier_line" {
        let (color, width) = match tags.get("barrier").map(|value| value.as_str()) {
            Some("hedge") => (0x9dc29a, 1.6),
            Some("fence") => (0xaaaaaa, 0.7),
            _ => (0x8a8a8a, 1.0),
        };
        return Some(StrokeStyle {
            sort_rank: 155,
            casing: None,
            center: StrokePassStyle { deck_m: 0.0,
                color,
                width: width * px_to_units,
                shape_id: 0.0,
                expand_class: EXPAND_CLASS_CONST_PX,
                depth_micro: 155.0 * DEPTH_MICRO_PER_RANK,
            },
        });
    }
    // Zoo / theme park perimeter: carto's muted purple boundary.
    if layer == "tourism_boundary" {
        return Some(StrokeStyle {
            sort_rank: 150,
            casing: None,
            center: StrokePassStyle { deck_m: 0.0,
                color: 0xa383a3,
                width: 1.6 * px_to_units,
                shape_id: 0.0,
                expand_class: EXPAND_CLASS_CONST_PX,
                depth_micro: 150.0 * DEPTH_MICRO_PER_RANK,
            },
        });
    }
    // Geodata overlay layers (layers.md). Transit route shapes above the
    // road network; nature and admin boundaries as outlines.
    match layer {
        "routes" => {
            // Transit-map look: each tram/metro line gets its own strong,
            // stable color (hash of the line ref) over a white casing so
            // routes read as a network, not faint threads under the roads.
            const LINE_PALETTE: [u32; 10] = [
                0xd7263d, 0x1b9e4b, 0x2456d7, 0xf2760c, 0x8e2bbf, 0x0b8f8f,
                0xc72b8e, 0x8a5a2b, 0x5a7d00, 0x364fc7,
            ];
            let mode = tags.get("mode").map(|v| v.as_str()).unwrap_or("");
            let line_ref = tags.get("ref").map(|v| v.as_str()).unwrap_or("");
            let (color, width) = match mode {
                "rail" => (0x37474f, 2.0),
                "ferry" => (0x1b78c4, 2.0),
                _ => {
                    // tram/metro: stable per-line color
                    let mut h: u32 = 5381;
                    for b in line_ref.bytes() {
                        h = h.wrapping_mul(33) ^ b as u32;
                    }
                    (LINE_PALETTE[(h % LINE_PALETTE.len() as u32) as usize], 2.6)
                }
            };
            return Some(StrokeStyle {
                sort_rank: 730,
                casing: Some(StrokePassStyle { deck_m: 0.0,
                    color: 0xffffff,
                    width: (width + 2.0) * px_to_units,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_CONST_PX,
                    depth_micro: 729.0 * DEPTH_MICRO_PER_RANK,
                }),
                center: StrokePassStyle { deck_m: 0.0,
                    color,
                    width: width * px_to_units,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_CONST_PX,
                    depth_micro: 730.0 * DEPTH_MICRO_PER_RANK,
                },
            });
        }
        "natura2000" | "wetlands" => {
            return Some(StrokeStyle {
                sort_rank: 240,
                casing: None,
                center: StrokePassStyle { deck_m: 0.0,
                    color: 0x2e8b57,
                    width: 1.3 * px_to_units,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_CONST_PX,
                    depth_micro: 240.0 * DEPTH_MICRO_PER_RANK,
                },
            });
        }
        "gemeenten" | "wijken" | "buurten" => {
            // Administrative boundary look: purple-gray, weight by tier,
            // finer tiers only appear as you zoom in.
            let (width, min_zoom) = match layer {
                "gemeenten" => (1.8, 6),
                "wijken" => (1.2, 11),
                _ => (0.9, 13),
            };
            if render_zoom < min_zoom {
                return None;
            }
            let width: f32 = width;
            return Some(StrokeStyle {
                sort_rank: 380,
                casing: None,
                center: StrokePassStyle { deck_m: 0.0,
                    color: 0x8a4e9e,
                    width: width * px_to_units,
                    shape_id: 11.0,
                    expand_class: EXPAND_CLASS_CONST_PX,
                    depth_micro: 380.0 * DEPTH_MICRO_PER_RANK,
                },
            });
        }
        _ => {}
    }
    // Platform slabs get a thin constant-px edge like carto.
    if layer == "platforms" {
        let edge = theme
            .bridge_area_fill
            .map(contrast_edge)
            .unwrap_or(0x9a938b);
        return Some(StrokeStyle {
            sort_rank: 140,
            casing: None,
            center: StrokePassStyle { deck_m: 0.0,
                color: edge,
                width: 0.8 * px_to_units,
                shape_id: 0.0,
                expand_class: EXPAND_CLASS_CONST_PX,
                depth_micro: 140.0 * DEPTH_MICRO_PER_RANK,
            },
        });
    }

    let mut width_scale = zoom_mult * px_to_units;
    let mut rank_bias = 0_i16;

    if tag_is_truthy(tags, "link") {
        width_scale *= 0.7;
        rank_bias -= 10;
    }
    if tag_is_truthy(tags, "tunnel") {
        rank_bias -= 22;
    }
    let mut deck_m = 0.0f32;
    if tag_is_truthy(tags, "bridge") {
        rank_bias += 26;
        // 3D deck clearance. Slightly exaggerated vs reality (like every
        // nav renderer) so crossings read at overview zooms; OSM's layer
        // attr is shadowed by the MVT layer-name tag, so stacked decks
        // share a height for now.
        deck_m = 9.0;
    }

    if let Some(highway) = tags.get("highway") {
        let key = highway.trim().to_ascii_lowercase();
        // shortbread carries rail/tram lines inside the streets layer; don't
        // let them fall through to the generic road style
        if tag_is_truthy(tags, "rail")
            || matches!(
                key.as_str(),
                "rail" | "tram" | "light_rail" | "subway" | "narrow_gauge" | "funicular"
                    | "monorail"
            )
        {
            let template = theme.railway_rule?;
            let rail_scale = zoom_mult.max(1.0).powf(0.35) * px_to_units;
            // shortbread sets rail=true on ALL railways including trams;
            // the kind decides tram/metro vs heavy rail.
            let heavy = matches!(
                key.as_str(),
                "rail" | "narrow_gauge" | "funicular" | "monorail"
            ) || (tag_is_truthy(tags, "rail")
                && !matches!(key.as_str(), "tram" | "light_rail" | "subway"));
            return Some(rail_stroke_style(
                template,
                heavy,
                render_zoom,
                tags,
                rank_bias,
                rail_scale,
            ));
        }
        let template = theme.road_rules.get(&key).copied().or(theme.road_default)?;
        // Carto hides footways/paths well before roads; clutter at city
        // scale otherwise (salmon dot confetti over every block).
        if (render_zoom as f32) < template.min_zoom {
            return None;
        }
        // Paths/footways/cycleways (thin, uncased) barely grow with zoom in
        // carto (~1px at z15 and z17 alike) while regular roads grow steeply.
        let mut thin_growth = false;
        if template.casing.is_none() && template.center.width <= 2.0 {
            width_scale = zoom_mult.max(1.0).powf(0.35) * px_to_units;
            if tag_is_truthy(tags, "link") {
                width_scale *= 0.7;
            }
            thin_growth = true;
        }
        let mut style = scaled_style(template, rank_bias, width_scale);
        if thin_growth {
            style.center.expand_class = EXPAND_CLASS_THIN;
            // carto grays out paths you can't freely walk (zoos, private
            // grounds) instead of the public salmon.
            if matches!(
                tags.get("access").map(|value| value.as_str()),
                Some("private" | "no" | "customers")
            ) {
                style.center.color = 0x9c9c9c;
            }
            // Footway bridges: dark rim (casing pass) + light deck (center
            // pass); the faint dots ride a companion style one rank above
            // (thin_bridge_dots_for_tags). Colors derive from the theme's
            // bridge fill so dark mode just works.
            if tag_is_truthy(tags, "bridge") {
                // Deck = the pedestrian-street surface color (white in the
                // light palette, dark slab in dark), not the gray bridge-
                // structure fill.
                let deck = theme
                    .road_rules
                    .get("pedestrian")
                    .map(|rule| rule.center.color)
                    .or(theme.bridge_area_fill)
                    .unwrap_or(0xf8f8f8);
                // The solid carrier is one physical bridge even when OSM
                // splits it between pedestrian members. Keep their symbolic
                // dotted widths in the companion pass below, but canonicalize
                // the slab to the theme's footway width so the union has no
                // shoulders or internal tier changes. A custom narrow road
                // rule is not part of that semantic family and keeps its own
                // configured carrier width.
                let physical_width = if is_pedestrian_bridge_member(&key) {
                    theme
                        .road_rules
                        .get("footway")
                        .map_or(1.0, |rule| rule.center.width)
                        * width_scale
                } else {
                    style.center.width
                };
                style.casing = Some(StrokePassStyle { deck_m: 0.0,
                    color: contrast_edge(deck),
                    width: physical_width * 3.0,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_THIN,
                    depth_micro: style.center.depth_micro - 2.0 * DEPTH_MICRO_PER_RANK,
                });
                style.center = StrokePassStyle { deck_m: 0.0,
                    color: deck,
                    width: physical_width * 2.2,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_THIN,
                    depth_micro: style.center.depth_micro - DEPTH_MICRO_PER_RANK,
                };
            }
        }
        // Bridges float above (and tunnels below) their base rank in the
        // tilt-mode micro-depth as well, so crossings resolve stably.
        if rank_bias != 0 {
            let micro_bias = rank_bias as f32 * DEPTH_MICRO_PER_RANK;
            style.center.depth_micro += micro_bias;
            if let Some(casing) = style.casing.as_mut() {
                casing.depth_micro += micro_bias;
            }
        }
        // carto draws bridge ROADS with a dark casing edge — never thin
        // footpaths/steps: their sub-px dark casing under the salmon dots
        // reads as black dashed fragments at every canal crossing.
        if tag_is_truthy(tags, "bridge") && !thin_growth {
            let width = style
                .casing
                .map_or(style.center.width * 1.35, |casing| casing.width);
            style.casing = Some(StrokePassStyle { deck_m: 0.0,
                color: 0x4a4a4a,
                width,
                shape_id: 0.0,
                expand_class: EXPAND_CLASS_ROAD,
                // One step under the center: a tie would let the dark edge
                // noise-win over the road fill (black bridges).
                depth_micro: style.center.depth_micro - DEPTH_MICRO_PER_RANK,
            });
        }
        if tag_is_truthy(tags, "tunnel") {
            style.center.shape_id = 11.0;
            if let Some(casing) = style.casing.as_mut() {
                casing.shape_id = 11.0;
            }
        }
        if deck_m > 0.0 {
            style.center.deck_m = deck_m;
            if let Some(casing) = style.casing.as_mut() {
                casing.deck_m = deck_m;
            }
        }
        return Some(style);
    }

    if let Some(waterway) = tags.get("waterway") {
        let key = waterway.trim().to_ascii_lowercase();
        let template = theme
            .waterway_rules
            .get(&key)
            .copied()
            .or(theme.waterway_default)?;
        if (render_zoom as f32) < template.min_zoom {
            return None;
        }
        // Waterway centerlines shrink faster than roads below z14 — carto
        // keeps canals at ~1px at z12/z13, letting the water polygons carry
        // the visual weight; linear scaling read as "the city is all water".
        let water_scale = if zoom_mult < 1.0 {
            zoom_mult.powf(1.6) * px_to_units
        } else {
            width_scale
        };
        return Some(scaled_style(template, rank_bias, water_scale));
    }

    if let Some(railway) = tags.get("railway") {
        let key = railway.trim().to_ascii_lowercase();
        // Non-track railway features (platforms, stations, dead lines) are
        // not linework.
        if matches!(
            key.as_str(),
            "platform" | "station" | "razed" | "abandoned" | "proposed" | "disused"
        ) {
            return None;
        }
        let template = theme.railway_rule?;
        let rail_scale = zoom_mult.max(1.0).powf(0.35) * px_to_units;
        let heavy = matches!(key.as_str(), "rail" | "narrow_gauge" | "funicular" | "monorail");
        return Some(rail_stroke_style(
            template,
            heavy,
            render_zoom,
            tags,
            rank_bias,
            rail_scale,
        ));
    }

    None
}

/// Zoom from which heavy rail draws the carto sleeper look (solid dark
/// casing + even light dash core); below it, and for tram/metro always,
/// rails stay the theme's single thin line. z14 and out, parallel station
/// tracks merge into a striped blob — stay thin there.
const RAIL_SLEEPER_MIN_ZOOM: u32 = 15;
/// Trams darken toward carto's near-black only at street level; at city
/// scale they stay the theme's faint gray so they don't dominate.
const TRAM_DARK_MIN_ZOOM: u32 = 16;

/// A readable edge color for a fill: dark fills get a lighter rim, light
/// fills a darker one — the single hook that keeps outline-style strokes
/// working in both palettes.
pub fn contrast_edge(color: u32) -> u32 {
    let r = color >> 16 & 0xff;
    let g = color >> 8 & 0xff;
    let b = color & 0xff;
    let luminance = (r * 3 + g * 6 + b) / 10;
    let scale = |v: u32, factor: u32| (v * factor / 100).min(0xff);
    if luminance >= 0x80 {
        (scale(r, 58) << 16) | (scale(g, 58) << 8) | scale(b, 58)
    } else {
        let lift = |v: u32| (v + (0xff - v) * 45 / 100).min(0xff);
        (lift(r) << 16) | (lift(g) << 8) | lift(b)
    }
}

/// Companion pass for thin footway bridges: the ORIGINAL dotted centerline
/// drawn one rank above the rim+deck, so the dots stay faintly visible on
/// the span (they do on osm.org).
pub fn thin_bridge_dots_for_tags(
    theme: &CompiledMapTheme,
    tags: &HashMap<String, String>,
    render_zoom: u32,
    zoom_mult: f32,
    px_to_units: f32,
) -> Option<StrokeStyle> {
    if !tag_is_truthy(tags, "bridge") {
        return None;
    }
    let highway = tags.get("highway")?;
    let key = highway.trim().to_ascii_lowercase();
    let template = theme.road_rules.get(&key).copied().or(theme.road_default)?;
    if (render_zoom as f32) < template.min_zoom {
        return None;
    }
    // Thin uncased paths only — road bridges keep their own casing look.
    if template.casing.is_some() || template.center.width > 2.0 {
        return None;
    }
    let width_scale = zoom_mult.max(1.0).powf(0.35) * px_to_units;
    let mut style = scaled_style(template, 26, width_scale);
    style.sort_rank = style.sort_rank.saturating_add(1);
    style.center.expand_class = EXPAND_CLASS_THIN;
    if matches!(
        tags.get("access").map(|value| value.as_str()),
        Some("private" | "no" | "customers")
    ) {
        style.center.color = 0x9c9c9c;
    }
    style.center.depth_micro += 26.0 * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK;
    Some(style)
}

fn rail_stroke_style(
    template: StrokeTemplate,
    heavy: bool,
    render_zoom: u32,
    tags: &HashMap<String, String>,
    rank_bias: i16,
    width_scale: f32,
) -> StrokeStyle {
    let mut template = template;
    if !heavy && render_zoom >= TRAM_DARK_MIN_ZOOM {
        // Trams/metro draw as a solid near-black line at street zooms in
        // carto. Only darken light-theme grays; the dark palette stays.
        let c = template.center.color;
        let avg = ((c >> 16 & 0xff) + (c >> 8 & 0xff) + (c & 0xff)) / 3;
        if avg < 0x90 {
            template.center.color = 0x505050;
        }
        template.center.width = template.center.width.max(1.2);
    }
    if heavy && render_zoom >= RAIL_SLEEPER_MIN_ZOOM && !tag_is_truthy(tags, "tunnel") {
        let rank = template.sort_rank as f32;
        // Sidings/yards/spurs (dead-end service tracks) fade toward the
        // background like carto's lighter service rail.
        let service = tags.contains_key("service");
        let casing_color = if service {
            let c = template.center.color;
            let lighten = |v: u32| v + (0xff - v) * 45 / 100;
            (lighten(c >> 16 & 0xff) << 16) | (lighten(c >> 8 & 0xff) << 8) | lighten(c & 0xff)
        } else {
            template.center.color
        };
        template.casing = Some(StrokePassStyle { deck_m: 0.0,
            // The theme's rail line color becomes the casing band.
            color: casing_color,
            width: if service { 2.0 } else { 2.4 },
            shape_id: 0.0,
            expand_class: EXPAND_CLASS_THIN,
            depth_micro: rank * DEPTH_MICRO_PER_RANK,
        });
        template.center = StrokePassStyle { deck_m: 0.0,
            color: 0xf7f7f7,
            width: if service { 1.0 } else { 1.2 },
            shape_id: 12.0,
            expand_class: EXPAND_CLASS_THIN,
            depth_micro: rank * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK,
        };
    }
    let mut style = scaled_style(template, rank_bias, width_scale);
    if tag_is_truthy(tags, "bridge") {
        style.center.deck_m = 9.0;
        if let Some(casing) = style.casing.as_mut() {
            casing.deck_m = 9.0;
        }
    }
    style
}

/// The live light-theme rules mirrored without the script VM so headless
/// tile tests can use the production road classes. Keep in sync with the
/// `style:` block in view.rs when those classes change materially.
pub fn probe_compiled_theme() -> CompiledMapTheme {
    fn road(
        kind: &str,
        sort_rank: u32,
        casing: Option<(u32, f32)>,
        center: (u32, f32),
        center_shape_id: f32,
        min_zoom: f32,
    ) -> MapRoadRule {
        MapRoadRule {
            source: Default::default(),
            kind: kind.to_string(),
            sort_rank,
            casing_color: Vec4f::from_u32(casing.map_or(0, |(color, _)| (color << 8) | 0xff)),
            casing_width: casing.map_or(0.0, |(_, width)| width),
            casing_shape_id: 0.0,
            center_color: Vec4f::from_u32((center.0 << 8) | 0xff),
            center_width: center.1,
            center_shape_id,
            min_zoom,
        }
    }
    fn fill(group: &str, value: &str, color: u32) -> MapFillRule {
        MapFillRule {
            source: Default::default(),
            group: group.to_string(),
            value: value.to_string(),
            color: Vec4f::from_u32((color << 8) | 0xff),
        }
    }
    fn waterway(kind: &str, width: f32, min_zoom: f32) -> MapWaterwayRule {
        MapWaterwayRule {
            source: Default::default(),
            kind: kind.to_string(),
            sort_rank: 140,
            casing_color: Vec4f::from_u32(0),
            casing_width: 0.0,
            casing_shape_id: 0.0,
            center_color: Vec4f::from_u32(0xaad3dfff),
            center_width: width,
            center_shape_id: 0.0,
            min_zoom,
        }
    }
    let mut style = MapThemeStyle::default();
    style.fill_rules = vec![
        fill("building", "", 0xd9d0c9),
        fill("building_outline", "", 0xb5aa9b),
        fill("street_area", "", 0xdddde8),
        fill("bridge_area", "", 0xb8b8b8),
        fill("water", "", 0xaad3df),
        fill("landuse", "residential", 0xe0dfdf),
        fill("landuse", "forest", 0xadd19e),
        fill("landuse", "grass", 0xcdebb0),
        fill("landuse", "*", 0xe8e7e2),
        fill("leisure", "park", 0xc8facc),
        fill("leisure", "*", 0xc8facc),
    ];
    style.road_rules = vec![
        road("motorway", 700, Some((0xdc2a67, 7.2)), (0xe892a2, 6.0), 0.0, 0.0),
        road("trunk", 640, Some((0xc84e2f, 7.2)), (0xf9b29c, 6.0), 0.0, 0.0),
        road("primary", 560, Some((0xa06b00, 6.4)), (0xfcd6a4, 5.0), 0.0, 0.0),
        road("secondary", 470, Some((0x707d05, 6.4)), (0xf7fabf, 5.0), 0.0, 0.0),
        road("busway", 470, Some((0x707d05, 6.4)), (0xf7fabf, 5.0), 0.0, 0.0),
        road("tertiary", 390, Some((0x8f8f8f, 6.2)), (0xffffff, 5.0), 0.0, 0.0),
        road("residential", 310, Some((0xbbbbbb, 4.2)), (0xffffff, 3.0), 0.0, 0.0),
        road("unclassified", 310, Some((0xbbbbbb, 4.2)), (0xffffff, 3.0), 0.0, 0.0),
        road("living_street", 310, Some((0xbbbbbb, 4.0)), (0xededed, 3.0), 0.0, 0.0),
        road("service", 240, Some((0xbbbbbb, 3.0)), (0xffffff, 2.0), 0.0, 0.0),
        road("pedestrian", 240, Some((0x999999, 4.0)), (0xdddde8, 3.0), 0.0, 0.0),
        road("pedestrian", 300, Some((0xb5b5b5, 4.0)), (0xfdfdfd, 2.8), 0.0, 14.0),
        road("cycleway", 160, None, (0x6262ff, 0.9), 10.0, 14.0),
        road("footway", 160, None, (0xaaa8a5, 0.9), 10.0, 15.0),
        road("path", 160, None, (0xaaa8a5, 0.8), 10.0, 15.0),
        road("steps", 160, None, (0xaaa8a5, 2.0), 10.0, 15.0),
        road("track", 160, None, (0xaaa8a5, 1.0), 10.0, 14.0),
        road("*", 280, Some((0xbbbbbb, 3.6)), (0xffffff, 2.5), 0.0, 0.0),
    ];
    style.waterway_rules = vec![
        waterway("river", 4.0, 0.0),
        waterway("canal", 3.0, 12.0),
        waterway("stream", 1.4, 13.0),
        waterway("*", 1.2, 13.0),
    ];
    style.railway_rule = Some(MapRailRule {
        source: Default::default(),
        sort_rank: 710,
        casing_color: Vec4f::from_u32(0),
        casing_width: 0.0,
        casing_shape_id: 0.0,
        center_color: Vec4f::from_u32(0x6e6e6eff),
        center_width: 1.0,
        center_shape_id: 0.0,
    });
    style.compile()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motorway_tags(layer: &str) -> HashMap<String, String> {
        HashMap::from([
            ("layer".to_string(), layer.to_string()),
            ("highway".to_string(), "motorway".to_string()),
        ])
    }

    #[test]
    fn street_label_geometry_never_produces_a_road_stroke() {
        let theme = probe_compiled_theme();
        let tags = motorway_tags("street_labels");

        for tile_zoom in [0, 13, 14, 22] {
            assert!(
                stroke_style_for_tags(&theme, &tags, tile_zoom, 18, zoom_width_mult(18), 1.0)
                    .is_none(),
                "street-label geometry emitted a physical stroke at z{tile_zoom}"
            );
        }
    }

    #[test]
    fn physical_street_geometry_still_produces_a_low_zoom_road_stroke() {
        let theme = probe_compiled_theme();
        let tags = motorway_tags("streets");

        assert!(
            stroke_style_for_tags(&theme, &tags, 13, 18, zoom_width_mult(18), 1.0).is_some()
        );
    }

    #[test]
    fn thin_bridge_members_share_one_physical_carrier_width() {
        let theme = probe_compiled_theme();
        let tags = |highway: &str| {
            HashMap::from([
                ("layer".to_string(), "streets".to_string()),
                ("highway".to_string(), highway.to_string()),
                ("bridge".to_string(), "1".to_string()),
            ])
        };
        let zoom_mult = zoom_width_mult(18);
        let members = ["footway", "path", "steps", "cycleway"];
        let styles = members.map(|highway| {
            stroke_style_for_tags(&theme, &tags(highway), 14, 18, zoom_mult, 1.0).unwrap()
        });

        for style in &styles[1..] {
            assert_eq!(styles[0].sort_rank, style.sort_rank);
            assert_eq!(
                styles[0].center.width.to_bits(),
                style.center.width.to_bits()
            );
            assert_eq!(
                styles[0].casing.unwrap().width.to_bits(),
                style.casing.unwrap().width.to_bits()
            );
        }

        let footway = tags("footway");
        let steps = tags("steps");
        let footway_dots =
            thin_bridge_dots_for_tags(&theme, &footway, 18, zoom_mult, 1.0).unwrap();
        let steps_dots =
            thin_bridge_dots_for_tags(&theme, &steps, 18, zoom_mult, 1.0).unwrap();
        assert_ne!(
            footway_dots.center.width.to_bits(),
            steps_dots.center.width.to_bits()
        );
        assert_eq!(footway_dots.center.shape_id, 10.0);
        assert_eq!(steps_dots.center.shape_id, 10.0);
    }

    #[test]
    fn unrelated_narrow_bridge_rule_keeps_its_configured_carrier_width() {
        let mut theme = probe_compiled_theme();
        let configured_width = 1.35;
        let service = theme.road_rules.get_mut("service").unwrap();
        service.casing = None;
        service.center.width = configured_width;

        let tags = HashMap::from([
            ("layer".to_string(), "streets".to_string()),
            ("highway".to_string(), "service".to_string()),
            ("bridge".to_string(), "1".to_string()),
        ]);
        let zoom_mult = zoom_width_mult(18);
        let thin_scale = zoom_mult.max(1.0).powf(0.35);
        let style = stroke_style_for_tags(&theme, &tags, 14, 18, zoom_mult, 1.0).unwrap();

        assert_eq!(
            style.center.width.to_bits(),
            (configured_width * thin_scale * 2.2).to_bits()
        );
        assert_eq!(
            style.casing.unwrap().width.to_bits(),
            (configured_width * thin_scale * 3.0).to_bits()
        );
    }
}
