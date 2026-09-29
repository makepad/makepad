//! The screen view page under Containers: a view that is a display window,
//! with a bezel, a recessed face and glass over what it holds.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let ScreenCaption = Label{
        draw_text +: {color: theme.color_screen_ink text_style: theme.font_regular{font_size: theme.font_size_p * 0.8}}
    }

    mod.stories.ScreenViewOverview = StoryPage{
        StoryNote{text: "A container that is a display: a slim bezel, a face set into it with the surround's shadow under its top edge, and a sheet of glass over whatever it holds. The children draw between the face and the glass, so the sheen lies over the digits, not only on the empty face around them."}
        StoryHeading{text: "One screen, under the controls"}
        StoryRow{
            subject := ScreenView{
                ScreenCaption{text: "OUTPUT"}
                Readout{text: "-6.0" cells: 5 digit_height: 30.}
                ScreenCaption{text: "dB"}
            }
        }

        StoryHeading{text: "What it holds"}
        StoryNote{text: "Anything a view holds: a readout with its caption, a level ladder, a line of text. Captions on a screen take the screen's own ink, a pale one on the dark glass of the dark theme and a dark one on the pale glass of the light theme."}
        StoryRow{
            ScreenView{
                flow: Right
                spacing: theme.space_2
                align: Align{y: 1.}
                Readout{text: "12:45" digit_height: 24.}
                ScreenCaption{text: "PM"}
            }
            ScreenView{
                width: 180.
                ScreenCaption{text: "INPUT L / R"}
                LevelMeter{width: Fill height: 6. lamp: false level: 0.72 draw_bg.segment: 4.}
                LevelMeter{width: Fill height: 6. lamp: false level: 0.58 draw_bg.segment: 4.}
            }
            ScreenView{
                width: 180.
                ScreenCaption{text: "READY"}
                ScreenCaption{text: "Take 3 of 12, 00:41 recorded"}
            }
        }

        StoryHeading{text: "Bezel, recess, sheen and scan lines"}
        StoryNote{text: "No bezel is a face cut straight into the panel; a wider one frames it. The recess is how far the surround's shadow reaches down the face. The sheen is quiet on purpose: a band of a few percent from the top left and a lighter line under the top edge. Scan lines are off unless a page or a sheet asks for them."}
        StoryRow{
            ScreenView{bezel: 0.0 Readout{text: "1.0" digit_height: 22.}}
            ScreenView{bezel: 5.0 Readout{text: "2.0" digit_height: 22.}}
            ScreenView{recess: 0.0 Readout{text: "3.0" digit_height: 22.}}
            ScreenView{recess: 9.0 Readout{text: "4.0" digit_height: 22.}}
            ScreenView{sheen: 0.0 Readout{text: "5.0" digit_height: 22.}}
            ScreenView{scan: 0.3 Readout{text: "6.0" digit_height: 22.}}
        }
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "containers/screenview/overview",
        category: "Containers",
        component: "ScreenView",
        also: &[],
        name: "Overview",
        dsl: "ScreenViewOverview",
        added: "2026-09-28",
        tags: &["instruments", "new"],
        doc: "# ScreenView\n\nA view that is a display window: a slim bezel, the screen face recessed into it with an inner shadow under its top edge, and glass over the children it holds. The children draw between the face and the glass, so the sheen lies over them.\n\n- `bezel` is the ring's width in points; `recess` how far the surround's shadow reaches down the face; `sheen` the glass's strength; `scan` the depth of scan lines, off by default.\n- The face is `color_screen`, the theme's display glass. Put `Readout`s, labels and meters inside; a caption on a screen reads best in `color_screen_ink`.\n- Everything else is a view's: `flow`, `spacing`, `padding`, `align`, `width`, `height`.\n\nThe face is the view's `draw_bg` and the glass `draw_glass`; both read `bezel` (and the face `recess` and `scan`, the glass `sheen`) as instances the widget keeps in step with its properties, so a sheet may replace either pixel function and still draw to the page's geometry. The glass is one quad drawn after the children, inside the widget's own turtle.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Bezel", target: "subject", kind: ControlKind::Number { prop: "bezel", min: 0., max: 12., step: 0.5, default: 2. } },
            Control { label: "Recess", target: "subject", kind: ControlKind::Number { prop: "recess", min: 0., max: 12., step: 0.5, default: 4. } },
            Control { label: "Sheen", target: "subject", kind: ControlKind::Number { prop: "sheen", min: 0., max: 0.12, step: 0.005, default: 0.03 } },
            Control { label: "Scan lines", target: "subject", kind: ControlKind::Number { prop: "scan", min: 0., max: 0.5, step: 0.01, default: 0. } },
            Control { label: "Corner", target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 16., step: 0.5, default: 2. } },
        ],
        on_actions: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page builds, both of the screen's shaders compile, and the
    /// subject is a screen.
    #[test]
    fn the_page_builds_and_its_subject_is_a_screen() {
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
        assert_eq!(makepad_platform::shader_error::take(), None, "a shader on the page failed to compile");
        assert!(page.widget(&cx, ids!(subject)).borrow::<ScreenView>().is_some(), "no screen at subject");
    }
}
