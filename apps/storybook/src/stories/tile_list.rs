//! The tile list story: a thousand things laid out across a width, of which
//! only the rows on screen exist. The page is one driver widget used four
//! times over, so that every wall on it is the same loop with a different
//! column setting — which is the claim the page makes.
use crate::makepad_widgets::tile_list::TileList;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // A tile list is Fill by nature, and Fill inside a scrolling page comes
    // out as nothing at all, so every wall here stands in a box with a real
    // height. The face of the box is part of the point: the empty wall
    // further down only reads as an empty wall if the space it does not
    // fill can be seen. Hence the HIGH container and not the low one the
    // list pages use — the low one lands within a few percent of the page
    // ground in all three themes (a step of 3 in 255 in the light theme,
    // 8 in the dark, 9 in the skeleton), which is enough for a box that
    // only has to hold rows and not enough for one that has to be seen
    // holding nothing.
    let TileBox = RoundedView{
        width: Fill
        height: 280.
        padding: theme.mspace_1
        draw_bg +: {
            color: theme.color_surface_container_high
            border_radius: theme.radius_m
        }
    }

    let TileNote = Label{width: Fill draw_text.color: theme.color_text_meta}

    mod.storybook.StoryTileWallBase = #(StoryTileWall::register_widget(vm))

    // The driver holds no tile list of its own: every wall on the page adds
    // its own as a child, so the four walls differ in the DSL and in
    // nothing else. What the widget does is fill whatever tile list is
    // inside it, and that loop is the same four times over.
    mod.storybook.StoryTileWall = set_type_default() do mod.storybook.StoryTileWallBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2

        /** how many items the wall is told it holds */
        items: 1000
    }

    mod.stories.TileListOverview = StoryPage{
        StoryNote{text: "A thousand things to pick from, laid out across the width. The rows scroll in a portal list, so only the rows on screen exist; each row is as many tiles across as the width allows. Neither half of that is new — what this widget is, is the join, plus the arithmetic that turns a row and a slot back into an item."}

        StoryHeading{text: "As many across as the width allows"}
        StoryNote{text: "`columns: 0` fits as many tiles of at least `min_tile_width` as there is room for, and is what makes a wall answer a resize. The least width is a floor on the COUNT and not on the width the tiles get: once the count is settled the tiles share the row equally, so a 150 minimum in a 700 point box gives four tiles of about 170. Drag the panel's least-width control and watch the count change under the same thousand items."}
        wall := mod.storybook.StoryTileWall{
            TileBox{
                subject := TileList{
                    width: Fill
                    height: Fill
                    min_tile_width: 150.
                    column_gap: 8.
                    row_gap: 8.
                    Tile := Button{
                        height: 64.
                        text: ""
                    }
                }
            }
            report := TileNote{text: "nothing picked yet"}
        }
        StoryNote{text: "Pick a tile. The line under the wall is what the widget answered, not what the page worked out: a tile raises an action knowing nothing about item numbers, and the list turns it back into an index by asking the row which slot it was. `tiles_with_actions` is the whole of the hit test."}

        StoryHeading{text: "One across is a list"}
        StoryNote{text: "`columns: 1` is the degenerate case, and it is worth knowing it is the same widget: a picker that reflows down to one column on a narrow window does not become a different container at the bottom of the range, it becomes this. The tiles are still widened to the row, so a one-across wall is a list of full-width rows."}
        rows := mod.storybook.StoryTileWall{
            TileBox{
                height: 200.
                narrow := TileList{
                    width: Fill
                    height: Fill
                    columns: 1
                    row_gap: 6.
                    Tile := Button{
                        height: 40.
                        text: ""
                    }
                }
            }
        }

        StoryHeading{text: "Or a number, when the count is part of the design"}
        StoryNote{text: "`columns: 3` is three across at any width. A wall that has to keep its shape — beside a fixed sidebar, or in a panel narrow enough that a derived count would flicker between two and three as the window moves — wants the number rather than the floor."}
        three := mod.storybook.StoryTileWall{
            items: 40
            TileBox{
                height: 220.
                fixed := TileList{
                    width: Fill
                    height: Fill
                    columns: 3
                    column_gap: 10.
                    row_gap: 10.
                    Tile := Button{
                        height: 56.
                        text: ""
                    }
                }
            }
        }

        StoryHeading{text: "What it does not keep"}
        StoryNote{text: "Nothing about an item. A row that scrolls off the top comes back as the row for some other set of items, with the same tile widgets inside it, so anything a tile was told stays true of the widget and stops being true of the item. Set every visual property of a tile from its index on every draw — the text, the state, whether it is selected, whether it is disabled. That it looked right last frame is not evidence, and a checkbox left ticked in a recycled tile is the classic way this goes wrong."}
        StoryNote{text: "The count is not kept either. `set_item_count` may be called during a draw, because the row range is not taken until the first tile is asked for; but the tiles come from the template on the instance, so a set of two kinds of thing wants one template that can be either, not two templates."}

        StoryHeading{text: "Nothing in it"}
        StoryNote{text: "A wall of no items draws its background and stops — no message, no apology, exactly as the lists do, and for the same reason: the widget cannot know why it is empty and the reader has to. Nothing yet, nothing matched, not allowed, the fetch failed. Put an `EmptyState` in the space the wall would have filled."}
        empty := mod.storybook.StoryTileWall{
            items: 0
            TileBox{
                height: 120.
                blank := TileList{
                    width: Fill
                    height: Fill
                    min_tile_width: 150.
                    Tile := Button{height: 64. text: ""}
                }
            }
        }
    }
}

/// A wall of numbered tiles. The count is a property so that the four walls
/// on the page can be one widget: the loop below is the whole of what a host
/// writes, and it does not change with the column count, which is the point
/// being made.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryTileWall {
    #[deref]
    view: View,
    #[live(1000)]
    items: usize,
}

impl Widget for StoryTileWall {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut tiles) = step.borrow_mut::<TileList>() else {
                continue;
            };
            tiles.set_item_count(cx, self.items);
            // Fill, and only fill. The tiles of a row are drawn together
            // once the row's last slot has been handed out, so a
            // `draw_all` here would draw them a second time, in the wrong
            // turtle.
            while let Some(tile) = tiles.next_tile(cx) {
                tile.widget.set_text(cx, &format!("{}", tile.index));
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// Report what was picked, in the widget's own terms: the item, and where it
/// was standing when it was picked.
fn tile_list_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let tiles = root.tile_list(cx, ids!(wall.subject));
    // The list's own reader rather than a scan of every action: it matches
    // the tile against the rows this list owns, so the wall further down the
    // page cannot answer for this one.
    let picked = tiles
        .tiles_with_actions(actions)
        .into_iter()
        .find(|(_, tile)| tile.as_button().clicked(actions));
    let Some((index, _)) = picked else {
        return;
    };
    let place = tiles
        .place_of(index)
        .map(|(row, column)| format!("row {row}, slot {column}"))
        .unwrap_or_else(|| "off the end of the set".to_string());
    root.label(cx, ids!(wall.report)).set_text(
        cx,
        &format!(
            "picked item {index}: {place}, at {} across",
            tiles.column_count()
        ),
    );
}

pub const STORIES: &[Story] = &[Story {
    key: "data display/tilelist/overview",
    category: "Data display",
    component: "TileList",
    also: &["TileRow"],
    name: "Overview",
    dsl: "TileListOverview",
    added: "2026-09-10",
    tags: &[
        "new",
        "grid",
        "gallery",
        "picker",
        "virtualisation",
        "thumbnails",
        "tiles",
        "reflow",
    ],
    doc: "# TileList\n\nA virtualised set of items laid out N across. `columns: 1` is a row list; `columns: 0` fits as many as the width allows.\n\n**This is the container for a picker of thousands.** The two halves of what it does already existed and could not be had together: the containers that lay items out across a width draw every child they hold, so a thousand of them is a thousand draws on every frame, and the containers that draw only what is on screen are one item wide. A tile list virtualises by ROW — the rows are a portal list, so only the rows on screen exist — and lays out N tiles inside each one.\n\n## How many across\n\nEither a number or a floor.\n\n| | |\n|---|---|\n| `columns: 3` | three across at any width |\n| `columns: 1` | a plain row list, the same widget |\n| `columns: 0` with `min_tile_width: 150` | as many tiles of at least 150 as fit |\n\nThe derived count is what makes a wall answer a changing width, and the count comes from the same arithmetic the masonry uses, so the two agree about what a width holds. Note what the least width is: a floor on the COUNT, not on the width the tiles get. Once the count is settled the tiles share the row equally, so a 150 minimum in a 700 point box gives four tiles of about 170.\n\nA tile list whose own width is indefinite — one inside a `Fit` parent — has nothing to divide, so it falls back to one across.\n\n## Driving it\n\n```\nwhile let Some(step) = self.view.draw_walk(cx, scope, walk).step() {\n    let Some(mut tiles) = step.borrow_mut::<TileList>() else { continue };\n    tiles.set_item_count(cx, 1000);\n    while let Some(tile) = tiles.next_tile(cx) {\n        tile.widget.set_text(cx, &format!(\"{}\", tile.index));\n    }\n}\n```\n\n**Fill the tile; do not draw it.** The tiles of a row are drawn together, in the row's own turtle, once that row's last slot has been handed out — a `draw_all` in the loop draws the tile a second time and in the wrong place. **Run the loop to `None`.** A host that stops early still gets every row drawn, but the tiles it did not fill are not blank: the rows are recycled, so an unfilled tile is still showing whatever the last item to stand in it wrote, and the wall ends in somebody else's items.\n\n`set_item_count` may be called during the draw: the row range is not taken until the first tile is asked for.\n\n## What it reports\n\nEvery answer from `next_tile` carries the flat `index` together with the `row` and `column` it landed in, and the `columns` this draw settled on. That is the point of the type. The column count is the widget's answer to a width the host has not measured, so a host that computed `row * columns + slot` for itself would be deriving an index from a number it does not have — right until the first resize.\n\nThe same goes for a click. A tile raises an action knowing nothing about item numbers; `tiles_with_actions(actions)` gives back `(index, widget)` pairs by asking the row which slot the tile was, and the slot plus the row is the item. `place_of(index)` runs it the other way for a host holding an index and wanting to know where it is on screen, at the count you have set and the column count the last draw settled on.\n\n`scroll_to_item(index)` puts the row an item is in at the top of the viewport. Which row that is, is worked out on the next draw and not at the moment of asking: it depends on how many go across, that depends on a width, and a host restoring a saved position — `set_item_count` then `scroll_to_item`, before anything has been drawn — has not given the widget one. An item past the end of the set lands on the last row.\n\nWhen the column count changes under a reader — a resized window, a dragged splitter — the row at the top of the viewport is retargeted so that the item at the top stays the item at the top. Row 40 of three across is item 120, which is row 30 of four; without that correction a resize teleports the reader by the difference.\n\n## What it deliberately does not do\n\n**It keeps nothing about an item.** Rows are recycled as they scroll and the tiles ride along inside them, so anything a tile was told stays true of the widget and stops being true of the item. Set every visual property of a tile from its index, on every draw. A checkbox left ticked in a recycled tile is the classic way this goes wrong, and it is not a bug in the list.\n\n**It does not size tiles to each other.** A row is as tall as its tallest tile and rows may differ, which is the difference between this and a table. Tiles need a definite or a `Fit` height: there is no row to fill against, so a `Fill` height collapses to nothing. Their width is not theirs to choose — a tile is widened to its share of the row, because a wall of unequal tiles is not a wall.\n\n**It is `Fill` by nature.** Inside a scrolling page `Fill` resolves to nothing, and a list laid out with no height is drawn with none. Put it in a box with a real height.\n\n**One template.** Tiles come from the `Tile := …` written on the instance, and there is only the one. A set of two kinds of thing wants one template that can be either, not two templates.\n\n## Which container\n\n| Widget | Reach for it when |\n|---|---|\n| `TileList` | many items, laid out across a width, all the same shape |\n| `Masonry` | items that keep their own heights and should pack |\n| `Grid` | a fixed arrangement whose cells line up in both directions |\n| `PortalList` | many items, one across, each as tall as it likes |",
    subject: "wall.subject",
    feature: None,
    controls: &[
        Control { label: "Across", target: "wall.subject", kind: ControlKind::Number { prop: "columns", min: 0., max: 8., step: 1., default: 0. } },
        Control { label: "Least tile width", target: "wall.subject", kind: ControlKind::Number { prop: "min_tile_width", min: 80., max: 400., step: 10., default: 150. } },
        Control { label: "Gap across", target: "wall.subject", kind: ControlKind::Number { prop: "column_gap", min: 0., max: 32., step: 2., default: 8. } },
        Control { label: "Gap down", target: "wall.subject", kind: ControlKind::Number { prop: "row_gap", min: 0., max: 32., step: 2., default: 8. } },
    ],
    on_actions: Some(tile_list_actions),
}];
