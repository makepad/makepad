pub use makepad_widgets;
use makepad_widgets::app_main;
use makepad_widgets::*;

#[cfg(target_os = "android")]
mod client;
#[cfg(not(target_os = "android"))]
mod gpu_capture;
#[cfg(not(target_os = "android"))]
mod host_scene;
mod protocol;
mod scene;
mod wire;

#[cfg(not(target_os = "android"))]
mod host;

#[cfg(target_os = "android")]
pub use client::App;

#[cfg(not(target_os = "android"))]
pub use host::App;

app_main!(App);
