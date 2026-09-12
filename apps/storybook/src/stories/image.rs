//! The image stories: one bitmap under every fit mode, ported from the widget
//! zoo, and the page for a picture that rounds, strokes and crops itself.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ImageOverview = StoryPage{
        H4{text: "Default"}
        // A plain View's pixel is transparent and never reads `color`: paint it here.
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1) pixel: fn() {return Pal.premul(self.color)}} width: Fill height: 150 flow: Down
            Image{src: crate_resource("self:resources/ducky.png")}
        }

        Hr{}
        H4{text: "fit: Stretch"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1) pixel: fn() {return Pal.premul(self.color)}} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Stretch}
        }

        Hr{}
        H4{text: "fit: Horizontal"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1) pixel: fn() {return Pal.premul(self.color)}} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Horizontal}
        }

        Hr{}
        H4{text: "fit: Vertical"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1) pixel: fn() {return Pal.premul(self.color)}} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Vertical}
        }

        Hr{}
        H4{text: "fit: Smallest"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1) pixel: fn() {return Pal.premul(self.color)}} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Smallest}
        }

        Hr{}
        H4{text: "fit: Biggest"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1) pixel: fn() {return Pal.premul(self.color)}} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Biggest}
        }

        Hr{}
        H4{text: "fit: CropToFill"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1) pixel: fn() {return Pal.premul(self.color)}} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.CropToFill}
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

    let RoundFrame = RoundedView{
        width: 160
        height: 96
        padding: 0
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: #x2f4858 border_radius: 10.0}
    }

    let Step = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5 y: 0.0}
    }

    let Caption = Label{draw_text +: {color: theme.color_text_meta}}

    mod.stories.ImageRoundedCrop = StoryPage{
        StoryNote{text: "A picture rounds its own corners, strokes its own edge and dials between the whole of itself and a covering crop. All of it is the image's own shader, so the corners of a photograph are drawn by the thing that draws the photograph, and there is nothing left to line up."}

        StoryHeading{text: "Every dial on one picture"}
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

        StoryHeading{text: "The corners are the picture's own"}
        StoryNote{text: "A rounded ground with a square picture over it is what the library did until now, and the four corners are where it shows. The same number on the picture and on the view behind it rounds both alike, because both hand it to the same box function."}
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

pub const STORIES: &[Story] = &[Story {
    key: "media/image/overview",
    category: "Media",
    component: "Image",
    also: &[],
    name: "Overview",
    dsl: "ImageOverview",
    added: "2026-06-04",
    tags: &["ported"],
    doc: "# Image\n\nImages display bitmap content.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}, Story {
    key: "media/image/rounded-and-cropped",
    category: "Media",
    component: "Image",
    also: &[],
    name: "Rounded and cropped",
    dsl: "ImageRoundedCrop",
    added: "2026-09-11",
    tags: &["new", "rounded", "corner", "radius", "border", "stroke", "crop", "cover", "contain", "letterbox", "bar", "pixel", "texel"],
    doc: "# A picture that rounds, strokes and crops itself

Until now a rounded photograph was somebody else's problem. The picture drew a square quad, and whatever wanted it rounded worked around that: a ground with a radius under a picture without one, a private mask shader copied into one widget, or a whole offscreen texture so the corners could be sampled out of it. Four widgets in the library carry four different answers, and the module note on one of them states the defect outright.

The image's own shader does it now. `border_radius`, `border_size`, `border_color` and `letterbox_color` sit beside the fit and the pan on `draw_bg`, and a picture left alone by all four draws the same picture it always drew, returned before any of the shape work.

## The radius is the view's radius

`border_radius` is the number `RoundedView` takes, handed to the same box function, so the same number on a view and on the picture inside it rounds both alike — which is the only way the two edges can agree at the corner. As there, the visible radius is twice the number: a view at `10.0` and a picture at `10.0` both draw a twenty-pixel corner. The two part company only below one, which a view floors and the picture does not, and where no corner is visible either way.

The stroke is placed where a view's is: the box is inset by the stroke's width on all four sides, and the band is then drawn centred on that inset edge — half of it over the photograph and half outside it, exactly as a view strokes its own. That is what lets the two meet at a shared edge without a seam.

## The crop is a dial

`crop` on the widget says how much of the overflow a `CropToFill` picture crops away. One is the crop this fit has always done, so it is the default and nothing that was written before this page changes. Zero puts the whole picture inside the same box and leaves the ends over. In between is genuinely in between, which is the setting worth knowing about: a wide clip cropped all the way to cover loses its subject off both sides, and the same clip at two thirds reads large and still has its middle.

Halfway is drawn at a size halfway between the two, in the picture's own shape: the window narrows on one axis exactly as fast as it widens on the other, so the ratio between them never moves and no setting of the dial squashes anything. Part way along a wide box that means bars at the sides and a crop at the top and bottom at the same time, which is what being between contain and cover looks like.

The other fits read nothing here. They resize the rect around the picture instead of choosing a window onto it, and a picture that got its own rect never has anything left over.

## The bars take a colour

What is left over is `letterbox_color`, and it is clear. A bar forced to black is a decision made on the caller's behalf, and it is the wrong one as often as it is right: over a coloured panel it is a black band nobody asked for. Clear means the ground behind shows through, which is what a page already has behind the picture; ask for a colour and the bar is that colour, inside the rounding along with everything else.

The bars belong to the framing, not to the caller's own pan. `image_pan` is how a sprite sheet picks its cell and how a viewer moves a picture under its window, and both still read the edge texel past the edge exactly as they always did.

## Whole texels

`sample_mode` was declared on the draw struct and read by nothing; the picture always filtered. Below zero every read is now one whole texel, for pixel art and for a close look. Above zero it is the device pixel ratio, and the read stays filtered until a texel is drawn more than four device pixels wide.

It is done by snapping the read to the texel's centre and then filtering as usual, not by asking the sampler for a nearest read. That is not a detail: the filtered read is the one whose channel order the web backend corrects, and a nearest read on that target comes back with red and blue swapped.

## What it deliberately does not do

It does not take a radius per corner. One number rounds all four, which is what a picture in a rounded box wants; a card whose picture rounds only its top two still needs the view that can say so.

It does not clip anything but itself. The picture has no children, and rounding it rounds the picture — a panel that wants its contents clipped to a curve is still a different problem.

It does not reach a vector source. An image given an SVG draws through its own vector call and never through this shader, so all four of these belong to the bitmap path and an SVG takes none of them — the markup will accept them there without a word.

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
    on_actions: None,
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
    fn the_rounded_page_builds_and_its_subject_carries_every_dial() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = STORIES.iter().find(|story| story.dsl == "ImageRoundedCrop").unwrap();
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
