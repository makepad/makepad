//! Shaping stays here; the HAL owns all rasterisation and atlas storage.
use makepad_font_core::{
    font::{Font, FontId},
    font_face::{FontData, FontFace},
    rustybuzz,
};
use makepad_hal::{
    font::{wire, FontSource, GlyphEntry},
    CmdWriter, HalEvent, Id,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const FONT_ID: u32 = 1;
// The existing bundled asset, without linking any widget code or copying fonts.
const FONT: &[u8] = include_bytes!("../../../../widgets/resources/IBMPlexSans-Text.ttf");
type Key = (u32, u32); // glyph id, exact requested pixels/em bits

pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub row_width: f32,
}

pub struct Shaped {
    pub glyphs: Vec<Glyph>,
    pub rows: usize,
    pub width: f32,
    row_starts: Vec<usize>,
    baseline: [f32; 2],
}

impl Shaped {
    pub fn visible(&self, top: f32, bottom: f32, size: f32, height: f32) -> &[Glyph] {
        let first = ((top - self.baseline[1] - size) / height).floor().max(0.0) as usize;
        let end = (((bottom - self.baseline[0] + size) / height).floor() + 1.0).max(0.0) as usize;
        &self.glyphs
            [self.row_starts[first.min(self.rows)]..self.row_starts[end.max(first).min(self.rows)]]
    }
}

#[derive(Hash, PartialEq, Eq)]
struct RunKey {
    text: String,
    size: u32,
    width: u32,
    height: u32,
}
struct Cached {
    shaped: Arc<Shaped>,
    used: bool,
}

pub struct Text {
    font: Font,
    opened: bool,
    atlas: Id,
    atlas_size: [f32; 2],
    generation: u32,
    ready: HashMap<Key, GlyphEntry>,
    pending: HashSet<Key>,
    used: HashSet<Key>,
    runs: HashMap<RunKey, Cached>,
}

impl Text {
    pub fn new(writer: &mut CmdWriter) -> Result<Self, String> {
        let face = FontFace::from_data_and_index(FontData::from_vec(FONT.to_vec()), 0)
            .ok_or("Invalid bundled boot font")?;
        let font = Font::new(FontId::from("relax-boot"), face, 0.0, 0.0);
        writer.open_font(FONT_ID, 0, &[], FontSource::Bytes(FONT));
        Ok(Self {
            font,
            opened: false,
            atlas: Id::NONE,
            atlas_size: [1.0; 2],
            generation: 0,
            ready: HashMap::new(),
            pending: HashSet::new(),
            used: HashSet::new(),
            runs: HashMap::new(),
        })
    }

    pub fn event(&mut self, event: &HalEvent<'_>) -> Result<bool, String> {
        match *event {
            HalEvent::FontOpened {
                font: FONT_ID,
                status,
                ..
            } => {
                if status != wire::font_status::OPENED {
                    return Err(format!(
                        "HAL could not open the boot font (status {status})"
                    ));
                }
                self.opened = true;
            }
            HalEvent::AtlasInfo {
                kind: wire::atlas::RASTER,
                texture,
                w,
                h,
                generation,
            } => {
                if self.generation != 0 && self.generation != generation {
                    self.ready.clear();
                    self.pending.clear();
                }
                self.generation = generation;
                self.atlas = texture;
                self.atlas_size = [w as f32, h as f32];
            }
            HalEvent::GlyphsReady {
                font: FONT_ID,
                count,
                entries,
            } => {
                for entry in wire::read_entries(entries, count) {
                    if entry.kind != wire::kind::SDF {
                        continue;
                    }
                    let key = (entry.glyph, entry.dpxs_bits);
                    self.pending.remove(&key);
                    self.ready.insert(key, entry);
                }
            }
            HalEvent::AtlasEvicted {
                kind: wire::atlas::RASTER,
                generation,
                count,
                slots,
            } => {
                if generation == self.generation {
                    let slots: HashSet<_> = wire::read_slots(slots, count).into_iter().collect();
                    self.ready
                        .retain(|_, entry| !slots.contains(&(entry.slot, entry.slot_gen)));
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn begin_layout(&mut self) {
        for run in self.runs.values_mut() {
            run.used = false;
        }
    }

    pub fn end_layout(&mut self) {
        self.runs.retain(|_, run| run.used);
    }

    pub fn shape(&mut self, text: &str, size: f32, width: f32, height: f32) -> Arc<Shaped> {
        let key = RunKey {
            text: text.to_owned(),
            size: size.to_bits(),
            width: width.to_bits(),
            height: height.to_bits(),
        };
        if let Some(run) = self.runs.get_mut(&key) {
            run.used = true;
            return run.shaped.clone();
        }
        let scale = size / self.font.units_per_em();
        let mut glyphs = Vec::<Glyph>::new();
        let mut row = 0;
        let mut row_starts = vec![0];
        let mut run_width = 0.0_f32;
        let ascender = self.font.ascender_in_ems() * size;
        let mut baseline = [ascender; 2];
        for paragraph in text.split('\n') {
            let shaped = self.font.with_rustybuzz_face(|face| {
                let mut input = rustybuzz::UnicodeBuffer::new();
                input.push_str(paragraph);
                input.guess_segment_properties();
                rustybuzz::shape(face, &[], input)
            });
            let infos = shaped.glyph_infos();
            let positions = shaped.glyph_positions();
            // Keep shaping clusters intact. Prefer whitespace breaks; a single
            // long path/identifier still wraps at clusters instead of overflowing.
            let mut clusters = Vec::new();
            let mut i = 0;
            while i < infos.len() {
                let mut end = i + 1;
                while end < infos.len() && infos[end].cluster == infos[i].cluster {
                    end += 1;
                }
                let advance = positions[i..end]
                    .iter()
                    .map(|p| p.x_advance as f32 * scale)
                    .sum::<f32>();
                let space = paragraph
                    .get(infos[i].cluster as usize..)
                    .and_then(|s| s.chars().next())
                    .is_some_and(char::is_whitespace);
                clusters.push((i, end, advance, space));
                i = end;
            }
            let mut x = 0.0_f32;
            let mut cluster = 0;
            while cluster < clusters.len() {
                let mut end = cluster + 1;
                if !clusters[cluster].3 {
                    while end < clusters.len() && !clusters[end].3 {
                        end += 1;
                    }
                }
                let word_width = clusters[cluster..end].iter().map(|c| c.2).sum::<f32>();
                let wrap_word = x > 0.0 && !clusters[cluster].3 && x + word_width > width;
                if wrap_word {
                    run_width = run_width.max(x);
                    for glyph in &mut glyphs[*row_starts.last().unwrap()..] {
                        glyph.row_width = x;
                    }
                    x = 0.0;
                    row += 1;
                    row_starts.push(glyphs.len());
                }
                for &(start, end, advance, space) in &clusters[cluster..end] {
                    if space && (x == 0.0 || x + advance > width) {
                        continue;
                    }
                    if x > 0.0 && x + advance > width {
                        run_width = run_width.max(x);
                        for glyph in &mut glyphs[*row_starts.last().unwrap()..] {
                            glyph.row_width = x;
                        }
                        x = 0.0;
                        row += 1;
                        row_starts.push(glyphs.len());
                    }
                    for j in start..end {
                        let offset = ascender - positions[j].y_offset as f32 * scale;
                        baseline[0] = baseline[0].min(offset);
                        baseline[1] = baseline[1].max(offset);
                        glyphs.push(Glyph {
                            id: infos[j].glyph_id,
                            x: x + positions[j].x_offset as f32 * scale,
                            y: row as f32 * height + offset,
                            row_width: 0.0,
                        });
                        x += positions[j].x_advance as f32 * scale;
                    }
                }
                cluster = end;
            }
            for glyph in &mut glyphs[*row_starts.last().unwrap()..] {
                glyph.row_width = x;
            }
            row += 1;
            run_width = run_width.max(x);
            row_starts.push(glyphs.len());
        }
        let shaped = Arc::new(Shaped {
            glyphs,
            rows: row,
            width: run_width,
            row_starts,
            baseline,
        });
        self.runs.insert(
            key,
            Cached {
                shaped: shaped.clone(),
                used: true,
            },
        );
        shaped
    }

    pub fn begin_frame(&mut self) {
        self.used.clear();
    }

    pub fn lookup(
        &mut self,
        glyph: u32,
        size: f32,
        dpi: f32,
        request: &mut Vec<(u32, f32)>,
    ) -> Option<GlyphEntry> {
        let dpxs = size * dpi;
        let key = (glyph, dpxs.to_bits());
        self.used.insert(key);
        if let Some(entry) = self.ready.get(&key) {
            return Some(*entry);
        }
        if self.opened && self.pending.insert(key) {
            request.push((glyph, dpxs));
        }
        None
    }

    pub fn finish_frame(&mut self, writer: &mut CmdWriter, requests: &[(u32, f32)]) -> bool {
        if !requests.is_empty() {
            writer.request_glyphs(FONT_ID, wire::kind::SDF, requests);
        }
        // Only current visible text pins slots. Size changes and scrolling release
        // old claims; HAL retirement fences protect already submitted frames.
        let live: HashSet<_> = self
            .ready
            .iter()
            .filter(|(key, _)| self.used.contains(*key))
            .map(|(_, entry)| (entry.slot, entry.slot_gen))
            .collect();
        let mut release = HashSet::new();
        self.ready.retain(|key, entry| {
            let keep = self.used.contains(key);
            if !keep && entry.slot != 0 && !live.contains(&(entry.slot, entry.slot_gen)) {
                release.insert((entry.slot, entry.slot_gen));
            }
            keep
        });
        if !release.is_empty() {
            writer.release_glyphs(&release.into_iter().collect::<Vec<_>>());
        }
        self.opened
            && !self.atlas.is_none()
            && self.used.iter().all(|key| self.ready.contains_key(key))
    }

    pub fn atlas(&self) -> Id {
        self.atlas
    }

    /// Identical raster metric convention to DrawText: origin is relative to
    /// the baseline, with padding around the host's raster rectangle.
    pub fn quad(&self, entry: GlyphEntry, baseline: [f32; 2], size: f32) -> ([f32; 4], [f32; 4]) {
        let scale = size / entry.scale;
        let [x, y, w, h] = entry.rect.map(|v| v as f32);
        let padding = entry.padding as f32;
        let rect = [
            baseline[0] + (entry.origin[0] - padding) * scale,
            baseline[1] + (-entry.origin[1] - h + padding) * scale,
            w * scale,
            h * scale,
        ];
        let [aw, ah] = self.atlas_size;
        (rect, [x / aw, y / ah, (x + w) / aw, (y + h) / ah])
    }
}
