//! The button stories: every rung of the button ladder, an icon button and a
//! click counter, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;
use std::sync::atomic::{AtomicUsize, Ordering};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ButtonOverview = StoryPage{
        H4{text: "Standard"}
        StoryRow{
            Button{}
            Button{
                draw_bg +: {
                    color_2: uniform(#f00)
                    color_2_hover: uniform(#f00)
                    color_2_down: uniform(#f00)
                    color_2_focus: uniform(#f00)
                    color_2_disabled: uniform(#f00)

                    border_color_2: uniform(#f00)
                    border_color_2_hover: uniform(#f00)
                    border_color_2_down: uniform(#f00)
                    border_color_2_focus: uniform(#f00)
                    border_color_2_disabled: uniform(#f00)
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
                    border_radius: uniform(4.0)

                    color: uniform(#xC00)
                    color_hover: uniform(#xF0F)
                    color_down: uniform(#800)

                    color_2: uniform(#x0CC)
                    color_2_hover: uniform(#x0FF)
                    color_2_down: uniform(#088)

                    border_color: uniform(#xC)
                    border_color_hover: uniform(#xF)
                    border_color_down: uniform(#0)

                    border_color_2: uniform(#3)
                    border_color_2_hover: uniform(#6)
                    border_color_2_down: uniform(#8)
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
                    border_radius: uniform(4.0)

                    color: uniform(#xC00)
                    color_hover: uniform(#xF0F)
                    color_down: uniform(#800)

                    color_2: uniform(#x0CC)
                    color_2_hover: uniform(#x0FF)
                    color_2_down: uniform(#088)

                    border_color: uniform(#xC)
                    border_color_hover: uniform(#xF)
                    border_color_down: uniform(#0)

                    border_color_2: uniform(#3)
                    border_color_2_hover: uniform(#6)
                    border_color_2_down: uniform(#8)
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
}

pub const STORIES: &[Story] = &[Story {
    key: "actions/button/overview",
    category: "Actions",
    component: "Button",
    also: &[],
    name: "Overview",
    dsl: "ButtonOverview",
    added: "2026-02-23",
    tags: &["ported"],
    doc: "# Button\n\nButtons trigger actions when clicked.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(overview_actions),
}];
