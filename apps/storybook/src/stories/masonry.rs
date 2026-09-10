//! The masonry story: columns that stay level because each item goes into
//! whichever one is shortest, and what that costs the order.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let MTile = RoundedView{
        width: Fill
        height: 110.
        show_bg: true
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: theme.color_surface_container_high border_radius: theme.radius_s}
    }
    let MAccent = MTile{draw_bg +: {color: theme.color_primary_container}}
    let MLabel = Label{width: Fit draw_text.color: theme.color_text_meta}
    let MNote = Label{width: Fill draw_text.color: theme.color_text_meta}

    mod.stories.MasonryOverview = StoryPage{
        StoryNote{text: "Each item keeps the height it asked for, and the next item is put into whichever column is shortest at that moment. That is what closes the ragged band of nothing a grid of uneven cards leaves along the bottom of every row."}

        StoryHeading{text: "A wall of uneven cards"}
        subject := Masonry{
            width: Fill
            height: Fit
            min_column_width: 180.
            column_gap: 12.
            row_gap: 12.
            MTile{height: 150. MLabel{text: "150"}}
            MTile{height: 90. MLabel{text: "90"}}
            MTile{height: 120. MLabel{text: "120"}}
            MTile{height: 70. MLabel{text: "70"}}
            MTile{height: 160. MLabel{text: "160"}}
            MTile{height: 100. MLabel{text: "100"}}
            MTile{height: 130. MLabel{text: "130"}}
            MTile{height: 80. MLabel{text: "80"}}
            MTile{height: 110. MLabel{text: "110"}}
        }

        StoryHeading{text: "The width decides how many columns there are"}
        StoryNote{text: "`columns: 0` derives the count from `min_column_width`: as many columns as will hold that width. The columns then share the whole container equally, so the minimum sets the COUNT and not the width the columns end up with. Drag the width and watch the block reflow."}
        StoryRow{
            wall_width := Slider{
                width: 300.
                text: "Container width"
                min: 220.0
                max: 860.0
                default: 820.0
                step: 10.0
            }
            width_note := Label{text: "820 points"}
        }
        mframe := View{
            width: 820.
            height: Fit
            flow: Down
            spacing: theme.space_2
            MNote{text: "min_column_width: 200 — four columns at 860, three at 820, two at 600, one at 380"}
            Masonry{
                width: Fill
                height: Fit
                min_column_width: 200.
                column_gap: 10.
                row_gap: 10.
                MTile{height: 100. MLabel{text: "1"}}
                MTile{height: 60. MLabel{text: "2"}}
                MTile{height: 140. MLabel{text: "3"}}
                MTile{height: 80. MLabel{text: "4"}}
                MTile{height: 110. MLabel{text: "5"}}
                MTile{height: 70. MLabel{text: "6"}}
            }
        }

        StoryHeading{text: "Or a number, when the count is part of the design"}
        StoryNote{text: "`columns: 2` is two columns at any width. A block that has to keep its shape — beside a fixed sidebar, or on a printed page — wants the number rather than the floor."}
        Masonry{
            width: Fill
            height: Fit
            columns: 2
            column_gap: 10.
            row_gap: 10.
            MTile{height: 90. MLabel{text: "1"}}
            MTile{height: 140. MLabel{text: "2"}}
            MTile{height: 70. MLabel{text: "3"}}
            MTile{height: 110. MLabel{text: "4"}}
        }

        StoryHeading{text: "What the packing costs the order"}
        StoryNote{text: "Item 2 is tall, so items 3 to 6 all go under item 1 in the left column and the right column holds nothing else. The numbers no longer march across the rows. Filling the shortest column is exactly what closes the gaps, and the shortest column is rarely the next one along — strict order and tight packing cannot both be had, and this container chooses packing."}
        Masonry{
            width: Fill
            height: Fit
            columns: 2
            column_gap: 10.
            row_gap: 10.
            MTile{height: 40. MLabel{text: "1"}}
            MAccent{height: 200. MLabel{text: "2"}}
            MTile{height: 40. MLabel{text: "3"}}
            MTile{height: 40. MLabel{text: "4"}}
            MTile{height: 40. MLabel{text: "5"}}
            MTile{height: 40. MLabel{text: "6"}}
        }

        StoryHeading{text: "The two gaps are separate"}
        StoryNote{text: "`column_gap` runs between columns and `row_gap` between the items down one. They are not the same measurement: a wide column gap keeps the columns readable as columns, while the row gap is the rhythm of one column and is usually the smaller of the two."}
        Masonry{
            width: Fill
            height: Fit
            columns: 3
            column_gap: 28.
            row_gap: 4.
            MTile{height: 60. MLabel{text: "1"}}
            MTile{height: 90. MLabel{text: "2"}}
            MTile{height: 50. MLabel{text: "3"}}
            MTile{height: 70. MLabel{text: "4"}}
            MTile{height: 50. MLabel{text: "5"}}
            MTile{height: 80. MLabel{text: "6"}}
        }

        StoryHeading{text: "Nothing in it"}
        StoryNote{text: "An empty masonry is a block of no height, not a block of empty columns."}
        View{
            width: Fill
            height: Fit
            flow: Down
            MNote{text: "above the empty one"}
            Masonry{width: Fill height: Fit columns: 3}
            MNote{text: "and directly below it"}
        }
    }
}

fn masonry_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let Some(width) = root.slider(cx, ids!(wall_width)).slided(actions) else {
        return;
    };
    let mut frame = root.widget(cx, ids!(mframe));
    script_apply_eval!(cx, frame, { width: #(width) });
    root.label(cx, ids!(width_note))
        .set_text(cx, &format!("{width:.0} points"));
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/masonry/overview",
    category: "Containers",
    component: "Masonry",
    also: &[],
    name: "Overview",
    dsl: "MasonryOverview",
    added: "2026-09-10",
    tags: &["new", "layout", "columns", "packing", "cards", "waterfall", "staggered", "gallery"],
    doc: "# Masonry\n\nColumns of items that each keep their own height. The next item goes into whichever column is shortest at that moment, so the columns stay level with one another and the gaps close.\n\n**This is the layout for uneven content.** A grid gives every cell in a row the height of the tallest one, which is right when the cells are a set — a week of days, a row of prices — and wrong when they are a pile: photographs of different shapes, notes of different lengths, cards whose text runs to two lines or to six. A grid of those leaves a ragged band of nothing along the bottom of every row. A masonry has no rows for it to leave.\n\n## Strict order and tight packing cannot both be had\n\nSay it plainly, because it is the one thing to decide before using this container: **it will move items past one another.** Filling the shortest column is what closes the gaps, and the shortest column is rarely the next one along. Put a tall item second and the four after it all stack under the first, in the other column — the numbers stop marching across the rows.\n\nThe rule is one pass, in order: each item takes the shortest column as things stand when it is reached, and nothing is shuffled afterwards to even the columns up. That keeps the drift local, so an item is never far from where it was written, and it keeps the layout stable, so adding an item at the end cannot rearrange the ones before it. Perfect packing would mean sorting by height, and then the order would be gone altogether. If the order carries meaning — a ranking, a sequence of steps, anything numbered — this is the wrong container.\n\n## How many columns\n\nEither a number or a floor.\n\n| | |\n|---|---|\n| `columns: 3` | three columns at any width |\n| `columns: 0` with `min_column_width: 200` | as many columns of at least 200 as fit |\n\nThe derived count is what makes the block answer a changing width. Note what `min_column_width` is: a minimum for the COUNT, not the width the columns get. Once the count is settled the columns share the whole container equally, so at 820 points a 200 minimum gives three columns of about 266.\n\nA masonry whose own width is indefinite — one inside a `Fit` parent — has nothing to divide, so it falls back to a single column of `min_column_width`. Give it a definite width if you want more than one.\n\n## What the items must be\n\nAn item is widened to its column, and keeps its authored height. It needs a definite or a `Fit` height: there is no row to fill against, so a `Fill` height collapses to nothing. Hidden items leave no hole — the columns never hear about them.\n\n## What it deliberately does not do\n\nIt does not scroll; put it in a view that does. It does not virtualize — every child is drawn, so a feed of thousands wants a list. It does not animate an item from one column to another when the count changes; the block repacks. And it does not stretch the shortest column to meet the tallest, so the bottom edge is level only to the extent the items allow.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Columns", target: "subject", kind: ControlKind::Number { prop: "columns", min: 0., max: 6., step: 1., default: 0. } },
        Control { label: "Least column width", target: "subject", kind: ControlKind::Number { prop: "min_column_width", min: 80., max: 400., step: 10., default: 180. } },
        Control { label: "Column gap", target: "subject", kind: ControlKind::Number { prop: "column_gap", min: 0., max: 48., step: 2., default: 12. } },
        Control { label: "Row gap", target: "subject", kind: ControlKind::Number { prop: "row_gap", min: 0., max: 48., step: 2., default: 12. } },
    ],
    on_actions: Some(masonry_actions),
}];
