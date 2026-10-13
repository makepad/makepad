//! The Windows backend: the Win32 shell (`win32`: windows, input, the message loop on the main
//! thread) and the D3D11 GPU backend (`d3d11`). Replaces platform/src/os/windows (the old Cx
//! backend) behind the HAL boundary; see local plan WINDOWS_AND_DELETE.md.

pub mod d3d11;
pub mod media;
pub mod win32;

pub use d3d11::D3d11;
