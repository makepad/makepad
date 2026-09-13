//! The layout stories: width and height, margin, padding, spacing, flow and
//! alignment, including the under-filling Fill cases; and the responsive page,
//! where weights, clamps, wrapping and an adaptive view answer a changing width.
use crate::makepad_widgets::*;
use crate::registry::Story;
use std::sync::Mutex;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // Plain values: a `uniform(..)` inside a merge redeclares the input
    // instead of setting it, and the boxes drew as nothing but their labels.
    let Box = RoundedView{
        show_bg: true
        draw_bg +: {
            color: theme.color_surface_container_high
            border_size: 1.
            border_radius: 0.
            border_color: theme.color_outline_variant
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
        StoryNote{text: "A row hands its slack to its children by WEIGHT, and a child can refuse to grow past a maximum or to shrink below a minimum. A wrapping row spends the width it has and moves the rest to the next line. An adaptive view answers in a step instead, and swaps one arrangement for another. Drag the width below and watch each rule answer."}

        StoryRow{
            frame_width := Slider{
                width: 300.
                text: "Frame width"
                min: 280.0
                max: 900.0
                default: 600.0
                step: 10.0
            }
            width_note := Label{text: "600 points, 4 per row"}
        }

        frame := RoundedView{
            width: 600.
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

            CellLabel{text: "Steps: under 440 points of room the side goes above the body"}
            // The variants keep the default selector's names, Desktop and
            // Mobile: that selector picks on the window until the page's
            // handler installs one that reads the room this frame gives.
            adaptive := AdaptiveView{
                width: Fill
                height: Fit
                Desktop := View{
                    width: Fill height: 72.
                    flow: Right
                    spacing: theme.space_2
                    Cell{width: 160. height: Fill CellLabel{text: "side, beside the body"}}
                    Cell{width: Fill height: Fill CellLabel{text: "body"}}
                }
                Mobile := View{
                    width: Fill height: Fit
                    flow: Down
                    spacing: theme.space_2
                    Cell{width: Fill CellLabel{text: "side, above the body"}}
                    Cell{width: Fill height: 72. CellLabel{text: "body"}}
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "layout/layout/overview",
    category: "Layout",
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
    key: "layout/layout/responsive",
    category: "Layout",
    component: "Layout",
    also: &["AdaptiveView"],
    name: "Responsive",
    dsl: "LayoutResponsive",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Responsive layout

Three rules answer a changing width smoothly and a fourth answers it in a step. The width slider is there so each one can be seen answering.

**Weight** splits the slack. `Fill{weight: 2.0}` takes twice the share of `Fill{weight: 1.0}`, and the proportion holds at every width rather than being a set of numbers that only add up at one size.

**Clamps** say where a share stops. `Fill{max: 160.}` grows with the row until it has 160 and then hands the rest back; `Fill{min: 220.}` gives up its share last, and drag the width to its narrowest to see what happens when even that cannot be honoured: a minimum is a priority, not a guarantee. A side column that caps and a body that takes the rest is the everyday shell, and it is those two rules and nothing else.

**Wrapping** answers with rows instead of size. `flow: Flow.Right{wrap: true}` keeps every cell the width it asked for and moves what does not fit to the next line, which is the grid behaviour a catalogue of cards wants: the cells stay legible and the column count is what changes.

**Steps** answer with a different arrangement. Below 440 points of room the shell at the bottom of the frame stops putting its side column beside the body and puts it above. Nothing between the two sizes is interpolated: past the breakpoint it is simply the other layout.

`Fit{min, max}` is the last member of the sizing family, for a box that takes its content's size but refuses to get silly about it.

## AdaptiveView

`AdaptiveView` holds named templates and shows one of them. A selector picks which, and the view builds the chosen template when the choice changes. The template it stops showing is dropped with its state, unless `retain_unused_variants` is on.

**By default it picks on the window, not on its parent.** The default selector returns `Desktop` for a window 860 points wide or more and `Mobile` below that, so a view inside a panel that a person resizes never changes on its own. This page installs a selector that reads the room the parent hands the view, which is what lets the width slider flip it:

```rust
root.adaptive_view(cx, ids!(adaptive)).set_variant_selector(|_cx, parent| {
    if parent.x < 440.0 { live_id!(Mobile) } else { live_id!(Desktop) }
});
```

The selector runs when the view is drawn at a new parent size, and when the window changes. It may return the name of any template the view declares, so three steps are three templates. `set_default_variant_selector` puts the window rule back.

Because the handler installs the selector, the first frame after the page opens is picked by the window rule. The demo's templates keep the default names for that reason, and the frame starts wide enough that in a window 860 points or wider both rules pick the same one.",
    subject: "frame",
    feature: None,
    controls: &[],
    on_actions: Some(responsive_actions),
}];

/// Where the step demo changes arrangement, in points of the room its parent
/// gives it rather than of the window.
const STEP_AT: f64 = 440.0;

/// The adaptive view the parent-width selector was last installed on. A
/// story is rebuilt when it is opened again, and the new view starts out with
/// the window rule, so a changed id is the signal to install it again.
static STEP_SELECTOR_ON: Mutex<u64> = Mutex::new(0);

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

/// The default selector reads the window, which the width slider never
/// changes, so the page gives the view one that reads its parent instead.
fn install_step_selector(cx: &mut Cx, root: &WidgetRef) {
    let widget = root.widget(cx, ids!(adaptive));
    let uid = widget.widget_uid().0;
    if uid == 0 {
        return;
    }
    let Ok(mut installed) = STEP_SELECTOR_ON.lock() else {
        return;
    };
    if *installed == uid {
        return;
    }
    widget.as_adaptive_view().set_variant_selector(|_cx, parent| {
        if parent.x < STEP_AT {
            live_id!(Mobile)
        } else {
            live_id!(Desktop)
        }
    });
    *installed = uid;
}

fn responsive_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    install_step_selector(cx, root);
    let Some(w) = root.slider(cx, ids!(frame_width)).slided(actions) else {
        return;
    };
    let mut frame = root.widget(cx, ids!(frame));
    script_apply_eval!(cx, frame, { width: #(w) });
    let text = format!("{:.0} points, {} per row", w, cells_per_row(w));
    root.label(cx, ids!(width_note)).set_text(cx, &text);
}
