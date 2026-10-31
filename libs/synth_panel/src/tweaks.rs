//! `SynthTweaks{node: "acid" PanelModule{…}}`: an instrument's detailed
//! controls (its envelope, filter, effects, macros). The voice tree draws
//! them in place, unfolded right under the voice that plays the instrument
//! when it (or the voice) is clicked; where the panel lists them they take
//! no room. Unfolding opens their modules. The panel's author (the score's
//! agent) fills one per instrument; the tree and the variants are the
//! fixed part.

use crate::host::{self as synth, Inline};
use makepad_value_pickers::panel::host::{open_modules, tree_elements};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SynthTweaksBase = #(SynthTweaks::register_widget(vm))
    /** An instrument's tweaks, shown while it (or a voice playing it) is selected. */
    mod.widgets.SynthTweaks = set_type_default() do mod.widgets.SynthTweaksBase{
        width: Fill height: Fit flow: Down spacing: 6
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct SynthTweaks {
    #[deref]
    view: View,
    /// The instrument these are for.
    #[live]
    node: String,
    /// The selection last acted on, and the modules drawn in it (as the
    /// panel's module index first listed them).
    #[rust]
    seen: u64,
    #[rust]
    modules: Vec<u64>,
    /// Drawn by the tree this frame, and so in the last (its events).
    #[rust]
    drawn: bool,
    #[rust]
    inline: bool,
    /// The selection a missing inline draw asked a redraw for (once).
    #[rust]
    healed: u64,
}

impl SynthTweaks {
    /// Whether the tree unfolds it now: its instrument, or a voice playing
    /// it, selected for its controls.
    fn shown(&self) -> bool {
        if synth::selected_for_variations() {
            return false;
        }
        let Some(selected) = synth::selected() else { return false };
        selected == self.node || synth::tracks().iter().any(|t| t.name == selected && t.instrument.as_deref() == Some(self.node.as_str()))
    }
}

impl Widget for SynthTweaks {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        synth::register_inline(Inline::Tweaks(self.node.clone()), self.widget_uid());
        if self.inline {
            self.view.handle_event(cx, event, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !synth::drawing_inline() {
            // The panel's own pass: the tree drew it (or not) already.
            synth::register_inline(Inline::Tweaks(self.node.clone()), self.widget_uid());
            self.inline = std::mem::take(&mut self.drawn);
            cx.walk_turtle(Walk { height: Size::Fixed(0.0), ..walk });
            // Selected before the tree knew of it: drawn on the next frame.
            let rev = synth::selection_rev();
            if !self.inline && self.shown() && self.healed != rev {
                self.healed = rev;
                cx.redraw_all();
            }
            return DrawStep::done();
        }
        self.drawn = true;
        // The modules drawn here: those the index learns of while it draws.
        let before: Vec<u64> = tree_elements().into_iter().flat_map(|(_, ids)| ids).collect();
        while !self.view.draw_walk(cx, scope, walk).is_done() {}
        for id in tree_elements().into_iter().flat_map(|(_, ids)| ids) {
            if !before.contains(&id) && !self.modules.contains(&id) {
                self.modules.push(id);
            }
        }
        // Just selected: its modules open, the first comes into view.
        let rev = synth::selection_rev();
        if self.seen != rev {
            self.seen = rev;
            if synth::selected_for_variations() {
                return DrawStep::done();
            }
            open_modules(&self.modules);
            // Its next frame opens them and brings the first into view.
            self.view.redraw(cx);
            cx.new_next_frame();
        }
        DrawStep::done()
    }
}
