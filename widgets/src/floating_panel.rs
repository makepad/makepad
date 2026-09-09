//! FloatingPanel — a panel the person moves and sizes, that does not stop
//! the work.
//!
//! That last clause is the whole difference from a dialog, and it decides
//! every structural choice here. A dialog asks a question and is owed an
//! answer, so it paints a scrim, takes the keyboard, blocks scrolling and
//! goes away when you press outside it. An inspector, a palette, a set of
//! readings you keep an eye on while you work is none of those things: it
//! stays where you put it, the page underneath keeps working, and clicking
//! elsewhere is not a dismissal. So this derefs `View` and **not** `Modal` —
//! every one of Modal's behaviours would be wrong here.
//!
//! **The title bar is the handle and the bottom-right corner is the grip.**
//! Both are read from what was actually drawn rather than guessed from
//! padding, and both claim a press before the panel's own contents see it:
//! a drag that begins by dropping a caret into a text field is a drag the
//! user has to undo.
//!
//! **It cannot be dragged off the screen.** The panel is pulled back inside
//! the pass every draw with `span_inboard`, the same arithmetic every
//! anchored popup already uses. A panel whose title bar is off the top edge
//! cannot be moved back, which makes losing it permanent.
//!
//! Promoted from the design overlay's note card, which has carried this
//! mechanism for months. What stayed behind is everything particular to
//! that overlay: its notes file, its session singleton, its leader line and
//! its own chrome. What came across is the ~80 lines that are the panel.

use crate::{
    button::ButtonWidgetRefExt, label::LabelWidgetRefExt, makepad_derive_widget::*,
    makepad_draw::*, overlay_place::span_inboard, view::View, widget::*,
};

/// What a panel reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum FloatingPanelAction {
    /// The close mark was pressed.
    Closed,
    /// The panel was moved or sized, and where it now is.
    Placed { pos: Vec2d, size: Vec2d },
    #[default]
    None,
}

/// What the pointer is doing to the panel.
#[derive(Clone, Copy, Debug)]
enum Handling {
    /// Moving it: where the pointer took hold, and where the panel was.
    Move { held_at: Vec2d, from: Vec2d },
    /// Sizing it: where the pointer took hold, and how big it was.
    Size { held_at: Vec2d, from: Vec2d },
}

/// How big the corner grip is, in layout points.
const GRIP: f64 = 14.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FloatingPanelBase = #(FloatingPanel::register_widget(vm))

    /** A panel the person moves and sizes, that does not stop the work:
     * an inspector, a palette, a set of readings kept in view. */
    mod.widgets.FloatingPanel = set_type_default() do mod.widgets.FloatingPanelBase{
        flow: Overlay
        /** where it opens, in window points */
        pos: vec2(80., 80.)
        /** how big it opens */
        size: vec2(320., 240.)
        /** it never gets smaller than this: the title row needs the width */
        min_size: vec2(160., 96.)
        /** the title bar moves it 0..1 step 1 */
        movable: true
        /** the bottom-right corner sizes it 0..1 step 1 */
        resizable: true
        /** the title, also what `text()` answers */
        title: ""
        /** a close mark on the title bar 0..1 step 1 */
        closable: true

        content := RoundedShadowView{
            width: Fill
            height: Fill
            flow: Down
            show_bg: true
            draw_bg +: {
                color: theme.color_surface_container_high
                border_color: theme.color_outline
                border_size: 1.0
                border_radius: theme.radius_l
                shadow_color: theme.color_elevation_4
                shadow_radius: uniform(theme.elevation_4_radius)
                shadow_offset: uniform(vec2(0., theme.elevation_4_offset_y))
            }

            header := View{
                width: Fill
                height: Fit
                flow: Right
                align: Align{y: 0.5}
                padding: Inset{left: 14. right: 6. top: 8. bottom: 8.}
                spacing: theme.space_2
                title_label := Label{
                    width: Fill
                    draw_text +: {
                        text_style: theme.font_title_s
                        color: theme.color_on_surface
                    }
                }
                close := ButtonFlat{
                    width: 22.
                    height: 22.
                    text: "\u{00d7}"
                }
            }

            body := View{
                width: Fill
                height: Fill
                flow: Down
                spacing: theme.space_2
                padding: Inset{left: 14. right: 14. top: 0. bottom: 12.}
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
            }
        }
    }
}

#[derive(Script, Widget)]
pub struct FloatingPanel {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    #[rust]
    draw_list: Option<DrawList2d>,

    #[live(Vec2d { x: 80., y: 80. })]
    pub pos: Vec2d,
    #[live(Vec2d { x: 320., y: 240. })]
    pub size: Vec2d,
    #[live(Vec2d { x: 160., y: 96. })]
    pub min_size: Vec2d,
    #[live(true)]
    pub movable: bool,
    #[live(true)]
    pub resizable: bool,
    #[live]
    pub title: String,
    #[live(true)]
    pub closable: bool,

    #[rust]
    open: bool,
    /// Whether the chrome has been written from the props this open.
    #[rust]
    dressed: bool,
    #[rust]
    handling: Option<Handling>,
    /// The title bar and the close mark as they were last drawn. Read from
    /// the draw rather than computed from padding, so restyling the header
    /// cannot quietly move the handle out from under the pointer.
    #[rust]
    band: Rect,
    #[rust]
    close_mark: Rect,
}

impl ScriptHook for FloatingPanel {
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
        // The panel takes NO room in the layout that holds it: it paints on
        // its own overlay list against a root turtle for the pass. Reporting
        // `Fill` upward would make it a deferred fill and hand it a share of
        // its parent's spare height for a panel that may not even be open —
        // the bug spelled out in `modal.rs`, forced closed here for the same
        // reason it is forced closed there.
        self.view.walk = Walk::empty();
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl FloatingPanel {
    pub fn open(&mut self, cx: &mut Cx) {
        if !self.open {
            self.open = true;
            self.dressed = false;
            self.handling = None;
            self.redraw(cx);
        }
    }

    pub fn close(&mut self, cx: &mut Cx) {
        if self.open {
            self.open = false;
            self.handling = None;
            self.redraw(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Where the panel is and how big, in window points.
    pub fn placement(&self) -> (Vec2d, Vec2d) {
        (self.pos, self.size)
    }

    /// Put the panel somewhere. The caller keeps the value if it wants it
    /// back next run: a widget that writes files is a widget that has
    /// learned something it has no business knowing.
    pub fn place(&mut self, cx: &mut Cx, pos: Vec2d, size: Vec2d) {
        self.pos = pos;
        self.size = size;
        self.redraw(cx);
    }

    fn dress(&mut self, cx: &mut Cx) {
        let content = self.view.widget(cx, ids!(content));
        content.label(cx, ids!(title_label)).set_text(cx, &self.title);
        content.widget(cx, ids!(close)).set_visible(cx, self.closable);
    }

    fn say_placed(&mut self, cx: &mut Cx) {
        let uid = self.widget_uid();
        let (pos, size) = (self.pos, self.size);
        cx.widget_action(uid, FloatingPanelAction::Placed { pos, size });
    }
}

impl Widget for FloatingPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(self.view.layout);

        if self.open {
            if !self.dressed {
                self.dressed = true;
                self.dress(cx.cx.cx);
            }
            // Never off the screen. A panel whose title bar has gone past an
            // edge cannot be dragged back, so losing it would be permanent.
            let pass = cx.current_pass_size();
            self.size.x = self.size.x.max(self.min_size.x).min(pass.x.max(self.min_size.x));
            self.size.y = self.size.y.max(self.min_size.y).min(pass.y.max(self.min_size.y));
            self.pos.x = span_inboard(self.pos.x, self.size.x, 0.0, pass.x);
            self.pos.y = span_inboard(self.pos.y, self.size.y, 0.0, pass.y);

            let content = self.view.widget(cx.cx.cx, ids!(content));
            let mut view = content.borrow_mut::<crate::view::View>();
            if let Some(view) = view.as_mut() {
                view.walk = Walk {
                    width: Size::Fixed(self.size.x),
                    height: Size::Fixed(self.size.y),
                    abs_pos: Some(self.pos),
                    ..Walk::default()
                };
            }
            drop(view);
            let _ = content.draw_all(cx, scope);

            // The handle and the mark, as drawn.
            self.band = content.widget(cx.cx.cx, ids!(header)).area().rect(cx.cx.cx);
            self.close_mark = content.widget(cx.cx.cx, ids!(close)).area().rect(cx.cx.cx);
        }

        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.open {
            return;
        }
        let grip = Rect {
            pos: dvec2(self.pos.x + self.size.x - GRIP, self.pos.y + self.size.y - GRIP),
            size: dvec2(GRIP, GRIP),
        };
        let panel = Rect { pos: self.pos, size: self.size };

        // The handle claims a press BEFORE the panel's own contents see it.
        // A drag that starts by dropping a caret into a field underneath is
        // a drag the person then has to undo.
        match event {
            Event::MouseDown(me) if self.handling.is_none() => {
                if self.resizable && grip.contains(me.abs) {
                    self.handling = Some(Handling::Size { held_at: me.abs, from: self.size });
                    return;
                }
                if self.movable && self.band.contains(me.abs) && !self.close_mark.contains(me.abs) {
                    self.handling = Some(Handling::Move { held_at: me.abs, from: self.pos });
                    return;
                }
            }
            Event::MouseMove(me) => {
                match self.handling {
                    Some(Handling::Move { held_at, from }) => {
                        self.pos = from + (me.abs - held_at);
                        cx.set_cursor(MouseCursor::Move);
                        self.redraw(cx);
                        return;
                    }
                    Some(Handling::Size { held_at, from }) => {
                        let to = from + (me.abs - held_at);
                        self.size = dvec2(to.x.max(self.min_size.x), to.y.max(self.min_size.y));
                        cx.set_cursor(MouseCursor::NwseResize);
                        self.redraw(cx);
                        return;
                    }
                    None => {
                        // The corner says what it does before it is grabbed.
                        if self.resizable && grip.contains(me.abs) {
                            cx.set_cursor(MouseCursor::NwseResize);
                        }
                    }
                }
            }
            Event::MouseUp(_) => {
                if self.handling.take().is_some() {
                    self.say_placed(cx);
                    self.redraw(cx);
                    return;
                }
            }
            _ => {}
        }

        self.view.widget(cx, ids!(content)).handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            let content = self.view.widget(cx, ids!(content));
            if content.widget(cx, ids!(close)).as_button().clicked(actions) {
                let uid = self.widget_uid();
                self.close(cx);
                cx.widget_action(uid, FloatingPanelAction::Closed);
            }
        }
        // A press anywhere on the panel is the panel's, so the page beneath
        // it does not also act on it. Outside the panel nothing is claimed —
        // that is what "does not stop the work" means.
        if let Event::MouseDown(me) = event {
            if panel.contains(me.abs) {
                cx.set_key_focus(self.view.area());
            }
        }
    }

    fn text(&self) -> String {
        self.title.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.title != v {
            self.title = v.to_string();
            self.dressed = false;
            self.redraw(cx);
        }
    }
}

impl FloatingPanelRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    /// Where it is and how big, for a caller that wants to put it back.
    pub fn placement(&self) -> Option<(Vec2d, Vec2d)> {
        self.borrow().map(|inner| inner.placement())
    }

    pub fn place(&self, cx: &mut Cx, pos: Vec2d, size: Vec2d) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.place(cx, pos, size);
        }
    }

    /// Whether the close mark was pressed this pass.
    pub fn closed(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|a| matches!(a.cast::<FloatingPanelAction>(), FloatingPanelAction::Closed))
            .unwrap_or(false)
    }

    /// Where a drag just left it, if one did.
    pub fn placed(&self, actions: &Actions) -> Option<(Vec2d, Vec2d)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<FloatingPanelAction>() {
            FloatingPanelAction::Placed { pos, size } => Some((pos, size)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel is pulled back inside the pass, so one dragged at the edge
    /// cannot end up somewhere it can never be dragged back from.
    #[test]
    fn a_panel_cannot_be_left_off_the_screen() {
        // Off the right: pulled back so its right edge sits on the edge.
        assert_eq!(span_inboard(900.0, 320.0, 0.0, 1000.0), 680.0);
        // Off the left: the low edge wins.
        assert_eq!(span_inboard(-50.0, 320.0, 0.0, 1000.0), 0.0);
        // Already inside: untouched.
        assert_eq!(span_inboard(100.0, 320.0, 0.0, 1000.0), 100.0);
    }

    /// A panel wider than the window keeps its low edge on screen rather
    /// than being pushed off the other side to make its width fit.
    #[test]
    fn a_panel_wider_than_the_window_keeps_its_near_edge() {
        assert_eq!(span_inboard(40.0, 1200.0, 0.0, 1000.0), 0.0);
    }
}
