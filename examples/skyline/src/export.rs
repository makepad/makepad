//! Export of the strip, and the strip as data: tiles at most 4096 px wide, each a view of its own drawn by
//! [`Skyline::draw_view`] into an RGBA16F target and read back as it is, then written as PNG (toned, un-premultiplied
//! 8-bit) or EXR (linear float32, premultiplied), or handed over as a [`Band`] (the HDRI generator's skyline layer).
//! A job runs on a `Skyline` of its own, so the views are never disturbed. Tile k is drawn in frame k and read in frame
//! k + 1 (two targets in turn); its scene has the blur's and the water's margins (`draw_view`), so tiles meet without a
//! seam. Metal leaves out the first draw into a new target layout while it builds the pipeline (and a tile can be the first
//! to draw the water or a props layer), so there every tile is read until two reads agree.
use crate::params::{Params, P};
use crate::render::{SceneTarget, Skyline, ViewSpec};
use makepad_widgets::makepad_draw::{makepad_platform::SignalToUI, makepad_script::pod_heap::f16_to_f32, *};
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind { Png, Exr }

/// The strip as premultiplied linear HDR texels, `w` x `h`, top row first.
#[allow(dead_code)] // the HDRI layer's input
pub struct Band { pub w: usize, pub h: usize, pub texels: Vec<[f32; 4]> }

/// Receives a requested band once, on a worker thread.
#[allow(dead_code)] // the HDRI layer's input
pub type BandDone = Box<dyn FnOnce(Result<Band, String>) + Send>;

const TILE: usize = 4096;
type Texels = Vec<[f32; 4]>;
/// Where a job's result goes.
enum Sink { File(ExportKind, std::path::PathBuf), Band(BandDone) }
/// From the workers: tile i's texels, or the job's end (its message).
enum News { Tile(usize, Result<Texels, String>), Done(Result<String, String>) }

/// A running job: the strip `w` x `h` at `p`; the next tile to draw, the one drawn last frame,
/// the reads in flight, the tiles done; whether each tile is read until two reads agree (`first`: its last read, `rounds`).
pub struct Job {
    p: Params, w: usize, h: usize, next: usize, drawn: Option<usize>, reads: usize, done: usize, check: bool,
    first: Option<Texels>, rounds: usize, out: Texels, sink: Option<Sink>, t0: f64, targets: Vec<SceneTarget>, tx: Sender<News>, rx: Receiver<News>,
}

impl Job {
    fn tiles(&self) -> usize { self.w.div_ceil(TILE) }
    /// Tile i's view: its share of the turn.
    fn view(&self, i: usize) -> ViewSpec {
        let (cw, w) = (TILE.min(self.w - i * TILE), self.w as f64);
        ViewSpec { u0: (i * TILE) as f64 / w, span: cw as f64 / w, w: cw, h: self.h, horizon: self.p[P::Horizon] }
    }
}

/// Sends news from a worker and wakes the UI thread.
fn send(tx: &Sender<News>, n: News) { let _ = tx.send(n); SignalToUI::set_ui_signal(); }

impl Skyline {
    /// Starts exporting the strip at `p`, `width` px wide (the height from the field of view), into the Downloads folder;
    /// the message is for the message line.
    pub fn start_export(&mut self, _cx: &mut Cx, kind: ExportKind, width: usize, p: &Params) -> Result<String, String> {
        let h = ViewSpec::strip(p, width).h;
        let path = downloads().join(format!("skyline_{width}x{h}.{}", if kind == ExportKind::Png { "png" } else { "exr" }));
        self.start(p, width, Sink::File(kind, path)).map_err(|(e, _)| e)?;
        Ok(format!("Rendering {kind:?} {width} x {h} ..."))
    }

    /// Asks for the strip at `p`, `w` px wide, as a [`Band`]; `done` gets it when the last tile has been read back, or
    /// the reason it failed. The holder's draw must call [`Skyline::draw_job`].
    #[allow(dead_code)] // the HDRI layer's input
    pub fn request_band(&mut self, _cx: &mut Cx, p: &Params, w: usize, done: BandDone) {
        if let Err((e, Sink::Band(done))) = self.start(p, w, Sink::Band(done)) { std::thread::spawn(move || done(Err(e))); }
    }

    /// Starts a job, or hands the sink back when one is running.
    fn start(&mut self, p: &Params, w: usize, sink: Sink) -> Result<(), (String, Sink)> {
        if self.job.is_some() { return Err(("An export is still running".into(), sink)); }
        let (v, (tx, rx)) = (ViewSpec::strip(p, w), channel());
        let check = cfg!(any(target_os = "macos", target_os = "ios"));
        let out = vec![[0.0; 4]; v.w * v.h];
        let t0 = Cx::monotonic_now();
        let (p, sink) = (p.clone(), Some(sink));
        self.job = Some(Job { p, w: v.w, h: v.h, next: 0, drawn: None, reads: 0, done: 0, check, first: None, rounds: 0, out, sink, t0, targets: vec![], tx, rx });
        Ok(())
    }

    /// Inside a Draw event: asks for the read of the tile drawn last frame, and draws the next tile into its own pass
    /// (attached to the current one). True while the job needs frames: the caller draws again the next frame.
    pub fn draw_job(&mut self, cx: &mut Cx2d) -> bool {
        let Some(mut job) = self.job.take() else { return false };
        if let Some(i) = job.drawn.take() {
            let (tx, size) = (job.tx.clone(), (job.view(i).w, job.h));
            let read = cx.read_render_texture(&job.targets[i % 2].color, move |r| {
                let lut: Vec<f32> = (0..=u16::MAX).map(f16_to_f32).collect();
                let f = |b: &[u8]| lut[u16::from_le_bytes([b[0], b[1]]) as usize];
                let decode = |(w, h, b): (usize, usize, std::sync::Arc<[u8]>)| if (w, h) != size { Err(format!("tile {i} read back {w} x {h}, not {} x {}", size.0, size.1)) }
                    else { Ok(par(b.as_chunks::<8>().0, |t| [f(&t[0..]), f(&t[2..]), f(&t[4..]), f(&t[6..])])) };
                send(&tx, News::Tile(i, r.map_err(|e| format!("{e:?}")).and_then(decode)));
            });
            if let Err(e) = read { send(&job.tx, News::Tile(i, Err(format!("{e:?}")))); }
            job.reads += 1;
        }
        if job.next < job.tiles() && (!job.check || job.reads == 0) {
            let (i, view) = (job.next, job.view(job.next));
            if job.targets.len() <= i % 2 { job.targets.push(SceneTarget::new(cx, "skyline_export", false)); }
            let rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(view.w as f64, view.h as f64) };
            let tone = matches!(job.sink, Some(Sink::File(ExportKind::Png, _)));
            let t = &mut job.targets[i % 2];
            cx.make_child_pass(&t.pass);
            t.record(cx, view.w, view.h, |cx| self.draw_view(cx, rect, &view, &job.p, tone));
            (job.drawn, job.next) = (Some(i), i + 1);
        }
        let more = job.sink.is_some();
        self.job = Some(job);
        more
    }

    /// Takes the workers' news: a tile read back, the file written or the band handed over, or a failure; returns the
    /// message for the message line, when there is one.
    pub fn job_event(&mut self, _cx: &mut Cx, _event: &Event) -> Option<String> {
        let job = self.job.as_mut()?;
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
                    let (w, h, out, sink, tx) = (job.w, job.h, std::mem::take(&mut job.out), job.sink.take(), job.tx.clone());
                    std::thread::spawn(move || send(&tx, News::Done(match sink {
                        Some(Sink::File(kind, path)) => if kind == ExportKind::Png { png(w, h, &out) } else { Ok(exr(w, h, &out)) }
                            .and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())).map(|_| format!("Saved {}", path.display())),
                        Some(Sink::Band(done)) => { done(Ok(Band { w, h, texels: out })); Ok(format!("Band {w} x {h} handed over")) }
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

/// `f` of every item of `t`, in order, the work split over every core (decoding and toning millions of texels).
fn par<T: Sync, U: Send + Copy + Default>(t: &[T], f: impl Fn(&T) -> U + Sync) -> Vec<U> {
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
