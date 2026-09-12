//! The button stories: every rung of the button ladder, an icon button and a
//! click counter, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::sync::atomic::{AtomicUsize, Ordering};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ButtonOverview = StoryPage{        StoryHeading{text: "One button, under the controls"}
        StoryNote{text: "One button. Every control on the right writes into it."}
        StoryRow{
            subject := Button{text: "Button"}
        }
        StoryRow{
            clicks := Label{text: "not clicked yet"}
        }

        H4{text: "Standard"}
        StoryRow{
            Button{}
            Button{
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

            basicbutton := Button{}

            iconbutton := Button{
                draw_icon +: {
                    gradient_fill_horizontal: instance(1.0)
                    color: #f00
                    color_2: #00f
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
                text: "Button"
            }
        }

        Hr{}
        H4{text: "Standard, disabled"}
        StoryRow{
            Button{
                text: "Button"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
        }

        Hr{}
        H4{text: "ButtonIcon"}
        StoryRow{
            ButtonIcon{
                draw_icon +: {
                    gradient_fill_horizontal: instance(1.0)
                    color: #f00
                    color_2: #00f
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
            }
        }

        Hr{}
        H4{text: "GradientX"}
        StoryRow{
            ButtonGradientX{text: "ButtonGradientX"}
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
        }

        Hr{}
        H4{text: "ButtonGradientXIcon"}
        StoryRow{
            ButtonGradientXIcon{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
            }
        }

        Hr{}
        H4{text: "GradientY"}
        StoryRow{
            ButtonGradientY{text: "ButtonGradientY"}
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

        Hr{}
        H4{text: "ButtonGradientYIcon"}
        StoryRow{
            ButtonGradientYIcon{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
            }
        }

        Hr{}
        H4{text: "Flat"}
        StoryRow{
            ButtonFlat{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
                text: "ButtonFlat"
            }

            ButtonFlat{
                flow: Down
                icon_walk: Walk{width: 15.}
                draw_icon +: {
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
                text: "ButtonFlat"
            }
        }

        Hr{}
        H4{text: "ButtonFlatIcon"}
        StoryRow{
            ButtonFlatIcon{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
            }
        }

        Hr{}
        H4{text: "Flatter"}
        StoryRow{
            ButtonFlatter{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
                text: "ButtonFlatter"
            }
        }

        Hr{}
        H4{text: "ButtonFlatterIcon"}
        StoryRow{
            ButtonFlatterIcon{
                draw_icon +: {
                    color: #f00
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }
            }
        }
    }
}

/// Clicks on the two counting buttons, shared the way the zoo's one counter was.
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(basicbutton)).clicked(actions) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        log!("BASIC BUTTON CLICKED {}", n);
        root.button(cx, ids!(basicbutton))
            .set_text(cx, &format!("Clicky clicky! {}", n + 1));
    }

    if root.button(cx, ids!(iconbutton)).clicked(actions) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        log!("ICON BUTTON CLICKED {}", n);
        root.button(cx, ids!(iconbutton))
            .set_text(cx, &format!("Icon button clicked: {}", n + 1));
    }
    subject_actions(cx, root, actions);
}

fn subject_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(subject)).clicked(actions) {
        let n = crate::stories::bump(live_id!(button_subject));
        root.label(cx, ids!(clicks)).set_text(cx, &format!("clicked {n} time{}", if n == 1 { "" } else { "s" }));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "actions/button/overview",
    category: "Actions",
    component: "Button",
    also: &[],
    name: "Overview",
    dsl: "ButtonOverview",
    added: "2026-02-23",
    tags: &["controls", "ported"],
    doc: "# Button\n\nButtons trigger actions when clicked.",
    subject: "",
    feature: None,
    controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Button" } },
            Control { label: "Corner radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 16., step: 0.5, default: 2.5 } },
            Control { label: "Fill", target: "subject", kind: ControlKind::Color { prop: "draw_bg.color", default: 0xFFFFFF88 } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
    on_actions: Some(overview_actions),
}];
