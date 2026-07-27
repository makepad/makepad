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
    mod.widgets.MapThemeStyle = #(MapThemeStyle::script_component(vm))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokePassStyle {
    pub color: u32,
    pub width: f32,
    pub shape_id: f32,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledMapTheme {
    pub background: Vec4f,
    pub status_text: Vec4f,
    pub label: Vec4f,
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
}

impl Default for CompiledMapTheme {
    fn default() -> Self {
        Self {
            background: Vec4f::from_u32(0xddd7ccff),
            status_text: Vec4f::from_u32(0xdee9f4ff),
            label: Vec4f::from_u32(0x000000ff),
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
            Some(StrokePassStyle {
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
            })
        } else {
            None
        },
        center: StrokePassStyle {
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
        },
    }
}

fn stroke_template_from_waterway_rule(rule: &MapWaterwayRule) -> StrokeTemplate {
    StrokeTemplate {
        sort_rank: clamp_u32_to_i16(rule.sort_rank),
        casing: if rule.casing_width > 0.0 {
            Some(StrokePassStyle {
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
            })
        } else {
            None
        },
        center: StrokePassStyle {
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
        },
    }
}

fn stroke_template_from_rail_rule(rule: &MapRailRule) -> StrokeTemplate {
    StrokeTemplate {
        sort_rank: clamp_u32_to_i16(rule.sort_rank),
        casing: if rule.casing_width > 0.0 {
            Some(StrokePassStyle {
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
            })
        } else {
            None
        },
        center: StrokePassStyle {
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
        },
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

pub fn fill_color_for_tags(
    theme: &CompiledMapTheme,
    tags: &HashMap<String, String>,
    closed: bool,
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
    if tag_is(tags, "natural", "water") || tag_is(tags, "waterway", "riverbank") {
        return theme.water_fill;
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

/// Semantic paint order for the fill pass (carto-like): land/landcover as the
/// base, sites above land, water above both, then buildings, then road areas.
/// Raw MVT layer order painted `land`/`sites` over the buildings.
pub fn fill_layer_rank(tags: &HashMap<String, String>) -> u8 {
    if tags.contains_key("building") {
        return 40;
    }
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) {
        return 50;
    }
    match layer {
        "ocean" => 5,
        "land" | "landuse" | "landcover" => 10,
        "sites" | "park" | "pois" => 15,
        "water" | "water_polygons" | "water_polygons_labels" => 20,
        "dam_polygons" | "pier_polygons" => 25,
        "bridges" => 45,
        _ => {
            if tag_is(tags, "natural", "water") || tag_is(tags, "waterway", "riverbank") {
                20
            } else if tags.contains_key("leisure") {
                15
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
        _ => 4.2,
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

pub fn stroke_style_for_tags(
    theme: &CompiledMapTheme,
    tags: &HashMap<String, String>,
    tile_zoom: u32,
    zoom_mult: f32,
    px_to_units: f32,
) -> Option<StrokeStyle> {
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) || layer == "bridges" {
        return None;
    }

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
        if !(layer == "street_labels" && tile_zoom < 14) {
            return None;
        }
    }

    if matches!(layer, "street_labels_points" | "streets_polygons_labels") {
        return None;
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
    if tag_is_truthy(tags, "bridge") {
        rank_bias += 26;
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
            return Some(scaled_style(template, rank_bias, rail_scale));
        }
        let template = theme.road_rules.get(&key).copied().or(theme.road_default)?;
        // Paths/footways/cycleways (thin, uncased) barely grow with zoom in
        // carto (~1px at z15 and z17 alike) while regular roads grow steeply.
        if template.casing.is_none() && template.center.width <= 2.0 {
            width_scale = zoom_mult.max(1.0).powf(0.35) * px_to_units;
            if tag_is_truthy(tags, "link") {
                width_scale *= 0.7;
            }
        }
        let mut style = scaled_style(template, rank_bias, width_scale);
        if tag_is_truthy(tags, "tunnel") {
            style.center.shape_id = 11.0;
            if let Some(casing) = style.casing.as_mut() {
                casing.shape_id = 11.0;
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
        return Some(scaled_style(template, rank_bias, width_scale));
    }

    if tags.contains_key("railway") {
        let template = theme.railway_rule?;
        return Some(scaled_style(template, rank_bias, width_scale));
    }

    None
}
