//! The image story: a picture that rounds, strokes and crops itself under
//! the controls, one bitmap under every fit, a picture turned in its box, a
//! crossfade between two pictures, and a gif that plays.
use crate::makepad_widgets::animated_image_gif::AnimatedImageGif;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

/// A three-frame gif, eight pixels square, written out here rather than added
/// to the repository as a file: the page needs something that actually moves,
/// and a hundred and twenty-six bytes of it is cheaper than an asset.
const TINY_GIF: &[u8] = &[
    71, 73, 70, 56, 57, 97, 8, 0, 8, 0, 241, 0, 0, 79, 195, 247, 171, 71, 188, 102, 187, 106, 0, 0,
    0, 33, 255, 11, 78, 69, 84, 83, 67, 65, 80, 69, 50, 46, 48, 3, 1, 0, 0, 0, 33, 249, 4, 4, 30, 0,
    0, 0, 44, 0, 0, 0, 0, 8, 0, 8, 0, 0, 2, 6, 132, 143, 169, 203, 237, 93, 0, 33, 249, 4, 4, 30, 0,
    0, 0, 44, 0, 0, 0, 0, 8, 0, 8, 0, 0, 2, 6, 140, 143, 169, 203, 237, 93, 0, 33, 249, 4, 4, 30, 0,
    0, 0, 44, 0, 0, 0, 0, 8, 0, 8, 0, 0, 2, 6, 148, 143, 169, 203, 237, 93, 0, 59,
];

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryGifBase = #(StoryGif::register_widget(vm))

    mod.storybook.StoryGif = set_type_default() do mod.storybook.StoryGifBase{
        width: 96. height: 96.
        inner: Image{
            fit: ImageFit.Stretch
            width: Fill
            height: Fill
        }
    }

    // One photo all the way down the page. What changes is the shape of the
    // box around it, because the box's shape against the picture's is the
    // whole of what the crop dial decides.
    let Photo = Image{
        width: Fill
        height: Fill
        fit: ImageFit.CropToFill
        src: crate_resource("self:resources/photo_landscape.jpg")
    }

    // A ground with a colour of its own, so a bar that takes no colour is
    // plainly the ground showing through and not a black bar.
    let Frame = SolidView{
        width: 160
        height: 96
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: #x2f4858}
    }

    // Square and as wide as the turned picture's diagonal, so no angle
    // carries a corner of it out of the box.
    let TurnFrame = SolidView{
        width: 120
        height: 120
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: #x2f4858}
    }

    let RoundFrame = RoundedView{
        width: 160
        height: 96
        padding: 0
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: #x2f4858 border_radius: 10.0}
    }

    // The box a fit is judged against: wider than the nearly square picture
    // it holds, so a fit that keeps the picture's shape shows it. Plain
    // values: the solid view declares its colour as an instance already.
    let FitFrame = SolidView{
        width: 100
        height: 64
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: theme.color_inset_1}
    }

    let Step = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5 y: 0.0}
    }

    let Caption = Label{draw_text +: {color: theme.color_text_meta}}

    let GifTile = View{
        width: Fit height: Fit flow: Down spacing: theme.space_1
        align: Align{x: 0.5}
        padding: theme.mspace_2
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_low}
    }

    mod.stories.ImageOverview = StoryPage{
        StoryNote{text: "A bitmap sized like any other widget. A picture rounds its own corners, strokes its own edge and dials between the whole of itself and a covering crop. All of it is the image's own shader, so the corners of a photograph are drawn by the thing that draws the photograph, and there is nothing left to line up."}
        StoryNote{text: "Which one to use: Image for a picture the app ships or already holds. Media, on Loading and fallback, for a picture that has to arrive and may not: it keeps the box's shape while it waits and tries a second source when the first fails. ImageBlend crossfades between two pictures, AnimatedImageGif plays a gif, and Svg is for a drawing rather than a bitmap."}

        StoryHeading{text: "Every dial on one picture, under the controls"}
        StoryNote{text: "The radius is the number RoundedView takes, the stroke sits on the edge where a view's does, and the crop is a dial rather than a switch: one covers the box, zero puts the whole picture inside it, and in between the picture reads large without losing its middle. The bars left over take a colour of their own, clear here, so the ground shows through."}
        StoryRow{
            SolidView{
                width: Fill
                height: 170
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {color: #x2f4858}
                subject := Photo{
                    width: 300
                    height: 150
                    crop: 0.65
                    draw_bg +: {
                        border_radius: 10.0
                        border_size: 1.5
                        border_color: #xffffff66
                        letterbox_color: #0000
                    }
                }
            }
        }

        StoryHeading{text: "One picture under every fit"}
        StoryNote{text: "The fit decides how a picture meets the box it is given. Stretch, the default, fills the box and gives up the picture's shape. Horizontal keeps the width and sets the height from the picture, and Vertical keeps the height and sets the width. Smallest fits the whole picture inside, Biggest covers the box and runs past it, and CropToFill keeps the box and crops the picture in the shader. The boxes are wider than the nearly square picture, so Vertical and Smallest leave the ground showing at the sides. Horizontal, Biggest and CropToFill look alike here: the first two grow the widget past its box, which the box clips, and the third keeps the box and crops in the shader. An image given no size at all is a hundred points square."}
        View{
            width: Fill
            height: Fit
            flow: Flow.Right{wrap: true}
            spacing: theme.space_2
            Step{
                FitFrame{ Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Stretch} }
                Caption{text: "Stretch"}
            }
            Step{
                FitFrame{ Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Horizontal} }
                Caption{text: "Horizontal"}
            }
            Step{
                FitFrame{ Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Vertical} }
                Caption{text: "Vertical"}
            }
            Step{
                FitFrame{ Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Smallest} }
                Caption{text: "Smallest"}
            }
            Step{
                FitFrame{ Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Biggest} }
                Caption{text: "Biggest"}
            }
            Step{
                FitFrame{ Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.CropToFill} }
                Caption{text: "CropToFill"}
            }
        }

        StoryHeading{text: "The corners are the picture's own"}
        StoryNote{text: "A rounded ground with a square picture over it shows at the four corners. The same number on the picture and on the view behind it rounds both alike, because both hand it to the same box function."}
        StoryRow{
            Step{
                RoundFrame{ Photo{} }
                Caption{text: "a rounded box, a square picture"}
            }
            Step{
                RoundFrame{ Photo{draw_bg +: {border_radius: 10.0}} }
                Caption{text: "the same number on the picture"}
            }
            Step{
                Frame{
                    Photo{
                        draw_bg +: {
                            border_radius: 10.0
                            border_size: 1.5
                            border_color: #xffffff80
                        }
                    }
                }
                Caption{text: "and a stroke on its edge"}
            }
        }

        StoryHeading{text: "From the whole picture to a covering one"}
        StoryNote{text: "The same photo in the same box at three settings of the dial. At zero the whole of it is inside, with the bars beside it; at one it covers the box and whatever will not fit is cropped away; halfway is halfway. Nothing moves the picture off centre: half of what is cropped comes off each side, and half of what is left over goes to each end."}
        StoryRow{
            Step{
                Frame{ Photo{crop: 0.0} }
                Caption{text: "crop: 0"}
            }
            Step{
                Frame{ Photo{crop: 0.5} }
                Caption{text: "crop: 0.5"}
            }
            Step{
                Frame{ Photo{crop: 1.0} }
                Caption{text: "crop: 1"}
            }
        }

        StoryHeading{text: "The bars take a colour"}
        StoryNote{text: "A bar is clear unless it is asked for a colour, so a picture that does not fill its box shows whatever is behind it rather than a black band nobody chose. Ask, and it is the colour asked for — which is what a photograph in a dark panel usually wants."}
        StoryRow{
            Step{
                Frame{ Photo{crop: 0.0} }
                Caption{text: "clear: the ground shows through"}
            }
            Step{
                Frame{ Photo{crop: 0.0 draw_bg +: {letterbox_color: #x101418}} }
                Caption{text: "a colour of its own"}
            }
            Step{
                Frame{
                    Photo{
                        crop: 0.0
                        draw_bg +: {
                            letterbox_color: #x101418
                            border_radius: 10.0
                            border_size: 1.5
                            border_color: #xffffff80
                        }
                    }
                }
                Caption{text: "bars inside the rounding"}
            }
        }

        StoryHeading{text: "Turned"}
        StoryNote{text: "rotation turns the picture, in degrees, once image_dim_w and image_dim_h say how large the picture is drawn inside its box. It turns whole about the box's middle and keeps its shape at every angle, and what it leaves uncovered is a bar, clear here. Without that size, rotation does nothing."}
        StoryRow{
            Step{
                TurnFrame{ Photo{fit: ImageFit.Stretch draw_bg +: {rotation: 0.0 image_dim_w: 78.0 image_dim_h: 76.0}} }
                Caption{text: "rotation: 0"}
            }
            Step{
                TurnFrame{ Photo{fit: ImageFit.Stretch draw_bg +: {rotation: 30.0 image_dim_w: 78.0 image_dim_h: 76.0}} }
                Caption{text: "rotation: 30"}
            }
            Step{
                TurnFrame{ Photo{fit: ImageFit.Stretch draw_bg +: {rotation: -45.0 image_dim_w: 78.0 image_dim_h: 76.0}} }
                Caption{text: "rotation: -45"}
            }
        }

        StoryHeading{text: "Two pictures, crossfaded"}
        StoryNote{text: "ImageBlend holds two images, one over the other, and fades the second in or out when it is told to switch. Press the button to fade between the two."}
        StoryRow{
            blendbutton := Button{text: "Blend Image"}
        }
        StoryRow{
            blendimage := ImageBlend{
                width: 320
                height: 180
                align: Align{x: 0.0 y: 0.0}
                image_a +: {
                    src: crate_resource("self:resources/ducky.png")
                    fit: ImageFit.Smallest
                    width: Fill
                    height: Fill
                }
                image_b +: {
                    src: crate_resource("self:resources/photo_landscape.jpg")
                    fit: ImageFit.Smallest
                    width: Fill
                    height: Fill
                }
            }
        }

        StoryHeading{text: "A gif, played"}
        StoryNote{text: "AnimatedImageGif takes the gif's bytes, not a source: it has no property to name a file with, so a host hands it the data in Rust. The frames and their delays come from the file; the widget decodes them, uploads each to a texture and steps through them on its own clock. Both of these hold the same three-frame gif. The one on the left plays it; the one on the right was given autoplay: false and sits on its first frame until something starts it."}
        StoryRow{
            GifTile{
                playing := mod.storybook.StoryGif{}
                Caption{text: "autoplay"}
            }
            GifTile{
                halted := mod.storybook.StoryGif{autoplay: false}
                Caption{text: "autoplay: false"}
            }
        }

        StoryHeading{text: "Whole texels"}
        StoryNote{text: "Both of these are the same thirty texels of the photograph blown up to the same size. The right one reads the texture one whole texel at a time, which is what pixel art and a close look want; the left one is filtered, which is what a photograph wants. It is done by snapping the read to the texel's centre rather than by asking the sampler for nearest, because only the filtered read has its channel order corrected on the web target — a nearest one comes back there with red and blue swapped."}
        StoryRow{
            Step{
                Frame{
                    Photo{
                        fit: ImageFit.Stretch
                        draw_bg +: {image_scale: vec2(0.04, 0.04) image_pan: vec2(0.46, 0.44)}
                    }
                }
                Caption{text: "sample_mode: 0"}
            }
            Step{
                Frame{
                    Photo{
                        fit: ImageFit.Stretch
                        draw_bg +: {
                            image_scale: vec2(0.04, 0.04)
                            image_pan: vec2(0.46, 0.44)
                            sample_mode: -1.0
                        }
                    }
                }
                Caption{text: "sample_mode: -1"}
            }
        }
    }
}

#[derive(Script, Widget)]
pub struct StoryGif {
    #[deref]
    gif: AnimatedImageGif,
}

impl Widget for StoryGif {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.gif.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.gif.handle_event(cx, event, scope);
    }
}

impl ScriptHook for StoryGif {
    /// The gif arrives as bytes, and there is nowhere in the DSL to put them:
    /// the widget has no source property, only `load_gif_from_data`. So a
    /// host has to hand it the data itself, and this is the smallest place to
    /// do that from.
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            if let Err(e) = self.gif.load_gif_from_data(cx, TINY_GIF) {
                error!("story gif did not decode: {e:?}");
            }
        });
    }
}

/// The crossfade's button: the one thing on this page that reacts to a press.
fn image_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(blendbutton)).clicked(actions) {
        root.image_blend(cx, ids!(blendimage)).switch_image(cx);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/image/overview",
    category: "Media",
    component: "Image",
    also: &["ImageBlend", "AnimatedImageGif"],
    name: "Overview",
    dsl: "ImageOverview",
    added: "2026-06-04",
    tags: &["ported", "rounded", "corner", "radius", "border", "stroke", "crop", "cover", "contain", "letterbox", "bar", "pixel", "texel", "rotation", "turned"],
    doc: "# Image

A bitmap sized like any other widget. `src` takes the resource, `fit` decides how the picture meets the box, and `draw_bg` carries the picture's own shape: its corners, its stroke, the colour of the bars a fit leaves over, and how it is sampled.

## Which one to use

| Want | Use |
|---|---|
| a picture the app ships or already holds | `Image` |
| a picture that has to arrive and may not | `Media`, on Loading and fallback |
| a crossfade between two pictures | `ImageBlend` |
| a gif that plays | `AnimatedImageGif` |
| a drawing rather than a bitmap | `Svg` |

## Fits

| Fit | What it does |
|---|---|
| `Stretch` | the default: fills the box and gives up the picture's shape |
| `Horizontal` | keeps the box's width and sets the height from the picture |
| `Vertical` | keeps the box's height and sets the width from the picture |
| `Smallest` | resizes the widget so the whole picture fits inside the box |
| `Biggest` | resizes the widget so the picture covers the box, and may run past it |
| `CropToFill` | keeps the box, and crops the long axis in the shader |
| `Size` | ignores the box and takes the picture's own size |

An image given no size is a hundred points square.

## The radius is the view's radius

`border_radius`, `border_size`, `border_color` and `letterbox_color` sit beside the fit and the pan on `draw_bg`, and a picture left alone by all four draws the same picture it always did, returned before any of the shape work.

`border_radius` is the number `RoundedView` takes, handed to the same box function, so the same number on a view and on the picture inside it rounds both alike — which is the only way the two edges can agree at the corner. As there, the visible radius is twice the number: a view at `10.0` and a picture at `10.0` both draw a twenty-pixel corner. The two part company only below one, which a view floors and the picture does not, and where no corner is visible either way.

The stroke is placed where a view's is: the box is inset by the stroke's width on all four sides, and the band is then drawn centred on that inset edge — half of it over the photograph and half outside it, exactly as a view strokes its own. That is what lets the two meet at a shared edge without a seam.

## The crop is a dial

`crop` on the widget says how much of the overflow a `CropToFill` picture crops away. One is the crop this fit has always done, so it is the default. Zero puts the whole picture inside the same box and leaves the ends over. In between is genuinely in between, which is the setting worth knowing about: a wide clip cropped all the way to cover loses its subject off both sides, and the same clip at two thirds reads large and still has its middle.

Halfway is drawn at a size halfway between the two, in the picture's own shape: the window narrows on one axis exactly as fast as it widens on the other, so the ratio between them never moves and no setting of the dial squashes anything. Part way along a wide box that means bars at the sides and a crop at the top and bottom at the same time, which is what being between contain and cover looks like.

The other fits read nothing here. They resize the rect around the picture instead of choosing a window onto it, and a picture that got its own rect never has anything left over.

## The bars take a colour

What is left over is `letterbox_color`, and it is clear. A bar forced to black is a decision made on the caller's behalf, and it is the wrong one as often as it is right: over a coloured panel it is a black band nobody asked for. Clear means the ground behind shows through, which is what a page already has behind the picture; ask for a colour and the bar is that colour, inside the rounding along with everything else.

The bars belong to the framing, not to the caller's own pan. `image_pan` is how a sprite sheet picks its cell and how a viewer moves a picture under its window, and both still read the edge texel past the edge.

## Turned

`rotation` on `draw_bg` turns the picture, in degrees. It applies only when `image_dim_w` and `image_dim_h` are set: they are the size, in points, the picture is drawn at inside the box. The picture turns whole about the box's middle and keeps its shape at every angle, and whatever it leaves uncovered is a bar in `letterbox_color`. With `image_dim_w` at zero, `rotation` does nothing. A box that has to hold the picture at every angle wants sides as long as the picture's diagonal.

## ImageBlend

Two images, `image_a` and `image_b`, one over the other. `switch_image` fades `image_b` in, or back out, over about half a second. `set_texture` does the same and puts the texture into the image it is fading to, so a host can crossfade to a picture it has just decoded.

## AnimatedImageGif

A gif, played. It decodes the file, uploads each frame to a texture, and steps through them on its own clock using the delays the file carries.

**There is no source property.** Everything else that shows a picture takes a resource in the DSL; this one does not. The only way in is `load_gif_from_data(cx, &[u8])`, so a host has to hold the bytes and hand them over — from `include_bytes!`, from disk, or from the network.

`inner` is the `Image` the frames are drawn into, so its `fit` is what decides how the gif sits in the space you gave it. A `loop_count` of zero loops for ever. `autoplay` is `true`; the second one here is `false` and holds its first frame.

## Whole texels

`sample_mode` below zero reads every texel whole, for pixel art and for a close look. Above zero it is the device pixel ratio, and the read stays filtered until a texel is drawn more than four device pixels wide. Zero, the default, always filters.

It is done by snapping the read to the texel's centre and then filtering as usual, not by asking the sampler for a nearest read. That is not a detail: the filtered read is the one whose channel order the web backend corrects, and a nearest read on that target comes back with red and blue swapped.

## What it deliberately does not do

It does not take a radius per corner. One number rounds all four, which is what a picture in a rounded box wants; a card whose picture rounds only its top two still needs the view that can say so.

It does not clip anything but itself. The picture has no children, and rounding it rounds the picture — a panel that wants its contents clipped to a curve is still a different problem.

It does not reach a vector source. An image given an SVG draws through its own vector call and never through this shader, so the corner, stroke, bar and texel settings belong to the bitmap path and an SVG takes none of them — the markup will accept them there without a word.

And it does not choose where a cropped picture is taken from. Centred, both axes, always.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Crop", target: "subject", kind: ControlKind::Number { prop: "crop", min: 0., max: 1., step: 0.05, default: 0.65 } },
        Control { label: "Corner radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 24., step: 0.5, default: 10. } },
        Control { label: "Stroke", target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_size", min: 0., max: 6., step: 0.5, default: 1.5 } },
        Control { label: "Stroke colour", target: "subject", kind: ControlKind::Color { prop: "draw_bg.border_color", default: 0xFFFFFF66 } },
        Control { label: "Bar colour", target: "subject", kind: ControlKind::Color { prop: "draw_bg.letterbox_color", default: 0x00000000 } },
        Control { label: "Texels", target: "subject", kind: ControlKind::Number { prop: "draw_bg.sample_mode", min: -1., max: 3., step: 1., default: 0. } },
    ],
    on_actions: Some(image_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is markup, which the compiler never reads, and the panel
    /// addresses one widget inside it by name. Building it and reaching that
    /// widget is what turns a mistake in either into a failed build; and
    /// every setting the page demonstrates has to have arrived on the draw
    /// struct, because the DSL takes a misspelled one just as happily and
    /// declares a shader prop of its own that nothing reads.
    #[test]
    fn the_page_builds_and_its_subject_carries_every_dial() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = STORIES.iter().find(|story| story.key == "media/image/overview").unwrap();
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile");
        for target in std::iter::once(story.subject)
            .chain(story.controls.iter().map(|c| c.target))
            .chain(["blendbutton", "blendimage", "playing", "halted"])
            .filter(|t| !t.is_empty())
        {
            assert!(!page.widget(&cx, &[LiveId::from_str(target)]).is_empty(), "no widget at {target}");
        }
        let subject = page.widget(&cx, &[LiveId::from_str(story.subject)]);
        let image = subject.borrow::<Image>().expect("the subject is an Image");
        assert_eq!(image.crop, 0.65, "the page opens with the dial part way");
        assert_eq!(image.draw_bg.border_radius, 10.0);
        assert_eq!(image.draw_bg.border_size, 1.5);
        assert_eq!(image.draw_bg.letterbox_color.w, 0.0, "the bar is clear, so the ground shows through");
    }

    /// A control's default is what the panel shows before anyone touches it,
    /// and what Reset goes back to. A default the slider cannot reach is a
    /// value shown once and never again; a default that disagrees with the
    /// markup is a panel describing a picture that is not there.
    #[test]
    fn the_controls_agree_with_the_page_they_drive() {
        for story in STORIES {
            for control in story.controls {
                if let ControlKind::Number { prop, min, max, step, default } = &control.kind {
                    assert!(min < max, "{} {}: min {} is not below max {}", story.key, prop, min, max);
                    assert!(*step > 0., "{} {}: step {} does not move", story.key, prop, step);
                    assert!(
                        min <= default && default <= max,
                        "{} {}: default {} is outside {}..{}",
                        story.key, prop, default, min, max
                    );
                }
            }
        }
    }
}
