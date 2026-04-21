pub mod implement_com;
#[macro_use]
pub mod win32_app;
pub mod dataobject;
pub mod dropfiles;
pub mod dropsource;
pub mod droptarget;
pub mod enumformatetc;
pub mod media_foundation;
pub mod wasapi;
pub mod win32_event;
pub mod win32_window;
pub mod windows_media;
pub mod windows_video_playback;
pub mod windows_video_player;
pub mod winrt_midi;

//pub mod com_sys;
pub mod angle;
pub mod d3d11;
pub mod windows;
pub mod windows_game_input;
pub mod windows_stdin;

pub(crate) use crate::os::windows::d3d11::*;
pub(crate) use crate::os::windows::windows::*;
pub(crate) use crate::os::windows::winrt_midi::{OsMidiInput, OsMidiOutput};
