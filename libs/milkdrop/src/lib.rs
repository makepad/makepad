//! MilkDrop preset rendering for makepad, shared by the VJ and music apps.
//!
//! * [`parse`] reads `.milk` files into a [`Preset`].
//! * [`expr`] compiles the equations to a register VM; [`eel_gpu`] compiles
//!   the per-vertex and per-point blocks to shader code when they can run in
//!   parallel.
//! * [`hlsl`] translates the preset shaders (MilkDrop's HLSL dialect) to
//!   makepad shader source.
//! * [`state`] runs a frame's CPU work; [`shaders`] and [`render`] its GPU
//!   passes; [`view`] is the widget ([`MilkdropView`]).
//! * [`audio`] turns played samples into what the equations read; [`noise`]
//!   makes MilkDrop's noise textures.
//! * [`library`] indexes preset folders and downloads preset packs on
//!   demand; presets are never bundled. [`browser`] is the browsing widget
//!   every host shares ([`MilkdropBrowser`]), with its own library worker
//!   and one data folder for all apps ([`browser::data_dir`]).

pub use makepad_widgets;

pub mod audio;
pub mod browser;
pub mod eel_gpu;
pub mod expr;
pub mod hlsl;
pub mod library;
pub mod noise;
pub mod parse;
pub mod preset_tree;
pub mod render;
pub mod shaders;
pub mod state;
pub mod user_presets;
pub mod view;

pub use parse::Preset;
pub use browser::{MilkdropBrowser, MilkdropBrowserAction, MilkdropBrowserRef, MilkdropBrowserWidgetRefExt, PresetPick};
pub use preset_tree::BrowserItem;
pub use view::{MilkdropView, MilkdropViewRef, MilkdropViewWidgetRefExt};

/// Register the shaders and the widget (after `makepad_widgets`).
pub fn script_mod(vm: &mut makepad_widgets::ScriptVm) {
    shaders::script_mod(vm);
    view::script_mod(vm);
    browser::script_mod(vm);
}
