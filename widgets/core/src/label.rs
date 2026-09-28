use crate::{
    makepad_derive_widget::*, makepad_draw::shader::draw_text::TextOverflow, makepad_draw::*,
    widget::*, widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.LabelBase = #(Label::register_widget(vm))
    mod.widgets.Label = set_type_default() do mod.widgets.LabelBase{
        width: Fit
        height: Fit
        padding: theme.mspace_1

        // The compact face this wears when its row runs out of width. Declared
        // with `:=` so it lands in the vec: a widget proto is frozen VALIDATED,
        // so a key its props do not list is a hard error at construction -- but
        // the checked path looks in the vec first, and declaring it once here
        // makes `tight: {...}` legal on every instance and every preset below.
        /** the face this wears when its row runs out of width */
        tight := {}

        draw_text +: {
            // A label is a box with text in it, and the boxes apps put labels
            // in are centered by their align, not by their baselines: center
            // the ink, so a label reads as centered when its parent says it is.
            ink_centered: true

            color_dither: uniform(1.0)
            color: theme.color_label_outer
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            gradient_fill_horizontal: uniform(0.0)

            get_color: fn() {
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                let mut color_2 = self.color_2

                let mut gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                if (self.color_2.x < -0.5) {
                    color_2 = self.color
                }

                return mix(self.color, color_2, gradient_fill_dir)
            }
            text_style: theme.font_regular{
                line_spacing: theme.font_wdgt_line_spacing
            }
        }
    }

    mod.widgets.Labelbold = mod.widgets.Label{
        draw_text +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
            }
        }
    }

    mod.widgets.LabelGradientX = mod.widgets.Label{
        width: Fit
        height: Fit
        draw_text +: {
            color: #f00
            color_2: #ff0
            gradient_fill_horizontal: 1.0
        }
    }

    mod.widgets.LabelGradientY = mod.widgets.Label{
        draw_text +: {
            color: #f00
            color_2: #ff0
        }
    }

    mod.widgets.TextBox = mod.widgets.Label{
        width: Fill
        height: Fit
        padding: Inset{left: 0., right: 0., top: theme.space_1, bottom: 0.}
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{
                line_spacing: theme.font_longform_line_spacing
                font_size: theme.font_size_p
            }
        }
        text: "TextBox"
    }

    mod.widgets.H1 = mod.widgets.Label{
        width: Fill
        padding: 0.
        draw_text +: {
            color: theme.color_text_hl
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_1
            }
        }
        text: "H1"
    }

    mod.widgets.H1italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_1
            }
        }
        text: "H1 italic"
    }

    mod.widgets.H2 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_2
            }
        }
        text: "H2"
    }

    mod.widgets.H2italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_2
            }
        }
        text: "H2 italic"
    }

    mod.widgets.H3 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_3
            }
        }
        text: "H3"
    }

    mod.widgets.H3italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_3
            }
        }
        text: "H3 italic"
    }

    mod.widgets.H4 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_4
            }
        }
        text: "H4"
    }

    mod.widgets.H4italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_4
            }
        }
        text: "H4 italic"
    }

    mod.widgets.H5 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_5
            }
        }
        text: "H5"
    }

    mod.widgets.H5italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_5
            }
        }
        text: "H5 italic"
    }

    mod.widgets.H6 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_6
            }
        }
        text: "H6"
    }

    mod.widgets.H6italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_6
            }
        }
        text: "H6 italic"
    }

    mod.widgets.P = mod.widgets.TextBox{
        text: "Paragraph"
    }

    mod.widgets.Pbold = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.Pitalic = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_italic{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.Pbolditalic = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_bold_italic{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.IconSet = mod.widgets.Label{
        width: Fit
        draw_text +: {
            // An icon font's cap height describes a capital nobody is drawing:
            // these glyphs are pictures placed in their own box, so leave them
            // on the baseline the font asks for.
            ink_centered: false
            text_style: theme.font_icons{
                line_spacing: theme.font_wdgt_line_spacing
                font_size: 100.
            }
            color: theme.color_text
        }
        text: "Car"
    }
}

#[derive(Clone, Debug, Default)]
pub enum LabelAction {
    HoverIn(Rect),
    HoverOut,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Label {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    pub draw_text: DrawText,

    #[walk]
    pub walk: Walk,
    #[live]
    pub align: Align,
    #[live(Flow::right_wrap())]
    flow: Flow,
    #[live]
    padding: Inset,

    /// Maximum number of lines to display. 0 means unlimited (default).
    /// Combined with `text_overflow: Ellipsis`, truncated text shows "…".
    #[live(0usize)]
    pub max_lines: usize,
    /// Controls how text overflow is handled when text exceeds the container.
    #[live]
    pub text_overflow: TextOverflow,

    #[rust]
    area: Area,
    #[live]
    text: ArcStringMut,

    #[live(true)]
    #[visible]
    visible: bool,

    /// Report where the pointer is over this label, for a tooltip to hang
    /// off. Off by default: it is one more widget asking about every mouse
    /// move, and most labels have nothing to say about it.
    ///
    /// It does NOT consume presses. A label reports the pointer; it never
    /// owns it. See `handle_event`.
    #[live(false)]
    hover_actions_enabled: bool,
}

impl Widget for Label {
    /// What this label would be worth on a row, with `over` in force.
    ///
    /// A label that can wrap answers `None` unless its width is stated: where a
    /// wrapping label breaks is the turtle's business, so its own width is not a
    /// number it can give.
    fn measure_width(
        &mut self,
        cx: &mut Cx2d,
        over: Option<&crate::width_override::WidthOverride>,
    ) -> Option<f64> {
        if over.is_some_and(|o| o.opaque) {
            return None;
        }
        // A block that states `visible` states it: the child may be wearing the
        // opposite right now, and a price taken off what it is wearing is a
        // price of the concession rather than of the face being priced.
        let visible = over.and_then(|o| o.visible).unwrap_or(self.visible);
        if !visible {
            return Some(0.0);
        }

        let margin = over.and_then(|o| o.margin).unwrap_or(self.walk.margin);
        let width = over.and_then(|o| o.width).unwrap_or(self.walk.width);
        if let Size::Fixed(w) = width {
            return Some(w + margin.width());
        }
        // A Fill takes what the others leave, so what it adds to the row's own
        // width is its minimum -- nothing, for a plain spacer. Answering None
        // here would make a row with a spacer in it unpriceable, which is most
        // rows.
        if let Size::Fill { min, .. } = width {
            return Some(min.unwrap_or(0.0) + margin.width());
        }
        // Only a Fit label has a width of its own to report.
        if !width.is_fit() {
            return None;
        }

        let pad = over.and_then(|o| o.padding).unwrap_or(self.padding);
        let text: &str = match over.and_then(|o| o.text.as_deref()) {
            Some(t) => t,
            None => self.text.as_ref(),
        };
        // An empty label draws a space: `draw_walk` writes one in before it
        // draws. Priced as nothing, it is a label a space narrower than it
        // draws for as long as it stays empty -- which is only until its
        // first draw, and then again every time somebody sets it empty. An
        // app does that on every rebuild, so a row on its edge took that
        // space as room and gave a concession back for exactly one frame.
        let text = if text.is_empty() { " " } else { text };
        let text_w = crate::badge::advance(&self.draw_text, cx, text);
        Some(pad.width() + text_w + margin.width())
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(text) {
            let str_val = vm.bx.heap.new_string_from_str(self.text.as_ref());
            return ScriptAsyncResult::Return(str_val.into());
        }
        if method == live_id!(set_text) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    if let Some(new_text) = vm
                        .bx
                        .heap
                        .cast_to_owned_string(value, "copying label text")
                    {
                        vm.with_cx_mut(|cx| {
                            self.set_text(cx, &new_text);
                        });
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }

        let walk = cx.resolve_walk(
            walk.with_add_padding(self.padding),
            ResolveAt::BeforeBegin,
        );
        cx.begin_turtle(
            walk,
            Layout {
                flow: self.flow,
                ..Default::default()
            },
        );
        // here we need to check if the text is empty, if so we need to set it to a space
        // or the text draw will not work(seems like lazy drawtext bug)
        let _ = self.text.as_ref().is_empty().then(|| {
            let _ = self.set_text(cx, " ");
        });
        self.draw_text.max_lines = self.max_lines;
        self.draw_text.text_overflow = self.text_overflow;
        self.draw_text
            .draw_walk(cx, walk, self.align, self.text.as_ref());
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        // Identical text produces identical visuals, so skip the string
        // rebuild and the redraw that would dirty the whole draw list.
        if self.text.as_ref() == v {
            return;
        }
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }

        let uid = self.widget_uid();

        // A label REPORTS where the pointer is. It never takes it.
        //
        // This asked with `capture_overload`, which buys a hover nothing at
        // all -- `hits` reads that flag only on a press -- while costing the
        // label a co-capture of every press that landed on it, and the
        // `handled` mark of the widget that really took that press along with
        // it. Under the pointer-capture rule a capture is a claim on the
        // pointer, and a host that sees one stands its own gesture down: a
        // plain row of text with hover actions on was enough to stop a
        // drag-to-scroll dead.
        //
        // Hovers leave `hits` on these two events and no others, so asking on
        // them alone reports exactly what it reported before and takes
        // nothing. There is no press path left to take anything on.
        if self.hover_actions_enabled && matches!(event, Event::MouseMove(_) | Event::MouseLeave(_))
        {
            match event.hits(cx, self.area) {
                Hit::FingerHoverIn(fh) => {
                    cx.widget_action(uid, LabelAction::HoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, LabelAction::HoverOut);
                }
                _ => (),
            }
        }
    }
}

impl Label {
    /// Sets the text color.
    ///
    /// Does nothing if the color is unchanged.
    pub fn set_text_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.draw_text.color == color {
            return;
        }
        self.draw_text.color = color;
        self.redraw(cx);
    }
}

impl LabelRef {
    pub fn text(&self) -> String {
        if let Some(inner) = self.borrow() {
            inner.text()
        } else {
            String::new()
        }
    }

    pub fn set_text(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, text);
        }
    }

    /// See [`Label::set_text_color()`].
    pub fn set_text_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text_color(cx, color);
        }
    }

    pub fn hover_in(&self, actions: &Actions) -> Option<Rect> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                LabelAction::HoverIn(rect) => Some(rect),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn hover_out(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                LabelAction::HoverOut => true,
                _ => false,
            }
        } else {
            false
        }
    }

    pub fn set_text_with<F: FnOnce(&mut String)>(&self, f: F) {
        if let Some(mut inner) = self.borrow_mut() {
            f(inner.text.as_mut())
        }
    }
}

/// THE POINTER-CAPTURE RULE, as it applies to a label.
///
/// A label with hover actions on REPORTS where the pointer is. It must not
/// take the pointer to do it: a capture is a claim, and every host that drags,
/// pans or scrolls stands its own gesture down while something else holds the
/// mouse. A row of text that quietly captured every press on it was enough to
/// stop a list scrolling.
#[cfg(test)]
mod pointer_capture_tests {
    #![allow(dead_code)]
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);

    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx) }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: Vec2d) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn mouse_move(abs: Vec2d) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 0.02,
            handled: Cell::new(Area::Empty),
        })
    }

    fn send(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()))
    }

    fn middle(cx: &Cx, widget: &WidgetRef) -> Vec2d {
        let rect = widget.area().rect(cx);
        assert!(rect.size.x > 0.0 && rect.size.y > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    note := Label{
                        width: 200.
                        height: 40.
                        text: "hover me"
                        hover_actions_enabled: true
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let note = root.widget(cx, ids!(note));
        (root, note)
    }

    fn hovered(actions: &Actions, note: &WidgetRef) -> bool {
        actions
            .iter()
            .filter_map(|a| a.as_widget_action())
            .any(|a| {
                a.widget_uid == note.widget_uid()
                    && matches!(a.cast::<LabelAction>(), LabelAction::HoverIn(_))
            })
    }

    /// The control. Without this the test below would pass on a label that
    /// had stopped reporting hovers altogether.
    #[test]
    fn a_label_with_hover_actions_on_still_reports_the_hover() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, note) = start(&mut cx);
        let at = middle(&cx, &note);
        let actions = send(&mut cx, &root, &mouse_move(at));
        assert!(hovered(&actions, &note), "the pointer arriving is reported");
    }

    /// The bug: reporting a hover used to cost a co-capture of every press,
    /// which under the pointer-capture rule tells every host around it to
    /// stand its own gesture down.
    #[test]
    fn a_press_on_a_label_takes_no_hold_on_the_pointer() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, note) = start(&mut cx);
        let at = middle(&cx, &note);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let event = press(at);
        send(&mut cx, &root, &event);
        assert!(
            !cx.fingers.any_areas_captured(),
            "a label reports the pointer; it never holds it"
        );
        let Event::MouseDown(e) = &event else { unreachable!() };
        assert!(
            e.handled.get().is_empty(),
            "and it does not claim the press out from under whoever the press was for"
        );
        cx.fingers.first_mouse_button = None;
    }
}
