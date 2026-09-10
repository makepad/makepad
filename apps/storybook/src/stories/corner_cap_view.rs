//! The corner cap story: a rounded corner that is not one, next to the
//! rounded corner that is, so the difference is on the page rather than in
//! the prose.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Tile = View{
        width: Fit height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5}
        caption := Label{text: "" draw_text +: {color: theme.color_text_meta}}
    }

    // The stand-in for the content this widget exists for. A photograph is
    // not expensive to draw, but it is content whose pixels the library did
    // not author, which is the part that matters here.
    let Surface = Image{
        width: Fill height: Fill
        fit: ImageFit.CropToFill
        src: crate_resource("self:resources/photo_landscape.jpg")
    }

    // A band of flat colour to stand the surface on, so the corners have
    // something other than the page ground behind them.
    // Wider than the radius on purpose: a cap painted the wrong colour is
    // only legible when there is enough of the right colour around it to
    // compare against.
    // SolidView, not a View with show_bg: a bare View's draw_bg has no
    // colour in its shader, so it takes the property and paints nothing.
    let Band = SolidView{
        width: Fit height: Fit
        padding: theme.space_5
        draw_bg +: {color: theme.color_primary}
    }

    mod.stories.CornerCapViewOverview = StoryPage{
        StoryNote{text: "Nothing on this page is rounded. CornerCapView draws its children square and then paints four small patches of the surrounding colour over the corners, so what reads as a curve is really the chrome arriving early. On a flat ground of exactly that colour nobody can tell. Anywhere else they can, and the rows below are arranged so you can."}

        StoryHeading{text: "On the page, where the trick holds"}
        StoryNote{text: "All three sit on the window's own ground, which is what the cap colour defaults to. The capped one and the cached one are indistinguishable; the bare one is the same picture with nothing done to it. Only one of the three costs a render target."}
        StoryRow{
            Tile{
                caption: Label{text: "CornerCapView"}
                CornerCapView{width: 200. height: 120. Surface{}}
            }
            Tile{
                caption: Label{text: "CachedRoundedView"}
                CachedRoundedView{
                    width: 200. height: 120.
                    // Half of the visual radius: sdf.box draws twice what it
                    // is given, so 4 here is the 8 the caps are using.
                    draw_bg +: {border_radius: 4.0}
                    Surface{}
                }
            }
            Tile{
                caption: Label{text: "nothing"}
                View{width: 200. height: 120. Surface{}}
            }
        }

        StoryHeading{text: "Off the page, where it does not"}
        StoryNote{text: "The same widget three times over a coloured band. The first still believes it is on the window ground and paints that into the corners, which is exactly the failure this widget cannot detect: nothing about a widget lets it see the pixels beneath it. The second is told what is really behind it and is right again. The third is over a gradient, where no single colour is right and two of the corners must be wrong whichever one you pick."}
        StoryRow{
            Tile{
                caption: Label{text: "cap colour left at the default"}
                Band{
                    CornerCapView{
                        width: 180. height: 110.
                        radius: theme.radius_xl
                        Surface{}
                    }
                }
            }
            Tile{
                caption: Label{text: "cap colour told the truth"}
                Band{
                    CornerCapView{
                        width: 180. height: 110.
                        radius: theme.radius_xl
                        draw_cap +: {cap_color: theme.color_primary}
                        Surface{}
                    }
                }
            }
            Tile{
                caption: Label{text: "over a gradient, no colour is right"}
                GradientXView{
                    width: Fit height: Fit
                    padding: theme.space_5
                    draw_bg +: {
                        color: theme.color_primary
                        color_2: theme.color_primary_container
                    }
                    CornerCapView{
                        width: 180. height: 110.
                        radius: theme.radius_xl
                        draw_cap +: {cap_color: theme.color_primary}
                        Surface{}
                    }
                }
            }
        }

        StoryHeading{text: "One corner at a time"}
        StoryNote{text: "A corner left at -1 follows the shared radius; a corner given a number of its own takes that instead, and 0 leaves it square. This is the same spelling every other radius in the library uses, and it is what a card wants: the picture band rounds its top two corners because the body of the card is directly under the other two."}
        StoryRow{
            Tile{
                caption: Label{text: "top corners only"}
                CornerCapView{
                    width: 180. height: 110.
                    radius_br: 0.0
                    radius_bl: 0.0
                    Surface{}
                }
            }
            Tile{
                caption: Label{text: "one big, three square"}
                CornerCapView{
                    width: 180. height: 110.
                    radius: 0.0
                    radius_tl: 40.0
                    Surface{}
                }
            }
            Tile{
                caption: Label{text: "a radius larger than the box"}
                CornerCapView{
                    width: 180. height: 110.
                    radius: 999.0
                    Surface{}
                }
            }
        }

        StoryHeading{text: "The caps are paint, not a mask"}
        StoryNote{text: "The strip below is a second child laid over the picture, square-cornered and running the full width. It is drawn in full, corners and all; the caps simply arrive after it. Nothing inside is clipped, moved or told anything, which is also why a press landing in a fake corner still reaches whatever is under it."}
        StoryRow{
            Tile{
                caption: Label{text: "a child that reaches into the corner"}
                CornerCapView{
                    width: 240. height: 110.
                    radius: 24.0
                    Surface{}
                    RectView{
                        width: Fill height: 26.
                        draw_bg +: {color: theme.color_primary}
                    }
                }
            }
            Tile{
                caption: Label{text: "one to drive"}
                subject := CornerCapView{
                    width: 240. height: 110.
                    Surface{}
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/cornercapview/overview",
    category: "Containers",
    component: "CornerCapView",
    also: &[],
    name: "Overview",
    dsl: "CornerCapViewOverview",
    added: "2026-09-10",
    tags: &["new", "layout"],
    doc: "# CornerCapView

**It is a fake.** Nothing is rounded. The children are drawn square and four small patches of the surrounding colour are painted over the corners afterwards, so the curve you see is the chrome arriving a few pixels early. Over a flat ground of exactly that colour the illusion is complete. Over a gradient, a picture, a shadow or a glass panel each corner shows as a slightly wrong square, and no property on this widget fixes it, because none could. **`CachedRoundedView` is the version that is right in every case** — it draws its children into a texture and samples that inside a real rounded path — and it is the one to reach for unless the next paragraph is about your content.

**Why it exists.** `CachedRoundedView` costs an offscreen render target the size of its rect, rebuilt whenever anything inside changes, plus an SDF test over every pixel on the way back. For a panel of buttons that is free: the target is built once and reused while the panel sits still. For content that changes every frame and whose shader the library cannot edit — video, a live chart, a page of a document, a map canvas, an embedded browser — it is the wrong trade twice, because the target is rebuilt on every one of those frames and the whole-rect test runs on every one of them too. Four patches the width of the radius do their per-pixel arithmetic over a few hundred pixels instead, and the expensive surface keeps drawing the same plain quad it always drew.

**The colour is an assertion nobody checks.** `draw_cap.cap_color` defaults to the page ground, which is true while the surface sits on the page and false the moment somebody moves it into a card, a well or a coloured band — and the widget will not notice, because no widget can read the pixels under it. This is the failure worth rehearsing: it does not look like a bug, it looks like a corner that is very slightly the wrong colour, and it survives review. A translucent cap colour is worse still, because the content shows through the patch: the caps composite, they do not erase.

**Per corner.** `radius` is what every corner takes unless it was given a number of its own; `-1` on a corner means \"follow `radius`\", which is the spelling `Button` and the button groups already use, and `0` squares it. Every radius is then held to half the shorter side, so two caps on one edge may meet but never pass through each other, and a box with no area gets no caps rather than caps bigger than itself.

**`radius` here is the visual radius.** That is worth saying because `CachedRoundedView`'s `border_radius` is *half* of one — `sdf.box` draws twice what it is handed — so matching the two by eye means writing 4 in one place and 8 in the other.

**What it will not do.** It does not clip: a child painting into the corner still paints there and is merely covered up, so a corner is only ever as clean as the colour is right. It claims no hits and reads no events, so a press in a fake corner reaches the content under it — the one respect in which the fake behaves better than the real thing. And it is a container, not an overlay: it draws its own children and caps its own rect, so there is no ordering rule to remember and no way to forget to put it last.",
    subject: "subject",
    feature: None,
    controls: &[
        Control {
            label: "Radius",
            target: "subject",
            kind: ControlKind::Number { prop: "radius", min: 0., max: 40., step: 0.5, default: 8. },
        },
        Control {
            label: "Top left",
            target: "subject",
            kind: ControlKind::Number {
                prop: "radius_tl",
                min: -1.,
                max: 40.,
                step: 0.5,
                default: -1.,
            },
        },
        Control {
            label: "Top right",
            target: "subject",
            kind: ControlKind::Number {
                prop: "radius_tr",
                min: -1.,
                max: 40.,
                step: 0.5,
                default: -1.,
            },
        },
        Control {
            label: "Bottom right",
            target: "subject",
            kind: ControlKind::Number {
                prop: "radius_br",
                min: -1.,
                max: 40.,
                step: 0.5,
                default: -1.,
            },
        },
        Control {
            label: "Bottom left",
            target: "subject",
            kind: ControlKind::Number {
                prop: "radius_bl",
                min: -1.,
                max: 40.,
                step: 0.5,
                default: -1.,
            },
        },
    ],
    on_actions: None,
}];
