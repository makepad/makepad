//! The rasterising workers of the font service: a few long-lived threads that take glyph jobs
//! from a queue and return pixels (or curves) through a mailbox. They never touch the HAL's
//! state; the service applies what they return. A worker keeps the fonts it was given as its
//! own `Font` objects (they are not `Send`), built from the host's mapping on first use.

use makepad_font_core::{
    font::{Font, FontId, GlyphId},
    font_atlas::GlyphImageKey,
    font_face::FontFace,
    geom::{Point, Rect, Size},
    glyph_outline::GlyphOutline,
    image::{Bgra, Image},
    msdfer::Msdfer,
    raster::{plan_color, plan_outline_msdf, plan_outline_sdf, render_msdf, OutlineRasterizationMode, Settings},
    sdfer::Sdfer,
    slug::slug_curves,
    MappedBytes, SharedBytes,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::Instant;

/// A font the host opened: its bytes (mapped once per process, shared by every session and every
/// worker) and how to read them.
pub struct HostFont {
    pub id: u32,
    pub source: HostSource,
    pub index: u32,
    pub vars: Vec<(u32, f32)>,
}

pub enum HostSource {
    Mapped(Arc<MappedBytes>),
    Inline(Arc<Vec<u8>>),
}

/// What a request is, for the whole HAL: the same request from two sessions is one job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReqKey {
    pub font: u32,
    pub glyph: u16,
    pub dpxs_bits: u32,
    pub kind: u32,
}

pub enum JobKind {
    /// Rasterise a requested glyph.
    Glyph { req: ReqKey },
    /// Upgrade a plain distance field to a multi-channel one.
    Msdf { key: GlyphImageKey, outline: GlyphOutline, dpxs: f32, sdf_slot: u32 },
}

pub struct Job {
    pub kind: JobKind,
    pub font: Arc<HostFont>,
    pub glyph: u16,
    pub dpxs: f32,
}

/// What came of a glyph's image: rendered by this worker, already in the atlas when the worker
/// looked, or being rendered by another worker right now (the service waits for that one).
pub enum Work<T> {
    Rendered(T),
    InAtlas,
    Elsewhere,
}

/// The right to render one atlas entry: one worker holds it until the service has placed the
/// result, so two requests that need the same image render it once.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Claim {
    Raster(GlyphImageKey),
    Slug(u32, u16),
}

/// What a worker found out about a glyph.
pub enum Payload {
    Sdf {
        key: GlyphImageKey,
        pixels: Work<Vec<u8>>,
        padding: usize,
        origin: Point<f32>,
        dpxs: f32,
        msdf: Option<(GlyphImageKey, GlyphOutline, f32)>,
    },
    Color {
        key: GlyphImageKey,
        pixels: Work<Vec<u32>>,
        padding: usize,
        origin: Point<f32>,
        dpxs: f32,
    },
    Slug {
        origin_in_ems: Point<f32>,
        size_in_ems: Size<f32>,
        curves: Work<Vec<f32>>,
    },
    Msdf {
        key: GlyphImageKey,
        pixels: Vec<u32>,
    },
    /// nothing to draw (no outline, no bitmap, or the font cannot be read)
    Empty,
}

pub struct Done {
    pub job: JobKind,
    pub payload: Payload,
    /// the claim this job held, to release once its result is placed
    pub claim: Option<Claim>,
    pub ns: u64,
    /// no render was needed or done here
    pub cached: bool,
}

/// The keys the atlases hold, readable by the workers (so a glyph already in the atlas is not
/// rendered again).
#[derive(Default)]
pub struct Known {
    pub raster: HashSet<GlyphImageKey>,
    pub slug: HashSet<(u32, u16)>,
    claimed: HashSet<Claim>,
}

impl Known {
    /// Looks the entry up and, when nobody has it or is making it, takes the right to make it.
    fn claim(&mut self, claim: Claim) -> ClaimResult {
        let present = match &claim {
            Claim::Raster(k) => self.raster.contains(k),
            Claim::Slug(f, g) => self.slug.contains(&(*f, *g)),
        };
        if present {
            ClaimResult::InAtlas
        } else if !self.claimed.insert(claim) {
            ClaimResult::Elsewhere
        } else {
            ClaimResult::Mine
        }
    }

    pub fn release(&mut self, claim: &Claim) {
        self.claimed.remove(claim);
    }

    pub fn is_claimed(&self, claim: &Claim) -> bool {
        self.claimed.contains(claim)
    }
}

enum ClaimResult {
    InAtlas,
    Elsewhere,
    Mine,
}

struct Shared {
    queue: Mutex<VecDeque<Job>>,
    cv: Condvar,
    done: Mutex<Vec<Done>>,
    done_count: AtomicUsize,
    in_flight: AtomicUsize,
    shutdown: AtomicBool,
    known: Arc<RwLock<Known>>,
    settings: Settings,
    notify: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    busy_ns: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Pool {
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Pool {
    pub fn new(settings: Settings, known: Arc<RwLock<Known>>, notify: Option<Arc<dyn Fn() + Send + Sync>>) -> Pool {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            done: Mutex::new(Vec::new()),
            done_count: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            known,
            settings,
            notify: Mutex::new(notify),
            busy_ns: AtomicU64::new(0),
        });
        let count = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 4));
        let threads = (0..count)
            .filter_map(|i| {
                let shared = shared.clone();
                std::thread::Builder::new().name(format!("hal-font-{i}")).spawn(move || worker(shared)).ok()
            })
            .collect();
        Pool { shared, threads }
    }

    pub fn submit(&self, job: Job) {
        self.shared.in_flight.fetch_add(1, Ordering::AcqRel);
        lock(&self.shared.queue).push_back(job);
        self.shared.cv.notify_one();
    }

    /// Jobs queued or running (results not taken yet included).
    pub fn in_flight(&self) -> usize {
        self.shared.in_flight.load(Ordering::Acquire)
    }

    /// Whether a worker has finished something not taken yet.
    pub fn has_done(&self) -> bool {
        self.shared.done_count.load(Ordering::Acquire) > 0
    }

    pub fn take_done(&self) -> Vec<Done> {
        let mut done = lock(&self.shared.done);
        self.shared.done_count.store(0, Ordering::Release);
        let taken = std::mem::take(&mut *done);
        self.shared.in_flight.fetch_sub(taken.len(), Ordering::AcqRel);
        taken
    }

    /// CPU time the workers spent rendering, ns.
    pub fn busy_ns(&self) -> u64 {
        self.shared.busy_ns.load(Ordering::Relaxed)
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

struct WorkerState {
    settings: Settings,
    sdfer: Sdfer,
    msdfer: Msdfer,
    fonts: HashMap<u32, Option<Font>>,
}

/// The worker's own `Font` for a host font, built from the shared mapping on first use.
fn font_for<'a>(fonts: &'a mut HashMap<u32, Option<Font>>, host: &HostFont) -> Option<&'a Font> {
    fonts
        .entry(host.id)
        .or_insert_with(|| {
            let data = match &host.source {
                HostSource::Mapped(m) => SharedBytes::from_mapped(m.clone()),
                HostSource::Inline(v) => SharedBytes::from_vec(v.as_slice().to_vec()),
            };
            let mut face = FontFace::from_data_and_index(data, host.index)?;
            if !host.vars.is_empty() {
                face.set_variations(&host.vars);
            }
            Some(Font::new(FontId::from(host.id as u64), face, 0.0, 0.0))
        })
        .as_ref()
}

fn worker(shared: Arc<Shared>) {
    let settings = shared.settings;
    let mut state = WorkerState { settings, sdfer: Sdfer::new(settings.sdfer), msdfer: Msdfer::new(settings.msdfer), fonts: HashMap::new() };
    loop {
        let job = {
            let mut q = lock(&shared.queue);
            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    return;
                }
                if let Some(job) = q.pop_front() {
                    break job;
                }
                q = shared.cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let started = Instant::now();
        let glyph = job.glyph;
        let Job { kind, font, dpxs, .. } = job;
        let mut claim = None;
        let outcome = catch_unwind(AssertUnwindSafe(|| run(&mut state, &shared.known, &font, glyph, dpxs, &kind, &mut claim)));
        let (payload, cached) = outcome.unwrap_or((Payload::Empty, false));
        let ns = started.elapsed().as_nanos() as u64;
        if !cached {
            shared.busy_ns.fetch_add(ns, Ordering::Relaxed);
        }
        lock(&shared.done).push(Done { job: kind, payload, claim, ns, cached });
        shared.done_count.fetch_add(1, Ordering::AcqRel);
        let notify = lock(&shared.notify).clone();
        if let Some(notify) = notify {
            notify();
        }
    }
}

fn to_image_bytes(sdf: &Image<makepad_font_core::image::R>, size: Size<usize>) -> Vec<u8> {
    let mut out = Vec::with_capacity(size.width * size.height);
    for y in 0..size.height {
        for x in 0..size.width {
            out.push(sdf[Point::new(x, y)].r());
        }
    }
    out
}

fn run(state: &mut WorkerState, known: &RwLock<Known>, host: &HostFont, glyph: GlyphId, dpxs: f32, job: &JobKind, claim: &mut Option<Claim>) -> (Payload, bool) {
    let settings = state.settings;
    if let JobKind::Msdf { key, outline, dpxs, .. } = job {
        let msdf = render_msdf(&mut state.msdfer, outline, *dpxs, key.size);
        let pixels = msdf.into_pixels().into_iter().map(|p| p.bits).collect();
        return (Payload::Msdf { key: key.clone(), pixels }, false);
    }
    let JobKind::Glyph { req } = job else { unreachable!() };
    let kind = req.kind;
    let mut take = |c: Claim| -> ClaimResult {
        let result = known.write().unwrap_or_else(|e| e.into_inner()).claim(c.clone());
        if matches!(result, ClaimResult::Mine) {
            *claim = Some(c);
        }
        result
    };
    if kind == super::wire::kind::SLUG {
        let none = |curves| Payload::Slug { origin_in_ems: Point::new(0.0, 0.0), size_in_ems: Size::new(0.0, 0.0), curves };
        match take(Claim::Slug(host.id, glyph)) {
            // the service has the box of a glyph it holds
            ClaimResult::InAtlas => return (none(Work::InAtlas), true),
            ClaimResult::Elsewhere => return (none(Work::Elsewhere), true),
            ClaimResult::Mine => {}
        }
        let Some(font) = font_for(&mut state.fonts, host) else { return (Payload::Empty, false) };
        return match slug_curves(font, glyph) {
            Some(c) => (Payload::Slug { origin_in_ems: c.origin_in_ems, size_in_ems: c.size_in_ems, curves: Work::Rendered(c.curves) }, false),
            None => (Payload::Empty, false),
        };
    }
    let WorkerState { sdfer, fonts, .. } = state;
    let Some(font) = font_for(fonts, host) else { return (Payload::Empty, false) };
    // the embedded bitmap first
    if let Some(plan) = plan_color(font, glyph, dpxs) {
        let padding = makepad_font_core::raster::COLOR_PADDING;
        let (pixels, cached) = match take(Claim::Raster(plan.key.clone())) {
            ClaimResult::InAtlas => (Work::InAtlas, true),
            ClaimResult::Elsewhere => (Work::Elsewhere, true),
            ClaimResult::Mine => {
                let size = plan.key.size;
                let mut image = Image::<Bgra>::new(size);
                plan.render(font, glyph, dpxs, image.subimage_mut(Rect::from(size)));
                (Work::Rendered(image.into_pixels().into_iter().map(|p| p.bits).collect()), false)
            }
        };
        return (Payload::Color { key: plan.key, pixels, padding, origin: plan.origin_in_dpxs, dpxs: plan.dpxs_per_em }, cached);
    }
    let msdf_plan = (kind == super::wire::kind::AUTO && settings.outline_rasterization_mode == OutlineRasterizationMode::Msdf)
        .then(|| plan_outline_msdf(font, glyph, dpxs, &settings))
        .flatten();
    let Some(plan) = plan_outline_sdf(font, glyph, dpxs, &settings) else { return (Payload::Empty, false) };
    let (pixels, cached) = match take(Claim::Raster(plan.key.clone())) {
        ClaimResult::InAtlas => (Work::InAtlas, true),
        ClaimResult::Elsewhere => (Work::Elsewhere, true),
        ClaimResult::Mine => (Work::Rendered(to_image_bytes(&plan.render(sdfer), plan.key.size)), false),
    };
    // the multi-channel upgrade is worth a job when its slot is not there yet
    let msdf = msdf_plan.and_then(|m| (!known.read().unwrap_or_else(|e| e.into_inner()).raster.contains(&m.key)).then(|| (m.key.clone(), m.outline(), m.dpxs_per_em)));
    (Payload::Sdf { key: plan.key.clone(), pixels, padding: plan.padding, origin: plan.origin_in_dpxs, dpxs: plan.dpxs_per_em, msdf }, cached)
}
