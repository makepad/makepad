//! camera as a library: the camera roll (`roll`), stills from camera frames
//! (`still`) and the camera widget (`view`). The app (`src/main.rs`) is a
//! window around `CameraView{}`.

pub use makepad_widgets;

pub mod roll;
pub mod still;
pub mod view;

pub use view::CameraView;
