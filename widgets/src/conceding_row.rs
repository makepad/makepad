//! A row that gives way in a stated order when it runs out of width.
//!
//! Each child that can give way carries two things: a rank saying when it goes,
//! and the face it wears once it has gone.
//!
//! ```text
//! toolbar := ConcedingRow{
//!     inspect := ButtonFlat{ text: "Inspect"   give_up := 1  tight: { text: "<>" } }
//!     title   := H4{         text: "Catalogue" give_up := 2  tight: { visible: false } }
//!     new_only := Toggle{    text: "New only"  give_up := 3  tight: { text: "New" } }
//! }
//! ```
//!
//! Children sharing a rank give way together. A child with no rank never gives
//! way, and a row with no ranks at all does nothing and costs nothing.
//!
//! The row decides its level BEFORE it draws, from arithmetic: it asks every
//! child how wide it would be at each level, which makes what a rung saves an
//! exact measured number rather than something learned by taking the rung and
//! looking afterwards. That is the whole reason it settles in one frame.

use crate::{
    button_group::ConcessionLadder, makepad_derive_widget::*, makepad_draw::*, view::View,
    widget::*, width_override::WidthOverride,
};
use std::collections::BTreeMap;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.ConcedingRowBase = set_type_default() do #(ConcedingRow::register_widget(vm))
    mod.widgets.ConcedingRow = mod.widgets.ConcedingRowBase{
        width: Fill
        height: Fit
        flow: Right
        align: Align{x: 0. y: 0.5}
    }
}

/// How much slack the row keeps in hand. The two thresholds for one rung sit
/// its own cost apart, so a row on the edge does not flap between two faces.
const MARGIN: f64 = 6.0;

/// One step of the ladder: the children that give way at it, and what each of
/// them wears once they have.
#[derive(Default)]
struct Rung {
    wear: Vec<(LiveId, ScriptObjectRef, WidthOverride)>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ConcedingRow {
    #[deref]
    view: View,

    #[rust]
    ladder: ConcessionLadder,
    #[rust]
    rungs: Vec<Rung>,
    #[rust]
    applied: usize,
    #[rust]
    collected: bool,
    /// Each child's authored face, for exactly the keys its rung names, read
    /// before any rung was put on.
    ///
    /// Nothing in the library unwinds an apply -- the animator does not either
    /// -- so taking a face off is putting the face underneath back on, and that
    /// face has to have been kept.
    #[rust]
    base: Vec<(LiveId, ScriptObjectRef)>,
    /// The same authored face, read as what it is worth in width. Pricing
    /// starts from this and lays the rungs in force over it, so a price never
    /// depends on the face the child is wearing at the time.
    #[rust]
    base_over: Vec<(LiveId, WidthOverride)>,
}

impl ConcedingRow {
    /// Read the ladder off the children: who gives way, in what order, and what
    /// they wear when they do.
    ///
    /// Done once after an apply rather than per draw, so a row whose children
    /// name no ranks pays one heap read each and nothing afterwards.
    fn collect(&mut self, cx: &mut Cx) {
        self.rungs.clear();
        self.applied = 0;
        let children: Vec<(LiveId, WidgetRef)> = self.view.children.iter().cloned().collect();
        let mut by_rank: BTreeMap<u64, Rung> = BTreeMap::new();
        let mut base: Vec<(LiveId, ScriptObjectRef)> = Vec::new();
        let mut base_over: Vec<(LiveId, WidthOverride)> = Vec::new();
        cx.with_vm(|vm| {
            for (id, child) in children {
                let src = child.script_source();
                // A small integer literal is stored as U40, so as_i32, as_u32 and
                // as_f64 all answer None for it. as_number is the only accessor
                // that sees every numeric representation, and reading a rank with
                // any of the others finds no ranks at all, silently.
                let Some(rank) = vm
                    .bx
                    .heap
                    .value(src, id!(give_up).into(), NoTrap)
                    .as_number()
                else {
                    continue;
                };
                let block = vm.bx.heap.value(src, id!(tight).into(), NoTrap);
                let Some(obj) = block.as_object() else {
                    continue;
                };
                let over = WidthOverride::parse(vm, obj);

                // The authored face, taken now, before a rung has been put on.
                //
                // Read off the widget rather than off its script object: a
                // property the use site did not state is a Rust default and is
                // not in the object at all, so `visible` on a button that never
                // mentions it reads as nothing -- and a face that restores
                // nothing is a concession the row can never take back.
                let keys = WidthOverride::keys(vm, obj);
                let was = vm.bx.heap.new_object();
                for key in keys {
                    let v = if key == id!(visible) {
                        ScriptValue::from_bool(child.visible())
                    } else if key == id!(text) {
                        let s = child.text();
                        vm.bx.heap.new_string_from_str(&s)
                    } else {
                        vm.bx.heap.value(src, key.into(), NoTrap)
                    };
                    if !v.is_nil() {
                        vm.bx.heap.set_value_def(was, key.into(), v);
                    }
                }
                base_over.push((id, WidthOverride::parse(vm, was)));
                base.push((id, vm.bx.heap.new_object_ref(was)));

                let handle = vm.bx.heap.new_object_ref(obj);
                by_rank
                    .entry(rank as u64)
                    .or_default()
                    .wear
                    .push((id, handle, over));
            }
        });
        // The author's numbers need not be dense; the ladder indexes a
        // contiguous slice and silently ignores a step outside it.
        self.base = base;
        self.base_over = base_over;
        self.rungs = by_rank.into_values().collect();
        self.ladder = ConcessionLadder::new(self.rungs.len(), MARGIN);
        self.collected = true;
    }

    /// The row's natural outer width with rungs `0..level` in force, or `None`
    /// when a child on it cannot price itself under those rungs.
    fn span(&mut self, cx: &mut Cx2d, level: usize) -> Option<f64> {
        let mut total = 0.0;
        let mut shown = 0usize;
        let children: Vec<(LiveId, WidgetRef)> = self.view.children.iter().cloned().collect();
        for (id, child) in children {
            // The authored face, then every rung in force laid over it in order
            // -- not what the child is wearing now, which is already a
            // concession and would make every later price a price of that.
            let over = match self.base_over.iter().find(|(b, _)| *b == id) {
                Some((_, base)) => {
                    let mut eff = base.clone();
                    for rung in &self.rungs[..level] {
                        if let Some((_, _, o)) = rung.wear.iter().find(|(w, _, _)| *w == id) {
                            eff.overlay(o);
                        }
                    }
                    Some(eff)
                }
                None => None,
            };
            if over.as_ref().is_some_and(WidthOverride::hides) {
                continue;
            }
            let w = child.measure_width(cx, over.as_ref())?;
            if w > 0.0 {
                shown += 1;
            }
            total += w;
        }
        Some(total + self.layout.spacing * shown.saturating_sub(1) as f64)
    }

    /// Price every rung and settle the level, before a thing is drawn.
    fn fit(&mut self, cx: &mut Cx2d, walk: Walk) -> usize {
        if self.rungs.is_empty() {
            return 0;
        }
        // Room on the line. `None` inside a Fit-width parent: there is no width
        // to answer with, and a row deciding off that number would walk its
        // whole ladder down on a measurement that is not one.
        let Some(room) = cx.turtle().max_width(Walk {
            width: Size::fill(),
            ..walk
        }) else {
            return self.ladder.level();
        };
        if !(room > 0.0) {
            return self.ladder.level();
        }

        let mut spans: Vec<f64> = Vec::with_capacity(self.rungs.len() + 1);
        for level in 0..=self.rungs.len() {
            let Some(s) = self.span(cx, level) else { break };
            spans.push(s);
        }
        if spans.is_empty() {
            return self.ladder.level();
        }

        // What rung `i` saves is the difference its face makes, measured this
        // frame rather than learned by taking it. A rung past the first one
        // nobody could price keeps its zero, which the ladder reads as NOT KNOWN
        // and walks one per draw -- the old behaviour, reached without a new
        // rule.
        self.ladder.clear_costs();
        for i in 1..spans.len() {
            let saved = spans[i - 1] - spans[i];
            if saved > 0.0 {
                self.ladder.set_cost(i - 1, saved);
            }
        }
        let at = self.ladder.level().min(spans.len() - 1);
        self.ladder.measured(room - spans[at]);
        self.ladder.level()
    }

    /// Put every child in the face its level calls for.
    ///
    /// Nothing unwinds -- not here and not in the animator -- so a rung names
    /// both faces and level 0 is a rung like any other.
    fn wear(&mut self, cx: &mut Cx, level: usize) {
        // Everything a rung can touch goes back to what it was authored as
        // first, then the rungs in force land on top, in order.
        let mut put: Vec<(LiveId, ScriptObjectRef)> = self.base.clone();
        for rung in &self.rungs[..level.min(self.rungs.len())] {
            for (id, block, _) in &rung.wear {
                put.push((*id, block.clone()));
            }
        }
        if put.is_empty() {
            return;
        }
        let children: Vec<(LiveId, WidgetRef)> = self.view.children.iter().cloned().collect();
        cx.with_vm(|vm| {
            for (id, block) in put {
                let Some((_, child)) = children.iter().find(|(c, _)| *c == id) else {
                    continue;
                };
                let mut child = child.clone();
                // Apply::Animate, never ScriptReapply: under an apply that
                // preserves runtime state, String, ArcStringMut and every
                // #[visible] field return early, so text and visibility would
                // silently not move.
                child.script_apply(
                    vm,
                    &Apply::Animate,
                    &mut Scope::empty(),
                    block.as_object().into(),
                );
            }
        });
    }
}

impl Widget for ConcedingRow {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.collected {
            self.collect(cx.cx);
        }
        let level = self.fit(cx, walk);
        if level != self.applied {
            self.wear(cx.cx, level);
            self.applied = level;
        }
        // One draw, at the level already settled. The deciding happened before
        // the drawing, so there is nothing here to ask for a redraw about.
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }
}
