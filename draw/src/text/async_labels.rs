//! Worker-owned label layout and raster preparation. The UI only admits immutable
//! runs and places their resident glyphs. All channels are bounded; UI sends and
//! receives never block. A caller must request frames while `pending()` is true.
use super::{
    font::FontId,
    font_family::{FontDiagnostics, FontFamilyId},
    fonts::Fonts,
    geom::Point,
    layouter::{BorrowedLayoutParams, LayoutOptions, Layouter, Settings, Style},
    loader::{FontDefinition, FontFamilyDefinition},
    rasterizer::{PublishedGlyph, RasterizedGlyph},
};
use crate::{makepad_platform::*, Cx2d, DrawText};
use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, Instant},
};

use crate::makepad_platform::thread::ui_hang::hashing::{HashMap, HashSet};

pub(super) struct FontSnapshot {
    pub id: FontId,
    pub data: Vec<u8>,
    pub index: u32,
    pub variations: Vec<(u32, f32)>,
    pub ascender_fudge: f32,
    pub descender_fudge: f32,
}
struct FamilySnapshot {
    id: FontFamilyId,
    font_ids: Vec<FontId>,
    fonts: Vec<FontSnapshot>,
    diagnostics: FontDiagnostics,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    text: LabelText,
    family: FontFamilyId,
    size: u32,
    dpi: u32,
    scale: u32,
    spacing: u32,
}
impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // A label has only a few font/size variants. Bucket by its unique
        // text id; complete equality still distinguishes every metric variant.
        state.write_u64(self.text.id);
    }
}
/// The cache owns every interned string for its lifetime. Hashing a run key
/// therefore needs only this stable id, including when a resident table grows.
#[derive(Clone, Debug)]
struct LabelText {
    id: u64,
    value: Arc<str>,
}
impl PartialEq for LabelText {
    fn eq(&self, other: &Self) -> bool { self.id == other.id }
}
impl Eq for LabelText {}
impl std::hash::Hash for LabelText {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { state.write_u64(self.id); }
}
impl std::ops::Deref for LabelText {
    type Target = str;
    fn deref(&self) -> &str { &self.value }
}
struct Request {
    key: Key,
    family: Option<FamilySnapshot>,
}
struct Glyph {
    origin: Point<f32>,
    size: f32,
    image: Arc<PublishedGlyph>,
}
struct Publication {
    #[cfg(test)]
    shaped_on: std::thread::ThreadId,
    width: f64,
    missing_glyphs: bool,
    glyphs: Vec<Glyph>,
}
struct Resident {
    publication: Arc<Publication>,
    epoch: u64,
    glyphs: Vec<(Point<f32>, f32, RasterizedGlyph)>,
}
struct Worker {
    sender: SyncSender<Request>,
    receiver: Receiver<(Key, Arc<Publication>)>,
}

#[derive(Default)]
struct InstanceStorage {
    free: Vec<LabelDrawStorage>,
    job: Option<crate::makepad_platform::thread::TaskHandle<Vec<LabelDrawStorage>>>,
    queue_full_reported: bool,
}

pub struct LabelDrawStorage {
    pub instances: Vec<f32>,
    pub recording: DrawListRecordingStorage,
}

#[derive(Default)]
pub struct LabelCache {
    worker: Option<Worker>,
    texts: HashMap<Arc<str>, LabelText>,
    last_text: Option<LabelText>,
    families: HashSet<FontFamilyId>,
    sent_fonts: HashSet<FontId>,
    waiting: HashSet<Key>,
    unsent: VecDeque<Request>,
    admitting: VecDeque<(Key, Resident)>,
    ready: HashMap<Key, Resident>,
    instance_storage: [InstanceStorage; 32],
}

impl LabelCache {
    fn start(&mut self, cx: &mut Cx) {
        if self.worker.is_some() {
            return;
        }
        let (sender, input) = mpsc::sync_channel::<Request>(128);
        let (output, receiver) = mpsc::sync_channel(128);
        match cx.spawn_worker(move || {
            let mut layouter = Layouter::new(Settings::default());
            let mut glyphs = HashMap::default();
            while let Ok(request) = input.recv() {
                if let Some(family) = request.family {
                    for f in family.fonts {
                        if !layouter.is_font_known(f.id) {
                            layouter.define_font(
                                f.id,
                                FontDefinition {
                                    data: SharedBytes::from_vec(f.data),
                                    index: f.index,
                                    ascender_fudge_in_ems: f.ascender_fudge,
                                    descender_fudge_in_ems: f.descender_fudge,
                                    weight: None,
                                    variations: f.variations,
                                },
                            );
                        }
                    }
                    layouter.define_font_family(
                        family.id,
                        FontFamilyDefinition {
                            expected_member_count: family.font_ids.len(),
                            font_ids: family.font_ids,
                            diagnostics: family.diagnostics,
                        },
                    );
                }
                let result = prepare(&mut layouter, &mut glyphs, &request.key);
                if output.send((request.key, Arc::new(result))).is_err() {
                    break;
                }
            }
        }) {
            Ok(task) => {
                task.detach();
                self.worker = Some(Worker { sender, receiver });
            }
            Err(error) => crate::error!("label worker unavailable: {error}"),
        }
    }

    /// Reserve known scene inventory once, outside incremental label emission.
    pub fn reserve_inventory(&mut self, count: usize) {
        self.texts.reserve(count.saturating_sub(self.texts.len()));
        self.waiting.reserve(count.saturating_sub(self.waiting.len()));
        self.ready.reserve(count.saturating_sub(self.ready.len()));
    }

    fn intern_text(&mut self, text: &str) -> LabelText {
        if let Some(last) = &self.last_text {
            if last.value.as_ref() == text { return last.clone(); }
        }
        let value = if let Some(value) = self.texts.get(text) {
            value.clone()
        } else {
            let value = LabelText { id: self.texts.len() as u64, value: text.into() };
            self.texts.insert(value.value.clone(), value.clone());
            value
        };
        self.last_text = Some(value.clone());
        value
    }

    fn key(&mut self, draw: &DrawText, dpi: f64, text: &str) -> Key {
        Key {
            text: self.intern_text(text),
            family: draw.text_style.font_family_id(),
            size: draw.text_style.font_size.to_bits(),
            dpi: (dpi as f32).to_bits(),
            scale: draw.font_scale.to_bits(),
            spacing: draw.text_style.line_spacing.to_bits(),
        }
    }

    pub fn pending(&self) -> bool {
        !self.waiting.is_empty() || self.instance_storage.iter().any(|pool| pool.job.is_some())
    }

    fn poll_instance_storage(&mut self) {
        for pool in &mut self.instance_storage {
            if let Some(result) = pool.job.as_mut().and_then(|job| job.try_take()) {
                pool.job = None;
                match result {
                    Ok(free) => pool.free = free,
                    Err(error) => crate::error!("label instance storage failed: {error:?}"),
                }
            }
        }
    }

    /// Unique CPU recording storage, prefaulted on Heavy workers. Each batch
    /// is at most32 buffers and normally256KiB; long runs get one buffer.
    /// The caller owns retirement of any replaced recording storage.
    pub fn take_draw_storage(&mut self, cx: &mut Cx2d, draw: &DrawText, text: &str) -> Option<LabelDrawStorage> {
        self.poll_instance_storage();
        let key = self.key(draw, cx.current_dpi_factor(), text);
        let glyphs = self.ready.get(&key)?.glyphs.len();
        let floats = glyphs.checked_mul(draw.draw_vars.as_slice().len())?.max(1).checked_next_power_of_two()?;
        let pool = self.instance_storage.get_mut(floats.trailing_zeros() as usize)?;
        let storage = pool.free.pop();
        if pool.free.is_empty() && pool.job.is_none() {
            use crate::makepad_platform::thread::Lane;
            match cx.task_pool().reserve(Lane::Heavy) {
                Ok(slot) => {
                    pool.queue_full_reported = false;
                    let mut free = std::mem::take(&mut pool.free);
                    pool.job = Some(slot.submit_named("text.label-instance-storage", move || {
                        let count = (65536 / floats).clamp(1, 32);
                        free.reserve(count);
                        for _ in 0..count {
                            let mut buffer = vec![1.0; floats];
                            std::hint::black_box(buffer.as_slice());
                            buffer.clear();
                            free.push(LabelDrawStorage {
                                instances: buffer,
                                recording: DrawListRecordingStorage::new(2),
                            });
                        }
                        free
                    }));
                }
                Err(error) if !pool.queue_full_reported => {
                    pool.queue_full_reported = true;
                    crate::log!("label instance storage queue unavailable; retrying: {error:?}");
                }
                Err(_) => {}
            }
        }
        storage
    }

    /// Drain replies and bind glyph images within 0.5ms. A partially admitted
    /// run remains pending; drawing never pays its first-use raster cost.
    pub fn pump(&mut self, cx: &mut Cx) -> bool {
        self.poll_instance_storage();
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        let start = Instant::now();
        let mut changed = false;
        while let Some(request) = self.unsent.pop_front() {
            match worker.sender.try_send(request) {
                Ok(()) => {}
                Err(TrySendError::Full(request)) => {
                    self.unsent.push_front(request);
                    break;
                }
                Err(TrySendError::Disconnected(request)) => {
                    self.unsent.push_front(request);
                    crate::error!("label worker disconnected with pending labels");
                    break;
                }
            }
            if start.elapsed() >= Duration::from_micros(500) {
                break;
            }
        }
        let fonts = cx
            .has_global::<Rc<RefCell<Fonts>>>()
            .then(|| cx.get_global::<Rc<RefCell<Fonts>>>().clone());
        while start.elapsed() < Duration::from_micros(500) {
            if self.admitting.is_empty() {
                let Ok((key, publication)) = worker.receiver.try_recv() else {
                    break;
                };
                self.admitting.push_back((
                    key,
                    Resident {
                        publication,
                        epoch: u64::MAX,
                        glyphs: Vec::new(),
                    },
                ));
            }
            let (key, run) = self.admitting.front_mut().unwrap();
            if let Some(fonts) = &fonts {
                let fonts = fonts.borrow();
                let mut rasterizer = fonts.rasterizer().borrow_mut();
                if run.epoch != rasterizer.epoch() {
                    run.epoch = rasterizer.epoch();
                    run.glyphs.clear();
                }
                while let Some(glyph) = run.publication.glyphs.get(run.glyphs.len()) {
                    if start.elapsed() >= Duration::from_micros(500) {
                        return changed;
                    }
                    let Some(image) = rasterizer.import_glyph(&glyph.image) else {
                        return changed;
                    };
                    run.glyphs.push((glyph.origin, glyph.size, image));
                }
            }
            self.waiting.remove(key);
            let (key, run) = self.admitting.pop_front().unwrap();
            self.ready.insert(key, run);
            changed = true;
        }
        changed
    }

    pub fn measure(&mut self, cx: &mut Cx2d, draw: &DrawText, text: &str) -> Option<f64> {
        let key = self.key(draw, cx.current_dpi_factor(), text);
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let epoch = fonts.borrow().rasterizer().borrow().epoch();
        if self.ready.get(&key).is_some_and(|r| r.epoch != epoch) {
            let run = self.ready.remove(&key).unwrap();
            self.waiting.insert(key.clone());
            self.admitting.push_back((key.clone(), run));
        }
        if let Some(run) = self.ready.get(&key) {
            let width = run.publication.width;
            if run.publication.missing_glyphs {
                let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
                let mut requested = false;
                for family in [LazyFontFamily::Cjk, LazyFontFamily::Emoji] {
                    requested |= fonts
                        .borrow_mut()
                        .take_lazy_font_request(key.family, family, text, true);
                }
                if requested {
                    draw.text_style.ensure_fonts_loaded(cx);
                    cx.redraw_all();
                }
            }
            return Some(width);
        }
        if self.waiting.contains(&key) {
            return None;
        }
        self.start(cx);
        draw.text_style.ensure_fonts_loaded(cx);
        let family = if self.families.insert(key.family) {
            let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
            let family = fonts.borrow_mut().get_or_load_font_family(key.family);
            Some(FamilySnapshot {
                id: key.family,
                font_ids: family.fonts().iter().map(|f| f.id()).collect(),
                fonts: family
                    .fonts()
                    .iter()
                    // Requests and their family definitions share one FIFO.
                    // Repeated fallback faces need no second full byte copy.
                    .filter(|f| self.sent_fonts.insert(f.id()))
                    .map(|f| f.worker_definition())
                    .collect(),
                diagnostics: family.diagnostics().clone(),
            })
        } else {
            None
        };
        self.waiting.insert(key.clone());
        self.unsent.push_back(Request { key, family });
        None
    }

    /// Place a ready run. A missing run is deferred, never synchronously shaped.
    pub fn draw(&mut self, cx: &mut Cx2d, draw: &mut DrawText, pos: Vec2d, text: &str) -> bool {
        self.draw_with_storage(cx, draw, pos, text, None)
    }

    pub fn draw_with_storage(&mut self, cx: &mut Cx2d, draw: &mut DrawText, pos: Vec2d,
        text: &str, storage: Option<&mut Vec<f32>>) -> bool {
        let key = self.key(draw, cx.current_dpi_factor(), text);
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let epoch = fonts.borrow().rasterizer().borrow().epoch();
        if self.ready.get(&key).is_some_and(|run| run.epoch != epoch) {
            let run = self.ready.remove(&key).unwrap();
            self.waiting.insert(key.clone());
            self.admitting.push_back((key.clone(), run));
        }
        let Some(run) = self.ready.get_mut(&key) else {
            self.measure(cx, draw, text);
            draw.draw_vars.area = Area::Empty;
            return false;
        };
        if run.glyphs.is_empty() {
            draw.draw_vars.area = Area::Empty;
            return true;
        }
        // Reuse the run storage for placement and restore its local coordinates.
        for glyph in &mut run.glyphs {
            glyph.0.x += pos.x as f32;
            glyph.0.y += pos.y as f32;
        }
        draw.draw_rasterized_glyphs_abs_with_storage(cx, &run.glyphs, draw.color, storage);
        for (glyph, local) in run.glyphs.iter_mut().zip(&run.publication.glyphs) {
            glyph.0 = local.origin;
        }
        true
    }
}

fn prepare(
    layouter: &mut Layouter,
    images: &mut HashMap<(FontId, u16, u32), Arc<PublishedGlyph>>,
    key: &Key,
) -> Publication {
    let scale = f32::from_bits(key.scale);
    let dpi = f32::from_bits(key.dpi);
    let laidout = layouter.get_or_layout(BorrowedLayoutParams {
        text: &key.text,
        style: Style {
            font_family_id: key.family,
            font_size_in_pts: f32::from_bits(key.size),
            color: None,
        },
        options: LayoutOptions {
            line_spacing_scale: f32::from_bits(key.spacing),
            ..Default::default()
        },
    });
    let mut result = Publication {
        #[cfg(test)]
        shaped_on: std::thread::current().id(),
        width: (laidout.size_in_lpxs.width * scale) as f64,
        missing_glyphs: laidout
            .rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .any(|g| g.id == 0),
        glyphs: Vec::new(),
    };
    for row in &laidout.rows {
        for glyph in &row.glyphs {
            let size = glyph.font_size_in_lpxs * scale;
            let id = (glyph.font.id(), glyph.id, (size * dpi).to_bits());
            let image = if let Some(image) = images.get(&id) {
                image.clone()
            } else {
                let mut raster = glyph.rasterize(size * dpi);
                if raster.is_none() {
                    let mut rasterizer = layouter.rasterizer().borrow_mut();
                    if rasterizer.color_atlas_mut().reset_if_needed() {
                        rasterizer.on_atlas_reset();
                    }
                    drop(rasterizer);
                    raster = glyph.rasterize(size * dpi);
                }
                let Some(raster) = raster else {
                    continue;
                };
                let Some(image) = layouter.rasterizer().borrow().export_glyph(raster) else {
                    continue;
                };
                let image = Arc::new(image);
                images.insert(id, image.clone());
                image
            };
            result.glyphs.push(Glyph {
                origin: Point::new(
                    (row.origin_in_lpxs.x + glyph.origin_in_lpxs.x + glyph.offset_in_lpxs())
                        * scale,
                    (row.origin_in_lpxs.y + glyph.origin_in_lpxs.y) * scale,
                ),
                size: glyph.font_size_in_lpxs,
                image,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_cache_worker_shapes_exact_runs_and_ui_only_places_publications() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let family = FontFamilyId::from(0x50414e01_u64);
        let font = FontId::from(0x50414e02_u64);
        let bytes = include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
        let mut cache = LabelCache::default();
        let key = Key {
            text: cache.intern_text("office · café → libs/"),
            family,
            size: 7.5_f32.to_bits(),
            dpi: 2.0_f32.to_bits(),
            scale: 1.0_f32.to_bits(),
            spacing: 1.0_f32.to_bits(),
        };
        cache.start(&mut cx);
        cache.waiting.insert(key.clone());
        cache.unsent.push_back(Request {
            key: key.clone(),
            family: Some(FamilySnapshot {
                id: family,
                font_ids: vec![font],
                diagnostics: FontDiagnostics::default(),
                fonts: vec![FontSnapshot {
                    id: font,
                    data: bytes.to_vec(),
                    index: 0,
                    variations: Vec::new(),
                    ascender_fudge: 0.03,
                    descender_fudge: -0.02,
                }],
            }),
        });
        let start = Instant::now();
        while cache.pending() && start.elapsed() < Duration::from_secs(10) {
            cache.pump(&mut cx);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !cache.pending(),
            "bounded channel must drain without worker signals"
        );
        let run = &cache.ready[&key].publication;
        assert_ne!(run.shaped_on, std::thread::current().id());
        let mut reference = Layouter::new(Settings::default());
        reference.define_font(
            font,
            FontDefinition {
                data: SharedBytes::from_vec(bytes.to_vec()),
                index: 0,
                variations: Vec::new(),
                weight: None,
                ascender_fudge_in_ems: 0.03,
                descender_fudge_in_ems: -0.02,
            },
        );
        reference.define_font_family(
            family,
            FontFamilyDefinition {
                font_ids: vec![font],
                expected_member_count: 1,
                diagnostics: FontDiagnostics::default(),
            },
        );
        let expected = prepare(&mut reference, &mut HashMap::default(), &key);
        assert_eq!(run.width, expected.width);
        assert_eq!(run.glyphs.len(), expected.glyphs.len());
        let mut ui_raster =
            super::super::rasterizer::Rasterizer::new(Settings::default().loader.rasterizer);
        for (a, b) in run.glyphs.iter().zip(&expected.glyphs) {
            assert_eq!(a.origin, b.origin);
            assert_eq!(a.size, b.size);
            let resident = ui_raster.import_glyph(&a.image).unwrap();
            let again = ui_raster.import_glyph(&a.image).unwrap();
            assert_eq!(resident.atlas_image_bounds, again.atlas_image_bounds);
        }
        // DPI and size are distinct publications; camera position is absent from the key.
        let mut next = key.clone();
        next.dpi = 1.0_f32.to_bits();
        assert!(!cache.ready.contains_key(&next));
        next = key.clone();
        next.size = 8.0_f32.to_bits();
        assert!(!cache.ready.contains_key(&next));
    }
}
