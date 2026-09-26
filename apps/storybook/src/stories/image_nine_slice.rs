//! The nine-slice page for `Image`: one small panel texture drawn at any
//! size, its corners kept, its edges and middle stretched, tiled or left out.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // The panel texture every example below slices: sixteen texels of border
    // on each side, studs in the corners, dashes along the edges and a checker
    // in the middle, so each kind of fill shows in what it does to them.
    let Panel = Image{
        fit: ImageFit.Slice
        src: crate_resource("self:resources/panel_slice.png")
        slice: 16
        slice_edge: ImageSliceEdge.Round
        slice_center: ImageSliceCenter.Tile
    }

    // A ground with a theme colour behind each small panel, so a hidden
    // middle plainly shows the ground and changes with the theme while the
    // panel's own colours do not.
    let Ground = SolidView{
        width: Fit
        height: Fit
        padding: theme.mspace_2
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

    mod.stories.ImageNineSlice = StoryPage{
        StoryNote{text: "A panel drawn from one small texture has to appear at whatever size the layout gives it. Slicing cuts the texture into nine parts along an inset in texels: the corners keep their size, the edges fill one way, and the middle fills both ways or is left out."}

        StoryHeading{text: "One texture at any size"}
        StoryNote{text: "The controls resize this panel. Its corners stay sixteen points, its edge dashes stay whole, and the checker in the middle keeps its squares. Squeeze it below thirty-two points and all four corners shrink together, so they stay round."}
        StoryRow{
            SolidView{
                width: Fill
                height: 440
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {color: theme.color_inset_1}
                subject := Panel{width: 280 height: 180}
            }
        }

        StoryHeading{text: "Stretched whole, and sliced"}
        StoryNote{text: "The same texture in the same box. Stretched whole, the corners stretch with it and the studs go oval. Sliced, the corners keep their size; with the edges and the middle tiled, the pattern keeps its spacing too."}
        StoryRow{
            Step{ Ground{ Panel{width: 200 height: 120 fit: ImageFit.Stretch} } Caption{text: "fit: Stretch"} }
            Step{ Ground{ Panel{width: 200 height: 120 slice_edge: ImageSliceEdge.Stretch slice_center: ImageSliceCenter.Stretch} } Caption{text: "sliced, all stretched"} }
            Step{ Ground{ Panel{width: 200 height: 120} } Caption{text: "sliced, edges and middle tiled"} }
        }

        StoryHeading{text: "Edges"}
        StoryNote{text: "Stretch draws an edge's texels once, drawn out to the length. Tile repeats them at the border's own scale, centred, so both ends are cut alike. Round repeats them a whole number of times and adjusts the spacing a little, so nothing is cut."}
        // 220 wide, so the three grounds fit the canvas the catalogue opens
        // with, and so the 188-point edge is not a whole number of tiles:
        // Tile then shows a cut dash where Round shows none.
        StoryRow{
            Step{ Ground{ Panel{width: 220 height: 96 slice_edge: ImageSliceEdge.Stretch slice_center: ImageSliceCenter.Stretch} } Caption{text: "slice_edge: Stretch"} }
            Step{ Ground{ Panel{width: 220 height: 96 slice_edge: ImageSliceEdge.Tile slice_center: ImageSliceCenter.Stretch} } Caption{text: "slice_edge: Tile"} }
            Step{ Ground{ Panel{width: 220 height: 96 slice_edge: ImageSliceEdge.Round slice_center: ImageSliceCenter.Stretch} } Caption{text: "slice_edge: Round"} }
        }

        StoryHeading{text: "The middle"}
        StoryNote{text: "A tiled middle follows the edges' spacing, so its pattern lines up with theirs. Hidden leaves the middle out and the texture becomes a frame, with the ground showing through."}
        StoryRow{
            Step{ Ground{ Panel{width: 150 height: 120 slice_center: ImageSliceCenter.Stretch} } Caption{text: "slice_center: Stretch"} }
            Step{ Ground{ Panel{width: 150 height: 120 slice_center: ImageSliceCenter.Tile} } Caption{text: "slice_center: Tile"} }
            Step{ Ground{ Panel{width: 150 height: 120 slice_center: ImageSliceCenter.Hidden} } Caption{text: "slice_center: Hidden"} }
        }

        StoryHeading{text: "Smaller than its corners"}
        StoryNote{text: "When the box cannot hold both borders, all four shrink by the one factor the box needs. One factor for all four is what keeps a corner round when the panel is squeezed in only one direction."}
        StoryRow{
            Step{ Ground{ Panel{width: 48 height: 48} } Caption{text: "48 by 48"} }
            Step{ Ground{ Panel{width: 32 height: 32} } Caption{text: "32 by 32: the corners meet"} }
            Step{ Ground{ Panel{width: 20 height: 20} } Caption{text: "20 by 20"} }
            Step{ Ground{ Panel{width: 20 height: 64} } Caption{text: "20 by 64: still round"} }
        }

        StoryHeading{text: "Texels, points and pixels"}
        StoryNote{text: "The inset is always in texels. A border texel is drawn at one point by default, the size the picture has at its natural size. Scale draws it larger; device pixels put one texel on one pixel of the screen whatever its density; and a whole-texel read keeps the edges hard wherever the box lands."}
        StoryRow{
            Step{ Ground{ Panel{width: 160 height: 110} } Caption{text: "slice_scale: 1"} }
            Step{ Ground{ Panel{width: 160 height: 110 slice_scale: 2.0} } Caption{text: "slice_scale: 2"} }
            Step{ Ground{ Panel{width: 160 height: 110 slice_units: ImageSliceUnits.DevicePixels} } Caption{text: "slice_units: DevicePixels"} }
            Step{ Ground{ Panel{width: 160 height: 110 slice_scale: 3.0 draw_bg +: {sample_mode: -1.0}} } Caption{text: "slice_scale: 3, whole texels"} }
        }

        StoryHeading{text: "A tiled fill"}
        StoryNote{text: "With no inset there are no borders, and a tiled middle is the whole texture repeated across the box."}
        StoryRow{
            Step{ Ground{ Panel{width: 400 height: 96 slice: 0 slice_edge: ImageSliceEdge.Tile slice_center: ImageSliceCenter.Tile} } Caption{text: "slice: 0, tiled"} }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/image/nine-slice",
    category: "Media",
    component: "Image",
    also: &[],
    name: "Nine-slice",
    dsl: "ImageNineSlice",
    added: "2026-09-13",
    tags: &["new", "nine slice", "slice", "panel", "frame", "skin", "border", "corners", "tile", "repeat", "stretch", "inset", "texel"],
    doc: "# A picture that keeps its corners

A panel drawn from one small texture has to be drawn at every size the layout asks for, and stretching the whole texture stretches its corners with it: the rounding turns oval, the studs smear, and the line along an edge grows thick on the long side. `fit: ImageFit.Slice` cuts the texture into nine parts along an inset and gives each part the stretch that suits it. The four corners keep their size. The top and bottom edges fill their length across and keep their height; the left and right edges fill theirs down and keep their width. The middle fills both ways, or is left out.

## The inset is in texels

`slice` says how many texels of the texture belong to each border, as one number for all four or as `Inset{left top right bottom}`. It counts the texture's own pixels because that is where the cut is: the corner of this panel is sixteen texels whatever size the panel is shown at. An inset is rounded to whole texels, and a pair wider than the texture is scaled down until it fits.

A border texel is drawn at `slice_scale` points, one by default. That is the size `fit: ImageFit.Size` gives the same texture, so a sliced panel at its natural size is the same picture, corners and all. `slice_units: ImageSliceUnits.DevicePixels` counts `slice_scale` in device pixels instead, for a border that should put one texel on one pixel of the screen whatever the display's density. An axis written as `Fit` takes the natural size.

## Edges and middle

`slice_edge` is how the four edges fill their length. `Stretch` draws the edge's texels once, drawn out to the length. `Tile` repeats them at the border's own scale, centred on the edge, so a pattern keeps its spacing and both ends are cut alike. `Round` repeats them a whole number of times and adjusts the spacing a little, so nothing is cut.

`slice_center` is how the middle fills. `Stretch` and `Tile` work as they do for the edges, and a tiled middle takes the edges' spacing, so its pattern lines up with the edges around it. `Hidden` leaves the middle out, which turns a panel texture into a frame.

## A box smaller than its borders

The borders shrink, all four by one factor, and only as far as the box needs. One factor is what keeps a corner round when the panel is squeezed in only one direction.

## Every read stays inside its own part

A filtered read blends a texel with its neighbours, and at a cut the neighbour belongs to another part. Every read is kept half a texel inside its own part, so a stretched edge never picks up a streak of the corner beside it and a tile's last column never borrows the next part's first. A middle one texel wide therefore reads as one flat colour, which is what a one-texel middle is for.

## One quad

The slicing is done in the picture's own pixel shader, mapping each pixel of the one quad the picture already draws to the part of the texture it belongs to. Nine quads would leave cracks along the joins wherever a join falls between two pixels. And because it is the picture's own shader, the rounding, the stroke, the opacity and the whole-texel read the Overview page explains all still apply on top of it.

## What it deliberately does not do

It does not reach a vector source: an image given an SVG draws through its own vector call. It does not slice a picture whose rotation the viewer is driving. It reads neither the crop dial nor any other fit, because a sliced picture takes the box it is given. And it does not take a different scale per border: one texture has one density.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Width", target: "subject", kind: ControlKind::Number { prop: "width", min: 16., max: 560., step: 1., default: 280. } },
        Control { label: "Height", target: "subject", kind: ControlKind::Number { prop: "height", min: 16., max: 400., step: 1., default: 180. } },
        Control { label: "Inset", target: "subject", kind: ControlKind::Number { prop: "slice", min: 0., max: 32., step: 1., default: 16. } },
        Control {
            label: "Edges",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "slice_edge",
                options: &["ImageSliceEdge.Stretch", "ImageSliceEdge.Tile", "ImageSliceEdge.Round"],
                default: 2,
            },
        },
        Control {
            label: "Middle",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "slice_center",
                options: &["ImageSliceCenter.Stretch", "ImageSliceCenter.Tile", "ImageSliceCenter.Hidden"],
                default: 1,
            },
        },
        Control { label: "Scale", target: "subject", kind: ControlKind::Number { prop: "slice_scale", min: 0.5, max: 4., step: 0.25, default: 1. } },
        Control {
            label: "Units",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "slice_units",
                options: &["ImageSliceUnits.Points", "ImageSliceUnits.DevicePixels"],
                default: 0,
            },
        },
        Control {
            label: "Fit",
            target: "subject",
            kind: ControlKind::Choice { prop: "fit", options: &["ImageFit.Stretch", "ImageFit.Slice"], default: 1 },
        },
        Control { label: "Texels", target: "subject", kind: ControlKind::Number { prop: "draw_bg.sample_mode", min: -1., max: 3., step: 1., default: 0. } },
    ],
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    fn story() -> &'static Story {
        STORIES.iter().find(|story| story.dsl == "ImageNineSlice").unwrap()
    }

    /// The page as the canvas builds it, with every template it names
    /// registered and any shader error left over from that cleared first, so
    /// an error taken afterwards is the page's own.
    fn build_page(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = story();
        cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// The page is markup, which the compiler never reads, and the panel
    /// addresses one widget inside it by name. Building it and reaching that
    /// widget is what turns a mistake in either into a failed build; and the
    /// subject has to carry every slice setting the page opens on, because
    /// the markup takes a misspelled one just as happily and declares a
    /// property of its own that nothing reads.
    #[test]
    fn the_nine_slice_page_builds_and_its_subject_is_sliced() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        let story = story();
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
        assert!(matches!(image.fit(), ImageFit::Slice), "the subject is not sliced");
        let slice = image.slice();
        assert_eq!((slice.left, slice.top, slice.right, slice.bottom), (16.0, 16.0, 16.0, 16.0));
        assert_eq!(image.slice_modes(), (ImageSliceEdge::Round, ImageSliceCenter::Tile));
        assert_eq!(image.slice_scale(), (1.0, ImageSliceUnits::Points));
        assert_eq!(image.walk.width, Size::Fixed(280.0));
        assert_eq!(image.walk.height, Size::Fixed(180.0));
    }

    /// A control's default is what the panel shows before anyone touches it,
    /// and what Reset goes back to. A default the slider cannot reach is a
    /// value shown once and never again, and a default that disagrees with
    /// the markup is a panel describing a picture that is not there.
    #[test]
    fn the_numbers_open_on_what_the_subject_carries() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        let story = story();
        let subject = page.widget(&cx, &[LiveId::from_str(story.subject)]);
        let image = subject.borrow::<Image>().expect("the subject is an Image");
        for control in story.controls {
            let ControlKind::Number { prop, min, max, step, default } = control.kind else {
                continue;
            };
            assert!(min < max, "{prop}: min {min} is not below max {max}");
            assert!(step > 0., "{prop}: step {step} does not move");
            assert!(min <= default && default <= max, "{prop}: default {default} is outside {min}..{max}");
            let carried = match prop {
                "width" => match image.walk.width {
                    Size::Fixed(width) => width,
                    other => panic!("the subject's width is {other:?}, not a number a slider can show"),
                },
                "height" => match image.walk.height {
                    Size::Fixed(height) => height,
                    other => panic!("the subject's height is {other:?}, not a number a slider can show"),
                },
                "slice" => image.slice().left,
                "slice_scale" => image.slice_scale().0,
                "draw_bg.sample_mode" => image.draw_bg.sample_mode as f64,
                other => panic!("no reading of {other} on the subject; add one here"),
            };
            assert_eq!(carried, default, "{prop} opens at {default} on a subject that carries {carried}");
        }
    }

    /// A choice writes its option string verbatim, so the option a choice
    /// opens on has to name the variant the subject was built with, or the
    /// first touch of any other control shows a panel that disagrees with
    /// the picture.
    #[test]
    fn the_choices_open_on_what_the_subject_carries() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        let story = story();
        let subject = page.widget(&cx, &[LiveId::from_str(story.subject)]);
        let image = subject.borrow::<Image>().expect("the subject is an Image");
        let mut seen = 0;
        for control in story.controls {
            let ControlKind::Choice { prop, options, default } = control.kind else {
                continue;
            };
            let option = options
                .get(default)
                .unwrap_or_else(|| panic!("{prop}: default {default} is past its {} options", options.len()));
            let carried = match prop {
                "slice_edge" => format!("ImageSliceEdge.{:?}", image.slice_edge),
                "slice_center" => format!("ImageSliceCenter.{:?}", image.slice_center),
                "slice_units" => format!("ImageSliceUnits.{:?}", image.slice_units),
                "fit" => format!("ImageFit.{:?}", image.fit()),
                other => panic!("no reading of {other} on the subject; add one here"),
            };
            assert_eq!(*option, carried, "{prop} opens on {option} on a subject built with {carried}");
            seen += 1;
        }
        assert_eq!(seen, 4, "the page lost a choice this test reads");
    }

    /// The width the canvas has when the catalogue opens (`app.rs`): the
    /// 1400-point window less the navigator's 260, the bar beside it, the
    /// side panels' 380 and the canvas holder's padding on either side.
    /// `/snap` measures the canvas at exactly this.
    const OPENING_CANVAS_WIDTH: f64 = 1400.0 - 260.0 - 6.0 - 380.0 - 2.0 * 6.0;

    fn collect_images(widget: &WidgetRef, out: &mut Vec<WidgetRef>) {
        widget.children(&mut |_, child| {
            if child.borrow::<Image>().is_some() {
                out.push(child.clone());
            }
            collect_images(&child, out);
        });
    }

    /// Every panel on the page is whole on the canvas the catalogue opens
    /// with. The page does not scroll sideways, so a row wider than the
    /// canvas is cut at the page's inner edge, and a demonstration cut there
    /// shows the opposite of what it is for: the edges row lost its last
    /// panel that way, the Round one, whose point is that nothing is cut.
    #[test]
    fn every_panel_is_whole_on_the_canvas_the_catalogue_opens_with() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        let size = dvec2(OPENING_CANVAS_WIDTH, 4000.0);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, size);
        let mut draw_list = DrawList2d::new(&mut cx);
        {
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            page.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            draw_list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }
        let padding = page.borrow::<View>().expect("the page is a View").layout.padding;
        let (left, right) = (padding.left, OPENING_CANVAS_WIDTH - padding.right);
        let mut panels = Vec::new();
        collect_images(&page, &mut panels);
        assert_eq!(panels.len(), 19, "the page lost or gained a panel this test counts");
        for (index, panel) in panels.iter().enumerate() {
            let rect = panel.area().rect(&cx);
            assert!(rect.size.x > 0.0, "panel {index} was not drawn");
            assert!(
                rect.pos.x >= left - 1e-3 && rect.pos.x + rect.size.x <= right + 1e-3,
                "panel {index} spans {:.1}..{:.1}, past the page's inner edges {left:.1}..{right:.1}",
                rect.pos.x,
                rect.pos.x + rect.size.x
            );
        }
    }

    /// The doc leans on another page for the whole-texel read. A page named
    /// by where it sits points somewhere else as soon as its folder is
    /// reordered, which is how this doc came to send the reader to a page
    /// with no such read on it. So it names the page, and the page it names
    /// is one of this component's and does explain the read.
    #[test]
    fn the_doc_names_the_page_it_leans_on_not_where_that_page_sits() {
        let story = story();
        for position in ["previous page", "next page", "page before", "page after"] {
            assert!(
                !story.doc.contains(position),
                "the doc points at the {position}, which is another page whenever the folder is reordered"
            );
        }
        assert!(story.doc.contains("the Overview page"), "the doc no longer says which page explains the whole-texel read");
        let overview = crate::registry::all()
            .find(|page| page.component == story.component && page.name == "Overview")
            .expect("the component has an Overview page");
        assert!(
            overview.doc.contains("`sample_mode` below zero"),
            "{} no longer explains the whole-texel read the nine-slice doc sends the reader to",
            overview.key
        );
    }
}
