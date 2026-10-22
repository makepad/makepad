//! Export of the strip, and the strip as data: tiles at most 4096 px wide, each a view of its own drawn by
//! [`Skyline::draw_view`] into an RGBA16F target and read back as it is, then written as PNG (toned, un-premultiplied
//! 8-bit) or EXR (linear float32, premultiplied), or handed over as a [`Band`] (the HDRI generator's skyline layer).
//! A job runs on a `Skyline` of its own, so the views are never disturbed. Tile k is drawn in frame k and read as soon
//! as that frame has been built (the next event, `job_event`; at the latest the next frame's draw), two targets in turn; its scene has the blur's and the water's margins (`draw_view`), so tiles meet without a
//! seam. Metal leaves out the first draw into a new target layout while it builds the pipeline (and a tile can be the first
//! to draw the water or a props layer), so there every tile is read until two reads agree.
use crate::light::SkyLight;
use crate::params::{Params, P};
use crate::render::{SceneTarget, Skyline, ViewSpec};
use makepad_widgets::makepad_draw::{makepad_platform::SignalToUI, *};
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind { Png, Exr }

/// The strip as premultiplied linear HDR texels, `w` x `h`, top row first.
pub struct Band { pub w: usize, pub h: usize, pub texels: Vec<[f32; 4]> }

/// Receives a requested band once, on a worker thread.
pub type BandDone = Box<dyn FnOnce(Result<Band, String>) + Send>;

const TILE: usize = 4096;

/// An IEEE half's value. Written here rather than taken from `makepad_script::pod_heap::f16_to_f32`, whose subnormal branch
/// keeps the implicit leading bit (it masks with 0x7FFFFF before the shift), so about half the subnormals (below 6.1e-5)
/// come out twice their value: the strip's darkest texels, which a host's small `design_gain` makes common.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let (e, m) = ((h >> 10) & 0x1f, (h & 0x3ff) as f32);
    match e {
        0 => sign * m * (1.0 / 16_777_216.0),
        31 => if m == 0.0 { sign * f32::INFINITY } else { f32::NAN },
        _ => sign * (1.0 + m / 1024.0) * 2f32.powi(e as i32 - 15),
    }
}
type Texels = Vec<[f32; 4]>;
/// Where a job's result goes.
enum Sink { File(ExportKind, std::path::PathBuf), Band(BandDone) }
/// From the workers: tile i's texels, or the job's end (its message).
enum News { Tile(usize, Result<Texels, String>), Done(Result<String, String>) }

/// A running job: the strip `w` x `h` at `p`, lit by `light`; the next tile to draw, the one drawn and not yet read
/// (with `Cx::frame_submission_serial` as it was when it was drawn), the reads in flight, the tiles done; whether each
/// tile is read until two reads agree (`first`: its last read, `rounds`).
pub struct Job {
    p: Params, light: SkyLight, w: usize, h: usize, next: usize, drawn: Option<usize>, drawn_serial: u64, reads: usize, done: usize, check: bool,
    first: Option<Texels>, rounds: usize, out: Texels, sink: Option<Sink>, t0: f64, targets: Vec<SceneTarget>, tx: Sender<News>, rx: Receiver<News>,
}

impl Job {
    fn tiles(&self) -> usize { self.w.div_ceil(TILE) }
    /// Tile i's view: its share of the turn.
    fn view(&self, i: usize) -> ViewSpec {
        let (cw, w) = (TILE.min(self.w - i * TILE), self.w as f64);
        ViewSpec { u0: (i * TILE) as f64 / w, span: cw as f64 / w, w: cw, h: self.h, horizon: self.p[P::Horizon], pad: 0 }
    }

    /// Asks for the read of the tile drawn last; its texels come back on a worker as `News::Tile`.
    fn read_drawn(&mut self, cx: &mut Cx) {
        let Some(i) = self.drawn.take() else { return };
        let (tx, size) = (self.tx.clone(), (self.view(i).w, self.h));
        let read = cx.read_render_texture(&self.targets[i % 2].color, move |r| {
            let lut = half_table();
            let f = |b: &[u8]| lut[u16::from_le_bytes([b[0], b[1]]) as usize];
            let decode = |(w, h, b): (usize, usize, std::sync::Arc<[u8]>)| if (w, h) != size { Err(format!("tile {i} read back {w} x {h}, not {} x {}", size.0, size.1)) }
                else { Ok(par(b.as_chunks::<8>().0, |t| [f(&t[0..]), f(&t[2..]), f(&t[4..]), f(&t[6..])])) };
            send(&tx, News::Tile(i, r.map_err(|e| format!("{e:?}")).and_then(decode)));
        });
        if let Err(e) = read { send(&self.tx, News::Tile(i, Err(format!("{e:?}")))); }
        self.reads += 1;
    }
}

/// Sends news from a worker and wakes the UI thread.
fn send(tx: &Sender<News>, n: News) { let _ = tx.send(n); SignalToUI::set_ui_signal(); }

impl Skyline {
    /// Starts exporting the strip at `p` lit by `light`, `width` px wide (the height from the field of view), into the
    /// Downloads folder; the message is for the message line.
    pub fn start_export(&mut self, _cx: &mut Cx, kind: ExportKind, width: usize, p: &Params, light: &SkyLight) -> Result<String, String> {
        let h = ViewSpec::strip(p, width).h;
        let path = downloads().join(format!("skyline_{width}x{h}.{}", if kind == ExportKind::Png { "png" } else { "exr" }));
        self.start(p, light, width, Sink::File(kind, path)).map_err(|(e, _)| e)?;
        Ok(format!("Rendering {kind:?} {width} x {h} ..."))
    }

    /// Asks for the strip at `p` lit by `light`, `w` px wide, as a [`Band`]; `done` gets it when the last tile has been read back, or
    /// the reason it failed. The holder's draw must call [`Skyline::draw_job`].
    pub fn request_band(&mut self, _cx: &mut Cx, p: &Params, light: &SkyLight, w: usize, done: BandDone) {
        if let Err((e, Sink::Band(done))) = self.start(p, light, w, Sink::Band(done)) { std::thread::spawn(move || done(Err(e))); }
    }

    /// True while a job (an export or a band) runs: [`Skyline::request_band`] and `start_export` would be refused.
    pub fn busy(&self) -> bool { self.job.is_some() }

    /// Starts a job, or hands the sink back when one is running.
    fn start(&mut self, p: &Params, light: &SkyLight, w: usize, sink: Sink) -> Result<(), (String, Sink)> {
        if self.job.is_some() { return Err(("An export is still running".into(), sink)); }
        let (v, (tx, rx)) = (ViewSpec::strip(p, w), channel());
        let check = cfg!(any(target_os = "macos", target_os = "ios"));
        let out = vec![[0.0; 4]; v.w * v.h];
        let t0 = Cx::monotonic_now();
        let (p, light, sink) = (p.clone(), light.clone(), Some(sink));
        self.job = Some(Job { p, light, w: v.w, h: v.h, next: 0, drawn: None, drawn_serial: 0, reads: 0, done: 0, check, first: None, rounds: 0, out, sink, t0, targets: vec![], tx, rx });
        Ok(())
    }

    /// Inside a Draw event: asks for the read of the tile drawn last frame if [`Skyline::job_event`] has not yet, and
    /// draws the next tile into its own pass (attached to the current one). True while the job needs frames: the caller
    /// draws again the next frame.
    pub fn draw_job(&mut self, cx: &mut Cx2d) -> bool {
        let Some(mut job) = self.job.take() else { return false };
        job.read_drawn(cx);
        if job.next < job.tiles() && (!job.check || job.reads == 0) {
            let (i, view) = (job.next, job.view(job.next));
            if job.targets.len() <= i % 2 { job.targets.push(SceneTarget::new(cx, "skyline_export", false)); }
            let rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(view.w as f64, view.h as f64) };
            let tone = matches!(job.sink, Some(Sink::File(ExportKind::Png, _)));
            let t = &mut job.targets[i % 2];
            cx.make_child_pass(&t.pass);
            t.record(cx, view.w, view.h, |cx| self.draw_view(cx, rect, &view, &job.p, tone, &job.light));
            (job.drawn, job.drawn_serial, job.next) = (Some(i), cx.frame_submission_serial(), i + 1);
            // The tile's pass is built into this frame's commands after the Draw event: the next event (this signal
            // is its wake) finds the submission serial past `drawn_serial` and asks for the read then, ordered after
            // the pass, rather than a frame later (`job_event`).
            SignalToUI::set_ui_signal();
        }
        let more = job.sink.is_some();
        self.job = Some(job);
        more
    }

    /// Asks for the read of the tile drawn last as soon as its frame has been built (its pass is then in the commands
    /// ahead of the read), and takes the workers' news: a tile read back, the file written or the band handed over, or a
    /// failure; returns the message for the message line, when there is one.
    pub fn job_event(&mut self, cx: &mut Cx, _event: &Event) -> Option<String> {
        let job = self.job.as_mut()?;
        if job.drawn.is_some() && cx.frame_submission_serial() > job.drawn_serial { job.read_drawn(cx); }
        while let Ok(news) = job.rx.try_recv() {
            let e = match news {
                News::Tile(i, Ok(t)) => {
                    job.reads -= 1;
                    if job.check && job.first.as_ref() != Some(&t) && job.rounds < 6 {
                        (job.first, job.rounds, job.next) = (Some(t), job.rounds + 1, i);
                        continue;
                    }
                    let cw = job.view(i).w;
                    (job.first, job.rounds, job.done) = (None, 0, job.done + 1);
                    for (y, row) in t.chunks(cw).enumerate() { job.out[y * job.w + i * TILE..][..cw].copy_from_slice(row); }
                    if job.done < job.tiles() { continue; }
                    let (w, h, mut out, sink, tx) = (job.w, job.h, std::mem::take(&mut job.out), job.sink.take(), job.tx.clone());
                    let (light, horizon) = (job.light.clone(), job.p[P::Horizon]);
                    std::thread::spawn(move || send(&tx, News::Done(match sink {
                        Some(Sink::File(kind, path)) => if kind == ExportKind::Png { png(w, h, &out) } else { Ok(exr(w, h, &out)) }
                            .and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())).map(|_| format!("Saved {}", path.display())),
                        Some(Sink::Band(done)) => {
                            // a host that shows the night as the example's display does gets it decoded (`SkyLight::to_host`)
                            light.to_host(&mut out, w, h, horizon);
                            done(Ok(Band { w, h, texels: out })); Ok(format!("Band {w} x {h} handed over"))
                        }
                        None => Err("no destination".into()),
                    })));
                    continue;
                }
                News::Done(Ok(msg)) => { let ms = (Cx::monotonic_now() - job.t0) * 1000.0; self.job = None; self.release(); return Some(format!("{msg} ({ms:.0} ms)")); }
                News::Tile(_, Err(e)) | News::Done(Err(e)) => e,
            };
            if let Some(Sink::Band(done)) = job.sink.take() { let e = e.clone(); std::thread::spawn(move || done(Err(e))); }
            self.job = None; self.release();
            return Some(format!("Export failed: {e}"));
        }
        None
    }
}

/// Every half's value (`f16_to_f32`), built once.
fn half_table() -> &'static [f32] {
    static TABLE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| (0..=u16::MAX).map(f16_to_f32).collect())
}

/// Below this many items `par` stays on the calling thread: a held strip's 23 552 texels decode in a fraction of what
/// starting a thread per core costs (plan hdri-5 Task 8 measured 2.6 ms from the read to the texels, most of it that).
pub(crate) const PAR_MIN: usize = 1 << 17;

/// `f` of every item of `t`, in order, the work split over every core (decoding and toning millions of texels).
fn par<T: Sync, U: Send + Copy + Default>(t: &[T], f: impl Fn(&T) -> U + Sync) -> Vec<U> {
    if t.len() < PAR_MIN { return t.iter().map(f).collect(); }
    let (n, mut out, f) = (t.len().div_ceil(std::thread::available_parallelism().map_or(4, |n| n.get())).max(1), vec![U::default(); t.len()], &f);
    std::thread::scope(|s| for (o, c) in out.chunks_mut(n).zip(t.chunks(n)) { s.spawn(move || for (o, x) in o.iter_mut().zip(c) { *o = f(x) }); });
    out
}

/// The user's Downloads folder when there is one, else the working directory.
fn downloads() -> std::path::PathBuf {
    let homes = ["USERPROFILE", "HOME"].into_iter().filter_map(std::env::var_os);
    homes.map(|h| std::path::PathBuf::from(h).join("Downloads")).find(|d| d.is_dir()).unwrap_or_default()
}

/// The toned texels as an 8-bit RGBA PNG, un-premultiplied (the page's `renderPngBlob`).
fn png(w: usize, h: usize, t: &[[f32; 4]]) -> Result<Vec<u8>, String> {
    use makepad_widgets::makepad_draw::makepad_zune_png::{PngEncoder, makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions}};
    let u = |x: f32, k: f32| (x * k).round().min(255.0) as u8;
    let px = par(t, |c| {
        let (a, k) = (c[3].clamp(0.0, 1.0), if c[3] > 0.0 { 255.0 / c[3].min(1.0) } else { 0.0 });
        [u(c[0], k), u(c[1], k), u(c[2], k), u(a, 255.0)]
    });
    let (mut out, options) = (Vec::new(), EncoderOptions::default().set_width(w).set_height(h).set_depth(BitDepth::Eight).set_colorspace(ColorSpace::RGBA));
    PngEncoder::new(px.as_flattened(), options).encode(&mut out).map_err(|e| format!("png: {e:?}"))?;
    Ok(out)
}

/// Uncompressed scanline OpenEXR, float32 A, B, G, R, premultiplied, top row first (the page's `exrBytes`).
fn exr(w: usize, h: usize, t: &[[f32; 4]]) -> Vec<u8> {
    let chans: Vec<u8> = b"ABGR".iter().flat_map(|c| [*c, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]).chain([0]).collect();
    let (bx, one) = ([0, 0, w as i32 - 1, h as i32 - 1].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>(), 1f32.to_le_bytes());
    let mut o = vec![0x76, 0x2f, 0x31, 0x01, 2, 0, 0, 0];
    let attrs = [("channels", "chlist", &chans[..]), ("compression", "compression", &[0]), ("dataWindow", "box2i", &bx), ("displayWindow", "box2i", &bx),
        ("lineOrder", "lineOrder", &[0]), ("pixelAspectRatio", "float", &one), ("screenWindowCenter", "v2f", &[0; 8]), ("screenWindowWidth", "float", &one)];
    for (name, ty, v) in attrs {
        for s in [name, ty] { o.extend(s.as_bytes()); o.push(0); }
        o.extend((v.len() as i32).to_le_bytes()); o.extend(v);
    }
    o.push(0);
    let (start, block) = (o.len() + 8 * h, 8 + 16 * w);
    for y in 0..h { o.extend(((start + y * block) as u64).to_le_bytes()); }
    for (y, row) in t.chunks(w).enumerate() {
        o.extend((y as i32).to_le_bytes()); o.extend(((16 * w) as i32).to_le_bytes());
        for c in [3, 2, 1, 0] { for px in row { o.extend(px[c].to_le_bytes()); } }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::{f16_to_f32, half_table, par, PAR_MIN};

    #[test]
    fn a_small_read_decodes_on_the_calling_thread() {
        let here = std::thread::current().id();
        let small = vec![0x3c00u16; PAR_MIN - 1];
        assert!(par(&small, |_| std::thread::current().id() == here).iter().all(|&on_caller| on_caller));
        let big = vec![0x3c00u16; PAR_MIN * 2];
        let decoded = par(&big, |h| half_table()[*h as usize]);
        assert!(decoded.iter().all(|&v| v == 1.0));
        assert!(par(&big, |_| std::thread::current().id() == here).iter().any(|&on_caller| !on_caller), "a big one is spread");
        assert!(std::ptr::eq(half_table(), half_table()), "the table is built once");
    }
    use makepad_widgets::makepad_draw::makepad_script::pod_heap::f16_to_f32 as platform_f16;

    #[test]
    fn halves_decode_exactly_subnormals_included() {
        for m in 1..1024u16 {
            assert_eq!(f16_to_f32(m), m as f32 / 16_777_216.0, "subnormal {m}");
            assert_eq!(f16_to_f32(0x8000 | m), -(m as f32) / 16_777_216.0);
        }
        // the normals: (1 + m/1024) 2^(e-15), each exact in f32 (the platform's normal branch is right too, but in a debug
        // build it panics below exponent 15: `exponent as u32 - 15` underflows)
        for h in (0x0400..0x7c00u16).chain(0x8400..0xfc00) {
            let (s, e, m) = (if h & 0x8000 != 0 { -1.0f64 } else { 1.0 }, ((h >> 10) & 0x1f) as i32, (h & 0x3ff) as f64);
            assert_eq!(f16_to_f32(h) as f64, s * (1.0 + m / 1024.0) * 2f64.powi(e - 15), "{h:#06x}");
        }
        assert_eq!((f16_to_f32(0), f16_to_f32(0x3c00), f16_to_f32(0x7c00)), (0.0, 1.0, f32::INFINITY));
        assert!(f16_to_f32(0x7e00).is_nan());
        // the platform's doubles a subnormal whose leading bit is odd (2^-23 for 2^-24 x 2); why this one is local
        assert_eq!((f16_to_f32(2), platform_f16(2)), (2.0 / 16_777_216.0, 4.0 / 16_777_216.0));
    }
}
