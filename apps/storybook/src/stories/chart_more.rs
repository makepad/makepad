//! The chart shapes story: six pictures that are not a series over time,
//! and the empty frame each one draws when it is handed nothing.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Plot = View{
        width: Fill
        height: 230.
        flow: Down
        spacing: theme.space_1
        caption := Label{text: "" draw_text +: {color: theme.color_text_meta}}
    }

    mod.stories.ChartShapes = StoryPage{
        StoryNote{text: "Six shapes for data that is not a series over time. A pie and a donut divide a whole; a radial bar measures each part against a target; a funnel shows a count falling away stage by stage; a radar scores one thing on several scales at once; a bubble plot puts a weight on each point of a scatter. Every one of them takes its parts as lines of markup — a name, then its numbers."}

        StoryHeading{text: "Sharing out a whole"}
        StoryNote{text: "A pie asks the eye to compare angles at the middle, where every wedge has narrowed to a point. A donut leaves only the arcs, which is the part of a wedge the eye can actually judge, and the hole is the one place on a circular chart with room for a number. Both label a wedge only where the wedge is wide enough to hold the words; the slivers keep their colour and lose their names."}
        StoryRow{
            Plot{
                caption: Label{text: "PieChart"}
                PieChart{
                    series: [
                        "Rent 1250"
                        "Food 480"
                        "Transport 210"
                        "Power 160"
                        "Other 140"
                    ]
                }
            }
            Plot{
                caption: Label{text: "DonutChart"}
                subject := DonutChart{
                    caption: "per month"
                    series: [
                        "Rent 1250"
                        "Food 480"
                        "Transport 210"
                        "Power 160"
                        "Other 140"
                    ]
                }
            }
        }
        StoryRow{
            picked_note := Label{text: "Point at a wedge, or press one."}
        }

        StoryHeading{text: "Against a target"}
        StoryNote{text: "The arcs here do not divide anything. Each is its part against `max_value` — a hundred, below — so three parts at eighty per cent are three arcs at eighty per cent, and not a third of the circle each. The outermost track is the first line. Rings further in are shorter for the same share, which flatters whatever is written last, so this is a shape for a handful of parts and never for a ranking."}
        StoryRow{
            Plot{
                caption: Label{text: "RadialBarChart"}
                RadialBarChart{
                    max_value: 100.
                    series: [
                        "Coverage 92"
                        "Uptime 78"
                        "Battery 64"
                        "Storage 41"
                    ]
                }
            }
            Plot{
                caption: Label{text: "No names, no gutter"}
                RadialBarChart{
                    max_value: 100.
                    label_width: 0.
                    track_size: 12.
                    series: ["Coverage 92" "Uptime 78" "Battery 64" "Storage 41"]
                }
            }
        }

        StoryHeading{text: "Falling away"}
        StoryNote{text: "The stages of a funnel do not add up to anything — the same people are counted again in every stage they reached — so each band is measured against the WIDEST band and not against a total. Adding a stage therefore does not narrow the ones already drawn. With `taper` on, a band's bottom edge is as wide as the next band's top, which draws the drop between two stages as a slope instead of a step the eye has to measure. Off, they are plain centred bars, which is the honest shape when the order is not a sequence."}
        StoryRow{
            Plot{
                caption: Label{text: "FunnelChart"}
                funnel := FunnelChart{
                    series: [
                        "Visited 8400"
                        "Signed up 3100"
                        "Configured 1450"
                        "Invited a colleague 620"
                        "Renewed 380"
                    ]
                }
            }
            Plot{
                caption: Label{text: "taper: false"}
                FunnelChart{
                    taper: false
                    series: [
                        "Visited 8400"
                        "Signed up 3100"
                        "Configured 1450"
                        "Invited a colleague 620"
                        "Renewed 380"
                    ]
                }
            }
        }

        StoryHeading{text: "Several scales at once"}
        StoryNote{text: "Every axis runs from nothing at the middle to `max_value` at the rim — one scale for all of them. A radar with a scale per axis can be made to say anything, because the shape then depends on numbers that appear nowhere on the picture. Read the distances along the spokes; the room a face encloses is proportional to nothing and changes when the axes are reordered."}
        StoryRow{
            Plot{
                height: 280.
                caption: Label{text: "RadarChart"}
                RadarChart{
                    max_value: 10.
                    axes: ["Speed" "Range" "Comfort" "Price" "Quiet" "Boot"]
                    series: [
                        "Estate: 6 9 8 5 7 10"
                        "Hatchback: 8 5 6 9 5 4"
                    ]
                }
            }
            Plot{
                height: 280.
                caption: Label{text: "One face, three axes"}
                RadarChart{
                    max_value: 10.
                    rings: 2
                    fill_alpha: 0.45
                    axes: ["Cost" "Effort" "Risk"]
                    series: ["Option A: 3 8 6"]
                }
            }
        }

        StoryHeading{text: "Position and weight"}
        StoryNote{text: "Three numbers a line — across, up, and how much. The weight scales the circle's AREA and not its radius, which is the one place a bubble chart is routinely a lie: a circle drawn twice as wide carries four times the ink, so a radius taken straight from the number shows the big values as four times what they are. It costs one square root not to do that. A name goes inside its circle when it fits and under it when it does not."}
        StoryRow{
            Plot{
                height: 260.
                caption: Label{text: "BubbleChart"}
                BubbleChart{
                    series: [
                        "North: 12 44 900"
                        "South: 31 28 420"
                        "East: 48 61 1600"
                        "West: 22 70 260"
                        "Central: 39 39 700"
                    ]
                }
            }
        }

        StoryHeading{text: "A chart with nothing in it"}
        StoryNote{text: "These four were declared empty. A pie with no parts is an empty box, a radial bar draws its tracks and no arcs, a radar draws its web, and a bubble plot draws its grid. None of them invents a plausible series to fill itself with — which the plots on the Overview page do, and which is why a chart there whose data never arrived looks exactly like one that is working."}
        StoryRow{
            Plot{
                height: 150.
                caption: Label{text: "PieChart{}"}
                PieChart{}
            }
            Plot{
                height: 150.
                caption: Label{text: "RadarChart with axes and no parts"}
                RadarChart{axes: ["A" "B" "C" "D" "E"]}
            }
        }
        StoryRow{
            Plot{
                height: 150.
                caption: Label{text: "BubbleChart{}"}
                BubbleChart{}
            }
            Plot{
                height: 150.
                caption: Label{text: "FunnelChart{}"}
                FunnelChart{}
            }
        }

        StoryHeading{text: "The surface underneath"}
        StoryNote{text: "`ChartFigure` is where the six colours, the two text layers, the padding and the data lines live; each shape reaches them through it, so a palette set once is the palette all six use. On its own it is an empty box and there is no reason to place one."}
    }
}

/// The parts of the donut above, in the order its lines were written. The
/// action carries a part's NUMBER and not its name — a host almost always
/// has the data the chart was built from and can look the name up, and a
/// widget that posted strings back would be handing the caller its own
/// copy of something the caller already has.
const PARTS: &[&str] = &["Rent", "Food", "Transport", "Power", "Other"];

fn chart_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let donut = root.donut_chart(cx, ids!(subject));
    let note = root.label(cx, ids!(picked_note));
    if let Some(i) = donut.picked(actions) {
        note.set_text(cx, &format!("Pressed {}.", PARTS.get(i).copied().unwrap_or("?")));
    } else if let Some(hot) = donut.hovered(actions) {
        match hot {
            Some(i) => note.set_text(cx, &format!("Over {}.", PARTS.get(i).copied().unwrap_or("?"))),
            None => note.set_text(cx, "Point at a wedge, or press one."),
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/charts/shapes",
    category: "Data display",
    component: "Charts",
    also: &[
        "ChartFigure",
        "PieChart",
        "DonutChart",
        "RadialBarChart",
        "FunnelChart",
        "RadarChart",
        "BubbleChart",
    ],
    name: "Shapes",
    dsl: "ChartShapes",
    added: "2026-09-10",
    tags: &["new", "pie", "donut", "radial", "funnel", "radar", "bubble", "proportion", "share"],
    doc: "# Chart shapes

Six pictures for data that is not a series over time: `PieChart`, `DonutChart`, `RadialBarChart`, `FunnelChart`, `RadarChart` and `BubbleChart`. The Overview page plots a series against axes and answers *what happened next*; these answer *how does this whole divide*, *how far did each of these get*, *where did the count fall away*, *how does this one thing score on several scales*, and *where do these named points sit when each also carries a weight*. There is no time axis here and nothing to pan.

`ChartFigure` is the surface underneath all six — the palette, the two text layers, the padding and the data.

## The data is lines of markup

One line per part: `\"Rent 1250\"`, `\"Public transport 120\"`, `\"Speed: 4 3 5 2\"`. The words are the label and the numbers are the numbers. A colon, where there is one, says exactly where the name stops, which is how a name may hold a number of its own — `\"Q1 2024: 480\"` is one quarter worth 480, where `\"Q1 2024 480\"` is a quarter called Q1 carrying two numbers.

Lines rather than a list of numbers because a list of numbers is not a live type in the markup layer, and lines turn out to be the better trade anyway: the data reads as data in the file that declares it. `set_rows` takes the same thing from Rust.

Which numbers a shape reads:

| Shape | Numbers a line |
|---|---|
| pie, donut, funnel | one: the part's value |
| radial bar | one, against `max_value` |
| radar | one per axis, in axis order |
| bubble | three: across, up, weight |

## The shapes, and what each is honest about

**Pie and donut** divide a total. Both stop labelling a wedge that is too narrow to hold the words rather than letting labels spill across their neighbours, so a chart of many small parts keeps its colours and loses its names — which is the true picture of a pie asked to do too much. Past about seven parts the wedges are too close in angle to rank by eye; that is a property of circles, and a list too long for a pie is a bar chart.

The donut's hole is not decoration. It removes the middle of every wedge, where the angle has narrowed to a point and the eye has nothing to compare, and it is the one place on a circular chart with room for a number. The total lives there — or, while the pointer is on a wedge, that wedge does.

**The radial bar** measures each part against `max_value`, not against a sum. Its arcs are not shares of a circle and cannot be added up. Rings further in are physically shorter for the same share, which flatters whatever comes last, so it is a shape for a handful of parts and never for a ranking. `label_width` sets the gutter the names are written in; nothing centres the rings in the whole box and drops the names, which is what a tile wants.

**The funnel** measures each band against the WIDEST band. The stages of a funnel do not add up to anything — the same people are counted again in every stage they reached — so drawing a band as a share of that sum would narrow every band the more stages there are. With `taper` on, a band's bottom edge is as wide as the next band's top, which draws the drop as a slope; off, they are plain centred bars.

**The radar** runs every axis from nothing at the middle to `max_value` at the rim: ONE scale for all of them. A radar with a scale per axis can be made to say anything, because the shape then depends on numbers that appear nowhere on the picture. Read the distances along the spokes — the area a face encloses is proportional to nothing and changes when the axes are reordered. Fewer than three axes draws nothing at all: two axes are a line and one is a point, and neither should be offered as a radar.

**The bubble plot** scales a weight by AREA. A circle twice as wide carries four times the ink, so a radius taken straight from the number draws the big values as four times what they are. It costs one square root to be honest and this one pays it. Its grid has no round tick values, only the four extremes written round the frame: with a handful of named points the labels are the reading, and the grid is for judging position rather than for looking numbers up.

## Nothing to draw

**A chart with no data draws its empty frame and stops.** No fabricated series, no invented curve. The plots on the Overview page do invent one — a fixed-seed sine wave or a walked price — which makes a chart whose data never arrived look exactly like a chart that is working. That is worth not repeating: an empty circle is a true picture of an empty list.

## Reading the parts

Point at a part and the chart reports `Hovered(Some(i))`, and `Hovered(None)` when the pointer leaves them all; press one and it reports `Picked(i)`. Parts are numbered in the order their lines were written, so part three is the third line. The action carries the number and not the name — the caller has the data the chart was built from, and a widget posting strings back would be handing it a second copy of what it already has.

A funnel's whole row is the target and not the trapezoid inside it: the stage that has narrowed to a sliver is exactly the one worth pointing at. A bubble plot picks the SMALLEST circle under the pointer, which is the one drawn nearest the top and the one the eye takes itself to be aiming at. The radar reports nothing at all, because with faces lying over each other there is no one part under the pointer to name.

## How they are drawn

A wedge or an arc is two comparisons per pixel — a radius test and an angle test — over one quad. A funnel band is one interpolation and one comparison. A radar's face is a fan of triangles, each a quad with three half-plane tests, which fills it exactly because every corner sits on its own spoke. Nothing is an outline walked as a path, and the whole of any one chart is a handful of draw calls.

Angles are radians clockwise from straight up, in the hit test and in the shader alike, so the wedge that is drawn and the wedge a pointer picks are the same wedge. Spans do not wrap: an end is always after its start, and the last one runs past a full turn rather than coming back round to a smaller number — which is the classic way to lose the wedge that straddles twelve o'clock.

## Colour

Six colours from the theme's accent roles, going round again after the sixth, rather than fading or darkening past it: two parts the same colour are honestly ambiguous, where two nearly the same colour look distinguishable and are not. A label sitting ON a part gets near-black or near-white ink worked out from that part's brightness, since a palette is a list of colours and not a list of pairs.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Hole", target: "subject", kind: ControlKind::Number { prop: "hole", min: 0., max: 0.9, step: 0.02, default: 0.58 } },
        Control { label: "First part at", target: "subject", kind: ControlKind::Number { prop: "start_angle", min: 0., max: 360., step: 5., default: 0. } },
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 8., step: 0.25, default: 1. } },
        Control { label: "Padding", target: "subject", kind: ControlKind::Number { prop: "pad", min: 0., max: 48., step: 1., default: 10. } },
        Control { label: "Caption", target: "subject", kind: ControlKind::Text { prop: "caption", default: "per month" } },
        Control { label: "Names", target: "subject", kind: ControlKind::Bool { prop: "show_labels", default: true } },
        Control { label: "Numbers", target: "subject", kind: ControlKind::Bool { prop: "show_values", default: true } },
    ],
    on_actions: Some(chart_actions),
}];
