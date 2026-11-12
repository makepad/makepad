//! Remote rendering for headsets over Desk Link XR (`makepad_desk_link::xr`): the serving side.
//!
//! [`XrServer`] (`server`): an app serves its XR view (a headset linking gives the process a
//! remote headset slot): it renders as on a headset (its XR pass, paced by the headset's poses);
//! the server takes each frame from the Cx's XR frame sink, packs it on the GPU, encodes and sends
//! it. The caption's [`XrCaptionButton`] installs it; [`XrMirrorView`] (or
//! [`DrawXrMirror::draw_headset`]) mirrors it on the desktop. The headset end is the companion
//! app, which speaks the same protocol: its poses, controllers, hands, cameras and microphone come
//! up the link, the pictures go down.

use makepad_widgets::*;

mod auto;
mod desktop;
mod mirror;
mod panel;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
mod settings_types;
pub mod verdict;
pub use mirror::{DrawXrMirror, XrMirrorClock, XrMirrorView};
pub use desktop::{XrDesktopCache, XrDesktopFreeze};
pub use panel::{XrCaptionButton, XrStreamSettings};
#[cfg(not(target_arch = "wasm32"))]
pub use makepad_desk_link::xr::mic_state;
#[cfg(not(target_arch = "wasm32"))]
pub use server::{launch_enabled, HostLoad, LiveHeadset, MicEvent, MicSink, ServerStats, XrServer};
pub use settings_types::{HeadsetSettings, StreamAa, StreamView, STREAM_AA_DEFAULT, STREAM_AA_MODES};
/// The serving side's shaders and widgets: the server's pack, the mirror and the caption's
/// `XrCaptionButton` (an app with the control registers them).
pub fn serve_script_mod(vm: &mut ScriptVm) -> ScriptValue {
    #[cfg(not(target_arch = "wasm32"))]
    server::script_mod(vm);
    mirror::shader::script_mod(vm);
    mirror::script_mod(vm);
    panel::shader::script_mod(vm);
    panel::script_mod(vm)
}
