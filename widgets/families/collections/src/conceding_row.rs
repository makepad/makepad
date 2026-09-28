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

#[derive(Script, Widget)]
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
    /// The children were applied again from their markup, so the faces
    /// they wear may no longer be the ones the level calls for. See the
    /// `ScriptHook` impl.
    #[rust]
    rewear: bool,
    /// Every width asked of a child since the row was made, `None` answers
    /// included. A running count rather than a per-frame one, so that
    /// keeping it is one add and a caller reads a frame's cost as the
    /// difference across that frame.
    #[rust]
    measures: u64,
    /// The children that could not say how wide they are when the row last
    /// priced itself, and never give way.
    #[rust]
    unpriced: Vec<LiveId>,
    /// What each of those drew at, margin included, read right after the row
    /// drew and not before. See [`ConcedingRow::span`].
    #[rust]
    drawn: Vec<(LiveId, f64)>,
}

impl ScriptHook for ConcedingRow {
    /// A reload re-applies every child from its markup, and a live edit's
    /// `Reload` puts a child's authored text back while leaving a
    /// visibility its markup does not state where a rung put it -- so after
    /// one the row can stand in a mix of faces that belongs to no level. The
    /// level itself has not moved, so nothing would put the faces right
    /// again: the next draw wears the level once more whatever it is.
    ///
    /// It used to be put right by accident: the app was laid out at full
    /// width on the frame after every reload, which moved the level and so
    /// re-wore it. That frame was the flicker, and it is gone.
    fn on_after_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        if apply.is_reload() {
            self.rewear = true;
        }
    }
}

impl ConcedingRow {
    /// How many rungs are in force: the level the row last drew at.
    pub fn level(&self) -> usize {
        self.applied
    }

    /// How many widths the row has asked of its children, in all. What a
    /// draw costs is the difference across it.
    pub fn measures(&self) -> u64 {
        self.measures
    }

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
    /// when a child ON A RUNG cannot price itself under those rungs.
    ///
    /// A child that gives way has to answer exactly, because what a rung saves
    /// is the difference its face makes and an inexact answer prices the rung
    /// wrong -- which is a rung the ladder never gives back.
    ///
    /// A child that never gives way is a different matter: it is the same width
    /// at every level, so it cancels out of `span(i) - span(i+1)` entirely and
    /// only moves the absolute number. For those, the width it last drew at
    /// will do, and a row full of widgets that cannot measure themselves still
    /// gets its rungs priced exactly. Before the first draw that width is zero,
    /// so a cold row reads narrower than it is and settles on the draw after --
    /// once, not per rung.
    ///
    /// That width is the one kept in `drawn`, read after the row drew. It
    /// cannot be read off the child's area here: pricing happens part way
    /// into a redraw, after the list the child draws into has been begun
    /// again, and every area in that list is a frame stale until it is drawn
    /// again -- a stale area answers a zero rect. Read here, the storybook's
    /// theme picker was worth nothing on every frame, and the row gave its
    /// title back into an overflow of the picker's whole width.
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
            let ranked = over.is_some();
            self.measures += 1;
            let w = match child.measure_width(cx, over.as_ref()) {
                Some(w) => w,
                None if ranked => return None,
                None => {
                    if !self.unpriced.contains(&id) {
                        self.unpriced.push(id);
                    }
                    self.drawn.iter().find(|(d, _)| *d == id).map_or(0.0, |(_, w)| *w)
                }
            };
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

        self.unpriced.clear();
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

    /// Keep what each child that cannot price itself has just drawn at, for
    /// the next pricing to count it at.
    ///
    /// Read now because now is the one moment its area is current: the row
    /// has just drawn it. Only those children, and only a rect each, so an
    /// ordinary frame asks no child for anything more than it did. A child
    /// that drew nothing -- a draw that stopped part way -- keeps the width
    /// it had rather than being counted as taking no room.
    fn keep_drawn_widths(&mut self, cx: &mut Cx) {
        for i in 0..self.unpriced.len() {
            let id = self.unpriced[i];
            let Some((_, child)) = self.view.children.iter().find(|(c, _)| *c == id) else {
                continue;
            };
            let child = child.clone();
            let width = child.area().rect(cx).size.x;
            if !(width > 0.0) {
                continue;
            }
            let outer = width + child.walk(cx).margin.width();
            match self.drawn.iter_mut().find(|(d, _)| *d == id) {
                Some((_, w)) => *w = outer,
                None => self.drawn.push((id, outer)),
            }
        }
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
        let rewear = std::mem::take(&mut self.rewear);
        if rewear || level != self.applied {
            self.wear(cx.cx, level);
            self.applied = level;
        }
        // One draw, at the level already settled. The deciding happened before
        // the drawing, so there is nothing here to ask for a redraw about.
        let step = self.view.draw_walk(cx, scope, walk);
        self.keep_drawn_widths(cx.cx);
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }
}

#[cfg(test)]
mod tests {
    //! A row on its own, drawn the way a window draws it but with no platform
    //! window under it: a test that makes its own graphics device takes the
    //! rest of the suite down with it.
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;

    struct Rig {
        cx: Cx,
        root: WidgetRef,
        pass: DrawPass,
        list: DrawList2d,
        /// What the row is built from, so that a rebuild builds it again.
        source: fn(&mut ScriptVm) -> ScriptValue,
    }

    fn rig(source: fn(&mut ScriptVm) -> ScriptValue) -> Rig {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let _ = crate::makepad_draw::makepad_platform::shader_error::take();
        let root = cx.with_vm(|vm| {
            let value = source(vm);
            assert!(vm.take_errors().is_empty(), "the row did not build");
            WidgetRef::script_from_value(vm, value)
        });
        let pass = DrawPass::new(&mut cx);
        let list = DrawList2d::new(&mut cx);
        Rig { cx, root, pass, list, source }
    }

    /// A title that goes first, a count its app leaves empty, and words that
    /// stay whatever happens.
    fn with_an_empty_count(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{
                width: Fill
                height: Fit
                flow: Down
                row := ConcedingRow{
                    spacing: 8.
                    title := Label{
                        text: "A title that can go"
                        width: 150.
                        give_up := 1
                        tight: { visible: false }
                    }
                    count := Label{ text: "" }
                    words := Label{ text: "Words that stay where they are" }
                }
            }
        })
    }

    /// The same title, beside a child that cannot say how wide it is -- a Fit
    /// view, whose width is its children's business -- and a stated end
    /// marker, whose drawn rect says where the row really ends.
    fn with_a_box(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{
                width: Fill
                height: Fit
                flow: Down
                row := ConcedingRow{
                    spacing: 8.
                    title := Label{
                        text: "A title that can go"
                        width: 150.
                        give_up := 1
                        tight: { visible: false }
                    }
                    boxed := View{
                        width: Fit
                        height: Fit
                        inner := View{ width: 60. height: 10. }
                    }
                    end := View{ width: 20. height: 10. }
                }
            }
        })
    }

    /// One frame at `width`, as a window draws one: the whole list again.
    fn frame(rig: &mut Rig, width: f64) {
        let size = dvec2(width, 100.0);
        let cx = &mut rig.cx;
        rig.pass.set_size(cx, size);
        cx.redraw_all();
        let event = std::mem::take(&mut cx.new_draw_event);
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&rig.pass, Some(1.0));
        rig.list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::flow_down());
        rig.root.draw_all(&mut cx2d, &mut Scope::empty());
        cx2d.end_pass_sized_turtle();
        rig.list.end(&mut cx2d);
        cx2d.end_pass(&rig.pass);
    }

    fn row(rig: &Rig) -> WidgetRef {
        rig.root.widget(&rig.cx, ids!(row))
    }

    fn level(rig: &Rig) -> usize {
        row(rig).borrow::<ConcedingRow>().expect("the row is a ConcedingRow").level()
    }

    fn measures(rig: &Rig) -> u64 {
        row(rig).borrow::<ConcedingRow>().expect("the row is a ConcedingRow").measures()
    }

    /// Draw a few frames at `width` and answer the level the row ends on.
    /// A cold row may take two: a child that cannot price itself has no
    /// drawn width until it has drawn once.
    fn settle(rig: &mut Rig, width: f64) -> usize {
        for _ in 0..3 {
            frame(rig, width);
        }
        level(rig)
    }

    /// A width at which the row has only just given its title up: a point
    /// narrower than the narrowest width at which it has the title back. A
    /// row this close to the edge gives the title back for any child that
    /// prices itself a point or two narrower than it draws.
    fn edge(rig: &mut Rig) -> f64 {
        let mut width = 300.0;
        while settle(rig, width) > 0 {
            width += 1.0;
            assert!(width < 1200.0, "the row never had its title back");
        }
        let edge = width - 1.0;
        assert_eq!(settle(rig, edge), 1, "a point narrower did not take the title again");
        edge
    }

    /// Build the row again from its template in a module run, and apply it
    /// the way a style reload does -- the landing of every theme install.
    fn rebuild(rig: &mut Rig) {
        let source = rig.source;
        let mut root = rig.root.clone();
        rig.cx.with_vm(|vm| {
            let value = vm.with_reload(|vm| {
                crate::script_mod(vm);
                source(vm)
            });
            root.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), value);
            assert!(vm.take_errors().is_empty(), "the rebuild did not apply cleanly");
        });
        let _ = crate::makepad_draw::makepad_platform::shader_error::take();
    }

    /// A row that had settled draws that same level on its first draw after
    /// its children are rebuilt from their template, and on every draw after.
    ///
    /// The rebuild on its own never moved it. What moved it is what an app
    /// does on the event that follows every rebuild: it writes its own state
    /// back into the rebuilt children, and the storybook writes an empty
    /// count back into a label that had drawn itself a space. An empty label
    /// priced at its padding alone is a label the row believes is a space
    /// narrower than it draws, so for one frame a row on its edge found room
    /// for its title -- the flash on every theme install.
    #[test]
    fn a_rebuilt_row_draws_the_level_it_had_settled_on() {
        let mut rig = rig(with_an_empty_count);
        let at = edge(&mut rig);
        for write_back in [false, true] {
            rebuild(&mut rig);
            if write_back {
                let count = rig.root.widget(&rig.cx, ids!(count));
                count.set_text(&mut rig.cx, "");
            }
            frame(&mut rig, at);
            assert_eq!(level(&rig), 1, "the first draw after the rebuild (write back {write_back}) moved the row");
            assert!(
                !rig.root.widget(&rig.cx, ids!(title)).visible(),
                "the first draw after the rebuild (write back {write_back}) had the title"
            );
            for _ in 0..3 {
                frame(&mut rig, at);
                assert_eq!(level(&rig), 1, "the row moved after the rebuild (write back {write_back})");
            }
        }
    }

    /// A child that cannot price itself is counted at the width it drew at.
    ///
    /// It was read off its area while the row priced, which is part way into
    /// a redraw: the list has been begun again, every area in it is a frame
    /// stale, and a stale area answers nothing. The child took no room as far
    /// as the row could tell, so the row gave its title back into an overflow
    /// of that child's whole width. Caught off the stated end marker, whose
    /// drawn rect is where the row really ends.
    #[test]
    fn a_child_that_cannot_price_itself_is_counted_at_the_width_it_drew() {
        let mut rig = rig(with_a_box);
        let mut width = 250.0;
        while width < 700.0 {
            if settle(&mut rig, width) == 0 {
                let row = row(&rig).area().rect(&rig.cx);
                let end = rig.root.widget(&rig.cx, ids!(end)).area().rect(&rig.cx);
                assert!(
                    end.pos.x + end.size.x <= row.pos.x + row.size.x + 0.5,
                    "at {width} the row kept its title and ran {} past its own end",
                    end.pos.x + end.size.x - (row.pos.x + row.size.x)
                );
            }
            width += 2.0;
        }
    }

    /// What an ordinary frame costs the row: one width asked of each child
    /// at each level, less the children a level hides -- here three at level
    /// nought and two at level one, the title being gone. Pinned, so that
    /// remembering what a child drew at stays a read of its rect and never
    /// becomes another round of asking.
    #[test]
    fn an_ordinary_frame_asks_each_child_once_per_level() {
        let mut rig = rig(with_a_box);
        for width in [900.0, 260.0] {
            settle(&mut rig, width);
            let before = measures(&rig);
            frame(&mut rig, width);
            assert_eq!(measures(&rig) - before, 5, "a frame at {width} asked more than it did");
        }
    }
}
