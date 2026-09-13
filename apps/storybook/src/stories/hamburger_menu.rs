//! The hamburger menu story: one set of destinations shown three ways, and
//! the same menu folding when the room runs out.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // A small window for a menu to stand in: a bar along the top and room
    // under it, so a panel dropped under the button has somewhere to fall
    // that is still inside the frame.
    let HamburgerFrame = RoundedView{
        width: Fill
        height: 300.
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_2
        draw_bg +: {color: theme.color_surface_container_low}
    }

    let HamburgerBar = View{
        width: Fill
        height: 56.
        flow: Right
        spacing: theme.space_3
        align: Align{y: 0.5}
        padding: Inset{left: 8. right: 8. top: 0. bottom: 0.}
    }

    let HamburgerBarTitle = Label{
        draw_text +: {
            text_style: theme.font_title_s
            color: theme.color_on_surface
        }
    }

    mod.stories.HamburgerMenuOverview = StoryPage{
        StoryNote{text: "The navigation of a narrow window: a row of destinations while there is room, and a button that brings the same destinations out when there is not. Every part of it already existed. What the menu adds is the wiring each app would otherwise write by hand: one set of destinations feeding three lists, one of them lit wherever it was chosen, and one report."}

        StoryHeading{text: "Into a drawer"}
        StoryNote{text: "Collapsed, the destinations live behind the button. Press it and they come in from the side; choose one and they go back, and the button's cross turns back into bars."}
        HamburgerFrame{
            HamburgerBar{
                subject := HamburgerMenu{
                    mode: HamburgerMode.Collapsed
                    surface: HamburgerSurface.Drawer
                    drawer_title: "Places"
                    labels: ["Home" "Library" "History" "Settings"]
                }
                HamburgerBarTitle{text: "Library"}
            }
            place := Label{text: "You are at Home"}
        }

        StoryHeading{text: "Under the button"}
        StoryNote{text: "The same destinations in a panel dropped under the button, for a window too wide to want a drawer but too narrow for a row."}
        HamburgerFrame{
            HamburgerBar{
                drop := HamburgerMenu{
                    mode: HamburgerMode.Collapsed
                    surface: HamburgerSurface.Drop
                    labels: ["Home" "Library" "History" "Settings"]
                }
                HamburgerBarTitle{text: "Library"}
            }
        }

        StoryHeading{text: "Coming out and going away"}
        StoryNote{text: "The panel comes out along the theme's emphasized decelerate curve and goes away along its standard accelerate curve, and the bars turn on the same curve at the same time. The Enter ease and Exit ease controls give the drawer at the top any of the theme's eight easings, the curves Foundations > Motion plays side by side: change one and open the drawer again. On the spring the drawer stretches past its width and settles without lifting off the window's edge. The drawer and the popover let go the moment the panel is put away, so a press made while it is still leaving lands on whatever is under it. With Reduced motion on, the panel and the bars land at once."}

        StoryHeading{text: "One place you are, three ways of showing it"}
        StoryNote{text: "Choosing in any of them lights the same destination in all of them."}
        StoryRow{
            inline := HamburgerMenu{
                mode: HamburgerMode.Inline
                labels: ["Home" "Library" "History" "Settings"]
            }
        }

        StoryHeading{text: "Arrows look, a press chooses"}
        StoryNote{text: "In the panel the arrows move through the destinations and choose as they go, without putting the panel away, so you can look. Return, a press, Escape or a press outside puts it away."}

        StoryHeading{text: "Folding when the room runs out"}
        StoryNote{text: "Wider than the breakpoint the destinations sit in the bar. Narrower, they fold behind the button. They come back out only a little past the breakpoint, so a window resting on the line does not flicker. The frame's width is a control: drag it across the line and back."}
        // No padding across the frame or its bar: the menu measures the room
        // its parent leaves, and any inset would put the fold that far from
        // the width the control shows, a different distance in each theme.
        // No wider than the canvas the catalogue opens with, either: a frame
        // cut off at the canvas's edge measures room nobody can see, and the
        // fold would not line up with what the reader sees.
        frame := RoundedView{
            width: 700.
            height: Fit
            flow: Down
            spacing: theme.space_2
            padding: Inset{left: 0. right: 0. top: theme.space_2 bottom: theme.space_2}
            draw_bg +: {color: theme.color_surface_container_low}
            HamburgerBar{
                padding: Inset{left: 0. right: 0. top: 0. bottom: 0.}
                responsive := HamburgerMenu{
                    measure: HamburgerMeasure.Parent
                    breakpoint: 640.
                    labels: ["Overview" "Projects" "People" "Reports" "Settings"]
                }
            }
            mode_label := Label{margin: Inset{left: 8.} text: "in the bar"}
        }
    }
}

fn hamburger_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // Each menu keeps its own three lists in step. Keeping three separate
    // menus in step is the app's job, and this is all of it.
    let menus: Vec<HamburgerMenuRef> = [ids!(subject), ids!(drop), ids!(inline)]
        .iter()
        .map(|path| root.hamburger_menu(cx, *path))
        .collect();
    for (at, menu) in menus.iter().enumerate() {
        if let Some(id) = menu.chosen(actions) {
            for (other_at, other) in menus.iter().enumerate() {
                if other_at != at {
                    other.select(cx, id);
                }
            }
            let place = menu.text();
            root.label(cx, ids!(place))
                .set_text(cx, &format!("You are at {place}"));
        }
    }
    if let Some(collapsed) = root.hamburger_menu(cx, ids!(responsive)).mode_changed(actions) {
        let said = if collapsed { "folded behind the button" } else { "in the bar" };
        root.label(cx, ids!(mode_label)).set_text(cx, said);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/hamburger-menu/overview",
    category: "Navigation",
    component: "HamburgerMenu",
    also: &["BurgerButton"],
    name: "Overview",
    dsl: "HamburgerMenuOverview",
    added: "2026-09-13",
    tags: &["new"],
    doc: "# HamburgerMenu

The navigation of a narrow window. A row of destinations shows while there is room; below a width it folds into a burger button that brings the same destinations out, in a drawer from the side or in a panel dropped under the button. The bars turn into a cross while the destinations are out and back when they go away.

**Every part already exists** — `BurgerButton`, `NavList`, `Drawer` and `Popover`. What does not is the wiring every app would otherwise write by hand: one set of destinations feeding three lists, a choice in any of them lighting the other two, the button following two different surfaces' open and close, and a switch that does not flicker when the window rests on its breakpoint.

## Three views, always built

The row, the drawer's list and the drop panel's list are all built up front and only shown or hidden. A view rebuilt at every crossing of the breakpoint would throw away the lit destination and an open panel each time the window was dragged across the line.

| Property | What it does |
|---|---|
| `labels` | the destinations, by name; ids are 1, 2, 3 in order |
| `mode` | `HamburgerMode.Responsive` folds below `breakpoint`; `Collapsed` always, `Inline` never |
| `surface` | `HamburgerSurface.Drawer` or `HamburgerSurface.Drop` |
| `measure` | `HamburgerMeasure.Window` compares the window's width; `Parent` the room the parent leaves |
| `breakpoint`, `hysteresis` | a folded menu unfolds only at `breakpoint + hysteresis` |
| `close_on_pick` | a press or Return in the panel puts it away |
| `drawer_side`, `drawer_size`, `drawer_title` | written to the drawer |
| `drop_width` | the drop panel's width |
| `enter_secs`, `enter_ease` | how long the panel takes to come out and the curve it comes out on; `theme.motion_medium_2` and `theme.motion_ease_emphasized_decelerate` |
| `exit_secs`, `exit_ease` | how long it takes to go away and the curve it goes on; `theme.motion_short_4` and `theme.motion_ease_standard_accelerate` |
| `reduced_motion` | the panel and the bars land at once, without movement |

`Window` is right anywhere, including a bar slot that sizes itself and so has no room of its own to report.

## Arrows look, a press chooses

A nav list chooses as its arrows move, so a panel that closed on every choice would go away on the first arrow and nobody could look down the list. What decides is where the choice came from: arrows, Home and End never put the panel away; a press and release on the list, or Return or Space, do. The close happens on the next frame, after the choice has been reported, because a drawer that closes stops forwarding events at once and would swallow the press it was closing for.

Escape, a press on the scrim or outside the drop panel, and the menu unfolding under an open panel put it away too.

## Coming and going

The drawer slides on a curve of its own and the popover does not move, so the menu moves the panel itself. Each surface places a holder that rests where the panel belongs; the surface hit-tests that holder and hands it the keyboard, so neither waits for the panel to arrive. The panel inside comes out along `enter_ease` over `enter_secs`: the drawer's panel slides in from its edge, and the drop panel grows away from the button, down or up depending on where the popover hung it. Put away, the surface lets go at once and the menu draws the panel going back along `exit_ease` over `exit_secs` on an overlay of its own, where it takes no press and no key. Opened again on its way out, it turns round from where it had got to.

The panel is always laid out where it rests, and only a view transform carries its pixels along the travel. A press made while it is still coming out lands on the row that will rest under it, and nothing clips the growing panel, so its shadow is whole.

The bars turn to the cross and back on the same curve at the same time. A button's own open track has a fixed length and curve, so the menu writes the turn itself rather than playing that track.

Any of the theme's eight easings fits either curve; Foundations > Motion plays them side by side. On the spring a drawer stretches past its width and settles, its outer side staying on the window's edge.

```
HamburgerMenu{enter_ease: theme.motion_ease_spring exit_ease: theme.motion_ease_emphasized_accelerate}
```

The panels are reached for styling inside their holders, under the part that moves them: `drawer +: { content +: { drawer_motion +: { drawer_sheet +: { body +: {...} } } } }` and `burger_pop +: { content +: { drop_motion +: { drop_sheet +: {...} } } }`.

## The keyboard

Brought out from the burger, by a press or by Return or Space on it, the panel gives the keyboard back to the burger when it goes away, so Return brings it out again. In the drawer the arrows reach the list at once. The drawer and the drop panel both stop the page under them: a press on a row lands on the row, and a press beside the panel goes to the scrim or the outside press, never to what lies underneath.

## What it reports

| Action | When |
|---|---|
| `Selected(id)` | a destination was chosen in any of the three lists, once |
| `Opened` | the destinations came out |
| `Closed` | they went away, whichever way |
| `ModeChanged(collapsed)` | the menu folded or unfolded |

`chosen`, `opened`, `closed` and `mode_changed` read them. `select` lights a destination in all three lists and reports nothing; `set_destinations` hands the same destinations to all three.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Surface", target: "subject", kind: ControlKind::Choice { prop: "surface", options: &["HamburgerSurface.Drawer", "HamburgerSurface.Drop"], default: 0 } },
        Control { label: "Drawer side", target: "subject", kind: ControlKind::Choice { prop: "drawer_side", options: &["PanelEdge.Left", "PanelEdge.Right"], default: 0 } },
        Control { label: "Close on pick", target: "subject", kind: ControlKind::Bool { prop: "close_on_pick", default: true } },
        Control {
            label: "Enter ease",
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
        Control {
            label: "Exit ease",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "exit_ease",
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
                default: 2,
            },
        },
        Control { label: "Reduced motion", target: "subject", kind: ControlKind::Bool { prop: "reduced_motion", default: false } },
        Control { label: "Frame width", target: "frame", kind: ControlKind::Number { prop: "width", min: 320., max: 720., step: 10., default: 700. } },
        Control { label: "Breakpoint", target: "responsive", kind: ControlKind::Number { prop: "breakpoint", min: 240., max: 1200., step: 10., default: 640. } },
        Control { label: "Hysteresis", target: "responsive", kind: ControlKind::Number { prop: "hysteresis", min: 0., max: 96., step: 1., default: 16. } },
    ],
    on_actions: Some(hamburger_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::id_path;

    /// The page is markup, which the compiler never reads: the menus are
    /// reached by name, their enums are written out by name, and every
    /// control writes a property the page only claims to have. Building the
    /// page turns a mistake in any of that into a failed test rather than an
    /// empty bar in the catalogue.
    #[test]
    fn the_page_builds_and_every_menu_is_the_one_the_page_describes() {
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
        assert_eq!(
            makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        // The subject, every control's target, and the labels the page's
        // own handler writes to.
        for target in [story.subject, "drop", "inline", "place", "mode_label"]
            .into_iter()
            .chain(story.controls.iter().map(|control| control.target))
            .filter(|target| !target.is_empty())
        {
            assert!(!page.widget(&cx, &id_path(target)).is_empty(), "no widget at {target}");
        }
        for (target, mode, surface, measure) in [
            ("subject", HamburgerMode::Collapsed, HamburgerSurface::Drawer, HamburgerMeasure::Window),
            ("drop", HamburgerMode::Collapsed, HamburgerSurface::Drop, HamburgerMeasure::Window),
            ("inline", HamburgerMode::Inline, HamburgerSurface::Drawer, HamburgerMeasure::Window),
            ("responsive", HamburgerMode::Responsive, HamburgerSurface::Drawer, HamburgerMeasure::Parent),
        ] {
            let widget = page.widget(&cx, &id_path(target));
            let menu = widget
                .borrow::<HamburgerMenu>()
                .unwrap_or_else(|| panic!("{target} is not a HamburgerMenu"));
            assert_eq!(menu.mode, mode, "{target} is not folded the way the page says");
            assert_eq!(menu.surface, surface, "{target} does not open where the page says");
            assert_eq!(menu.measure, measure, "{target} does not measure what the page says");
            assert!(!menu.labels.is_empty(), "{target} has no destinations");
        }
        assert_eq!(page.widget(&cx, &id_path("subject")).borrow::<HamburgerMenu>().unwrap().drawer_title, "Places");
        assert_eq!(page.widget(&cx, &id_path("responsive")).borrow::<HamburgerMenu>().unwrap().breakpoint, 640.0);
        // The Ease controls offer the theme's easings by token and start on
        // the ones the menu already has, so the panel never names a curve
        // the menu is not on.
        for (label, prop) in [("Enter ease", "enter_ease"), ("Exit ease", "exit_ease")] {
            let control = story.controls.iter().find(|control| control.label == label).unwrap_or_else(|| panic!("no {label} control"));
            let ControlKind::Choice { prop: written, options, default } = &control.kind else {
                panic!("{label} is a choice");
            };
            assert_eq!(*written, prop);
            assert_eq!(options.len(), 8, "every theme easing");
            let mut shown_as = Vec::new();
            for option in options.iter() {
                assert!(option.starts_with("theme.motion_ease_"), "{option} is not a theme easing");
                shown_as.push(crate::controls::choice_label(option));
            }
            // Read as the names Foundations > Motion gives them, never as the
            // token paths a narrow drop-down cuts to one shared prefix.
            assert_eq!(
                shown_as,
                ["Standard", "Standard decelerate", "Standard accelerate", "Emphasized decelerate", "Emphasized accelerate", "Linear", "Spring", "Bounce"],
                "{label} shows its easings by name"
            );
            let shown = page
                .widget(&cx, &id_path(control.target))
                .borrow::<HamburgerMenu>()
                .map(|menu| if prop == "enter_ease" { menu.enter_ease } else { menu.exit_ease });
            let token = options[*default].trim_start_matches("theme.");
            let named = crate::stories::foundations::theme_ease(&mut cx, token);
            assert_eq!(shown, Some(named), "{label} starts on the menu's own easing");
        }
        // The responsive menu folds on the room its parent leaves, so the
        // frame the width control sizes adds no inset across it: the fold
        // then sits at the width the control shows, in every theme.
        let frame = page.widget(&cx, &id_path("frame"));
        let frame = frame.borrow::<View>().expect("the frame is a view");
        assert_eq!((frame.layout.padding.left, frame.layout.padding.right), (0.0, 0.0));

        // The frame, at its default width and at the widest the control
        // offers, fits the canvas the catalogue opens with: the width the
        // menu measures is then the width the reader sees.
        let page_padding = page.borrow::<View>().expect("the page is a view").layout.padding;
        let room = OPENING_CANVAS_WIDTH - page_padding.left - page_padding.right;
        let width = story.controls.iter().find(|control| control.label == "Frame width").expect("a Frame width control");
        let ControlKind::Number { max, default, .. } = width.kind else {
            panic!("Frame width is a number");
        };
        assert_eq!(frame.walk.width, Size::Fixed(default), "the frame starts at the control's default");
        assert!(max <= room, "the widest frame, {max}, is cut off by a canvas with {room} of room");
        // Wide enough at its default to show the row, so the page opens on
        // the bar and a drag narrower folds it.
        let responsive = page.widget(&cx, &id_path("responsive"));
        let responsive = responsive.borrow::<HamburgerMenu>().expect("the responsive menu");
        assert!(default >= responsive.breakpoint + responsive.hysteresis, "the page opens folded");
    }

    /// The width the canvas has when the catalogue opens (`app.rs`): the
    /// 1400-point window less the navigator's 260, the bar beside it, the
    /// side panels' 380 and the canvas holder's padding on either side.
    const OPENING_CANVAS_WIDTH: f64 = 1400.0 - 260.0 - 6.0 - 380.0 - 2.0 * 6.0;
}
