//! A PTY terminal in the project folder: the terminal app's `MpTerm` in the
//! Code look. Contract: `focus(cx)` gives it the keyboard; `close(cx)` ends
//! its shell (the shell calls it before closing the tab; dropping the panel
//! ends it too). Each terminal tab is its own panel with its own PTY.
//!
//! Keys: the window's keys (Cmd+Shift+F, Cmd+`, Cmd+Shift+` and, on macOS,
//! Cmd+P) never reach the shell in the terminal. On Linux and Windows the
//! primary modifier is Ctrl, and Ctrl+P is the shell's (history); Ctrl+`,
//! Ctrl+Shift+` and Ctrl+Shift+F stay the window's. `terminal_has_focus`
//! tells the window when a terminal holds the keyboard.
//!
//! When the shell exits the panel says so and Enter starts a new one in the
//! same folder.

use super::Workspace;
use crate::ui::theme::{vec, PALETTE};
use makepad_app_terminal::widget::{MpTerm, MpTermAction};
use makepad_widgets::*;

const P: &crate::ui::theme::Palette = &PALETTE;

/// A palette colour as a script colour value (0xRRGGBBAA), what
/// `theme_tokens::color` reads.
const fn token(rgb: u32) -> ScriptValue {
    ScriptValue::from_color((rgb << 8) | 0xff)
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The terminal's own ink: MpTerm reads these theme tokens (as colour
    // values, not vectors).
    mod.theme.color_terminal_bg = #(token(P.terminal_bg))
    mod.theme.color_terminal_text = #(token(P.terminal_text))
    mod.theme.color_terminal_cursor = #(token(P.primary))

    mod.widgets.TerminalPanelBase = #(TerminalPanel::register_widget(vm))

    mod.widgets.TerminalPanel = set_type_default() do mod.widgets.TerminalPanelBase{
        width: Fill
        height: Fill
        flow: Overlay
        show_bg: true
        draw_bg +: { color: #(vec(P.terminal_bg, 1.0)) }

        term := MpTerm{
            width: Fill
            height: Fill
            font_size: 9.0
            pad_x: 8.0
            pad_y: 4.0
            focus_on_first_draw: false
        }
        exited := View{
            visible: false
            width: Fill
            height: Fill
            align: Align{x: 0.5 y: 1.0}
            padding: Inset{bottom: 10}
            View{
                width: Fit
                height: Fit
                padding: Inset{left: 10 right: 10 top: 3 bottom: 3}
                show_bg: true
                draw_bg +: { color: mod.look.raised }
                Label{
                    text: "[process exited — Enter to restart]"
                    draw_text +: {
                        color: mod.look.agent
                        text_style: mod.look.mono{font_size: 8.5}
                    }
                }
            }
        }
    }
}

/// The terminal that last took the keyboard (a `Cx` global): its focus area.
struct TerminalFocus(Option<Area>);

/// Whether a terminal holds the keyboard now: the window leaves the shell's
/// own Ctrl keys (Ctrl+P) to it on Linux and Windows. Asked of the key focus
/// itself, not of a flag the panel keeps: a folded terminal panel gets no
/// events, so a flag it set while focused stayed set after the focus moved on
/// (Ctrl+P never opened quick-open on Linux once the start terminal had it).
pub fn terminal_has_focus(cx: &Cx) -> bool {
    cx.get_global_ref::<TerminalFocus>().and_then(|f| f.0).is_some_and(|area| cx.has_key_focus(area))
}

/// A window key the terminal must not hand to the shell.
pub fn is_window_key(ke: &KeyEvent) -> bool {
    let m = ke.modifiers;
    if !m.is_primary() || m.alt {
        return false;
    }
    match ke.key_code {
        KeyCode::Backtick => true,
        KeyCode::KeyF => m.shift,
        KeyCode::KeyP => cfg!(target_os = "macos") && !m.shift,
        _ => false,
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct TerminalPanel {
    #[deref]
    view: View,
    #[rust]
    started: bool,
    #[rust]
    exited: bool,
    #[rust]
    want_focus: bool,
    #[rust]
    had_focus: bool,
}

impl TerminalPanel {
    pub fn focus(&mut self, cx: &mut Cx) {
        self.want_focus = true;
        if self.started {
            if let Some(mut term) = self.view.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                term.take_key_focus(cx);
            }
        }
        self.view.redraw(cx);
    }

    /// Ends the shell (kills the PTY child and its process group).
    pub fn close(&mut self, cx: &mut Cx) {
        if let Some(mut term) = self.view.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            term.unload(cx);
        }
        self.set_focus_global(cx, false);
    }

    fn restart(&mut self, cx: &mut Cx) {
        if let Some(mut term) = self.view.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            let cwd = term.cwd.clone();
            term.restart_with(cx, cwd, None);
        }
        self.exited = false;
        self.view.view(cx, ids!(exited)).set_visible(cx, false);
        self.view.redraw(cx);
    }

    fn set_focus_global(&mut self, cx: &mut Cx, focus: bool) {
        if focus == self.had_focus {
            return;
        }
        self.had_focus = focus;
        let Some(area) = self.view.widget(cx, ids!(term)).borrow::<MpTerm>().map(|t| t.focus_area()) else { return };
        let current = cx.get_global_ref::<TerminalFocus>().and_then(|f| f.0);
        if focus {
            cx.set_global(TerminalFocus(Some(area)));
        } else if current == Some(area) {
            cx.set_global(TerminalFocus(None));
        }
    }
}

impl Widget for TerminalPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let focused = self.view.widget(cx, ids!(term)).borrow::<MpTerm>().is_some_and(|t| t.has_input_focus(cx));
        if let Event::KeyDown(ke) = event {
            if focused && is_window_key(ke) {
                return;
            }
            if focused && self.exited && ke.key_code == KeyCode::ReturnKey && !ke.modifiers.any() {
                self.restart(cx);
                return;
            }
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        for action in actions.iter() {
            if let Some(wa) = action.as_widget_action() {
                if let MpTermAction::Exited = wa.cast::<MpTermAction>() {
                    if !self.exited {
                        self.exited = true;
                        self.view.view(cx, ids!(exited)).set_visible(cx, true);
                        self.view.redraw(cx);
                    }
                }
            }
        }
        let focused = self.view.widget(cx, ids!(term)).borrow::<MpTerm>().is_some_and(|t| t.has_input_focus(cx));
        self.set_focus_global(cx, focused);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.started {
            // The shell starts in the project folder on the first draw.
            let root = Workspace::of(cx).map(|w| w.root.clone());
            if let Some(mut term) = self.view.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                if term.cwd.is_none() {
                    term.cwd = root;
                }
            }
        }
        let step = self.view.draw_walk(cx, scope, walk);
        self.started = true;
        if std::mem::take(&mut self.want_focus) {
            if let Some(mut term) = self.view.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                term.take_key_focus(cx);
            }
        }
        step
    }
}
