//! The scrolling overview: a bounded box that scrolls on both axes with
//! draggable bars, and aligned boxes whose content overflows and still
//! scrolls, in every flow.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // Plain values: a `uniform(..)` inside a merge redeclares the input
    // instead of setting it, and the boxes drew as nothing but their labels.
    let AlignScrollBox = RoundedView{
        show_bg: true
        draw_bg +: {
            color: theme.color_surface_container_high
            border_size: 1.
            border_radius: 0.
            border_color: theme.color_outline_variant
        }
        padding: 3.
        align: Align{x: 0.5 y: 0.5}
    }

    let ScrollContainer = SolidView{
        draw_bg +: {
            color: theme.color_surface_container_low
        }
    }

    /** One aligned box and its caption. */
    let CaseTile = View{
        width: 300.
        height: Fit
        flow: Down
        spacing: theme.space_1
    }

    let CaseCaption = Label{
        width: Fill
        draw_text +: {color: theme.color_text_meta}
    }

    let Corners = View{
        width: Fill
        height: Fit
        flow: Right
    }

    mod.stories.AlignScrollOverview = StoryPage{
        StoryNote{text: "A view scrolls when it has scroll bars and its content is larger than the view. A bar is drawn only on an axis whose content overflows, and the content can be moved with the wheel, by dragging a handle, or by dragging the content itself."}

        StoryHeading{text: "A box that scrolls"}
        StoryNote{text: "A 1600 point square of gradient in a box 240 points tall, with a bar on each axis. Scroll to each corner: the labels say where you are."}
        subject := ScrollXYView{
            width: Fill
            height: 240.
            GradientYView{
                width: 1600.
                height: 1600.
                flow: Down
                padding: theme.mspace_3
                draw_bg +: {
                    color: theme.color_tertiary_container
                    color_2: theme.color_primary_container
                }
                Corners{
                    Label{text: "top left"}
                    View{width: Fill height: 1.}
                    Label{text: "top right"}
                }
                View{width: Fill height: Fill}
                Corners{
                    Label{text: "bottom left"}
                    View{width: Fill height: 1.}
                    Label{text: "bottom right"}
                }
            }
        }

        StoryHeading{text: "One axis or both, and cached"}
        StoryNote{text: "ScrollXView scrolls across, ScrollYView down and ScrollXYView both ways. CachedScrollX, CachedScrollY and CachedScrollXY scroll the same way and draw their content into a texture, which they show again while the view sits still and nothing inside it changes; a scroll draws the content afresh. Each box is too small for what it holds on the axis it scrolls."}
        View{
            width: Fill
            height: Fit
            flow: Flow.Right{wrap: true}
            spacing: theme.space_3

            CaseTile{
                width: 200.
                CaseCaption{text: "ScrollXView"}
                ScrollContainer{
                    width: Fill height: 64.
                    ScrollXView{
                        width: Fill height: Fill
                        flow: Right
                        spacing: theme.space_2
                        padding: theme.mspace_1
                        AlignScrollBox{width: 90. height: 40. P{width: Fit text: "one"}}
                        AlignScrollBox{width: 90. height: 40. P{width: Fit text: "two"}}
                        AlignScrollBox{width: 90. height: 40. P{width: Fit text: "three"}}
                    }
                }
            }

            CaseTile{
                width: 200.
                CaseCaption{text: "ScrollYView"}
                ScrollContainer{
                    width: Fill height: 64.
                    ScrollYView{
                        width: Fill height: Fill
                        flow: Down
                        spacing: theme.space_1
                        padding: theme.mspace_1
                        P{text: "one"} P{text: "two"} P{text: "three"}
                        P{text: "four"} P{text: "five"} P{text: "six"}
                    }
                }
            }

            CaseTile{
                width: 200.
                CaseCaption{text: "CachedScrollX"}
                ScrollContainer{
                    width: Fill height: 64.
                    CachedScrollX{
                        width: Fill height: Fill
                        flow: Right
                        spacing: theme.space_2
                        padding: theme.mspace_1
                        AlignScrollBox{width: 90. height: 40. P{width: Fit text: "one"}}
                        AlignScrollBox{width: 90. height: 40. P{width: Fit text: "two"}}
                        AlignScrollBox{width: 90. height: 40. P{width: Fit text: "three"}}
                    }
                }
            }

            CaseTile{
                width: 200.
                CaseCaption{text: "CachedScrollY"}
                ScrollContainer{
                    width: Fill height: 64.
                    CachedScrollY{
                        width: Fill height: Fill
                        flow: Down
                        spacing: theme.space_1
                        padding: theme.mspace_1
                        P{text: "one"} P{text: "two"} P{text: "three"}
                        P{text: "four"} P{text: "five"} P{text: "six"}
                    }
                }
            }

            CaseTile{
                width: 200.
                CaseCaption{text: "CachedScrollXY"}
                ScrollContainer{
                    width: Fill height: 64.
                    CachedScrollXY{
                        width: Fill height: Fill
                        flow: Right
                        spacing: theme.space_2
                        padding: theme.mspace_1
                        AlignScrollBox{width: 140. height: 80. P{width: Fit text: "one"}}
                        AlignScrollBox{width: 140. height: 80. P{width: Fit text: "two"}}
                    }
                }
            }
        }

        StoryHeading{text: "Centred content still scrolls"}
        StoryNote{text: "Each box centres its content, and the content is larger than the box on the axis it scrolls. Alignment places content that fits; on an axis the content overflows, the content starts at the box's edge, so the first part and the last are both within reach."}
        View{
            width: Fill
            height: Fit
            flow: Flow.Right{wrap: true}
            spacing: theme.space_3
            align: Align{x: 0. y: 0.}

            CaseTile{
                CaseCaption{text: "flow: Down, align: {y: 0.5}, scrolls down"}
                ScrollContainer{
                    width: Fill height: 180.
                    flow: Down
                    align: Align{x: 0.5 y: 0.5}
                    scroll_bars: ScrollBars{
                        show_scroll_x: false show_scroll_y: true
                        scroll_bar_y.drag_scrolling: true
                    }
                    AlignScrollBox{width: 120. height: 60. P{width: Fit text: "Box 1"}}
                    AlignScrollBox{width: 120. height: 60. P{width: Fit text: "Box 2"}}
                    AlignScrollBox{width: 120. height: 60. P{width: Fit text: "Box 3"}}
                    AlignScrollBox{width: 120. height: 60. P{width: Fit text: "Box 4"}}
                    AlignScrollBox{width: 120. height: 60. P{width: Fit text: "Box 5"}}
                }
            }

            CaseTile{
                CaseCaption{text: "flow: Down, align: {x: 0.5}, scrolls across"}
                ScrollContainer{
                    width: Fill height: Fit
                    flow: Down
                    align: Align{x: 0.5 y: 0.0}
                    scroll_bars: ScrollBars{
                        show_scroll_x: true show_scroll_y: false
                        scroll_bar_x.drag_scrolling: true
                    }
                    AlignScrollBox{width: 400. height: 40. P{width: Fit text: "400 points wide in a 300 point box"}}
                    AlignScrollBox{width: 150. height: 40. P{width: Fit text: "150 wide"}}
                    AlignScrollBox{width: 400. height: 40. P{width: Fit text: "400 wide again"}}
                }
            }

            CaseTile{
                CaseCaption{text: "flow: Right, align: {y: 0.5}, scrolls down"}
                ScrollContainer{
                    width: Fill height: 150.
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    scroll_bars: ScrollBars{
                        show_scroll_x: false show_scroll_y: true
                        scroll_bar_y.drag_scrolling: true
                    }
                    AlignScrollBox{width: 70. height: 50. P{width: Fit text: "Short"}}
                    AlignScrollBox{width: 70. height: 250. P{width: Fit text: "Tall, 250"}}
                    AlignScrollBox{width: 70. height: 50. P{width: Fit text: "Short"}}
                }
            }

            CaseTile{
                CaseCaption{text: "flow: Right, align: {x: 0.5}, scrolls across"}
                ScrollContainer{
                    width: Fill height: Fit
                    flow: Right
                    align: Align{x: 0.5 y: 0.5}
                    scroll_bars: ScrollBars{
                        show_scroll_x: true show_scroll_y: false
                        scroll_bar_x.drag_scrolling: true
                    }
                    AlignScrollBox{width: 100. height: 60. P{width: Fit text: "A"}}
                    AlignScrollBox{width: 100. height: 60. P{width: Fit text: "B"}}
                    AlignScrollBox{width: 100. height: 60. P{width: Fit text: "C"}}
                    AlignScrollBox{width: 100. height: 60. P{width: Fit text: "D"}}
                    AlignScrollBox{width: 100. height: 60. P{width: Fit text: "E"}}
                }
            }

            CaseTile{
                CaseCaption{text: "flow: Overlay, centred, scrolls both ways"}
                ScrollContainer{
                    width: Fill height: 150.
                    flow: Overlay
                    align: Align{x: 0.5 y: 0.5}
                    scroll_bars: ScrollBars{
                        show_scroll_x: true show_scroll_y: true
                        scroll_bar_x.drag_scrolling: true
                        scroll_bar_y.drag_scrolling: true
                    }
                    AlignScrollBox{
                        width: 400. height: 300.
                        P{width: Fit text: "400 by 300 in a 300 by 150 box"}
                    }
                }
            }
        }

        StoryHeading{text: "Content that fits"}
        StoryNote{text: "The same kind of box with room to spare. Nothing overflows, so no bar is drawn and the content sits exactly where the alignment puts it."}
        CaseTile{
            CaseCaption{text: "flow: Down, align: {x: 0.5, y: 0.5}"}
            ScrollContainer{
                width: Fill height: 150.
                flow: Down
                align: Align{x: 0.5 y: 0.5}
                scroll_bars: ScrollBars{
                    show_scroll_x: false show_scroll_y: true
                    scroll_bar_y.drag_scrolling: true
                }
                AlignScrollBox{width: 120. height: 40. P{width: Fit text: "Centred"}}
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "layout/scrolling/overview",
    category: "Layout",
    component: "Scrolling",
    also: &[
        "ScrollBar", "ScrollBars", "ScrollXYView", "ScrollXView", "ScrollYView",
        "CachedScrollX", "CachedScrollY", "CachedScrollXY", "GradientYView",
    ],
    name: "Overview",
    dsl: "AlignScrollOverview",
    added: "2026-04-16",
    tags: &["ported"],
    doc: "# Scrolling

A view scrolls when it has `scroll_bars` and its content is larger than the view on that axis. `ScrollXView`, `ScrollYView` and `ScrollXYView` are views with the bars already set for one axis or both; any view takes `scroll_bars: ScrollBars{...}` to the same effect. `CachedScrollX`, `CachedScrollY` and `CachedScrollXY` are the same three on a `CachedView`: the content is drawn into a texture and shown again while the view sits still and nothing inside it changes, which suits a long run of rows that are costly to draw. A scroll draws the content afresh, and content that redraws every frame gains nothing from the texture.

A bar is drawn only on an axis whose content overflows, along the far edge of the view and over the content. The handle's length is the share of the content in view, down to `min_handle_size`. The content moves with the wheel, with a drag on the handle, and, when `drag_scrolling` is on, with a drag on the content itself, the way a finger moves it.

## Alignment and overflow

`align` places content that fits. On an axis where the content is larger than the view, the alignment offset is held at zero, so the content starts at the view's edge and scrolling reaches all of it. A centred child larger than its box would otherwise begin above the top, where no scroll position can reach it. Content that fits is aligned as it would be without the bars.

## ScrollBars

| Property | What it does |
|---|---|
| `show_scroll_x`, `show_scroll_y` | which axes may scroll |
| `scroll_bar_x`, `scroll_bar_y` | the bar on each axis, a `ScrollBar` |

## ScrollBar

| Property | Default | What it does |
|---|---|---|
| `bar_size` | 10 | the thickness of the bar's lane |
| `bar_side_margin` | 3 | the gap between the handle and the ends of the lane |
| `min_handle_size` | 30 | the shortest the handle gets, however long the content |
| `drag_scrolling` | off | a press on the content can drag it |
| `bounce_at_start`, `bounce_at_end` | on | stretch past an edge and spring back |
| `draw_bg.size` | 6 | the drawn thickness of the handle |
| `draw_bg.color`, `color_hover`, `color_drag` | theme | the handle at rest, under the pointer, and held |",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: None,
}];
