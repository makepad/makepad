#[cfg(not(any(linux_direct, target_os="android")))]
pub mod x11; 

#[cfg(linux_direct)]
pub mod direct;

pub mod gl_sys;
pub mod libc_sys;
pub mod opengl;

#[cfg(not(target_os="android"))]
pub mod alsa_sys;
#[cfg(not(target_os="android"))]
pub mod linux_media;
#[cfg(not(target_os="android"))]
pub mod alsa_audio;
#[cfg(not(target_os="android"))]
pub mod alsa_midi;
#[cfg(not(target_os="android"))]
pub mod select_timer;
#[cfg(not(target_os="android"))] 
pub mod pulse_audio; 
#[cfg(not(target_os="android"))]
pub mod pulse_sys;

#[cfg(target_os="android")]
pub mod android;

#[cfg(target_os="android")]
pub(crate) use self::android::android::CxOs;

#[cfg(not(any(linux_direct, target_os="android")))]
pub(crate) use self::x11::linux_x11::*;


#[cfg(linux_direct)]
pub(crate) use self::direct::linux_direct::*;

pub(crate) use self::opengl::*; 

#[cfg(not(target_os="android"))]
pub(crate) use self::alsa_midi::{OsMidiInput, OsMidiOutput};

#[cfg(target_os="android")]
pub(crate) use self::android::amidi::{OsMidiInput, OsMidiOutput};

