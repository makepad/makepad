//! Shared semantic presentation helpers. Widget state and actions remain on the widget.
use crate::{widget::*, *};

pub fn apply_overrides(
    widget: &dyn Widget,
    fallback: Option<&ScriptObjectRef>,
    cx: &Cx,
    node: &mut a11y_tree::Node,
) {
    let Some(vm) = cx.script_vm.as_ref() else {
        return;
    };
    let heap = vm.heap_ref();
    let source = fallback.map_or_else(|| widget.script_source(), |source| source.as_object());
    if source == ScriptObject::ZERO
        || (fallback.map_or_else(
            || widget.script_source_heap_key(),
            |source| source.heap_key(),
        ) != 0
            && fallback.map_or_else(
                || widget.script_source_heap_key(),
                |source| source.heap_key(),
            ) != heap.heap_key())
    {
        return;
    }
    let Some(props) = heap
        .value(source, live_id!(a11y).into(), NoTrap)
        .as_object()
    else {
        return;
    };
    let get = |id: LiveId| heap.value(props, id.into(), NoTrap);
    let role = get(live_id!(role));
    let role_name = heap
        .string_with(role, |_, text| text.to_owned())
        .or_else(|| role.as_id().map(crate::widget_tree::live_id_token))
        .or_else(|| {
            role.as_object()
                .and_then(|obj| heap.object_type_id(obj))
                .and_then(|ty| {
                    cx.components
                        .get::<WidgetRegistry>()
                        .map
                        .get(&ty)
                        .map(|(info, _)| crate::widget_tree::live_id_token(info.name))
                })
        });
    if let Some(name) = role_name {
        if let Some(role) = a11y_tree::Role::ALL.iter().find(|r| {
            r.short()
                .replace('_', "")
                .eq_ignore_ascii_case(&name.replace('_', ""))
                || format!("{r:?}").eq_ignore_ascii_case(&name)
        }) {
            node.role = *role;
        }
    }
    for (key, target) in [
        (live_id!(label), &mut node.name),
        (live_id!(description), &mut node.description),
    ] {
        heap.string_with(get(key), |_, text| {
            makepad_platform::a11y::set_string(target, text)
        });
    }
    if let Some(hidden) = get(live_id!(hidden)).as_bool() {
        node.states.set(a11y_tree::States::HIDDEN, hidden);
    }
    let live = get(live_id!(live));
    let live = heap
        .string_with(live, |_, text| text.to_owned())
        .or_else(|| live.as_id().map(crate::widget_tree::live_id_token));
    if let Some(live) = live {
        node.live = match live.to_ascii_lowercase().as_str() {
            "polite" => a11y_tree::Live::Polite,
            "assertive" => a11y_tree::Live::Assertive,
            _ => a11y_tree::Live::Off,
        };
    }
}

/// Line boxes and character boundaries from the layout used to draw the text. No shaping here.
pub fn text_layout(
    text: &mut a11y_tree::TextInfo,
    layout: &text::layouter::LaidoutText,
    origin: DVec2,
    scale: f32,
    glyphs: bool,
) {
    use unicode_segmentation::UnicodeSegmentation;
    text.lines.clear();
    text.glyph_x.clear();
    for row in &layout.rows {
        let x = origin.x as f32 + row.origin_in_lpxs.x * scale;
        let baseline = origin.y as f32 + row.origin_in_lpxs.y * scale;
        text.lines.push(a11y_tree::LineBox {
            start: row.text.start_in_parent() as u32,
            rect: a11y_tree::Rect {
                x,
                y: baseline - row.ascender_in_lpxs * scale,
                w: row.width_in_lpxs * scale,
                h: (row.ascender_in_lpxs - row.descender_in_lpxs) * scale,
            },
            baseline,
        });
        if !glyphs {
            continue;
        }
        // Walk shaping clusters once. Ligatures divide their advance over graphemes;
        // combining characters share their grapheme's x, just as cursor hit testing does.
        let mut glyph = 0;
        let mut byte = 0;
        while glyph < row.glyphs.len() {
            let start = row.glyphs[glyph].cluster;
            let from = row.glyphs[glyph].origin_in_lpxs.x;
            let mut next = glyph + 1;
            while next < row.glyphs.len() && row.glyphs[next].cluster == start {
                next += 1;
            }
            let end = row.glyphs.get(next).map_or(row.text.len(), |g| g.cluster);
            let to = row
                .glyphs
                .get(next)
                .map_or(row.width_in_lpxs, |g| g.origin_in_lpxs.x);
            if start >= byte
                && end >= start
                && end <= row.text.len()
                && row.text.is_char_boundary(start)
                && row.text.is_char_boundary(end)
            {
                for _ in row.text[byte..start].chars() {
                    text.glyph_x.push(x + from * scale);
                }
                let segment = &row.text[start..end];
                let count = segment.graphemes(true).count().max(1);
                for (i, grapheme) in segment.graphemes(true).enumerate() {
                    let gx = x + (from + (to - from) * i as f32 / count as f32) * scale;
                    for _ in grapheme.chars() {
                        text.glyph_x.push(gx);
                    }
                }
                byte = end;
            }
            glyph = next;
        }
        for _ in row.text[byte..].chars() {
            text.glyph_x.push(x + row.width_in_lpxs * scale);
        }
        text.glyph_x.push(x + row.width_in_lpxs * scale);
        if row.newline {
            text.glyph_x.push(x + row.width_in_lpxs * scale);
        }
    }
}

/// Directly drawn controls (such as a scroll bar) enter the same builder as WidgetRef leaves.
pub fn record_leaf(widget: &dyn Widget, cx: &mut Cx2d) {
    let list = cx.draw_list_stack.last().copied();
    let token = cx.a11y.begin_node(widget.widget_uid().0, list);
    let mut node = cx.a11y.take_node(token);
    widget.semantics(cx, &mut node);
    apply_overrides(widget, None, cx, &mut node);
    if let Some(rect) = makepad_platform::a11y::snapshot_rect(&widget.area(), cx) {
        node.bounds = makepad_platform::a11y::semantic_rect(rect);
    }
    cx.a11y.end_node(token, node, widget.area());
}

/// A label's target is found in the widget graph, so references can precede their target draw.
pub fn label_target(widget: &dyn Widget, fallback: Option<&ScriptObjectRef>, cx: &Cx) -> u64 {
    let Some(vm) = cx.script_vm.as_ref() else {
        return 0;
    };
    let heap = vm.heap_ref();
    let source = fallback.map_or_else(|| widget.script_source(), |source| source.as_object());
    if source == ScriptObject::ZERO
        || (fallback.map_or_else(
            || widget.script_source_heap_key(),
            |source| source.heap_key(),
        ) != 0
            && fallback.map_or_else(
                || widget.script_source_heap_key(),
                |source| source.heap_key(),
            ) != heap.heap_key())
    {
        return 0;
    }
    let Some(props) = heap
        .value(source, live_id!(a11y).into(), NoTrap)
        .as_object()
    else {
        return 0;
    };
    let target = heap.value(props, live_id!(label_for).into(), NoTrap);
    let path: Vec<LiveId> = if let Some(path) = heap.string_with(target, |_, text| {
        text.split('.').map(LiveId::from_str).collect()
    }) {
        path
    } else if let Some(id) = target.as_id() {
        vec![id]
    } else {
        return 0;
    };
    cx.widget_tree()
        .find_flood(widget.widget_uid(), &path)
        .widget_uid()
        .0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_and_widget_actions_share_the_public_state() {
        crate::test_cx::on_test_cx(|| {
            let mut cx = crate::test_cx::checkout_test_cx(crate::script_mod);
            let (mut button, mut check, mut slider, mut input) = cx.with_vm(|vm| {
                let button = script_eval!(vm, {use mod.widgets.* Button{text: "disk" a11y: {role: Button label: "Save" description: "Document" live: @Polite}}});
                let check = script_eval!(vm, {use mod.widgets.* CheckBox{text: "Enabled"}});
                let slider = script_eval!(vm, {use mod.widgets.* Slider{text: "Gain" min: 0. max: 10. step: 1. default: 2.}});
                let input = script_eval!(vm, {use mod.widgets.* TextInput{text: "aé🦀" is_multiline: true}});
                (crate::button::Button::script_from_value(vm, button), crate::check_box::CheckBox::script_from_value(vm, check), crate::slider::Slider::script_from_value(vm, slider), crate::text_input::TextInput::script_from_value(vm, input))
            });
            let mut node = a11y_tree::Node::default();
            button.semantics(&cx, &mut node);
            apply_overrides(&button, None, &cx, &mut node);
            assert_eq!(
                (
                    node.role,
                    node.name.as_str(),
                    node.description.as_str(),
                    node.live
                ),
                (
                    a11y_tree::Role::Button,
                    "Save",
                    "Document",
                    a11y_tree::Live::Polite
                )
            );
            let mut request = a11y_tree::ActionRequest {
                req: 1,
                node: a11y_tree::NodeId::NONE,
                action: a11y_tree::Actions::PRESS,
                arg: a11y_tree::ActionArg::None,
            };
            let actions = cx.capture_actions(|cx| {
                assert_eq!(
                    button.handle_action(cx, &request, &mut Scope::empty()),
                    a11y_tree::ActionResult::Handled
                );
            });
            assert!(button.clicked(&actions));
            request.action = a11y_tree::Actions::TOGGLE;
            let actions = cx.capture_actions(|cx| {
                assert_eq!(
                    check.handle_action(cx, &request, &mut Scope::empty()),
                    a11y_tree::ActionResult::Handled
                );
            });
            assert_eq!(check.changed(&actions), Some(true));
            request.action = a11y_tree::Actions::SET_VALUE;
            request.arg = a11y_tree::ActionArg::Number(7.0);
            let actions = cx.capture_actions(|cx| {
                assert_eq!(
                    slider.handle_action(cx, &request, &mut Scope::empty()),
                    a11y_tree::ActionResult::Handled
                );
            });
            assert!(!actions.is_empty());
            assert_eq!(slider.value(), 7.0);
            request.action = a11y_tree::Actions::REPLACE_TEXT;
            request.arg = a11y_tree::ActionArg::Replace(1, 3, "β".into());
            let actions = cx.capture_actions(|cx| {
                assert_eq!(
                    input.handle_action(cx, &request, &mut Scope::empty()),
                    a11y_tree::ActionResult::Handled
                );
            });
            assert!(!actions.is_empty());
            assert_eq!(input.text(), "aβ🦀");
            request.arg = a11y_tree::ActionArg::Replace(2, 3, "x".into());
            assert_eq!(
                input.handle_action(&mut cx, &request, &mut Scope::empty()),
                a11y_tree::ActionResult::Unsupported
            );
            assert_eq!(input.text(), "aβ🦀");
        });
    }
}
