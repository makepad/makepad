//! The needle meter page under Feedback: a needle on a printed scale that
//! eases to where it is sent.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.NeedleMeterOverview = StoryPage{
        StoryNote{text: "A value read by angle against a scale the eye already knows. The face carries the scale, major and minor ticks on an arc and a red zone at the loud end; the needle eases to a new value the way a needle with mass does, and the meter stops asking for frames when it lands."}
        StoryHeading{text: "One meter, under the controls"}
        StoryRow{
            subject := NeedleMeter{value: 0.62 label: "VU"}
        }

        StoryHeading{text: "Sent from Rust"}
        StoryNote{text: "set_value sends the needle; it eases over ease_secs. Press the buttons in quick succession and it turns from wherever it had got to."}
        StoryRow{
            meter := NeedleMeter{value: 0.2 label: "LOAD"}
            low := Button{text: "Low"}
            mid := Button{text: "Mid"}
            high := Button{text: "Into the red"}
        }

        StoryHeading{text: "Scales"}
        StoryNote{text: "The sweep, the divisions and the zone are the face's own: a wide sweep with ten divisions, a narrow one with no red zone, and a small meter."}
        StoryRow{
            NeedleMeter{width: 180. height: 110. value: 0.35 draw_bg.sweep: 120. draw_bg.majors: 10. draw_bg.minors: 2.}
            NeedleMeter{value: 0.5 draw_bg.sweep: 70. draw_bg.zone: 1.0 label: "BIAS"}
            NeedleMeter{width: 90. height: 56. value: 0.8 draw_bg.tick_major: 4. draw_bg.tick_minor: 2. draw_bg.pivot_size: 3.}
            NeedleMeter{value: 0.4 disabled: true label: "OFF"}
        }
    }
}

fn needle_meter_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let meter = root.needle_meter(cx, ids!(meter));
    if root.button(cx, ids!(low)).clicked(actions) {
        meter.set_value(cx, 0.15);
    }
    if root.button(cx, ids!(mid)).clicked(actions) {
        meter.set_value(cx, 0.5);
    }
    if root.button(cx, ids!(high)).clicked(actions) {
        meter.set_value(cx, 0.93);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/needlemeter/overview",
        category: "Feedback",
        component: "NeedleMeter",
        also: &[],
        name: "Overview",
        dsl: "NeedleMeterOverview",
        added: "2026-09-28",
        tags: &["instruments", "new"],
        doc: "# NeedleMeter\n\nA needle on a printed scale for a value from 0 to 1.\n\n- `value` is where the needle is sent; `set_value(cx, value)` eases it there over `ease_secs`, and the meter stops asking for frames when it lands. A value off the scale is held to it.\n- `label` prints a caption on the face under the scale.\n- The face, `draw_bg`, has the scale: `sweep` in degrees, `majors` and `minors` divisions, `tick_major` and `tick_minor` lengths, `zone` where the red zone starts (1 draws none), `pivot_size`, `border_radius` and a quiet `sheen`.\n- Colours: the face is `color_screen`, the scale and needle `color_screen_ink`, the zone `color_error`.\n\n`needle_angle(value, sweep)` is the mapping from a value to an angle, straight up at the middle of the scale; the shader runs the same arithmetic. The face reads `value` and `opacity` as instances and offers `pivot()`, `scale_radius()` and `needle_angle(v)` to a sheet that redraws it.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Value", target: "subject", kind: ControlKind::Number { prop: "value", min: 0., max: 1., step: 0.01, default: 0.62 } },
            Control { label: "Sweep", target: "subject", kind: ControlKind::Number { prop: "draw_bg.sweep", min: 30., max: 300., step: 1., default: 96. } },
            Control { label: "Majors", target: "subject", kind: ControlKind::Number { prop: "draw_bg.majors", min: 1., max: 20., step: 1., default: 5. } },
            Control { label: "Minors", target: "subject", kind: ControlKind::Number { prop: "draw_bg.minors", min: 1., max: 10., step: 1., default: 4. } },
            Control { label: "Red zone from", target: "subject", kind: ControlKind::Number { prop: "draw_bg.zone", min: 0., max: 1., step: 0.01, default: 0.8 } },
            Control { label: "Ease (s)", target: "subject", kind: ControlKind::Number { prop: "ease_secs", min: 0., max: 2., step: 0.05, default: 0.3 } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(needle_meter_actions),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_builds_and_every_id_it_is_driven_by_resolves() {
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
        assert_eq!(makepad_platform::shader_error::take(), None, "the meter's shader failed to compile");
        assert!(page.needle_meter(&cx, ids!(subject)).borrow().is_some(), "no subject");
        assert!(page.needle_meter(&cx, ids!(meter)).borrow().is_some(), "no meter to send");
        for button in [ids!(low), ids!(mid), ids!(high)] {
            assert!(page.button(&cx, button).borrow().is_some());
        }
    }
}
