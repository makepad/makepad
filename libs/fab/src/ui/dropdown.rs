//! Lane D. The popup buttons: the header dropdown (label + SVG chevron,
//! `FabDropdownButton`) and the menu-bar-style entry (`FabMenuButton`, same
//! machine minus the chevron). Hover/press lit; the one whose menu is up
//! carries the accent "open" look.
//!
//! Pressing one is deliberately opinion-free: it reports
//! `FabUiAction::DropdownClicked { tag, anchor }` and nothing else. Whoever
//! owns that `tag` builds the item list and raises the menu. That is what
//! lets lane G's Tours header get a working editor-type dropdown without
//! lane G writing any menu code, and it keeps one button style behind every
//! menu in the app.
//!
//! The open look is never a per-button toggle. The button mirrors the menu
//! layer's `MenuOpened` / `MenuClosed` broadcast for its `owner`
//! ([`open_after`]), so a menu going down for *any* reason — a pick, Escape,
//! a click outside, another popup opening over it, focus loss — returns the
//! button to idle in the same pass. A `MenuClickAway` landing on this button
//! re-presses it: the press that dismissed a neighbour's menu opens this one
//! on that same click instead of needing a second. (Hover cannot linger
//! either: the layer clears hover tree-wide on every popup transition,
//! because its modal grab is what kept the hover-outs from arriving.)
//!
//! Registered with the control kit (before every lane) so lanes can place it.

use crate::ui::popover::{open_after, ui_actions, FabUiAction};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    mod.widgets.FabDropdownButtonBase = #(FabDropdownButton::register_widget(vm))
    mod.widgets.FabDropdownButton = set_type_default() do mod.widgets.FabDropdownButtonBase{
        width: Fit
        height: fab.row_height
        flow: Right
        // The Fit-height label is centred on its ink. The DrawVector chevron
        // stays in a Fill-height, symmetrically padded slot, so y-align has no
        // deferred shift to apply to it.
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 6 right: 3 top: 0 bottom: 0}
        spacing: fab.chevron_gap
        cursor: MouseCursor.Hand
        show_bg: true
        tag: @editor_type
        draw_bg +: {
            hover: instance(0.0)
            down: instance(0.0)
            open: instance(0.0)
            focus: instance(0.0)
            disabled: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                let fill = vec4(fab.color_button_hover.xyz, self.hover * 0.85 + self.down * 0.15)
                    .mix(vec4(fab.color_editor_alt.xyz, 0.4), self.disabled)
                    .mix(vec4(fab.color_accent.xyz, 1.0), self.open)
                sdf.fill_keep(fill)
                sdf.stroke(vec4(fab.color_focus_ring.xyz, self.focus), 1.0)
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
            down: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {down: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {down: 1.0} }
                }
            }
            open: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {open: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {open: 1.0} }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {focus: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: { draw_bg: {focus: 1.0} }
                }
            }
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: { draw_bg: {disabled: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {disabled: 1.0} }
                }
            }
        }
        label := mod.widgets.FabLabel{ height: Fit text: "Menu" }
        chevron_slot := View{
            width: fab.chevron_size
            height: Fill
            padding: Inset{top: 5 bottom: 5 left: 0 right: 0}
            chevron := mod.widgets.FabIconMuted{
                width: fab.chevron_size
                height: fab.chevron_size
                icon_walk: Walk{ width: fab.chevron_size height: fab.chevron_size }
                draw_icon +: {
                    svg: crate_resource("self://resources/icons/chevron_down.svg")
                }
            }
        }
    }

    // Menu-bar-style entry (the viewport header's View / Select / Object,
    // the ☰ / ⋯ overflow buttons): the same popup state machine, minus the
    // chevron.
    mod.widgets.FabMenuButton = mod.widgets.FabDropdownButton{
        align: Align{x: 0.5 y: 0.5}
        padding: Inset{left: 7 right: 7 top: 0 bottom: 0}
        spacing: 0
        chevron_slot +: { visible: false }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabDropdownButton {
    #[deref]
    view: View,
    #[live(live_id!(editor_type))]
    tag: LiveId,
    /// The menu-owner id the host raises this button's menu under. The
    /// button mirrors the layer's `MenuOpened` / `MenuClosed` for it. Hosts
    /// with a fixed menu set it in the DSL (`owner: @fab_vp_mode`);
    /// `FabArea` stamps its per-slot id onto the editor-type buttons.
    #[live(LiveId(0))]
    owner: LiveId,
    /// Mirror of "my owner's menu is up" — written only from the broadcast.
    #[rust]
    open: bool,
}

impl FabDropdownButton {
    fn press(&mut self, cx: &mut Cx) {
        let anchor = self.view.area().rect(cx);
        cx.action(FabUiAction::DropdownClicked {
            tag: self.tag,
            anchor,
        });
    }

    fn sync_open(&mut self, cx: &mut Cx, open: bool) {
        if self.open == open {
            return;
        }
        self.open = open;
        if open {
            self.view.animator_play(cx, ids!(open.on));
        } else {
            self.view.animator_play(cx, ids!(open.off));
        }
    }
}

impl FabDropdownButtonRef {
    /// Stamp the menu-owner id this button mirrors (`FabArea` calls this
    /// with its per-slot id, the viewport header with pane-scoped ids; the
    /// rest take theirs from the DSL).
    pub fn set_owner(&self, owner: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.owner = owner;
        }
    }

    /// Stamp the press tag — paired with `set_owner` where one DSL template
    /// serves several instances (the viewport header exists once per pane).
    pub fn set_tag(&self, tag: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.tag = tag;
        }
    }

    /// Drive the open look directly, for a button whose popup the menu
    /// layer does not own (the viewport's ⋯ overflow popover). The host
    /// funnels every open/close of that popup through one place — the same
    /// single-writer discipline the broadcast gives menu-owned buttons —
    /// and mirrors it here.
    pub fn set_open(&self, cx: &mut Cx, open: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.sync_open(cx, open);
        }
    }
}

impl Widget for FabDropdownButton {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let clicked = actions
            .find_widget_action(self.view.widget_uid())
            .map(|a| match a.cast::<ViewAction>() {
                ViewAction::FingerUp(e) => e.is_over && e.device.is_primary_hit(),
                _ => false,
            })
            .unwrap_or(false);
        if clicked {
            self.press(cx);
        }
        // `was_open` is the state before this pass: a click-away's
        // `MenuClosed` rides the same pass as its `MenuClickAway`, and the
        // press that toggled this button's own menu closed must not also
        // re-open it.
        let was_open = self.open;
        for a in ui_actions(actions) {
            if let FabUiAction::MenuClickAway { at } = a {
                if !was_open
                    && self.view.visible
                    && self.view.area().clipped_rect(cx).contains(*at)
                {
                    self.press(cx);
                }
            }
        }
        if let Some(open) = open_after(ui_actions(actions), self.owner) {
            self.sync_open(cx, open);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}
