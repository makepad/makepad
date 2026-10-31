//! The one per-engine tile fan-out of every Desk Link stream (a desktop's and an XR picture's):
//! a picture's tiles, each a stream of its own on one of the GPU's encode engines, each encoded
//! on a worker thread that owns its encoder, all reading the same encode surface where it lies.
//! [`TilePlan`] cuts a picture into tiles; [`TileEncoders`] runs them.
//!
//! A worker takes its tile's work in order: the rate when it changed, a key picture or a
//! refinement when asked, the push (which waits for the frame's GPU completion, reads the tile
//! and encodes it), then the surface's hold is dropped and the answer goes to the stream's
//! `done` (from the worker's thread: what the stream does with it, sends or gathers, is its own).

use makepad_video::{EncodedPacket, GpuCompletion, QpGrid, StreamTile, StreamTuning, SurfaceHandle, VideoStreamEncoder, VideoStreamEncoderOptions};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;

pub use crate::tile_plan::TilePlan;


/// One tile's stream: what its encoder is opened with.
pub struct TileSpec {
    pub options: VideoStreamEncoderOptions,
    pub tile: StreamTile,
    pub tuning: StreamTuning,
}

/// A tile of one picture for its worker.
pub struct TileWork<T> {
    pub surface: SurfaceHandle,
    /// The completion of the GPU work that drew the surface (the push waits for it).
    pub ready: GpuCompletion,
    /// Dropped once this tile's encode read the surface: with the last tile's, the picture's
    /// surface may be drawn over.
    pub held: Arc<dyn Send + Sync>,
    /// This tile's rate (kbit/s; 0: unchanged), set on the encoder when it changed.
    pub kbps: u32,
    pub key: bool,
    /// The last picture again at a near-lossless QP (a still desktop).
    pub refine: bool,
    pub pts_100ns: i64,
    /// The stream's own of this picture (handed back with the answer).
    pub tag: T,
}

/// A tile's answer.
pub struct TileDone<T> {
    pub tile: usize,
    pub tag: T,
    /// The work asked for a key picture.
    pub key: bool,
    /// The push into the encoder until its bitstream (the GPU wait, the read, the encode), and
    /// the encoder's own part of it (µs).
    pub pushed_ns: u64,
    pub encoded_ns: u64,
    pub encode_us: u32,
    pub packets: Result<Vec<EncodedPacket>, String>,
}

impl<T> TileDone<T> {
    pub fn push(&self) -> Duration {
        Duration::from_nanos(self.encoded_ns.saturating_sub(self.pushed_ns))
    }
}

enum Cmd<T> {
    Work(TileWork<T>),
    QpMap(Vec<i8>),
    MaxQp(u32),
}

struct Worker<T> {
    tx: Sender<Cmd<T>>,
    tile: StreamTile,
    size: [u32; 2],
    qp_grid: Option<QpGrid>,
    description: String,
}

/// A picture's tile streams, a worker thread each (dropping this ends them once their work is
/// done).
pub struct TileEncoders<T> {
    workers: Vec<Worker<T>>,
}

/// Where the workers' answers go (called on a worker's thread).
pub type TileSink<T> = Arc<dyn Fn(TileDone<T>) + Send + Sync>;

impl<T: Send + 'static> TileEncoders<T> {
    /// The tiles of `specs` (threads named `<name>-<tile>`), each answering into `done`.
    pub fn open(name: &str, specs: Vec<TileSpec>, done: TileSink<T>) -> Result<TileEncoders<T>, String> {
        let mut workers = Vec::with_capacity(specs.len());
        for (i, spec) in specs.into_iter().enumerate() {
            let size = [spec.options.width, spec.options.height];
            let mut applied = spec.options.bitrate_kbps;
            let mut encoder = VideoStreamEncoder::new_tuned(spec.options, spec.tile, spec.tuning).map_err(|e| format!("{e:?}"))?;
            let (qp_grid, description) = (encoder.qp_grid(), encoder.description());
            let (tx, rx) = channel::<Cmd<T>>();
            let done = done.clone();
            std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || {
                    while let Ok(cmd) = rx.recv() {
                        let w = match cmd {
                            Cmd::Work(w) => w,
                            Cmd::QpMap(map) => {
                                encoder.set_qp_map(&map);
                                continue;
                            }
                            Cmd::MaxQp(qp) => {
                                encoder.set_max_qp(qp);
                                continue;
                            }
                        };
                        if w.kbps != applied && w.kbps != 0 {
                            encoder.set_bitrate_kbps(w.kbps);
                            applied = w.kbps;
                        }
                        if w.key {
                            encoder.request_keyframe();
                        } else if w.refine {
                            encoder.request_refinement();
                        }
                        let pushed_ns = makepad_hal::clock_ns();
                        let packets = encoder.push_surface(&w.surface, &w.ready, w.pts_100ns).map_err(|e| format!("{e:?}"));
                        let encoded_ns = makepad_hal::clock_ns();
                        // the surface was read: it may be drawn over once every tile's was
                        drop(w.held);
                        let encode_us = encoder.timings().encode_us.min(((encoded_ns - pushed_ns) / 1000) as u32);
                        done(TileDone { tile: i, tag: w.tag, key: w.key, pushed_ns, encoded_ns, encode_us, packets });
                    }
                })
                .map_err(|e| e.to_string())?;
            workers.push(Worker { tx, tile: spec.tile, size, qp_grid, description });
        }
        Ok(TileEncoders { workers })
    }
}

impl<T> TileEncoders<T> {
    pub fn len(&self) -> usize {
        self.workers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    /// Tile `i`'s work (false: its worker ended).
    pub fn push(&self, i: usize, work: TileWork<T>) -> bool {
        self.workers[i].tx.send(Cmd::Work(work)).is_ok()
    }

    /// Tile `i`'s QP deltas on its encoder's [`Self::qp_grid`], from its next picture on.
    pub fn set_qp_map(&self, i: usize, deltas: Vec<i8>) {
        let _ = self.workers[i].tx.send(Cmd::QpMap(deltas));
    }

    /// A QP ceiling for every tile (0: none), from their next pictures on.
    pub fn set_max_qp(&self, qp: u32) {
        for w in &self.workers {
            let _ = w.tx.send(Cmd::MaxQp(qp));
        }
    }

    /// Tile `i`'s QP regions (None: its encoder takes no QP map).
    pub fn qp_grid(&self, i: usize) -> Option<QpGrid> {
        self.workers[i].qp_grid
    }

    /// Tile `i`'s place in the picture and its size (pixels).
    pub fn tile(&self, i: usize) -> (StreamTile, [u32; 2]) {
        (self.workers[i].tile, self.workers[i].size)
    }

    pub fn description(&self, i: usize) -> &str {
        &self.workers[i].description
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_8k_wide_picture_spreads_over_every_engine_in_columns_within_the_encoders_width() {
        let max = Some([4096, 4096]);
        assert_eq!(TilePlan::new([7680, 2160], 120, true, 4, max, false, false, 1), TilePlan { cols: 2, rows: 2 });
        assert_eq!(TilePlan::new([7680, 2160], 120, true, 3, max, false, false, 1), TilePlan { cols: 2, rows: 3 }, "3 x 2560x2160 is past the decoder's rate at 120");
        assert_eq!(TilePlan::new([7680, 2160], 60, true, 3, max, false, false, 1), TilePlan { cols: 3, rows: 1 });
        assert_eq!(TilePlan::new([7680, 2160], 120, true, 2, max, false, false, 1), TilePlan { cols: 2, rows: 2 });
        assert_eq!(TilePlan::new([7680, 2160], 120, false, 4, max, false, false, 1), TilePlan { cols: 2, rows: 1 });
        assert_eq!(TilePlan::new([7680, 2160], 120, false, 4, max, false, false, 1), TilePlan { cols: 2, rows: 1 });
        assert_eq!(TilePlan::new([3840, 2160], 120, true, 2, max, false, false, 1), TilePlan { cols: 1, rows: 2 });
        assert_eq!(TilePlan::new([3840, 2160], 120, true, 1, max, false, false, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([3840, 2160], 120, false, 2, max, false, false, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([1280, 100], 120, true, 4, max, false, false, 1), TilePlan { cols: 4, rows: 1 });
        // H.265 (8192 wide): one stream while a decoder keeps up, else bands; a band per engine
        let hevc = Some([8192, 8192]);
        assert_eq!(TilePlan::new([3840, 2160], 120, true, 3, hevc, true, false, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([7680, 2160], 60, true, 3, hevc, true, false, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([7680, 2160], 120, true, 3, hevc, true, false, 1), TilePlan { cols: 1, rows: 2 }, "8K wide at 120: two bands from the first picture");
        assert_eq!(TilePlan::new([7680, 2160], 120, true, 1, hevc, true, false, 1), TilePlan { cols: 1, rows: 2 }, "one engine bands too");
        assert_eq!(TilePlan::new([7680, 4320], 120, true, 3, hevc, true, false, 1), TilePlan { cols: 1, rows: 4 });
        assert_eq!(TilePlan::new([7680, 2160], 120, true, 3, hevc, true, true, 1), TilePlan { cols: 1, rows: 3 });
        assert_eq!(TilePlan::new([3840, 2160], 120, true, 3, hevc, true, false, 3), TilePlan { cols: 1, rows: 3 }, "a viewer's measured decode asked for 3");
        assert_eq!(TilePlan::new([7680, 4320], 120, false, 3, hevc, true, true, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([1280, 100], 120, true, 3, hevc, true, true, 1), TilePlan::WHOLE, "bands of at least 64 rows");
        assert_eq!(TilePlan::new([1280, 200], 120, true, 3, hevc, true, true, 1), TilePlan { cols: 1, rows: 3 });
    }
    #[test]
    fn tile_edges_are_whole_macroblocks_and_cover_the_picture_once() {
        assert_eq!(TilePlan::columns(7680, 2), vec![(0, 3840), (3840, 3840)]);
        assert_eq!(TilePlan::columns(7680, 3), vec![(0, 2560), (2560, 2560), (5120, 2560)]);
        assert_eq!(TilePlan::bands(2160, 2), vec![(0, 1072), (1072, 1088)]);
        assert_eq!(TilePlan::columns(1000, 1), vec![(0, 1000)]);
        // a viewer-sized desktop: the first column a 64 multiple, the last takes the rest
        assert_eq!(TilePlan::columns(6800, 2), vec![(0, 3392), (3392, 3408)]);
        assert_eq!(TilePlan::columns(5776, 3), vec![(0, 1920), (1920, 1920), (3840, 1936)]);
    }
}
