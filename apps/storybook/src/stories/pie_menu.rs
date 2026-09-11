//! The pie menu story: a ring that reads a direction and nothing else, and
//! the hole in the middle that lets it read "not yet".
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.PieMenuOverview = StoryPage{
        StoryNote{text: "A list menu asks the hand for two things: a direction and a distance. Every row is a different way away and each one is a thin thing to stop on. A ring asks for a direction only. Past the hole in the middle a wedge goes on forever, so a flick that leaves the middle and stops anywhere along a wedge picks it, and the same flick means the same choice wherever the ring was opened."}

        StoryHeading{text: "The ring"}
        StoryNote{text: "Six choices, the first pointing at twelve o'clock and the rest clockwise from it. This one is pinned: it is up from the first draw and stays up after a pick, which is a radial control sitting on the surface rather than a menu you summon. Aim at a wedge and it lights; the small number in each is the key that picks it."}
        StoryRow{
            subject := PieMenu{
                pinned: true
                labels: ["Move" "Rotate" "Scale" "Mirror" "Extrude" "Delete"]
            }
        }

        StoryHeading{text: "Opened where you press"}
        StoryNote{text: "Press anywhere in the panel below. The ring opens around the press, so the gesture never has to travel to the menu. Press and drag out in one movement to pick, or press once to leave it up, aim, and press again. A ring asked for near an edge is nudged in until all of it can be aimed at."}
        StoryRow{
            View{
                width: Fill
                height: 300.
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                summoned := PieMenu{
                    width: Fill
                    height: Fill
                    radius: 84.
                    labels: ["Cut" "Copy" "Paste" "Rename" "Delete"]
                }
            }
        }

        StoryHeading{text: "What came back"}
        StoryNote{text: "A pick reports which position was chosen and nothing else; the page below turns that into the word and the key. The ring does not keep it: a control that has to show its current setting is a radio group and not a menu."}
        StoryRow{
            picked := Label{text: "nothing picked yet"}
        }

        StoryHeading{text: "Three, five, seven"}
        StoryNote{text: "The first choice always points at twelve o'clock, so the seam between the last wedge and the first lands ON twelve whatever the count. An odd ring has no wedge opposite another and no seam on any axis; none of that changes how a direction is read."}
        StoryRow{
            three := PieMenu{
                pinned: true
                radius: 66. hub_radius: 20.
                labels: ["Red" "Green" "Blue"]
            }
            five := PieMenu{
                pinned: true
                radius: 66. hub_radius: 20.
                labels: ["One" "Two" "Three" "Four" "Five"]
            }
            seven := PieMenu{
                pinned: true
                radius: 66. hub_radius: 20.
                labels: ["Mon" "Tue" "Wed" "Thu" "Fri" "Sat" "Sun"]
            }
        }

        StoryHeading{text: "The dead zone"}
        StoryNote{text: "The hole is the dead zone: inside it there is no direction to read, so nothing is hot and letting go there picks nothing. They are one number rather than two, so the rule is drawn instead of having to be told: 'nothing happens here' is a thing you can see."}
        StoryRow{
            wide_hub := PieMenu{
                pinned: true
                radius: 96. hub_radius: 58.
                labels: ["North" "East" "South" "West"]
            }
        }

        StoryHeading{text: "The keyboard"}
        StoryNote{text: "The whole ring is one tab stop rather than one per wedge. Tab into the panel below or press in it, raise a ring, and then the digits 1 to 9 pick by position and Escape takes it down. There are no arrow keys on purpose: in a ring an arrow would have to mean a compass direction and a step through a list at the same time."}
        StoryRow{
            View{
                width: Fill
                height: 220.
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                keyed := PieMenu{
                    width: Fill
                    height: Fill
                    radius: 76.
                    labels: ["First" "Second" "Third" "Fourth"]
                }
            }
        }
        StoryNote{text: "That one starts closed, the way a summoned menu does: press in its panel to raise one, then try a digit."}
    }
}

fn pie_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for (name, id) in [
        ("the ring", ids!(subject)),
        ("the panel", ids!(summoned)),
        ("three", ids!(three)),
        ("five", ids!(five)),
        ("seven", ids!(seven)),
        ("the wide hub", ids!(wide_hub)),
        ("the keyed one", ids!(keyed)),
    ] {
        let menu = root.pie_menu(cx, id);
        if let Some(index) = menu.picked(actions) {
            let label = menu.label_at(index);
            let text = format!("{name}: {label}, picked by key {}", index + 1);
            root.label(cx, ids!(picked)).set_text(cx, &text);
        }
        if menu.cancelled(actions) {
            root.label(cx, ids!(picked)).set_text(cx, &format!("{name}: taken down, nothing picked"));
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/pie-menu/overview",
    category: "Navigation",
    component: "PieMenu",
    also: &[],
    name: "Overview",
    dsl: "PieMenuOverview",
    added: "2026-09-10",
    tags: &["new", "radial", "menu"],
    doc: "# PieMenu

A ring of choices around the point it was opened at.

A list menu asks the hand for a direction **and** a distance: every row is a different way away, and each one is a thin thing to stop on. A ring asks for a direction. Past the dead zone in the middle a wedge is unbounded, so a flick that leaves the hub and stops anywhere along a wedge picks it — and because the ring opens around the pointer, the same flick means the same thing wherever it was raised.

`PieRing` is the arithmetic, with its own tests: the wedge a direction picks, the span each wedge is drawn across, and where its label sits. Both the picking and the shader measure angles **clockwise from straight up**, and both get that measure from the same place — one measure in one place is what stops the wedge you see and the wedge you get from drifting apart by half a step.

The wedges are two comparisons per pixel in the shader, a radius test and an angle test, rather than outlines walked as paths.

`pinned` keeps the ring up, centred in its own field, and leaves it up after a pick: a radial control on the surface rather than a menu you summon. Unpinned, a press in the field raises one where the press landed, a release out along a wedge picks it, and Escape, a press outside, or a release back in the dead zone takes it down.

Keyboard: the digits 1 to 9 pick by position and Escape closes. **No arrow keys, deliberately** — in a ring an arrow would have to name a compass direction and a step through a list at once.

What it deliberately does NOT do: it does not remember what was picked (a control that must show its current setting is a radio group), and it has no submenus (a ring that opens another ring throws away the direction the first one just taught the hand). Labels are drawn at their measured width and are not shortened, so a ring is for short words.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Reach", target: "subject", kind: ControlKind::Number { prop: "radius", min: 50., max: 160., step: 2., default: 96. } },
        Control { label: "Dead zone", target: "subject", kind: ControlKind::Number { prop: "hub_radius", min: 12., max: 80., step: 2., default: 30. } },
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 12., step: 0.5, default: 2. } },
        Control { label: "Numbers", target: "subject", kind: ControlKind::Bool { prop: "show_numbers", default: true } },
        Control { label: "Pinned", target: "subject", kind: ControlKind::Bool { prop: "pinned", default: true } },
    ],
    on_actions: Some(pie_actions),
}];
