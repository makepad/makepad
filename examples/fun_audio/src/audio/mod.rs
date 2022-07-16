use crate::makepad_platform::Cx;

pub mod iron_fish;
pub mod audio_graph;
pub mod basic_synth;
pub mod instrument;
pub mod mixer;
#[macro_use]
mod audio_component;

#[cfg(target_os = "macos")]
pub mod audio_unit_effect;
#[cfg(target_os = "macos")]
pub mod audio_unit_instrument;

pub use audio_graph::*;
pub use audio_component::*;
pub use audio_component_factory;

pub fn live_register(cx:&mut Cx){
    #[cfg(target_os = "macos")]
    crate::audio::audio_unit_instrument::live_register(cx);
    #[cfg(target_os = "macos")]
    crate::audio::audio_unit_effect::live_register(cx);
    
    crate::audio::basic_synth::live_register(cx);
    crate::audio::iron_fish::live_register(cx);
    crate::audio::instrument::live_register(cx);
    crate::audio::mixer::live_register(cx);
    crate::audio::audio_graph::live_register(cx);
}
