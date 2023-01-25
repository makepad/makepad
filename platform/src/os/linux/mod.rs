#[cfg(not(linux_kms))]
pub mod x11; 

#[cfg(linux_kms)]
pub mod kms;

pub mod gl_sys;
pub mod libc_sys;
pub mod alsa_sys;
pub mod linux_media;
pub mod opengl;
pub mod alsa_audio;
pub mod alsa_midi;
pub mod select_timer;

#[cfg(not(linux_kms))]
pub(crate) use self::x11::linux_x11::*;

#[cfg(linux_kms)]
pub(crate) use self::kms::linux_kms::*;

pub(crate) use self::opengl::*; 
pub(crate) use self::alsa_midi::{OsMidiOutput};

