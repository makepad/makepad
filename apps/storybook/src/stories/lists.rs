//! The lists story: the three list widgets, what each one is for, and what
//! none of them keeps. The feed asks for a thousand rows and holds a
//! screenful. The reorder demo owns the order and applies the move itself,
//! because that is the whole of the widget's contract: it reports where a row
//! was dropped and moves nothing.
use crate::makepad_widgets::flat_list::FlatList;
use crate::makepad_widgets::reorder_list::ReorderList;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // A list is Fill by nature and Fill inside a scrolling page resolves to
    // nothing at all, so every list here sits in a box with a real height.
    // The face is part of the point: the EMPTY list further down is only
    // legible as an empty list if the box it does not fill can be seen.
    let ListBox = RoundedView{
        width: Fill
        height: 168.
        draw_bg +: {
            color: theme.color_surface_container_low
            border_radius: theme.radius_m
        }
    }

    mod.storybook.StoryFlatRowsBase = #(StoryFlatRows::register_widget(vm))

    mod.storybook.StoryFlatRows = set_type_default() do mod.storybook.StoryFlatRowsBase{
        width: Fill
        height: Fit
        flow: Down

        /** hand the list its rows; false leaves the model empty */
        filled: true

        ListBox{
            flat := FlatList{
                width: Fill height: Fill
                Row := View{
                    width: Fill height: Fit
                    padding: theme.mspace_1
                    label := Label{text: ""}
                }
            }
        }
    }

    mod.storybook.StoryFeedBase = #(StoryFeed::register_widget(vm))

    mod.storybook.StoryFeed = set_type_default() do mod.storybook.StoryFeedBase{
        width: Fill
        height: Fill

        list := PortalList{
            width: Fill height: Fill
            scroll_bar: ScrollBar{}
            // One template, rows of different heights: each post is as tall
            // as its text, which is what the list has to measure as it goes.
            Post := View{
                width: Fill height: Fit
                flow: Down
                padding: Inset{left: theme.space_2, right: theme.space_2, top: theme.space_2}
                text := P{text: ""}
                Divider{}
            }
        }
    }

    mod.storybook.StoryReorderRowsBase = #(StoryReorderRows::register_widget(vm))

    mod.storybook.StoryReorderRows = set_type_default() do mod.storybook.StoryReorderRowsBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2

        ListBox{
            height: 220.
            // The gripper is named here and pointed at: the list watches that
            // one child of every row and starts a reorder from a press on it,
            // so a drag anywhere else on the row still scrolls.
            reorder := ReorderList{
                width: Fill height: Fill
                drag_handle: @gripper
                Row := View{
                    width: Fill height: Fit
                    flow: Right
                    spacing: theme.space_2
                    align: Align{y: 0.5}
                    padding: theme.mspace_1
                    gripper := View{
                        width: Fit height: Fit
                        padding: theme.mspace_1
                        show_bg: true
                        draw_bg +: {color: theme.color_surface_container_high}
                        Label{text: "\u{2261}" draw_text +: {color: theme.color_text_meta}}
                    }
                    label := Label{text: ""}
                }
            }
        }
        // What the list said, in the list's own terms. It starts out saying
        // nothing, because until a row is dropped the list has said nothing.
        report := Label{text: "nothing reported yet"}
    }

    mod.stories.ListsOverview = StoryPage{
        StoryNote{text: "Six widgets show many items. Three of them are here: a list that draws every row, a list that draws only the rows on screen, and a list you can reorder. The table, the data grid and the tile list have pages of their own. What separates them is how much each one holds on your behalf, and the answer is never your rows."}

        StoryHeading{text: "A list that draws every row"}
        StoryNote{text: "`FlatList` is asked for each row by an id you choose, and it draws all of them: no visible range to work out, nothing recycled, no scroll arithmetic. Reach for it when the count is bounded by the design rather than by the data — a settings group, a legend, the ten rows below. Hand it a thousand expensive rows and it will draw a thousand expensive rows, on every frame."}
        flat_demo := mod.storybook.StoryFlatRows{}
        StoryNote{text: "The id is the row's identity. The widget built for it is kept and handed back, so what a row holds — a cursor in a text field, a half-typed number — survives the next draw, and nothing evicts it either. Ids that come and go with the data leave their widgets behind, so number rows by position unless per-row state has to follow the row rather than the place."}

        StoryHeading{text: "Many rows"}
        StoryNote{text: "`PortalList` is asked only for the rows on screen. It is given a range of ids, works out which of them fall inside its height at the current scroll, and hands those out one at a time to be filled and drawn. The feed below is a thousand posts long and holds a screenful."}
        ListBox{
            height: 320.
            feed_demo := mod.storybook.StoryFeed{}
        }
        StoryNote{text: "Rows need not be the same height. The list measures a row the first time it draws it and guesses the rest from the rows it has seen, so the scroll bar is an estimate that settles as the list is read. A row that scrolls out of view is let go and built again from its template when it comes back, so set everything a row shows on every draw, from its id."}

        StoryHeading{text: "A list you can reorder"}
        StoryNote{text: "`ReorderList` is a portal list with one addition, and it is driven exactly like one: same templates, same item range, same virtualisation, same item actions. What it adds is `drag_handle`, which names one child of the row template — the gripper here. A press on that child is taken before the inner list ever sees it, so a drag on the gripper reorders and a drag anywhere else on the row still scrolls. Four pixels of travel decide which: under that, a press on the gripper is still a click."}
        reorder_demo := mod.storybook.StoryReorderRows{}
        StoryNote{text: "Take hold of a gripper and move. A line marks the gap the row would land in, Escape abandons the gesture, the wheel is ignored while it lasts so the rows cannot slide out from under the pointer, and holding at the top or bottom edge crawls the list, so a long one can be reordered end to end in one gesture."}
        StoryNote{text: "On release it reports `from` and `to` and stops there — it moves nothing. The line under the list is this page removing that row and inserting it, which is all a host has to do: `to` arrives already adjusted for the removal, and adjusting it a second time lands every downward drag one row short of the gap it was dropped in. A drop back where the row started reports nothing, and neither does a press that never became a drag."}

        StoryHeading{text: "With no rows"}
        StoryNote{text: "A list with nothing in it paints its background and stops: no message, no placeholder, no apology. That is a division of labour rather than an oversight, because the list cannot know why it is empty and the reader has to. Nothing yet, nothing matched, not allowed, the fetch failed, no connection — five answers, and a blank box is none of them."}
        empty_demo := mod.storybook.StoryFlatRows{filled: false}
        StoryNote{text: "The same widget and the same draw loop as the first list, over an empty model. Put an `EmptyState` in the space the list would have filled and say which of the five this is."}
        ListBox{
            height: Fit
            EmptyStateNothingYet{}
        }

        StoryHeading{text: "Which one"}
        StoryNote{text: "`FlatList` when the rows are few and the count is bounded by the design: it draws them all and costs what they cost."}
        StoryNote{text: "`PortalList` when the rows are many or unbounded: it works out which of them are on screen, asks for those, and lets go of the rest, so a thousand posts cost a screenful."}
        StoryNote{text: "`ReorderList` when the order is itself the data — a playlist, a queue, a set of steps to run in turn. It is a portal list, so that choice is already made; what it adds is the gesture and the report."}
        StoryNote{text: "`Table` when the rows are a fixed set of records whose whole job is to line up and be read. It holds every row, so it suits tens of rows rather than millions, and a table with markup in it draws the markup."}
        StoryNote{text: "`DataGrid` when a row is columns that line up and can be selected, resized, moved about and edited: a spreadsheet rather than a list. Both axes are virtualised and it asks for one cell at a time."}
        StoryNote{text: "`TileList` when the items sit several across, as in a picker of thousands. Its rows are a portal list and each row is as many tiles as the width allows."}
    }
}

/// The rows both demos show. Ten, so that the box scrolls: a list that fits
/// inside its box demonstrates nothing about how either widget fills one.
const NAMES: &[&str] = &[
    "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliett",
];

/// Apply a reorder the list has only reported: take the row out at `from`,
/// put it back at `to`.
///
/// `to` arrives ALREADY adjusted for the removal — the widget did that when
/// it turned the drop slot into an index — so this must not adjust it again.
/// This page did, and every downward drag landed one row short of the gap it
/// was dropped in.
fn apply_move<T>(rows: &mut Vec<T>, from: usize, to: usize) -> bool {
    // `to` indexes the list with that row already taken out of it, so the
    // last place either index may name is the same one.
    if from >= rows.len() || to >= rows.len() || from == to {
        return false;
    }
    let row = rows.remove(from);
    rows.insert(to, row);
    true
}

/// A flat list over the names, or over nothing at all: the empty section of
/// the page is this same widget with `filled: false`, so what stands there is
/// a real list with a real row template and no rows.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryFlatRows {
    #[deref]
    view: View,
    #[live(true)]
    filled: bool,
}

impl StoryFlatRows {
    fn rows(&self) -> &'static [&'static str] {
        if self.filled {
            NAMES
        } else {
            &[]
        }
    }
}

impl Widget for StoryFlatRows {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<FlatList>() else {
                continue;
            };
            // Every row, every draw. There is no visible range to ask for and
            // nothing to recycle, so the loop runs over the whole model.
            for (i, name) in self.rows().iter().enumerate() {
                // Row ids are the caller's to choose, and they start at one
                // here because LiveId(0) is the id of nothing.
                let Some(row) = list.item(cx, LiveId(i as u64 + 1), live_id!(Row)) else {
                    continue;
                };
                row.label(cx, ids!(label)).set_text(cx, name);
                row.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// What the feed's posts say, picked by id. Four lengths, so the rows are four
/// heights and the list has something to measure.
const POSTS: &[&str] = &[
    "Only the rows inside the box exist at any moment, whichever of the thousand they are.",
    "A short one.",
    "A row as tall as its text. The list measures each row the first time it draws it, and until then it guesses from the rows it has already seen, so the scroll bar is an estimate that settles as you read. Scroll back up and this one is the height it was.",
    "Rows that leave the screen are let go, and built again from the template when they come back.",
];

/// The feed: a portal list of a thousand posts, each filled from its id as it
/// comes into view.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryFeed {
    #[deref]
    view: View,
}

impl Widget for StoryFeed {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<PortalList>() else {
                continue;
            };
            // The range is the whole feed; what the loop hands out is the
            // part of it that is on screen.
            list.set_item_range(cx, 0, 1000);
            while let Some(id) = list.next_visible_item(cx) {
                let row = list.item(cx, id, live_id!(Post));
                let text = format!("{}. {}", id + 1, POSTS[id % POSTS.len()]);
                row.label(cx, ids!(text)).set_text(cx, &text);
                row.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// The reorder demo. The order lives here because the list deliberately does
/// not keep it: the widget reports a drop and this applies it.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryReorderRows {
    #[deref]
    view: View,
    #[rust(NAMES.iter().map(|s| s.to_string()).collect::<Vec<String>>())]
    order: Vec<String>,
}

impl StoryReorderRows {
    /// Apply what the list reported, and answer with the row that moved.
    fn apply(&mut self, from: usize, to: usize) -> Option<String> {
        apply_move(&mut self.order, from, to).then(|| self.order[to].clone())
    }
}

impl Widget for StoryReorderRows {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            // Borrowed as a ReorderList, never as the PortalList it derefs
            // to: the concrete widget here IS the reorder list, a downcast to
            // the base matches nothing, and a host that asks for the base
            // draws no rows at all and is told nothing about why.
            let Some(mut list) = item.borrow_mut::<ReorderList>() else {
                continue;
            };
            // From here on it is driven exactly like the portal list it is.
            list.set_item_range(cx, 0, self.order.len());
            while let Some(id) = list.next_visible_item(cx) {
                let Some(name) = self.order.get(id) else {
                    continue;
                };
                let name = name.clone();
                let row = list.item(cx, id, live_id!(Row));
                row.label(cx, ids!(label)).set_text(cx, &name);
                row.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

fn lists_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let list = root.reorder_list(cx, ids!(reorder_demo.reorder));
    // The list's own reader rather than a scan of every action: it filters by
    // the list's uid, so a page carrying more than one list cannot pick up
    // the wrong one's report.
    let reported = {
        let borrowed = list.borrow();
        let Some(inner) = borrowed else { return };
        inner.reordered(actions)
    };
    let Some((from, to)) = reported else { return };
    let host = root.widget(cx, ids!(reorder_demo));
    let mut moved = None;
    if let Some(mut inner) = host.borrow_mut::<StoryReorderRows>() {
        moved = inner.apply(from, to);
    }
    let Some(name) = moved else { return };
    root.label(cx, ids!(reorder_demo.report)).set_text(
        cx,
        &format!("reported from {from} to {to}: this page moved {name}"),
    );
    host.redraw(cx);
}

pub const STORIES: &[Story] = &[Story {
    key: "collections/lists/overview",
    category: "Collections",
    component: "Lists",
    also: &["FlatList", "PortalList", "ReorderList"],
    name: "Overview",
    dsl: "ListsOverview",
    added: "2025-05-06",
    tags: &["list", "rows", "reorder", "drag", "gripper", "virtualisation", "empty", "ported"],
    doc: "# Lists

Six widgets show many items. Three of them are on this page: `FlatList`, `PortalList` and `ReorderList`. `Table`, `DataGrid` and `TileList` have pages of their own. What separates them is **how much each one holds on your behalf**, and the answer is never your rows.

## FlatList

Asked for each row by an id you choose, and it draws all of them. There is no visible range to compute, nothing to recycle and no scroll arithmetic: it is a scrolling box with your rows in it.

Reach for it when the count is bounded by the design rather than by the data — a settings group, a legend, a menu of twenty things. Hand it a thousand expensive rows and it draws a thousand expensive rows on every frame, which is not slow once, it is slow always.

**The id is the row's identity.** `item(cx, id, template)` builds the widget the first time and hands the same one back afterwards, so what a row holds — a cursor in a text field, a half-typed number — survives the next draw. Nothing evicts it, either: an id you stop asking for keeps its widget for the life of the list. Numbering rows by position keeps that map bounded, at the price of per-row state following the place rather than the row.

## PortalList

Asked only for the rows on screen. It is given a range of ids with `set_item_range(cx, 0, count)`, works out which of them fall inside its height at the current scroll, and hands those out one at a time: `while let Some(id) = list.next_visible_item(cx)`. For each, `item(cx, id, template)` gives the row's widget, which the host fills and draws. A thousand rows cost a screenful.

Rows need not be the same height. The list measures a row the first time it draws it and guesses the others from the average of the rows it has seen, so the scroll bar is an estimate that settles as the list is read.

**What it keeps is the window.** A row that scrolls out of view is let go and built again from its template when it comes back, so anything that row's widget held goes with it. `reuse_items: true` pools those widgets by template instead, and resets each one to its template before another row gets it. `keep_invisible: true` keeps every row it ever built. Either way, set everything a row shows on every draw, from its id.

## ReorderList

A `PortalList` that can also reorder itself. The portal list is its `#[deref]` base, so templates, scrolling, virtualisation and item actions all pass straight through, and it is driven exactly like one: `set_item_range`, then `next_visible_item` in a loop.

**Borrow it as a `ReorderList`, not as a `PortalList`.** The concrete widget is the reorder list; the portal list is the field it derefs to, and a downcast to the base matches nothing, silently, leaving a demo that draws no rows at all.

What it adds is one rule: `drag_handle` names **a child of the row template**, and a press on that child becomes a reorder instead of a scroll. The press is captured by the reorder list *before* the inner portal list sees it, so drag-to-scroll never fights the gesture — and a drag anywhere else on the row still scrolls, which is what you want. `drag_threshold`, four pixels by default, is where a click ends and a drag begins.

While the gesture lasts, an indicator line marks the gap the row would land in, Escape abandons it, the wheel is swallowed so the rows cannot slide out from under the pointer, and a pointer held at the top or bottom edge crawls the list, so a long list can be reordered end to end in one gesture. `drag_state()` answers with `(from, slot)` while you draw, for a host that wants to tint the row being carried.

### What it reports

**It moves nothing.** On release it raises `Reordered { from, to }` — read with `reordered(actions)` — carrying indices into the host's own range, and stops. The model is yours, the move is yours, the redraw is yours:

```
let row = rows.remove(from);
rows.insert(to, row);
```

`to` is **already adjusted for the removal**. Subtracting one again is the easy mistake, and it lands every downward drag one row short of the gap it was dropped in. A drop back where the row started reports nothing, and neither does a press that never passed the threshold.

The widget's own source records why no `Area` is cached anywhere inside it: a finger capture is keyed on the captured widget's area, every redraw remaps that capture to a fresh one, and any area a widget snapshots for itself goes stale on the first redraw — `hits` then fails `is_valid` and returns nothing for ever. The gripper's current area is re-resolved from the live row on every event instead.

## With no rows

All three draw no rows and stop. No message, no placeholder: the list cannot know *why* it is empty and the reader has to. Nothing yet, nothing matched, not allowed to see them, the fetch failed, the device is offline — five answers, and a blank rectangle is none of them. Put an `EmptyState` in the space the list would have filled.

## Which one

| Widget | Reach for it when | What it holds |
|---|---|---|
| `FlatList` | the rows are few and bounded by the design | every row widget, by your id, for the life of the list |
| `PortalList` | the rows are many or unbounded | the rows in view |
| `ReorderList` | the order is itself the data | the rows in view, plus one live gesture |
| `Table` | a fixed set of records to line up and read | every row, and one widget per widget cell |
| `DataGrid` | a row is columns that line up, select, resize and edit | a sparse size table for both axes |
| `TileList` | the items sit several across, as in a picker of thousands | the rows in view, each as many tiles as the width allows |",
    subject: "reorder_demo.reorder",
    feature: None,
    controls: &[],
    on_actions: Some(lists_actions),
}];

#[cfg(test)]
mod tests {
    use super::apply_move;

    fn rows() -> Vec<&'static str> {
        vec!["A", "B", "C", "D"]
    }

    #[test]
    fn a_reported_move_is_applied_exactly_as_reported() {
        // What the list reports for A dragged into the gap between C and D.
        let mut rows = rows();
        assert!(apply_move(&mut rows, 0, 2));
        // Adjusting `to` for the removal a second time would give B A C D:
        // the row one place short of the gap it was dropped in.
        assert_eq!(rows, ["B", "C", "A", "D"]);
    }

    #[test]
    fn dragging_upward_lands_on_the_index_reported() {
        let mut rows = rows();
        assert!(apply_move(&mut rows, 3, 0));
        assert_eq!(rows, ["D", "A", "B", "C"]);
    }

    #[test]
    fn a_move_that_moves_nothing_is_refused() {
        let mut rows = rows();
        assert!(!apply_move(&mut rows, 2, 2), "onto its own place");
        assert!(!apply_move(&mut rows, 4, 0), "no such row");
        assert!(
            !apply_move(&mut rows, 0, 4),
            "no such place: `to` indexes the shortened list"
        );
        assert_eq!(rows, ["A", "B", "C", "D"], "and nothing moved");
    }
}
