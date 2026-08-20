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
    /// A track: which one it is, its picture, where the playhead is, and
    /// what to print.
    ///
    /// `track` is the host's own name for it — an asset id, a path, whatever
    /// is stable — and it comes on EVERY refresh. `clip` is the real file and
    /// comes ONCE, when the host has finally fetched it; the well then
    /// decodes it and draws the spectrogram and waveform of THIS track, with
    /// a toggle between them. `picture` is the catalog thumbnail: instant,
    /// already in the grid, and only ever the placeholder underneath.
    ///
    /// The pair is what makes a refresh cheap AND safe: without the name,
    /// "no bytes this time" is indistinguishable from "no track", and the
    /// decode a host is waiting for gets cancelled by its own next redraw.
    Audio {
        track: String,
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
        // A well is as tall as what it is showing needs — see
        // `ContentPreview::natural_height`. A host that wants to rule the
        // height itself says so by handing over a Fill or a fixed size.
        height: Fit
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

/// What SHAPE the thing on show wants the well to be.
///
/// A preview is not one shape, and pretending it is gives every kind the
/// worst of the others: a track is a band of picture and a row of transport,
/// so a tall well around it is a void the rail's own sections could have
/// used, while a turntable or a walkthrough is the opposite — height is the
/// whole point of looking at it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WellShape {
    /// A line of text: enough room to read it and no more.
    Note,
    /// A picture, at this aspect (width ÷ height).
    Picture { aspect: f64 },
    /// A track: a band of picture with the transport under it.
    Track,
    /// A turntable or a walkthrough: generous, because that is the point.
    Scene,
}

/// The band a track's picture gets, as an aspect. The spectrogram is 4:1 and
/// the waveform 8:1; the band takes the taller of the two so toggling between
/// the faces does not resize the panel under the pointer.
const TRACK_BAND_ASPECT: f64 = 4.0;
/// Play button + position + face toggle, plus the spacing above them.
const TRANSPORT_H: f64 = 26.0;
/// A band narrower than this is unreadable and unscrubbable, wider than this
/// is a poster of a waveform.
const TRACK_BAND_MIN: f64 = 48.0;
const TRACK_BAND_MAX: f64 = 220.0;
/// A note needs one or two lines and its padding.
const NOTE_H: f64 = 96.0;
/// A turntable / walkthrough well.
const SCENE_H: f64 = 300.0;
/// A picture is never squatter than this, however wide the source is: an 8:1
/// sprite sheet still has to be something you can look at.
const PICTURE_MIN_H: f64 = 90.0;

/// A preview well: hand it content, it picks the face.
#[derive(Script, ScriptHook, Widget)]
pub struct ContentPreview {
    #[deref]
    view: View,
    /// Set while the still face holds a cycling animation: the well keeps
    /// the frame pump running. The `AssetThumb` picks the frames.
    #[rust]
    animating: bool,
    /// What the current content wants the well to be shaped like.
    #[rust(WellShape::Note)]
    shape: WellShape,
}

impl ContentPreview {
    /// How tall this well wants to be at the width it has been given.
    ///
    /// Public because a host that lays the well out itself (a fixed-height
    /// panel, a splitter pane) can still ask what the content would like.
    pub fn natural_height(&self, width: f64) -> f64 {
        match self.shape {
            WellShape::Note => NOTE_H,
            WellShape::Picture { aspect } => {
                (width / aspect.max(0.01)).clamp(PICTURE_MIN_H, width)
            }
            WellShape::Track => {
                (width / TRACK_BAND_ASPECT).clamp(TRACK_BAND_MIN, TRACK_BAND_MAX) + TRANSPORT_H
            }
            WellShape::Scene => SCENE_H,
        }
    }

    /// The shape a picture wants: its own aspect, read off the texture.
    fn picture_shape(cx: &mut Cx, media: Option<&crate::thumb::ThumbMedia>) -> WellShape {
        let Some(texture) = media.map(crate::thumb::ThumbMedia::first) else {
            return WellShape::Note;
        };
        let texture = texture.clone();
        match texture.get_format(cx).vec_width_height() {
            Some((w, h)) if w > 0 && h > 0 => WellShape::Picture {
                aspect: w as f64 / h as f64,
            },
            _ => WellShape::Note,
        }
    }

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
                self.shape = WellShape::Note;
                self.view.label(cx, ids!(empty_note)).set_text(cx, &note);
                self.face(cx, id!(empty_face));
            }
            PreviewContent::Still(texture) => {
                let media = crate::thumb::ThumbMedia::still(texture);
                self.shape = Self::picture_shape(cx, Some(&media));
                self.set_still(cx, Some(media));
                self.face(cx, id!(still_face));
            }
            PreviewContent::Animation { frames, fps } => {
                let media = (!frames.is_empty())
                    .then(|| crate::thumb::ThumbMedia::anim(frames, fps));
                self.shape = Self::picture_shape(cx, media.as_ref());
                self.set_still(cx, media);
                self.face(cx, id!(still_face));
            }
            PreviewContent::Audio { track, picture, clip, fraction, playing, position } => {
                self.set_still(cx, None);
                self.shape = WellShape::Track;
                if let Some(mut audio) =
                    self.view.widget(cx, ids!(audio)).borrow_mut::<AudioView>()
                {
                    // The thumbnail goes up first and instantly; the real
                    // file replaces it the moment the worker has drawn it.
                    // Which of those two a refresh means is decided by the
                    // track NAME, not by whether bytes came with it.
                    audio.set_picture(cx, picture);
                    audio.show_track(cx, &track, clip);
                    audio.set_transport(cx, fraction, playing, &position);
                }
                self.face(cx, id!(audio_face));
            }
            PreviewContent::Mesh { glb, texture_png } => {
                self.set_still(cx, None);
                self.shape = WellShape::Scene;
                self.show_scene(cx, glb, texture_png, false);
            }
            PreviewContent::World { glb, texture_png } => {
                self.set_still(cx, None);
                self.shape = WellShape::Scene;
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
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, mut walk: Walk) -> DrawStep {
        // A host that asks for a Fit height is asking the CONTENT how tall
        // this should be — a track gets a band and a transport row, a scene
        // gets room to turn in. A host that names a height keeps it.
        if matches!(walk.height, Size::Fit { .. }) {
            let width = cx.peek_walk_turtle(walk).size.x;
            if width.is_finite() && width > 0.0 {
                walk.height = Size::Fixed(self.natural_height(width));
            }
        }
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


