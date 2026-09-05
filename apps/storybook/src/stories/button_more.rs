//! The button's variants: the semantic ladder, the sizes, icons before and
//! after the label, the wait state, the compound label, the icon-only
//! appearances, and the close, copy and burger buttons.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ButtonVariants = StoryPage{
        StoryHeading{text: "Semantic ladder"}
        StoryRow{
            ButtonPrimary{text: "Primary"}
            ButtonSecondary{text: "Secondary"}
            ButtonTertiary{text: "Tertiary"}
            ButtonOutline{text: "Outline"}
            ButtonDashed{text: "Dashed"}
            ButtonDanger{text: "Danger"}
        }
        StoryRow{
            ButtonPrimary{text: "Disabled" animator +: {disabled: {default: @on}}}
            ButtonOutline{text: "Disabled" animator +: {disabled: {default: @on}}}
            ButtonDanger{text: "Disabled" animator +: {disabled: {default: @on}}}
        }
        StoryHeading{text: "Sizes"}
        StoryRow{
            ButtonXs{text: "Extra small"}
            ButtonSm{text: "Small"}
            Button{text: "Default"}
            ButtonLg{text: "Large"}
            ButtonXl{text: "Extra large"}
        }
        StoryHeading{text: "Icon positions"}
        StoryRow{
            Button{
                text: "Leading"
                draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}
            }
            Button{
                text: "Trailing"
                draw_icon_end +: {svg: crate_resource("self:resources/mark_arrow.svg") color: theme.color_label_inner}
            }
            ButtonPrimary{
                text: "Both"
                draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}
                draw_icon_end +: {svg: crate_resource("self:resources/mark_arrow.svg")}
            }
        }
        StoryHeading{text: "Loading"}
        StoryNote{text: "A waiting button fades its label under a turning arc and ignores presses. The controls flip it; the button beside it does too."}
        StoryRow{
            subject := ButtonPrimary{text: "Save changes" loading: true}
            wait_toggle := Button{text: "Toggle loading"}
            clicks := Label{text: "not clicked yet"}
        }
        StoryHeading{text: "Compound"}
        StoryRow{
            ButtonCompound{
                text: "Export"
                description: "PDF, A4, every page"
                draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg") color: theme.color_label_inner}
            }
            ButtonCompound{
                text: "Discard"
                description: "Cannot be undone"
            }
        }
        StoryHeading{text: "Icon-only appearances"}
        StoryRow{
            ButtonIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
            ButtonFlatIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
            ButtonTonalIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}
            ButtonSubtleIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}
            ButtonFlatterIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
        }
        StoryHeading{text: "Close, copy, burger"}
        StoryRow{
            CloseButtonSm{}
            CloseButton{}
            CloseButtonLg{}
            copy := CopyButton{text_to_copy: "Copied from the catalogue"}
            burger := BurgerButton{}
            burger_label := Label{text: "burger: closed"}
        }
    }
}

fn button_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.button(cx, ids!(subject));
    if root.button(cx, ids!(wait_toggle)).clicked(actions) {
        let loading = !subject.loading();
        subject.set_loading(cx, loading);
    }
    if subject.clicked(actions) {
        let n = crate::stories::bump(live_id!(button_variants));
        root.label(cx, ids!(clicks))
            .set_text(cx, &format!("clicked {n} time{}", if n == 1 { "" } else { "s" }));
    }
    let burger = root.button(cx, ids!(burger));
    if burger.clicked(actions) {
        let open = !burger.open();
        burger.set_open(cx, open);
        root.label(cx, ids!(burger_label))
            .set_text(cx, if open { "burger: open" } else { "burger: closed" });
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "actions/button/variants",
    category: "Actions",
    component: "Button",
    name: "Variants",
    dsl: "ButtonVariants",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Button variants\n\nThe semantic ladder draws its faces from the theme's role tokens: `ButtonPrimary`, `ButtonSecondary`, `ButtonTertiary` (tonal), `ButtonOutline`, `ButtonDashed` and `ButtonDanger`. A `layer_color` on the face is laid over it by hover and press at the theme's state opacities.\n\n`ButtonXs`, `ButtonSm`, `ButtonLg` and `ButtonXl` take their heights from the control sizes. `draw_icon_end` puts a second icon after the label. `loading` swaps the label for a turning arc and blocks presses. `ButtonCompound` draws a `description` line under the label. `ButtonTonalIcon` and `ButtonSubtleIcon` carry an icon alone.\n\n`CloseButton` is a standalone cross in three sizes. `CopyButton` copies `text_to_copy` to the clipboard, shows a check and `Copied` for a moment, and raises `CopyButtonAction::Copied`. `BurgerButton` morphs three bars into a cross through `open`.\n\nThe controls drive the waiting button.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Save changes" } },
        Control { label: "Loading", target: "subject", kind: ControlKind::Bool { prop: "loading", default: true } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(button_actions),
}];
