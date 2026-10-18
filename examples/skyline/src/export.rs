//! Export of the whole strip: tiles at most 4096 px wide, each a view of its own drawn with the
//! raw HDR final into an RGBA16F target and read back as half floats (`Cx::read_render_texture`),
//! then written as PNG (toned, un-premultiplied 8-bit) or EXR (linear float32, premultiplied); and
//! [`band`], the strip as premultiplied linear HDR texels for the HDRI generator's skyline layer.
//!
//! Owner after the foundation: stage D. The app calls [`Export::start`] from its buttons,
//! [`Export::draw`] while the strip view draws (an export renders inside a Draw event) and
//! [`Export::handle_event`] for every event, which reports progress for the status line. Stubs.
use crate::params::Params;
use makepad_widgets::makepad_draw::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Png,
    Exr,
}

/// The export in progress, if any.
#[derive(Default)]
pub struct Export {}

impl Export {
    /// Starts exporting the strip `width` px wide (height from the strip's vertical field of view).
    pub fn start(&mut self, _cx: &mut Cx, kind: ExportKind, width: usize, _p: &Params) -> Result<String, String> {
        Err(format!("{kind:?} export at {width} px is not built yet"))
    }

    /// Records the export's offscreen passes while a view draws.
    pub fn draw(&mut self, _cx: &mut Cx2d, _p: &Params) {}

    /// Progress for the status line, when there is news.
    pub fn handle_event(&mut self, _cx: &mut Cx, _event: &Event) -> Option<String> {
        None
    }
}

/// The strip `w` px wide as premultiplied linear HDR texels, top row first (stage D).
#[allow(dead_code)]
pub fn band(_p: &Params, _w: usize) -> Option<(usize, usize, Vec<[f32; 4]>)> {
    None
}
