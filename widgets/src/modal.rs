use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_platform::{KeyCode, KeyEvent},
    view::*,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ModalBase = #(Modal::register_widget(vm))

    mod.widgets.Modal = mod.widgets.ModalBase{
        width: Fill
        height: Fill
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
                color: uniform(#000000B3)
                pixel: fn() {
                    return self.color
                }
            }
        }

        content := View{
            width: Fit
            height: Fit
            flow: Down
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum ModalAction {
    Dismissed,
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct Modal {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    #[rust]
    draw_list: Option<DrawList2d>,

    #[live]
    draw_bg: DrawQuad,

    #[rust]
    is_open: bool,
    /// Whether the modal can be dismissed via an external interaction, including:
    /// clicking outside the content view, pressing Escape, or performing
    /// the back navigational gesture (e.g., on Android).
    #[live(true)]
    can_dismiss: bool,
}

impl ScriptHook for Modal {
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

impl Widget for Modal {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.is_open {
            return;
        }

        // Forward the event to the inner `content` view.
        let content = self.view.widget(cx, ids!(content));
        content.handle_event(cx, event, scope);

        // Proactively consume any hit that occurred in the bg area, which prevents the hit
        // from being handled by any views underneath this modal.
        let bg_area = self.draw_bg.area();
        let bg_area_hit = event.hits(cx, bg_area);

        if self.can_dismiss {
            // This is fine, because we already let `content` handle this event above.
            let content_area_hit = event.hits(cx, content.area());

            // Close the modal if any of the following conditions occur:
            // * If the back navigational action/gesture was triggered (e.g., on Android),
            // * If the Escape key was released while `content` has key focus.
            //   We look for KeyUp (not KeyDown) to match the FingerUp dismissal,
            //   which also prevents a widget behind the modal from handling that Escape keypress.
            // * If there was a click/tap in the background area, outside of the inner `content` view.
            let should_close = event.back_pressed()
                || match bg_area_hit {
                    Hit::FingerUp(fe) => !content.area().rect(cx).contains(fe.abs),
                    _ => false,
                }
                || match content_area_hit {
                    Hit::KeyUp(KeyEvent {
                        key_code: KeyCode::Escape,
                        ..
                    }) => true,
                    _ => false,
                };
            if should_close {
                cx.widget_action(content.widget_uid(), ModalAction::Dismissed);
                self.close(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(self.view.layout);
        self.draw_bg.begin(cx, self.view.walk, self.view.layout);

        if self.is_open {
            let bg_view = self.view.widget(cx, ids!(bg_view));
            let _ = bg_view.draw_walk(cx, scope, walk.with_abs_pos(Vec2d { x: 0., y: 0. }));

            let content = self.view.widget(cx, ids!(content));
            let _ = content.draw_all(cx, scope);
        }

        self.draw_bg.end(cx);
        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);

        // We must re-set the blocked scrolling area, as it might've changed after each draw.
        if self.is_open {
            let content_area = self.view.widget(cx, ids!(content)).area();
            cx.block_scrolling_except_within(content_area);
        }
        DrawStep::done()
    }
}

impl Modal {
    pub fn open(&mut self, cx: &mut Cx) {
        self.is_open = true;
        // Redraw the overlay draw_list directly so the first open is visible
        // even before the overlay content has refreshed its draw area.
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
        let content = self.view.widget(cx, ids!(content));
        cx.set_key_focus(content.area());
        content.set_scroll_pos(cx, Vec2d { x: 0.0, y: 0.0 });
    }

    pub fn close(&mut self, cx: &mut Cx) {
        // Closing an already-closed modal must be a no-op. App code routinely
        // calls `close()` defensively (handlers that dismiss a modal whether
        // or not it happens to be open). Without this guard the
        // `revert_key_focus()` below would still run and yank key focus away
        // from an unrelated widget — e.g. a text field the user just tapped,
        // which on mobile then dismisses the soft keyboard.
        if !self.is_open {
            return;
        }
        // Inform the inner modal content that its modal is being dismissed.
        let content = self.view.widget(cx, ids!(content));
        content.handle_event(
            cx,
            &Event::Actions(vec![Box::new(ModalAction::Dismissed)]),
            &mut Scope::empty(),
        );
        self.is_open = false;
        // Overlay widgets need their dedicated draw_list invalidated explicitly;
        // a background redraw alone can leave the previous frame visible too long.
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
        cx.revert_key_focus();
        cx.unblock_scrolling();
    }

    pub fn dismissed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).cast(),
            ModalAction::Dismissed
        )
    }
}

impl ModalRef {
    /// Returns whether the modal is currently open (displayed).
    pub fn is_open(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.is_open
        } else {
            false
        }
    }

    /// Opens (displays) the model.
    #[doc(alias = "show")]
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    /// Closes (hides) the modal.
    #[doc(alias = "hide")]
    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    /// Returns `true` if this modal was dismissed by the given `actions`.
    pub fn dismissed(&self, actions: &Actions) -> bool {
        if let Some(inner) = self.borrow() {
            inner.dismissed(actions)
        } else {
            false
        }
    }
}
