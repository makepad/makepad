//! Text drawn as glyphs for a document writer, with final layout positions and clips.
//! Glyph-only draws, including prepared and rasterized glyph draws, have no text to
//! capture: they stay in the pixels and are not recorded. A widget that draws glyphs
//! from their outlines itself (a typeset formula) records them as an outline row: the
//! glyphs as shapes, and the text they picture (the formula's source) to set invisible
//! over them.

use {
    crate::{
        makepad_platform::*,
        text::{
            font::{Font, FontId},
            font_face::FontData,
        },
        Cx2d,
    },
    fxhash::FxHashMap,
    std::ops::Range,
};

/// Text drawn while `cx.glyph_capture` is on, as glyphs a document writer can set
/// as text. Only stock text in embeddable faces is left out of the pixels.
pub struct GlyphCapture {
    record: bool,
    stock_shader: Option<DrawShaderId>,
    face_indices: FxHashMap<FontId, u32>,
    faces: Vec<CapturedFace>,
    rows: Vec<PendingRow>,
    dpi: Option<f32>,
}

struct PendingRow {
    area: Area,
    drawn: Rect,
    row: CapturedRow,
}

/// What a recording capture saw, in the draw's logical units and final positions.
/// It owns its text and shares only font bytes, so a writer can send it to a worker.
pub struct CapturedText {
    pub faces: Vec<CapturedFace>,
    pub rows: Vec<CapturedRow>,
}

/// A face used by captured text, including the variations and embedding permissions.
pub struct CapturedFace {
    /// The font file's shared bytes; cloning this buffer does not copy the file.
    pub data: FontData,
    /// The face's index in its font collection.
    pub index: u32,
    /// The variation coordinates the face draws at, as `(tag, value)` pairs.
    pub coords: Vec<(u32, f32)>,
    /// The face's permissions allow embedding a subset of its outlines.
    pub embeddable: bool,
}

/// A laid-out row with its own text, glyphs and final clip.
pub struct CapturedRow {
    pub text: String,
    pub glyphs: Vec<CapturedGlyph>,
    /// The row's glyphs draw as flat fills in their colour (the stock text shader, or an
    /// outline row), even if its faces could not be embedded.
    pub plain: bool,
    /// An outline row's text: its glyphs only picture it (a formula's source), so a
    /// writer draws them as shapes and sets this text invisible over them.
    pub pictured: Option<CapturedPictured>,
    /// The final clip, or a rect larger than the drawing when nothing clips it.
    pub clip: Rect,
}

/// The text an outline row's glyphs picture, set on one line over them.
pub struct CapturedPictured {
    pub text: String,
    /// Index into `CapturedText::faces`: the face to set the text in.
    pub face: u32,
    /// Em size in the draw's logical units.
    pub size: f32,
    /// The left end of the line and its baseline, at their final positions.
    pub x: f32,
    pub baseline: f32,
    /// The width the text spans: the picture's.
    pub width: f32,
}

/// The text an outline row pictures, for [`GlyphCapture::begin_outline_row`], in the
/// draw's logical units.
pub struct PicturedText<'a> {
    pub text: &'a str,
    /// A face to set the text in, and its em size.
    pub font: &'a Font,
    pub size: f32,
    /// The left end of the line the text spans and its baseline.
    pub x: f32,
    pub baseline: f32,
    /// The width the text spans.
    pub width: f32,
}

/// A glyph a widget draws from its outline, for [`GlyphCapture::outline_glyph`].
pub struct OutlineGlyph {
    pub id: u16,
    /// The pen position on the baseline, in the draw's logical units.
    pub x: f32,
    pub baseline: f32,
    /// Em size in logical units.
    pub size: f32,
    /// Straight RGBA.
    pub color: [f32; 4],
}

/// A glyph at its final origin and baseline, with the text its cluster represents.
pub struct CapturedGlyph {
    /// Index into `CapturedText::faces`.
    pub face: u32,
    pub id: u16,
    /// The pen position plus the glyph's x offset, after alignment.
    pub x: f32,
    /// The final baseline, snapped to a device pixel as the text shader snaps it.
    pub baseline: f32,
    /// Em size in the draw's logical units, including the font scale.
    pub size: f32,
    /// Straight RGBA from the glyph's own colour or the draw's current colour.
    pub color: [f32; 4],
    /// The cluster's byte range in its row's text; empty for an ellipsis.
    pub text: Range<usize>,
    pub rtl: bool,
    /// The glyph remains in the pixels, so a writer should add it as invisible text.
    pub in_pixels: bool,
}

impl GlyphCapture {
    /// With `record`, keeps one draw's text; otherwise only leaves eligible glyphs
    /// out of the pixels on repeated draws. `stock_shader` comes from `stock_text_shader`.
    pub fn new(record: bool, stock_shader: Option<DrawShaderId>) -> Self {
        Self {
            record,
            stock_shader,
            face_indices: FxHashMap::default(),
            faces: Vec::new(),
            rows: Vec::new(),
            dpi: None,
        }
    }

    /// Whether this capture keeps what is drawn.
    pub fn records(&self) -> bool {
        self.record
    }

    /// Finishes after the draw's turtles have ended, reading each row's final
    /// position and clip from its aligned rect area. Returns `None` without recording.
    pub fn finish(self, cx: &Cx) -> Option<CapturedText> {
        if !self.record {
            return None;
        }
        let dpi = self.dpi.unwrap_or(1.0).max(0.0001);
        let mut rows = Vec::with_capacity(self.rows.len());
        for pending in self.rows {
            let live = match pending.area {
                Area::Rect(area) => cx.draw_lists.checked_index(area.draw_list_id).and_then(|list| {
                    if list.redraw_id != area.redraw_id {
                        return None;
                    }
                    list.rect_areas.get(area.rect_id)
                }),
                _ => None,
            };
            let Some(live) = live else {
                log!("glyph capture: dropping a row whose aligned rect area is no longer live");
                continue;
            };
            let shift = live.rect.pos - pending.drawn.pos;
            let mut row = pending.row;
            row.clip = Rect { pos: live.draw_clip.0, size: live.draw_clip.1 - live.draw_clip.0 };
            for glyph in &mut row.glyphs {
                glyph.x += shift.x as f32;
                let baseline = glyph.baseline + shift.y as f32;
                glyph.baseline = (baseline * dpi + 0.5).floor() / dpi;
            }
            if let Some(pictured) = &mut row.pictured {
                pictured.x += shift.x as f32;
                pictured.baseline += shift.y as f32;
            }
            rows.push(row);
        }
        Some(CapturedText { faces: self.faces, rows })
    }

    pub(crate) fn is_plain(&self, shader: Option<DrawShaderId>) -> bool {
        self.stock_shader.is_some() && shader == self.stock_shader
    }

    pub(crate) fn face(&mut self, font: &Font) -> (u32, bool) {
        let index = *self.face_indices.entry(font.id()).or_insert_with(|| {
            let embeddable = font.with_ttf_parser_face(|face| {
                face.tables().os2.is_none()
                    || (!matches!(face.permissions(), Some(ttf_parser::Permissions::Restricted))
                        && face.is_subsetting_allowed()
                        && face.is_outline_embedding_allowed())
            });
            let index = self.faces.len() as u32;
            self.faces.push(CapturedFace {
                data: font.data().clone(),
                index: font.face().index(),
                coords: font.variation_coords(),
                embeddable,
            });
            index
        });
        (index, self.faces[index as usize].embeddable)
    }

    pub(crate) fn begin_row(
        &mut self,
        cx: &mut Cx2d,
        drawn: Rect,
        text: &str,
        plain: bool,
    ) -> Option<usize> {
        if !self.record {
            return None;
        }
        if self.dpi.is_none() {
            self.dpi = Some(cx.current_dpi_factor() as f32);
        }
        let mut area = Area::Empty;
        cx.add_aligned_rect_area(&mut area, drawn);
        if let Area::Rect(area) = area {
            cx.draw_lists[area.draw_list_id].rect_areas[area.rect_id].draw_clip =
                (dvec2(-1.0e9, -1.0e9), dvec2(1.0e9, 1.0e9));
        }
        let index = self.rows.len();
        self.rows.push(PendingRow {
            area,
            drawn,
            row: CapturedRow { text: text.to_string(), glyphs: Vec::new(), plain, pictured: None, clip: Rect::default() },
        });
        Some(index)
    }

    pub(crate) fn push_glyph(&mut self, row: usize, glyph: CapturedGlyph) {
        self.rows[row].row.glyphs.push(glyph);
    }

    /// Starts a row of glyphs a widget draws from their outlines itself rather than
    /// through `DrawText`, each a flat fill in its colour, that picture `pictured` (a
    /// typeset formula and its source): `drawn` is the row's rect as drawn. Answers the
    /// row for [`Self::outline_glyph`], or `None` when this draw is not recorded.
    pub fn begin_outline_row(&mut self, cx: &mut Cx2d, drawn: Rect, pictured: PicturedText) -> Option<usize> {
        if !self.record {
            return None;
        }
        let (face, _) = self.face(pictured.font);
        let row = self.begin_row(cx, drawn, "", true)?;
        self.rows[row].row.pictured = Some(CapturedPictured {
            text: pictured.text.to_string(),
            face,
            size: pictured.size,
            x: pictured.x,
            baseline: pictured.baseline,
            width: pictured.width,
        });
        Some(row)
    }

    /// A glyph of an outline row, recorded in `row` while recording. Answers whether
    /// the writer draws it as a shape and leaves it out of the pixels (its face allows
    /// embedding): the widget then does not draw it. A page's draws all get the same
    /// answer for a glyph, recorded or not.
    pub fn outline_glyph(&mut self, row: Option<usize>, font: &Font, glyph: OutlineGlyph) -> bool {
        let (face, embeddable) = self.face(font);
        if let Some(row) = row {
            self.push_glyph(row, CapturedGlyph {
                face,
                id: glyph.id,
                x: glyph.x,
                baseline: glyph.baseline,
                size: glyph.size,
                color: glyph.color,
                text: 0..0,
                rtl: false,
                in_pixels: !embeddable,
            });
        }
        embeddable
    }
}
