//! The readout page under Data display: numbers and short codes drawn as
//! seven-segment cells, with the unlit segments faintly behind.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ReadoutOverview = StoryPage{
        StoryNote{text: "A value as a display shows it: lit segments, and the ones it is not lighting still there behind them, faintly. That ghost is what makes a readout read as a display rather than as text in a digital-looking font. The digits carry no light beyond their own edge."}
        StoryHeading{text: "One readout, under the controls"}
        StoryNote{text: "Type into Text: digits, minus, point, colon and space, and the letters seven segments can draw. Cells fixes the row's length; Justify says which end the text sits against when it is shorter."}
        StoryRow{
            ScreenView{
                subject := Readout{text: "-12.50" digit_height: 28.}
            }
        }

        StoryHeading{text: "Numbers, times and codes"}
        StoryNote{text: "A point rides on the digit before it, as it does on a display, and a colon is a narrow cell of its own. A fixed row of cells keeps its width however the value changes, so the neighbours do not move."}
        StoryRow{
            ScreenView{Readout{text: "12:45:30" digit_height: 22.}}
            ScreenView{Readout{text: "-18.5" cells: 6 digit_height: 22.}}
            ScreenView{Readout{text: "440.0" digit_height: 22.}}
            ScreenView{Readout{text: "OFF" digit_height: 22. justify: ReadoutJustify.Left cells: 4}}
        }

        StoryHeading{text: "Set from Rust"}
        StoryNote{text: "set_number writes a value with its decimals. A number too long for a fixed row shows as dashes rather than a number with a digit missing."}
        StoryRow{
            ScreenView{number := Readout{text: "0.00" cells: 5 digit_height: 22.}}
            up := Button{text: "Add 1.25"}
            down := Button{text: "Take 12.5"}
            big := Button{text: "Too long"}
        }

        StoryHeading{text: "Ghost, slant and size"}
        StoryNote{text: "The ghost measured on the references sits between 7 and 15 percent of the lit ink; none at all reads as a font, much more reads as a broken display. A slant of 0.1 leans the digits about six degrees."}
        StoryRow{
            ScreenView{Readout{text: "17.4" digit_height: 22. ghost: 0.0}}
            ScreenView{Readout{text: "17.4" digit_height: 22. ghost: 0.1}}
            ScreenView{Readout{text: "17.4" digit_height: 22. ghost: 0.25}}
            ScreenView{Readout{text: "17.4" digit_height: 22. slant: 0.1}}
        }
        StoryRow{
            Readout{text: "123" digit_height: 12.}
            Readout{text: "123" digit_height: 16.}
            Readout{text: "123" digit_height: 24.}
            Readout{text: "123" digit_height: 40.}
            Readout{text: "123" digit_height: 24. disabled: true}
        }
    }
}

fn readout_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let number = root.readout(cx, ids!(number));
    let now = number.text().parse::<f64>().unwrap_or(0.0);
    if root.button(cx, ids!(up)).clicked(actions) {
        number.set_number(cx, now + 1.25, 2);
    }
    if root.button(cx, ids!(down)).clicked(actions) {
        number.set_number(cx, now - 12.5, 2);
    }
    if root.button(cx, ids!(big)).clicked(actions) {
        number.set_number(cx, 123456.0, 2);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "data-display/readout/overview",
        category: "Data display",
        component: "Readout",
        also: &[],
        name: "Overview",
        dsl: "ReadoutOverview",
        added: "2026-09-28",
        tags: &["instruments", "new"],
        doc: "# Readout\n\nA number or a short code drawn as seven-segment cells, the unlit segments faintly behind the lit ones.\n\n- `text` is turned into cells: one per digit, letter, minus or space; a point or comma rides on the cell before it; a colon is a narrow cell of its own. A character seven segments cannot draw is a blank.\n- `cells` fixes the row's length (0 is as long as the text); `justify` (`Right` or `Left`) says which end the text sits against, and a text longer than the row keeps that end.\n- `ghost` is how much of an unlit segment shows, as a share of the lit ink; the references measure 7 to 15 percent. It defaults to the theme's `screen_ghost`.\n- `slant` leans the digits (a run over the digit height; 0.1 is about six degrees), `digit_height`, `pitch` and `colon_pitch` size the cells.\n- `set_text(cx, text)` and `set_number(cx, value, decimals)`; a number too long for a fixed row shows as dashes.\n\nEvery cell is one quad with its segments in the instance, so every readout that shares an ink is one draw call, and nothing animates: a readout that is not being written costs nothing. The digits draw no halo. The face, `draw_bg`, reads `mask` (segments a to g as bits 1 to 64), `dot`, `colon`, `span`, `ghost`, `slant` and `opacity`, and offers `digit(q)` and `segment(...)` to a sheet that redraws it.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Text", target: "subject", kind: ControlKind::Text { prop: "text", default: "-12.50" } },
            Control { label: "Cells", target: "subject", kind: ControlKind::Number { prop: "cells", min: 0., max: 10., step: 1., default: 0. } },
            Control { label: "Justify", target: "subject", kind: ControlKind::Choice { prop: "justify", options: &["Right", "Left"], default: 0 } },
            Control { label: "Digit height", target: "subject", kind: ControlKind::Number { prop: "digit_height", min: 8., max: 64., step: 1., default: 28. } },
            Control { label: "Ghost", target: "subject", kind: ControlKind::Number { prop: "ghost", min: 0., max: 0.3, step: 0.01, default: 0.1 } },
            Control { label: "Slant", target: "subject", kind: ControlKind::Number { prop: "slant", min: 0., max: 0.25, step: 0.01, default: 0. } },
            Control { label: "Stroke", target: "subject", kind: ControlKind::Number { prop: "draw_bg.stroke", min: 0.05, max: 0.2, step: 0.005, default: 0.12 } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(readout_actions),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page builds, its face compiles, and every id the page is driven
    /// by resolves to the type it is driven as.
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
        assert_eq!(makepad_platform::shader_error::take(), None, "a shader on the page failed to compile");
        assert!(page.readout(&cx, ids!(subject)).borrow().is_some(), "no subject");
        assert!(page.readout(&cx, ids!(number)).borrow().is_some(), "no number to set");
        for button in [ids!(up), ids!(down), ids!(big)] {
            assert!(page.button(&cx, button).borrow().is_some());
        }
    }
}
