//! The encode's foveation: fewer pixels towards each eye's edges (`makepad_desk_link::xr::Fovea`),
//! the eye's colour and band warped into a smaller column before the encode (`sink`) and back on
//! the headset. Encode and decode time follow the pixels.

use makepad_desk_link::xr::Fovea;

/// The stream's foveation: `MAKEPAD_XR_REMOTE_FOVEATE=0` none (every eye pixel encoded, for
/// comparisons), else [`Fovea::DEFAULT`].
pub(crate) fn fovea_setting() -> Fovea {
    if std::env::var("MAKEPAD_XR_REMOTE_FOVEATE").as_deref() == Ok("0") { Fovea::OFF } else { Fovea::DEFAULT }
}
