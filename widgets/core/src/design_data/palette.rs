use super::json_str;
use crate::{makepad_draw::*, widget_tree::live_id_token, WidgetRegistry};
use std::collections::HashSet;

const CURATED: &[(&str, &str, &str)] = &[
    ("View", "Layout", "{width: Fill height: Fit flow: Down spacing: 4 padding: 4}"),
    ("RoundedView", "Layout", "{width: Fill height: 40 draw_bg +: {color: #x3a3a44}}"),
    ("ScrollYView", "Layout", "{width: Fill height: Fill flow: Down}"),
    ("Label", "Text", "{text: \"Label\"}"),
    ("Button", "Controls", "{text: \"Button\"}"),
    ("TextInput", "Controls", "{width: 160 empty_text: \"Type here\"}"),
    ("CheckBox", "Controls", "{text: \"Check\"}"),
    ("Slider", "Controls", "{width: 160}"),
    ("Image", "Media", "{width: 64 height: 64}"),
    ("Hr", "Layout", "{}"),
    ("Vr", "Layout", "{}"),
    ("Filler", "Layout", "{}"),
];

fn palette_group(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["view", "splitter", "dock", "grid", "masonry", "panel", "layout", "spacer", "filler", "hr", "vr", "modal", "dialog", "popover", "drawer", "card", "accordion", "fold"]) {
        "Layout"
    } else if has(&["label", "text", "markdown", "html", "code", "rich", "math", "badge", "tooltip", "tip"]) {
        "Text"
    } else if has(&["button", "check", "radio", "slider", "input", "dropdown", "drop_down", "toggle", "knob", "picker", "field", "combo", "switch", "stepper", "range", "fader", "color"]) {
        "Controls"
    } else if has(&["list", "table", "tree", "portal", "kanban", "data", "tile", "carousel", "wheel"]) {
        "Lists & tables"
    } else if has(&["nav", "menu", "tab", "bar", "breadcrumb", "pill", "stack", "page", "toolbar"]) {
        "Navigation"
    } else if has(&["image", "video", "icon", "svg", "chart", "spark", "gauge", "meter", "wave", "progress", "spinner", "canvas", "vector"]) {
        "Media"
    } else {
        "Other"
    }
}

#[derive(Default)]
struct Cache {
    generation: u64,
    heap: usize,
    json: String,
}

pub(super) fn remote(cx: &mut Cx) -> String {
    let generation = cx.reload_generation;
    let heap = cx.with_vm(|vm| vm.heap().heap_key());
    if let Some(cache) = cx.get_global_ref::<Cache>() {
        if cache.generation == generation && cache.heap == heap {
            return cache.json.clone();
        }
    }

    let registered: HashSet<_> = cx.components.get::<WidgetRegistry>().map.keys().copied().collect();
    let mut names = cx.with_vm(|vm| {
        let widgets = vm.module(id!(widgets));
        vm.heap().map_entries(widgets).into_iter().filter_map(|entry| {
            let name = live_id_token(entry.key.as_id()?);
            if name.ends_with("Base") || CURATED.iter().any(|&(n, _, _)| n == name) { return None; }
            let mut object = entry.value.as_object();
            let mut visited = HashSet::new();
            while let Some(o) = object {
                if !visited.insert(o) { break; }
                if vm.heap().object_type_id(o).is_some_and(|id| registered.contains(&id)) {
                    return Some(name);
                }
                object = vm.heap().proto(o).as_object();
            }
            None
        }).collect::<Vec<_>>()
    });
    names.sort();
    let rows = CURATED.iter().copied().chain(names.iter().map(|name| (name.as_str(), palette_group(name), "{}")))
        .map(|(name, group, body)| format!(r#"{{"name":{},"group":{},"body":{}}}"#, json_str(name), json_str(group), json_str(body)))
        .collect::<Vec<_>>();
    let json = format!(r#"{{"palette":[{}]}}"#, rows.join(","));
    *cx.global::<Cache>() = Cache { generation, heap, json: json.clone() };
    json
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_micro_serde::{DeJson, JsonValue};
    use crate::makepad_platform::makepad_script::syntax;

    #[test]
    fn palette_lists_curated_and_typed_widgets_deterministically() {
        let mut cx = crate::checkout_test_cx();
        cx.with_vm(|vm| crate::script_eval!(vm, {mod.widgets.PaletteTestWidget = mod.widgets.View{}}));
        let json = remote(&mut cx);
        assert_eq!(remote(&mut cx), json, "repeated palette requests have identical ordering and bodies");
        let data = JsonValue::deserialize_json_strict(&json).unwrap();
        let entries = data.get("palette").unwrap().as_array().unwrap();
        let mut groups = HashSet::new();
        let mut registered = Vec::new();
        for (i, row) in entries.iter().enumerate() {
            let name = row.get("name").unwrap().as_str().unwrap();
            let body = row.get("body").unwrap().as_str().unwrap();
            if i < CURATED.len() {
                groups.insert(row.get("group").unwrap().as_str().unwrap());
            } else {
                registered.push(name);
            }
            assert!(syntax::parse(&format!("{name}{body}")).is_ok(), "{name} must supply a valid constructor");
        }
        assert_eq!(groups, HashSet::from(["Layout", "Text", "Controls", "Media"]), "curated categories are all populated");
        assert!(registered.contains(&"PaletteTestWidget"), "typed module templates join the palette");
        assert!(registered.windows(2).all(|pair| pair[0] < pair[1]), "registered entries are sorted and unique");
    }
}
