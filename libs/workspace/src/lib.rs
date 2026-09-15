//! makepad_workspace: the shell layer shared by the Makepad IDE apps
//! (director and scope): command line and on-disk state, the appearance
//! choice, UI-owned documents with a disk worker and the real CodeEditor
//! view over them, the camera-scaled canvas ground and card, the camera and
//! presentation seam zoomable presentations project through, and the input
//! remap through such a camera.
pub use makepad_widgets;
pub mod appearance;
pub mod camera;
pub use camera as workspace;
pub mod canvas_draw;
pub mod canvas_input;
pub mod document;
pub mod document_worker;
pub mod presentation;
pub mod state;
