#[cfg(not(any(linux_direct, target_env="ohos", target_os="android")))]
pub mod x11; 

#[cfg(linux_direct)]
pub mod direct;

#[cfg(target_os="android")]
pub mod openxr_sys;
#[cfg(target_os="android")]
pub mod openxr;
#[cfg(target_os="android")]
pub mod openxr_anchor;
#[cfg(target_os="android")]
pub mod openxr_input;

#[cfg(target_env="ohos")]
pub mod open_harmony;

pub mod egl_sys;
#[macro_use]
pub mod gl_sys;
pub mod libc_sys;
pub mod opengl;
pub mod module_loader;

#[cfg(not(any(target_env="ohos", target_os="android")))]
pub mod dma_buf;
#[cfg(not(any(target_env="ohos", target_os="android")))]
pub mod ipc;

#[cfg(not(any(target_env="ohos", target_os="android")))]
pub mod alsa_sys;
#[cfg(not(any(target_env="ohos", target_os="android")))]
pub mod linux_media;
#[cfg(not(any(target_env="ohos", target_os="android")))]
pub mod alsa_audio;
#[cfg(not(any(target_env="ohos", target_os="android")))]
pub mod alsa_midi;

#[cfg(not(target_os="android"))]
pub mod select_timer;

#[cfg(not(any(target_env="ohos", target_os="android")))] 
pub mod pulse_audio; 
#[cfg(not(any(target_env="ohos", target_os="android")))]
pub mod pulse_sys;

#[cfg(not(target_os="android"))]
mod web_socket;

#[cfg(target_os="android")]
pub mod android;

#[cfg(target_os="android")]
pub(crate) use self::android::android::CxOs;

#[cfg(not(any(linux_direct, target_os="android", target_env="ohos")))]
pub(crate) use self::x11::linux_x11::*;

#[cfg(target_env="ohos")]
pub(crate) use self::open_harmony::open_harmony::*;

#[cfg(linux_direct)]
pub(crate) use self::direct::linux_direct::*;

pub(crate) use self::opengl::*;

#[cfg(not(any(target_os="android", target_env="ohos")))]
pub(crate) use self::alsa_midi::{OsMidiInput, OsMidiOutput};

#[cfg(target_os="android")]
pub(crate) use self::android::android_midi::{OsMidiInput, OsMidiOutput};

//#[cfg(target_env="ohos")]
//pub(crate) use self::open_harmony::oh_media::{OsMidiInput, OsMidiOutput};

#[cfg(not(target_os="android"))]
pub (crate) use web_socket::OsWebSocket;

#[cfg(target_os="android")]
pub (crate) use self::android::android_web_socket::OsWebSocket;
