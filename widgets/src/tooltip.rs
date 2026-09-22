use crate::makepad_draw::event::TouchUpdateEvent;

use crate::{label::*, makepad_derive_widget::*, makepad_draw::*, view::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.TooltipBase = #(Tooltip::register_widget(vm))

    mod.widgets.Tooltip = mod.widgets.TooltipBase{
        content := RoundedView{
            width: Fit
            height: Fit
            padding: 15

            draw_bg +: {
                color: #fff
                border_size: 1.0
                border_color: #D0D5DD
                radius: 2.
            }

            tooltip_label := Label{
                width: 270
                draw_text +: {
                    text_style: theme.font_regular{font_size: 9}
                    color: #000
                }
            }
        }
    }
}

/// Which side of its anchor a tooltip goes on. When the content doesn't fit there,
/// it flips to the opposite side if that has more room.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipPosition {
    Top,
    Bottom,
    Left,
    #[default]
    Right,
}

/// The widget rect a tooltip is placed against, and how.
#[derive(Clone, Copy, Debug)]
pub struct TooltipAnchor {
    pub rect: Rect,
    pub side: TooltipPosition,
    /// Space kept between the tooltip and the anchor, and from the edges of the available area.
    pub gap: f64,
    /// The content's width when nothing constrains it, so the tooltip can be drawn narrower
    /// (wrapping its text) when that doesn't fit on the chosen side. None keeps the content's own width.
    pub natural_width: Option<f64>,
}

/// Where an anchored tooltip's content landed in the latest draw.
#[derive(Clone, Copy, Debug)]
pub struct TooltipPlacement {
    pub rect: Rect,
    pub side: TooltipPosition,
}

/// An overlay that draws its `content` child above everything else, either at a fixed position
/// (`set_pos`) or next to an anchor rect (`show_anchored`), where it's laid out first and then
/// moved into place within the same draw so it never appears in the wrong spot.
#[derive(Script, Widget)]
pub struct Tooltip {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    #[rust]
    draw_list: Option<DrawList2d>,

    #[rust]
    opened: bool,

    /// Top-left of the content when shown via `set_pos`.
    #[rust]
    tooltip_pos: Vec2d,

    #[rust]
    anchor: Option<TooltipAnchor>,

    #[rust]
    placement: Option<TooltipPlacement>,
}

impl ScriptHook for Tooltip {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl Widget for Tooltip {
    fn visit_cancel(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) -> bool {
        self.opened && self.cancel_children_impl(visit)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.opened {
            return;
        }

        let content = self.view.widget(cx, ids!(content));
        content.handle_event(cx, event, scope);

        // Hide the tooltip if any kind of user interaction occurs (taps/clicks, drags, scrolls, etc).
        //
        // Typically you don't handle raw events, but we do so here because:
        // 1. We don't want to impact the way that hit handling occurs for other views.
        // 2. We don't care about the details of the hit, only the fact that it happened.
        match event {
            Event::BackPressed { .. }
            | Event::MouseDown(_)
            | Event::MouseUp(_) => {
                self.hide(cx);
            }
            // Fingers resting on a trackpad send zero-delta scrolls, which aren't an interaction.
            Event::Scroll(scroll) if scroll.scroll != Vec2d::default() => {
                self.hide(cx);
            }
            Event::TouchUpdate(TouchUpdateEvent { touches, .. }) => {
                if touches
                    .iter()
                    .any(|tp| matches!(tp.state, event::TouchState::Start))
                {
                    self.hide(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        // The overlay list has to begin every frame, even while hidden, or the window drops it.
        self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);

        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::default());
        self.placement = None;

        if self.opened {
            let content = self.view.widget(cx, ids!(content));
            let mut walk = content.walk(cx);
            let anchored = self.anchor.map(|anchor| {
                let insets = cx.display_context.safe_area_insets;
                let avail = Rect {
                    pos: dvec2(insets.left, insets.top),
                    size: dvec2(
                        (pass_size.x - insets.left - insets.right).max(0.0),
                        (pass_size.y - insets.top - insets.bottom).max(0.0),
                    ),
                };
                // The width decides how the text wraps, so it must be fixed before drawing.
                // The height is only known afterwards.
                if let Some(natural_width) = anchor.natural_width {
                    walk.width = Size::Fixed(Self::place(&anchor, avail, dvec2(natural_width, 0.0)).rect.size.x);
                }
                (anchor, avail)
            });
            walk.abs_pos = Some(if anchored.is_some() { Vec2d::default() } else { self.tooltip_pos });

            let start = cx.align_list_len();
            content.draw_walk_all(cx, scope, walk);
            if let Some((anchor, avail)) = anchored {
                let drawn = content.area().rect(cx);
                let placement = Self::place(&anchor, avail, drawn.size);
                cx.shift_align_range(
                    &TurtleAlignRange { start, end: cx.align_list_len() },
                    placement.rect.pos - drawn.pos,
                );
                self.placement = Some(placement);
            }
        }

        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);

        DrawStep::done()
    }

    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        self.label(cx, ids!(content.tooltip_label))
            .set_text(cx, text);
    }
}

impl Tooltip {
    /// Chooses the side and rect for content of the given `size` next to `anchor`, kept inside `avail`.
    /// The anchor's side is kept when the content fits there or when the opposite side has even less room.
    pub fn place(anchor: &TooltipAnchor, avail: Rect, size: Vec2d) -> TooltipPlacement {
        let a = anchor.rect;
        let gap = anchor.gap;
        let (left, top) = (avail.pos.x + gap, avail.pos.y + gap);
        let (right, bottom) = (avail.pos.x + avail.size.x - gap, avail.pos.y + avail.size.y - gap);
        let room_left = a.pos.x - gap - left;
        let room_right = right - (a.pos.x + a.size.x) - gap;
        let room_above = a.pos.y - gap - top;
        let room_below = bottom - (a.pos.y + a.size.y) - gap;

        let (needed, room, room_opposite, opposite) = match anchor.side {
            TooltipPosition::Top => (size.y, room_above, room_below, TooltipPosition::Bottom),
            TooltipPosition::Bottom => (size.y, room_below, room_above, TooltipPosition::Top),
            TooltipPosition::Left => (size.x, room_left, room_right, TooltipPosition::Right),
            TooltipPosition::Right => (size.x, room_right, room_left, TooltipPosition::Left),
        };
        let side = if needed <= room || room >= room_opposite { anchor.side } else { opposite };

        let width = match side {
            TooltipPosition::Left => size.x.min(room_left),
            TooltipPosition::Right => size.x.min(room_right),
            TooltipPosition::Top | TooltipPosition::Bottom => size.x.min(right - left),
        }.max(0.0);
        let center = a.center();
        let pos = match side {
            TooltipPosition::Top => dvec2(center.x - width * 0.5, a.pos.y - gap - size.y),
            TooltipPosition::Bottom => dvec2(center.x - width * 0.5, a.pos.y + a.size.y + gap),
            TooltipPosition::Left => dvec2(a.pos.x - gap - width, center.y - size.y * 0.5),
            TooltipPosition::Right => dvec2(a.pos.x + a.size.x + gap, center.y - size.y * 0.5),
        };
        // min before max, so content bigger than the available area pins to its top-left.
        let pos = dvec2(
            pos.x.min(right - width).max(left),
            pos.y.min(bottom - size.y).max(top),
        );
        TooltipPlacement { rect: Rect { pos, size: dvec2(width, size.y) }, side }
    }

    pub fn set_pos(&mut self, _cx: &mut Cx, pos: Vec2d) {
        self.tooltip_pos = pos;
        self.anchor = None;
    }

    pub fn anchor(&self) -> Option<TooltipAnchor> {
        self.anchor
    }

    pub fn anchor_mut(&mut self) -> Option<&mut TooltipAnchor> {
        self.anchor.as_mut()
    }

    /// Only set while the tooltip is shown anchored and has been drawn.
    pub fn placement(&self) -> Option<TooltipPlacement> {
        self.placement
    }

    pub fn show(&mut self, cx: &mut Cx) {
        self.opened = true;
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }

    pub fn show_with_options(&mut self, cx: &mut Cx, pos: Vec2d, text: &str) {
        self.set_text(cx, text);
        self.set_pos(cx, pos);
        self.show(cx);
    }

    /// Shows the content next to `anchor`, placing it once its drawn size is known.
    pub fn show_anchored(&mut self, cx: &mut Cx, anchor: TooltipAnchor) {
        self.anchor = Some(anchor);
        self.show(cx);
    }

    pub fn hide(&mut self, cx: &mut Cx) {
        self.opened = false;
        self.anchor = None;
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }
}

impl TooltipRef {
    pub fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, text);
        }
    }

    pub fn set_pos(&self, cx: &mut Cx, pos: Vec2d) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_pos(cx, pos);
        }
    }

    pub fn show(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show(cx);
        }
    }

    pub fn show_with_options(&self, cx: &mut Cx, pos: Vec2d, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_with_options(cx, pos, text);
        }
    }

    pub fn show_anchored(&self, cx: &mut Cx, anchor: TooltipAnchor) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_anchored(cx, anchor);
        }
    }

    pub fn hide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.hide(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { pos: dvec2(x, y), size: dvec2(w, h) }
    }

    fn anchor(side: TooltipPosition, rect: Rect, natural_width: f64) -> TooltipAnchor {
        TooltipAnchor { rect, side, gap: 0.0, natural_width: Some(natural_width) }
    }

    const SCREEN: Rect = Rect { pos: Vec2d { x: 0.0, y: 0.0 }, size: Vec2d { x: 400.0, y: 600.0 } };

    #[test]
    fn top_centers_on_anchor_when_it_fits() {
        // anchor spans x 180-220, so a 100 wide tooltip is centered at 150.
        let p = Tooltip::place(&anchor(TooltipPosition::Top, rect(180.0, 100.0, 40.0, 30.0), 100.0), SCREEN, dvec2(100.0, 50.0));
        assert_eq!(p.side, TooltipPosition::Top);
        assert_eq!(p.rect, rect(150.0, 50.0, 100.0, 50.0));
    }

    #[test]
    fn top_clamps_to_left_edge() {
        let p = Tooltip::place(&anchor(TooltipPosition::Top, rect(10.0, 100.0, 20.0, 30.0), 100.0), SCREEN, dvec2(100.0, 50.0));
        assert_eq!(p.rect.pos.x, 0.0);
        assert_eq!(p.rect.size.x, 100.0);
    }

    #[test]
    fn top_clamps_to_right_edge() {
        let p = Tooltip::place(&anchor(TooltipPosition::Top, rect(370.0, 100.0, 20.0, 30.0), 100.0), SCREEN, dvec2(100.0, 50.0));
        assert_eq!(p.rect.pos.x, 300.0);
        assert_eq!(p.rect.size.x, 100.0);
    }

    #[test]
    fn wider_than_available_pins_left_and_narrows() {
        let p = Tooltip::place(&anchor(TooltipPosition::Top, rect(350.0, 100.0, 30.0, 30.0), 600.0), SCREEN, dvec2(600.0, 50.0));
        assert_eq!(p.rect.pos.x, 0.0);
        assert_eq!(p.rect.size.x, 400.0);
    }

    #[test]
    fn safe_area_insets_bound_the_left_edge_and_width() {
        // landscape phone: 60px insets on both sides of an 800 wide screen.
        let avail = rect(60.0, 0.0, 680.0, 400.0);
        let p = Tooltip::place(&anchor(TooltipPosition::Top, rect(700.0, 350.0, 50.0, 50.0), 800.0), avail, dvec2(800.0, 80.0));
        assert_eq!(p.rect.pos.x, 60.0);
        assert_eq!(p.rect.size.x, 680.0);
    }

    #[test]
    fn safe_area_insets_bound_the_right_edge() {
        let avail = rect(60.0, 0.0, 680.0, 400.0);
        let p = Tooltip::place(&anchor(TooltipPosition::Top, rect(720.0, 200.0, 20.0, 20.0), 200.0), avail, dvec2(200.0, 60.0));
        assert_eq!(p.rect.pos.x, 540.0);
        assert_eq!(p.rect.size.x, 200.0);
    }

    #[test]
    fn top_flips_below_when_no_room_above() {
        let p = Tooltip::place(&anchor(TooltipPosition::Top, rect(180.0, 10.0, 40.0, 30.0), 100.0), SCREEN, dvec2(100.0, 50.0));
        assert_eq!(p.side, TooltipPosition::Bottom);
        assert_eq!(p.rect.pos.y, 40.0);
    }

    #[test]
    fn bottom_flips_above_when_no_room_below() {
        let p = Tooltip::place(&anchor(TooltipPosition::Bottom, rect(180.0, 560.0, 40.0, 30.0), 100.0), SCREEN, dvec2(100.0, 50.0));
        assert_eq!(p.side, TooltipPosition::Top);
        assert_eq!(p.rect.pos.y, 510.0);
    }

    #[test]
    fn right_flips_left_when_no_room_right() {
        let p = Tooltip::place(&anchor(TooltipPosition::Right, rect(350.0, 100.0, 40.0, 30.0), 100.0), SCREEN, dvec2(100.0, 50.0));
        assert_eq!(p.side, TooltipPosition::Left);
        assert_eq!(p.rect, rect(250.0, 90.0, 100.0, 50.0));
    }

    #[test]
    fn keeps_requested_side_when_it_has_more_room_even_if_too_narrow() {
        // 30 to the left, 330 to the right, content wants 500: stay right and wrap to 330.
        let p = Tooltip::place(&anchor(TooltipPosition::Right, rect(30.0, 100.0, 40.0, 30.0), 500.0), SCREEN, dvec2(500.0, 50.0));
        assert_eq!(p.side, TooltipPosition::Right);
        assert_eq!(p.rect.pos.x, 70.0);
        assert_eq!(p.rect.size.x, 330.0);
    }

    #[test]
    fn taller_than_available_pins_to_top() {
        let p = Tooltip::place(&anchor(TooltipPosition::Right, rect(30.0, 100.0, 40.0, 30.0), 100.0), SCREEN, dvec2(100.0, 700.0));
        assert_eq!(p.rect.pos.y, 0.0);
    }

    #[test]
    fn gap_keeps_the_tooltip_off_the_anchor_and_the_edges() {
        let mut a = anchor(TooltipPosition::Bottom, rect(180.0, 100.0, 40.0, 30.0), 100.0);
        a.gap = 4.0;
        let p = Tooltip::place(&a, SCREEN, dvec2(100.0, 50.0));
        assert_eq!(p.rect.pos, dvec2(150.0, 134.0));
        a.rect = rect(0.0, 100.0, 20.0, 30.0);
        let p = Tooltip::place(&a, SCREEN, dvec2(100.0, 50.0));
        assert_eq!(p.rect.pos.x, 4.0);
    }
}
