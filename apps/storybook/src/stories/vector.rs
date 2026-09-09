//! The vector story: shapes written down rather than loaded, and the pieces
//! that go inside one.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Tile = View{
        width: Fit height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5}
        padding: theme.mspace_2
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_low}
        caption := Label{text: "" draw_text +: {color: theme.color_text_meta}}
    }

    let sweep = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #x4fc3f7}
        Stop{offset: 1 color: #xab47bc}
    }
    let bloom = RadGradient{cx: 0.5 cy: 0.5 r: 0.5
        Stop{offset: 0 color: #xffca28 opacity: 1.0}
        Stop{offset: 1 color: #xef5350 opacity: 0.15}
    }
    let lift = Filter{
        DropShadow{dx: 0 dy: 3 blur: 5 color: #x000000 opacity: 0.55}
    }

    mod.stories.VectorOverview = StoryPage{
        StoryNote{text: "A drawing written in the DSL rather than loaded from a file. Where Svg reads a document, this IS the document: shapes, gradients, transforms and filters declared as children, redrawn from the same tree the rest of the page is built from."}

        StoryHeading{text: "The primitives"}
        StoryNote{text: "Each takes a viewbox and draws inside it. fill takes a colour, a gradient or false; stroke and stroke_width draw the outline; a Path takes the same d string an SVG would."}
        StoryRow{
            Tile{
                caption: Label{text: "Rect"}
                Vector{width: 64 height: 64 viewbox: vec4(0 0 24 24)
                    Rect{x: 3 y: 5 w: 18 h: 14 rx: 3 fill: #x4fc3f7}
                }
            }
            Tile{
                caption: Label{text: "Circle"}
                Vector{width: 64 height: 64 viewbox: vec4(0 0 24 24)
                    Circle{cx: 12 cy: 12 r: 9 fill: #x66bb6a}
                }
            }
            Tile{
                caption: Label{text: "Ellipse"}
                Vector{width: 64 height: 64 viewbox: vec4(0 0 24 24)
                    Ellipse{cx: 12 cy: 12 rx: 10 ry: 6 fill: #xffca28}
                }
            }
            Tile{
                caption: Label{text: "Line"}
                Vector{width: 64 height: 64 viewbox: vec4(0 0 24 24)
                    Line{x1: 3 y1: 20 x2: 21 y2: 4 stroke: #xef5350 stroke_width: 2.5 stroke_linecap: "round"}
                }
            }
        }
        StoryRow{
            Tile{
                caption: Label{text: "Polyline"}
                Vector{width: 64 height: 64 viewbox: vec4(0 0 24 24)
                    Polyline{pts: [2 18 8 8 13 14 22 4] fill: false stroke: #x4fc3f7 stroke_width: 2.0 stroke_linejoin: "round" stroke_linecap: "round"}
                }
            }
            Tile{
                caption: Label{text: "Polygon"}
                Vector{width: 64 height: 64 viewbox: vec4(0 0 24 24)
                    Polygon{pts: [12 2 22 20 2 20] fill: #xab47bc}
                }
            }
            Tile{
                caption: Label{text: "Path"}
                Vector{width: 64 height: 64 viewbox: vec4(0 0 24 24)
                    Path{d: "M20 6L9 17L4 12" fill: false stroke: #x26a69a stroke_width: 2.5 stroke_linecap: "round" stroke_linejoin: "round"}
                }
            }
            Tile{
                caption: Label{text: "stroke, no fill"}
                Vector{width: 64 height: 64 viewbox: vec4(0 0 24 24)
                    Rect{x: 3 y: 3 w: 18 h: 18 rx: 4 fill: false stroke: #x90a4ae stroke_width: 1.5}
                }
            }
        }

        StoryHeading{text: "Gradients and a shadow"}
        StoryNote{text: "A Gradient or a RadGradient is a named thing with Stop children; a shape names it as its fill. A Filter holding a DropShadow is named the same way and applies to a shape or a whole Group."}
        StoryRow{
            Tile{
                caption: Label{text: "Gradient"}
                Vector{width: 80 height: 80 viewbox: vec4(0 0 24 24)
                    Rect{x: 2 y: 2 w: 20 h: 20 rx: 4 fill: sweep}
                }
            }
            Tile{
                caption: Label{text: "RadGradient"}
                Vector{width: 80 height: 80 viewbox: vec4(0 0 24 24)
                    Circle{cx: 12 cy: 12 r: 10 fill: bloom}
                }
            }
            Tile{
                caption: Label{text: "DropShadow"}
                Vector{width: 80 height: 80 viewbox: vec4(0 0 24 24)
                    Rect{x: 4 y: 4 w: 14 h: 14 rx: 3 fill: #x4fc3f7 filter: lift}
                }
            }
        }

        StoryHeading{text: "Transforms apply to a group"}
        StoryNote{text: "A Group takes a list of transforms and carries its children through them in order. The same three squares, moved, turned, scaled and skewed."}
        StoryRow{
            Tile{
                caption: Label{text: "none"}
                Vector{width: 80 height: 80 viewbox: vec4(0 0 24 24)
                    Group{
                        Rect{x: 6 y: 6 w: 12 h: 12 fill: #x4fc3f7}
                    }
                }
            }
            Tile{
                caption: Label{text: "Rotate"}
                Vector{width: 80 height: 80 viewbox: vec4(0 0 24 24)
                    Group{transform: [Translate{x: 12 y: 12} Rotate{deg: 30} Translate{x: -12 y: -12}]
                        Rect{x: 6 y: 6 w: 12 h: 12 fill: #x66bb6a}
                    }
                }
            }
            Tile{
                caption: Label{text: "Scale"}
                Vector{width: 80 height: 80 viewbox: vec4(0 0 24 24)
                    Group{transform: [Scale{x: 1.4 y: 0.7}]
                        Rect{x: 6 y: 6 w: 12 h: 12 fill: #xffca28}
                    }
                }
            }
            Tile{
                caption: Label{text: "SkewX"}
                Vector{width: 80 height: 80 viewbox: vec4(0 0 24 24)
                    Group{transform: [SkewX{deg: 20}]
                        Rect{x: 6 y: 6 w: 12 h: 12 fill: #xef5350}
                    }
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/vector/overview",
    category: "Data display",
    component: "Vector",
    also: &[
        "Path",
        "Rect",
        "Circle",
        "Ellipse",
        "Line",
        "Polyline",
        "Polygon",
        "Stop",
        "DropShadow",
        "Rotate",
        "Scale",
        "Translate",
        "SkewX",
        "SkewY",
        "Group",
        "Gradient",
        "RadGradient",
        "Filter",
    ],
    name: "Overview",
    dsl: "VectorOverview",
    added: "2025-05-06",
    tags: &[],
    doc: "# Vector

A drawing written in the DSL rather than loaded from a file.

Where `Svg` reads a document off disk, `Vector` *is* the document: `Path`, `Rect`, `Circle`, `Ellipse`, `Line`, `Polyline` and `Polygon` declared as children, inside a `viewbox` that says what coordinate system they are drawn in. A `Path` takes the same `d` string an SVG file would, so a shape can be moved from one to the other unchanged.

`fill` takes a colour, a named gradient, or `false` for no fill at all; `stroke` and `stroke_width` draw the outline, with `stroke_linecap` and `stroke_linejoin` shaping its ends and corners.

**The pieces that are not shapes are named and referred to.** A `Gradient` or `RadGradient` holds `Stop` children and is bound with a `let`; a shape then says `fill: that_name`. A `Filter` holding a `DropShadow` works the same way, and applies to a single shape or to a whole `Group`.

**Transforms belong to a group, not to a shape.** `Group{transform: [...]}` carries its children through the list in order — `Translate`, `Rotate`, `Scale`, `SkewX`, `SkewY`. Rotating about a point other than the origin is the usual translate-rotate-translate sandwich, which the third tile below does.

Choose this over `Svg` when the drawing is small, when it has to be composed from the same DSL as the rest of the page, or when a value in it should come from somewhere else. Choose `Svg` when the artwork already exists as a file.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];
