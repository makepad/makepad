//! The layout stories: width and height, margin, padding, spacing, flow and alignment, including the under-filling Fill cases, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Box = RoundedView{
        show_bg: true
        draw_bg +: {
            color: uniform(#x0F02)
            border_size: uniform(1.)
            border_radius: uniform(0.)
            border_color: uniform(#xfff8)
        }
        padding: 3.
        align: Align{x: 0.5 y: 0.5}
    }

    let BoxLabel = P{
        width: Fit
        align: Align{x: 0.5}
    }

    let RedBox = Box{ draw_bg +: { color: #xD8483C } }
    let GreenBox = Box{ draw_bg +: { color: #x3CA83C } }
    let BlueBox = Box{ draw_bg +: { color: #x4A6ED8 } }

    mod.stories.LayoutOverview = StoryPage{
        H4{text: "Width & Height"}
        StoryRow{
            flow: Right
            height: 100.
            Box{
                width: 100. height: 60.
                BoxLabel{text: "width: 100.\nheight: 60"}
            }
            Box{
                width: 100. height: Fill
                BoxLabel{text: "width: 100.\nheight: Fill"}
            }
            Box{
                width: 150. height: Fit
                BoxLabel{text: "width: 150.\nheight: Fit"}
            }
        }

        Hr{}
        H4{text: "Margin"}
        StoryRow{
            align: Align{x: 0. y: 0.}
            flow: Right
            spacing: 0.
            Box{
                width: Fit height: Fit
                margin: 0.
                BoxLabel{text: "margin: 0."}
            }
            Box{
                width: Fit height: Fit
                margin: 0.
                BoxLabel{text: "margin: 0."}
            }
            Box{
                width: Fit height: Fit
                margin: 10.
                BoxLabel{text: "margin: 10."}
            }
            Box{
                width: Fit height: Fit
                margin: Inset{top: 0. left: 40 right: 0 bottom: 0.}
                BoxLabel{text: "margin: {left: 40}"}
            }
        }

        Hr{}
        H4{text: "Padding"}
        StoryRow{
            Box{
                width: Fit height: Fit
                padding: 20.
                BoxLabel{text: "padding: 20."}
            }
            Box{
                width: Fit height: Fit
                padding: Inset{left: 40. right: 10.}
                BoxLabel{text: "padding: {left: 40., right: 10.}"}
            }
        }

        Hr{}
        H4{text: "Spacing"}
        Pbold{text: "spacing: 10."}
        StoryRow{
            spacing: 10.
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
        }
        Pbold{text: "spacing: 30."}
        StoryRow{
            spacing: 30.
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
        }

        Hr{}
        H4{text: "Flow Direction"}
        Pbold{text: "flow: Right"}
        StoryRow{
            spacing: 10.
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
        }
        Pbold{text: "flow: Down"}
        StoryRow{
            flow: Down
            spacing: 10.
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
            Box{height: 50 width: 50.}
        }

        Hr{}
        H4{text: "Align"}
        Pbold{text: "align: {x: 0., y: 0.}"}
        StoryRow{
            align: Align{x: 0. y: 0.}
            Box{height: 100 width: 50.}
            Box{height: 20 width: 50.}
            Box{height: 50 width: 50.}
        }
        Pbold{text: "align: {x: 0.0, y: 0.5}"}
        StoryRow{
            align: Align{x: 0.0 y: 0.5}
            Box{height: 100 width: 50.}
            Box{height: 20 width: 50.}
            Box{height: 50 width: 50.}
        }
        Pbold{text: "align: {x: 0., y: 1.}"}
        StoryRow{
            align: Align{x: 0.0 y: 1.0}
            Box{height: 100 width: 50.}
            Box{height: 20 width: 50.}
            Box{height: 50 width: 50.}
        }
        Pbold{text: "align: {x: 0.5, y: 0.}"}
        StoryRow{
            align: Align{x: 0.5 y: 0.}
            Box{height: 100 width: 50.}
            Box{height: 20 width: 50.}
            Box{height: 50 width: 50.}
        }
        Pbold{text: "align: {x: 1.0, y: 1.}"}
        StoryRow{
            align: Align{x: 1.0 y: 1.}
            Box{height: 100 width: 50.}
            Box{height: 20 width: 50.}
            Box{height: 50 width: 50.}
        }

        Hr{}
        H4{text: "Align x of an under-filling Fill child"}
        // A `width: Fill{max}` child that caps below the row width leaves slack.
        // align.x must position that slack. These three should read left / center /
        // right; before the Flow::Right deferred-fill fix they all anchored left.
        Pbold{text: "flow: Right, align: {x: 0.},  child width: Fill{max: 120.}"}
        StoryRow{
            align: Align{x: 0. y: 0.5}
            RedBox{height: 50 width: Fill{max: 120.}
                BoxLabel{text: "Fill{max: 120.}"}
            }
        }
        Pbold{text: "flow: Right, align: {x: 0.5}, child width: Fill{max: 120.}"}
        StoryRow{
            align: Align{x: 0.5 y: 0.5}
            GreenBox{height: 50 width: Fill{max: 120.}
                BoxLabel{text: "Fill{max: 120.}"}
            }
        }
        Pbold{text: "flow: Right, align: {x: 1.0}, child width: Fill{max: 120.}"}
        StoryRow{
            align: Align{x: 1.0 y: 0.5}
            BlueBox{height: 50 width: Fill{max: 120.}
                BoxLabel{text: "Fill{max: 120.}"}
            }
        }

        Hr{}
        H4{text: "Align y of an under-filling Fill child"}
        // The Flow::Down mirror: a `height: Fill{max}` child caps below the column
        // height, and align.y positions the slack. These read top / center / bottom.
        // The faint outer box is the full column; the colored box is the Fill child.
        StoryRow{
            height: 180.
            spacing: 20.
            Box{width: 140. height: Fill flow: Down align: Align{y: 0.}
                RedBox{width: Fill height: Fill{max: 60.}
                    BoxLabel{text: "align y: 0."}
                }
            }
            Box{width: 140. height: Fill flow: Down align: Align{y: 0.5}
                GreenBox{width: Fill height: Fill{max: 60.}
                    BoxLabel{text: "align y: 0.5"}
                }
            }
            Box{width: 140. height: Fill flow: Down align: Align{y: 1.0}
                BlueBox{width: Fill height: Fill{max: 60.}
                    BoxLabel{text: "align y: 1.0"}
                }
            }
        }
    }

    let Cell = RoundedView{
        height: 40.
        show_bg: true
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: theme.color_surface_container_high border_radius: theme.radius_s}
    }
    let CellLabel = Label{width: Fit draw_text.color: theme.color_text_meta}

    mod.stories.LayoutResponsive = StoryPage{
        StoryNote{text: "A row hands its slack to its children by WEIGHT, and a child can refuse to grow past a maximum or to shrink below a minimum. A wrapping row spends the width it has and moves the rest to the next line. Drag the width below and watch each rule answer."}

        StoryRow{
            frame_width := Slider{
                width: 300.
                text: "Frame width"
                min: 280.0
                max: 900.0
                default: 820.0
                step: 10.0
            }
            width_note := Label{text: "820 points, 6 per row"}
        }

        frame := RoundedView{
            width: 820.
            height: Fit
            flow: Down
            spacing: theme.space_3
            padding: theme.mspace_3
            show_bg: true
            draw_bg +: {color: theme.color_surface_container_low border_radius: theme.radius_m}

            CellLabel{text: "Weights: the slack is split 1 : 2 : 1, whatever the width"}
            View{width: Fill height: Fit flow: Right spacing: theme.space_2
                Cell{width: Fill{weight: 1.0} CellLabel{text: "1"}}
                Cell{width: Fill{weight: 2.0} CellLabel{text: "2"}}
                Cell{width: Fill{weight: 1.0} CellLabel{text: "1"}}
            }

            CellLabel{text: "Clamps: one stops growing at 160, one gives up its share last — until the row itself runs out"}
            View{width: Fill height: Fit flow: Right spacing: theme.space_2
                Cell{width: Fill{max: 160.0} CellLabel{text: "max 160"}}
                Cell{width: Fill CellLabel{text: "the rest"}}
                Cell{width: Fill{min: 220.0} CellLabel{text: "min 220"}}
            }

            CellLabel{text: "Wrapping: fixed cells keep their size and the row count answers instead"}
            View{width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: theme.space_2
                Cell{width: 120. CellLabel{text: "1"}}
                Cell{width: 120. CellLabel{text: "2"}}
                Cell{width: 120. CellLabel{text: "3"}}
                Cell{width: 120. CellLabel{text: "4"}}
                Cell{width: 120. CellLabel{text: "5"}}
                Cell{width: 120. CellLabel{text: "6"}}
                Cell{width: 120. CellLabel{text: "7"}}
                Cell{width: 120. CellLabel{text: "8"}}
            }

            CellLabel{text: "The everyday shell: a side column that caps, and a body that takes the rest"}
            View{width: Fill height: 72. flow: Right spacing: theme.space_2
                Cell{width: Fill{max: 220.0} height: Fill CellLabel{text: "side, max 220"}}
                Cell{width: Fill height: Fill CellLabel{text: "body"}}
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/layout/overview",
    category: "Containers",
    component: "Layout",
    also: &[],
    name: "Overview",
    dsl: "LayoutOverview",
    added: "2026-07-23",
    tags: &["ported"],
    doc: "# Layout\n\nLayout demos show width, height, margin, padding, spacing, flow, and alignment.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}, Story {
    key: "containers/layout/responsive",
    category: "Containers",
    component: "Layout",
    also: &[],
    name: "Responsive",
    dsl: "LayoutResponsive",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Responsive layout

Three rules do the work, and the width slider is there so each one can be seen answering.

**Weight** splits the slack. `Fill{weight: 2.0}` takes twice the share of `Fill{weight: 1.0}`, and the proportion holds at every width rather than being a set of numbers that only add up at one size.

**Clamps** say where a share stops. `Fill{max: 160.}` grows with the row until it has 160 and then hands the rest back; `Fill{min: 220.}` gives up its share last, and drag the width to its narrowest to see what happens when even that cannot be honoured: a minimum is a priority, not a guarantee. A side column that caps and a body that takes the rest is the everyday shell, and it is those two rules and nothing else.

**Wrapping** answers with rows instead of size. `flow: Flow.Right{wrap: true}` keeps every cell the width it asked for and moves what does not fit to the next line, which is the grid behaviour a catalogue of cards wants: the cells stay legible and the column count is what changes.

`Fit{min, max}` is the fourth member of the family, for a box that takes its content's size but refuses to get silly about it.",
    subject: "frame",
    feature: None,
    controls: &[],
    on_actions: Some(responsive_actions),
}];


/// Cells are 120 wide with `space_2` between them inside `mspace_3` padding;
/// the count is what the wrap rule works out, said in words so the reflow is
/// readable rather than only visible.
fn cells_per_row(frame_width: f64) -> usize {
    const CELL: f64 = 120.0;
    const GAP: f64 = 8.0;
    const PAD: f64 = 24.0;
    let room = frame_width - PAD;
    if room < CELL {
        return 1;
    }
    (((room + GAP) / (CELL + GAP)).floor() as usize).clamp(1, 8)
}

fn responsive_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let Some(w) = root.slider(cx, ids!(frame_width)).slided(actions) else {
        return;
    };
    let mut frame = root.widget(cx, ids!(frame));
    script_apply_eval!(cx, frame, { width: #(w) });
    let text = format!("{:.0} points, {} per row", w, cells_per_row(w));
    root.label(cx, ids!(width_note)).set_text(cx, &text);
}
