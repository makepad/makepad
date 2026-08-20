//! The audio face of a content preview: a track's picture, a playhead you
//! can drag, and a transport button.
//!
//! # Two sources for the picture
//!
//! The catalog thumbnail is what a host has INSTANTLY — it is already in the
//! grid — so it goes up the moment the well opens. But a thumbnail is a
//! picture somebody baked once, at one size, and it is not the track. Give
//! this widget the actual file ([`AudioView::set_clip`]) and it decodes it on
//! a worker and draws the spectrogram and the waveform of THIS clip, with a
//! toggle between them. The thumbnail is the placeholder; the real file is
//! the picture.
//!
//! Both pictures come from `makepad-audio-picture`, the same recipe the
//! importer bakes with, so the live view and the baked thumbnail are the
//! same picture at the same definition rather than two ideas of one.
//!
//! Whatever the source, the picture draws FITTED (never stretched: a 2048×512
//! spectrogram in a tall panel is letterboxed, not squashed into a line), with
//! a playhead at the fraction the host reports; clicks and drags on it become
//! [`AudioAction::Seek`].
//!
//! It deliberately owns no audio: the host has the mixer, the clip and the
//! clock. This widget knows one number — where the playhead is — and reports
//! where the user wants it instead. It does not open the file either; the
//! host hands over bytes.

use crate::clip::{texture_from_rgba, ClipDecoder, ClipFace, ClipFormat};
use makepad_widgets::*;

/// What the user did to the transport. The host carries it out against
/// whatever it owns.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum AudioAction {
    /// Play/pause was pressed; the host decides which of the two it means.
    TogglePlay,
    /// Move the playhead to this fraction of the clip (0..=1).
    Seek(f64),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AudioViewBase = #(AudioView::register_widget(vm))
    mod.widgets.AudioView = set_type_default() do mod.widgets.AudioViewBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 4

        // The picture, letterboxed inside whatever room the host gives it.
        wave := View{
            width: Fill
            height: Fill
            cursor: MouseCursor.Hand
            flow: Overlay
            picture := Image{
                width: Fill
                height: Fill
                fit: ImageFit.Smallest
            }
            playhead := View{
                width: 2
                height: Fill
                abs_pos: vec2(0.0, 0.0)
                show_bg: true
                draw_bg +: {
                    color: #xff5a4d
                    pixel: fn() {
                        return self.color
                    }
                }
            }
        }
        controls := View{
            width: Fill height: Fit
            flow: Right spacing: 6
            align: Align{y: 0.5}
            play := RoundedView{
                width: 34 height: 22
                cursor: MouseCursor.Hand
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {
                    color: #x1b1b1f
                    border_color: #xffffff14
                    border_size: 1.0
                    border_radius: 3.0
                }
                play_glyph := Label{
                    text: "▶"
                    draw_text +: {
                        color: #xdfe6ec
                        text_style: theme.font_regular{font_size: 10}
                    }
                }
            }
            position := Label{
                width: Fit
                draw_text +: {
                    color: #x8a939d
                    text_style: theme.font_regular{font_size: 8}
                }
            }
            spacer := View{width: Fill height: 1}
            // Which picture of the clip. Hidden until the file has actually
            // been decoded — offering a toggle over a thumbnail would
            // promise a face the well cannot show.
            face_toggle := RoundedView{
                width: 52 height: 22
                visible: false
                cursor: MouseCursor.Hand
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {
                    color: #x1b1b1f
                    border_color: #xffffff14
                    border_size: 1.0
                    border_radius: 3.0
                }
                face_label := Label{
                    text: "FFT"
                    draw_text +: {
                        color: #xdfe6ec
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
            }
        }
    }
}

/// A track's picture with a draggable playhead.
#[derive(Script, ScriptHook, Widget)]
pub struct AudioView {
    #[deref]
    view: View,
    /// Where the host says the playhead is, 0..=1.
    #[rust]
    fraction: f64,
    /// True while the user is dragging it: the host's own playhead updates
    /// must not fight the finger that is moving it.
    #[rust]
    scrubbing: bool,
    #[rust]
    playing: bool,
    /// The catalog thumbnail: what goes up instantly, and what stays if the
    /// real file never arrives or has nothing to show.
    #[rust]
    placeholder: Option<Texture>,
    /// The pictures of the actual clip, once its bytes have been decoded.
    #[rust]
    fft: Option<Texture>,
    #[rust]
    wave: Option<Texture>,
    #[rust]
    face: ClipFace,
    #[rust]
    decoder: ClipDecoder,
}

impl AudioView {
    /// The placeholder picture: the catalog thumbnail, drawn immediately
    /// while the real file decodes. A host with no picture at all passes
    /// `None` and gets an empty well rather than a stretched nothing.
    pub fn set_picture(&mut self, cx: &mut Cx, texture: Option<Texture>) {
        self.placeholder = texture;
        self.show_face(cx);
    }

    /// The real file. Decoded on a worker this widget owns; when it lands,
    /// the well switches from the thumbnail to the spectrogram and waveform
    /// of THIS clip, and the face toggle appears.
    ///
    /// Passing new bytes abandons any decode already in flight, so clicking
    /// down a list never lands the track before last.
    pub fn set_clip(&mut self, cx: &mut Cx, bytes: Vec<u8>, format: ClipFormat) {
        self.fft = None;
        self.wave = None;
        self.decoder.start(bytes, format);
        self.show_face(cx);
        cx.new_next_frame();
    }

    /// Forget the clip and go back to the placeholder.
    pub fn clear_clip(&mut self, cx: &mut Cx) {
        self.decoder.clear();
        self.fft = None;
        self.wave = None;
        self.show_face(cx);
    }

    /// Which picture of the clip is showing.
    pub fn face(&self) -> ClipFace {
        self.face
    }

    pub fn set_face(&mut self, cx: &mut Cx, face: ClipFace) {
        self.face = face;
        self.show_face(cx);
    }

    /// True once the real file's pictures are up — the thumbnail is no
    /// longer what the viewer is looking at.
    pub fn has_clip(&self) -> bool {
        self.fft.is_some() || self.wave.is_some()
    }

    /// Put the right texture in the image and keep the toggle honest.
    fn show_face(&mut self, cx: &mut Cx) {
        let clip = match self.face {
            ClipFace::Fft => self.fft.clone().or_else(|| self.wave.clone()),
            ClipFace::Wave => self.wave.clone().or_else(|| self.fft.clone()),
        };
        let texture = clip.or_else(|| self.placeholder.clone());
        self.view.image(cx, ids!(picture)).set_texture(cx, texture);
        // Only offer the toggle when there are genuinely two pictures.
        let both = self.fft.is_some() && self.wave.is_some();
        self.view.widget(cx, ids!(face_toggle)).set_visible(cx, both);
        self.view.label(cx, ids!(face_label)).set_text(cx, self.face.label());
        self.view.redraw(cx);
    }

    /// Take a finished decode, if one landed this pass.
    fn collect_clip(&mut self, cx: &mut Cx) -> bool {
        let Some(pictures) = self.decoder.poll() else {
            return false;
        };
        self.fft = pictures
            .fft
            .map(|(rgba, w, h)| texture_from_rgba(cx, &rgba, w, h));
        self.wave = pictures
            .wave
            .map(|(rgba, w, h)| texture_from_rgba(cx, &rgba, w, h));
        // A clip with nothing to show keeps the thumbnail rather than
        // replacing it with a blank: "no picture" is not a picture.
        self.show_face(cx);
        true
    }

    /// Where the playhead sits and what the button should say. Ignored
    /// while the user is dragging — their finger is the truth then.
    pub fn set_transport(&mut self, cx: &mut Cx, fraction: f64, playing: bool, position: &str) {
        if !self.scrubbing {
            self.fraction = fraction.clamp(0.0, 1.0);
        }
        self.playing = playing;
        self.view
            .label(cx, ids!(play_glyph))
            .set_text(cx, if playing { "■" } else { "▶" });
        self.view.label(cx, ids!(position)).set_text(cx, position);
        self.view.redraw(cx);
    }

    pub fn is_scrubbing(&self) -> bool {
        self.scrubbing
    }

    /// Move the playhead to where the finger is and tell the host.
    fn seek_to(&mut self, cx: &mut Cx, uid: WidgetUid, x: f64, area: Area) {
        let rect = area.rect(cx);
        if rect.size.x <= 1.0 {
            return;
        }
        self.fraction = ((x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0);
        cx.widget_action(uid, AudioAction::Seek(self.fraction));
        self.view.redraw(cx);
    }
}

impl Widget for AudioView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.view.draw_walk(cx, scope, walk).is_step() {}
        // The playhead is positioned after the wave area is known, so it
        // tracks a resize without the host doing anything.
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        // A decode running on the worker keeps the frame clock ticking only
        // until it lands — an idle well costs nothing.
        if matches!(event, Event::NextFrame(_)) {
            if !self.collect_clip(cx) && self.decoder.is_running() {
                cx.new_next_frame();
            }
        }
        let uid = self.widget_uid();
        // Press, drag, release anywhere on the picture: the playhead follows
        // the finger and the host is told where it landed. A click without a
        // drag is the same gesture with one sample.
        let wave_area = self.view.view(cx, ids!(wave)).area();
        match event.hits(cx, wave_area) {
            Hit::FingerDown(fe) => {
                self.scrubbing = true;
                self.seek_to(cx, uid, fe.abs.x, wave_area);
            }
            Hit::FingerMove(fe) if self.scrubbing => {
                self.seek_to(cx, uid, fe.abs.x, wave_area);
            }
            Hit::FingerUp(fe) => {
                if self.scrubbing {
                    self.seek_to(cx, uid, fe.abs.x, wave_area);
                }
                self.scrubbing = false;
            }
            _ => {}
        }
        let play_area = self.view.view(cx, ids!(play)).area();
        if let Hit::FingerDown(_) = event.hits(cx, play_area) {
            cx.widget_action(uid, AudioAction::TogglePlay);
        }
        // The face toggle is a view of the SAME clip, so it is handled here
        // rather than reported: nothing outside this widget changes.
        let face_area = self.view.view(cx, ids!(face_toggle)).area();
        if let Hit::FingerDown(_) = event.hits(cx, face_area) {
            let next = self.face.other();
            self.set_face(cx, next);
        }
    }
}

impl AudioViewRef {
    pub fn set_picture(&self, cx: &mut Cx, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_picture(cx, texture);
        }
    }

    pub fn set_clip(&self, cx: &mut Cx, bytes: Vec<u8>, format: ClipFormat) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_clip(cx, bytes, format);
        }
    }

    pub fn clear_clip(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_clip(cx);
        }
    }

    pub fn has_clip(&self) -> bool {
        self.borrow().map(|inner| inner.has_clip()).unwrap_or(false)
    }

    pub fn set_transport(&self, cx: &mut Cx, fraction: f64, playing: bool, position: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_transport(cx, fraction, playing, position);
        }
    }

    pub fn is_scrubbing(&self) -> bool {
        self.borrow().map(|inner| inner.scrubbing).unwrap_or(false)
    }

    /// What the user asked of the transport this pass.
    pub fn action(&self, actions: &Actions) -> AudioAction {
        match actions.find_widget_action(self.widget_uid()) {
            Some(action) => action.cast(),
            None => AudioAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam: the widget reports what the user wants, never does it. A
    /// host that ignores the action keeps playing exactly as before.
    #[test]
    fn actions_are_requests_not_effects() {
        assert_eq!(AudioAction::default(), AudioAction::None);
        assert_eq!(AudioAction::Seek(0.5), AudioAction::Seek(0.5));
        assert_ne!(AudioAction::Seek(0.5), AudioAction::Seek(0.25));
        assert_ne!(AudioAction::TogglePlay, AudioAction::None);
    }
}
