//! The svg-select story: a drawing that is the control, and the ids in the
//! file that name its parts.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SvgSelectOverview = StoryPage{
        StoryNote{text: "A picture whose parts are pickable. The parts are named by the id the file carries on a path or a group, and that id is what comes back when one is picked \u{2014} so the drawing is both the picture and the list of things it offers, and there is no second list to keep in step with it."}

        StoryHeading{text: "The drawing is the control"}
        StoryNote{text: "The map below is one SVG. Every continent is a path with an id on it; the sea is a rect with none. The pointer lights whatever it is over and a press takes it, and both the lighting and the press use the shape itself \u{2014} the gap between two coastlines belongs to neither of them, which a box around each shape could not say."}
        StoryRow{
            subject := SvgSelect{
                width: 560. height: 300.
                draw_svg +: {svg: crate_resource("self:resources/world_map.svg")}
            }
        }
        StoryRow{
            subject_note := Label{text: "nothing picked" draw_text +: {color: theme.color_text_meta}}
        }

        StoryHeading{text: "One id on a group covers everything under it"}
        StoryNote{text: "Two things to try in the map above. Australia and the island beside it are two paths inside a group carrying one id, so they light together and answer as one region. The island off the east coast of Africa is a path with no id at all: it is drawn like everything else and a press on it picks nothing, which is what makes a decoration a decoration."}

        StoryHeading{text: "More than one at once"}
        StoryNote{text: "multiple: true toggles instead of moving: a press adds a region and a press on one already held takes it away. A press on the sea, or anywhere between the parts, changes nothing \u{2014} on a picture a miss is ordinary, and a miss that cleared the answer would be a trap."}
        StoryRow{
            many := SvgSelect{
                width: 480. height: 260.
                multiple: true
                default_selected: ["africa" "south-america"]
                draw_svg +: {svg: crate_resource("self:resources/world_map.svg")}
            }
        }
        StoryRow{
            many_note := Label{text: "africa, south-america" draw_text +: {color: theme.color_text_meta}}
        }

        StoryHeading{text: "A colour per region"}
        StoryNote{text: "set_tint gives one id an ink of its own in place of the one the file was authored with, which is what a map showing a quantity per region needs. Hover and selection then mix on top of the tint, so a tinted region still lights under the pointer and still shows that it is held."}
        StoryRow{
            tinted := SvgSelect{
                width: 480. height: 260.
                draw_svg +: {svg: crate_resource("self:resources/world_map.svg")}
            }
        }
        StoryRow{
            tint_on := Button{text: "Show the quantity"}
            tint_off := Button{text: "Back to the file's colours"}
        }

        StoryHeading{text: "Off"}
        StoryNote{text: "A disabled drawing flattens toward one colour rather than going away: the picture still says what it is a picture of, and plainly is not for pressing."}
        StoryRow{
            SvgSelect{
                width: 360. height: 200.
                default_selected: ["asia"]
                draw_svg +: {svg: crate_resource("self:resources/world_map.svg")}
                animator +: {disabled: {default: @on}}
            }
        }
    }
}

/// A quantity per region, as a map showing one would have: the ink is the
/// value, and the page holds the numbers because the control holds none.
const QUANTITY: &[(&str, f32)] = &[
    ("north-america", 0.62),
    ("south-america", 0.34),
    ("europe", 0.81),
    ("africa", 0.22),
    ("asia", 0.97),
    ("oceania", 0.45),
    ("greenland", 0.08),
    ("antarctica", 0.02),
];

/// A low value is quiet and a high one is loud: one ramp, so the map can be
/// read without a key beside it.
fn ramp(t: f32) -> Vec4f {
    Vec4f {
        x: 0.12 + 0.74 * t,
        y: 0.30 + 0.28 * t,
        z: 0.42 - 0.18 * t,
        w: 1.0,
    }
}

fn svg_select_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.svg_select(cx, ids!(subject));
    if let Some(id) = subject.picked(actions) {
        root.label(cx, ids!(subject_note)).set_text(cx, &format!("picked {id}"));
    }
    // The pointer says what a press would take before it takes it, which on
    // a picture is the only place the name is written down at all.
    if let Some(id) = subject.hovered_action(actions) {
        root.label(cx, ids!(subject_note)).set_text(cx, &format!("a press here takes {id}"));
    }
    if subject.hover_ended(actions) {
        let held = subject.selection();
        let text = if held.is_empty() { "nothing picked".to_string() } else { format!("picked {}", held.join(", ")) };
        root.label(cx, ids!(subject_note)).set_text(cx, &text);
    }

    let many = root.svg_select(cx, ids!(many));
    if let Some(held) = many.changed(actions) {
        let text = if held.is_empty() { "nothing picked".to_string() } else { held.join(", ") };
        root.label(cx, ids!(many_note)).set_text(cx, &text);
    }

    let tinted = root.svg_select(cx, ids!(tinted));
    if root.button(cx, ids!(tint_on)).clicked(actions) {
        for (id, value) in QUANTITY {
            tinted.set_tint(cx, id, ramp(*value));
        }
    }
    if root.button(cx, ids!(tint_off)).clicked(actions) {
        tinted.clear_tints(cx);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/svg-select/overview",
    category: "Inputs",
    component: "SvgSelect",
    also: &["SvgSelectFlat"],
    name: "Overview",
    dsl: "SvgSelectOverview",
    added: "2026-09-10",
    tags: &["new", "svg", "map", "picker", "regions", "hit test", "drawing", "vector"],
    doc: "# SvgSelect

A drawing that is the control. A map, a floor plan, a seating chart, a diagram of a machine: a picture whose parts already mean something, where the thing being chosen has a shape on screen and a list of names beside it would be a worse way of saying the same thing.

Each pickable part is named by the `id` the file carries on a path or on a group, and that id is what comes back when one is picked. The drawing is therefore the source of both the picture and the vocabulary, and there is no second list to keep in step with it. A part with no id is drawn like any other and picks nothing, which is how a coastline stays a coastline and a decoration stays a decoration.

## The picking is the geometry, not a box around it

There is no second parser and no second renderer here: the document is the one the SVG layer already parses and retains, and the picking walks that same node tree. Each shape's segments \u{2014} lines and curves alike \u{2014} are flattened once into closed rings in the drawing's own coordinates, and a press is tested against them by casting a ray and counting the edges it crosses, under the shape's own `fill-rule`. Nonzero and even-odd disagree about a ring inside a ring, which is why the rule is carried from the file rather than assumed.

So the test is exact. The sea between two coastlines belongs to neither of them, a country with a hole in it is not picked through the hole, and a concave shape does not answer for the bite out of its own side.

## Lighting a region costs one tessellation, not one per frame

Rather than draw a second copy of a region over the first, the fill on the region's own nodes is rewritten inside the retained document and the cached geometry is marked stale. The picture is re-tessellated once per change of state \u{2014} a press, or the pointer crossing from one region into the next \u{2014} and never per frame. Nothing changes, nothing is rebuilt.

## Single and multiple

| Property | A press does |
|---|---|
| `multiple: false` | moves the selection to whatever was pressed |
| `multiple: true` | adds a region, or takes away one already held |

Either way a press on a part with no id, or on nothing at all, changes nothing and says nothing. On a picture a miss is ordinary \u{2014} most of a map is sea \u{2014} and a miss that cleared the answer would be a trap.

## Colour

`hover_color` and `select_color` are mixed into whatever colour a region already has, by `hover_weight` and `select_weight`. `set_tint` replaces that colour for one id, which is what a map showing a quantity per region needs; hover and selection then mix on top of the tint, so a tinted region still lights and still shows that it is held. A region painted with a gradient has no single colour to mix with and is left alone.

## Keyboard

The arrows walk the regions in the order the drawing paints them, lighting where they are, and Return or Space picks. Home and End are the first and last region. There is no pointer to hover with, so the arrows do the hovering.

## What it deliberately does not do

It does not pan, zoom or project: the drawing is fitted to the widget once and stays there. It does not label anything \u{2014} the ids are names for the program, not words for a reader, and the words belong to the host. It does not hit-test strokes: a region is picked by the area it fills, so a shape drawn as a bare outline is picked anywhere inside that outline. `<use>` instances are drawn but never picked, since the geometry they stand for lives in a symbol rather than in the node.

## Reading it

`picked` reports the id a press landed on and `changed` the whole selection after it. `hovered_action` and `hover_ended` follow the pointer, for a page that wants to name the region in words \u{2014} which is where the words live, since the control has none.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Multiple", target: "subject", kind: ControlKind::Bool { prop: "multiple", default: false } },
        Control { label: "Hover weight", target: "subject", kind: ControlKind::Number { prop: "hover_weight", min: 0., max: 1., step: 0.05, default: 0.35 } },
        Control { label: "Select weight", target: "subject", kind: ControlKind::Number { prop: "select_weight", min: 0., max: 1., step: 0.05, default: 0.8 } },
        Control { label: "Off weight", target: "subject", kind: ControlKind::Number { prop: "disabled_weight", min: 0., max: 1., step: 0.05, default: 0.7 } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(svg_select_actions),
}];
