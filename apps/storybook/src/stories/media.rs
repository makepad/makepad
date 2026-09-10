//! The media story: the three answers a picture needs around it when the
//! app did not author the picture.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.MediaOverview = StoryPage{
        StoryNote{text: "An image has no size until its bytes are decoded, nothing to show while they are on their way, and no way to say that they never came. Media is those three answers around it: a box with a shape of its own from the first frame, something standing in that box while it waits, and a second source to try when the first one fails."}

        StoryHeading{text: "The box has a shape before the picture does"}
        StoryNote{text: "Each box below states a ratio and a width, so its height is settled before anything is loaded and the row does not jump when the picture lands. The same picture is in all three. Nothing here is cropped by accident: the ratio is the box, and the fit decides what happens to the picture inside it."}
        StoryRow{
            MediaFigure{
                width: 190. height: Fit
                media: Media{width: 190. height: Fit ratio: 2.4 src: crate_resource("self:resources/ducky.png")}
                caption: MediaCaption{text: "ratio 2.4"}
            }
            MediaFigure{
                width: 190. height: Fit
                media: Media{width: 190. height: Fit ratio: 1.0 src: crate_resource("self:resources/ducky.png")}
                caption: MediaCaption{text: "ratio 1.0"}
            }
            MediaFigure{
                width: 190. height: Fit
                media: Media{width: 190. height: Fit ratio: 0.7 src: crate_resource("self:resources/ducky.png")}
                caption: MediaCaption{text: "ratio 0.7"}
            }
        }

        StoryHeading{text: "Three ways to fit"}
        StoryNote{text: "Cover keeps the picture's shape and fills the box, cropping the overhang. Contain keeps its shape and puts the whole of it inside, so the ground shows where it does not reach. Fill stretches it to the box, shape and all. The picture is nearly square and the boxes are wide, so all three differ."}
        StoryRow{
            MediaFigure{
                width: 190. height: Fit
                media: Media{width: 190. height: Fit ratio: 2.4 fit: MediaFit.Cover src: crate_resource("self:resources/photo_landscape.jpg")}
                caption: MediaCaption{text: "Cover"}
            }
            MediaFigure{
                width: 190. height: Fit
                media: Media{width: 190. height: Fit ratio: 2.4 fit: MediaFit.Contain src: crate_resource("self:resources/photo_landscape.jpg")}
                caption: MediaCaption{text: "Contain"}
            }
            MediaFigure{
                width: 190. height: Fit
                media: Media{width: 190. height: Fit ratio: 2.4 fit: MediaFit.Fill src: crate_resource("self:resources/photo_landscape.jpg")}
                caption: MediaCaption{text: "Fill"}
            }
        }

        StoryHeading{text: "When the source is not there"}
        StoryNote{text: "The first box asks for a file that does not exist and names a second source to try; the second asks for the same missing file and names nothing. A source that is absent, that errored, or that answers with bytes which are not a picture — a redirect page, a truncated file — all count the same, because to the reader they are the same."}
        StoryRow{
            MediaFigure{
                width: 190. height: Fit
                media: Media{
                    width: 190. height: Fit ratio: 1.6
                    src: crate_resource("self:resources/no_such_picture.png")
                    fallback: crate_resource("self:resources/ducky.png")
                }
                caption: MediaCaption{text: "src missing, fallback shown"}
            }
            MediaFigure{
                width: 190. height: Fit
                media: Media{
                    width: 190. height: Fit ratio: 1.6
                    src: crate_resource("self:resources/no_such_picture.png")
                }
                caption: MediaCaption{text: "nothing left to try"}
            }
        }

        StoryHeading{text: "Waiting, and not waiting"}
        StoryNote{text: "These two are the slots themselves, side by side, because a file on this disk arrives too fast to watch. The shimmer says something is on its way; the still one says nothing is. Both are slots on the widget, so an app with a better answer — a blurred thumbnail, its own artwork — puts that there instead."}
        StoryRow{
            MediaFigure{
                width: 190. height: Fit
                media: ContentPlaceholder{width: 190. height: 119.}
                caption: MediaCaption{text: "while it loads"}
            }
            MediaFigure{
                width: 190. height: Fit
                media: ContentPlaceholder{width: 190. height: 119. animation: Static}
                caption: MediaCaption{text: "when it failed"}
            }
        }

        StoryHeading{text: "One to drive"}
        StoryNote{text: "The controls panel writes the fit and the ratio on this one. Watch the box change shape while the picture inside it does not."}
        StoryRow{
            subject := Media{
                width: 260. height: Fit
                ratio: 1.5
                fit: MediaFit.Cover
                src: crate_resource("self:resources/photo_landscape.jpg")
            }
            says := Label{text: "the picture above says: loading"}
        }
    }
}

/// The subject reports what it is showing, which is the same word a
/// snapshot reads off it.
fn media_actions(cx: &mut Cx, root: &WidgetRef, _actions: &Actions) {
    let status = root.media(cx, ids!(subject)).status().name();
    let text = format!("the picture above says: {status}");
    let label = root.label(cx, ids!(says));
    if label.text() != text {
        label.set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/media/overview",
    category: "Media",
    component: "Media",
    also: &["MediaFigure", "MediaCaption"],
    name: "Overview",
    dsl: "MediaOverview",
    added: "2026-09-10",
    tags: &["new"],
    doc: "# Media

A picture that copes with not having arrived yet, or not existing.

An `Image` is honest about exactly one thing: the picture. It has no size until the bytes are decoded, so the row it sits in reflows the moment they land; it has nothing to show while they are on their way; and a source that is absent, unreadable, or not a picture at all leaves a hole with no way to say so. Every screen that shows pictures it did not author — a feed, a gallery, a wall of covers — writes the same three answers around it, and writes them differently each time.

`Media` is those three answers and nothing else.

**A shape of its own.** `ratio` is the box's width over its height, and it fills in whichever axis the walk left to the content, so the page settles before the picture exists and does not jump when it arrives. An axis the walk pinned down always wins; `ratio: 0.0` gives the shape up and lets the picture decide — which is the jump, stated as a choice.

**Something in the box meanwhile.** A placeholder while a source is on its way, a still mark when there is nothing left to wait for. Both are slots, so an app with a better answer puts it there. Still means *not coming*: that difference is the whole reason there are two.

**A second source.** Absent, errored, or answering with bytes that are not a picture — a redirect page, a truncated file — all count as failure, because from the reader's side they are the same thing. The header is read the moment the bytes land rather than left to the decoder, which is both how a bad source is caught in time to try the next one and how the box knows the picture's real shape a frame or more before it can draw it.

**The fits.** Cover keeps the picture's shape and fills the box, cropping the overhang; Contain keeps its shape and fits the whole of it inside; Fill stretches it to the box. Cover is done by the image's own shader rather than by drawing something oversized and clipping it — the arithmetic works out the rect either way, and hands the box's own size to a picture that would overflow.

`MediaFigure` puts a caption under one, in the picture's own column. `MediaCaption` is the caption's dress, for the same reason `WellInput` exists: a slot filled with a bare `Label` takes the label's own.

**What it is not.** It is not a loader — the image widget and the image cache below it do every byte of the fetching, decoding and caching, and this only decides which source they are pointed at and what stands in the box while they work. It cannot be told where in the box a cropped picture is taken from: a fitted picture is centred, or wherever the box's own `align` puts it. It does not clip the picture to its own rounded corners, so a covering picture in a rounded box has square corners. It does not retry or back off: a source that failed is done until the source itself changes. And the source goes on the `Media`, not on the image in its slot — the box only knows about what it fetched itself.",
    subject: "subject",
    feature: None,
    controls: &[
        Control {
            label: "Fit",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "fit",
                options: &["MediaFit.Cover", "MediaFit.Contain", "MediaFit.Fill"],
                default: 0,
            },
        },
        Control {
            label: "Ratio",
            target: "subject",
            kind: ControlKind::Number { prop: "ratio", min: 0.3, max: 3.0, step: 0.05, default: 1.5 },
        },
    ],
    on_actions: Some(media_actions),
}];
