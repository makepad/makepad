//! The kind-dispatching preview well.
//!
//! One widget a host drops anywhere it wants to SHOW an asset, handed typed
//! content and left to pick the face: a still, a cycling sprite sheet, a
//! track's picture with a transport, or an honest note about why there is
//! nothing to draw.
//!
//! Faces that need the renderer (mesh orbit, world walker) are staged: they
//! live in the hosts today and move here next, behind the same
//! [`PreviewContent`] door, so adopting them later is a `show` call and a
//! deleted copy rather than a re-plumb.

use crate::audio_view::{AudioAction, AudioView};
use makepad_widgets::*;

/// What a host is asking the well to show. Everything is already decoded or
/// already bytes: the well never fetches, decodes from disk, or asks a
/// server for anything.
pub enum PreviewContent {
    /// Nothing to show, and why — "click an asset", "loading…", "no preview
    /// for this kind". An empty well always says which.
    Empty(String),
    /// One picture: an image asset, a rendered icon, a still.
    Still(Texture),
    /// A cycling sprite sheet's cells (a billboard), at `fps`.
    Animation { frames: Vec<Texture>, fps: f32 },
    /// A track: its picture, where the playhead is, and what to print.
    Audio {
        picture: Option<Texture>,
        fraction: f64,
        playing: bool,
        position: String,
    },
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ContentPreviewBase = #(ContentPreview::register_widget(vm))
    mod.widgets.ContentPreview = set_type_default() do mod.widgets.ContentPreviewBase{
        width: Fill
        height: Fill
        flow: Overlay
        show_bg: true
        draw_bg +: { color: theme.color_bg_app }

        faces := PageFlip{
            width: Fill
            height: Fill
            active_page: @empty_face
            empty_face := View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                padding: 16
                empty_note := Label{
                    width: Fill
                    draw_text +: {
                        color: #x8a939d
                        text_style: theme.font_regular{font_size: 8.5}
                        wrap: TextWrap.Word
                    }
                }
            }
            still_face := View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                // FITTED, never stretched: a wide picture in a tall panel
                // is letterboxed.
                still := Image{
                    width: Fill
                    height: Fill
                    fit: ImageFit.Smallest
                }
            }
            audio_face := View{
                width: Fill height: Fill
                audio := mod.widgets.AudioView{}
            }
        }
    }
}

/// A preview well: hand it content, it picks the face.
#[derive(Script, ScriptHook, Widget)]
pub struct ContentPreview {
    #[deref]
    view: View,
    /// Cells of the sheet currently cycling, and how fast.
    #[rust]
    frames: Vec<Texture>,
    #[rust]
    fps: f32,
}

impl ContentPreview {
    pub fn show(&mut self, cx: &mut Cx, content: PreviewContent) {
        match content {
            PreviewContent::Empty(note) => {
                self.frames.clear();
                self.view.label(cx, ids!(empty_note)).set_text(cx, &note);
                self.face(cx, id!(empty_face));
            }
            PreviewContent::Still(texture) => {
                self.frames.clear();
                self.view.image(cx, ids!(still)).set_texture(cx, Some(texture));
                self.face(cx, id!(still_face));
            }
            PreviewContent::Animation { frames, fps } => {
                if let Some(first) = frames.first() {
                    self.view.image(cx, ids!(still)).set_texture(cx, Some(first.clone()));
                }
                self.frames = frames;
                self.fps = fps.max(1.0);
                self.face(cx, id!(still_face));
                if self.frames.len() > 1 {
                    cx.new_next_frame();
                }
            }
            PreviewContent::Audio { picture, fraction, playing, position } => {
                self.frames.clear();
                if let Some(mut audio) =
                    self.view.widget(cx, ids!(audio)).borrow_mut::<AudioView>()
                {
                    audio.set_picture(cx, picture);
                    audio.set_transport(cx, fraction, playing, &position);
                }
                self.face(cx, id!(audio_face));
            }
        }
        self.view.redraw(cx);
    }

    /// Update the playhead without rebuilding the content — a host ticks
    /// this from its own clock while the track plays.
    pub fn set_transport(&self, cx: &mut Cx, fraction: f64, playing: bool, position: &str) {
        if let Some(mut audio) = self.view.widget(cx, ids!(audio)).borrow_mut::<AudioView>() {
            audio.set_transport(cx, fraction, playing, position);
        }
    }

    /// True while the user has hold of the playhead.
    pub fn is_scrubbing(&self, cx: &mut Cx) -> bool {
        self.view
            .widget(cx, ids!(audio))
            .borrow::<AudioView>()
            .map(|audio| audio.is_scrubbing())
            .unwrap_or(false)
    }

    /// What the user asked of the transport this pass.
    pub fn audio_action(&self, cx: &mut Cx, actions: &Actions) -> AudioAction {
        let uid = self.view.widget(cx, ids!(audio)).widget_uid();
        match actions.find_widget_action(uid) {
            Some(action) => action.cast(),
            None => AudioAction::None,
        }
    }

    fn face(&mut self, cx: &mut Cx, page: LiveId) {
        self.view
            .page_flip(cx, ids!(faces))
            .set_active_page(cx, page.into());
    }
}

impl Widget for ContentPreview {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // A cycling sheet picks its cell by the clock, so several wells (and
        // the grid behind them) stay in step.
        if self.frames.len() > 1 {
            let index = ((cx.time() * self.fps as f64) as usize) % self.frames.len();
            let texture = self.frames[index].clone();
            self.view.image(cx, ids!(still)).set_texture(cx, Some(texture));
        }
        while self.view.draw_walk(cx, scope, walk).is_step() {}
        if self.frames.len() > 1 {
            cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::NextFrame(_)) && self.frames.len() > 1 {
            self.view.redraw(cx);
        }
    }
}


