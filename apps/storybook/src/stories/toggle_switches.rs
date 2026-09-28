//! The switches page under CheckBox: the toggle's state drawn as a rocker
//! and as a slide switch.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ToggleSwitchesOverview = StoryPage{
        StoryNote{text: "Two more faces for the toggle, driven by the state it already has: a rocker, whose pressed half sits darker and lower, and a slide switch, a short slot with a knob that carries grip lines. On and off differ by position first, then by the lit mark, and never by filling the whole control with the accent."}
        StoryHeading{text: "One of each, under the controls"}
        StoryRow{
            subject := ToggleRocker{text: "Power"}
            slide := ToggleSlide{text: "Monitor"}
        }

        StoryHeading{text: "The rocker"}
        StoryNote{text: "Off presses the O half, on the I half, whose legend takes the lit ink. A rocker flips on a click and does not drag: it has no knob to carry."}
        StoryRow{
            ToggleRocker{text: "Off"}
            ToggleRocker{text: "On" active: true}
            ToggleRocker{text: "Error" error: true}
            ToggleRocker{text: "Disabled" animator +: {disabled: {default: @on}}}
            ToggleRocker{text: "Disabled on" active: true animator +: {disabled: {default: @on}}}
        }

        StoryHeading{text: "The slide switch"}
        StoryNote{text: "The knob travels half the slot and follows a drag, as the toggle's does. The mark in the free end of the slot is an open ring while off and a filled dot in the accent while on."}
        StoryRow{
            ToggleSlide{text: "Off"}
            ToggleSlide{text: "On" active: true}
            ToggleSlide{text: "Error" error: true}
            ToggleSlide{text: "Disabled" animator +: {disabled: {default: @on}}}
            ToggleSlide{text: "Disabled on" active: true animator +: {disabled: {default: @on}}}
        }

        StoryHeading{text: "Sizes"}
        StoryNote{text: "The face is drawn from the toggle's size uniform; a label clears the stock size, so a larger switch wants a wider label margin as well."}
        StoryRow{
            ToggleRocker{text: "12" active: true draw_bg +: {size: 12.} label_walk +: {margin: Inset{left: 32.}}}
            ToggleRocker{text: "20" active: true draw_bg +: {size: 20.} label_walk +: {margin: Inset{left: 48.}}}
            ToggleSlide{text: "12" active: true draw_bg +: {size: 12.} label_walk +: {margin: Inset{left: 32.}}}
            ToggleSlide{text: "20" active: true draw_bg +: {size: 20.} label_walk +: {margin: Inset{left: 48.}}}
        }
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "selection/checkbox/switches",
        category: "Selection",
        component: "CheckBox",
        also: &["ToggleRocker", "ToggleSlide"],
        name: "Switches",
        dsl: "ToggleSwitchesOverview",
        added: "2026-09-28",
        tags: &["instruments", "rocker", "slide switch", "new"],
        doc: "# Rocker and slide switch\n\nTwo faces for the `Toggle`, named beside it and driven by the state it already has (`active`, `hover`, `down`, `focus`, `disabled`, `error`). Neither needs new Rust: they are the toggle with another `draw_bg.pixel`.\n\n- `ToggleRocker`: a housing split at the pivot; the pressed half is darker, has the housing's shadow across its top and its legend a device pixel lower. Off presses the O half, on the I half, whose legend takes `mark_color_active`. It does not drag. `rocker_color`, `rocker_color_pressed` and `legend_color` colour it.\n- `ToggleSlide`: a slot `pill_aspect` times as wide as it is tall, a square knob (`knob_color`) with grip lines at `grip_pitch`, and a mark in the free end of the slot: an open ring off, a filled dot in the accent on. The knob is placed exactly as the toggle places its own, so a drag and the knob icons line up with it.\n\nBoth take the toggle's fill, bevel and state colours from the theme, so a sheet that sets those restyles them, and a sheet may replace either face's `pixel` like any stock control's.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "On", target: "subject slide", kind: ControlKind::Bool { prop: "active", default: false } },
            Control { label: "Size", target: "subject slide", kind: ControlKind::Number { prop: "draw_bg.size", min: 10., max: 28., step: 1., default: 15. } },
            Control { label: "Width", target: "subject slide", kind: ControlKind::Number { prop: "draw_bg.pill_aspect", min: 1.5, max: 3., step: 0.05, default: 2. } },
            Control { label: "Corner", target: "subject slide", kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 12., step: 0.5, default: 4. } },
            Control { label: "Disabled", target: "subject slide", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_builds_and_both_faces_compile() {
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
        assert_eq!(makepad_platform::shader_error::take(), None, "a switch face failed to compile");
        assert!(page.check_box(&cx, ids!(subject)).borrow().is_some(), "no rocker at subject");
        assert!(page.check_box(&cx, ids!(slide)).borrow().is_some(), "no slide switch");
    }
}
