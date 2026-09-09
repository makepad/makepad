//! The animated gif story: a widget that plays a gif, fed from bytes because
//! that is the only way it takes one.
use crate::makepad_widgets::animated_image_gif::AnimatedImageGif;
use crate::makepad_widgets::*;
use crate::registry::Story;

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

    mod.stories.AnimatedGifOverview = StoryPage{
        StoryNote{text: "A gif, played. The frames and their delays come from the file; the widget decodes them, uploads each to a texture and steps through them on its own clock."}

        StoryHeading{text: "Three frames, on a loop"}
        StoryNote{text: "Both of these hold the same three-frame gif. The one on the left plays it; the one on the right was given autoplay: false and sits on its first frame until something starts it."}
        StoryRow{
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1
                align: Align{x: 0.5}
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                playing := mod.storybook.StoryGif{}
                Label{text: "autoplay" draw_text +: {color: theme.color_text_meta}}
            }
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1
                align: Align{x: 0.5}
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                halted := mod.storybook.StoryGif{autoplay: false}
                Label{text: "autoplay: false" draw_text +: {color: theme.color_text_meta}}
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

pub const STORIES: &[Story] = &[Story {
    key: "media/animatedgif/overview",
    category: "Media",
    component: "AnimatedImageGif",
    also: &[],
    name: "Overview",
    dsl: "AnimatedGifOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# AnimatedImageGif

A gif, played. It decodes the file, uploads each frame to a texture, and steps through them on its own clock using the delays the file carries.

**There is no source property.** Everything else that shows a picture takes a resource in the DSL; this one does not. The only way in is `load_gif_from_data(cx, &[u8])`, so a host has to hold the bytes and hand them over — from `include_bytes!`, from disk, or from the network. The two on this page are fed by a few lines of Rust behind the story, because there is no other way to feed them.

`inner` is the `Image` the frames are drawn into, so its `fit` is what decides how the gif sits in the space you gave it. `loop_count` is zero for for ever. `autoplay` is `true`; the second one here is `false` and holds its first frame.

The gif on this page is a hundred and twenty-six bytes, written out as a byte array rather than added to the repository as a file — a catalogue page needs something that moves, and it did not need to be a picture of anything.",
    subject: "playing",
    feature: None,
    controls: &[],
    on_actions: None,
}];
