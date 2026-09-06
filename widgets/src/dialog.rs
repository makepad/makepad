//! Dialog — the overlay that stops the work and asks for an answer.
//!
//! A dialog is the opposite of a toast, and the difference is the whole
//! design. A toast reports and expects to be ignored; a dialog interrupts
//! and expects to be answered, so it takes the pointer, takes the keyboard,
//! dims what is behind it, and does not go away by itself. Everything a
//! dialog costs the person using the app is justified only by there being a
//! question that genuinely cannot wait, which is why the library gives the
//! shape a name rather than leaving every app to assemble one out of a
//! modal and some views.
//!
//! **Three slots, and a rule about the third.** A dialog is a `title`, a
//! `body` and a row of actions. The actions are pinned to the bottom and
//! never scroll, because a long body that pushes its buttons off the screen
//! is how a person ends up unable to answer the question they were stopped
//! for. The body scrolls; the actions do not.
//!
//! **The default action and the way out.** Return presses the dialog's
//! default action, Escape leaves without answering, and both are the same
//! promise: a dialog can always be dismissed by keyboard alone. A dialog
//! that is not `dismissable` still answers Return, because a question with
//! no way out at all is a trap, and the library will not help build one.
//! When it closes, the key focus goes back where it came from.
//!
//! **Destructive answers are drawn as such.** `destructive` puts the error
//! role on the confirming action, and moves it away from the edge the hand
//! rests on, because "delete everything" and "cancel" a few pixels apart is
//! a design that will eventually delete everything.
//!
//! **Nesting.** A dialog is an overlay like any other: it holds its level of
//! the scroll block, and takes Escape through the shared claim, so a
//! popover or a menu opened from inside it closes first and one press
//! closes one thing.

use crate::{
    button::ButtonWidgetRefExt,
    label::LabelWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    modal::Modal,
    overlay_place::claim_escape,
    widget::*,
};

/// How wide a dialog is. The rungs are the questions people ask: a
/// confirmation is small, a form is medium, a preview is large.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum DialogSize {
    /// A yes or no question.
    Xs = 0,
    /// A short form.
    #[pick]
    Sm = 1,
    /// A longer form.
    Md = 2,
    /// A form beside something to look at.
    Lg = 3,
    /// Nearly the window.
    Xl = 4,
    /// The window, for a task that has taken over.
    Full = 5,
}

impl DialogSize {
    /// The width in layout points, or `None` for one that takes the room
    /// it is given.
    pub fn width(self) -> Option<f64> {
        match self {
            DialogSize::Xs => Some(320.0),
            DialogSize::Sm => Some(420.0),
            DialogSize::Md => Some(560.0),
            DialogSize::Lg => Some(720.0),
            DialogSize::Xl => Some(920.0),
            DialogSize::Full => None,
        }
    }
}

/// What a dialog reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum DialogAction {
    /// The default action was taken, by its button or by Return.
    Confirmed,
    /// The other action was taken.
    Cancelled,
    /// The dialog was left without an answer: Escape, the close mark, or a
    /// press outside it.
    Dismissed,
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.DialogSize = set_type_default() do #(DialogSize::script_api(vm))
    mod.widgets.splat(mod.widgets.DialogSize)

    use mod.widgets.*

    mod.widgets.DialogBase = #(Dialog::register_widget(vm))
    /** A question that stops the work: a title, a body that scrolls, and a
     * row of answers that does not. */
    mod.widgets.Dialog = set_type_default() do mod.widgets.DialogBase{
        // A widget that derefs another does not inherit its DSL, so the
        // modal's own chrome is spelled out here: the overlay flow that
        // centres the card in the pass, the transparent quad the modal
        // begins with, and the scrim behind it.
        flow: Overlay
        align: Center
        draw_bg +: {
            pixel: fn() {
                return vec4(0. 0. 0. 0.0)
            }
        }
        bg_view := View{
            width: Fill
            height: Fill
            show_bg: true
            draw_bg +: {
                // The scrim is two tokens, not one: `color_scrim` is the
                // colour and `state_scrim_opacity` is how much of it. The
                // colour alone is opaque black, which would hide the page
                // rather than dim it.
                color: uniform(theme.color_scrim)
                dim: uniform(theme.state_scrim_opacity)
                pixel: fn() {
                    return vec4(self.color.xyz, self.color.a * self.dim)
                }
            }
        }

        /** how wide: Xs Sm Md Lg Xl Full */
        size: Sm
        /** Escape, the close mark and a press outside all leave without answering */
        dismissable: true
        // The modal underneath swallows presses on its scrim whatever this
        // says; what it must NOT do is close itself, because it would raise
        // its own action on this same widget uid and a lookup by uid answers
        // with whichever came first. The dialog closes itself instead.
        can_dismiss: false
        /** the confirming answer destroys something: it takes the error role */
        destructive: false
        /** the title, also what `text()` answers */
        title: ""
        /** the label on the answer Return takes */
        confirm_text: "OK"
        /** the label on the other answer; empty leaves it out */
        cancel_text: ""

        content := RoundedShadowView{
            width: 420.
            height: Fit
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
                padding: Inset{left: 20. right: 12. top: 16. bottom: 8.}
                spacing: theme.space_2
                title_label := Label{
                    width: Fill
                    draw_text +: {
                        text_style: theme.font_title_s
                        color: theme.color_on_surface
                    }
                }
                close := ButtonFlat{
                    width: 24.
                    height: 24.
                    text: "\u{00d7}"
                }
            }

            body := View{
                width: Fill
                height: Fit
                flow: Down
                spacing: theme.space_2
                padding: Inset{left: 20. right: 20. top: 0. bottom: 8.}
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
            }

            footer := View{
                width: Fill
                height: Fit
                flow: Right
                align: Align{x: 1.0 y: 0.5}
                spacing: theme.space_2
                padding: Inset{left: 20. right: 20. top: 8. bottom: 16.}
                Filler{}
                cancel := ButtonFlat{text: "Cancel"}
                confirm := Button{text: "OK"}
                /** the same answer in the error role, shown instead of
                 * `confirm` while `destructive` is set */
                confirm_danger := ButtonDanger{text: "Delete" visible: false}
            }
        }
    }

    /** A dialog that only tells: one way out, no question. */
    mod.widgets.AlertDialog = mod.widgets.Dialog{
        size: Xs
        confirm_text: "OK"
        cancel_text: ""
    }

    /** A dialog that asks: an answer and a way out. */
    mod.widgets.ConfirmDialog = mod.widgets.Dialog{
        size: Xs
        confirm_text: "Confirm"
        cancel_text: "Cancel"
    }

    /** A dialog that asks before destroying something. */
    mod.widgets.DangerDialog = mod.widgets.ConfirmDialog{
        destructive: true
        confirm_text: "Delete"
    }

    /** A task that has taken over the window. */
    mod.widgets.FullScreenDialog = mod.widgets.Dialog{
        size: Full
        confirm_text: "Done"
        cancel_text: ""
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Dialog {
    #[deref]
    modal: Modal,
    #[live]
    pub size: DialogSize,
    #[live(true)]
    pub dismissable: bool,
    #[live]
    pub destructive: bool,
    #[live]
    pub title: String,
    #[live]
    pub confirm_text: String,
    #[live]
    pub cancel_text: String,
    /// Where the keyboard was before this dialog took it.
    #[rust]
    restore: Area,
    /// Whether the chrome has been written from the props this open.
    #[rust]
    dressed: bool,
}

impl Dialog {
    /// Show the dialog and take the keyboard, remembering where it was.
    pub fn open_dialog(&mut self, cx: &mut Cx) {
        self.restore = cx.key_focus();
        self.dressed = false;
        self.modal.open(cx);
    }

    /// Close it and give the keyboard back to whatever had it.
    pub fn close_dialog(&mut self, cx: &mut Cx) {
        self.modal.close(cx);
        // Back where it came from: a dialog that leaves the focus on
        // nothing makes the next Tab start from the top of the page.
        cx.set_key_focus(self.restore);
    }

    pub fn is_open(&self) -> bool {
        self.modal.is_open()
    }

    /// Write the props into the chrome. Done on the way into a draw rather
    /// than on apply, because a host sets `title` and the labels at the
    /// moment it opens the dialog, not when the DSL was applied.
    fn dress(&mut self, cx: &mut Cx) {
        let content = self.modal.widget(cx, ids!(content));
        content.label(cx, ids!(title_label)).set_text(cx, &self.title);
        // The destructive answer is a different button, not a recoloured
        // one: a button's face colours are shader uniforms it owns.
        let confirm = content.button(cx, ids!(confirm));
        let confirm_danger = content.button(cx, ids!(confirm_danger));
        confirm.set_text(cx, &self.confirm_text);
        confirm_danger.set_text(cx, &self.confirm_text);
        confirm.set_visible(cx, !self.destructive);
        confirm_danger.set_visible(cx, self.destructive);
        let cancel = content.button(cx, ids!(cancel));
        cancel.set_text(cx, &self.cancel_text);
        // An answer with no label is not an answer: leave it out rather
        // than drawing an empty button.
        cancel.set_visible(cx, !self.cancel_text.is_empty());
        content.widget(cx, ids!(close)).set_visible(cx, self.dismissable);
    }

    fn answer(&mut self, cx: &mut Cx, action: DialogAction) {
        let uid = self.widget_uid();
        cx.widget_action(uid, action);
        self.close_dialog(cx);
    }
}

impl Widget for Dialog {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.is_open() && !self.dressed {
            self.dressed = true;
            self.dress(cx.cx.cx);
        }
        // The size rung is applied every draw, so a host that changes it
        // between opens gets the width it asked for.
        if self.is_open() {
            let content = self.modal.widget(cx.cx.cx, ids!(content));
            let width = match self.size.width() {
                Some(w) => Size::Fixed(w),
                None => Size::fill(),
            };
            let full = self.size == DialogSize::Full;
            let mut view = content.borrow_mut::<crate::view::View>();
            if let Some(view) = view.as_mut() {
                view.walk.width = width;
                if full {
                    view.walk.height = Size::fill();
                }
            }
            drop(view);
        }
        self.modal.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.is_open() {
            return;
        }
        // Escape and the outside press are the dialog's own: the modal's
        // versions are turned off in `dismissable`, so that one press
        // closes one overlay even with a menu open inside this dialog.
        if let Event::KeyDown(ke) = event {
            match ke.key_code {
                KeyCode::Escape if self.dismissable => {
                    if claim_escape(cx) {
                        self.answer(cx, DialogAction::Dismissed);
                        return;
                    }
                }
                // Return takes the default answer, which is the promise a
                // dialog makes to someone working by keyboard.
                KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                    self.answer(cx, DialogAction::Confirmed);
                    return;
                }
                _ => {}
            }
        }
        self.modal.handle_event(cx, event, scope);
        // A press that lands outside the card leaves without answering. The
        // modal has already stopped it reaching the page underneath.
        if self.dismissable {
            if let Event::MouseUp(me) = event {
                let card = self.modal.widget(cx, ids!(content)).area().rect(cx);
                if card.size.x > 0.0 && !card.contains(me.abs) {
                    self.answer(cx, DialogAction::Dismissed);
                    return;
                }
            }
        }
        if let Event::Actions(actions) = event {
            let content = self.modal.widget(cx, ids!(content));
            if content.button(cx, ids!(confirm)).clicked(actions)
                || content.button(cx, ids!(confirm_danger)).clicked(actions)
            {
                self.answer(cx, DialogAction::Confirmed);
            } else if content.button(cx, ids!(cancel)).clicked(actions) {
                self.answer(cx, DialogAction::Cancelled);
            } else if content.button(cx, ids!(close)).clicked(actions) {
                self.answer(cx, DialogAction::Dismissed);
            }
        }
    }

    /// The title: what the dialog is asking about.
    fn text(&self) -> String {
        self.title.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.title != v {
            self.title = v.to_string();
            self.dressed = false;
            self.modal.redraw(cx);
        }
    }
}

impl DialogRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open_dialog(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close_dialog(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    /// Set the question and its answers before opening.
    pub fn ask(&self, cx: &mut Cx, title: &str, confirm: &str, cancel: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.title = title.to_string();
            inner.confirm_text = confirm.to_string();
            inner.cancel_text = cancel.to_string();
            inner.open_dialog(cx);
        }
    }

    pub fn confirmed(&self, actions: &Actions) -> bool {
        self.answered(actions) == Some(DialogAction::Confirmed)
    }

    pub fn cancelled(&self, actions: &Actions) -> bool {
        self.answered(actions) == Some(DialogAction::Cancelled)
    }

    pub fn dismissed(&self, actions: &Actions) -> bool {
        self.answered(actions) == Some(DialogAction::Dismissed)
    }

    /// What the dialog answered this pass, if it answered.
    pub fn answered(&self, actions: &Actions) -> Option<DialogAction> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<DialogAction>() {
            DialogAction::None => None,
            answer => Some(answer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rungs are the questions people ask, and they only go up. Full
    /// takes the room it is given rather than a width of its own.
    #[test]
    fn the_size_rungs_climb_and_full_is_not_a_number() {
        let widths: Vec<f64> = [
            DialogSize::Xs,
            DialogSize::Sm,
            DialogSize::Md,
            DialogSize::Lg,
            DialogSize::Xl,
        ]
        .iter()
        .map(|s| s.width().unwrap())
        .collect();
        assert!(widths.windows(2).all(|w| w[0] < w[1]), "{widths:?} must climb");
        assert_eq!(DialogSize::Full.width(), None);
    }
}
