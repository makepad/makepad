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

pub fn live_design(cx:&mut Cx){
    #[cfg(target_os = "macos")]
    crate::audio_graph::audio_unit_instrument::live_design(cx);
    #[cfg(target_os = "macos")]
    crate::audio_graph::audio_unit_effect::live_design(cx);
    
    crate::audio_graph::test_synth::live_design(cx);
    crate::audio_graph::instrument::live_design(cx);
    crate::audio_graph::mixer::live_design(cx);
    crate::audio_graph::audio_graph::live_design(cx);
}
