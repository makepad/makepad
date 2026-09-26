//! The radial menu story: rings of choices whose outer rings open in the
//! direction of their parent, so one flick outward picks two or three rings
//! deep. The page before this one is the same widget drawn in its own field.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.RadialMenuOverview = StoryPage{
        StoryNote{text: "A ring of choices around the point you pressed, where a choice can hold choices of its own. Those open as an outer ring on the same centre, and that ring is centred on the direction of the choice that opened it, so the hand keeps travelling the way it was already going. Only the direction within the open rings decides what is picked; how far out the pointer went never does."}

        StoryHeading{text: "Rings that keep the direction"}
        StoryNote{text: "Press anywhere in the panel and drag out through Share to reach Mail in one movement. Share's ring opens the moment the pointer crosses Share's outer edge, and the ring is centred on Share, so Mail is still up and to the right. Export opens a third ring the same way. Paste is there but cannot be picked."}
        StoryRow{
            View{
                width: Fill
                height: 420.
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                subject := RadialMenu{
                    width: Fill
                    height: Fill
                    items: [
                        RadialItem{key: "move" label: "Move"}
                        RadialItem{key: "share" label: "Share"}
                        RadialItem{key: "share/mail" label: "Mail"}
                        RadialItem{key: "share/link" label: "Link"}
                        RadialItem{key: "share/print" label: "Print"}
                        RadialItem{key: "share/export" label: "Export"}
                        RadialItem{key: "share/export/picture" label: "Picture"}
                        RadialItem{key: "share/export/document" label: "Document"}
                        RadialItem{key: "share/export/text" label: "Text"}
                        RadialItem{key: "rotate" label: "Rotate"}
                        RadialItem{key: "scale" label: "Scale"}
                        RadialItem{key: "edit" label: "Edit"}
                        RadialItem{key: "edit/cut" label: "Cut"}
                        RadialItem{key: "edit/copy" label: "Copy"}
                        RadialItem{key: "edit/paste" label: "Paste" enabled: false}
                        RadialItem{key: "delete" label: "Delete"}
                    ]
                }
            }
        }

        StoryHeading{text: "How an outer ring opens"}
        StoryNote{text: "Three ways. Crossing the outer edge of a choice that has more opens its ring at once, because the hand is plainly heading there. Resting on it opens the ring after a short delay. A press and release on it opens the ring too. The small arc just inside a wedge's rim says it has more, and once its ring is open a band along that rim marks which choice the ring belongs to."}
        StoryNote{text: "Moving back inward is how you say no: the band you move into closes every ring further out, and the hole in the middle closes them all."}

        StoryHeading{text: "What came back"}
        StoryNote{text: "A pick reports the full key of the choice and the position chosen on each ring on the way to it."}
        StoryRow{
            picked := Label{text: "nothing picked yet"}
        }

        StoryHeading{text: "A context menu"}
        StoryNote{text: "A right press or a long touch anywhere in this panel opens the rings. A left press goes through to what is underneath, so the button still counts its presses."}
        StoryRow{
            View{
                width: Fill
                height: 260.
                flow: Overlay
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: theme.space_2
                    align: Align{x: 0.5 y: 0.5}
                    under := Button{text: "Under the menu"}
                    under_count := Label{text: "not pressed yet"}
                }
                context := RadialMenuContext{
                    width: Fill
                    height: Fill
                    items: [
                        RadialItem{key: "open" label: "Open"}
                        RadialItem{key: "arrange" label: "Arrange"}
                        RadialItem{key: "arrange/front" label: "To front"}
                        RadialItem{key: "arrange/back" label: "To back"}
                        RadialItem{key: "remove" label: "Remove"}
                    ]
                }
            }
        }

        StoryHeading{text: "The keyboard"}
        StoryNote{text: "The whole menu is one tab stop. Tab to the first panel and press Enter or Space to raise the rings around its middle; then the digits pick on the outermost open ring, and its numbers move outward with it. A digit on a choice that has more opens its ring. Backspace closes the outermost ring and Escape closes the menu. There are no arrow keys: in a ring an arrow would have to mean a compass direction and a step through a list at the same time."}
        StoryNote{text: "While the rings are up, Tab and the mouse wheel do nothing. The menu holds the keyboard and the pointer, so the page cannot scroll the panel away from under the rings."}

        StoryHeading{text: "How it moves"}
        StoryNote{text: "The rings grow open on the theme's motion tokens: the first ring over a short theme duration, each outer ring over a shorter one, both on the emphasized decelerate curve. The Ease control hands the rings any of the theme's eight easings, the same curves Foundations > Motion plays side by side; change it and press in the first panel again. On the spring a ring swings past its edge and back, and what the pointer picks is still decided by the edge the ring comes to rest at, never by the swing."}
        StoryNote{text: "Nothing animates away. A pick, a cancel or a move inward takes rings down at once, because a ring still shrinking under the pointer would read as one that is still open. With reduced motion on, every ring is at its full size from the first frame."}

        StoryHeading{text: "Near an edge"}
        StoryNote{text: "The rings keep inside the window, not inside the panel that opened them. Scroll the page until the first panel reaches the top or the bottom of the window, then press in it close to that edge. The centre is moved in once, far enough for the deepest ring the menu can open and for the furthest its curve swings, so no ring is cut off by the window and opening an outer ring never moves what is under the pointer. A press that opened the menu has to travel a little before its release picks, so the wedge the nudge put under the pointer is not picked by accident."}

        StoryHeading{text: "A frosted look"}
        StoryNote{text: "The wedges show what is behind them, blurred and tinted toward their role, and the words stay sharp because they are drawn on top. Press over the picture to see it."}
        StoryRow{
            View{
                width: Fill
                height: 440.
                flow: Overlay
                Image{
                    width: Fill
                    height: Fill
                    src: crate_resource("self:resources/photo_landscape.jpg")
                    fit: ImageFit.Stretch
                }
                View{
                    width: Fill
                    height: Fill
                    flow: Right
                    spacing: 18.
                    align: Align{x: 0.5 y: 0.5}
                    RoundedView{width: 56. height: 240. draw_bg +: {color: theme.color_primary border_radius: 8.}}
                    RoundedView{width: 56. height: 240. draw_bg +: {color: theme.color_secondary border_radius: 8.}}
                    RoundedView{width: 56. height: 240. draw_bg +: {color: theme.color_tertiary border_radius: 8.}}
                    RoundedView{width: 56. height: 240. draw_bg +: {color: theme.color_error border_radius: 8.}}
                }
                frosted := RadialMenuFrosted{
                    width: Fill
                    height: Fill
                    items: [
                        RadialItem{key: "move" label: "Move"}
                        RadialItem{key: "share" label: "Share"}
                        RadialItem{key: "share/mail" label: "Mail"}
                        RadialItem{key: "share/link" label: "Link"}
                        RadialItem{key: "share/print" label: "Print"}
                        RadialItem{key: "rotate" label: "Rotate"}
                        RadialItem{key: "scale" label: "Scale"}
                        RadialItem{key: "delete" label: "Delete"}
                    ]
                }
            }
        }
        StoryNote{text: "Without a picture of the window to sample, the rings fall back to a flat face in the same colours, and are opaque either way, so the words stay readable."}
    }
}

fn radial_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for id in [ids!(subject), ids!(context), ids!(frosted)] {
        let menu = root.radial_menu(cx, id);
        if let Some(pick) = menu.picked(actions) {
            let path: Vec<String> = pick.path.iter().map(|i| i.to_string()).collect();
            let text = format!("{}, path {}", pick.key, path.join("."));
            root.label(cx, ids!(picked)).set_text(cx, &text);
        }
        if menu.cancelled(actions) {
            root.label(cx, ids!(picked)).set_text(cx, "taken down, nothing picked");
        }
    }
    if root.button(cx, ids!(under)).clicked(actions) {
        let count = crate::stories::bump(live_id!(radial_menu_under));
        let text = if count == 1 { "pressed once".to_string() } else { format!("pressed {count} times") };
        root.label(cx, ids!(under_count)).set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/pie-menu/radial-menu",
    category: "Overlay",
    component: "PieMenu",
    also: &["RadialMenu", "RadialMenuContext", "RadialMenuFrosted", "RadialItem"],
    name: "Radial menu",
    dsl: "RadialMenuOverview",
    added: "2026-09-13",
    tags: &["new", "radial", "menu", "rings", "glass"],
    doc: "# RadialMenu

A ring of choices around a point, where a choice can hold choices of its own.

A choice with children opens an **outer ring on the same centre**, and that ring's arc is centred on the parent wedge's own direction. The hand keeps travelling the way it was already going, so one continuous flick outward picks a choice two or three rings deep. The distance past a ring's inner edge never matters, only the direction within the rings that are open.

It floats over everything on an overlay of its own. That lets the rings reach past the panel that opened them, and it is what makes the frosted look possible. `overlay: false` draws the rings in the field instead and `pinned` keeps a ring up there; `PieMenu`, the page before this one, is that preset of this same widget.

## Items

A flat list, the key carrying the nesting:

```
RadialMenu{
    items: [
        RadialItem{key: \"share\" label: \"Share\"}
        RadialItem{key: \"share/mail\" label: \"Mail\"}
        RadialItem{key: \"share/print\" label: \"Print\" enabled: false}
    ]
}
```

A parent has to come before its children. An item with an empty segment, a missing parent, a key used twice, or more than three segments is dropped and named in the log. `set_items` takes a tree of `RadialNode`s from Rust instead.

`labels: [\"Cut\" \"Copy\"]` is the short way to say a flat ring: each word's position is its key, and `picked_index` reads that position back. It is read only when `items` is empty.

## Three rings at most

Ring 0 and two outer rings. With the default radii a full-depth menu is a 472-point disc, which fits a 600-point window with a margin. A fourth ring would need more than 540 points, and by then its arcs have been split three times.

## The arc rule

An outer ring starts as wide as its parent wedge and widens, about its middle, only until every child gets `min_item_arc` points at the ring's middle radius. **The middle of a child arc is always the parent's direction**, which is the property that keeps a flick's direction.

## The pick rule

Inside the hub, nothing. Past it, the deepest open ring whose arc holds the direction wins, once the pointer is past the ring beneath it: an outer ring owns the gap before it and everything outward, within its arc. A direction outside every outer arc falls back to ring 0, which owns every direction from the hub out. A flick that overshoots by a hundred points still picks what a careful one does.

## Opening a ring, and going back

Crossing a parent's outer edge opens its ring at once. Resting on a parent opens it after `open_delay`. A press and release on a parent opens it. Moving inward is back: the band the pointer moves into closes every ring more than one level further out, and the hub closes them all.

A release on a choice picks it and closes the menu. A release on a disabled choice does nothing and leaves the menu up. The press that opened the menu, let go where it opened, leaves the menu up for a second gesture, and it only picks once it has travelled a hub's width, because near an edge the centre is nudged in and would otherwise put a wedge under the press.

`trigger` chooses what opens it: a primary press in the field (`RadialTrigger.Press`), a secondary press or a long touch (`RadialTrigger.Secondary`, with `RadialMenuContext` as the preset), or only `open_at` from Rust (`RadialTrigger.Manual`).

## Keyboard

One tab stop, and Enter or Space on it opens the rings around the middle of the field. While the menu is up, the digits 1 to 9 act on the outermost open ring: a choice picks, a parent opens its ring and the numbers move out with it. Backspace closes the outermost ring, Escape closes the menu. Each number sits near its wedge's inner edge, moved off any word laid across the wedge.

While the menu is up it holds the keyboard and the pointer: Tab finds no stop to move to and the mouse wheel scrolls nothing, so the page under the rings stays where it was when they opened. A menu with `RadialTrigger.Manual` is no tab stop, since only Rust opens it.

## Near an edge

The centre is moved in from the window's edges once, when the menu opens, far enough for the deepest ring the tree can open and for the furthest `enter_ease` swings a ring past its edge. Opening an outer ring never moves what is under the pointer, and a spring near an edge is never cut off by the window.

## The open path

The parent whose ring is open is filled `color_wedge_open` and carries a 3-point band along its rim in `color_wedge_open_rim` (`theme.color_secondary`). The band is what marks it in every theme, including one whose open and resting faces are all but the same grey.

## In the test tree

The node's rect is the field, and its value spells the state: `open at=400,300 focus=1 hot=share/mail rings=share`. Each choice on the open rings is a `RadialMenuItem` part, found by its key's id or its word, whose rect is centred on the wedge the pick reads.

## The frosted look

`look: RadialLook.Frosted` (or `RadialMenuFrosted`) fills each wedge with the window behind it, blurred to `frost_level` and tinted `frost_tint` toward the wedge's role colour. The words are drawn on top and stay sharp. Before there is a capture, or with the blur turned off, the wedges show a flat fallback in the same colours, opaque either way.

## Motion

The rings grow open on the theme's motion tokens. `enter_secs` (default `theme.motion_short_3`) is how long the first ring takes, `ring_enter_secs` (default `theme.motion_short_2`) how long an outer ring takes, and `enter_ease` (default `theme.motion_ease_emphasized_decelerate`) is the curve both follow. Any of the theme's eight easings fits; Foundations > Motion plays them side by side.

```
RadialMenu{enter_ease: theme.motion_ease_spring}
```

Only the drawing follows the curve. A spring carries a ring past its edge and back, and the pick, the digits and the outside press read the edge the ring comes to rest at, so a ring can be picked from while it is still growing and is never picked from by its swing. The labels appear once a ring's time is up.

Nothing animates away: a pick, a cancel or a move inward takes its rings down at once, because a ring still shrinking under the pointer reads as a ring that is still open.

`reduced_motion: true` draws every ring at its full size at once.

## What it does not do

No memory of the last pick, pinned or not. No arrow keys. No more than three rings. A floating menu is never pinned: `pinned` always draws the ring in its field, because a menu over the window holds the pointer for as long as it is up. Labels are drawn at their measured width and are not shortened, so a ring is for short words.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Reach", target: "subject", kind: ControlKind::Number { prop: "radius", min: 60., max: 160., step: 2., default: 96. } },
        Control { label: "Dead zone", target: "subject", kind: ControlKind::Number { prop: "hub_radius", min: 12., max: 80., step: 2., default: 30. } },
        Control { label: "Ring width", target: "subject", kind: ControlKind::Number { prop: "ring_width", min: 32., max: 120., step: 2., default: 64. } },
        // A step of 0.01 and not 0.05: the slider floors the travel to a
        // step, and 0.3 / 0.05 is 5.999..., so the control showed 0.25 for a
        // menu at 0.3.
        Control { label: "Open delay", target: "subject", kind: ControlKind::Number { prop: "open_delay", min: 0., max: 1., step: 0.01, default: 0.3 } },
        Control { label: "Least item arc", target: "subject", kind: ControlKind::Number { prop: "min_item_arc", min: 24., max: 96., step: 2., default: 44. } },
        Control { label: "Numbers", target: "subject", kind: ControlKind::Bool { prop: "show_numbers", default: true } },
        Control {
            label: "Ease",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "enter_ease",
                options: &[
                    "theme.motion_ease_standard",
                    "theme.motion_ease_standard_decelerate",
                    "theme.motion_ease_standard_accelerate",
                    "theme.motion_ease_emphasized_decelerate",
                    "theme.motion_ease_emphasized_accelerate",
                    "theme.motion_ease_linear",
                    "theme.motion_ease_spring",
                    "theme.motion_ease_bounce",
                ],
                default: 3,
            },
        },
        Control { label: "Enter time", target: "subject", kind: ControlKind::Number { prop: "enter_secs", min: 0., max: 0.6, step: 0.01, default: 0.15 } },
        Control { label: "Ring enter time", target: "subject", kind: ControlKind::Number { prop: "ring_enter_secs", min: 0., max: 0.6, step: 0.01, default: 0.1 } },
        Control { label: "Reduced motion", target: "subject", kind: ControlKind::Bool { prop: "reduced_motion", default: false } },
        Control { label: "Look", target: "subject", kind: ControlKind::Choice { prop: "look", options: &["RadialLook.Solid", "RadialLook.Frosted"], default: 0 } },
        Control { label: "Frost", target: "frosted", kind: ControlKind::Number { prop: "frost_level", min: 0., max: 6., step: 0.25, default: 2.5 } },
        Control { label: "Tint", target: "frosted", kind: ControlKind::Number { prop: "frost_tint", min: 0., max: 1., step: 0.05, default: 0.55 } },
    ],
    on_actions: Some(radial_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::id_path;

    /// The page is markup, which the compiler never reads: the widget it
    /// documents is reached by name, its item records are parsed at run
    /// time, and a qualified enum that is misspelt only shows as a menu that
    /// never opens. Building the page is what turns any of that into a
    /// failed test rather than an empty panel in the catalogue.
    #[test]
    fn the_page_builds_and_resolves_its_subject_and_controls() {
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
        // The subject, every control target, and what the page's own action
        // handler reads and writes.
        for target in [story.subject, "picked", "context", "under", "under_count", "frosted"]
            .into_iter()
            .chain(story.controls.iter().map(|control| control.target))
            .filter(|target| !target.is_empty())
        {
            assert!(!page.widget(&cx, &id_path(target)).is_empty(), "no widget at {target}");
        }
        // Each menu is what the page says it is: the three rings deep, the
        // context trigger, the frosted look.
        for (target, deepest, trigger, look) in [
            ("subject", "share/export/picture", RadialTrigger::Press, RadialLook::Solid),
            ("context", "arrange/back", RadialTrigger::Secondary, RadialLook::Solid),
            ("frosted", "share/print", RadialTrigger::Press, RadialLook::Frosted),
        ] {
            let widget = page.widget(&cx, &id_path(target));
            let menu = widget
                .borrow::<RadialMenu>()
                .unwrap_or_else(|| panic!("{target} is not a RadialMenu"));
            assert_eq!(menu.trigger, trigger, "{target} opens another way");
            assert_eq!(menu.look, look, "{target} has another look");
            drop(menu);
            let label = page.radial_menu(&cx, &id_path(target)).label_of(LiveId::from_str(deepest));
            assert!(!label.is_empty(), "{target} dropped {deepest}");
        }
        // The Ease control offers the theme's easings by token and starts on
        // the one the menu already has, so the panel never names a curve the
        // rings are not on.
        let ease = story.controls.iter().find(|control| control.label == "Ease").expect("an Ease control");
        let ControlKind::Choice { prop, options, default } = &ease.kind else {
            panic!("Ease is a choice");
        };
        assert_eq!(*prop, "enter_ease");
        assert_eq!(options.len(), 8, "every theme easing");
        for option in options.iter() {
            assert!(option.starts_with("theme.motion_ease_"), "{option} is not a theme easing");
        }
        let shown = page.widget(&cx, &id_path("subject")).borrow::<RadialMenu>().map(|menu| menu.enter_ease);
        let token = options[*default].trim_start_matches("theme.");
        let named = crate::stories::foundations::theme_ease(&mut cx, token);
        assert_eq!(shown, Some(named), "the control starts on the menu's own easing");
        // And it names each curve in words the drop-down can show whole,
        // not by a token path whose first nineteen letters all eight share.
        let labels: Vec<String> = options.iter().map(|option| crate::controls::choice_label(option)).collect();
        assert_eq!(
            labels,
            [
                "Standard",
                "Standard decelerate",
                "Standard accelerate",
                "Emphasized decelerate",
                "Emphasized accelerate",
                "Linear",
                "Spring",
                "Bounce",
            ]
        );
    }

    /// Every number control shows its default as the slider will show it:
    /// the slider floors its travel to a step, and a default that is not a
    /// whole number of steps in floating point shows one step short, as
    /// Open delay showed 0.25 for 0.3. Each default is also the value the
    /// menu itself starts with, so the panel never names a value the menu
    /// is not at.
    #[test]
    fn every_number_control_shows_the_menus_own_default() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            WidgetRef::script_from_value(vm, value)
        });
        for control in story.controls {
            let ControlKind::Number { prop, min, max, step, default } = control.kind else {
                continue;
            };
            let travel = taper_to_travel(SliderTaper::Linear, default, min, max, default, step);
            let shown = taper_to_value(SliderTaper::Linear, travel, min, max, default, step);
            assert!((shown - default).abs() < 1e-9, "{} shows {shown} for its default {default}", control.label);
            let widget = page.widget(&cx, &id_path(control.target));
            let menu = widget.borrow::<RadialMenu>().expect("a radial menu");
            let actual = match prop {
                "radius" => menu.radius,
                "hub_radius" => menu.hub_radius,
                "ring_width" => menu.ring_width,
                "open_delay" => menu.open_delay,
                "min_item_arc" => menu.min_item_arc,
                "enter_secs" => menu.enter_secs,
                "ring_enter_secs" => menu.ring_enter_secs,
                "frost_level" => menu.frost_level,
                "frost_tint" => menu.frost_tint,
                other => panic!("no check for {other}"),
            };
            assert!((actual - default).abs() < 1e-9, "{} starts at {default} on a menu at {actual}", control.label);
        }
    }
}
