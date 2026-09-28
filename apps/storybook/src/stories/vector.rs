//! The vector story: shapes written down rather than loaded, the pieces that
//! go inside one, three icons beside the files they were copied from, and a
//! whole layered icon.
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

    // The layered icon's ground: one dark surface under both twins rather
    // than a tile each, because the card's shadow spills past its 300-point
    // box and the padding has to hold it. Fixed colours throughout, since
    // the ground is fixed in every theme.
    let Ground = SolidView{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        spacing: 32
        padding: 32
        draw_bg +: {color: #x333333}
    }
    let Twin = View{
        width: Fit height: Fit
        flow: Down
        spacing: theme.space_2
        align: Align{x: 0.5}
    }
    let TwinCaption = Label{draw_text +: {color: #x9a9a9a}}

    // The layered icon's gradients
    let glass_bg = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #x556677 opacity: 0.45}
        Stop{offset: 1 color: #x334455 opacity: 0.35}
    }
    let glass_border = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #xffffff opacity: 0.35}
        Stop{offset: 0.4 color: #xffffff opacity: 0.08}
        Stop{offset: 1 color: #xffffff opacity: 0.2}
    }
    let glass_spec = Gradient{x1: 0.1 y1: 0 x2: 0.7 y2: 0.8
        Stop{offset: 0 color: #xffffff opacity: 0.14}
        Stop{offset: 0.5 color: #xffffff opacity: 0.02}
        Stop{offset: 1 color: #xffffff opacity: 0.0}
    }
    let brain_glow_grad = RadGradient{cx: 0.5 cy: 0.45 r: 0.45
        Stop{offset: 0 color: #x4466ee opacity: 0.4}
        Stop{offset: 0.45 color: #x4466dd opacity: 0.15}
        Stop{offset: 1 color: #x4466dd opacity: 0.0}
    }
    let brain_grad = Gradient{x1: 0.5 y1: 0 x2: 0.5 y2: 1
        Stop{offset: 0 color: #x77ccff}
        Stop{offset: 0.4 color: #x7799ee}
        Stop{offset: 0.75 color: #x8866dd}
        Stop{offset: 1 color: #x9944cc}
    }
    let fold_grad = Gradient{x1: 0.5 y1: 0 x2: 0.5 y2: 1
        Stop{offset: 0 color: #xaaddff}
        Stop{offset: 1 color: #xbb99ee}
    }
    let kb_grad = Gradient{x1: 0 y1: 0.5 x2: 1 y2: 0.5
        Stop{offset: 0 color: #x9955ee}
        Stop{offset: 0.5 color: #x6688ff}
        Stop{offset: 1 color: #x44ddcc}
    }
    let kb_body = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #x8855dd opacity: 0.45}
        Stop{offset: 1 color: #x44aacc opacity: 0.3}
    }
    let stem_grad = Gradient{x1: 0.5 y1: 0 x2: 0.5 y2: 1
        Stop{offset: 0 color: #x7799dd}
        Stop{offset: 1 color: #x44cccc}
    }

    // The layered icon's filters
    let icon_shadow = Filter{
        DropShadow{dx: 0 dy: 4 blur: 6 color: #x000000 opacity: 0.5}
    }
    let kb_shadow = Filter{
        DropShadow{dx: 0 dy: 1 blur: 2 color: #x000000 opacity: 0.3}
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

        StoryHeading{text: "A file and its DSL twin"}
        StoryNote{text: "Three icons, each twice: Svg reads it from a file, and beside it a Path draws it from the file's own d string, copied out unchanged. So these are real authored paths, with relative commands, several subpaths and a Z, in the viewbox each file declares. No path sets a fill, and a shape without one paints opaque black, so every tile is tinted to the theme's text colour through draw_svg.color."}
        StoryRow{
            flow: Flow.Right{wrap: true}
            Tile{
                caption: Label{text: "document, file"}
                Svg{width: 48 height: 48 animating: false
                    draw_svg +: {svg: crate_resource("makepad_widgets:resources/icons/icon_file.svg") color: theme.color_text}
                }
            }
            Tile{
                caption: Label{text: "document, DSL"}
                Vector{width: 48 height: 48 viewbox: vec4(0 0 49 49)
                    draw_svg +: {color: theme.color_text}
                    Path{d: "M12.069,11.678c-0,-2.23 1.813,-4.043 4.043,-4.043l10.107,0l-0,8.086c-0,1.118 0.903,2.021 2.021,2.021l8.086,0l-0,18.193c-0,2.23 -1.813,4.043 -4.043,4.043l-16.171,0c-2.23,0 -4.043,-1.813 -4.043,-4.043l-0,-24.257Zm24.257,4.043l-8.086,-0l0,-8.086l8.086,8.086Z"}
                }
            }
            Tile{
                caption: Label{text: "folder, file"}
                Svg{width: 48 height: 48 animating: false
                    draw_svg +: {svg: crate_resource("makepad_widgets:resources/icons/icon_folder.svg") color: theme.color_text}
                }
            }
            Tile{
                caption: Label{text: "folder, DSL"}
                Vector{width: 48 height: 48 viewbox: vec4(0 0 49 49)
                    draw_svg +: {color: theme.color_text}
                    Path{d: "M11.884,37.957l24.257,-0c2.23,-0 4.043,-1.813 4.043,-4.043l-0,-16.172c-0,-2.23 -1.813,-4.042 -4.043,-4.042l-10.107,-0c-0.638,-0 -1.238,-0.297 -1.617,-0.809l-1.213,-1.617c-0.765,-1.017 -1.965,-1.617 -3.235,-1.617l-8.085,-0c-2.23,-0 -4.043,1.813 -4.043,4.043l-0,20.214c-0,2.23 1.813,4.043 4.043,4.043Z"}
                }
            }
            Tile{
                caption: Label{text: "pointer, file"}
                Svg{width: 48 height: 48 animating: false
                    draw_svg +: {svg: crate_resource("makepad_widgets:resources/icons/icon_select.svg") color: theme.color_text}
                }
            }
            Tile{
                caption: Label{text: "pointer, DSL"}
                Vector{width: 48 height: 48 viewbox: vec4(0 0 48 49)
                    draw_svg +: {color: theme.color_text}
                    Path{d: "M33.21,28.207l-6.865,-0l3.562,8.807c0.259,0.582 0,1.295 -0.583,1.554l-3.173,1.36c-0.583,0.259 -1.295,-0.065 -1.554,-0.648l-3.432,-8.354l-5.569,5.764c-0.777,0.777 -1.943,0.194 -1.943,-0.842l0,-27.781c0,-1.101 1.23,-1.619 1.943,-0.842l18.391,18.909c0.777,0.777 0.194,2.073 -0.777,2.073Z"}
                }
            }
        }

        StoryHeading{text: "A layered icon"}
        StoryNote{text: "A whole icon written in the DSL, beside the file it was drawn from. It uses at once what the sections above show one piece at a time: gradients with several stops and an opacity per stop, a gradient used as a stroke, fill_opacity and stroke_opacity on a shape, rx with ry, a Group whose Translate and Scale place a 24-unit drawing on a card drawn in 256-unit coordinates, a Filter on a whole Group, and translucent layers over one another. The two boxes differ on purpose: Vector fits its 256-unit viewbox into 300 points, while Svg fits a document to the bounds of what it draws, the card, rather than to the viewBox it declares, so the file twin's box is 224/256 of 300 and the two cards come out the same size, and a margin of 18.75 points, the card's 16-unit inset at that scale, lines them up. The ground is a fixed dark grey in every theme, because the art is dark translucent glass and a light ground has nothing to show of it."}
        Ground{
            Twin{
                TwinCaption{text: "written in the DSL"}
                Vector{width: 300 height: 300 viewbox: vec4(0 0 256 256)

                    // Glass background
                    Rect{x: 16 y: 16 w: 224 h: 224 rx: 44 ry: 44
                        fill: #x444455 fill_opacity: 0.35 filter: icon_shadow}
                    Rect{x: 16 y: 16 w: 224 h: 224 rx: 44 ry: 44
                        fill: glass_bg}
                    Rect{x: 16 y: 16 w: 224 h: 224 rx: 44 ry: 44
                        fill: false stroke: glass_border stroke_width: 1.5}

                    // Glass specular
                    Path{d: "M60 16 C35.7 16 16 35.7 16 60 L16 105 Q55 55 160 30 L190 16 Z"
                        fill: glass_spec}

                    // Brain glow
                    Circle{cx: 128 cy: 95 r: 80 fill: brain_glow_grad}

                    // Brain paths
                    Group{transform: [Translate{x: 36.8 y: 11.4} Scale{x: 7.6 y: 7.6}]
                        Path{d: "M15.5 13a3.5 3.5 0 0 0 -3.5 3.5v1a3.5 3.5 0 0 0 7 0v-1.8"
                            fill: false stroke: brain_grad stroke_width: 0.35
                            stroke_linecap: "round" stroke_linejoin: "round"}
                        Path{d: "M8.5 13a3.5 3.5 0 0 1 3.5 3.5v1a3.5 3.5 0 0 1 -7 0v-1.8"
                            fill: false stroke: brain_grad stroke_width: 0.35
                            stroke_linecap: "round" stroke_linejoin: "round"}
                        Path{d: "M17.5 16a3.5 3.5 0 0 0 0 -7h-.5"
                            fill: false stroke: brain_grad stroke_width: 0.35
                            stroke_linecap: "round" stroke_linejoin: "round"}
                        Path{d: "M19 9.3v-2.8a3.5 3.5 0 0 0 -7 0"
                            fill: false stroke: brain_grad stroke_width: 0.35
                            stroke_linecap: "round" stroke_linejoin: "round"}
                        Path{d: "M6.5 16a3.5 3.5 0 0 1 0 -7h.5"
                            fill: false stroke: brain_grad stroke_width: 0.35
                            stroke_linecap: "round" stroke_linejoin: "round"}
                        Path{d: "M5 9.3v-2.8a3.5 3.5 0 0 1 7 0v10"
                            fill: false stroke: brain_grad stroke_width: 0.35
                            stroke_linecap: "round" stroke_linejoin: "round"}
                        Path{d: "M15 13a4.17 4.17 0 0 1-3-4 4.17 4.17 0 0 1-3 4"
                            fill: false stroke: fold_grad stroke_width: 0.28
                            stroke_linecap: "round" stroke_linejoin: "round" stroke_opacity: 0.5}
                    }

                    // Stem
                    Path{d: "M128 148 L128 178"
                        fill: false stroke: stem_grad stroke_width: 2.5 stroke_linecap: "round"}

                    // Keyboard
                    Group{filter: kb_shadow
                        Rect{x: 64 y: 185 w: 128 h: 38 rx: 7 ry: 7 fill: kb_body}
                        Rect{x: 64 y: 185 w: 128 h: 38 rx: 7 ry: 7
                            fill: false stroke: kb_grad stroke_width: 1.2}

                        // Keyboard Row 1
                        Rect{x: 73 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 85 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 97 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 109 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 121 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 133 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 145 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 157 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 169 y: 190 w: 15 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}

                        // Keyboard Row 2
                        Rect{x: 76 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                        Rect{x: 88 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                        Rect{x: 100 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                        Rect{x: 112 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                        Rect{x: 124 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                        Rect{x: 136 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                        Rect{x: 148 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                        Rect{x: 160 y: 199 w: 24 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}

                        // Keyboard Row 3
                        Rect{x: 73 y: 208 w: 12 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.12}
                        Rect{x: 88 y: 208 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.12}
                        Rect{x: 100 y: 208 w: 50 h: 6 rx: 2 ry: 2 fill: #xffffff fill_opacity: 0.18}
                        Rect{x: 153 y: 208 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.12}
                        Rect{x: 165 y: 208 w: 19 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.12}
                    }
                }
            }
            Twin{
                TwinCaption{text: "loaded from the file"}
                // 224/256 of 300: Svg fits the document to the bounds of what
                // it draws, the card, so this box gives its card the size the
                // Vector's viewbox gives the other one. The margin is the
                // card's 16-unit inset at the same scale, so it lines the two
                // cards up instead of leaving this one 19 points higher.
                Svg{width: 262.5 height: 262.5 margin: 18.75 animating: false
                    draw_svg +: {svg: crate_resource("self:resources/app_icon.svg")}
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/svg/vector",
    category: "Media",
    component: "Svg",
    also: &[
        "Vector",
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
    name: "Vector",
    dsl: "VectorOverview",
    added: "2025-05-06",
    tags: &["ported", "svg", "icon", "path", "gradient", "filter", "group", "transform", "opacity"],
    doc: "# Vector

A drawing written in the DSL rather than loaded from a file.

Where `Svg` reads a document off disk, `Vector` *is* the document: `Path`, `Rect`, `Circle`, `Ellipse`, `Line`, `Polyline` and `Polygon` declared as children, inside a `viewbox` that says what coordinate system they are drawn in. A `Path` takes the same `d` string an SVG file would, so a shape can be moved from one to the other unchanged.

`fill` takes a colour, a named gradient, or `false` for no fill at all; `stroke` and `stroke_width` draw the outline, with `stroke_linecap` and `stroke_linejoin` shaping its ends and corners.

**The pieces that are not shapes are named and referred to.** A `Gradient` or `RadGradient` holds `Stop` children and is bound with a `let`; a shape then says `fill: that_name`. A `Filter` holding a `DropShadow` works the same way, and applies to a single shape or to a whole `Group`.

**Transforms belong to a group, not to a shape.** `Group{transform: [...]}` carries its children through the list in order — `Translate`, `Rotate`, `Scale`, `SkewX`, `SkewY`. Rotating about a point other than the origin is the usual translate-rotate-translate sandwich, which the Rotate tile does.

## A file and its DSL twin

Three icons, each twice. `Svg` reads each one from the widget library's icon folder, and a `Vector` beside it draws the same shape from a `Path` whose `d` string was copied out of that file unchanged.

That is the point of `Path` taking SVG's own path syntax: a shape moves between a file and the DSL by copying one string, in either direction. These are real authored paths rather than hand-written ones — relative `c` and `l` commands, more than one subpath, a `Z` to close — and the `viewbox` is the one the file declares, `0 0 49 49` for the document and the folder and `0 0 48 49` for the pointer, so the shape lands at the same size and place on both sides.

**A shape with no fill paints opaque black.** That is SVG's default and the DSL keeps it, so an untinted icon vanishes on a dark theme. Every twin here sets `draw_svg.color`, which replaces every colour in the drawing with one; `Vector` carries the same `draw_svg` as `Svg`, so the tint is written the same way on both. Give the `Path` a `fill` instead when the drawing should keep colours of its own.

## A layered icon

A complete icon written in the DSL, with the file it was drawn from beside it. Everything the sections above show one piece at a time is here at once, in the layers a real icon needs.

- **Gradients with several stops, and an opacity per stop.** The glass card is a gradient from a slate at 45% to one at 35%; its border and the specular sweep are white fading to nothing. `Stop{offset: … color: … opacity: …}`.
- **A gradient as a stroke.** The brain outline and the keyboard frame say `fill: false stroke: <gradient>`. A gradient is a paint, and either side of a shape can take it.
- **`fill_opacity` and `stroke_opacity`** on a shape, apart from its colour: the keys are white at 12 to 18 percent, the inner fold a half-strength stroke.
- **`rx` with `ry`**, for a rounded corner that is not a circle.
- **A group placing a drawing.** The brain is drawn in a 24-unit space and put on the 256-unit card by `Group{transform: [Translate{…} Scale{…}]}`; the paths inside are untouched.
- **A filter on a whole group.** `Group{filter: kb_shadow …}` shadows the keyboard as one shape rather than each key on its own.

**The two boxes differ on purpose.** `Vector` fits its `viewbox` into its box, so the 256-unit page becomes 300 points and the card inside it 262.5. `Svg` fits a document to the bounds of what it draws, not to the `viewBox` it declares, so the file twin would fill a 300-point box with the 224-unit card alone; its box is 262.5 points, 224/256 of 300, and the two cards come out the same size, the shadow under the keyboard included. A margin of 18.75 points on every side, the card's 16-unit inset at that scale, lines the two cards up, and with it the file twin takes 300 points too.

The ground is a fixed dark grey in every theme: the art is dark translucent glass and there is nothing for a light ground to show.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];
