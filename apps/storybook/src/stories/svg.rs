//! The svg story: a vector drawing as a widget, and the frame loop it runs
//! unless you say otherwise.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SvgOverview = StoryPage{
        StoryNote{text: "A vector drawing sized like any other widget. Seven places in this repository use one. It is not the Icon: an Icon wraps the same drawing in a background and its own icon_walk so it can sit inside a control, and this is the drawing on its own."}

        StoryHeading{text: "It takes the room you give it"}
        StoryNote{text: "Fit by default, so it takes the drawing's own size. Given a width and a height it scales into them."}
        StoryRow{
            View{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                subject := Svg{
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            View{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 64. height: 64.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            View{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 96. height: 32.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
        }

        StoryHeading{text: "The document's colours, unless you say otherwise"}
        StoryNote{text: "draw_svg.color carries a sentinel meaning leave the drawing alone, so by default you get the colours it was authored with. Give it a colour and that colour replaces them, keeping the per-vertex alpha — which is how the same file serves as a picture in one place and a tinted mark in another."}
        StoryRow{
            View{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 48. height: 48.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            View{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                tinted := Svg{
                    width: 48. height: 48.
                    animating: false
                    draw_svg +: {
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                        color: theme.color_text_meta
                    }
                }
            }
            Label{text: "as authored, then tinted" draw_text +: {color: theme.color_text_meta}}
        }

        StoryHeading{text: "animating is on by default, and it is not free"}
        StoryNote{text: "An animating Svg asks for the next frame, every frame, for as long as it exists — that is how a drawing with time in it moves. A drawing with no time in it does exactly the same thing and shows exactly the same picture, so the only way to tell is a machine that never idles. The one caller in this repository that thought about it writes animating: false. Both of these look identical; the left one is spinning the frame loop."}
        StoryRow{
            View{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 48. height: 48.
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            View{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 48. height: 48.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            Label{text: "left: animating (default). right: animating: false." draw_text +: {color: theme.color_text_meta}}
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/svg/overview",
    category: "Data display",
    component: "Svg",
    also: &[],
    name: "Overview",
    dsl: "SvgOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Svg

A vector document drawn as a widget. Seven places in this repository use one.

It is not `Icon`, but the difference is not tinting — both draw through the same `DrawSvg`. An `Icon` wraps that drawing in a background quad and gives it an `icon_walk` and a `size`, so it can sit inside a button and be measured like a glyph. `Svg` is the drawing on its own, taking the widget's own walk.

`draw_svg.svg` takes the resource. The widget is `Fit` by default, so it takes the drawing's own size; give it a width and a height and it scales into them, ignoring the drawing's aspect if you ask it to.

`draw_svg.color` holds a sentinel that means *leave the drawing alone*, so the default is the colours the file was authored with. Setting a colour replaces them while keeping the per-vertex alpha, which is how one file serves as a picture in one place and a tinted mark in another.

**`animating` is `true` by default and it costs a frame loop.** An animating `Svg` asks for the next frame on every frame, for as long as it exists, which is what makes a drawing with time in it move. A drawing with no time in it does the same thing and shows the same still picture, so nothing on screen tells you it is happening — the only symptom is a process that never goes idle. Of the callers in this repository, one sets `animating: false` deliberately; the rest take the default. Set it false unless the drawing actually moves.",
    subject: "subject",
    feature: None,
    controls: &[Control {
        label: "animating",
        target: "",
        kind: ControlKind::Bool { prop: "animating", default: false },
    }],
    on_actions: None,
}];
