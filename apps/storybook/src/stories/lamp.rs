//! The lamp page under Feedback: indicator lamps, round and bar, lit by an
//! amount that eases, with a halo held to the measured ceiling.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let LampCell = View{
        width: 64.
        height: Fit
        flow: Down
        spacing: theme.space_2
        align: Align{x: 0.5}
    }

    let LampNote = Label{
        draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: theme.font_size_p * 0.85}}
    }

    mod.stories.LampOverview = StoryPage{
        StoryNote{text: "A lamp says that something is on. Off, it still shows its lens, dark and faintly the colour it would light: an empty hole reads as a missing part. Lit, it shows its plain colour, and a halo that is light added to the ground, part of the lamp's light rather than a glow around it. On a pale ground the light theme draws none: added light there only bleaches the ground."}
        StoryHeading{text: "One lamp, under the controls"}
        StoryRow{
            subject := Lamp{lit: 1.0 size: 12.}
        }

        StoryHeading{text: "Intents, off, half and lit"}
        StoryNote{text: "The colour comes from the theme's roles: the accent is the primary colour, success, warning and error their own, and plain a neutral light that means nothing more than on."}
        StoryRow{
            LampCell{LampNote{text: "accent"} Lamp{intent: LampIntent.Accent} Lamp{intent: LampIntent.Accent lit: 0.5} Lamp{intent: LampIntent.Accent lit: 1.0}}
            LampCell{LampNote{text: "success"} Lamp{intent: LampIntent.Success} Lamp{intent: LampIntent.Success lit: 0.5} Lamp{intent: LampIntent.Success lit: 1.0}}
            LampCell{LampNote{text: "warning"} Lamp{intent: LampIntent.Warning} Lamp{intent: LampIntent.Warning lit: 0.5} Lamp{intent: LampIntent.Warning lit: 1.0}}
            LampCell{LampNote{text: "error"} Lamp{intent: LampIntent.Error} Lamp{intent: LampIntent.Error lit: 0.5} Lamp{intent: LampIntent.Error lit: 1.0}}
            LampCell{LampNote{text: "plain"} Lamp{intent: LampIntent.Plain} Lamp{intent: LampIntent.Plain lit: 0.5} Lamp{intent: LampIntent.Plain lit: 1.0}}
        }

        StoryHeading{text: "Bars"}
        StoryNote{text: "The same lamp drawn long: three thicknesses by default, or the length it is given."}
        StoryRow{
            LampBar{intent: LampIntent.Success lit: 1.0}
            LampBar{intent: LampIntent.Warning lit: 1.0 size: 5. length: 28.}
            LampBar{intent: LampIntent.Error}
            LampBar{intent: LampIntent.Plain lit: 1.0 size: 4. length: 40.}
        }

        StoryHeading{text: "Easing on and off"}
        StoryNote{text: "set_lit eases the lamp to an amount: it comes on and goes out rather than blinking, and stops asking for frames the moment it arrives."}
        StoryRow{
            eased := Lamp{intent: LampIntent.Success size: 12.}
            on := Button{text: "On"}
            half := Button{text: "Half"}
            off := Button{text: "Off"}
        }

        StoryHeading{text: "The halo's ceiling"}
        StoryNote{text: "Measured on the reference collection, a lamp's light one device pixel outside it is 20 to 45 percent of the lamp's own, halves within a third to a half of the lamp's thickness, and is gone within one thickness. The two properties that move the halo end at that ceiling; asking for more draws the ceiling. From the left: no halo, the theme's, the ceiling, and a request for twice the ceiling."}
        StoryRow{
            Lamp{intent: LampIntent.Accent lit: 1.0 size: 14. halo: 0.0}
            Lamp{intent: LampIntent.Accent lit: 1.0 size: 14.}
            Lamp{intent: LampIntent.Accent lit: 1.0 size: 14. halo: 0.45 halo_reach: 1.0}
            Lamp{intent: LampIntent.Accent lit: 1.0 size: 14. halo: 0.9 halo_reach: 3.0}
        }
    }
}

fn lamp_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let lamp = root.lamp(cx, ids!(eased));
    if root.button(cx, ids!(on)).clicked(actions) {
        lamp.set_lit(cx, 1.0);
    }
    if root.button(cx, ids!(half)).clicked(actions) {
        lamp.set_lit(cx, 0.5);
    }
    if root.button(cx, ids!(off)).clicked(actions) {
        lamp.set_lit(cx, 0.0);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/lamp/overview",
        category: "Feedback",
        component: "Lamp",
        also: &["LampBar"],
        name: "Overview",
        dsl: "LampOverview",
        added: "2026-09-28",
        tags: &["instruments", "new"],
        doc: "# Lamp\n\nAn indicator lamp, round or a bar, lit by an amount from 0 to 1 that eases.\n\n- `intent` (`Accent`, `Success`, `Warning`, `Error`, `Plain`) takes the colour of the theme role of the same name; `Accent` is the primary colour.\n- `lit` is how far lit; `set_lit(cx, amount)` eases there over `ease_secs`, and the lamp stops asking for frames when it arrives.\n- Off is a dark lens (`color_lamp_off`) faintly tinted by the intent, with its rim: never an empty hole. Lit is the plain colour; `draw_bg.core` pales the middle toward white for a sheet that wants a hot centre, and is 0 by default.\n- `shape` is `Round` or `Bar` (`LampBar`); `size` is the diameter or the bar's thickness, `length` a bar's length.\n- `halo` is the halo one device pixel out, as a share of the lamp's light, and `halo_reach` how far it goes in lamp thicknesses. Their ranges end at the measured ceiling, 0.45 and one thickness; the widget and the shader both clamp them, so nothing can make a lamp glow louder than that. The halo halves at 0.4 of its reach and is gone at the reach. The halo is light added to the ground, so it never darkens it. `halo` defaults to the theme's `lamp_halo`, which the light bases set to 0; a disabled lamp draws none.\n\nThe layout box holds the lens and, when `halo` is above 0, the halo's room. The face, `draw_bg`, reads `lit`, `intent`, `bar`, `halo`, `reach` and `opacity`, and offers `intent_color()` and `halo_at(...)` to a sheet that redraws it. Lamps of every intent batch into one draw call.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Lit", target: "subject", kind: ControlKind::Number { prop: "lit", min: 0., max: 1., step: 0.01, default: 1. } },
            Control { label: "Intent", target: "subject", kind: ControlKind::Choice { prop: "intent", options: &["Accent", "Success", "Warning", "Error", "Plain"], default: 0 } },
            Control { label: "Shape", target: "subject", kind: ControlKind::Choice { prop: "shape", options: &["Round", "Bar"], default: 0 } },
            Control { label: "Size", target: "subject", kind: ControlKind::Number { prop: "size", min: 3., max: 32., step: 0.5, default: 12. } },
            Control { label: "Halo", target: "subject", kind: ControlKind::Number { prop: "halo", min: 0., max: 0.45, step: 0.01, default: 0.25 } },
            Control { label: "Halo reach", target: "subject", kind: ControlKind::Number { prop: "halo_reach", min: 0., max: 1., step: 0.05, default: 1. } },
            Control { label: "Pale core", target: "subject", kind: ControlKind::Number { prop: "draw_bg.core", min: 0., max: 1., step: 0.05, default: 0. } },
        ],
        on_actions: Some(lamp_actions),
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
        assert_eq!(makepad_platform::shader_error::take(), None, "the lamp's shader failed to compile");
        assert!(page.lamp(&cx, ids!(subject)).borrow().is_some(), "no subject");
        assert!(page.lamp(&cx, ids!(eased)).borrow().is_some(), "no lamp to ease");
        for button in [ids!(on), ids!(half), ids!(off)] {
            assert!(page.button(&cx, button).borrow().is_some());
        }
    }
}
