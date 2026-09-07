//! The grid stories: track lists, named areas, and the column count that
//! answers to the width rather than to a number someone typed.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let GCell = RoundedView{
        show_bg: true
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: theme.color_surface_container_high border_radius: theme.radius_s}
    }
    let GAccent = GCell{draw_bg +: {color: theme.color_primary_container}}
    let GLabel = Label{width: Fit draw_text.color: theme.color_text_meta}
    let GNote = Label{width: Fill draw_text.color: theme.color_text_meta}

    mod.stories.GridOverview = StoryPage{
        StoryNote{text: "A grid is told its TRACKS, not its children's sizes. Each track is a length, a share of what is left, or a range; the cells fall into them. Drag the width and watch which of those answers move."}

        StoryRow{
            grid_width := Slider{
                width: 300.
                text: "Grid width"
                min: 300.0
                max: 900.0
                default: 840.0
                step: 10.0
            }
            grid_note := Label{text: "840 points"}
        }

        gframe := View{
            width: 840.
            height: Fit
            flow: Down
            spacing: theme.space_3

            GNote{text: "Track kinds: a fixed 90, a fifth of the width, a range that never goes under 80, and two equal shares of the rest"}
            Grid{
                width: Fill height: 52.
                column_gap: 8. row_gap: 8.
                columns: ["90px", "20%", "minmax(80px, 1fr)", "repeat(2, minmax(40px, 1fr))"]
                rows: ["1fr"]
                GCell{GLabel{text: "90px"}}
                GCell{GLabel{text: "20%"}}
                GCell{GLabel{text: "minmax"}}
                GCell{GLabel{text: "1fr"}}
                GCell{GLabel{text: "1fr"}}
            }

            GNote{text: "The responsive one: repeat(auto-fit, minmax(150px, 1fr)) — the column count answers to the width, and the cells share what is left over"}
            Grid{
                width: Fill height: Fit
                column_gap: 8. row_gap: 8.
                columns: ["repeat(auto-fit, minmax(150px, 1fr))"]
                // Rows are implicit here: the cell count decides how many
                // there are, so the grid is told how TALL one is rather
                // than being given a row list it cannot know the length of.
                implicit_row_size: 44.
                GCell{GLabel{text: "1"}}
                GCell{GLabel{text: "2"}}
                GCell{GLabel{text: "3"}}
                GCell{GLabel{text: "4"}}
                GCell{GLabel{text: "5"}}
                GCell{GLabel{text: "6"}}
                GCell{GLabel{text: "7"}}
                GCell{GLabel{text: "8"}}
            }

            GNote{text: "auto-fill against auto-fit: the same request, except auto-fill KEEPS the columns it cannot fill and auto-fit collapses them, so the cells share the width"}
            View{width: Fill height: Fit flow: Down spacing: theme.space_1
                Grid{
                    width: Fill height: Fit
                    column_gap: 8. row_gap: 8.
                    columns: ["repeat(auto-fill, minmax(150px, 1fr))"]
                    implicit_row_size: 34.
                    GCell{GLabel{text: "auto-fill"}}
                    GCell{GLabel{text: "two"}}
                }
                Grid{
                    width: Fill height: Fit
                    column_gap: 8. row_gap: 8.
                    columns: ["repeat(auto-fit, minmax(150px, 1fr))"]
                    implicit_row_size: 34.
                    GCell{GLabel{text: "auto-fit"}}
                    GCell{GLabel{text: "two"}}
                }
            }

            GNote{text: "Down the columns instead: auto_flow decides which way an unplaced cell walks"}
            Grid{
                width: Fill height: 84.
                column_gap: 8. row_gap: 8.
                columns: ["repeat(4, minmax(60px, 1fr))"]
                rows: ["1fr", "1fr"]
                auto_flow: AutoFlow.Column
                GCell{GLabel{text: "1"}}
                GCell{GLabel{text: "2"}}
                GCell{GLabel{text: "3"}}
                GCell{GLabel{text: "4"}}
                GCell{GLabel{text: "5"}}
                GCell{GLabel{text: "6"}}
            }

            GNote{text: "Named areas: the shape is drawn in the areas list and each child says which name it wants"}
            Grid{
                width: Fill height: 150.
                column_gap: 8. row_gap: 8.
                columns: ["180px", "1fr"]
                rows: ["40px", "1fr"]
                areas: ["head head", "side body"]
                GAccent{cell: CellPlacement{area: @head} GLabel{text: "head"}}
                GCell{cell: CellPlacement{area: @side} GLabel{text: "side"}}
                GCell{cell: CellPlacement{area: @body} GLabel{text: "body"}}
            }

            GNote{text: "Placement: a cell can name its column and row, or span, or say nothing and take the next free one"}
            Grid{
                width: Fill height: 110.
                column_gap: 8. row_gap: 8.
                columns: ["repeat(4, minmax(60px, 1fr))"]
                rows: ["1fr", "1fr"]
                GAccent{cell: CellPlacement{col: 3 row: 1} GLabel{text: "col 3, row 1"}}
                GCell{cell: CellPlacement{col: 4 row: 1 row_span: 2} GLabel{text: "two rows"}}
                GCell{cell: CellPlacement{col_span: 2} GLabel{text: "spans two"}}
                GCell{GLabel{text: "auto"}}
                GCell{GLabel{text: "auto"}}
            }
        }
    }
}

fn grid_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let Some(w) = root.slider(cx, ids!(grid_width)).slided(actions) else {
        return;
    };
    let mut frame = root.widget(cx, ids!(gframe));
    script_apply_eval!(cx, frame, { width: #(w) });
    root.label(cx, ids!(grid_note)).set_text(cx, &format!("{w:.0} points"));
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/grid/overview",
    category: "Containers",
    component: "Grid",
    name: "Overview",
    dsl: "GridOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Grid\n\nA grid is told its TRACKS and the cells fall into them. That is the whole idea, and it is why a grid answers a changing width without anyone recomputing anything.\n\n**A track is one of four things.** A length, `\"90px\"`. A share of the container, `\"20%\"`. A share of what is left after the fixed ones are paid, `\"1fr\"`. Or a range, `\"minmax(80px, 1fr)\"`, which is a share that refuses to go under a floor. `repeat(n, ...)` writes the same track several times.\n\n**`repeat(auto-fit, minmax(150px, 1fr))` is the responsive one**, and it is worth understanding rather than copying. It does not mean \"150 wide\". It means: fit as many columns as will hold 150, then let them share the remainder equally. So the cells stay legible and the COLUMN COUNT is what changes, which is what a catalogue of cards wants and what a fixed column count never gives.\n\n**`areas` draws the shape in words.** One string per row, one name per cell, `.` for an empty one, and a child says `cell: CellPlacement{area: @body}`. A name repeated across neighbouring cells is a span, so a header across the top is `\"head head\"` and nothing else.\n\n**Placement is explicit, spanning, or automatic.** `CellPlacement{col: 3 row: 1}` puts a child exactly there, `col_span` widens it, and a child that says nothing takes the next free cell in `auto_flow` order. Mixing the three is normal: pin what matters and let the rest fall in.",
    subject: "gframe",
    feature: None,
    controls: &[],
    on_actions: Some(grid_actions),
}];
