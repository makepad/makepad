//! FieldWell — the box a field's input sits in, which knows what the input
//! is doing.
//!
//! Eight places in this repository put a text input inside a wrapper so the
//! wrapper can draw the box: a search well with a magnifier, a page number
//! between two arrows, a labelled row with a unit after it. Every one of
//! them does the same thing first — blanks the input's own chrome, a dozen
//! lines of `color_hover`, `color_focus`, `border_color_down` and the rest
//! all set to one flat colour — so that two boxes are not drawn inside each
//! other.
//!
//! **And then four of them can never say they have the keyboard.** Once the
//! input's chrome is blanked, the state lives in a widget that is no longer
//! drawing anything, while the wrapper that IS drawing is a plain view with
//! no hover, focus or disabled instance to set. The field looks identical
//! whether or not you are typing in it.
//!
//! So this well draws the chrome AND carries the slot's state into it. It
//! asks the input each pass whether it holds the key focus and paints
//! accordingly; a press anywhere on the well — the padding, the icon, the
//! gap after the text — hands the focus to the input, because the whole box
//! is the target a person is aiming at; and one `disabled` reaches all
//! three slots rather than each caller remembering to set three — reaches,
//! not repaints: a slot honours the flag if it has a disabled state, and a
//! plain `Label` has none.
//!
//! **What it is not.** It carries no label and no message. The label half of
//! this card measured out at roughly seventy-five hand-rolled instances that
//! would each have to re-override a shared shell — the same finding that
//! made a shared splitter preset worthless — and the validation half has no
//! caller at all. A well with three slots is the piece that is actually
//! asked for eight times.

use crate::{makepad_derive_widget::*, makepad_draw::*, text_input::TextInputWidgetRefExt, widget::*, CxWidgetExt};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawFieldWellBase = #(DrawFieldWell::script_component(vm))
    set_type_default() do #(DrawFieldWell::script_shader(vm)){
        ..mod.draw.DrawQuad

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size
                self.border_size
                self.rect_size.x - self.border_size * 2.
                self.rect_size.y - self.border_size * 2.
                self.border_radius
            )
            sdf.fill_keep(
                self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_disabled, self.disabled)
            )
            sdf.stroke(
                self.border_color
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_disabled, self.disabled)
                self.border_size
            )
            return sdf.result
        }
    }

    mod.widgets.FieldWellBase = #(FieldWell::register_widget(vm))

    /** A text input with no chrome of its own, for a well to hold.
     *
     * The well draws the box, so the input must not draw a second one
     * inside it. Every caller doing this by hand writes the same dozen
     * lines — every colour and every border colour of every state, set to
     * one flat value — and that ritual is what this retires. */
    mod.widgets.WellInput = TextInput{
        width: Fill
        height: Fit
        // The well's padding is the box's padding. A second set inside it
        // pushes the text off the centre of a short field, and every caller
        // that noticed had to zero this at its own call site.
        padding: 0.0
        margin: 0.0
        draw_bg +: {
            border_radius: uniform(0.0)
            border_size: uniform(0.0)
            color: #00000000
            color_hover: uniform(#00000000)
            color_focus: uniform(#00000000)
            color_down: uniform(#00000000)
            color_empty: uniform(#00000000)
            color_disabled: uniform(#00000000)
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color: uniform(#00000000)
            border_color_hover: uniform(#00000000)
            border_color_focus: uniform(#00000000)
            border_color_down: uniform(#00000000)
            border_color_empty: uniform(#00000000)
            border_color_disabled: uniform(#00000000)
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
        }
    }

    /** The box a field's input sits in: it draws the chrome, and it says
     * what the input inside it is doing. */
    mod.widgets.FieldWell = set_type_default() do mod.widgets.FieldWellBase{
        width: Fill
        height: Fit
        flow: Right
        align: Align{y: 0.5}
        padding: theme.mspace_1{left: theme.space_2, right: theme.space_2}
        spacing: theme.space_1
        /** nothing in the well answers 0..1 step 1 */
        disabled: false

        /** The well itself: an inset SDF box with a bevel stroke.
         *
         * Every value here is plain, and every one has a field on the draw
         * struct behind it. No uniform(): a caller that dresses the well in
         * its own palette overrides these, and overriding a uniform on a
         * draw type that also carries Rust instance fields regenerates the
         * shader's value table out from under them - the box then read a
         * border thickness of garbage and drew as one flat slab of the
         * border colour. Plain values are per-draw-call data, so a caller
         * is only passing different numbers to the same shader. */
        draw_bg +: {
            hover: 0.0
            focus: 0.0
            disabled: 0.0

            /** bevel border thickness in pixels 0..4 step 0.5 */
            border_size: theme.beveling
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius

            color: theme.color_inset
            color_hover: theme.color_inset_hover
            color_focus: theme.color_inset_focus
            color_disabled: theme.color_inset_disabled

            border_color: theme.color_bevel
            border_color_hover: theme.color_bevel_hover
            border_color_focus: theme.color_bevel_focus
            border_color_disabled: theme.color_bevel_disabled
        }

        // Bare slots, not named instances: a slot takes a value, so a caller
        // writes `input: TextInput{}` and not `input := TextInput{}`.
        leading: View{width: Fit height: Fit}
        input: View{width: Fill height: Fit}
        trailing: View{width: Fit height: Fit}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFieldWell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    #[live]
    border_size: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_focus: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_color_hover: Vec4f,
    #[live]
    border_color_focus: Vec4f,
    #[live]
    border_color_disabled: Vec4f,
}

#[derive(Script, Widget)]
pub struct FieldWell {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    pub draw_bg: DrawFieldWell,

    /// Before the input: a magnifier, a currency mark, a unit.
    #[find]
    #[live]
    pub leading: WidgetRef,
    /// The input itself. The well exists for this one.
    #[find]
    #[live]
    pub input: WidgetRef,
    /// After it: a clear mark, a unit, a stepper.
    #[find]
    #[live]
    pub trailing: WidgetRef,

    #[live]
    pub disabled: bool,

    #[rust]
    hovered: bool,
}

impl ScriptHook for FieldWell {
    /// A `disabled: true` written in the DSL sets the field directly and
    /// never reaches `set_disabled`, so without this the box would dim
    /// while the affixes and the input inside it stayed bright — the exact
    /// three-places-to-remember this widget exists to retire.
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        if self.disabled {
            vm.with_cx_mut(|cx| {
                for slot in [&self.leading, &self.input, &self.trailing] {
                    slot.set_disabled(cx, true);
                }
            });
        }
    }
}

impl FieldWell {
    /// Whether the input in this well holds the keyboard.
    pub fn focused(&self, cx: &Cx) -> bool {
        cx.has_key_focus(self.input.area())
    }

    /// Hand the keyboard to the slot. The well is the target a person aims
    /// at, so a press on its padding belongs to the input inside it.
    ///
    /// `take_key_focus` rather than the bare `set_key_focus`: it forces the
    /// caret and the focus animator on. That matters here because the well
    /// hands focus to a field the pointer is NOT over, and because of the
    /// release below.
    pub fn focus_input(&self, cx: &mut Cx) {
        let input = self.input.as_text_input();
        if input.borrow().is_some() {
            input.take_key_focus(cx);
        } else {
            self.input.set_key_focus(cx);
        }
    }
}

impl Widget for FieldWell {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Asked every pass rather than tracked: the focus can leave for
        // reasons this well never hears about — another widget taking it,
        // a dialog opening — and a well that only listened would keep
        // claiming the keyboard it no longer has.
        let focused = self.focused(cx.cx.cx);
        self.draw_bg.focus = if focused { 1.0 } else { 0.0 };
        self.draw_bg.hover = if self.hovered && !self.disabled { 1.0 } else { 0.0 };
        self.draw_bg.disabled = if self.disabled { 1.0 } else { 0.0 };

        self.draw_bg.begin(cx, walk, self.layout);
        // The trailing slot is reserved BEFORE the input is drawn. Every
        // preset makes the input `width: Fill`, so drawn in order it takes
        // the whole row and the unit, the clear mark or the stepper this
        // slot exists for is laid out into nothing and never appears.
        // Deferring is how the slider keeps a label beside a filling text
        // box, and this is the same problem.
        let trailing_walk = self.trailing.walk(cx.cx.cx);
        let deferred = cx.defer_walk_turtle(trailing_walk);
        for (name, slot) in [
            (live_id!(leading), &mut self.leading),
            (live_id!(input), &mut self.input),
        ] {
            // The slots are drawn here rather than by a container, so nothing
            // else puts them in the tree: without this a host could not reach
            // ids!(well.input) at all, and the app that tried wired its Enter
            // handler to an empty ref and never moved a page.
            cx.widget_tree_insert_child(self.uid, name, slot.clone());
            let slot_walk = slot.walk(cx.cx.cx);
            let _ = slot.draw_walk(cx, scope, slot_walk);
        }
        cx.widget_tree_insert_child(self.uid, live_id!(trailing), self.trailing.clone());
        let trailing_walk = match deferred {
            Some(mut dw) => dw.resolve(cx),
            None => self.trailing.walk(cx.cx.cx),
        };
        let _ = self.trailing.draw_walk(cx, scope, trailing_walk);
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for slot in [&self.leading, &self.input, &self.trailing] {
            slot.handle_event(cx, event, scope);
        }
        if self.disabled {
            return;
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                if !self.hovered {
                    self.hovered = true;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hovered {
                    self.hovered = false;
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(_) => {
                // The padding and the gap after the text belong to the
                // input: aiming at the box and getting nothing is the
                // commonest way a field feels broken.
                self.focus_input(cx);
                self.redraw(cx);
            }
            Hit::FingerUp(fe) if !fe.cancelled => {
                // Not redundant with the press. A focused TextInput drops
                // its own focus on a mouse-up outside its rect — a rule
                // that is right for a bare field and wrong for one in a
                // well, because the well's padding is outside that rect
                // and is still the field's target. The slot runs before
                // this arm and clears the focus the press just gave it, so
                // the well takes it back; and it must be `take_key_focus`,
                // since the focus never changed as far as `Cx` is
                // concerned and no second `Hit::KeyFocus` will arrive to
                // light the caret again.
                self.focus_input(cx);
            }
            _ => {}
        }
    }

    /// The slot's text, so a host reads the well the way it read the input.
    fn text(&self) -> String {
        self.input.text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.input.set_text(cx, v);
    }

    /// One `disabled` reaches all three slots, rather than each caller
    /// remembering to set three.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            for slot in [&self.leading, &self.input, &self.trailing] {
                slot.set_disabled(cx, disabled);
            }
            self.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }
}

impl FieldWellRef {
    /// Whether the input in this well holds the keyboard.
    pub fn focused(&self, cx: &Cx) -> bool {
        self.borrow().map(|inner| inner.focused(cx)).unwrap_or(false)
    }

    /// Hand the keyboard to the slot.
    pub fn focus_input(&self, cx: &mut Cx) {
        if let Some(inner) = self.borrow() {
            inner.focus_input(cx);
        }
    }

    /// The input inside, for a caller that needs the real widget.
    pub fn input(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.input.clone()).unwrap_or_default()
    }
}
