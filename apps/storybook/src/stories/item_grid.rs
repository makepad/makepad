//! The item grid story: a thousand things to choose from, of which only the
//! rows on screen exist. The page is one driver widget used four times over,
//! so that the difference between the walls on it is a property and not a
//! different loop — which is the claim the page makes about the widget.
use crate::makepad_widgets::item_grid::ItemGrid;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // A grid is Fill by nature, and Fill inside a scrolling page comes out
    // as nothing at all, so every wall here stands in a box with a real
    // height. The face of the box matters further down: the empty wall only
    // reads as an empty wall if the space it does not fill can be seen.
    let PickerBox = RoundedView{
        width: Fill
        height: 300.
        padding: theme.mspace_1
        draw_bg +: {
            color: theme.color_surface_container_high
            border_radius: theme.radius_m
        }
    }

    let PickerNote = Label{width: Fill draw_text.color: theme.color_text_meta}

    mod.storybook.StoryItemPickerBase = #(StoryItemPicker::register_widget(vm))

    // The driver holds no grid of its own: every wall on the page adds its
    // own as a child, so the four walls differ in the DSL and in nothing
    // else. What the widget does is fill whatever grid is inside it, and
    // that loop is the same four times over.
    mod.storybook.StoryItemPicker = set_type_default() do mod.storybook.StoryItemPickerBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2

        /** how many items the wall is told it holds */
        items: 1000
    }

    mod.stories.ItemGridOverview = StoryPage{
        StoryNote{text: "A thousand things to choose from, laid out across the width. The layout is a tile list, whole, held as a child — the rows scroll in a portal list, so only the rows on screen exist. What this widget adds is the part a host cannot write for itself: which items are chosen, where the keyboard is standing, and an answer that names an ITEM."}

        StoryHeading{text: "Pick one, pick several"}
        StoryNote{text: "A plain press takes one item and drops the rest. Shift sweeps the run between the last press and this one, and the primary key — command or control, whichever this machine calls primary — flips one item and leaves the others alone. The line under the wall is what the grid answered: an item index, plus how many are chosen and how many go across."}
        wall := mod.storybook.StoryItemPicker{
            PickerBox{
                subject := ItemGrid{
                    width: Fill
                    height: Fill
                    min_item_width: 150.
                    column_gap: 8.
                    row_gap: 8.
                }
            }
            report := PickerNote{text: "nothing picked yet"}
        }
        StoryNote{text: "Press a face, then use the arrow keys. Left and right step one item; up and down step a whole ROW, and a row is however many items across the grid settled on at this width — narrow the panel and the same key moves by a different number. Home and End are the ends of the set, not of a row; the page keys move by the rows on screen; Return opens the item under the ring; the space bar flips it. The ring is the grid's own: a face is recycled as it scrolls and the reader's place in the set is not."}

        StoryHeading{text: "Or one at a time"}
        StoryNote{text: "`multi: false` is a grid that holds one answer. A sweep has nothing to span there, so every gesture but a toggle lands on the item under the finger — and the toggle survives, because otherwise letting go of the single answer is unreachable."}
        single := mod.storybook.StoryItemPicker{
            items: 60
            PickerBox{
                height: 220.
                one := ItemGrid{
                    width: Fill
                    height: Fill
                    min_item_width: 150.
                    column_gap: 8.
                    row_gap: 8.
                    multi: false
                }
            }
            report := PickerNote{text: "nothing picked yet"}
        }

        StoryHeading{text: "A number across, when the count is part of the design"}
        StoryNote{text: "`columns: 3` is three across at any width, and the arrow keys follow it: down is three items, because down is a row. A wall that has to keep its shape — beside a fixed sidebar, or in a panel narrow enough that a derived count would flicker between two and three as the window moves — wants the number rather than the floor."}
        three := mod.storybook.StoryItemPicker{
            items: 40
            PickerBox{
                height: 240.
                fixed := ItemGrid{
                    width: Fill
                    height: Fill
                    columns: 3
                    column_gap: 10.
                    row_gap: 10.
                }
            }
        }

        StoryHeading{text: "What the host still owns"}
        StoryNote{text: "The data. The grid holds no items — it holds which of them are chosen — so the loop is `next_cell` until it answers `None`, and each answer carries the item index together with what is true of that item NOW: whether it is chosen, whether the keys are standing on it. Set the face from those two on every draw. They are handed over rather than left to be remembered because the face is recycled and the truth is not, and a tick left behind in a reused face is the classic way a grid like this goes wrong."}
        StoryNote{text: "The face does not answer the press. The grid tests the pointer against the items it put on screen and claims a press on one before the face sees it, so a face's own press handling never runs; the face the grid ships is a row with `interactive: false` so that it does not look pressable. The pick is made on the release, and only when the release was a tap: a finger that went on to scroll chose nothing, and a finger held on an item flips it. It must draw something of its own as well: where an item landed is read back from the face's area, so a face that painted nothing has no rect and that one item quietly stops answering the pointer."}

        StoryHeading{text: "Nothing in it"}
        StoryNote{text: "A wall of no items draws its background and stops — no message, no apology, exactly as the lists do and for the same reason: the widget cannot know why it is empty and the reader has to. Nothing yet, nothing matched, not allowed, the fetch failed. Put an `EmptyState` in the space the wall would have filled."}
        empty := mod.storybook.StoryItemPicker{
            items: 0
            PickerBox{
                height: 120.
                blank := ItemGrid{
                    width: Fill
                    height: Fill
                    min_item_width: 150.
                }
            }
        }
    }
}

/// A wall of numbered items. The count is a property so that the four walls
/// on the page can be one widget: the loop below is the whole of what a host
/// writes, and it does not change with the column count or the selection
/// mode, which is the point being made.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryItemPicker {
    #[deref]
    view: View,
    #[live(1000)]
    items: usize,
}

impl Widget for StoryItemPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut grid) = step.borrow_mut::<ItemGrid>() else {
                continue;
            };
            grid.set_item_count(cx, self.items);
            // Fill, and only fill. The faces of a row are drawn together
            // once that row's last slot has been handed out, so a
            // `draw_all` here would draw them a second time and in the
            // wrong turtle.
            while let Some(cell) = grid.next_cell(cx) {
                // Both flags, from the handout, every draw. What the face
                // was told last time is true of the widget and no longer
                // true of the item now standing in it — which is why the
                // mark below has to be written and unwritten rather than
                // left where it was put.
                let mark = if cell.cursor { " — under the keys" } else { "" };
                cell.widget
                    .set_text(cx, &format!("item {}{}", cell.index, mark));
                cell.widget
                    .as_list_item()
                    .set_selected(cx, cell.selected);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// Report what the grid said, in the grid's own terms: an item index, and
/// what the answer amounts to.
fn say(cx: &mut Cx, root: &WidgetRef, grid: &[LiveId], label: &[LiveId], actions: &Actions) {
    let grid = root.item_grid(cx, grid);
    // Asked in the order a reader would: opening an item outranks the press
    // that chose it, and a walk that chose nothing is not an answer.
    let line = if let Some(index) = grid.activated(actions) {
        format!("opened item {index}")
    } else if let Some(index) = grid.picked(actions) {
        format!(
            "picked item {index}: {} chosen, at {} across",
            grid.chosen().len(),
            grid.column_count()
        )
    } else if let Some(index) = grid.cursor_moved(actions) {
        format!("cursor on item {index}, and nothing chosen by the walk")
    } else {
        return;
    };
    root.label(cx, label).set_text(cx, &line);
}

fn item_grid_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // Each wall is asked about its own grid rather than the page scanning
    // every action: the wall further down must not answer for this one.
    say(cx, root, ids!(wall.subject), ids!(wall.report), actions);
    say(cx, root, ids!(single.one), ids!(single.report), actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "collections/tilelist/item-grid",
    category: "Collections",
    component: "TileList",
    also: &["ItemGrid"],
    name: "Item grid",
    dsl: "ItemGridOverview",
    added: "2026-09-11",
    tags: &[
        "new",
        "picker",
        "selection",
        "keyboard",
        "gallery",
        "thumbnails",
        "virtualisation",
        "reflow",
    ],
    doc: "# ItemGrid\n\nA picker of many items: a grid that reflows to its width, keeps which items are chosen, and answers the arrow keys.\n\n**It is not a second tile list; it owns one.** The layout — rows in a portal list, N items across each row, the column count derived from the width — is `TileList`, held whole as a child, and none of it is repeated here. What that widget says of itself is that it keeps nothing about an item (rows are recycled, so a face told it was selected stays selected under the next index that lands in it) and that it has no keyboard. That is the right division for a layout and the wrong one for a picker, because the two things a picker needs are the two a host is worst placed to supply: a selection that outlives the widget a recycled row hands back, and arrow keys that move by the column count, which is this draw's answer to a width nobody outside the draw has measured.\n\n## What it reports\n\nEvery answer is an item index.\n\n| Action | Raised by |\n|---|---|\n| `Picked(index)` | a press, or an arrow that took what it landed on |\n| `Activated(index)` | a second press, or Return on the item under the ring |\n| `CursorMoved(index)` | a walk with the primary key held, which chose nothing |\n\nNothing reports a row and a slot, and nothing asks a host to work out `row * columns + slot`. The column count belongs to the draw, so a host deriving an index from a count of its own would be right until the first resize. `picked`, `activated` and `cursor_moved` read the actions; `chosen()` is the whole answer in item order, `cursor()` the item the keys are standing on, and `set_chosen` puts one back from outside — a restore, or a host that owns the truth.\n\n## The keyboard\n\n| Key | Moves |\n|---|---|\n| left, right | one item |\n| up, down | one ROW, which is the column count as it stands |\n| Home, End | the ends of the set, not of a row |\n| page up, page down | the rows on screen |\n| Return | opens the item under the ring |\n| space | flips it |\n| primary + A | takes the lot |\n\nShift sweeps and the primary key toggles. A sweep runs in ITEM order and never as a rectangle: a rectangle would be the wrong answer the moment the window changed width, because the same two ends would enclose a different set of items and a reader who swept twelve things would find they had nine.\n\nThe cursor is scrolled into view when it is not already there, and only then — after an arrow walk, and after a `set_chosen` from outside, which is a restore and has to show the reader what came back. The layout scrolls by rows and puts the row it is asked for at the top, so revealing on every key press would jerk the whole grid up a row at a time under a reader who was only stepping sideways. Taking the lot with primary + A scrolls nowhere: everything is chosen, including whatever is on screen.\n\n## Driving it\n\n```\nwhile let Some(step) = self.view.draw_walk(cx, scope, walk).step() {\n    let Some(mut grid) = step.borrow_mut::<ItemGrid>() else { continue };\n    grid.set_item_count(cx, 1000);\n    while let Some(cell) = grid.next_cell(cx) {\n        let mark = if cell.cursor { \" — under the keys\" } else { \"\" };\n        cell.widget.set_text(cx, &format!(\"item {}{}\", cell.index, mark));\n        cell.widget.as_list_item().set_selected(cx, cell.selected);\n    }\n}\n```\n\n**Set the face from the handout, every draw.** Each answer carries `selected` and `cursor` alongside the index because the face is recycled and the truth is not: what a face was told last time is true of the WIDGET and no longer true of the item standing in it. **Fill the face; do not draw it** — the faces of a row are drawn together, in the row's own turtle, once that row's last slot has gone out. **Run the loop to `None`**, or the faces you did not fill are still showing whatever the last item to stand in them wrote.\n\n**The face must not answer the press.** The grid hit-tests the pointer against the items it actually put on screen — read back from where the faces landed, not worked out from the arithmetic a second time — and claims a press that lands on one before the layout sees it. Whoever claims a press first gets it, so a face's own press handling never runs and the pick is the grid's, made on the release: the face the grid ships is a row with `interactive: false` for that reason, and it has to draw a background of its own as well, because an item whose face painted nothing has no rect to be found under a pointer. A press anywhere else in the grid is left alone and reaches the layout, which is what keeps the rows flingable — and so is a press in the band along the right edge where the layout draws its scroll bar, because that bar is painted OVER the last column rather than beside it, and a grid claiming those pixels would leave the bar undraggable and not even hoverable. `bar_size` is how wide that band is: it lays nothing out, it only says how far in from the right edge the bar's own pixels reach.\n\n## How many across\n\nEither a number or a floor, and the same arithmetic the masonry and the tile list use, so a page that puts them side by side gets one answer about what a width holds.\n\n| Setting | Items across |\n|---|---|\n| `columns: 3` | three at any width |\n| `columns: 0` with `min_item_width: 150` | as many items of at least 150 as fit |\n\nThe least width is a floor on the COUNT and not on the width the items get: once the count is settled they share the row equally.\n\n## What it deliberately does not do\n\n**It holds no items.** The host owns the data and writes it into the face; this owns which of them are chosen. A set that shrinks lets go of the items that left — `set_item_count` is the moment that happens — because a selection holding indices nothing can show or clear again is a leak with a reader's work in it.\n\n**No rubber band, no reorder, no delete.** A picker answers what was picked. **No hover of its own** either: the ring says where the keys are and the face says what is chosen.\n\n**It is `Fill` by nature**, like the layout inside it. `Fill` in a `Fit` page resolves to nothing, so put it in a box with a real height.\n\nOne cost worth knowing: the selection model takes the display order on every gesture and keeps none of it, which is what lets a sorted or filtered list share it — so a press materialises one position per item. Thousands is nothing beside laying the rows out; millions would want a model that speaks in ranges.\n\nWhich container to use among the tile list, the item grid, the masonry and the grid is set out once, on TileList > Overview.",
    subject: "wall.subject",
    feature: None,
    controls: &[
        Control { label: "Across", target: "wall.subject", kind: ControlKind::Number { prop: "columns", min: 0., max: 8., step: 1., default: 0. } },
        Control { label: "Least item width", target: "wall.subject", kind: ControlKind::Number { prop: "min_item_width", min: 80., max: 400., step: 10., default: 150. } },
        Control { label: "Gap across", target: "wall.subject", kind: ControlKind::Number { prop: "column_gap", min: 0., max: 32., step: 2., default: 8. } },
        Control { label: "Gap down", target: "wall.subject", kind: ControlKind::Number { prop: "row_gap", min: 0., max: 32., step: 2., default: 8. } },
        Control { label: "Many at once", target: "wall.subject", kind: ControlKind::Bool { prop: "multi", default: true } },
    ],
    on_actions: Some(item_grid_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::id_path;

    /// The page is markup, which the compiler never reads: the widget it
    /// documents is reached by name, the ring's shader is compiled nowhere
    /// else in the library, and every wall claims a shape in the DSL that
    /// no Rust ever sees. Building the page is what turns a mistake in any
    /// of that into a failed test rather than an empty box in the
    /// catalogue.
    #[test]
    fn the_page_builds_and_every_wall_is_the_grid_the_page_says_it_is() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(
            makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        // The subject the docs and the controls address, and the two labels
        // the page's own action handler writes to.
        for target in [story.subject, "wall.report", "single.report"]
            .into_iter()
            .chain(story.controls.iter().map(|control| control.target))
            .filter(|target| !target.is_empty())
        {
            assert!(
                !page.widget(&cx, &id_path(target)).is_empty(),
                "no widget at {target}"
            );
        }
        // Four walls and one driver: what differs between them is
        // properties, which is the page's whole claim.
        for (target, columns, multi) in [
            ("wall.subject", 0usize, true),
            ("single.one", 0, false),
            ("three.fixed", 3, true),
            ("empty.blank", 0, true),
        ] {
            let widget = page.widget(&cx, &id_path(target));
            let grid = widget
                .borrow::<ItemGrid>()
                .unwrap_or_else(|| panic!("{target} is not an ItemGrid"));
            assert_eq!(grid.columns, columns, "{target} is not laid out as the page says");
            assert_eq!(grid.multi, multi, "{target} does not hold the answers the page says");
        }
        // The face is written under the name the layout inside looks for. It
        // reads its own `Tile` template and says nothing about a missing
        // one until a draw, so a face written under any other name would
        // leave every box on this page empty with nothing failing.
        let has_face = cx.with_vm(|vm| {
            let widgets = vm.module(id!(widgets));
            let declared = vm.bx.heap.value(widgets, live_id!(ItemGrid).into(), NoTrap);
            let Some(declared) = declared.as_object() else {
                return false;
            };
            let layout = vm.bx.heap.value(declared, live_id!(grid).into(), NoTrap);
            let Some(layout) = layout.as_object() else {
                return false;
            };
            vm.vec_with(layout, |_vm, vec| {
                vec.iter().any(|kv| kv.key.as_id() == Some(live_id!(Tile)))
            })
        });
        assert!(has_face, "the layout inside the grid has no Tile face to build");
        for (target, items) in [("wall", 1000usize), ("single", 60), ("three", 40), ("empty", 0)] {
            let widget = page.widget(&cx, &id_path(target));
            let wall = widget
                .borrow::<StoryItemPicker>()
                .unwrap_or_else(|| panic!("{target} is not a wall"));
            assert_eq!(wall.items, items, "{target} is told it holds another number");
        }
    }
}
