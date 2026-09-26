//! The view story: a View on its own, the presets that give it a
//! background to paint, and the two that draw their children once.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    /** One preset with its name under it. */
    let Tile = View{
        width: Fit height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5}
    }

    let Caption = Label{
        draw_text +: {color: theme.color_text_meta}
    }

    /** A row of tiles that wraps when the page is narrow. */
    let Tiles = View{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        spacing: theme.space_3
    }

    /** Something for a View to lay out, painted so the layout shows. */
    let Block = SolidView{
        width: 56. height: 36.
        draw_bg +: {color: theme.color_primary_container}
    }

    mod.stories.ViewOverview = StoryPage{
        StoryNote{text: "A View lays out its children and draws nothing of its own. Its presets are the same container with a background that paints: a flat colour, a shape, a gradient or a shadow. Two more draw their children into a texture once and show that until something inside changes."}

        StoryHeading{text: "A View on its own"}
        StoryNote{text: "Three blocks placed by a View with a flow, a spacing and a padding. The View paints nothing, so the page shows through the gaps and around the padding."}
        subject := View{
            width: Fit height: Fit
            flow: Right
            spacing: theme.space_2
            padding: theme.mspace_3
            Block{}
            Block{draw_bg +: {color: theme.color_secondary_container}}
            Block{draw_bg +: {color: theme.color_tertiary_container}}
        }

        StoryHeading{text: "Shapes"}
        StoryNote{text: "Every tile is the same size in the same colour, so what differs is the shape the background draws. SolidView fills its rect, and RectView fills it and can stroke a border. RoundedView rounds all four corners by one radius. RoundedXView takes one radius for the left corners and one for the right, RoundedYView one for the top and one for the bottom, and RoundedAllView one per corner: the shapes a tab, a sheet or a panel joined to an edge needs. CircleView and HexagonView draw the largest circle or hexagon the rect holds."}
        Tiles{
            Tile{SolidView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container}} Caption{text: "SolidView"}}
            Tile{RectView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container border_size: 1. border_color: theme.color_outline}} Caption{text: "RectView"}}
            Tile{RoundedView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container border_radius: 8.}} Caption{text: "RoundedView"}}
            Tile{RoundedXView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container border_radius: vec2(12. 0.)}} Caption{text: "RoundedXView"}}
            Tile{RoundedYView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container border_radius: vec2(12. 0.)}} Caption{text: "RoundedYView"}}
            Tile{RoundedAllView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container border_radius: vec4(12. 0. 12. 0.)}} Caption{text: "RoundedAllView"}}
            Tile{CircleView{width: 56. height: 56. draw_bg +: {color: theme.color_primary_container}} Caption{text: "CircleView"}}
            Tile{HexagonView{width: 56. height: 56. draw_bg +: {color: theme.color_primary_container}} Caption{text: "HexagonView"}}
        }
        StoryNote{text: "GradientXView blends color into color_2 from left to right and GradientYView from top to bottom; without a color_2 they paint one flat colour. The drawn shapes above take the same pair, with gradient_fill_horizontal choosing the direction."}
        Tiles{
            Tile{GradientXView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container color_2: theme.color_tertiary_container}} Caption{text: "GradientXView"}}
            Tile{GradientYView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container color_2: theme.color_tertiary_container}} Caption{text: "GradientYView"}}
            Tile{RoundedView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container color_2: theme.color_tertiary_container gradient_fill_horizontal: 1. border_radius: 8.}} Caption{text: "RoundedView, across"}}
        }

        StoryHeading{text: "Shadows"}
        StoryNote{text: "RectShadowView and RoundedShadowView cast a shadow of their own, set by shadow_color, shadow_radius and shadow_offset, and draw past their rect to do it, so they leave their clip off. The theme's five steps of elevation are on Foundations > Elevation."}
        Tiles{
            padding: theme.mspace_3
            spacing: theme.space_4
            Tile{RectShadowView{width: 96. height: 56. draw_bg +: {color: theme.color_surface_container_high}} Caption{text: "RectShadowView"}}
            Tile{RoundedShadowView{width: 96. height: 56. draw_bg +: {color: theme.color_surface_container_high border_radius: 8.}} Caption{text: "RoundedShadowView"}}
        }

        StoryHeading{text: "Drawn once"}
        StoryNote{text: "CachedView draws its children into a texture and shows the texture until something inside redraws. CachedRoundedView does the same and clips the texture to a rounded box, so it rounds what its children draw, over any ground. Both pay for a render target the size of their rect: cheap for content that sits still, and extra work for content that redraws every frame."}
        Tiles{
            Tile{
                CachedView{
                    width: Fit height: Fit
                    SolidView{
                        width: Fit height: Fit
                        padding: theme.mspace_2
                        draw_bg +: {color: theme.color_surface_container_high}
                        Label{text: "drawn once"}
                    }
                }
                Caption{text: "CachedView"}
            }
            Tile{
                CachedRoundedView{
                    width: 140. height: 56.
                    draw_bg +: {border_radius: 10.}
                    GradientXView{
                        width: Fill height: Fill
                        align: Align{x: 0.5 y: 0.5}
                        draw_bg +: {color: theme.color_primary_container color_2: theme.color_tertiary_container}
                        Label{text: "clipped round"}
                    }
                }
                Caption{text: "CachedRoundedView"}
            }
        }

        StoryHeading{text: "A circle takes the room it is given"}
        StoryNote{text: "A CircleView sized by Fit is only as large as its content, so the text runs past the curve. Padding or a fixed size gives the circle room around its content, and a border_radius above zero sets the circle's radius outright instead of taking the largest that fits."}
        StoryRow{
            spacing: theme.space_3
            align: Align{x: 0. y: 1.}
            Tile{
                CircleView{width: Fit height: Fit align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_secondary_container border_size: 1. border_color: theme.color_outline} Label{text: "Fit"}}
                Caption{text: "Fit"}
            }
            Tile{
                CircleView{width: Fit height: Fit padding: 24. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_secondary_container border_size: 1. border_color: theme.color_outline} Label{text: "Fit"}}
                Caption{text: "Fit, padding 24"}
            }
            Tile{
                CircleView{width: 64. height: 64. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_secondary_container border_size: 1. border_color: theme.color_outline} Label{text: "64"}}
                Caption{text: "64 by 64"}
            }
            Tile{
                CircleView{width: 64. height: 64. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_secondary_container border_size: 1. border_color: theme.color_outline border_radius: 20.} Label{text: "20"}}
                Caption{text: "border_radius 20"}
            }
        }

        StoryHeading{text: "A background with nothing to paint"}
        StoryNote{text: "show_bg: true on a plain View switches on a background whose pixel is transparent and never reads color, so the box on the left paints nothing. The box on the right is a SolidView given the same colour. For a flat colour reach for a preset, or give the background a pixel function of its own."}
        Tiles{
            Tile{View{width: 96. height: 56. show_bg: true draw_bg +: {color: theme.color_primary_container}} Caption{text: "View, show_bg"}}
            Tile{SolidView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container}} Caption{text: "SolidView"}}
        }

        StoryHeading{text: "Styling reference"}
        StoryNote{text: "The drawn shapes share one set of inputs on draw_bg. border_radius is half the radius you see, because the box the shader draws doubles it. border_inset pulls the shape in from the left, top, right and bottom edges of the rect."}
        Tiles{
            padding: Inset{bottom: 12.}
            Tile{RoundedView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container border_radius: 6. border_size: 2. border_color: theme.color_primary}} Caption{text: "border_size, border_color"}}
            Tile{RoundedView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container border_radius: 6. border_size: 3. border_color: theme.color_primary border_color_2: theme.color_tertiary gradient_border_horizontal: 1.}} Caption{text: "border_color_2"}}
            Tile{RoundedView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container border_radius: 6. border_inset: vec4(12. 8. 12. 8.)}} Caption{text: "border_inset"}}
            Tile{RoundedShadowView{width: 96. height: 56. draw_bg +: {color: theme.color_surface_container_high border_radius: 6. shadow_radius: 6. shadow_offset: vec2(6. 6.)}} Caption{text: "shadow_offset"}}
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/view/overview",
    category: "Containers",
    component: "View",
    also: &[
        "SolidView", "RectView", "RoundedView", "RoundedXView", "RoundedYView", "RoundedAllView",
        "CircleView", "HexagonView", "GradientXView", "GradientYView",
        "RectShadowView", "RoundedShadowView", "CachedView", "CachedRoundedView",
    ],
    name: "Overview",
    dsl: "ViewOverview",
    added: "2026-02-25",
    tags: &["ported", "layout"],
    doc: "# View

A `View` lays out its children and draws nothing of its own. `flow`, `spacing`, `padding` and `align` place the children, `width` and `height` size the view in its parent, and `clip_x` and `clip_y` cut off what spills past its rect. `scroll_bars` makes it scroll, which Layout > Scrolling shows. `show_bg: true` switches a background on, and a plain View's background is transparent: the presets are Views whose background paints something.

## Which one rounds the corners

| You want | Use |
|---|---|
| a rounded background, with children that stay inside it | `RoundedView`, or its X, Y and All variants |
| the children themselves cut to rounded corners, over any ground | `CachedRoundedView` |
| rounded corners on content that redraws every frame, over one flat colour | `CornerCapView`, on View > Corner caps |

`RoundedView` rounds only its own background: a child that fills it, such as a picture, still shows square corners. `CachedRoundedView` draws the children into a texture and clips that, which is right over anything and costs a render target. `CornerCapView` paints the ground colour over the corners, which costs little and is right only where the ground is that one colour.

## Presets

| Preset | Its background |
|---|---|
| `SolidView` | fills the rect with `color` |
| `RectView` | fills the rect and can stroke a border |
| `RoundedView` | rounds all four corners by `border_radius` |
| `RoundedXView` | `border_radius` is a pair: the left corners, then the right |
| `RoundedYView` | a pair: the top corners, then the bottom |
| `RoundedAllView` | four radii: top left, top right, bottom right, bottom left |
| `CircleView` | the largest circle the rect holds, or one of `border_radius` when that is above zero |
| `HexagonView` | the largest hexagon the rect holds |
| `GradientXView`, `GradientYView` | blend `color` into `color_2`, across or down |
| `RectShadowView`, `RoundedShadowView` | the square or rounded shape, with a shadow cast past the rect |
| `CachedView` | the children, drawn once into a texture |
| `CachedRoundedView` | that texture, clipped to a rounded box |

A shadow view carries its own `shadow_color`, `shadow_radius` and `shadow_offset`, and turns its clip off so the shadow can leave the rect. The theme's steps of elevation, `ElevatedView1` to `ElevatedView5`, set those from tokens, on Foundations > Elevation.

## Drawn once

`CachedView` and `CachedRoundedView` draw their children into a render target the size of their rect and show it until something inside redraws. For content that sits still that is cheaper than drawing it again. For content that changes every frame the target is rebuilt every frame, and the cache only adds work.

## A background with nothing to paint

`show_bg: true` on a plain `View` gives it a background whose pixel is transparent and does not read `color`, so a colour written there paints nothing. Use `SolidView` for a flat colour, or give the background a `pixel` function of its own.

## Styling reference

The drawn shapes share these inputs on `draw_bg`.

| Input | What it does |
|---|---|
| `color` | the fill |
| `color_2` | a second fill colour; when set, the fill blends into it |
| `gradient_fill_horizontal` | 1 blends across, 0 blends down |
| `border_size`, `border_color` | the stroke around the shape; none at 0 |
| `border_color_2`, `gradient_border_horizontal` | a stroke that blends, and its direction |
| `border_radius` | half the radius you see; a pair or four values on the X, Y and All variants |
| `border_inset` | pulls the shape in from the left, top, right and bottom edges |
| `color_dither` | grain that keeps a gradient from banding; 0 turns it off |
| `shadow_color`, `shadow_radius`, `shadow_offset` | on the shadow views: the shadow's colour, blur and offset |",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: None,
}];
