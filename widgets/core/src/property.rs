//! Property-panel rows with a capped label column. Narrow rows put the label and its
//! trailing actions above a full-width field. PropStack switches its rows together and joins their fields.

use crate::{
    bare_step::BareStep,
    label::Label,
    makepad_derive_widget::*,
    makepad_draw::*,
    tip::{TipAction, TipPlace, TipRequest},
    value_input::*,
    view::View,
    widget::*,
    widget_async::{ScriptAsyncId, ScriptAsyncResult},
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PropRowBase = #(PropRow::register_widget(vm))
    mod.widgets.PropRow = set_type_default() do mod.widgets.PropRowBase{
        width: Fill height: theme.prop_row_height
        stack_line_height: theme.prop_row_height
        flow: Right align: Align{y: 0.5} spacing: 0
        // Allow 2.9 em for a value such as "100.0" (font points × 96/72), plus both arrow zones.
        value_min: theme.prop_font_size * (96.0 / 72.0) * 2.9 + 2.0 * theme.prop_arrow_zone
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
                text_style: theme.font_regular{font_size: theme.prop_font_size line_spacing: 1.0}
            }
        }
        gap := View{width: theme.prop_label_gap height: Fill}
        value := View{width: Fill height: Fill}
        // No room is kept for trailing actions unless a host reserves it (`trail +: {width: theme.prop_trail_width}`)
        // on every row of its list, so a panel without them gives the width to its values.
        trail := View{width: 0 height: Fill align: Center}
    }
    mod.widgets.PropNumberRow = mod.widgets.PropRow{
        value +: {
            // The row sets the height: a sheet's taller field minimum would put the text below the field's middle.
            num := ValueInput{width: Fill height: Fill min_height: 0}
        }
    }
    // A text field on the property scale, in the number field's well: no border at rest, a
    // one-device-pixel focus ring, edges on device pixels, the text at the property size.
    mod.widgets.PropTextInput = TextInput{
        width: Fill height: theme.prop_row_height min_height: 0
        margin: Inset{}
        padding: Inset{left: theme.prop_pad + 2.0 right: theme.prop_pad + 2.0}
        draw_bg +: {
            color: theme.prop_field
            color_hover: theme.prop_field_hover
            color_focus: theme.prop_field_active
            color_down: theme.prop_field_active
            color_empty: theme.prop_field
            border_color_focus: theme.color_focus
            border_radius: theme.prop_radius
            pixel: fn() {
                let dpi = self.draw_pass.dpi_factor
                let px = 1.0 / dpi
                let lo = floor(self.rect_pos * dpi + 0.5) / dpi - self.rect_pos
                let hi = floor((self.rect_pos + self.rect_size) * dpi + 0.5) / dpi - self.rect_pos
                let sdf = Sdf2d.viewport_px(self.pos * self.rect_size, dpi)
                let face = self.color.mix(self.color_hover, self.hover).mix(self.color_focus, self.focus)
                sdf.box(lo.x + px * 0.5, lo.y + px * 0.5, hi.x - lo.x - px, hi.y - lo.y - px, self.border_radius)
                sdf.fill_keep(face.mix(self.color_disabled, self.disabled))
                // SDF strokes take a half-width.
                sdf.stroke(vec4(self.border_color_focus.rgb, self.border_color_focus.a * self.focus), px * 0.5)
                return sdf.result
            }
        }
        draw_text +: {
            ink_centered: true
            text_style: theme.font_regular{font_size: theme.prop_font_size line_spacing: 1.0}
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

#[derive(Script, Widget)]
pub struct PropRow {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    #[cast]
    view: View,
    /// Minimum inline field width before the label moves above it, in layout points.
    #[live(59.9)]
    pub value_min: f64,
    /// Height available to Fill children on each stacked line when the row itself is Fit.
    #[live]
    stack_line_height: f64,
    #[rust]
    stack_layout: Option<(u64, bool)>,
    #[rust]
    draw_state: DrawStateWrap<PropRowDraw>,
    #[rust]
    frame_children: SmallVec<[(LiveId, WidgetRef); 2]>,
}

#[derive(Clone)]
enum PropRowDraw {
    Wide,
    Trail(Option<DeferredWalk>),
    Label(Option<Walk>),
    Value(Walk),
}

impl ScriptHook for PropRow {
    fn on_after_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        if self.frame_children.is_empty() {
            let step = WidgetRef::new_with_inner(Box::new(BareStep::script_new(vm)));
            self.frame_children.push((id!(body_step), step));
        }
    }
}

impl PropRow {
    /// Keep the original wide layout whenever a child's width cannot be priced before drawing.
    fn needs_stack(&self, cx: &mut Cx2d, width: f64) -> bool {
        if !width.is_finite() || self.view.child_reserve().is_some()
            || !matches!(self.view.layout.flow, Flow::Right { wrap: false, .. }) {
            return false;
        }
        let inner = (width - self.view.layout.padding.width()).max(0.0);
        let mut remaining = inner;
        // A missing or hidden value needs no space. A Fill value uses the final remainder.
        let mut value_needs_stack = Some(false);
        let mut count = 0usize;
        for (id, child) in &self.view.children {
            if !child.visible() { continue; }
            let walk = child.walk(cx);
            let margin = walk.margin.width();
            let Some(size) = in_flow_width(walk) else { return false };
            let used = match size {
                size if *id == id!(value) && size == Size::fill() => 0.0,
                Size::Fixed(w) => w.max(0.0),
                Size::Fit { min: None, max: None } => {
                    let Some(width) = intrinsic_width(cx, child, walk) else { return false };
                    width - margin
                }
                Size::Fill { weight, shrink, basis, min, max }
                    if *id == id!(label) && weight == 0.0 && shrink == 0.0 => {
                    // The row is still unopened. A percentage basis belongs to its inner width.
                    let basis = match basis {
                        FitBound::Abs(w) => w,
                        FitBound::Rel { base: Base::Parent, factor } => inner * factor,
                        _ => return false,
                    };
                    basis.min(max.map_or(f64::INFINITY, |w| w - margin))
                        .max(min.map_or(0.0, |w| w - margin)).max(0.0)
                }
                _ => return false,
            };
            if *id == id!(value) {
                value_needs_stack = (!size.is_fill()).then_some(used < self.value_min);
            }
            remaining -= used + margin;
            count += 1;
        }
        remaining -= count.saturating_sub(1) as f64 * self.view.layout.spacing;
        value_needs_stack.unwrap_or(remaining.max(0.0) < self.value_min)
    }

    // Let View own the background, clip and hit area around both lines. Its draw step
    // opens the frame for our layout. Restore its children before drawing any of them
    // or returning a child's step to the host.
    fn draw_frame(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        std::mem::swap(&mut self.view.children, &mut self.frame_children);
        let flow = std::mem::replace(&mut self.view.layout.flow, Flow::Overlay);
        let step = self.view.draw_walk(cx, scope, Walk { height: Size::fit(), ..walk });
        self.view.layout.flow = flow;
        std::mem::swap(&mut self.view.children, &mut self.frame_children);
        step
    }
}

fn in_flow_width(walk: Walk) -> Option<Size> {
    (walk.abs_pos.is_none() && walk.min_width.is_none() && walk.max_width.is_none()
        && walk.aspect.is_none()).then_some(walk.width)
}

// Fit slots can be ordinary views containing actions. Buttons and labels measure their
// own text; the enclosing view adds its padding, margins and inter-child spacing.
fn intrinsic_width(cx: &mut Cx2d, child: &WidgetRef, walk: Walk) -> Option<f64> {
    if let Some(width) = child.measure_width(cx, None) { return Some(width); }
    let view = child.borrow::<View>()?;
    if view.child_reserve().is_some() { return None; }
    let horizontal = match view.layout.flow {
        Flow::Right { wrap: false, .. } => true,
        Flow::Down | Flow::Overlay => false,
        _ => return None,
    };
    let mut width = 0.0_f64;
    let mut count = 0usize;
    for (_, child) in &view.children {
        if !child.visible() { continue; }
        let walk = child.walk(cx);
        let child_width = match in_flow_width(walk)? {
            Size::Fixed(w) => w.max(0.0) + walk.margin.width(),
            Size::Fit { min: None, max: None } => intrinsic_width(cx, child, walk)?,
            _ => return None,
        };
        width = if horizontal { width + child_width } else { width.max(child_width) };
        count += 1;
    }
    if horizontal { width += count.saturating_sub(1) as f64 * view.layout.spacing; }
    Some(walk.margin.width() + view.layout.padding.width() + width)
}

impl Widget for PropRow {
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        self.view.semantics(cx, node);
    }

    fn is_interactive(&self) -> bool {
        self.view.is_interactive()
    }

    fn clips_content(&self) -> bool {
        self.view.clips_content()
    }

    fn script_call(&mut self, vm: &mut ScriptVm, method: LiveId, args: ScriptValue) -> ScriptAsyncResult {
        self.view.script_call(vm, method, args)
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        self.view.script_result(vm, id, result);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, PropRowDraw::Wide) {
            let stacked = match self.stack_layout.take() {
                Some((frame, stacked)) if frame == cx.redraw_id() => stacked,
                _ => {
                    let width = cx.peek_walk_turtle(walk).size.x;
                    self.needs_stack(cx, width)
                }
            };
            if stacked {
                if self.draw_frame(cx, scope, walk).is_done() {
                    self.draw_state.end();
                    return DrawStep::done();
                }
                cx.begin_turtle(Walk::fill_fit(), Layout::flow_down());
                let label = self.view.child(id!(label));
                let row_height = self.view.walk.height.to_fixed();
                cx.begin_turtle(Walk::fill_fit(), Layout::flow_right().with_align_x(1.0).with_align_y(0.5));
                if label.visible() || self.view.child(id!(trail)).visible() {
                    cx.turtle_mut().allocate_height(row_height.unwrap_or(self.stack_line_height));
                }
                // Leave the label's width open until the trail has taken its own width.
                let deferred = if label.visible() {
                    let mut walk = Walk { width: Size::fill(), abs_pos: None, ..label.walk(cx) };
                    if let Some(height) = row_height {
                        walk.height = Size::Fixed(height);
                    }
                    cx.defer_walk_turtle(walk)
                } else {
                    None
                };
                self.draw_state.set(PropRowDraw::Trail(deferred));
            }
        }
        match self.draw_state.as_ref() {
            Some(PropRowDraw::Wide) => return self.view.draw_walk(cx, scope, walk),
            None => return DrawStep::done(),
            _ => {}
        }
        if let Some(PropRowDraw::Trail(mut deferred)) = self.draw_state.get() {
            let trail = self.view.child(id!(trail));
            if trail.visible() {
                let walk = trail.walk(cx);
                trail.draw_walk(cx, scope, walk)?;
            }
            self.draw_state.set(PropRowDraw::Label(deferred.as_mut().map(|label| label.resolve(cx))));
        }
        if let Some(PropRowDraw::Label(walk)) = self.draw_state.get() {
            if let Some(walk) = walk {
                let label = self.view.child(id!(label));
                let align = label.borrow_mut::<Label>().map(|mut label| std::mem::replace(&mut label.align.x, 0.0));
                let step = label.draw_walk(cx, scope, walk);
                if let Some(x) = align {
                    label.borrow_mut::<Label>().unwrap().align.x = x;
                }
                step?;
            }
            cx.end_turtle();
            // A Fill field gets one line. A wrapping Fit field can grow the line as it draws.
            cx.begin_turtle(Walk::fill_fit(), Layout::flow_overlay().with_align_y(0.5));
            let value = self.view.child(id!(value));
            if value.visible() {
                let line_height = self.view.walk.height.to_fixed().unwrap_or(self.stack_line_height);
                cx.turtle_mut().allocate_height(line_height);
                let mut walk = Walk { width: Size::fill(), abs_pos: None, ..value.walk(cx) };
                if walk.height.is_fit() && walk.min_height.is_none() {
                    walk.min_height = Some(FitBound::Abs(line_height));
                }
                self.draw_state.set(PropRowDraw::Value(walk));
            }
        }
        if let Some(PropRowDraw::Value(walk)) = self.draw_state.get() {
            self.view.child(id!(value)).draw_walk(cx, scope, walk)?;
        }
        cx.end_turtle();
        cx.end_turtle();
        self.draw_state.end();
        self.draw_frame(cx, scope, walk)
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
    #[rust]
    draw_state: DrawStateWrap<()>,
}

impl Widget for PropStack {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.draw_state.begin(cx, ()) {
            return self.view.draw_walk(cx, scope, walk);
        }
        // A down-flow stack gives each Fill row its inner width less that row's margins.
        let inner = cx.peek_walk_turtle(walk).size.x - self.view.layout.padding.width();
        let reserve = self.view.child_reserve();
        let mut stacked = false;
        self.fields.clear();
        for (id, row) in &self.view.children {
            if !row.visible() { continue; }
            if !stacked {
                if let Some(row) = row.borrow::<PropRow>() {
                    let walk = row.view.walk;
                    stacked = match in_flow_width(walk) {
                        Some(Size::Fill { min: None, max: None, .. }) => {
                            let reserved = reserve.filter(|(child, _)| child == id)
                                .map_or(0.0, |(_, inset)| inset.width());
                            row.needs_stack(cx, inner - walk.margin.width() - reserved)
                        }
                        Some(Size::Fixed(width)) => row.needs_stack(cx, width),
                        _ => false,
                    };
                }
            }
            let field = row.value_input(cx, ids!(value.num));
            if field.borrow().is_some_and(|field| field.visible()) {
                self.fields.push(field);
            }
        }
        for (_, row) in &self.view.children {
            if let Some(mut row) = row.borrow_mut::<PropRow>() {
                row.stack_layout = Some((cx.redraw_id(), stacked));
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
