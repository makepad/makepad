//! On-device text detection, boxes only: where the lines of text are in a
//! picture, as fractions of the picture, for the camera translator to place
//! each translation over its line (and to crop each line for the reader).
//!
//! The model is a small DB-style detector (convolutional backbone, feature
//! pyramid, probability head) held as data ([`ir::Graph`]): our converter
//! writes it, the Vulkan executor runs it on the phone and the CPU
//! executor is the reference (and the fallback without a GPU).
//!
//! ```ignore
//! let mut det = TextDetector::load(path)?;
//! let lines = det.detect(&Frame { width, height, rgba: &pixels })?;
//! ```

pub mod cpu;
pub mod file;
pub mod ir;
pub mod paddle;
pub mod post;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod vulkan;

pub use post::LineBox;

use std::path::Path;
use std::time::Instant;

/// A picture: RGBA8, row-major, `width * height * 4` bytes.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

/// Where the time of the last [`TextDetector::detect`] went, in milliseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    pub prepare_ms: f32,
    pub network_ms: f32,
    pub boxes_ms: f32,
    /// Network input size.
    pub input: (usize, usize),
}

enum Exec {
    Cpu,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    Vulkan(vulkan::Detector),
}

pub struct TextDetector {
    graph: ir::Graph,
    exec: Exec,
    last: Timings,
}

impl TextDetector {
    /// Loads a detector file and prepares the GPU executor when a Vulkan
    /// device is there (else the CPU reference runs).
    pub fn load(path: &Path) -> Result<TextDetector, String> {
        Self::from_graph(file::load(path)?)
    }

    pub fn from_graph(graph: ir::Graph) -> Result<TextDetector, String> {
        graph.validate()?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if std::env::var("MAKEPAD_TEXTDET_CPU").is_err() {
            match vulkan::Detector::new(&graph) {
                Ok(gpu) => return Ok(TextDetector { graph, exec: Exec::Vulkan(gpu), last: Timings::default() }),
                Err(why) => eprintln!("textdet: no Vulkan executor ({why}); using the CPU"),
            }
        }
        Ok(TextDetector { graph, exec: Exec::Cpu, last: Timings::default() })
    }

    /// The CPU reference only (tests, comparisons).
    pub fn cpu(graph: ir::Graph) -> Result<TextDetector, String> {
        graph.validate()?;
        Ok(TextDetector { graph, exec: Exec::Cpu, last: Timings::default() })
    }

    pub fn backend(&self) -> &'static str {
        match self.exec {
            Exec::Cpu => "cpu",
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Exec::Vulkan(_) => "vulkan",
        }
    }

    /// Bytes retained by GPU weights, arena and staging buffers (zero on CPU).
    pub fn gpu_bytes(&self) -> u64 {
        match &self.exec {
            Exec::Cpu => 0,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Exec::Vulkan(gpu) => gpu.gpu_bytes(),
        }
    }

    pub fn graph(&self) -> &ir::Graph {
        &self.graph
    }

    /// The probability map `[1, h, w]` for a prepared input.
    pub fn probability(&mut self, input: &cpu::Map) -> Result<cpu::Map, String> {
        match &mut self.exec {
            Exec::Cpu => cpu::run(&self.graph, input.clone()),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Exec::Vulkan(gpu) => gpu.run(input),
        }
    }

    /// Text-line boxes in reading order, as fractions of the picture.
    /// Blocking; call it off the UI thread.
    pub fn detect(&mut self, frame: &Frame) -> Result<Vec<LineBox>, String> {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let t0 = Instant::now();
        let prep = post::prepare(w, h, frame.rgba, &self.graph.pre)?;
        let t1 = Instant::now();
        let prob = self.probability(&prep.input)?;
        let t2 = Instant::now();
        if (prob.h, prob.w) != (prep.input.h, prep.input.w) {
            return Err(format!("probability map {}x{} for input {}x{}", prob.w, prob.h, prep.input.w, prep.input.h));
        }
        let boxes = post::boxes(&prob, (prep.scale_x, prep.scale_y), (w, h), &self.graph.post);
        let t3 = Instant::now();
        let ms = |a: Instant, b: Instant| (b - a).as_secs_f32() * 1000.0;
        self.last = Timings { prepare_ms: ms(t0, t1), network_ms: ms(t1, t2), boxes_ms: ms(t2, t3), input: (prep.input.w, prep.input.h) };
        Ok(boxes)
    }

    pub fn last_timings(&self) -> Timings {
        self.last
    }
}

#[cfg(test)]
mod tests;
