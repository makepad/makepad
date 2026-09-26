//! The pie menu story: the radial menu drawn in its own field, a ring that
//! reads a direction and nothing else, and the hole in the middle that lets
//! it read "not yet". `PieMenu` is a preset of `RadialMenu` and no widget of
//! its own; the rings that open rings are the next page.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.PieMenuOverview = StoryPage{
        StoryNote{text: "A list menu asks the hand for two things: a direction and a distance. Every row is a different way away and each one is a thin thing to stop on. A ring asks for a direction only. Past the hole in the middle a wedge goes on forever, so a flick that leaves the middle and stops anywhere along a wedge picks it, and the same flick means the same choice wherever the ring was opened."}
        StoryNote{text: "PieMenu is RadialMenu with its rings drawn in its own field instead of floating over the window: one widget, and this page and the next are its two ways of being on a surface. Every ring here is a flat one, filled from bare words with labels."}

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
        StoryNote{text: "A pick reports the position chosen on the ring, counted from zero the way the labels are; the page below turns that into the word and the key. The ring does not keep it: a control that has to show its current setting is a radio group and not a menu."}
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
        StoryNote{text: "The whole ring is one tab stop rather than one per wedge. Tab into the panel below and press Enter or Space, or press in it, to raise a ring; then the digits 1 to 9 pick by position and Escape takes it down. Tab moves on as it does from any control, and takes a summoned ring down with it. There are no arrow keys on purpose: in a ring an arrow would have to mean a compass direction and a step through a list at the same time."}
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
        StoryNote{text: "That one starts closed, the way a summoned menu does: raise one in its panel, then try a digit."}
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
        let menu = root.radial_menu(cx, id);
        if let Some(index) = menu.picked_index(actions) {
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
    key: "overlay/pie-menu/overview",
    category: "Overlay",
    component: "PieMenu",
    also: &[],
    name: "Overview",
    dsl: "PieMenuOverview",
    added: "2026-09-10",
    tags: &["new", "radial", "menu"],
    doc: "# PieMenu

`RadialMenu` drawn in its own field: a ring of choices on the surface rather than over the window.

`PieMenu` is a preset and not a widget of its own. It is `RadialMenu{overlay: false}` in a Fit box, and everything on the next page holds here too: the items, the rings that open rings, the pick rule, the digits. This page is about the two things the field adds.

A list menu asks the hand for a direction **and** a distance: every row is a different way away, and each one is a thin thing to stop on. A ring asks for a direction. Past the dead zone in the middle a wedge is unbounded, so a flick that leaves the hub and stops anywhere along a wedge picks it — and because the ring opens around the pointer, the same flick means the same thing wherever it was raised.

## In its field

`overlay: false` draws the rings in the field, in the same draw list as everything around them. The field is the room a ring may open in: a press in it raises the ring where the press landed, nudged in until all of it is inside the field, and a Fit field is exactly the ring's own box. Rings in a field are a control among others. They hold neither the pointer nor the keyboard of the rest of the window, they light up only under a pointer that is over their field, the wheel still scrolls the page, and Tab moves on. A press outside the field, Escape, a release back in the dead zone or losing the keyboard takes a summoned ring down.

The frosted look needs the overlay, since glass can only sample the window from there; a frosted ring in its field shows its flat face.

## Pinned

`pinned` keeps the ring up, centred in its field, and leaves it up after a pick: a radial control on the surface rather than a menu you summon. Nothing dismisses it, so Escape and a press elsewhere pass it by. A pinned ring is always drawn in its field, whatever `overlay` says, because a floating menu holds the pointer for as long as it is up.

## Labels

`labels` fills a flat ring from bare words, the first at twelve o'clock and the rest clockwise. A word has no key of its own, so its position is its key, and that position is what a host reads back:

```
if let Some(index) = menu.picked_index(actions) { ... }
```

`label_at(index)` gives the word again. `items` wins when both are given, and says everything `labels` can and more: keys, glyphs, disabled choices and rings of their own.

## The measure

`ArcRing` is the arithmetic, with its own tests: the wedge a direction picks, the span each wedge is drawn across, and where its label sits. Both the picking and the shader measure angles **clockwise from straight up**, and both get that measure from the same place — one measure in one place is what stops the wedge you see and the wedge you get from drifting apart by half a step. The wedges are two comparisons per pixel in the shader, a radius test and an angle test, rather than outlines walked as paths.

Keyboard: one tab stop; Enter or Space on it raises a summoned ring around the middle of its field, the digits 1 to 9 pick by position and Escape closes. **No arrow keys, deliberately** — in a ring an arrow would have to name a compass direction and a step through a list at once.

What it deliberately does NOT do: it does not remember what was picked (a control that must show its current setting is a radio group). Labels are drawn at their measured width and are not shortened, so a ring is for short words.",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::id_path;

    /// The page is markup, which the compiler never reads, and `PieMenu` is
    /// a name with no Rust type behind it: a preset that failed to register
    /// would only show as seven empty boxes in the catalogue. Building the
    /// page turns that into a failed test. Every ring on it is the radial
    /// menu drawn in its field, pinned where the page says so, holding the
    /// words the page gives it, and every control starts on the subject's own
    /// value.
    #[test]
    fn the_page_builds_and_every_ring_is_the_radial_menu_in_its_field() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile");
        assert!(!page.widget(&cx, &id_path("picked")).is_empty(), "nowhere to say what came back");

        for (target, pinned, words) in [
            ("subject", true, 6),
            ("summoned", false, 5),
            ("three", true, 3),
            ("five", true, 5),
            ("seven", true, 7),
            ("wide_hub", true, 4),
            ("keyed", false, 4),
        ] {
            let widget = page.widget(&cx, &id_path(target));
            let menu = widget
                .borrow::<RadialMenu>()
                .unwrap_or_else(|| panic!("{target} is not a RadialMenu"));
            assert!(!menu.overlay, "{target} floats over the window");
            assert_eq!(menu.pinned, pinned, "{target}");
            assert_eq!(menu.trigger, RadialTrigger::Press, "{target} opens another way");
            drop(menu);
            let ring = page.radial_menu(&cx, &id_path(target));
            assert!(!ring.label_at(words - 1).is_empty(), "{target} has fewer than {words} words");
            assert!(ring.label_at(words).is_empty(), "{target} has more than {words} words");
        }

        let widget = page.widget(&cx, &id_path(story.subject));
        let subject = widget.borrow::<RadialMenu>().expect("the subject");
        for control in story.controls {
            assert_eq!(control.target, story.subject, "{} steers something else", control.label);
            match control.kind {
                ControlKind::Number { prop, default, .. } => {
                    let actual = match prop {
                        "radius" => subject.radius,
                        "hub_radius" => subject.hub_radius,
                        "gap" => subject.gap,
                        other => panic!("no check for {other}"),
                    };
                    assert!((actual - default).abs() < 1e-9, "{} starts at {default} on a ring at {actual}", control.label);
                }
                ControlKind::Bool { prop, default } => {
                    let actual = match prop {
                        "show_numbers" => subject.show_numbers,
                        "pinned" => subject.pinned,
                        other => panic!("no check for {other}"),
                    };
                    assert_eq!(actual, default, "{}", control.label);
                }
                _ => panic!("{} is a kind of control this page never had", control.label),
            }
        }
    }
}
