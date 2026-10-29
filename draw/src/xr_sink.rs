//! The XR frame sink: what an offscreen XR display's frames go to (a remote headset's stream
//! encoder, `makepad_xr_stream`), so an app only renders its XR pass as on any headset.
//!
//! An app marks its pass `DrawPass::set_as_xr_pass`; with an offscreen XR display
//! (`Cx::xr_offscreen`) the pass renders both eyes side by side into its own colour and depth
//! textures. Right after the app ends that pass ([`Cx2d::end_pass`]) the installed sink gets
//! them ([`XrFrameSink::presented`]) in the same draw, and packs them for its encoder. A frame
//! the app draws without content says so with [`xr_frame_empty`] (the sink asks for the next
//! pose instead of streaming a picture of nothing). The sink also hears every `Signal`
//! ([`XrFrameSink::signal`]): its own wakes (a picture submitted, room again).
//!
//! One sink per process ([`set_xr_frame_sink`]); none installed costs one global lookup per XR
//! pass end.

use crate::cx_2d::Cx2d;
use crate::makepad_platform::*;

/// The XR pass's attachments of the frame just drawn.
pub struct XrPresented<'a> {
    /// Both eyes side by side (premultiplied colour; the room shows where alpha is 0).
    pub color: &'a Texture,
    pub depth: &'a Texture,
}

pub trait XrFrameSink: 'static {
    /// The app ended its XR pass: the frame's colour and depth (drawn this frame).
    fn presented(&mut self, cx: &mut Cx2d, frame: &XrPresented);
    /// The app drew this XR frame without content (`why`, counted).
    fn empty(&mut self, cx: &mut Cx, why: &'static str);
    /// An `Event::Signal` (before the app's handler).
    fn signal(&mut self, cx: &mut Cx);
}

#[derive(Default)]
struct XrSinkSlot {
    sink: Option<Box<dyn XrFrameSink>>,
    hooked: bool,
}

/// Installs the process's XR frame sink (replacing one installed before).
pub fn set_xr_frame_sink(cx: &mut Cx, sink: Box<dyn XrFrameSink>) {
    let slot = cx.global::<XrSinkSlot>();
    slot.sink = Some(sink);
    if !std::mem::replace(&mut slot.hooked, true) {
        cx.signal_hooks.push(signal_hook);
    }
}

/// The app drew this XR frame without content (`why`): nothing is streamed for it, the sink
/// asks for the next pose. Call it instead of (or before) ending an XR pass with nothing in it.
pub fn xr_frame_empty(cx: &mut Cx, why: &'static str) {
    if !cx.xr_offscreen() {
        return;
    }
    with_sink(cx, |sink, cx| sink.empty(cx, why));
}

fn signal_hook(cx: &mut Cx) {
    with_sink(cx, |sink, cx| sink.signal(cx));
}

/// The sink taken out of its slot for the call (a call that reaches the slot again finds it
/// empty: no re-entry).
fn with_sink(cx: &mut Cx, f: impl FnOnce(&mut dyn XrFrameSink, &mut Cx)) {
    if !cx.has_global::<XrSinkSlot>() {
        return;
    }
    let Some(mut sink) = cx.global::<XrSinkSlot>().sink.take() else { return };
    f(sink.as_mut(), cx);
    let slot = cx.global::<XrSinkSlot>();
    if slot.sink.is_none() {
        slot.sink = Some(sink);
    }
}

impl<'a, 'b> Cx2d<'a, 'b> {
    /// Ends `pass` (`CxDraw::end_pass`); an XR pass of an offscreen XR display then goes to the
    /// XR frame sink.
    pub fn end_pass(&mut self, pass: &DrawPass) {
        self.cx.end_pass(pass);
        let id = pass.draw_pass_id();
        if !matches!(self.passes[id].parent, CxDrawPassParent::Xr) || !self.cx.cx.has_global::<XrSinkSlot>() || !self.xr_offscreen() {
            return;
        }
        let (Some(color), Some(depth)) = (self.passes[id].color_textures.first().map(|c| c.texture.clone()), self.passes[id].depth_texture.clone()) else { return };
        let Some(mut sink) = self.cx.cx.global::<XrSinkSlot>().sink.take() else { return };
        sink.presented(self, &XrPresented { color: &color, depth: &depth });
        let slot = self.cx.cx.global::<XrSinkSlot>();
        if slot.sink.is_none() {
            slot.sink = Some(sink);
        }
    }
}
