//! OverlayLayers — the one place an app declares its floating layers.
//!
//! Why: the layers that float over a whole window (the tooltip host today;
//! the menu layer, the toaster and the dialog host as they land) each want
//! to be declared once, last in the window's body, so they draw over every
//! panel and are walked after everything they float over. `Window` cannot
//! own them: it registers in the widget module before `tip`, `popup_menu`,
//! `text_input`, `slider` and `modal`, so its DSL default cannot name a
//! single layer widget. This host is therefore OPT-IN, declared by the app
//! the way `TipLayer` is declared today, but as one line that carries every
//! layer at once:
//!
//! ```text
//! body +: {
//!     ...the app...
//!     layers := OverlayLayers{}
//! }
//! ```
//!
//! It claims no space in the body's layout, whatever the body's flow: like
//! `Modal`, it pins the walk it reports upward to empty and draws its
//! children on a root turtle sized by the pass, inside its own overlay draw
//! list, so a layer that asks for `Fill` gets the window and not a share of
//! the body's spare height. It registers LAST in the widget module, after
//! every layer it owns, because a widget deriving from another must
//! register after it.

use crate::{makepad_derive_widget::*, makepad_draw::*, view::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.OverlayLayersBase = #(OverlayLayers::register_widget(vm))

    /** The window's floating layers, declared once as the last child of the
     * body. Owns the tooltip host now; the menu layer, the toaster and the
     * dialog host take their slots here as they land. */
    mod.widgets.OverlayLayers = set_type_default() do mod.widgets.OverlayLayersBase{
        // No `width`/`height`: the host claims no slot in the body's layout
        // (`on_after_apply` pins its walk to empty) and lays its layers out
        // over the whole pass instead.
        flow: Overlay

        /** the hover-tooltip host every `Tip{}` in the window reports to */
        tip_layer := TipLayer{}
        // Slots to come, in this order so each draws over the last:
        //   menu_layer := MenuLayer{}
        //   toaster := Toaster{}
        //   dialog_host := DialogHost{}
    }
}

#[derive(Script, Widget)]
pub struct OverlayLayers {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for OverlayLayers {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    /// The host occupies NO space in the layout that holds it: its layers
    /// paint over the whole pass on a root turtle, never in the body's
    /// flow. Forced here rather than only left out of the DSL, so that an
    /// instance writing `OverlayLayers{height: Fill}` cannot turn the host
    /// into a deferred fill that takes a share of the body's spare height.
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.view.walk = Walk::empty();
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(vm.cx_mut());
        }
    }
}

impl Widget for OverlayLayers {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    /// The incoming walk is deliberately ignored: the host is not laid out
    /// by the body at all. Its layers get the pass, so each one that asks
    /// for `Fill` is handed the window.
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let Some(mut draw_list) = self.draw_list.take() else {
            return DrawStep::done();
        };
        draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(Layout::default());
        self.view.draw_walk_all(cx, scope, Walk::fill());
        cx.end_pass_sized_turtle();
        draw_list.end(cx);
        self.draw_list = Some(draw_list);
        DrawStep::done()
    }
}
