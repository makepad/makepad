//! The kind-dispatching preview well.
//!
//! One widget a host drops anywhere it wants to SHOW an asset, handed typed
//! content and left to pick the face: a still, a cycling sprite sheet, a
//! track's picture with a transport, or an honest note about why there is
//! nothing to draw.
//!
//! The faces that need the renderer — a mesh on a turntable, a world that
//! walks itself — are behind the `renderer` feature, because
//! `makepad-render` is a real dependency and `apps/sandbox` (a nested cargo
//! workspace) must be able to take the 2D faces without it. Without the
//! feature a host that asks for one gets an honest note saying so, not a
//! blank panel.

use crate::audio_view::{AudioAction, AudioView};
use crate::clip::ClipFormat;
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
    ///
    /// `picture` is the catalog thumbnail — instant, already in the grid, and
    /// only ever a placeholder. `clip` is the real file; when it is present
    /// the well decodes it and draws the spectrogram and waveform of THIS
    /// track, with a toggle between them.
    Audio {
        picture: Option<Texture>,
        clip: Option<(Vec<u8>, ClipFormat)>,
        fraction: f64,
        playing: bool,
        position: String,
    },
    /// A mesh, turning on a turntable. Bytes in, never an asset id: the host
    /// resolved the catalog, this draws what it was handed.
    ///
    /// Needs the `renderer` feature.
    Mesh { glb: Vec<u8>, texture_png: Option<Vec<u8>> },
    /// A world, walked. The same autonomous walkthrough the VJ runs: build
    /// the level's collision and navigation off the frame thread, then let a
    /// walker tour it, opening doors as it goes.
    ///
    /// Needs the `renderer` feature.
    World { glb: Vec<u8>, texture_png: Option<Vec<u8>> },
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
                        // No `wrap:` property exists in the new system — a
                        // Label wraps because its own flow is right_wrap.
                    }
                }
            }
            still_face := View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                // THE thumbnail widget — fitted, never stretched; a wide
                // picture in a tall panel is letterboxed, and a declared
                // animation cycles at its declared rate.
                still := mod.widgets.AssetThumb{}
            }
            audio_face := View{
                width: Fill height: Fill
                audio := mod.widgets.AudioView{}
            }
            scene_face := View{
                width: Fill height: Fill
                scene := mod.widgets.SceneView{}
            }
        }
    }
}

/// A preview well: hand it content, it picks the face.
#[derive(Script, ScriptHook, Widget)]
pub struct ContentPreview {
    #[deref]
    view: View,
    /// Set while the still face holds a cycling animation: the well keeps
    /// the frame pump running. The `AssetThumb` picks the frames.
    #[rust]
    animating: bool,
}

impl ContentPreview {
    /// Bind the still face's thumbnail widget — the ONE thumbnail path.
    fn set_still(&mut self, cx: &mut Cx, media: Option<crate::thumb::ThumbMedia>) {
        self.animating = media
            .as_ref()
            .is_some_and(crate::thumb::ThumbMedia::is_animated);
        if let Some(mut thumb) = self
            .view
            .widget(cx, ids!(still))
            .borrow_mut::<crate::thumb::AssetThumb>()
        {
            thumb.set_media(cx, media);
        }
        if self.animating {
            cx.new_next_frame();
        }
    }

    pub fn show(&mut self, cx: &mut Cx, content: PreviewContent) {
        match content {
            PreviewContent::Empty(note) => {
                self.set_still(cx, None);
                self.view.label(cx, ids!(empty_note)).set_text(cx, &note);
                self.face(cx, id!(empty_face));
            }
            PreviewContent::Still(texture) => {
                self.set_still(cx, Some(crate::thumb::ThumbMedia::still(texture)));
                self.face(cx, id!(still_face));
            }
            PreviewContent::Animation { frames, fps } => {
                if frames.is_empty() {
                    self.set_still(cx, None);
                } else {
                    self.set_still(cx, Some(crate::thumb::ThumbMedia::anim(frames, fps)));
                }
                self.face(cx, id!(still_face));
            }
            PreviewContent::Audio { picture, clip, fraction, playing, position } => {
                self.set_still(cx, None);
                if let Some(mut audio) =
                    self.view.widget(cx, ids!(audio)).borrow_mut::<AudioView>()
                {
                    // The thumbnail goes up first and instantly; the real
                    // file replaces it the moment the worker has drawn it.
                    audio.set_picture(cx, picture);
                    match clip {
                        Some((bytes, format)) => audio.set_clip(cx, bytes, format),
                        None => audio.clear_clip(cx),
                    }
                    audio.set_transport(cx, fraction, playing, &position);
                }
                self.face(cx, id!(audio_face));
            }
            PreviewContent::Mesh { glb, texture_png } => {
                self.set_still(cx, None);
                self.show_scene(cx, glb, texture_png, false);
            }
            PreviewContent::World { glb, texture_png } => {
                self.set_still(cx, None);
                self.show_scene(cx, glb, texture_png, true);
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

    /// Hand a GLB to the 3D face. Without the `renderer` feature there is
    /// no such face, and the well says so rather than showing nothing.
    #[cfg(feature = "renderer")]
    fn show_scene(&mut self, cx: &mut Cx, glb: Vec<u8>, texture_png: Option<Vec<u8>>, world: bool) {
        use crate::scene_view::SceneView;
        if let Some(mut scene) = self.view.widget(cx, ids!(scene)).borrow_mut::<SceneView>() {
            match world {
                true => scene.show_world(cx, glb, texture_png, "", Vec::new()),
                false => scene.show_mesh(cx, glb, texture_png),
            }
        }
        self.face(cx, id!(scene_face));
    }

    #[cfg(not(feature = "renderer"))]
    fn show_scene(&mut self, cx: &mut Cx, _glb: Vec<u8>, _png: Option<Vec<u8>>, _world: bool) {
        self.view.label(cx, ids!(empty_note)).set_text(
            cx,
            "3D preview needs the renderer feature of makepad-asset-widgets",
        );
        self.face(cx, id!(empty_face));
    }

    fn face(&mut self, cx: &mut Cx, page: LiveId) {
        self.view
            .page_flip(cx, ids!(faces))
            .set_active_page(cx, page.into());
    }
}

impl Widget for ContentPreview {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The AssetThumb picks its cell by the shared clock; the well only
        // keeps the frame pump alive while an animation is on this face.
        while self.view.draw_walk(cx, scope, walk).is_step() {}
        if self.animating {
            cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::NextFrame(_)) && self.animating {
            self.view.redraw(cx);
        }
    }
}


