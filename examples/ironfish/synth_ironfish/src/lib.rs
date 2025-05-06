
pub mod ironfish;
pub mod waveguide;
pub mod delay_toys;
pub use makepad_audio_graph::makepad_platform;
use makepad_platform::Cx;
pub use makepad_audio_graph;

pub fn live_design(cx:&mut Cx){
    ironfish::live_design(cx);
}
