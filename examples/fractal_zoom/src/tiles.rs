//! Tiled parallel rendering: a worker pool that renders tiles, and layers of
//! tiles in fractal space.
//!
//! The design follows the original fractal zoomer: a layer is a set of
//! square tiles of one fractal size, queued in a spiral out from the point
//! of interest (the cursor while zooming), so the part you look at sharpens
//! first. Older layers stay on screen under a new one, and the GPU scales
//! them to the current view, so zoom and pan stay smooth while the workers
//! catch up; once a layer is complete it covers the view and the older ones
//! are dropped. Each layer is a refinement step: coarse texels while the
//! view moves, full resolution when it rests, then 2x2 supersampling.

use crate::deep::DeepRef;
use crate::kernels::{render_tile, Params, Precision, TileRect};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// A tile's edge in device pixels when its layer matches the view.
pub const TILE_PX: f64 = 256.0;

pub struct Job {
    pub layer: u64,
    pub index: usize,
    /// The view origin: `rect` is an offset from it (absolute x = origin.0 + rect.x).
    pub origin: (f64, f64),
    /// Set for a deep (perturbation) layer: only the local f64 path renders it.
    pub deep: Option<Arc<DeepRef>>,
    pub rect: TileRect,
    pub res: usize,
    pub precision: Precision,
    pub params: Params,
    /// A hedged twin of a tile a node holds: only the local workers take it.
    pub local_only: bool,
    pub buffer: Vec<u32>,
}

/// A finished tile. Every job taken off the queue comes back as exactly one
/// `Done`: `res == 0` marks one abandoned unrendered (its buffer still
/// comes home).
pub struct Done {
    pub layer: u64,
    pub index: usize,
    pub res: usize,
    pub buffer: Vec<u32>,
    pub micros: u64,
    /// Who rendered it: 0 is this machine, others are remote nodes.
    pub node: u16,
    /// The worker on that node.
    pub worker: u16,
    /// When the result was ready here (a remote one: when it arrived).
    pub at: Instant,
}

struct Shared {
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    quit: AtomicBool,
    /// Jobs of layers below this are stale: whoever holds one drops it.
    min_layer: AtomicU64,
}

/// The tile queue as other job runners (remote nodes) see it: they pop jobs
/// in the same spiral order as the local workers and answer on the same channel.
#[derive(Clone)]
pub struct QueueHandle {
    shared: Arc<Shared>,
    done: Sender<Done>,
}

impl QueueHandle {
    /// The last job (the far end of the spiral) this runner can do, e.g. a
    /// remote kernel that has no Newton or caps the iteration count. Nodes
    /// answer later than the local workers, so they take the tiles the
    /// local workers would reach last.
    pub fn pop_back_where(&self, can: impl Fn(&Job) -> bool) -> Option<Job> {
        let mut q = self.shared.queue.lock().unwrap();
        let at = q.iter().rposition(|j| can(j))?;
        q.remove(at)
    }

    pub fn min_layer(&self) -> u64 {
        self.shared.min_layer.load(Ordering::SeqCst)
    }

    pub fn is_quitting(&self) -> bool {
        self.shared.quit.load(Ordering::SeqCst)
    }

    pub fn done_sender(&self) -> Sender<Done> {
        self.done.clone()
    }
}

/// Worker threads pulling tile jobs off one queue.
pub struct TilePool {
    shared: Arc<Shared>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl TilePool {
    pub fn new(threads: usize) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            quit: AtomicBool::new(false),
            min_layer: AtomicU64::new(0),
        });
        let (done_tx, done_rx) = channel::<Done>();
        let threads = (0..threads.max(1))
            .map(|i| {
                let shared = shared.clone();
                let done_tx = done_tx.clone();
                std::thread::Builder::new()
                    .name(format!("fractal-tile-{i}"))
                    .spawn(move || worker(shared, done_tx, i as u16))
                    .expect("spawn tile worker")
            })
            .collect();
        TilePool { shared, done_tx, done_rx, threads }
    }

    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    pub fn queue_handle(&self) -> QueueHandle {
        QueueHandle { shared: self.shared.clone(), done: self.done_tx.clone() }
    }

    pub fn push(&self, jobs: impl IntoIterator<Item = Job>) {
        let mut q = self.shared.queue.lock().unwrap();
        q.extend(jobs);
        drop(q);
        self.shared.wake.notify_all();
    }

    /// Makes every layer below `layer` stale and takes back its jobs not
    /// started yet (their buffers come home); jobs in flight still answer.
    pub fn cancel_below(&self, layer: u64) -> Vec<Job> {
        self.shared.min_layer.store(layer, Ordering::SeqCst);
        let mut q = self.shared.queue.lock().unwrap();
        let (stale, keep): (Vec<Job>, Vec<Job>) = q.drain(..).partition(|j| j.layer < layer);
        q.extend(keep);
        stale
    }

    pub fn queued(&self) -> usize {
        self.shared.queue.lock().unwrap().len()
    }

    pub fn try_done(&self) -> Option<Done> {
        self.done_rx.try_recv().ok()
    }
}

impl Drop for TilePool {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn worker(shared: Arc<Shared>, done: Sender<Done>, worker: u16) {
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if shared.quit.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(job) = q.pop_front() {
                    break job;
                }
                q = shared.wake.wait(q).unwrap();
            }
        };
        let Job { layer, index, origin, deep, rect, res, precision, params, mut buffer, .. } = job;
        let start = Instant::now();
        buffer.resize(res * res, 0);
        // a layer the view has left (a jump, a newer layer) frees the worker mid-tile
        let stale = || layer < shared.min_layer.load(Ordering::Relaxed) || shared.quit.load(Ordering::Relaxed);
        let finished = render_tile(&params, origin, rect, res, precision, deep.as_deref(), &mut buffer, &stale);
        let micros = start.elapsed().as_micros() as u64;
        // res 0: not rendered (as a node's dropped tile)
        let res = if finished { res } else { 0 };
        if done.send(Done { layer, index, res, buffer, micros, node: 0, worker, at: Instant::now() }).is_err() {
            return;
        }
    }
}

/// The square tiles covering `view` (x0, y0, x1, y1 in fractal space) with
/// tiles of edge `size`, on a grid through `focus`, nearest the focus first:
/// the spiral walk of the original, as a sort by ring distance.
pub fn spiral_tiles(focus: (f64, f64), size: f64, view: (f64, f64, f64, f64)) -> Vec<TileRect> {
    let ox = focus.0 - 0.5 * size;
    let oy = focus.1 - 0.5 * size;
    let i0 = ((view.0 - ox) / size).floor() as i64 - 1;
    let i1 = ((view.2 - ox) / size).ceil() as i64;
    let j0 = ((view.1 - oy) / size).floor() as i64 - 1;
    let j1 = ((view.3 - oy) / size).ceil() as i64;
    let mut cells = Vec::new();
    for j in j0..=j1 {
        for i in i0..=i1 {
            let x = ox + i as f64 * size;
            let y = oy + j as f64 * size;
            if x + size < view.0 || y + size < view.1 || x > view.2 || y > view.3 {
                continue;
            }
            cells.push((i, j));
        }
    }
    // ring (Chebyshev distance) first, then the angle around the ring, as the walk did
    cells.sort_by(|a, b| {
        let ra = a.0.abs().max(a.1.abs());
        let rb = b.0.abs().max(b.1.abs());
        ra.cmp(&rb).then_with(|| {
            let aa = (a.1 as f64).atan2(a.0 as f64);
            let ab = (b.1 as f64).atan2(b.0 as f64);
            aa.partial_cmp(&ab).unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    cells.into_iter().map(|(i, j)| TileRect { x: ox + i as f64 * size, y: oy + j as f64 * size, size }).collect()
}
