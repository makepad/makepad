//! Property-panel rows on a 20 pt field / 22 pt row scale, with a capped
//! label column and reserved trailing actions. Stacks join fields at 1 pt gaps.

use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    tip::{TipAction, TipPlace, TipRequest},
    value_input::*,
    view::View,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PropRow = View{
        width: Fill height: theme.prop_row_height
        flow: Right align: Align{y: 0.5} spacing: 0
        label := Label{
            text: ""
            width: Fill{weight: 0 basis: "42%" max: theme.prop_label_width}
            height: Fit
            padding: Inset{}
            align: Align{x: 1.0 y: 0.5}
            flow: Right
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
            draw_text +: {
                ink_centered: true
                color: theme.prop_ink_dim
                text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
            }
        }
        gap := View{width: theme.prop_label_gap height: Fill}
        value := View{width: Fill height: Fill}
        trail := View{width: theme.prop_trail_width height: Fill align: Center}
    }
    mod.widgets.PropNumberRow = mod.widgets.PropRow{
        value +: {
            // The row sets the height: a sheet's taller field minimum would put the text below the field's middle.
            num := ValueInput{width: Fill height: Fill min_height: 0}
        }
    }
    mod.widgets.PropStackBase = #(PropStack::register_widget(vm))
    // Add named PropNumberRow children; each keeps its own label and trail slot.
    mod.widgets.PropStack = set_type_default() do mod.widgets.PropStackBase{
        width: Fill height: Fit flow: Down spacing: theme.prop_join_gap
    }
    mod.widgets.PropTrailButton = ButtonFlatterIcon{
        width: theme.prop_trail_width height: theme.prop_trail_width
        padding: Inset{} margin: Inset{} spacing: 0
        icon_walk: Walk{width: 14 height: 14}
        draw_bg +: {pixel: fn() { return vec4(0.0) }}
        draw_icon +: {color: theme.prop_ink_dim}
        animator +: {
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_icon: {color: theme.prop_ink_dim}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_icon: {color: theme.color_text}}
                }
                down: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_icon: {color: theme.color_text}}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct PropStack {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    fields: Vec<ValueInputRef>,
}

impl Widget for PropStack {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.fields.clear();
        for (_, row) in &self.view.children {
            if !row.visible() { continue; }
            let field = row.value_input(cx, ids!(value.num));
            if field.borrow().is_some_and(|field| field.visible()) {
                self.fields.push(field);
            }
        }
        let count = self.fields.len();
        for (index, field) in self.fields.iter().enumerate() {
            if let Some(mut field) = field.borrow_mut() {
                field.join = match (index == 0, index + 1 == count) {
                    (true, true) => ValueInputJoin::Both,
                    (true, false) => ValueInputJoin::Top,
                    (false, true) => ValueInputJoin::Bottom,
                    (false, false) => ValueInputJoin::None,
                };
            }
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

/// Hover hints for property rows, as a real tooltip. Hovering a row must never
/// change the layout, so a row's hint is not inserted into the list: the owner of
/// the rows reports which row the pointer is on and this raises the window's
/// `TipLayer` tip (after its reveal delay, kept inside the window, hidden on
/// leave, press or scroll) anchored at the row's final rect.
#[derive(Default)]
pub struct PropHint {
    row: Option<LiveId>,
}

impl PropHint {
    /// The row under the pointer, with its final rect and hint text (empty for
    /// none), or `None` off every row. Raises or takes down the tip only when the
    /// row changes.
    pub fn hover(&mut self, cx: &mut Cx, owner: WidgetUid, row: Option<(LiveId, Rect, &str)>) {
        let id = row.map(|r| r.0);
        if id == self.row {
            return;
        }
        if self.row.take().is_some() {
            cx.widget_action(owner, TipAction::HoverOut);
        }
        if let Some((id, rect, hint)) = row {
            if !hint.is_empty() {
                self.row = Some(id);
                let mut request = TipRequest::plain(hint.to_string(), rect);
                request.place = TipPlace::Bottom;
                request.wrap_width = 260.0;
                cx.widget_action(owner, TipAction::HoverInWith(request));
            } else {
                self.row = None;
            }
        }
    }

    /// Take the tip down (the pointer left the rows or a drag began).
    pub fn clear(&mut self, cx: &mut Cx, owner: WidgetUid) {
        self.hover(cx, owner, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use crate::makepad_script::script;
    use crate::tip::TipLayer;

    const SIZE: Vec2d = Vec2d { x: 320.0, y: 200.0 };

    fn draw(cx: &mut Cx, pass: &DrawPass, list: &mut DrawList2d, root: &WidgetRef) {
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(pass, None);
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(SIZE, Layout::flow_down());
        root.draw_all(&mut cx2d, &mut Scope::empty());
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(pass);
    }

    /// Hovering a row with a hint raises a tooltip and moves no row: the hint is
    /// never part of the list. Leaving takes the tip down.
    #[test]
    fn a_hint_is_a_tooltip_and_hovering_moves_no_row() {
        crate::on_test_cx(|| {
            let mut cx_pool = crate::checkout_test_cx();
            let cx: &mut Cx = &mut cx_pool;
            let root = cx.with_vm(|vm| {
                let value = vm.eval(script! {
                    use mod.prelude.widgets.*
                    View{
                        width: Fill height: Fill flow: Down
                        a := PropNumberRow{label +: {text: "Half size"}}
                        b := PropNumberRow{label +: {text: "Corner radius, which is long"}}
                        c := PropNumberRow{label +: {text: "Count"}}
                    }
                });
                WidgetRef::script_from_value(vm, value)
            });
            let mut layer = cx.with_vm(|vm| {
                let value = vm.eval(script! {
                    use mod.prelude.widgets.*
                    TipLayer{}
                });
                TipLayer::script_from_value(vm, value)
            });
            let pass = DrawPass::new(cx);
            pass.set_size(cx, SIZE);
            let mut list = DrawList2d::new(cx);
            draw(cx, &pass, &mut list, &root);
            let rects = |cx: &Cx| -> Vec<Rect> { [id!(a), id!(b), id!(c)].iter().map(|id| root.widget(cx, &[*id]).area().rect(cx)).collect() };
            let before = rects(cx);
            assert!(before[0].size.y > 0.0 && before[1].pos.y > before[0].pos.y);

            let owner = root.widget_uid();
            let mut hint = PropHint::default();
            let feed = |cx: &mut Cx, layer: &mut TipLayer, row: Option<(LiveId, Rect, &str)>, hint: &mut PropHint| {
                let actions = cx.capture_actions(|cx| hint.hover(cx, owner, row));
                layer.handle_event(cx, &Event::Actions(actions), &mut Scope::empty());
            };
            let text = "from the link's centre to the middle of its bar";
            feed(cx, &mut layer, Some((id!(a), before[0], text)), &mut hint);
            assert_eq!(layer.held_text().as_deref(), Some(text), "the hint is a tooltip");
            draw(cx, &pass, &mut list, &root);
            assert_eq!(rects(cx), before, "hovering must not move a row");

            feed(cx, &mut layer, Some((id!(a), before[0], text)), &mut hint);
            assert_eq!(layer.held_text().as_deref(), Some(text));
            feed(cx, &mut layer, Some((id!(b), before[1], "")), &mut hint);
            assert_eq!(layer.held_text(), None, "a row without a hint raises none");
            feed(cx, &mut layer, Some((id!(a), before[0], text)), &mut hint);
            feed(cx, &mut layer, None, &mut hint);
            assert_eq!(layer.held_text(), None, "leaving takes it down");
            draw(cx, &pass, &mut list, &root);
            assert_eq!(rects(cx), before);
        });
    }
}
