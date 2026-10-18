//! Export of the whole strip and the strip as data: tiles at most 4096 px wide, each a view of its
//! own drawn with [`Skyline::draw_view`] (raw HDR, `tone` off) into an offscreen RGBA16F/32F target
//! and read back (`Cx::read_render_texture`, asynchronous: the result arrives on a worker thread
//! after the frame), then written as PNG (toned, un-premultiplied 8-bit) or EXR (linear float32,
//! premultiplied), or handed over as a [`Band`] (the HDRI generator's skyline layer).
//!
//! A job runs on a `Skyline` of its own (the strip view holds one next to its view's), so the
//! views' targets and caches are never disturbed: one tile a frame, because a `Skyline` records
//! one picture into its scene target per frame.
//!
//! Owner after the foundation: stage D. The app calls [`Skyline::start_export`] from its buttons,
//! [`Skyline::draw_job`] while the strip view draws (and draws again the next frame while it says
//! so) and [`Skyline::job_event`] for every event, which reports progress and results for the
//! message line. Stubs.
use crate::params::Params;
use crate::render::Skyline;
use makepad_widgets::makepad_draw::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind { Png, Exr }

/// The strip as premultiplied linear HDR texels, `w` x `h`, top row first.
#[allow(dead_code)] // the HDRI layer's input
pub struct Band { pub w: usize, pub h: usize, pub texels: Vec<[f32; 4]> }

/// Receives a requested band once, on a worker thread.
#[allow(dead_code)] // the HDRI layer's input
pub type BandDone = Box<dyn FnOnce(Result<Band, String>) + Send>;

/// A running job: an export to a file or a band request (its tiles, the reads in flight, the
/// result's destination). Stage D fills it in.
pub struct Job {}

impl Skyline {
    /// Starts exporting the strip at parameters `p`, `width` px wide (height from the vertical
    /// field of view); the message is for the message line.
    pub fn start_export(&mut self, _cx: &mut Cx, kind: ExportKind, width: usize, _p: &Params) -> Result<String, String> {
        Err(format!("{kind:?} export at {width} px is not built yet"))
    }

    /// Asks for the strip at parameters `p`, `w` px wide, as a [`Band`]; `done` gets it when the
    /// last tile has been read back, or the reason it failed.
    #[allow(dead_code)] // the HDRI layer's input
    pub fn request_band(&mut self, _cx: &mut Cx, _p: &Params, w: usize, done: BandDone) {
        done(Err(format!("a band {w} px wide is not built yet")))
    }

    /// Draws the running job's next tile into its own pass (attached to the current one), inside a
    /// Draw event. True while more tiles follow: the caller draws again the next frame.
    pub fn draw_job(&mut self, _cx: &mut Cx2d) -> bool { self.job.is_some() }

    /// Progress and results for the message line, when there is news.
    pub fn job_event(&mut self, _cx: &mut Cx, _event: &Event) -> Option<String> { None }
}
