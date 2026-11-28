//! On-demand data operations on the running widget tree.
pub mod locate;
pub mod preview;
pub mod palette;
pub mod drop;
pub mod pick;
pub mod tree;
pub mod props;
pub mod apply;
pub mod typed;
pub mod states;
pub mod theme;

use crate::{makepad_draw::*, WidgetHooks, WidgetRef};
use crate::makepad_platform::remote::json_str;

type Args = [(String, String)];

fn arg<'a>(args: &'a Args, key: &str) -> Option<&'a str> {
    args.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn window_arg(args: &Args) -> Result<Option<usize>, String> {
    arg(args, "w").or_else(|| arg(args, "window"))
        .map(str::parse).transpose().map_err(|_| "invalid window id".into())
}

fn need<'a>(args: &'a Args, key: &str) -> Result<&'a str, String> {
    arg(args, key).ok_or_else(|| format!("need {key}="))
}

fn with_widget_vm<T>(cx: &mut Cx, widget: &WidgetRef, f: impl FnOnce(&mut Cx) -> Result<T, String>) -> Result<T, String> {
    let heap = widget.script_source_heap_key();
    if heap == 0 || cx.script_vm.as_ref().is_some_and(|vm| vm.heap_ref().heap_key() == heap) {
        return f(cx);
    }
    let vm = crate::widget_async::vm_for_heap(cx, heap).ok_or("widget's script VM is unavailable")?;
    crate::widget_async::with_isolate(cx, vm, f)
}

pub(crate) fn callback(cx: &mut Cx, op: &str, args: &Args) -> Result<String, String> {
    match op {
        "design_preview" | "design_revert" => preview::remote(cx, op, args),
        "design_preview_chunk" => preview::chunk(cx, args),
        "design_verify" if arg(args, "file").is_some() || arg(args, "path").is_some() || WidgetHooks::tweaker(cx).is_none() => {
            preview::verify_in_window(cx, arg(args, "file"), arg(args, "path"), window_arg(args)?)
        }
        "design_palette" => Ok(palette::remote(cx)),
        "design_drop" => drop::remote(cx, args),
        "design_tree" => tree::remote(cx, args),
        "design_pick" => pick::remote(cx, args),
        "design_rect" => pick::rect_json(cx, args),
        "design_theme" => theme::remote(cx, args),
        "design_props" | "design_apply" | "design_typed" | "design_locate" | "design_states" | "design_pose" => {
            let widget = pick::resolve_target(cx, need(args, "path")?, window_arg(args)?)?;
            let uid = widget.try_widget_uid().map(|u| u.0).filter(|&u| u != 0);
            let read_or_edit = |cx: &mut Cx| match op {
                "design_props" => Ok(props::remote(cx, &widget)),
                "design_apply" => apply::remote(cx, &widget, args),
                "design_typed" => typed::remote(cx, &widget, args),
                "design_states" => states::remote(cx, &widget, arg(args, "layer")),
                "design_pose" => states::pose(cx, &widget, need(args, "layer")?, need(args, "state")?),
                _ => locate::remote(cx, &widget),
            };
            let reply = with_widget_vm(cx, &widget, read_or_edit)?;
            if matches!(op, "design_apply" | "design_typed") {
                if let (Some(uid), Some(head)) = (uid, reply.strip_suffix('}')) {
                    if !head.trim_end().ends_with('{') {
                        return Ok(format!(r#"{head},"uid":{uid}}}"#));
                    }
                }
            }
            Ok(reply)
        }
        _ => {
            let hooks = WidgetHooks::tweaker(cx)
                .ok_or_else(|| "no tweaker (this app has no widgets ui root)".to_string())?;
            (hooks.tweak_callback)(cx, op, args)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{view::View, WidgetRef};
    use crate::script_eval;

    #[test]
    fn design_theme_get_set_and_install_data_only_without_eval() {
        use crate::makepad_micro_serde::{DeJson, JsonValue};

        fn call(cx: &mut Cx, args: &[(&str, &str)]) -> Result<JsonValue, String> {
            let args: Vec<_> = args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            let reply = super::callback(cx, "design_theme", &args)?;
            Ok(JsonValue::deserialize_json_strict(&reply).expect("theme reply must be JSON"))
        }
        fn set(cx: &mut Cx, name: &str, value: &str) -> Result<JsonValue, String> {
            call(cx, &[("set", "1"), ("name", name), ("value", value)])
        }
        fn token<'a>(data: &'a JsonValue, name: &str) -> &'a str {
            data.get("values").unwrap().as_array().unwrap().iter()
                .find(|row| row.get("name").and_then(JsonValue::as_str) == Some(name))
                .and_then(|row| row.get("value")?.as_str())
                .unwrap_or_else(|| panic!("missing theme token {name}"))
        }
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (dark_page, light_page) = cx.with_vm(|vm| {
            crate::desktop_style::uninstall(vm);
            crate::script_mod(vm);
            (script_eval!(vm, {mod.themes.dark.color_bg_app}).as_color().unwrap(),
             script_eval!(vm, {mod.themes.light.color_bg_app}).as_color().unwrap())
        });
        let get = |cx: &mut Cx| call(cx, &[("get", "1")]).unwrap();
        let initial = get(&mut cx);
        assert_eq!(initial.get("define_file"), Some(&JsonValue::Null), "a library-only theme without an app root has no write-back file");
        assert_eq!(initial.get("base").unwrap().as_str(), Some("dark"), "initial base");
        assert_eq!(initial.get("sheet"), Some(&JsonValue::Null), "initial sheet");
        assert_eq!(token(&initial, "color_bg_app"), theme::hex_of(dark_page), "initial page");
        assert_eq!(token(&initial, "space_factor"), "6", "initial spacing");
        let sheets = initial.get("sheets").unwrap().as_array().unwrap();
        for (id, label, base) in [("omarchy", "Omarchy", "dark"), ("macos", "macOS", "light")] {
            let sheet = sheets.iter().find(|s| s.get("id").unwrap().as_str() == Some(id)).expect("core sheet");
            for (key, expected) in [("label", label), ("base", base), ("appearance", base)] {
                assert_eq!(sheet.get(key).unwrap().as_str(), Some(expected), "{id} {key}");
            }
        }
        let filtered = call(&mut cx, &[("get", "1"), ("name", "space_factor")]).unwrap();
        assert_eq!(filtered.get("values").unwrap().as_array().unwrap().len(), 1, "get name filter");
        assert!(call(&mut cx, &[("get", "1"), ("name", "absent")]).is_err(), "unknown get token");
        for (input, old, changed) in [("#223344ff", token(&initial, "color_bg_app"), true), ("#223344", "#223344ff", false)] {
            let edit = set(&mut cx, "color_bg_app", input).unwrap();
            assert_eq!(edit.get("ok").unwrap().as_bool(), Some(true), "set succeeded");
            assert_eq!(edit.get("name").unwrap().as_str(), Some("color_bg_app"), "set token");
            assert_eq!(edit.get("old").unwrap().as_str(), Some(old), "previous colour");
            assert_eq!(edit.get("new").unwrap().as_str(), Some("#223344ff"), "normalized colour");
            assert_eq!(edit.get("changed").unwrap().as_bool(), Some(changed), "semantic change for {input}");
            assert_eq!(edit.get("global").unwrap().as_bool(), Some(false), "page edit is a pin");
        }
        let edited = get(&mut cx);
        assert_eq!(token(&edited, "color_bg_app"), "#223344ff", "edit reached the VM");
        assert_eq!(edited.get("edits").unwrap(), &JsonValue::deserialize_json_strict(r##"[["color_bg_app","#x223344FF"]]"##).unwrap(), "standing edits");
        for (key, value) in [("space_factor", "NaN"), ("space_factor", "inf"), ("space_factor", "-inf"),
                             ("space_factor", "fn(){}"), ("color_bg_app", "mix(#f00,#00f,0.5)")] {
            assert!(set(&mut cx, key, value).is_err(), "reject {key} = {value}");
        }
        assert_eq!(get(&mut cx), edited, "invalid sets must preserve the VM and edits");
        let alias = set(&mut cx, "color_bg_app", "theme.color_primary").unwrap();
        assert_eq!(alias.get("old").unwrap().as_str(), Some("#223344ff"), "alias previous colour");
        assert_eq!(alias.get("new").unwrap().as_str(), Some(token(&initial, "color_primary")), "resolved alias");

        let sheet = Some("macos-dark");
        for (spec, base, worn, mixed, pinned, page) in [
            (r#"{"base":"light"}"#, "light", None, false, false, light_page),
            (r#"{"base":"dark","sheet":"macos-dark"}"#, "dark", sheet, false, false, 0x28282aff),
            (r##"{"base":"dark","sheet":null,"globals":[["space_factor",14.0]],"overrides":[["color_bg_app","#abc"]]}"##,
                "dark", None, true, false, 0xaabbccff),
            (r##"{"base":"dark","sheet":"macos-dark","overrides":[["color_bg_app","#123456"]]}"##,
                "dark", sheet, false, true, 0x123456ff),
        ] {
            let reply = call(&mut cx, &[("install", "1"), ("spec", spec)]).unwrap();
            assert_eq!(reply.get("ok").unwrap().as_bool(), Some(true), "installed {spec}");
            assert_eq!(reply.get("base").unwrap().as_str(), Some(base), "installed base {spec}");
            assert_eq!(reply.get("sheet").unwrap().as_str(), worn, "installed sheet {spec}");
            cx.with_vm(|vm| {
                vm.capture_errors();
                vm.with_reload(crate::script_mod);
                let errors = vm.take_errors();
                assert!(errors.is_empty(), "reload {spec}: {errors:?}");
                let prelude = script_eval!(vm, {mod.prelude.widgets_internal.theme}).as_object().unwrap();
                for (source, object) in [("theme", vm.module(id!(theme))), ("widget prelude", prelude)] {
                    assert_eq!(vm.value(object, id!(color_bg_app)).as_color(), Some(page), "{source} page after {spec}");
                    if worn.is_some() {
                        assert_eq!(vm.value(object, id!(corner_radius)).as_f64(), Some(6.0), "{source} sheet radius");
                    }
                    if mixed {
                        let inset = vm.value(object, id!(mspace_2)).as_object().expect("re-derived inset");
                        assert_eq!(vm.value(inset, id!(top)).as_f64(), Some(14.0), "{source} re-derived spacing");
                    }
                }
                if pinned {
                    assert_eq!(script_eval!(vm, {mod.prelude.stock_internal.theme.color_bg_app}).as_color(), Some(dark_page), "stock page under sheet and pin");
                }
            });
            let state = get(&mut cx);
            assert_eq!(state.get("has_mix").unwrap().as_bool(), Some(mixed), "mix state for {spec}");
            assert_eq!(state.get("has_pins").unwrap().as_bool(), Some(pinned), "pins state for {spec}");
            assert!(state.get("edits").unwrap().as_array().unwrap().is_empty(), "install clears edits");
        }
        let installed = get(&mut cx);
        assert!(call(&mut cx, &[("install", "1")]).is_err(), "install needs a spec");
        for spec in [
            r#"{"base":"missing"}"#, r#"{"base":"dark","sheet":"missing"}"#,
            r#"{"base":"dark","sheet":"macos-dark","globals":[["space_factor",12]]}"#,
            r#"{"base":"dark","globals":[["unlisted",0]]}"#, r#"{"base":"dark","globals":[["color_tint",2]]}"#,
            r##"{"base":"dark","globals":[["space_factor","#fff"]]}"##,
            r#"{"base":"dark","overrides":[["a; mod.themes.dark={}",1]]}"#,
            r###"{"base":"dark","overrides":[["color_primary","##fff"]]}"###,
            r#"{"base":"dark","overrides":[["space_factor",1e309]]}"#,
            r#"{"base":"dark","globals":[["space_factor",-1e309]]}"#,
        ] {
            assert!(call(&mut cx, &[("install", "1"), ("spec", spec)]).is_err(), "reject {spec}");
        }
        let mut direct = theme::parse_install_spec(r#"{"base":"light"}"#).unwrap();
        direct.sheet = Some("absent".into());
        assert!(theme::apply_install_spec(&mut cx, &direct).is_err(), "direct install validates the sheet before changing base");
        direct.sheet = Some("macos".into());
        direct.globals.push(("space_factor".into(), theme::ThemeVal::Num(12.0)));
        assert!(theme::apply_install_spec(&mut cx, &direct).is_err(), "direct install rejects globals with a sheet");
        assert_eq!(get(&mut cx), installed, "invalid installs must preserve all installed state");
    }

    #[test]
    fn design_props_reports_shader_layers_io_kinds_and_fn_sources() {
        use crate::makepad_micro_serde::{DeJson, JsonValue};

        fn entry<'a>(object: &'a JsonValue, array: &str, key: &str, name: &str) -> Option<&'a JsonValue> {
            object.get(array)?.as_array()?.iter()
                .find(|entry| entry.get(key).and_then(JsonValue::as_str) == Some(name))
        }

        let mut cx = crate::test_cx::checkout_test_cx(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                Root{main := Window{body +: {
                    drawn := RectView{width: 32 height: 32 show_bg: true
                        draw_bg +: {color: #369 border_size: 1.0
                            pixel: fn() { return self.color * self.border_size }
                        }
                    }
                    undrawn := RectView{width: 32 height: 32 show_bg: false
                        draw_bg +: {color: #369 border_size: 1.0}
                    }
                }}}
            });
            WidgetRef::script_from_value(vm, value)
        });
        crate::set_ui_root(&mut cx, &root);
        for window in cx.windows.id_iter().collect::<Vec<_>>() {
            let geom = &mut cx.windows[window].window_geom;
            geom.inner_size = dvec2(160., 120.);
            geom.dpi_factor = 1.;
        }
        cx.redraw_all();
        let draw = Event::Draw(std::mem::take(&mut cx.new_draw_event));
        root.handle_event(&mut cx, &draw, &mut Scope::empty());
        let callback = cx.tweak_callback.unwrap();
        for (path, drawn) in [("drawn", true), ("undrawn", false)] {
            let reply = callback(&mut cx, "design_props", &[("path".into(), path.into())])
                .unwrap_or_else(|why| panic!("design props on {path}: {why}"));
            let data = JsonValue::deserialize_json_strict(&reply)
                .unwrap_or_else(|why| panic!("{path} must return valid JSON: {why:?}"));
            let bg = entry(&data, "shaders", "layer", "draw_bg")
                .unwrap_or_else(|| panic!("{path} must report its draw_bg shader layer"));
            assert_eq!(bg.get("drawn").and_then(JsonValue::as_bool), Some(drawn), "{path} draw_bg draw status");
            for (name, io) in [("color", "instance"), ("border_size", "uniform")] {
                let input = entry(bg, "inputs", "name", name)
                    .unwrap_or_else(|| panic!("{path} draw_bg must report its {name} input"));
                assert_eq!(input.get("io").and_then(JsonValue::as_str), Some(io), "{path} draw_bg.{name} IO kind");
            }
            let pixel = entry(bg, "fns", "name", "pixel")
                .unwrap_or_else(|| panic!("{path} draw_bg must report its pixel function"));
            assert!(pixel.get("loc").and_then(JsonValue::as_str).is_some_and(|loc| !loc.is_empty()),
                "{path} pixel function must have a source location");
            assert!(pixel.get("src").and_then(JsonValue::as_str).is_some_and(|src| src.contains("fn")),
                "{path} pixel function must include its source");
        }
    }

    #[test]
    fn design_tree_scopes_windows_pages_large_children_reveals_and_queries() {
        use crate::makepad_micro_serde::{DeJson, JsonValue};
        use crate::widget_hooks::TweakerHooks;
        use std::collections::BTreeSet;

        struct Restore<'a>(&'a mut Cx);
        impl Drop for Restore<'_> {
            fn drop(&mut self) { WidgetHooks::of(self.0).tweaker = None; }
        }
        fn read(cx: &mut Cx, args: &[(&str, &str)]) -> JsonValue {
            let args: Vec<_> = [("w", "1")].into_iter().chain(args.iter().copied())
                .map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
            let raw = callback(cx, "design_tree", &args).unwrap();
            JsonValue::deserialize_json_strict(&raw).expect("tree must be valid JSON")
        }
        fn array<'a>(v: &'a JsonValue, key: &str) -> &'a [JsonValue] {
            v.get(key).unwrap().as_array().unwrap()
        }
        fn path(node: &JsonValue) -> &str { node.get("path").unwrap().as_str().unwrap() }
        fn nodes(v: &JsonValue) -> impl Iterator<Item = &JsonValue> {
            array(v, "parents").iter().flat_map(|p| array(p, "nodes"))
        }

        let mut pooled = crate::test_cx::checkout_test_cx(crate::script_mod);
        let guard = Restore(&mut pooled);
        let cx = &mut *guard.0;
        WidgetHooks::of(cx).tweaker = Some(TweakerHooks {
            inspector_type: crate::grid::Grid::script_type_id_static(),
            window_intercept: |_, _, _, _| false, panel_owns_pointer: |_| false,
            tweak_callback: |_, _, _| unreachable!(), feedback_snapshot: |_| None, docked_band_width: || 0.,
        });
        let root = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                Root{
                    first := Window{body +: {other_window := View{}}}
                    second := Window{body +: {
                        mod.widgets.Grid{secret := View{}}
                        Grid := View{width: 24 height: 24}
                        list := View{}
                    }}
                }
            });
            let root = WidgetRef::script_from_value(vm, value);
            let list = root.child_by_path(ids!(second.body.list));
            let mut list = list.borrow_mut::<View>().unwrap();
            let proto = vm.eval(script! { mod.widgets.View{width: 1 height: 1} });
            for i in 0..600 {
                let id = if i == 510 { live_id!(needle) } else { LiveId(0) };
                list.children.push((id, WidgetRef::script_from_value(vm, proto)));
            }
            root
        });
        crate::set_ui_root(cx, &root);
        for window in cx.windows.id_iter().collect::<Vec<_>>() {
            let geom = &mut cx.windows[window].window_geom;
            geom.inner_size = dvec2(320., 240.);
            geom.dpi_factor = 1.;
        }
        cx.redraw_all();
        let draw = Event::Draw(std::mem::take(&mut cx.new_draw_event));
        root.handle_event(cx, &draw, &mut Scope::empty());

        let root_page = read(cx, &[]);
        assert_eq!(root_page.get("window").and_then(JsonValue::as_u64), Some(1), "requested window index");
        let root_nodes = array(&array(&root_page, "parents")[0], "nodes");
        assert_eq!(root_nodes.len(), 1, "window scope has one root");
        assert_eq!(path(&root_nodes[0]), "/second", "scope starts at the second window");
        assert!(root_nodes[0].get("rect").is_none(), "window has no selection ring");
        let grid = root.child_by_path(ids!(second.body.Grid));
        let grid_path = pick::indexed_path(cx, grid.widget_uid().0);
        assert!(grid_path.ends_with("/Grid.2"), "index counts the inspector before its named sibling: {grid_path}");
        let kept = nodes(&root_page).find(|n| path(n) == grid_path).expect("ordinary sibling keeps the pick path");
        assert!(kept.get("rect").is_some(), "drawn visible sibling includes its rectangle");
        assert!(!nodes(&root_page).any(|n| path(n).contains("/first") || path(n).contains("/Grid.1")), "other window and inspector are excluded");
        assert_eq!(read(cx, &[("query", "secret")]).get("total_matches").and_then(JsonValue::as_u64), Some(0), "inspector descendants cannot match");
        let page = read(cx, &[("path", "list"), ("limit", "200")]);
        let next = read(cx, &[("path", "list"), ("offset", "200"), ("limit", "200")]);
        let mut siblings = BTreeSet::new();
        for (reply, offset) in [(&page, 0), (&next, 200)] {
            let parent = &array(reply, "parents")[0];
            assert_eq!(parent.get("offset").and_then(JsonValue::as_u64), Some(offset), "page begins at requested sibling");
            assert_eq!(parent.get("total").and_then(JsonValue::as_u64), Some(600), "complete child count");
            assert_eq!(nodes(reply).count(), 200, "full sibling page");
            for n in nodes(reply) { assert!(siblings.insert(path(n)), "sibling pages overlap at {}", path(n)); }
        }
        let clamped = read(cx, &[("path", "list"), ("limit", "601")]);
        assert_eq!(nodes(&clamped).count(), tree::MAX_LIMIT, "oversize requests return a bounded page");
        let rest = read(cx, &[("path", "list"), ("offset", &tree::MAX_LIMIT.to_string())]);
        assert_eq!(nodes(&rest).count(), 600 - tree::MAX_LIMIT, "all remaining siblings are reachable");
        let reveal = read(cx, &[("reveal", "needle"), ("limit", "50")]);
        let chain = array(&reveal, "parents");
        for pair in chain.windows(2) {
            assert!(array(&pair[0], "nodes").iter().any(|n| path(n) == path(&pair[1])), "reveal includes each ancestor");
        }
        let last = chain.last().unwrap();
        assert_eq!(last.get("total").and_then(JsonValue::as_u64), Some(600), "reveal retains full sibling count");
        assert!(last.get("offset").unwrap().as_u64().unwrap() > 0 && array(last, "nodes").iter().any(|n| path(n).ends_with("/needle")), "reveal pages around the target");
        let hits = read(cx, &[("query", "vIeW"), ("limit", "50")]);
        let later = read(cx, &[("query", "vIeW"), ("limit", "50"), ("offset", "50")]);
        let mut matched = BTreeSet::new();
        for reply in [&hits, &later] {
            assert!(reply.get("total_matches").unwrap().as_u64().unwrap() >= 600, "query counts all matches");
            assert_eq!(reply.get("truncated").and_then(JsonValue::as_bool), Some(true), "more matches are available");
            let page_hits: Vec<_> = nodes(reply).filter(|n| n.get("matched").and_then(JsonValue::as_bool) == Some(true)).collect();
            assert_eq!(page_hits.len(), 50, "query pages count hits independently of ancestors");
            for n in page_hits { assert!(matched.insert(path(n)), "duplicate query match {}", path(n)); }
        }
        let paths: BTreeSet<_> = [&root_page, &page, &next, &clamped, &rest, &reveal, &hits, &later]
            .into_iter().flat_map(nodes).map(path).collect();
        for path in paths {
            callback(cx, "design_props", &[("w".into(), "1".into()), ("path".into(), path.into())])
                .unwrap_or_else(|why| panic!("tree path {path} must resolve for design props: {why}"));
        }
    }

    #[test]
    fn design_picks_and_edits_in_the_requested_window() {
        let mut cx = crate::test_cx::checkout_test_cx(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                let window = Window{
                    // Wider than the 320 px caption bar even without devhub's
                    // MAKEPAD_TEST_LANE / MAKEPAD_REMOTE badges or Relax's
                    // CAPTION_BADGE_ROOM: proves the app-drawn caption control
                    // stays unclipped regardless of window title length.
                    window.title: "Long test window title that overflows a 320px caption bar on its own"
                    caption_bar +: {
                        height: 32
                        caption_label +: {
                            label +: {visible: false}
                            action := Button{text: "Action" width: 64 height: 24}
                        }
                    }
                    body +: {card := View{width: Fill height: Fill}}
                }
                Root{first := window{} second := window{}}
            });
            WidgetRef::script_from_value(vm, value)
        });
        crate::set_ui_root(&mut cx, &root);
        let callback = cx.tweak_callback.unwrap();
        let first = root.widget(&cx, ids!(first.body.card));
        let second = root.widget(&cx, ids!(second.body.card));
        assert_eq!(pick::resolve_target(&cx, "card", Some(1)).unwrap().widget_uid(), second.widget_uid(),
            "a window also scopes its widgets before their first draw");
        let windows: Vec<_> = cx.windows.id_iter().collect();
        for window in windows {
            let geom = &mut cx.windows[window].window_geom;
            geom.inner_size = dvec2(320., 240.);
            geom.dpi_factor = 1.;
        }
        cx.redraw_all();
        let draw = Event::Draw(std::mem::take(&mut cx.new_draw_event));
        root.handle_event(&mut cx, &draw, &mut Scope::empty());
        let action = root.widget(&cx, ids!(second.caption_bar.caption_label.action));
        let caption_rect = action.area().clipped_rect_union(&cx);
        assert!(
            (caption_rect.size.x - 64.).abs() < 0.5 && (caption_rect.size.y - 24.).abs() < 0.5,
            "caption button must have drawn unclipped: {caption_rect:?}"
        );
        let centre = caption_rect.center();
        let reference = pick::indexed_path(&cx, action.widget_uid().0);
        for (key, value) in [("at", format!("{},{}", centre.x, centre.y)), ("path", reference)] {
            let args = [(key.into(), value), ("w".into(), "1".into())];
            let reply = callback(&mut cx, "design_pick", &args).unwrap();
            assert!(reply.contains(&format!(r#""uid":{},"#, action.widget_uid().0)), "caption button in window 1: {reply}");
        }
        let window_path = [("path".into(), "second".into()), ("w".into(), "1".into())];
        assert!(callback(&mut cx, "design_pick", &window_path).unwrap_err().contains("Window itself"));
        let rect = second.area().clipped_rect_union(&cx);
        assert!(rect.size.x > 0. && rect.size.y > 0., "second window's card must have drawn: {rect:?}");
        let point = rect.center();
        assert!(first.area().clipped_rect_union(&cx).contains(point));
        let at = ("at".into(), format!("{},{}", point.x, point.y));
        for key in ["w", "window"] {
            let reply = callback(&mut cx, "design_pick", &[at.clone(), (key.into(), "1".into())]).unwrap();
            assert!(reply.contains(&format!(r#""uid":{},"#, second.widget_uid().0)) && reply.contains(r#""window":1"#), "{reply}");
        }
        let reply = callback(&mut cx, "design_pick", &[at, ("window".into(), "1".into()), ("w".into(), "0".into())]).unwrap();
        assert!(reply.contains(&format!(r#""uid":{},"#, first.widget_uid().0)), "{reply}");

        let args = [("path".into(), "card".into()), ("w".into(), "1".into()),
            ("prop".into(), "padding.left".into()), ("value".into(), "17".into())];
        let uid_suffix = format!(r#","uid":{}}}"#, second.widget_uid().0);
        assert!(callback(&mut cx, "design_apply", &args).unwrap().ends_with(&uid_suffix));
        assert_eq!(second.borrow::<View>().unwrap().layout.padding.left, 17.);
        assert_eq!(first.borrow::<View>().unwrap().layout.padding.left, 0.);
        let uid = format!("uid:{}", first.widget_uid().0);
        let other = pick::indexed_path(&cx, first.widget_uid().0);
        for path in [uid, other] {
            let reply = callback(&mut cx, "design_props", &[("path".into(), path), ("w".into(), "1".into())]);
            assert!(reply.unwrap_err().contains("in window 1"));
        }

        let width = [("path".into(), "card".into()), ("w".into(), "1".into()),
            ("prop".into(), "width".into()), ("value".into(), "123".into())];
        assert!(callback(&mut cx, "design_typed", &width).unwrap().ends_with(&uid_suffix));
        let draw = Event::Draw(std::mem::take(&mut cx.new_draw_event));
        root.handle_event(&mut cx, &draw, &mut Scope::empty());
        let rect = second.area().clipped_rect_union(&cx);
        assert_eq!(rect.size.x, 123., "the edited card has been laid out");
        let rect_args = |uid: u64| [("uid".into(), uid.to_string()), ("w".into(), "1".into())];
        let reply = callback(&mut cx, "design_rect", &rect_args(second.widget_uid().0)).unwrap();
        assert_eq!(reply, format!(r#"{{"window":1,"rect":[{},{},{},{}]}}"#,
            rect.pos.x, rect.pos.y, rect.size.x, rect.size.y));
        let hidden = root.widget(&cx, ids!(second.caption_bar.caption_label.label));
        assert!(callback(&mut cx, "design_rect", &rect_args(first.widget_uid().0)).unwrap_err().contains("in window 1"));
        assert_eq!(callback(&mut cx, "design_rect", &rect_args(hidden.widget_uid().0)).unwrap_err(), "widget has no drawn rect");
        assert_eq!(callback(&mut cx, "design_rect", &width[..2]).unwrap_err(), "need uid=");
        assert_eq!(callback(&mut cx, "design_rect", &rect_args(0)).unwrap_err(), "invalid widget uid");
    }

    #[test]
    fn registered_root_applies_to_only_the_addressed_instance() {
        let mut cx = crate::test_cx::checkout_test_cx(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let list = vm.eval(script! { [1, -2] });
            assert_eq!(props::fmt_value(vm.heap(), list, 1).as_deref(), Some("[1, -2]"), "a list with a negative item reads back");
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                let panel = mod.widgets.View{padding: Inset{left: 2}}
                mod.widgets.View{first := panel{} second := panel{} button := Button{} tile := mod.widgets.RectView{padding: 3}}
            });
            WidgetRef::script_from_value(vm, value)
        });
        crate::set_ui_root(&mut cx, &root);
        let callback = cx.tweak_callback.unwrap();
        let path = vec![("path".to_string(), "first".to_string())];
        let inherited = callback(&mut cx, "design_props", &path).unwrap();
        assert!(inherited.contains(r#""name":"padding.left","value":"2","set":false,"origin":"inherited""#), "{inherited}");
        let mut args = path.clone();
        args.extend([("prop".into(), "padding".into()), ("value".into(), "Inset{left: 17}".into())]);
        let applied = callback(&mut cx, "design_apply", &args).unwrap();
        assert!(applied.contains(r#""padding.left":"2""#) && applied.contains(r#""padding.left":"17""#), "{applied}");
        let props = callback(&mut cx, "design_props", &path).unwrap();
        assert!(props.contains(r#""name":"padding.left","value":"17","set":false,"origin":"runtime""#), "{props}");
        let noop = callback(&mut cx, "design_apply", &args).unwrap();
        assert_eq!(noop.matches(r#""padding.left":"17""#).count(), 2, "{noop}");
        assert_eq!(root.child(id!(second)).borrow::<View>().unwrap().layout.padding.left, 2.0);

        // design_typed: a value built from data against the declared type, nothing evaluated.
        let typed = |cx: &mut Cx, prop: &str, value: &str| {
            let mut args = path.clone();
            args.extend([("prop".to_string(), prop.to_string()), ("value".to_string(), value.to_string())]);
            callback(cx, "design_typed", &args)
        };
        let first = root.child(id!(first));
        let second_walk = root.child(id!(second)).borrow::<View>().unwrap().walk.width;
        let fixed = typed(&mut cx, "width", "Size.Fixed(200)").unwrap();
        assert!(fixed.contains(r#""width":"Size.Fixed(200)""#), "{fixed}");
        let fill = "Size.Fill{weight: 100 basis: FitBound.Abs(0) shrink: 0}";
        for (prop, value) in [("flow", "Flow.Down"), ("padding.left", "21"), ("width", fill)] {
            let reply = typed(&mut cx, prop, value).unwrap_or_else(|why| panic!("{prop}: {value}: {why}"));
            assert!(reply.contains(&format!(r#""{prop}":"#)), "{prop}: {value}: {reply}");
        }
        {
            let view = first.borrow::<View>().unwrap();
            assert_eq!(view.walk.width, Size::Fill { weight: 100.0, basis: FitBound::Abs(0.0), shrink: 0.0, min: None, max: None });
            assert!(matches!(view.layout.flow, Flow::Down), "flow: {:?}", view.layout.flow);
            assert_eq!(view.layout.padding.left, 21.0);
        }
        let rows = |cx: &mut Cx| props::reflect_flat(cx, &first);
        let before = rows(&mut cx);
        for (prop, value) in [("width", "Danger(1)"), ("width", "loop{}"), ("width", "Flow.Down"), ("padding.left", "\"x\""),
            ("on_render", "fn(){}"), ("nope", "1"), ("padding.le ft", "1"), ("padding", "Inset{left := 3}"), ("width", "mod.std.print(1)")] {
            assert!(typed(&mut cx, prop, value).is_err(), "{prop}: {value} was applied");
        }
        let deep = format!("{}0{}", "Size.Fixed(".repeat(100_000), ")".repeat(100_000));
        assert!(typed(&mut cx, "width", &deep).is_err(), "a value 100k deep is refused, and the app lives");
        assert_eq!(rows(&mut cx), before, "a refused value changes nothing");
        assert_eq!(root.child(id!(second)).borrow::<View>().unwrap().walk.width, second_walk, "the other instance is untouched");
        // Round trip: every row a View lists builds back to the same row, or is refused.
        let mut refused = Vec::new();
        for (name, value, _) in rows(&mut cx) {
            match typed(&mut cx, &name, &value) {
                Ok(_) => {
                    let now = rows(&mut cx).into_iter().find(|(row, _, _)| *row == name).map(|(_, text, _)| text);
                    assert_eq!(now.as_deref(), Some(value.as_str()), "{name} does not round-trip");
                }
                Err(why) => {
                    if name == "draw_bg" {
                        assert!(why.contains("is a draw shader"), "the builder must refuse the whole draw layer: {why}");
                    }
                    refused.push(format!("{name}: {value} ({why})"));
                }
            }
        }
        // Scroll (`vec2f(0, 0)`) builds back to itself. Only the whole draw layer and the
        // unnamed id (`@-`) are refused.
        let names: Vec<&str> = refused.iter().filter_map(|row| row.split(':').next()).collect();
        assert_eq!(names, ["draw_bg", "container_id"], "refused rows:\n{}", refused.join("\n"));
        // A draw layer's inputs declared in script (`color: instance(#0000)`, `border_size:
        // uniform(0.0)`) take values of their own kind; its fns and Rust plumbing take nothing.
        let tile = root.child(id!(tile));
        let on_tile = |cx: &mut Cx, prop: &str, value: &str| {
            let args = [("path".to_string(), "tile".to_string()), ("prop".to_string(), prop.to_string()), ("value".to_string(), value.to_string())];
            callback(cx, "design_typed", &args)
        };
        // An input's row reads the source until the next draw: read the applied slots instead.
        let slots = |cx: &mut Cx| {
            let view = tile.borrow::<View>().unwrap();
            let (mut colour, mut colour_2, mut border) = ([0.0f32; 4], [0.0f32; 4], [0.0f32; 1]);
            view.draw_bg.get_instance(cx, id!(color), &mut colour);
            view.draw_bg.get_instance(cx, id!(color_2), &mut colour_2);
            view.draw_bg.get_uniform(cx, id!(border_size), &mut border);
            (colour, colour_2, border)
        };
        let reply = on_tile(&mut cx, "draw_bg.color", "#0f0").unwrap();
        assert!(reply.contains(r#""draw_bg.color":"#), "{reply}");
        on_tile(&mut cx, "draw_bg.border_size", "2").unwrap();
        // A vec4 input (a gradient's `color_2: instance(vec4(-1))`) takes a colour.
        on_tile(&mut cx, "draw_bg.color_2", "#f00").unwrap();
        let applied = ([0.0, 1.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [2.0]);
        assert_eq!(slots(&mut cx), applied, "a colour instance, a vec4 instance and a number uniform apply");
        // A field of a struct held as a shorthand (`padding: 3`) sets the struct whole, from its
        // rows: the other sides keep the shorthand's value.
        on_tile(&mut cx, "padding.left", "9").unwrap();
        let sides: Vec<String> = props::reflect_flat(&mut cx, &tile).into_iter().filter(|(name, _, _)| name.starts_with("padding.")).map(|(name, text, _)| format!("{name}={text}")).collect();
        assert_eq!(sides, ["padding.left=9", "padding.top=3", "padding.right=3", "padding.bottom=3"]);
        let tile_rows = props::reflect_flat(&mut cx, &tile);
        for (prop, value) in [("draw_bg.color", "1"), ("draw_bg.color_2", "1"), ("draw_bg.border_size", "#fff"), ("draw_bg.pixel", "fn(){ return #f00 }"),
            ("draw_bg.pixel", "1"), ("draw_bg.rect_pos", "1"), ("draw_bg.draw_depth", "1"), ("draw_bg.color", "Danger(1)")] {
            assert!(on_tile(&mut cx, prop, value).is_err(), "{prop}: {value} was applied");
        }
        assert_eq!(props::reflect_flat(&mut cx, &tile), tile_rows, "a refused input changes no row");
        assert_eq!(slots(&mut cx), applied, "a refused input changes nothing");
        on_tile(&mut cx, "draw_bg.color_2", "vec4f(0, 0, 1, 1)").unwrap();
        assert_eq!(slots(&mut cx).1, [0.0, 0.0, 1.0, 1.0], "a vector literal sets the gradient's instance slot");
        for (prop, value, reason) in [
            ("draw_bg.color_2", "vec2f(1 1)", "takes a colour or vec4f(..)"),
            ("draw_bg.border_size", "vec4f(1 1 1 1)", "takes a number"),
        ] {
            let why = on_tile(&mut cx, prop, value).unwrap_err();
            assert!(why.contains(reason), "{prop}: {value} was refused for another reason: {why}");
        }
        let row = |cx: &mut Cx, name: &str| props::reflect_flat(cx, &tile).into_iter().find(|(row, _, _)| row == name).map(|(_, text, _)| text);
        let mut refused = Vec::new();
        for (name, value, _) in props::reflect_flat(&mut cx, &tile) {
            match on_tile(&mut cx, &name, &value) {
                Ok(_) => assert_eq!(row(&mut cx, &name).as_deref(), Some(value.as_str()), "{name} does not round-trip"),
                Err(why) => refused.push(format!("{name}: {value} ({why})")),
            }
        }
        // The gradient inputs' pod default (`vec4f(-1, -1, -1, -1)`) also round-trips.
        // The whole draw layer and the unnamed id are refused here too.
        let names: Vec<&str> = refused.iter().filter_map(|row| row.split(':').next()).collect();
        assert_eq!(names, ["draw_bg", "container_id"],
            "refused tile rows:\n{}", refused.join("\n"));
        let mut pose = vec![("path".into(), "button".into()), ("layer".into(), "draw_bg".into())];
        let tracks = callback(&mut cx, "design_states", &pose).unwrap();
        assert!(tracks.contains("hover.down"), "{tracks}");
        pose.push(("state".into(), "hover.down".into()));
        callback(&mut cx, "design_pose", &pose).unwrap();
        for op in ["apply", "design_open"] {
            assert_eq!(callback(&mut cx, op, &args).unwrap_err(), "no tweaker (this app has no widgets ui root)");
        }
    }
}
