//! The streamed-world lane: build workers, the per-frame upload budget,
//! residency, selection, occlusion, the draw and the shadow casters for a
//! [`crate::stream::TileSource`]. The policy (what to load, what to draw
//! with which dither window, the occlusion raster) is device-free and lives
//! in `stream.rs`; this file moves bytes and records draws.
//!
//! Per frame, on the UI thread, bounded and allocation-light:
//! 1. drain finished builds (a `try_recv` loop — nothing here ever waits);
//! 2. queue the nearest wanted pieces up to twice the worker count, so a
//!    camera that turns re-prioritises within a frame or two;
//! 3. upload ready pieces within a byte budget (at least one per frame);
//! 4. evict pieces far outside the load radius, and the farthest near
//!    meshes if resident bytes exceed the budget;
//! 5. select, frustum-cull, occlusion-test, and record the shadow casters.

use super::*;
use crate::stream::*;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// A resident or pending piece.
enum Slot {
    Absent,
    Queued,
    /// `casts`: the layers that render into the shadow cascades; `scene`:
    /// how many leading layers the scene draws (a merged caster-only layer,
    /// when there is one, is the last and only in `casts`).
    Resident { model: LoadedModel, bytes: usize, casts: std::ops::Range<usize>, scene: usize },
}
impl Slot {
    fn resident(&self) -> bool { matches!(self, Slot::Resident { .. }) }
    fn casting(&self) -> Option<(&LoadedModel, std::ops::Range<usize>)> { if let Slot::Resident { model, casts, .. } = self { Some((model, casts.clone())) } else { None } }
    fn scene(&self) -> Option<(&LoadedModel, usize)> { if let Slot::Resident { model, scene, .. } = self { Some((model, *scene)) } else { None } }
}

type BuildResult = (u64, StreamPiece, Result<(PreparedStaticPreview, usize, std::ops::Range<usize>, usize), String>);
type BuildJob = (u64, StreamPiece, Arc<dyn TileSource>);

pub(super) struct Workers {
    jobs: Sender<BuildJob>,
    done: Receiver<BuildResult>,
    count: usize,
}

fn start_workers() -> Workers {
    let (jobs, rx) = channel::<BuildJob>();
    let (tx, done) = channel::<BuildResult>();
    let rx = Arc::new(Mutex::new(rx));
    // Half the cores, at least two: the sim, the renderer and other build
    // lanes (level products, models) share the machine.
    let count = std::thread::available_parallelism().map_or(4, |n| n.get()).div_ceil(2).clamp(2, 6);
    for n in 0..count {
        let (rx, tx) = (rx.clone(), tx.clone());
        let _ = std::thread::Builder::new().name(format!("stream-build-{n}")).spawn(move || loop {
            // Only workers lock the queue; the UI thread only sends.
            let next = { let guard = rx.lock().unwrap_or_else(|p| p.into_inner()); guard.recv() };
            let Ok((generation, piece, source)) = next else { return };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mesh = source.build(piece)?;
                let bytes = mesh.geometry_bytes();
                // Props never cast (only chunks and cells do).
                prepared_from_stream(mesh, !matches!(piece, StreamPiece::Prop(_))).map(|(p, casts, scene, merged_bytes)| (p, bytes + merged_bytes, casts, scene))
            })).unwrap_or_else(|_| Err("stream build panicked".into()));
            if tx.send((generation, piece, result)).is_err() { return; }
        });
    }
    Workers { jobs, done, count }
}

/// A streamed mesh as the renderer's prepared static payload: layers only,
/// no colliders, bake charts, parts or sidecars. Casting layers are moved
/// first. Several casting layers are also merged into one caster-only layer
/// at the end (the cascades draw a chunk's walls, roofs and glass as ONE
/// draw instead of one per layer: a city's hundreds of chunks were most of
/// the cascades' draw calls). Returns the layers the cascades draw, how
/// many leading layers the scene draws, and the merged layer's bytes.
fn prepared_from_stream(mut mesh: StreamMesh, merge: bool) -> Result<(PreparedStaticPreview, std::ops::Range<usize>, usize, usize), String> {
    mesh.layers.retain(|l| l.indices.len() >= 3);
    mesh.layers.sort_by_key(|l| !l.casts);
    let casts = mesh.layers.iter().filter(|l| l.casts).count();
    let scene = mesh.layers.len();
    let merged = (merge && casts >= 2).then(|| {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for l in &mesh.layers[..casts] {
            let base = (vertices.len() / crate::model::MODEL_VERTEX_FLOATS) as u32;
            vertices.extend_from_slice(&l.vertices);
            indices.extend(l.indices.iter().map(|i| i + base));
        }
        let first = &mesh.layers[0];
        crate::stream::StreamLayer { vertices, indices, texture: first.texture.clone(), detail: None, casts: true, material: first.material.clone() }
    });
    let merged_bytes = merged.as_ref().map_or(0, |l| l.vertices.len() * 4 + l.indices.len() * 4);
    let casts = if merged.is_some() { scene..scene + 1 } else { 0..casts };
    mesh.layers.extend(merged);
    use crate::material_surface::{PixelSemantic, PreparedTexture};
    let solid = |fallback: u32, semantic: PixelSemantic| {
        let mut image = ImageBuffer::default();
        image.width = 1;
        image.height = 1;
        image.data = vec![fallback];
        std::sync::Arc::new(PreparedTexture::prepare(image, semantic))
    };
    let detail = solid(0xff80_8080, PixelSemantic::Color);
    let orm = solid(0xffff_ffff, PixelSemantic::Data);
    let triangles = mesh.triangles();
    // The layer's surface kind rides the metallic slot (DrawSceneCity reads
    // it); orm_on keeps upload from zeroing it as "not shiny".
    let mut layers = mesh.layers.into_iter().map(|l| PreparedStaticLayer {
        detail: l.detail.as_ref().map_or_else(|| detail.clone(), |(t, _)| t.clone()), detail_scale: l.detail.as_ref().map_or([0.0, 0.0], |(_, s)| *s),
        cutout: l.texture.cutout(), vertices: l.vertices, indices: l.indices, texture: l.texture,
        orm: l.material.orm.clone().unwrap_or_else(|| orm.clone()), orm_on: true, surface: None,
        pbr: crate::model::PbrMaterial { metallic: l.material.kind.code(), roughness: 1.0, orm_png: None, surface: None, mag_nearest: false },
    });
    let main = layers.next().ok_or("streamed mesh has no triangles")?;
    let extra: Vec<_> = layers.collect();
    Ok((PreparedStaticPreview {
        lods: Vec::new(), morph: None, ao: None, lm_source: None, bake_stream: None, sdf: None, emitters: Default::default(),
        main, extra, positions: Default::default(),
        // Only the length is read (the triangle count); the indices live in
        // the layers.
        mesh_indices: std::sync::Arc::new(vec![0; triangles * 3]),
        authored_collisions: Default::default(), collider_parts: Default::default(), occluder_parts: Default::default(),
        anim_parts: Vec::new(), driven_parts: Vec::new(), sky: None, min: mesh.min, max: mesh.max, prelit: false,
    }, casts, scene, merged_bytes))
}

/// Counters for one frame of the streamed lane (see [`Renderer::stream_stats`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamStats {
    pub chunks: usize,
    pub cells: usize,
    pub resident_near: usize,
    pub resident_proxy: usize,
    pub resident_cells: usize,
    pub resident_bytes: usize,
    pub queued: usize,
    pub ready: usize,
    pub uploaded: usize,
    pub uploaded_bytes: usize,
    pub evicted: usize,
    pub drawn_near: usize,
    pub drawn_proxy: usize,
    pub drawn_cells: usize,
    pub frustum_culled: usize,
    pub occluded: usize,
    pub props: usize,
    pub movers: usize,
    pub triangles: usize,
    pub casters: usize,
    pub occluders: usize,
    pub prepare_us: u64,
    pub occlusion_us: u64,
    pub upload_us: u64,
}

pub(super) struct StreamState {
    source: Arc<dyn TileSource>,
    settings: StreamSettings,
    generation: u64,
    /// None on a MIRROR (a split-screen pane's fork): it builds nothing
    /// and draws what the live renderer has resident.
    workers: Option<Workers>,
    chunk_bounds: Vec<(Vec3f, Vec3f)>,
    cells: Vec<(Vec3f, Vec3f, Vec<u32>)>,
    props: Vec<Vec<StreamProp>>,
    occluders: Vec<Vec<(Vec3f, Vec3f)>>,
    prop_distance: Vec<f32>,
    near: Vec<Slot>,
    proxy: Vec<Slot>,
    cell: Vec<Slot>,
    prop_models: Vec<Slot>,
    inflight: usize,
    ready: Vec<(StreamPiece, PreparedStaticPreview, usize, std::ops::Range<usize>, usize)>,
    resident_bytes: usize,
    wanted: Vec<(f32, StreamPiece)>,
    selection: Selection,
    raster: OcclusionRaster,
    /// This frame's pieces to draw: (piece, dither code, distance).
    draw_list: Vec<(StreamPiece, f32, f32)>,
    /// Props of near-drawn chunks and movers: (kind, transform, tint, dither).
    prop_list: Vec<(u16, Mat4f, Vec4f, f32)>,
    movers: Vec<StreamProp>,
    last_eye: Option<Vec3f>,
    velocity: Vec3f,
    time: f64,
    extra_foci: Vec<Vec3f>,
    /// 0 by day, 1 at night: window glow and street lamps (from the sun).
    night: f32,
    /// Headlight radius (metres), eased toward the ~24th-nearest car.
    headlight_radius: f32,
    stats: StreamStats,
}

struct Res<'a>(&'a StreamState);
impl Residency for Res<'_> {
    fn near(&self, c: usize) -> bool { self.0.near[c].resident() }
    fn proxy(&self, c: usize) -> bool { self.0.proxy[c].resident() }
    fn cell(&self, c: usize) -> bool { self.0.cell[c].resident() }
}

impl StreamState {
    /// This frame's occlusion raster (the streamed city's big boxes); it
    /// hides nothing when occlusion is off or nothing was rasterized.
    pub(super) fn occluders(&self) -> &OcclusionRaster {
        &self.raster
    }

    fn slot(&mut self, piece: StreamPiece) -> &mut Slot {
        match piece {
            StreamPiece::Near(i) => &mut self.near[i as usize],
            StreamPiece::Proxy(i) => &mut self.proxy[i as usize],
            StreamPiece::Cell(i) => &mut self.cell[i as usize],
            StreamPiece::Prop(i) => &mut self.prop_models[i as usize],
        }
    }
    fn bounds(&self, piece: StreamPiece) -> (Vec3f, Vec3f) {
        match piece {
            StreamPiece::Near(i) | StreamPiece::Proxy(i) => self.chunk_bounds[i as usize],
            StreamPiece::Cell(i) => (self.cells[i as usize].0, self.cells[i as usize].1),
            StreamPiece::Prop(_) => (Vec3f::default(), Vec3f::default()),
        }
    }
    fn queue(&mut self, piece: StreamPiece) {
        *self.slot(piece) = Slot::Queued;
        self.inflight += 1;
        let sent = self.workers.as_ref().is_some_and(|w| w.jobs.send((self.generation, piece, self.source.clone())).is_ok());
        if !sent {
            // Workers gone (a panic in the channel machinery): the piece
            // stays unloaded and the HLOD keeps drawing.
            *self.slot(piece) = Slot::Absent;
            self.inflight -= 1;
        }
    }
}

static STREAM_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Renderer {
    /// Install (or with `None`, drop) a streamed world. Chunk bounds, props
    /// and occluders are read once here; geometry arrives from workers.
    pub fn set_tile_source(&mut self, source: Option<Arc<dyn TileSource>>, settings: StreamSettings) {
        // Keep the worker threads for the next source.
        if let Some(old) = self.stream.take() { if old.workers.is_some() { self.stream_workers = old.workers; } }
        self.stream_casters.clear();
        self.prune_texture_cache();
        let Some(source) = source else { return };
        let chunk_bounds: Vec<_> = (0..source.chunk_count()).map(|i| source.chunk_bounds(i)).collect();
        let cells: Vec<_> = (0..source.cell_count()).map(|i| { let (a, b) = source.cell_bounds(i); (a, b, source.cell_chunks(i)) }).collect();
        let mut props = Vec::with_capacity(chunk_bounds.len());
        let mut occluders = Vec::with_capacity(chunk_bounds.len());
        for i in 0..chunk_bounds.len() {
            let mut p = Vec::new();
            source.chunk_props(i, &mut p);
            props.push(p);
            let mut o = Vec::new();
            source.chunk_occluders(i, &mut o);
            // Biggest first: the budgeted raster draws the best occluders.
            o.sort_by(|a, b| {
                let v = |(lo, hi): &(Vec3f, Vec3f)| (hi.x - lo.x) * (hi.y - lo.y) * (hi.z - lo.z);
                v(b).total_cmp(&v(a))
            });
            occluders.push(o);
        }
        let kinds = source.prop_kinds();
        let n = chunk_bounds.len();
        let mut state = StreamState {
            prop_distance: (0..kinds).map(|k| source.prop_draw_distance(k)).collect(),
            source, settings, generation: STREAM_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            workers: Some(self.stream_workers.take().unwrap_or_else(start_workers)),
            near: (0..n).map(|_| Slot::Absent).collect(), proxy: (0..n).map(|_| Slot::Absent).collect(),
            cell: (0..cells.len()).map(|_| Slot::Absent).collect(), prop_models: (0..kinds).map(|_| Slot::Absent).collect(),
            chunk_bounds, cells, props, occluders,
            inflight: 0, ready: Vec::new(), resident_bytes: 0, wanted: Vec::new(), selection: Selection::default(),
            raster: OcclusionRaster::default(), draw_list: Vec::new(), prop_list: Vec::new(), movers: Vec::new(),
            last_eye: None, velocity: Vec3f::default(), time: 0.0, extra_foci: Vec::new(), night: 0.0, headlight_radius: 0.0, stats: StreamStats::default(),
        };
        // Drain results of a previous source (generation mismatch drops them).
        if let Some(w) = &state.workers { while w.done.try_recv().is_ok() {} }
        for k in 0..kinds { state.queue(StreamPiece::Prop(k as u16)); }
        self.stream = Some(Box::new(state));
    }

    pub fn has_tile_source(&self) -> bool { self.stream.is_some() }

    /// Draw `live`'s streamed world from this renderer too (a split-screen
    /// pane's fork): same source, and every piece `live` has resident is
    /// shared by handle — no second upload, no second build. Call once per
    /// frame before drawing; `live` should stream around every pane's
    /// camera ([`Self::set_stream_foci`]).
    pub fn mirror_stream_from(&mut self, live: &Renderer) {
        let Some(src) = live.stream.as_ref() else { self.stream = None; return };
        if !self.stream.as_ref().is_some_and(|s| Arc::ptr_eq(&s.source, &src.source)) {
            let n = src.chunk_bounds.len();
            self.stream = Some(Box::new(StreamState {
                source: src.source.clone(), settings: src.settings, generation: src.generation, workers: None,
                chunk_bounds: src.chunk_bounds.clone(), cells: src.cells.clone(), props: src.props.clone(), occluders: src.occluders.clone(),
                prop_distance: src.prop_distance.clone(),
                near: (0..n).map(|_| Slot::Absent).collect(), proxy: (0..n).map(|_| Slot::Absent).collect(),
                cell: (0..src.cells.len()).map(|_| Slot::Absent).collect(), prop_models: (0..src.prop_models.len()).map(|_| Slot::Absent).collect(),
                inflight: 0, ready: Vec::new(), resident_bytes: 0, wanted: Vec::new(), selection: Selection::default(),
                raster: OcclusionRaster::default(), draw_list: Vec::new(), prop_list: Vec::new(), movers: Vec::new(),
                last_eye: None, velocity: Vec3f::default(), time: 0.0, extra_foci: Vec::new(), night: 0.0, headlight_radius: 0.0, stats: StreamStats::default(),
            }));
        }
        let st = self.stream.as_mut().unwrap();
        st.settings = src.settings;
        st.time = src.time;
        let sync = |mine: &mut [Slot], theirs: &[Slot]| {
            for (m, t) in mine.iter_mut().zip(theirs) {
                match (&*m, t) {
                    (Slot::Resident { model: a, .. }, Slot::Resident { model: b, .. }) if a.geometry.geometry_id() == b.geometry.geometry_id() => {}
                    (_, Slot::Resident { model, bytes, casts, scene }) => *m = Slot::Resident { model: model.clone(), bytes: *bytes, casts: casts.clone(), scene: *scene },
                    (Slot::Absent, _) => {}
                    _ => *m = Slot::Absent,
                }
            }
        };
        sync(&mut st.near, &src.near);
        sync(&mut st.proxy, &src.proxy);
        sync(&mut st.cell, &src.cell);
        sync(&mut st.prop_models, &src.prop_models);
    }
    pub fn set_stream_settings(&mut self, settings: StreamSettings) { if let Some(s) = self.stream.as_mut() { s.settings = settings; } }
    pub fn stream_settings(&self) -> Option<StreamSettings> { self.stream.as_ref().map(|s| s.settings) }
    /// Other points detail should stream around (split-screen panes,
    /// spectated players). The camera is always one.
    pub fn set_stream_foci(&mut self, foci: &[Vec3f]) { if let Some(s) = self.stream.as_mut() { s.extra_foci.clear(); s.extra_foci.extend_from_slice(foci); } }
    /// Clock for the source's movers (ambient traffic), seconds.
    pub fn set_stream_time(&mut self, time: f64) { if let Some(s) = self.stream.as_mut() { s.time = time; } }
    pub fn stream_stats(&self) -> Option<StreamStats> { self.stream.as_ref().map(|s| s.stats) }

    /// Steps 1–5 of the file header, before the shadow passes (they need
    /// this frame's casters). `clip` is world → clip for the occlusion
    /// raster; `None` (XR) skips occlusion.
    pub(super) fn stream_prepare(&mut self, cx: &mut Cx, eye: Vec3f, clip: Option<&Mat4f>, frustum: Option<&Frustum>) {
        let Some(mut st) = self.stream.take() else { return };
        let t0 = Cx::monotonic_now();
        let s = st.settings;
        let mut stats = StreamStats { chunks: st.chunk_bounds.len(), cells: st.cells.len(), ..Default::default() };
        // 1. finished builds.
        while let Some(Ok((generation, piece, result))) = st.workers.as_ref().map(|w| w.done.try_recv()) {
            if generation != st.generation { continue; }
            st.inflight = st.inflight.saturating_sub(1);
            match result {
                Ok((prepared, bytes, casts, scene)) => st.ready.push((piece, prepared, bytes, casts, scene)),
                Err(e) => {
                    log!("stream: {piece:?} failed: {e}");
                    // Leave it Absent-but-not-requeued this frame; a later
                    // frame retries (a transient failure heals, a permanent
                    // one keeps its HLOD).
                    *st.slot(piece) = Slot::Absent;
                }
            }
        }
        // Motion lookahead: stream toward where the camera is going.
        if let Some(last) = st.last_eye {
            let v = (eye - last) * 60.0;
            st.velocity = st.velocity * 0.8 + v * 0.2;
        }
        st.last_eye = Some(eye);
        let mut foci = vec![eye];
        if st.velocity.length() > 2.0 { foci.push(eye + st.velocity * s.lookahead); }
        foci.extend_from_slice(&st.extra_foci);
        // 2. queue the nearest wanted pieces.
        let mut wanted = std::mem::take(&mut st.wanted);
        wanted_with_budget(&s, &foci, &st, &mut wanted);
        let cap = st.workers.as_ref().map_or(0, |w| w.count * 2);
        for &(_, piece) in &wanted {
            if st.inflight >= cap { break; }
            if matches!(st.slot(piece), Slot::Absent) && !st.ready.iter().any(|(p, ..)| *p == piece) { st.queue(piece); }
        }
        st.wanted = wanted;
        // 3. uploads within the byte budget, nearest first.
        let t_up = Cx::monotonic_now();
        if !st.ready.is_empty() {
            let prio = |st: &StreamState, p: StreamPiece| -> f32 {
                match p { StreamPiece::Prop(_) => -1.0, StreamPiece::Cell(_) => { let b = st.bounds(p); aabb_distance(b.0, b.1, eye) * 0.25 }, _ => { let b = st.bounds(p); aabb_distance(b.0, b.1, eye) } }
            };
            let mut ready = std::mem::take(&mut st.ready);
            ready.sort_by(|a, b| prio(&st, b.0).total_cmp(&prio(&st, a.0)));
            let mut spent = 0usize;
            while let Some((piece, prepared, bytes, casts, scene)) = ready.pop() {
                if spent > 0 && spent + bytes > s.upload_bytes_per_frame { ready.push((piece, prepared, bytes, casts, scene)); break; }
                // A piece that went out of range while building is dropped.
                if !matches!(piece, StreamPiece::Prop(_) | StreamPiece::Cell(_)) && evictable(&s, &foci, st.bounds(piece), piece) {
                    *st.slot(piece) = Slot::Absent;
                    continue;
                }
                let uploaded = self.upload_static_preview(cx, prepared);
                let model = Self::uploaded_static_model(uploaded);
                *st.slot(piece) = Slot::Resident { model, bytes, casts, scene };
                st.resident_bytes += bytes;
                spent += bytes;
                stats.uploaded += 1;
            }
            stats.uploaded_bytes = spent;
            st.ready = ready;
        }
        stats.upload_us = ((Cx::monotonic_now() - t_up) * 1e6) as u64;
        // 4. evictions: out of range, then over budget (farthest near first).
        // A mirror holds exactly what its live renderer holds.
        let chunks_to_check = if st.workers.is_some() { st.chunk_bounds.len() } else { 0 };
        for i in 0..chunks_to_check {
            for piece in [StreamPiece::Near(i as u32), StreamPiece::Proxy(i as u32)] {
                if st.slot(piece).resident() && evictable(&s, &foci, st.bounds(piece), piece) {
                    if let Slot::Resident { bytes, .. } = std::mem::replace(st.slot(piece), Slot::Absent) { st.resident_bytes -= bytes; }
                    stats.evicted += 1;
                }
            }
        }
        if st.workers.is_some() && st.resident_bytes > s.resident_bytes {
            let mut near: Vec<(f32, usize)> = (0..st.chunk_bounds.len()).filter(|&i| st.near[i].resident())
                .map(|i| (aabb_distance(st.chunk_bounds[i].0, st.chunk_bounds[i].1, eye), i)).collect();
            near.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (_, i) in near {
                if st.resident_bytes <= s.resident_bytes { break; }
                if let Slot::Resident { bytes, .. } = std::mem::replace(&mut st.near[i], Slot::Absent) { st.resident_bytes -= bytes; }
                stats.evicted += 1;
            }
        }
        // 5. selection, culling, occlusion.
        let mut selection = std::mem::take(&mut st.selection);
        select(&s, eye, &st.chunk_bounds, &st.cells, &Res(&st), &mut selection);
        let t_occ = Cx::monotonic_now();
        let mut raster = std::mem::take(&mut st.raster);
        let occlusion = s.occlusion && clip.is_some();
        if let (true, Some(clip)) = (occlusion, clip) {
            raster.begin(clip, eye);
            // The biggest boxes on screen first (extent over distance), up
            // to a budget — fewer occluders only ever means less culling.
            let mut candidates: Vec<(f32, Vec3f, Vec3f)> = Vec::new();
            for (i, (lo, hi)) in st.chunk_bounds.iter().enumerate() {
                if aabb_distance(*lo, *hi, eye) > s.occluder_range { continue; }
                for (blo, bhi) in &st.occluders[i] {
                    let size = (bhi.x - blo.x).max(bhi.z - blo.z).min(bhi.y - blo.y);
                    let score = size / aabb_distance(*blo, *bhi, eye).max(1.0);
                    if score >= 0.08 { candidates.push((score, *blo, *bhi)); }
                }
            }
            const OCCLUDERS: usize = 320;
            if candidates.len() > OCCLUDERS {
                candidates.select_nth_unstable_by(OCCLUDERS, |a, b| b.0.total_cmp(&a.0));
                candidates.truncate(OCCLUDERS);
            }
            for (_, lo, hi) in &candidates { raster.add_box(*lo, *hi); }
            raster.finish();
            stats.occluders = raster.occluders;
        } else {
            // No raster this frame: last frame's (another camera) must not
            // hide anything the placed models test against it.
            raster.occluders = 0;
        }
        stats.occlusion_us = ((Cx::monotonic_now() - t_occ) * 1e6) as u64;
        st.draw_list.clear();
        let mut casters = std::mem::take(&mut self.stream_casters);
        casters.clear();
        for &(cell, w) in &selection.cells {
            let (lo, hi) = (st.cells[cell as usize].0, st.cells[cell as usize].1);
            if let Some((m, n)) = st.cell[cell as usize].casting() { stream_casters_of(&mut casters, m, n); }
            if frustum.is_some_and(|f| !f.intersects_aabb(lo, hi)) { stats.frustum_culled += 1; continue; }
            if occlusion && raster.occluded(lo, hi) { stats.occluded += 1; continue; }
            st.draw_list.push((StreamPiece::Cell(cell), w.encode(), aabb_distance(lo, hi, eye)));
            stats.drawn_cells += 1;
        }
        st.prop_list.clear();
        let mut cast_last: Option<u32> = None;
        for &(chunk, near, w) in &selection.chunks {
            let ci = chunk as usize;
            let (lo, hi) = st.chunk_bounds[ci];
            // Shadows: once per chunk, the proxy standing in for both (a
            // near wall and its proxy cast the same shadow; the proxy is a
            // tenth the draws). A chunk in a crossfade appears twice in the
            // selection — only its first entry casts.
            if cast_last != Some(chunk) {
                cast_last = Some(chunk);
                if let Some((m, n)) = st.proxy[ci].casting().or_else(|| st.near[ci].casting()) { stream_casters_of(&mut casters, m, n); }
            }
            if frustum.is_some_and(|f| !f.intersects_aabb(lo, hi)) { stats.frustum_culled += 1; continue; }
            if occlusion && raster.occluded(lo, hi) { stats.occluded += 1; continue; }
            let d = aabb_distance(lo, hi, eye);
            st.draw_list.push((if near { StreamPiece::Near(chunk) } else { StreamPiece::Proxy(chunk) }, w.encode(), d));
            if near {
                stats.drawn_near += 1;
                for p in &st.props[ci] {
                    let pos = vec3f(p.transform.v[12], p.transform.v[13], p.transform.v[14]);
                    let limit = st.prop_distance.get(p.kind as usize).copied().unwrap_or(0.0);
                    if (pos - eye).length() > limit { continue; }
                    if frustum.is_some_and(|f| !f.intersects_sphere(pos, 12.0 * p.transform.v[5].abs().max(1.0))) { continue; }
                    st.prop_list.push((p.kind, p.transform, p.tint, w.encode()));
                }
            } else {
                stats.drawn_proxy += 1;
            }
        }
        // Movers (ambient traffic) near the eye.
        let mut movers = std::mem::take(&mut st.movers);
        movers.clear();
        st.source.movers(st.time, eye, 450.0, &mut movers);
        for m in &movers {
            let pos = vec3f(m.transform.v[12], m.transform.v[13], m.transform.v[14]);
            if frustum.is_some_and(|f| !f.intersects_sphere(pos, 4.0)) { continue; }
            if occlusion && raster.occluded(pos - vec3f(2.5, 0.0, 2.5), pos + vec3f(2.5, 2.0, 2.5)) { continue; }
            st.prop_list.push((m.kind, m.transform, m.tint, 0.0));
            stats.movers += 1;
        }
        st.movers = movers;
        stats.props = st.prop_list.len() - stats.movers;
        // Group props by kind so each kind is one instanced draw.
        st.prop_list.sort_by_key(|p| p.0);
        stats.casters = casters.len();
        self.stream_casters = casters;
        st.selection = selection;
        st.raster = raster;
        stats.resident_near = st.near.iter().filter(|s| s.resident()).count();
        stats.resident_proxy = st.proxy.iter().filter(|s| s.resident()).count();
        stats.resident_cells = st.cell.iter().filter(|s| s.resident()).count();
        stats.resident_bytes = st.resident_bytes;
        stats.queued = st.inflight;
        stats.ready = st.ready.len();
        stats.prepare_us = ((Cx::monotonic_now() - t0) * 1e6) as u64;
        st.stats = stats;
        self.stream = Some(st);
    }

    /// After dark, the streamed world's lamps (props whose kind carries a
    /// light, near the eye) join this frame's AUTHORED lights, so they go
    /// through the same budget: the ones that matter at the eye win, the
    /// tail fades (`budget_authored_lights`). Also sets the night factor the
    /// window glow reads. Call before `build_frame_lights`.
    pub(super) fn stream_lights(&mut self, eye: Vec3f, sun_dir_y: f32) {
        let Some(st) = self.stream.as_mut() else { return };
        let t = ((0.22 - sun_dir_y) / 0.3).clamp(0.0, 1.0);
        st.night = t * t * (3.0 - 2.0 * t);
        // Headlights (and any mover light, at its own level): forward spots on the movers that carry one (last
        // frame's movers; a frame late is invisible). No hard cut by rank,
        // which swapped lights between cars frame to frame: every lit car
        // within a RADIUS draws, fading over the radius' last quarter, and
        // the radius follows the ~24th-nearest car slowly (hysteresis), so a
        // light only ever fades in or out as its car crosses the edge.
        {
            const TARGET: usize = 24;
            const CAP: usize = 40;
            const RANGE: f32 = 140.0;
            let mut lights: Vec<Option<(Vec3f, Vec3f, Vec3f)>> = Vec::new();
            let mut cars: Vec<(f32, usize)> = Vec::new();
            for (i, m) in st.movers.iter().enumerate() {
                let k = m.kind as usize;
                if lights.len() <= k { lights.resize(k + 1, None); }
                if lights[k].is_none() { lights[k] = st.source.mover_light(k); }
                if lights[k].is_none() || st.source.mover_light_level(k, st.night) < 0.02 { continue; }
                let d = (vec3f(m.transform.v[12], m.transform.v[13], m.transform.v[14]) - eye).length();
                if d < RANGE { cars.push((d, i)); }
            }
            cars.sort_by(|a, b| a.0.total_cmp(&b.0));
            let target = cars.get(TARGET).map_or(RANGE, |c| c.0).clamp(30.0, RANGE);
            st.headlight_radius = if st.headlight_radius <= 0.0 { target } else { st.headlight_radius + (target - st.headlight_radius) * 0.03 };
            let r = st.headlight_radius;
            for &(d, i) in cars.iter().take(CAP) {
                let fade = ((r - d) / (r * 0.25)).clamp(0.0, 1.0);
                if fade <= 0.0 { break; }
                let m = &st.movers[i];
                let Some((offset, dir, color)) = lights[m.kind as usize] else { continue };
                let v = &m.transform.v;
                let pos = vec3f(
                    v[0] * offset.x + v[4] * offset.y + v[8] * offset.z + v[12],
                    v[1] * offset.x + v[5] * offset.y + v[9] * offset.z + v[13],
                    v[2] * offset.x + v[6] * offset.y + v[10] * offset.z + v[14],
                );
                let axis = vec3f(v[0] * dir.x + v[4] * dir.y + v[8] * dir.z, v[1] * dir.x + v[5] * dir.y + v[9] * dir.z, v[2] * dir.x + v[6] * dir.y + v[10] * dir.z).normalize();
                self.host_asset_lights.push(crate::lightmap::LmLight {
                    pos, color: color * (st.source.mover_light_level(m.kind as usize, st.night) * fade), radius: 26.0, dir: axis, spot: 1.0, cone: Some((10.0, 34.0)), shadows: false,
                    area: None,
                });
            }
        }
        if st.night < 0.05 { return; }
        let kinds: Vec<Option<(Vec3f, Vec3f, f32)>> = (0..st.prop_models.len()).map(|k| st.source.prop_light(k)).collect();
        if kinds.iter().all(Option::is_none) { return; }
        const RANGE: f32 = 220.0;
        for (i, (lo, hi)) in st.chunk_bounds.iter().enumerate() {
            if aabb_distance(*lo, *hi, eye) > RANGE { continue; }
            for p in &st.props[i] {
                let Some(Some((offset, color, mount))) = kinds.get(p.kind as usize) else { continue };
                let m = &p.transform.v;
                let pos = vec3f(
                    m[0] * offset.x + m[4] * offset.y + m[8] * offset.z + m[12],
                    m[1] * offset.x + m[5] * offset.y + m[9] * offset.z + m[13],
                    m[2] * offset.x + m[6] * offset.y + m[10] * offset.z + m[14],
                );
                if (pos - eye).length() > RANGE { continue; }
                let (radius, strength) = crate::lightmap::lamp_photometry(*mount);
                self.host_asset_lights.push(crate::lightmap::LmLight {
                    pos, color: *color * (strength * st.night), radius, dir: vec3f(0.0, -1.0, 0.0), spot: 1.0, cone: None, shadows: false,
                    area: None,
                });
            }
        }
    }

    /// Draw this frame's streamed pieces and props through the diffuse
    /// world-model shader (streamed materials are matte). Each piece is one
    /// draw per layer; each prop kind is one instanced draw per layer.
    pub(super) fn draw_stream(&mut self, cx: &mut Cx3d, diffuse: &mut DrawSceneSkinned, eye: Vec3f, fog: (Vec3f, f32), sun: &SunLight) {
        if self.stream.as_ref().is_none_or(|s| s.draw_list.is_empty() && s.prop_list.is_empty()) { return; }
        if self.city_draw.is_none() {
            // Held VM: draw matte through the diffuse lane this frame.
            self.city_draw = cx.cx.try_with_vm(|vm| Box::new(crate::shaders::DrawSceneCity::script_new_with_default(vm)));
        }
        let mut city = self.city_draw.take();
        let night = self.stream.as_ref().map_or(0.0, |s| s.night);
        // The stream clock (seconds, wrapped so f32 keeps sub-frame steps):
        // the city shader's puddle ripple.
        let stream_time = self.stream.as_ref().map_or(0.0, |s| (s.time % 3600.0) as f32);
        // Until the city pipeline can draw (Metal compiles it asynchronously
        // after a shader change) or if it failed, the city draws matte
        // rather than not at all.
        let hdr = self.hdr_output;
        // Ready: the city lane's shader for this frame's features (variants.rs).
        let stock = city.as_ref().and_then(|c| c.pbr.skinned.draw_vars.draw_shader_id);
        let shader = if stock.is_some() { self.lane_shaders(cx.cx, super::variants::ModelLane::City, stock).0 } else { None };
        let ready = shader.is_some_and(|id| cx.cx.draw_shader_ready(id, hdr));
        let mut draw = match city.as_deref_mut().filter(|_| ready && city_shader_on()) {
            Some(c) => { c.pbr.skinned.draw_vars.set_uniform(cx.cx, live_id!(city), &[night, stream_time, 0.0, 0.0]); ModelDraw::City(c) }
            None => ModelDraw::Diffuse(diffuse),
        };
        self.draw_stream_with(cx, &mut draw, eye, fog, sun);
        drop(draw);
        self.city_draw = city;
    }

    fn draw_stream_with(&mut self, cx: &mut Cx3d, draw: &mut ModelDraw<'_>, eye: Vec3f, fog: (Vec3f, f32), sun: &SunLight) {
        self.bind_model_lane(cx, draw, eye, fog, sun);
        // The lane's shader variant for this frame's features, and for
        // layers that cut no pixel (not dithered, texture fully opaque) the
        // one without `clip` (renderer/variants.rs).
        let model_lane = match draw {
            ModelDraw::City(_) => Some(super::variants::ModelLane::City),
            ModelDraw::Diffuse(_) => Some(super::variants::ModelLane::Diffuse),
            _ => None,
        };
        let opaque = match model_lane {
            Some(model_lane) => {
                let stock = draw.base().draw_vars.draw_shader_id;
                let (full, opaque) = self.lane_shaders(cx.cx, model_lane, stock);
                draw.base().draw_vars.draw_shader_id = full;
                opaque.filter(|id| cx.cx.draw_shader_ready(*id, self.hdr_output))
            }
            None => None,
        };
        // Statics: one transient-only light block for the whole lane.
        {
            let empty_block = LightBlock::default();
            let mut block = [0.0f32; LIGHT_BLOCK_FLOATS];
            let split = if self.clustered_enabled { 0 } else {
                merge_transients_into_block(&empty_block, &self.frame_lights, self.frame_baked_count..self.frame_lights.len(), eye, &mut self.light_rank, &mut block)
            };
            write_light_block(cx.cx, &mut draw.base().draw_vars, &block, split);
        }
        let Some(st) = self.stream.take() else { return };
        let glow = 1.0 + st.night.max(0.001);
        {
            let b = draw.base();
            b.draw_vars.set_uniform(cx.cx, live_id!(ao_enabled), &[0.0]);
            b.lm_rect = Vec4f::default();
            b.dl_apply = 0.0;
            b.ground_y = 0.0;
            b.depth_bias = 0.0;
            b.draw_vars.set_uniform(cx.cx, live_id!(morph_ctl), &[0.0, 0.0, 0.0, 0.0]);
            b.prelit = 0.0;
            b.tint = vec4(1.0, 1.0, 1.0, 1.0);
        }
        let mut triangles = 0usize;
        let mut fur_budget = 0usize;
        // Draws layers `layers` of `m` at one placement.
        let mut submit = |draw: &mut ModelDraw<'_>, cx: &mut Cx3d, m: &LoadedModel, layers: std::ops::Range<usize>, transform: Mat4f, tint: Vec4f, dither: f32| {
            draw.base().transform = transform;
            draw.base().tint = tint;
            draw.base().color_adjust_ctl = vec4(0.0, 1.0, 1.0, dither);
            let all = std::iter::once((&m.geometry, &m.texture, &m.detail, m.detail_scale, &m.material))
                .chain(m.extra_draws.iter().map(|(g, t, d, s, mat)| (g, t, d, *s, mat)));
            for (g, t, d, s, mat) in all.skip(layers.start).take(layers.len()) {
                draw.base().draw_vars.geometry_id = Some(g.geometry_id());
                draw.base().draw_vars.set_texture(0, t);
                draw.base().draw_vars.set_texture(5, d);
                draw.base().draw_vars.set_uniform(cx.cx, live_id!(detail_st), &[s[0], s[1]]);
                draw.set_material(cx.cx, mat);
                let cut = dither > 0.5 || mat.cutout;
                draw.submit_as(cx, 0.0, &mut fur_budget, opaque.filter(|_| !cut));
            }
        };
        for &(piece, dither, _) in &st.draw_list {
            let slot = match piece {
                StreamPiece::Near(i) => &st.near[i as usize],
                StreamPiece::Proxy(i) => &st.proxy[i as usize],
                StreamPiece::Cell(i) => &st.cell[i as usize],
                StreamPiece::Prop(_) => continue,
            };
            if let Some((m, scene)) = slot.scene() {
                submit(draw, cx, m, 0..scene, Mat4f::identity(), vec4(1.0, 1.0, 1.0, glow), dither);
                triangles += m.triangles;
            }
        }
        // Props, a kind at a time and layer-major within it: every copy's
        // first layer, then every copy's second. Copy by copy, a
        // multi-layer prop alternated geometries and each layer of each
        // copy was its own draw call.
        let mut group = 0;
        while group < st.prop_list.len() {
            let kind = st.prop_list[group].0;
            let end = group + st.prop_list[group..].iter().take_while(|p| p.0 == kind).count();
            if let Some((m, scene)) = st.prop_models.get(kind as usize).and_then(Slot::scene) {
                for layer in 0..scene {
                    for (_, transform, tint, dither) in &st.prop_list[group..end] {
                        submit(draw, cx, m, layer..layer + 1, *transform, *tint, *dither);
                    }
                }
                triangles += m.triangles * (end - group);
            }
            group = end;
        }
        drop(submit);
        draw.base().color_adjust_ctl = vec4(0.0, 1.0, 1.0, 0.0);
        draw.base().tint = vec4(1.0, 1.0, 1.0, 1.0);
        let mut st = st;
        st.stats.triangles = triangles;
        self.stream = Some(st);
    }
}

/// `MAKEPAD_CITY_SHADER=0` draws the streamed world through the matte
/// diffuse lane (the A/B for the city shader's cost).
fn city_shader_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_CITY_SHADER").map_or(true, |v| v != "0"))
}

/// The shadow-casting layers of a streamed model (`prepared_from_stream`:
/// its leading casting layers, or their merged caster-only layer).
fn stream_casters_of(casters: &mut Vec<crate::gpu_lightmap::GpuBakeMesh>, m: &LoadedModel, casting: std::ops::Range<usize>) {
    for (k, g) in std::iter::once(&m.geometry).chain(m.extra_draws.iter().map(|(g, ..)| g)).enumerate() {
        if !casting.contains(&k) { continue; }
        casters.push(crate::gpu_lightmap::GpuBakeMesh { geometry: g.geometry_id(), transform: Mat4f::identity(), min: m.min, max: m.max, cutout: None, band: Default::default() });
    }
}

/// [`wanted`] plus the budget rule: near meshes beyond what the resident
/// budget can hold are not requested (they would only be evicted).
fn wanted_with_budget(s: &StreamSettings, foci: &[Vec3f], st: &StreamState, out: &mut Vec<(f32, StreamPiece)>) {
    wanted(s, foci, &st.chunk_bounds, &st.cells, out);
    if st.resident_bytes > s.resident_bytes {
        out.retain(|(_, p)| !matches!(p, StreamPiece::Near(_)));
    }
}
