//#[macro_use]
//pub mod implement_com;
#[macro_use]
pub mod win32_app;
pub mod dataobject;
pub mod dropfiles;
pub mod dropsource;
pub mod droptarget;
pub mod enumformatetc;
pub mod http;
pub mod media_foundation;
pub mod wasapi;
pub mod win32_event;
pub mod win32_window;
pub mod windows_media;
pub mod winrt_midi;

//pub mod com_sys;
pub mod d3d11;
mod web_socket;
pub mod windows;
pub mod windows_game_input;
pub mod windows_stdin;

pub(crate) use crate::os::windows::d3d11::*;
pub(crate) use crate::os::windows::windows::*;
pub(crate) use crate::os::windows::winrt_midi::{OsMidiInput, OsMidiOutput};
pub(crate) use web_socket::OsWebSocket;
