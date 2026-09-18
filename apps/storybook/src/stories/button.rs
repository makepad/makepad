//! The button story: one button under the controls, what a press and a wait
//! look like, every face and size in rows, the link face, the close, copy
//! and burger buttons, and the styling reference last.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ButtonOverview = StoryPage{
        StoryHeading{text: "One button, under the controls"}
        StoryNote{text: "One button. The first four controls on the right write into it."}
        StoryRow{
            subject := Button{text: "Button"}
        }
        StoryRow{
            clicks := Label{text: "not clicked yet"}
        }

        StoryHeading{text: "What a press does"}
        StoryNote{text: "A button reports the press and keeps nothing, so what a press means is up to the host. These two count their presses into their own labels."}
        StoryRow{
            basicbutton := Button{text: "Count presses"}
            iconbutton := Button{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
                text: "With an icon"
            }
        }

        StoryHeading{text: "Loading"}
        StoryNote{text: "A waiting button fades its label under a turning arc and ignores presses, so a slow action cannot be started twice. The last three controls drive it, and so does the button beside it."}
        StoryRow{
            waiting := ButtonPrimary{text: "Save changes" loading: true}
            wait_toggle := Button{text: "Toggle loading"}
            waiting_clicks := Label{text: "not clicked yet"}
        }

        StoryHeading{text: "Faces"}
        StoryNote{text: "The semantic ladder takes its faces from the theme's role tokens, so the main action, a quiet one and a destructive one read apart in every theme."}
        StoryRow{
            ButtonPrimary{text: "Primary"}
            ButtonSecondary{text: "Secondary"}
            ButtonTertiary{text: "Tertiary"}
            ButtonOutline{text: "Outline"}
            ButtonDashed{text: "Dashed"}
            ButtonDanger{text: "Danger"}
        }
        StoryNote{text: "The standard face is a flat fill with the theme's bevel. The gradient faces fill it with two stops, the flat face drops the bevel, and the flatter one shows no face at all until it is disabled."}
        StoryRow{
            Button{text: "Button"}
            ButtonGradientX{text: "ButtonGradientX"}
            ButtonGradientY{text: "ButtonGradientY"}
            ButtonFlat{text: "ButtonFlat"}
            ButtonFlatter{text: "ButtonFlatter"}
        }
        StoryNote{text: "A link is a button drawn as text over a hairline, for an action that sits in a sentence or a list of links. A compound button adds a description line under its label."}
        StoryRow{
            LinkLabel{text: "Open the report"}
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

        StoryHeading{text: "Sizes"}
        StoryRow{
            ButtonXs{text: "Extra small"}
            ButtonSm{text: "Small"}
            Button{text: "Default"}
            ButtonLg{text: "Large"}
            ButtonXl{text: "Extra large"}
        }

        StoryHeading{text: "Icons"}
        StoryNote{text: "An icon can lead the label, trail it, do both, or sit above it."}
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
            ButtonFlat{
                flow: Down
                icon_walk: Walk{width: 15. height: 15.}
                draw_icon +: {
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
                text: "Above"
            }
        }
        StoryNote{text: "The icon-only faces, one of each: standard, both gradients, flat, flatter, tonal and subtle."}
        StoryRow{
            ButtonIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
            ButtonGradientXIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
            ButtonGradientYIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
            ButtonFlatIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
            ButtonFlatterIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
            ButtonTonalIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}
            ButtonSubtleIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}
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

        StoryHeading{text: "Disabled"}
        StoryNote{text: "A disabled button takes the theme's disabled colours and does not answer a press."}
        StoryRow{
            Button{text: "Button" animator +: {disabled: {default: @on}}}
            ButtonPrimary{text: "Primary" animator +: {disabled: {default: @on}}}
            ButtonOutline{text: "Outline" animator +: {disabled: {default: @on}}}
            ButtonDanger{text: "Danger" animator +: {disabled: {default: @on}}}
            LinkLabel{text: "A link" animator +: {disabled: {default: @on}}}
        }

        StoryHeading{text: "Styling reference"}
        StoryNote{text: "Every state has its own fill and border colour, and a second stop on either turns it into a gradient. The first button sets only the second stops, the gradient pair set every colour, and the link sets its ink, its hairline and its icon."}
        StoryRow{
            Button{
                text: "Second stops"
                draw_bg +: {
                    color_2: #f00
                    color_2_hover: #f00
                    color_2_down: #f00
                    color_2_focus: #f00
                    color_2_disabled: #f00

                    border_color_2: #f00
                    border_color_2_hover: #f00
                    border_color_2_down: #f00
                    border_color_2_focus: #f00
                    border_color_2_disabled: #f00
                }
            }
            ButtonGradientX{
                draw_bg +: {
                    border_radius: 4.0

                    color: #xC00
                    color_hover: #xF0F
                    color_down: #800

                    color_2: #x0CC
                    color_2_hover: #x0FF
                    color_2_down: #088

                    border_color: #xC
                    border_color_hover: #xF
                    border_color_down: #0

                    border_color_2: #3
                    border_color_2_hover: #6
                    border_color_2_down: #8
                }
                text: "ButtonGradientX"
            }
            ButtonGradientY{
                draw_bg +: {
                    border_radius: 4.0

                    color: #xC00
                    color_hover: #xF0F
                    color_down: #800

                    color_2: #x0CC
                    color_2_hover: #x0FF
                    color_2_down: #088

                    border_color: #xC
                    border_color_hover: #xF
                    border_color_down: #0

                    border_color_2: #3
                    border_color_2_hover: #6
                    border_color_2_down: #8
                }
                text: "ButtonGradientY"
            }
        }
        StoryRow{
            ButtonGradientXIcon{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
            }
            ButtonGradientYIcon{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
            }
            ButtonFlat{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
                text: "ButtonFlat"
            }
            ButtonFlatter{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
                text: "ButtonFlatter"
            }
            LinkLabel{
                draw_text +: {
                    color: #xA
                    color_hover: #xC
                    color_down: #8
                    text_style +: {
                        font_size: 20.
                        line_spacing: 1.4
                    }
                }

                draw_bg +: {
                    color: #x0A0
                    color_hover: #x0C0
                    color_down: #080
                }

                icon_walk: Walk{
                    width: 20.
                    height: Fit
                }

                draw_icon +: {
                    color: #xA00
                    color_hover: #xC00
                    color_down: #800
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }

                text: "Styled link"
            }
        }
    }
}

fn times(n: usize) -> String {
    format!("clicked {n} time{}", if n == 1 { "" } else { "s" })
}

fn presses(n: usize) -> String {
    format!("{n} press{}", if n == 1 { "" } else { "es" })
}

fn press_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // Each counting button keeps a count of its own.
    if root.button(cx, ids!(basicbutton)).clicked(actions) {
        let n = crate::stories::bump(live_id!(button_basic));
        root.button(cx, ids!(basicbutton)).set_text(cx, &presses(n));
    }

    if root.button(cx, ids!(iconbutton)).clicked(actions) {
        let n = crate::stories::bump(live_id!(button_icon));
        root.button(cx, ids!(iconbutton)).set_text(cx, &presses(n));
    }

    if root.button(cx, ids!(subject)).clicked(actions) {
        let n = crate::stories::bump(live_id!(button_subject));
        root.label(cx, ids!(clicks)).set_text(cx, &times(n));
    }
}

fn waiting_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let waiting = root.button(cx, ids!(waiting));
    if root.button(cx, ids!(wait_toggle)).clicked(actions) {
        let loading = !waiting.loading();
        waiting.set_loading(cx, loading);
    }
    if waiting.clicked(actions) {
        let n = crate::stories::bump(live_id!(button_variants));
        root.label(cx, ids!(waiting_clicks)).set_text(cx, &times(n));
    }
    let burger = root.button(cx, ids!(burger));
    if burger.clicked(actions) {
        let open = !burger.open();
        burger.set_open(cx, open);
        root.label(cx, ids!(burger_label))
            .set_text(cx, if open { "burger: open" } else { "burger: closed" });
    }
}

/// The page's one handler: the presses, then the waiting button and the
/// burger.
fn button_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    press_actions(cx, root, actions);
    waiting_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "actions/button/overview",
    category: "Actions",
    component: "Button",
    also: &["BurgerButton", "ButtonCompound", "ButtonDanger", "ButtonDashed", "ButtonLg", "ButtonOutline", "ButtonPrimary", "ButtonSecondary", "ButtonSm", "ButtonSubtleIcon", "ButtonTertiary", "ButtonTonalIcon", "ButtonXl", "ButtonXs", "CloseButton", "CloseButtonLg", "CloseButtonSm", "CopyButton", "LinkLabel"],
    name: "Overview",
    dsl: "ButtonOverview",
    added: "2026-02-23",
    tags: &["controls", "ported"],
    doc: "# Button\n\nA button runs an action when it is pressed. It reports the press through `clicked` and keeps nothing itself, so what a press means is up to the host.\n\n## States\n\nHover, press, focus and disabled each mix the face towards colours of their own. `loading` fades the label under a turning arc and ignores presses until it is cleared, so a slow action cannot be started twice.\n\n## Faces\n\nThe semantic ladder draws its faces from the theme's role tokens: `ButtonPrimary`, `ButtonSecondary`, `ButtonTertiary` (tonal), `ButtonOutline`, `ButtonDashed` and `ButtonDanger`. A `layer_color` on the face is laid over it by hover and press at the theme's state opacities.\n\n`Button` is the standard face: a flat fill with the theme's outset bevel. `ButtonGradientX` fills it with a vertical two-stop gradient and `ButtonGradientY` runs the same fill left to right. `ButtonFlat` has no bevel, and `ButtonFlatter` shows no face or border until it is disabled.\n\n`LinkLabel` is a button drawn as text over a hairline, for an action that sits in a sentence or a list of links. It takes the same text, icon and states as a button.\n\n`ButtonCompound` draws a `description` line under the label.\n\n## Sizes and icons\n\n`ButtonXs`, `ButtonSm`, `ButtonLg` and `ButtonXl` take their heights from the control sizes. `draw_icon` puts an icon before the label and `draw_icon_end` puts a second one after it. `ButtonIcon`, `ButtonFlatIcon`, `ButtonFlatterIcon`, `ButtonTonalIcon` and `ButtonSubtleIcon` carry an icon alone.\n\n## Close, copy and burger\n\n`CloseButton` is a standalone cross in three sizes. `CopyButton` copies `text_to_copy` to the clipboard, shows a check and `Copied` for a moment, and raises `CopyButtonAction::Copied`. `BurgerButton` morphs three bars into a cross through `open`.\n\n## Styling\n\nEach state has its own fill and border colour: `color`, `color_hover`, `color_down`, `color_focus` and `color_disabled`, and the same five for `border_color`. A `color_2` stop turns the fill into a gradient and `border_color_2` does the same for the border.\n\nThe first four controls drive the button at the top of the page; the last three drive the waiting button.",
    subject: "",
    feature: None,
    controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Button" } },
            Control { label: "Corner radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 16., step: 0.5, default: 2.5 } },
            Control { label: "Fill", target: "subject", kind: ControlKind::Color { prop: "draw_bg.color", default: 0xFFFFFF88 } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
            Control { label: "Waiting label", target: "waiting", kind: ControlKind::Text { prop: "text", default: "Save changes" } },
            Control { label: "Loading", target: "waiting", kind: ControlKind::Bool { prop: "loading", default: true } },
            Control { label: "Waiting disabled", target: "waiting", kind: ControlKind::Disabled { default: false } },
        ],
    on_actions: Some(button_actions),
}];
