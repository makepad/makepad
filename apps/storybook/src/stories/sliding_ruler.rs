//! The sliding ruler story: a scale column on the wheel picker, lying down.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SlidingRulerOverview = StoryPage{
        StoryNote{text: "A ruler that slides under the band. It is not a second widget: it is a `WheelPicker` column that names a `step`, so a scale inherits the drum's whole gesture — drag, flick, catch, tap, wheel and arrow keys — without a line of new physics."}

        StoryHeading{text: "A scale"}
        StoryNote{text: "`min`, `max` and `step` make the column a scale; `items` stays empty and the graduations are generated. The number in the band is `min + index * step`, and `value(column)` hands it back so a readout beside it cannot disagree."}
        StoryRow{
            width: Fill
            subject := SlidingRuler{
                columns: [
                    WheelColumn{
                        min: -50.0
                        max: 50.0
                        step: 1.0
                        label_every: 10
                        selected: 50
                    }
                ]
            }
            ruler_note := Label{text: "0"}
        }

        StoryHeading{text: "Endless means cyclic"}
        StoryNote{text: "`loop_items` carries a scale round its declared range for as long as the finger keeps going: past 359° comes 0° again. A scale with no ends at all is not on offer — the drum's whole travel is `count * row_height`, and a control that claimed otherwise would be lying about where its own stops are."}
        StoryRow{
            width: Fill
            angle := SlidingRuler{
                visible_items: 61
                columns: [
                    WheelColumn{
                        min: 0.0
                        max: 359.0
                        step: 1.0
                        label_every: 15
                        unit: "°"
                        loop_items: true
                        selected: 90
                    }
                ]
            }
            angle_note := Label{text: "90 °"}
        }

        StoryHeading{text: "Standing on end"}
        StoryNote{text: "`axis` turns the whole control, not the shader alone: the walk transposes, the band stands across the travel, and the finger, the wheel and the arrow keys all read the other axis. The axis across the travel is what the columns are laid out along, so a ruler lying down holds exactly one column."}
        StoryRow{
            width: Fill
            SlidingRulerVertical{
                visible_items: 21
                row_height: 14.0
                columns: [
                    WheelColumn{
                        min: 0.0
                        max: 100.0
                        step: 1.0
                        label_every: 10
                        unit: "%"
                        selected: 40
                    }
                ]
            }
            SlidingRulerVertical{
                visible_items: 21
                row_height: 14.0
                column_width: 96.0
                columns: [
                    WheelColumn{
                        min: -24.0
                        max: 6.0
                        step: 0.5
                        precision: 1
                        unit: "dB"
                        label_every: 6
                        selected: 48
                    }
                ]
            }
        }

        StoryHeading{text: "The pitch is the rate"}
        StoryNote{text: "There is no tick-pitch property. `row_height` is the distance from one graduation to the next AND the distance the drag moves the scale by, because they have to be the same number: a ruler whose marks disagreed with its own motion would slide under the band at a rate the eye could not read. A finer ruler is a smaller `row_height`, and the drag gets finer with it."}
        StoryRow{
            width: Fill
            flow: Down
            spacing: 8.0
            SlidingRuler{
                row_height: 6.0
                visible_items: 81
                columns: [
                    WheelColumn{min: 0.0 max: 200.0 step: 1.0 label_every: 20 selected: 40}
                ]
            }
            SlidingRuler{
                row_height: 20.0
                visible_items: 25
                columns: [
                    WheelColumn{min: 0.0 max: 200.0 step: 1.0 label_every: 5 selected: 40}
                ]
            }
        }

        StoryHeading{text: "Numbers that do not fit are skipped"}
        StoryNote{text: "A number is skipped, never clipped and never shrunk: half a number says less than no number and costs the reader a second working out that it is half a number. Both rulers below are drawn at the same pitch; the left one asks for a number on every graduation and gets none, and its ticks are still there and still true."}
        StoryRow{
            width: Fill
            SlidingRuler{
                row_height: 8.0
                visible_items: 41
                columns: [
                    WheelColumn{min: 0.0 max: 100.0 step: 1.0 label_every: 1 selected: 20}
                ]
            }
            SlidingRuler{
                row_height: 8.0
                visible_items: 41
                columns: [
                    WheelColumn{min: 0.0 max: 100.0 step: 1.0 label_every: 10 selected: 20}
                ]
            }
        }

        StoryHeading{text: "Decimals, units and the marks"}
        StoryNote{text: "`precision` and `unit` are spelled the way `Slider` spells them and go through the same formatter, so a graduation and a readout cannot disagree about either. `tick_len` and `tick_len_major` say how far a plain and a major graduation reach across the column; naming neither takes both from the column's own width."}
        StoryRow{
            width: Fill
            SlidingRuler{
                column_width: 52.0
                row_height: 16.0
                visible_items: 31
                columns: [
                    WheelColumn{
                        min: 0.0
                        max: 8.0
                        step: 0.05
                        precision: 2
                        unit: "s"
                        label_every: 10
                        tick_len: 5.0
                        tick_len_major: 13.0
                        selected: 24
                    }
                ]
            }
        }

        StoryHeading{text: "A column of words is untouched"}
        StoryNote{text: "`step: 0.0` — the default — is the picker this widget has always been, to the pixel. A scale and a list can stand in one upright picker and turn independently."}
        StoryRow{
            width: Fill
            WheelPicker{
                column_width: 56.0
                columns: [
                    WheelColumn{
                        min: 0.0
                        max: 60.0
                        step: 5.0
                        label_every: 2
                        unit: "m"
                        selected: 6
                    }
                    WheelColumn{
                        selected: 1
                        items: ["Sharp" "Round" "Flat"]
                    }
                ]
            }
        }
    }
}

fn ruler_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // A spin reports every graduation it passes, which is what a readout
    // beside a moving ruler needs: one that kept showing the number from
    // before the flick would be telling the truth about nothing.
    let ruler = root.wheel_picker(cx, ids!(subject));
    if ruler.changed(actions).is_some() {
        root.label(cx, ids!(ruler_note)).set_text(cx, &format!("{}", ruler.value(0)));
    }
    let angle = root.wheel_picker(cx, ids!(angle));
    if angle.changed(actions).is_some() {
        root.label(cx, ids!(angle_note)).set_text(cx, &format!("{} °", angle.value(0)));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "selection/slidingruler/overview",
    category: "Selection",
    component: "SlidingRuler",
    also: &["SlidingRulerVertical"],
    name: "Overview",
    dsl: "SlidingRulerOverview",
    added: "2026-09-18",
    tags: &["new", "controls", "ruler", "scale", "graduations", "ticks", "value", "picker"],
    doc: "# SlidingRuler\n\nA scale that slides under the band, and rests on a graduation. It is the `WheelPicker` drum with a column that names a `step` instead of a list of words, so it inherits the whole gesture — drag, flick, catch a moving one, tap a graduation, wheel, arrow keys — and no new physics was written for it.\n\n## What makes a column a scale\n\n`step > 0.0` over a range `max > min`. That is the entire distinction: no mode, no discriminant, no second widget. Such a column carries no `items` at all; its rows are the numbers `min`, `min + step`, \u{2026} up to `max`, both ends included. `step: 0.0` \u{2014} the default \u{2014} is the column of words the picker always had, unchanged in every respect.\n\n## The pitch is the rate\n\nThere is deliberately NO tick-pitch property. `row_height` is the distance from one graduation to the next, and it is the same number the drag and the wheel move the drum by. They have to be one number: a ruler that drew its marks at one pitch and slid at another would be disagreeing with itself under the reader's finger. A finer ruler is a smaller `row_height`, and the drag gets finer with it.\n\n## Endless means cyclic\n\n`loop_items` carries a scale round its declared `min..max` for as long as the finger keeps going, which is the endless a ruler is reached for. A scale with no ends at all is not expressible: the drum's whole travel is `count * row_height`, and a control that implied otherwise would be lying about where its own stops are. There is no rubber band at the ends either \u{2014} a drum stops dead at its stops, because on a handful of rows the stretch is most of the control and reads as the picker having lost its place.\n\n## Either way round\n\n`axis` turns the whole control. The walk transposes \u{2014} lying down a `Fit` WIDTH becomes the graduations and a `Fit` HEIGHT becomes the column's depth \u{2014} the band stands across the travel, and the finger, the wheel notch and the arrow keys all read the other axis. The axis across the travel is the one the columns are laid out along and the one the hit test splits, so a picker lying down holds exactly ONE column: the cross axis has been spent on the ruler itself.\n\n## The graduations\n\n| Property | What it says |\n|---|---|\n| `min`, `max`, `step` | the scale, and therefore how many graduations there are |\n| `precision`, `unit` | how a number reads \u{2014} `Slider`'s words, and `Slider`'s formatter |\n| `label_every` | one graduation in this many is major: a longer tick, and the only kind numbered. Zero is every tenth |\n| `tick_len`, `tick_len_major` | how far a graduation reaches across the column. Zero takes both from the column's own width |\n\nA number is SKIPPED, never clipped and never shrunk, when it does not fit the room between itself and the next one: half a number says less than no number. The graduation's tick stays.\n\n## Reading it\n\n`value(column)` is the number in the band; `selected(column)` is still the graduation's index, and `changed` still reports while the ruler is moving. A host reading a ruler should not be made to re-derive `min + index * step` and get the rounding subtly different from the number the ruler is drawing.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Graduations on screen", target: "subject", kind: ControlKind::Number { prop: "visible_items", min: 9., max: 81., step: 2., default: 41. } },
        Control { label: "Pitch, and the drag rate", target: "subject", kind: ControlKind::Number { prop: "row_height", min: 4., max: 32., step: 1., default: 10. } },
        Control { label: "Depth across the travel", target: "subject", kind: ControlKind::Number { prop: "column_width", min: 24., max: 160., step: 4., default: 40. } },
        Control { label: "Ink lost at the edge", target: "subject", kind: ControlKind::Number { prop: "dim_far", min: 0., max: 1., step: 0.02, default: 0.45 } },
        Control { label: "Size lost at the edge", target: "subject", kind: ControlKind::Number { prop: "shrink_far", min: 0., max: 0.6, step: 0.01, default: 0.16 } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(ruler_actions),
}];
