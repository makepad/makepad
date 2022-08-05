use crate::makepad_platform::Cx;

pub mod audio_graph;
pub mod test_synth;
pub mod instrument;
pub mod mixer;
#[macro_use]
mod audio_traits;

#[cfg(target_os = "macos")]
pub mod audio_unit_effect;
#[cfg(target_os = "macos")]
pub mod audio_unit_instrument;

pub use audio_graph::*;
pub use audio_traits::*;
pub use audio_component;

pub fn live_register(cx:&mut Cx){
    #[cfg(target_os = "macos")]
    crate::audio_graph::audio_unit_instrument::live_register(cx);
    #[cfg(target_os = "macos")]
    crate::audio_graph::audio_unit_effect::live_register(cx);
    
    crate::audio_graph::test_synth::live_register(cx);
    crate::audio_graph::instrument::live_register(cx);
    crate::audio_graph::mixer::live_register(cx);
    crate::audio_graph::audio_graph::live_register(cx);
}
