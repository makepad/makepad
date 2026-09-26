//! The story for the two widgets that say what a scrolling box is not
//! showing: marks along a track, and a fade at an edge with more past it.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Line = Label{
        width: Fill
        draw_text +: {color: theme.color_text_meta}
    }

    mod.stories.ScrollMarksAndFadesOverview = StoryPage{
        StoryNote{text: "A scroll view says almost nothing about the part it is not showing. These two say something: a track carrying a mark for every place worth going to, and an edge that goes soft when there is more content past it."}

        StoryHeading{text: "A track that doubles as a map"}
        StoryNote{text: "Every mark is a place in the document — a search hit, a warning, an error, a line somebody changed — drawn at the position it holds in the whole. The handle travels beside the marks rather than over them, so nothing you might want to drag towards is hidden by the thing you drag."}
        StoryRow{
            width: Fill
            View{
                width: Fill
                height: 260.
                flow: Right
                spacing: theme.space_2
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: theme.space_1
                    padding: theme.mspace_2
                    show_bg: true
                    // A plain View's pixel is transparent and never reads `color`: paint it here.
                    draw_bg +: {color: uniform(theme.color_surface_container_high) pixel: fn() {return Pal.premul(self.color)}}
                    Line{text: "2400 lines."}
                    Line{text: "Nineteen of them are worth a mark: two errors, two warnings,"}
                    Line{text: "four changed lines and eleven plain hits."}
                    Line{text: ""}
                    Line{text: "Drag the handle beside this panel, or roll the wheel over it."}
                }
                subject := mod.widgets.AnnotatedScrollBar{
                    height: Fill
                    view_total: 2400.
                    marks: ["0.02" "0.05 change" "0.09" "0.14 error" "0.21" "0.22" "0.23" "0.35 warn" "0.41" "0.47 change" "0.52" "0.58 error" "0.63" "0.71" "0.78 warn" "0.84" "0.88" "0.93 change" "0.97"]
                }
            }
        }
        StoryRow{
            reading := Label{text: "showing 0 to 260 of 2400"}
        }

        StoryHeading{text: "The same bar across the page"}
        StoryNote{text: "`vertical: false` turns the whole thing on its side: the lane runs along the top and the handle along the bottom. A timeline uses this to show where the interesting moments are without a second widget under it."}
        StoryRow{
            width: Fill
            mod.widgets.AnnotatedScrollBarX{
                width: Fill
                view_total: 1200.
                marks: ["0.04" "0.12 warn" "0.3" "0.31" "0.55 error" "0.62 change" "0.8" "0.95"]
            }
        }

        StoryHeading{text: "A cut-off list looks cut off"}
        StoryNote{text: "The bottom of this box is soft because there is more below it. Scroll down and the top goes soft too; reach the last line and the bottom comes back flat. Nothing fades that has nothing past it, so a box whose content fits is a plain box."}
        StoryRow{
            width: Fill
            cutoff := mod.widgets.ScrollShadowView{
                width: Fill
                height: 200.
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: uniform(theme.color_surface_container_high) pixel: fn() {return Pal.premul(self.color)}}
                Line{text: "One — the top of this list is flat, because nothing is above it."}
                Line{text: "Two"}
                Line{text: "Three"}
                Line{text: "Four"}
                Line{text: "Five"}
                Line{text: "Six"}
                Line{text: "Seven"}
                Line{text: "Eight"}
                Line{text: "Nine"}
                Line{text: "Ten"}
                Line{text: "Eleven"}
                Line{text: "Twelve"}
                Line{text: "Thirteen"}
                Line{text: "Fourteen — and this is the last line, so the bottom is flat here."}
            }
        }

        StoryHeading{text: "One edge at a time"}
        StoryNote{text: "Each edge is its own decision. This box has `fade_top: false`, for a list under a heading that is already a hard line — a second one under it would be saying the same thing twice."}
        StoryRow{
            width: Fill
            View{
                width: Fill
                height: Fit
                flow: Down
                Label{text: "Pinned heading" margin: theme.mspace_1}
                mod.widgets.ScrollShadowView{
                    width: Fill
                    height: 140.
                    flow: Down
                    spacing: theme.space_1
                    padding: theme.mspace_2
                    fade_top: false
                    show_bg: true
                    draw_bg +: {color: uniform(theme.color_surface_container_high) pixel: fn() {return Pal.premul(self.color)}}
                    Line{text: "First"}
                    Line{text: "Second"}
                    Line{text: "Third"}
                    Line{text: "Fourth"}
                    Line{text: "Fifth"}
                    Line{text: "Sixth"}
                    Line{text: "Seventh"}
                    Line{text: "Eighth"}
                    Line{text: "Ninth"}
                    Line{text: "Tenth"}
                }
            }
        }

        StoryHeading{text: "Content that fits fades nothing"}
        StoryNote{text: "The same widget, with three lines in it. There is nothing past any edge, so all four stay flat: the fade is information, not decoration."}
        StoryRow{
            width: Fill
            mod.widgets.ScrollShadowView{
                width: Fill
                height: 140.
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: uniform(theme.color_surface_container_high) pixel: fn() {return Pal.premul(self.color)}}
                Line{text: "One"}
                Line{text: "Two"}
                Line{text: "Three"}
            }
        }

        StoryHeading{text: "A box that grows until it is told to stop"}
        StoryNote{text: "`height: Fit` with a `max` is as tall as its content up to the ceiling, and a scrolling box from there on. This one reached its ceiling at 140, so it scrolls and its bottom is soft like any other; with three lines in it, it would be three lines tall and flat."}
        StoryRow{
            width: Fill
            grown := mod.widgets.ScrollShadowView{
                width: Fill
                height: Fit{max: FitBound.Abs(140)}
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: uniform(theme.color_surface_container_high) pixel: fn() {return Pal.premul(self.color)}}
                Line{text: "First"}
                Line{text: "Second"}
                Line{text: "Third"}
                Line{text: "Fourth"}
                Line{text: "Fifth"}
                Line{text: "Sixth"}
                Line{text: "Seventh"}
                Line{text: "Eighth"}
                Line{text: "Ninth"}
                Line{text: "Tenth — the last line, so the bottom is flat here."}
            }
        }

        StoryHeading{text: "Both ways at once"}
        StoryNote{text: "`ScrollShadowXYView` lets the content overflow across as well as down and fades all four edges on the same rule. The right edge is soft here because the rows are wider than the box."}
        StoryRow{
            width: Fill
            mod.widgets.ScrollShadowXYView{
                width: Fill
                height: 160.
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: uniform(theme.color_surface_container_high) pixel: fn() {return Pal.premul(self.color)}}
                Label{text: "A row long enough that it runs off the right of the box and keeps going for a good while after that."}
                Label{text: "Another row of about the same length, so the box has somewhere to scroll across to."}
                Label{text: "A third, and a fourth below it, so it has somewhere to scroll down to as well."}
                Label{text: "A fourth."}
                Label{text: "A fifth."}
                Label{text: "A sixth."}
                Label{text: "A seventh, which is past the bottom."}
            }
        }
    }
}

fn scroll_marks_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let bar = root.annotated_scroll_bar(cx, ids!(subject));
    if let Some(pos) = bar.scrolled(actions) {
        root.label(cx, ids!(reading))
            .set_text(cx, &format!("showing {:.0} to {:.0} of 2400", pos, pos + 260.0));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "layout/scrolling/marks-and-shadows",
    category: "Layout",
    component: "Scrolling",
    also: &["AnnotatedScrollBar", "AnnotatedScrollBarX", "ScrollShadowView", "ScrollShadowXYView"],
    name: "Marks and shadows",
    dsl: "ScrollMarksAndFadesOverview",
    added: "2026-09-10",
    tags: &[
        "new",
        "scroll",
        "minimap",
        "marks",
        "annotations",
        "fade",
        "edge",
        "cut off",
        "scrollbar",
        "container",
    ],
    doc: "# AnnotatedScrollBar and ScrollShadowView\n\nTwo widgets for the same problem: a scrolling box is very quiet about the part of the content it is not showing. The bar says roughly how far down the handle is and nothing about what is down there; the edge of the box says nothing at all, so a list cut off mid-row looks exactly like a list that happened to end.\n\n## AnnotatedScrollBar\n\nA scrollbar whose track carries marks — a coloured tick at every place worth knowing about. The track stops being a position readout and becomes a map of the whole document: a search that matched forty times is forty ticks you can aim at, rather than a count and a \"next\" button pressed forty times.\n\nA mark is a position between 0 and 1 and a colour. Positions are a share of the whole rather than a line number or a pixel offset, because the bar has no idea what the content is counted in and telling it twice is how the two get out of step.\n\n| Written as | Means |\n|---|---|\n| `marks: [\"0.42\"]` | a plain mark, in `mark_color` |\n| `marks: [\"0.42 warn\"]` | `mark_color_warn` |\n| `marks: [\"0.42 error\"]` | `mark_color_error` |\n| `marks: [\"0.42 change\"]` | `mark_color_change` |\n\nThe declared list is a convenience for a fixed set. Anything a program works out goes through `set_marks`, which takes positions and colours of its own and then owns the track.\n\n### Two marks in the same place\n\nThey collapse into one tick. A long document has far more interesting places than the track has rows of pixels — five thousand search hits down a four hundred pixel lane would paint it solid, which is the one result that carries no information at all. The first mark to claim a row keeps it, so a caller that lists errors before hits gets errors drawn.\n\n### What it does not do\n\nIt does not scroll anything. It is told how much content there is with `view_total`, and reports where the handle went as `Scrolled`; wiring that to a view is the host's job, because the thing being mapped is usually not a plain scroll view but a virtualized list, a document model or a timeline. Its own drawn length is the visible extent, since a bar beside a viewport is exactly as long as the viewport.\n\nA press on a mark does not jump to it either. The track already means \"go to here\", and a second meaning on the same press is one too many.\n\n## ScrollShadowView\n\nA scrolling box that fades an edge whenever there is content past it, and only then. Each edge decides for itself, so the top of a list is flat until it has been scrolled and the bottom is soft until the last row is reached. `fade_top`, `fade_bottom`, `fade_left` and `fade_right` turn any of the four off, for the case where something else already draws that line.\n\nThe fade comes up over the first `fade_ramp` pixels of overflow rather than switching on. Scrolling is continuous, and a box moved by one pixel has barely hidden anything; a hard edge appearing at that point reads as a glitch rather than as information.\n\nAn axis that cannot scroll never fades. A permanent gradient down the side of a box whose content fits is a decoration nobody chose, and it makes the fade mean nothing on the boxes where it is doing real work.

The box is as long as its bars say it is. A `Fit` height with a `max` is as tall as its content until it reaches the ceiling and a scrolling box from there on, and its far edge fades like any other's.",
    subject: "subject",
    feature: None,
    controls: &[
        Control {
            label: "Content length",
            target: "subject",
            kind: ControlKind::Number { prop: "view_total", min: 300., max: 10000., step: 100., default: 2400. },
        },
        Control {
            label: "Mark thickness",
            target: "subject",
            kind: ControlKind::Number { prop: "mark_thickness", min: 1., max: 8., step: 0.5, default: 2. },
        },
        Control {
            label: "Lane inset",
            target: "subject",
            kind: ControlKind::Number { prop: "track_inset", min: 0., max: 12., step: 0.5, default: 3. },
        },
        Control {
            label: "Fade depth",
            target: "cutoff",
            kind: ControlKind::Number { prop: "fade_size", min: 0., max: 80., step: 1., default: 20. },
        },
        Control {
            label: "Fade ramp",
            target: "cutoff",
            kind: ControlKind::Number { prop: "fade_ramp", min: 1., max: 200., step: 1., default: 24. },
        },
        Control {
            label: "Fade the top",
            target: "cutoff",
            kind: ControlKind::Bool { prop: "fade_top", default: true },
        },
        Control {
            label: "Fade the bottom",
            target: "cutoff",
            kind: ControlKind::Bool { prop: "fade_bottom", default: true },
        },
    ],
    on_actions: Some(scroll_marks_actions),
}];
