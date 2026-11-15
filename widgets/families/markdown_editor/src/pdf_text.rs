//! Captured page text and the font programs its PDF needs.

use makepad_font_subset::static_instance;
use makepad_pdf_write::{
    FontId, FontMetrics, Glyph, PathCommand, PdfWriter, Rect, Replacement, RunTag, TextMode, TextRun, Type3Glyph,
};
use makepad_widgets_core::makepad_draw::{
    glyph_capture::{CapturedFace, CapturedPictured, CapturedRow, CapturedText},
    text::{
        font_face::{FontData, FontFace},
        glyph_outline::Command,
        glyph_raster_image::GlyphRasterImage,
        image::{Bgra, Image},
    },
};
use crate::pdf_structure::PageTags;
use makepad_widgets_core::Cx;
use std::{collections::HashMap, ops::Range};

/// Turns pages of captured text into PDF runs, then supplies their used font programs.
/// It travels with the writer between workers, holding only shared bytes and plain data.
pub(crate) struct PdfText {
    indices: HashMap<FaceKey, usize>,
    faces: Vec<Face>,
    stats: TextStats,
}

/// Counts the text and fonts written so a print job can report its work.
#[derive(Clone, Copy, Default)]
pub(crate) struct TextStats {
    pub runs: usize,
    pub glyphs: usize,
    /// Glyphs written invisible because their pixels stayed in the picture.
    pub invisible: usize,
    /// Glyphs left out because they lie outside their row's clip: wholly beside it, or with
    /// the middle of their line box past its top or bottom (a page break's neighbour line).
    pub clipped: usize,
    pub sfnt_fonts: usize,
    pub type3_fonts: usize,
    /// Bytes of font programs and Type 3 procedures before Flate; outlines count as zero.
    pub font_bytes: usize,
    /// Seconds spent building the fonts at finish.
    pub font_time: f64,
    /// Text lines owned by a structure element, marked as pagination artifacts, or left
    /// unmarked (an untagged document's lines).
    pub tagged_lines: usize,
    pub artifact_lines: usize,
    pub untagged_lines: usize,
    /// Right-to-left stretches given one replacement text in reading order.
    pub rtl_stretches: usize,
    /// Rows whose glyphs picture a text (formulas), and the glyphs drawn as shapes for them.
    pub pictured_rows: usize,
    pub shape_glyphs: usize,
}

#[derive(Eq, Hash, PartialEq)]
struct FaceKey {
    address: usize,
    length: usize,
    index: u32,
    coords: Vec<(u32, u32)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Invisible,
    Bitmap,
    Sfnt,
    Outline,
}

struct Face {
    data: FontData,
    index: u32,
    coords: Vec<(u32, f32)>,
    font: FontId,
    name: String,
    upem: f32,
    ascent: f32,
    descent: f32,
    /// The face's space glyph, which the shaper also uses for default-ignorable characters.
    space: Option<u16>,
    kind: Kind,
    advances: HashMap<u16, u16>,
    /// The Type 3 registration an embedded sfnt face's glyphs drawn as shapes take their
    /// outlines from; any other face draws its shapes from its own procedures.
    shapes: Option<FontId>,
}

impl Face {
    /// The font a run of this face's glyphs drawn as shapes uses.
    fn shape_font(&mut self, writer: &mut PdfWriter) -> FontId {
        if self.kind != Kind::Sfnt {
            return self.font;
        }
        *self.shapes.get_or_insert_with(|| writer.add_type3_font(FontMetrics {
            postscript_name: self.name.clone(),
            units_per_em: self.upem as u16,
            ascent: self.ascent as i16,
            descent: self.descent as i16,
            cap_height: 0,
            bbox: [0; 4],
        }))
    }

    /// The glyph's advance in font units, read once.
    fn advance(&mut self, parsed: &FontFace, id: u16) -> u16 {
        *self.advances.entry(id).or_insert_with(|| {
            parsed.with_ttf_parser_face(|face| face.glyph_hor_advance(ttf_parser::GlyphId(id)).unwrap_or(0))
        })
    }
}

impl PdfText {
    /// An empty document registry, ready to collect the faces its pages use.
    pub(crate) fn new() -> Self {
        Self { indices: HashMap::new(), faces: Vec::new(), stats: TextStats::default() }
    }

    fn register(&mut self, writer: &mut PdfWriter, captured: &CapturedFace, parsed: &FontFace) -> usize {
        let key = FaceKey {
            address: captured.data.as_slice().as_ptr() as usize,
            length: captured.data.len(),
            index: captured.index,
            coords: captured.coords.iter().map(|&(tag, value)| (tag, value.to_bits())).collect(),
        };
        if let Some(&index) = self.indices.get(&key) {
            return index;
        }
        let index = self.faces.len();
        let (metrics, kind, space) = parsed.with_ttf_parser_face(|face| {
            let name = face.names().into_iter()
                .filter(|name| name.name_id == 6)
                .find_map(|name| name.to_string().or_else(|| {
                    // A PostScript name can also be ASCII in a legacy platform record.
                    (!name.is_unicode() && name.name.is_ascii())
                        .then(|| String::from_utf8_lossy(name.name).into_owned())
                }))
                .unwrap_or_else(|| format!("Face{index}"));
            let bbox = face.global_bounding_box();
            let metrics = FontMetrics {
                postscript_name: name,
                units_per_em: face.units_per_em(),
                ascent: face.ascender(),
                descent: face.descender(),
                cap_height: face.capital_height().unwrap_or(0),
                bbox: [bbox.x_min, bbox.y_min, bbox.x_max, bbox.y_max],
            };
            let tables = face.tables();
            let kind = if !captured.embeddable {
                Kind::Invisible
            } else if tables.sbix.is_some() || tables.bdat.is_some() || tables.ebdt.is_some() || tables.cbdt.is_some() {
                Kind::Bitmap
            } else if tables.glyf.is_some()
                && static_instance(captured.data.as_slice(), captured.index, &captured.coords, &[]).is_ok()
            {
                Kind::Sfnt
            } else {
                Kind::Outline
            };
            (metrics, kind, face.glyph_index(' ').map(|glyph| glyph.0))
        });
        let name = metrics.postscript_name.clone();
        let upem = f32::from(metrics.units_per_em);
        let ascent = f32::from(metrics.ascent);
        let descent = f32::from(metrics.descent);
        let font = if kind == Kind::Sfnt {
            self.stats.sfnt_fonts += 1;
            writer.add_font(metrics)
        } else {
            self.stats.type3_fonts += 1;
            writer.add_type3_font(metrics)
        };
        self.faces.push(Face {
            data: captured.data.clone(), index: captured.index, coords: captured.coords.clone(),
            font, name, upem, ascent, descent, space, kind, advances: HashMap::new(), shapes: None,
        });
        self.indices.insert(key, index);
        index
    }

    /// This page's runs, registering each face with the writer on its first page. With `tags`
    /// each line goes to the structure element (or artifact) its first glyph lies in.
    pub(crate) fn page_runs(&mut self, writer: &mut PdfWriter, text: &CapturedText, tags: Option<&PageTags>) -> Vec<TextRun> {
        // Parsed faces never leave this worker task. The document retains their bytes.
        let faces: Vec<_> = text.faces.iter().map(|captured| {
            let mut parsed = FontFace::from_data_and_index(captured.data.clone(), captured.index)
                .expect("a captured font face must parse");
            parsed.set_variations(&captured.coords);
            let index = self.register(writer, captured, &parsed);
            (index, parsed)
        }).collect();
        let mut runs: Vec<TextRun> = Vec::new();
        let mut placed = Vec::new();
        for row in &text.rows {
            if let Some(pictured) = &row.pictured {
                self.pictured_row(writer, &faces, row, pictured, tags, &mut runs);
                continue;
            }
            let first_run = runs.len();
            placed.clear();
            let mut tag = None;
            // The cluster whose text is still owed to its first glyph in the PDF.
            let mut owed = None;
            for (index, glyph) in row.glyphs.iter().enumerate() {
                let (face_index, parsed) = &faces[glyph.face as usize];
                let face = &mut self.faces[*face_index];
                if index == 0 || row.glyphs[index - 1].text != glyph.text {
                    owed = Some(glyph.text.clone());
                }
                // The shaper draws a default-ignorable character (a soft hyphen that does not break,
                // a joiner, a variation selector) as the face's space glyph with no advance. It paints
                // nothing, and in a PDF a reader would take it for a space between words.
                let text = &row.text[glyph.text.clone()];
                if Some(glyph.id) == face.space && !text.is_empty() && !text.chars().any(char::is_whitespace) {
                    continue;
                }
                let advance = face.advance(parsed, glyph.id);
                let line = LineBox::new(face, glyph.x, glyph.baseline, f32::from(advance) * glyph.size / face.upem, glyph.size);
                let Some(clip) = line.clip(row) else {
                    self.stats.clipped += 1;
                    continue;
                };
                let (x0, x1, middle) = (line.x0, line.x1, line.middle());
                let invisible = glyph.in_pixels || face.kind == Kind::Invisible;
                let mode = if invisible { TextMode::Invisible } else { TextMode::Fill };
                let color = if invisible { [0.0, 0.0, 0.0, 1.0] } else { glyph.color.map(|value| value.clamp(0.0, 1.0)) };
                // A run keeps one direction, so a right-to-left stretch is whole runs.
                let continues = runs.len() > first_run && runs.last().is_some_and(|run| {
                    run.font == face.font && run.size.to_bits() == glyph.size.to_bits()
                        && run.color == color && run.mode == mode && run.clip == clip
                }) && placed.last().is_some_and(|placed: &Placed| placed.rtl == glyph.rtl);
                // The line's first kept glyph places it in the structure: the middle of its em box.
                let tag = *tag.get_or_insert_with(|| {
                    let tag = tags.map_or(RunTag::None, |tags| tags.tag(((x0 + x1) * 0.5) as f32, middle as f32));
                    match tag {
                        RunTag::Element(_) => self.stats.tagged_lines += 1,
                        RunTag::Artifact(_) => self.stats.artifact_lines += 1,
                        RunTag::None => self.stats.untagged_lines += 1,
                    }
                    tag
                });
                if !continues {
                    runs.push(TextRun {
                        font: face.font, size: glyph.size, color, origin: [glyph.x, glyph.baseline],
                        clip, mode, glyphs: Vec::new(), stretch: 1.0, tag, replacement: Replacement::None,
                    });
                }
                // The cluster's first glyph in the PDF carries its text.
                let value = match owed.take() {
                    None => "",
                    Some(range) if range.is_empty() => "\u{2026}",
                    Some(range) => &row.text[range],
                };
                let run = runs.last_mut().unwrap();
                run.glyphs.push(Glyph {
                    id: glyph.id, x: glyph.x - run.origin[0], y: glyph.baseline - run.origin[1],
                    advance, text: value.to_string(),
                });
                placed.push(Placed { run: runs.len() - 1, rtl: glyph.rtl, text: glyph.text.clone() });
                self.stats.glyphs += 1;
                self.stats.invisible += usize::from(invisible);
            }
            self.stats.rtl_stretches += logical_rtl_text(&mut runs, &placed, &row.text);
        }
        self.stats.runs += runs.len();
        runs
    }

    /// A row whose glyphs picture a text (a typeset formula and its source): the glyphs as
    /// shapes, which hold no text, then the text set invisible on the picture's baseline and
    /// stretched to its width, for a reader to copy and search in line order. Both go to the
    /// structure element (a formula) the text's line lies in.
    fn pictured_row(
        &mut self,
        writer: &mut PdfWriter,
        faces: &[(usize, FontFace)],
        row: &CapturedRow,
        pictured: &CapturedPictured,
        tags: Option<&PageTags>,
        runs: &mut Vec<TextRun>,
    ) {
        let (source_index, source_parsed) = &faces[pictured.face as usize];
        let source = &mut self.faces[*source_index];
        let line = LineBox::new(source, pictured.x, pictured.baseline, pictured.width, pictured.size);
        let tag = tags.map_or(RunTag::None, |tags| tags.tag(((line.x0 + line.x1) * 0.5) as f32, line.middle() as f32));
        let first_run = runs.len();
        for glyph in &row.glyphs {
            // A glyph whose face may not be embedded stays in the picture.
            if glyph.in_pixels {
                continue;
            }
            let (face_index, parsed) = &faces[glyph.face as usize];
            let face = &mut self.faces[*face_index];
            let advance = face.advance(parsed, glyph.id);
            let glyph_line = LineBox::new(face, glyph.x, glyph.baseline, f32::from(advance) * glyph.size / face.upem, glyph.size);
            let Some(clip) = glyph_line.clip(row) else {
                self.stats.clipped += 1;
                continue;
            };
            let font = face.shape_font(writer);
            let color = glyph.color.map(|value| value.clamp(0.0, 1.0));
            let continues = runs.len() > first_run && runs.last().is_some_and(|run| {
                run.font == font && run.size.to_bits() == glyph.size.to_bits() && run.color == color && run.clip == clip
            });
            if !continues {
                runs.push(TextRun {
                    font, size: glyph.size, color, origin: [glyph.x, glyph.baseline], clip, mode: TextMode::Outlines,
                    glyphs: Vec::new(), stretch: 1.0, tag, replacement: Replacement::None,
                });
            }
            let run = runs.last_mut().unwrap();
            run.glyphs.push(Glyph {
                id: glyph.id, x: glyph.x - run.origin[0], y: glyph.baseline - run.origin[1], advance, text: String::new(),
            });
            self.stats.shape_glyphs += 1;
        }
        let source = &mut self.faces[*source_index];
        let mut glyphs = Vec::new();
        let mut pen = 0.0;
        for ch in pictured.text.chars() {
            // A line break in the source reads as a space on the one line.
            let ch = if ch.is_whitespace() { ' ' } else { ch };
            let id = source_parsed.with_ttf_parser_face(|face| face.glyph_index(ch).map_or(0, |id| id.0));
            let advance = source.advance(source_parsed, id);
            glyphs.push(Glyph { id, x: pen, y: 0.0, advance, text: ch.to_string() });
            pen += f32::from(advance) * pictured.size / source.upem;
        }
        match line.clip(row) {
            Some(clip) if !glyphs.is_empty() => {
                let stretch = if pen > 0.0 && pictured.width > 0.0 { pictured.width / pen } else { 1.0 };
                runs.push(TextRun {
                    font: source.font, size: pictured.size, color: [0.0, 0.0, 0.0, 1.0],
                    origin: [pictured.x, pictured.baseline], clip, mode: TextMode::Invisible, glyphs, stretch, tag,
                    replacement: Replacement::None,
                });
            }
            _ => self.stats.clipped += glyphs.len(),
        }
        if runs.len() > first_run {
            match tag {
                RunTag::Element(_) => self.stats.tagged_lines += 1,
                RunTag::Artifact(_) => self.stats.artifact_lines += 1,
                RunTag::None => self.stats.untagged_lines += 1,
            }
            self.stats.pictured_rows += 1;
        }
    }

    /// Supplies every used face's program before the writer finishes the document.
    pub(crate) fn finish(mut self, writer: &mut PdfWriter) -> Result<TextStats, String> {
        let start = Cx::time_now();
        for face in &self.faces {
            // An sfnt face's glyphs drawn as shapes are outlines in a Type 3 registration.
            if let Some(shapes) = face.shapes {
                let mut parsed = FontFace::from_data_and_index(face.data.clone(), face.index)
                    .ok_or_else(|| format!("font {} could not be read for its outlines", face.name))?;
                parsed.set_variations(&face.coords);
                for id in writer.used_glyphs(shapes) {
                    writer.set_type3_glyph(shapes, id, outline_glyph(&parsed, id, face.upem));
                }
            }
            let used = writer.used_glyphs(face.font);
            if used.is_empty() {
                continue;
            }
            if face.kind == Kind::Sfnt {
                let program = static_instance(face.data.as_slice(), face.index, &face.coords, &used)
                    .map_err(|error| format!("font {} could not be embedded: {error}", face.name))?;
                self.stats.font_bytes += program.len();
                writer.set_font_program(face.font, program);
                continue;
            }
            if face.kind == Kind::Invisible {
                for id in used {
                    writer.set_type3_glyph(face.font, id, Type3Glyph::Outline(Vec::new()));
                }
                continue;
            }
            let mut parsed = FontFace::from_data_and_index(face.data.clone(), face.index)
                .ok_or_else(|| format!("font {} could not be read for its outlines", face.name))?;
            parsed.set_variations(&face.coords);
            for id in used {
                let bitmap = if face.kind == Kind::Bitmap {
                    parsed.with_ttf_parser_face(|parsed| image_glyph(parsed, id, face.upem))
                } else {
                    None
                };
                let glyph = bitmap.unwrap_or_else(|| outline_glyph(&parsed, id, face.upem));
                if let Type3Glyph::Image { rgba, .. } = &glyph {
                    self.stats.font_bytes += rgba.len();
                }
                writer.set_type3_glyph(face.font, id, glyph);
            }
        }
        self.stats.font_time = Cx::time_now() - start;
        Ok(self.stats())
    }

    /// What the pages have held so far, for the job's log line.
    pub(crate) fn stats(&self) -> TextStats {
        self.stats
    }
}

/// A glyph's outline in font units, empty when it has none.
fn outline_glyph(parsed: &FontFace, id: u16, upem: f32) -> Type3Glyph {
    let commands = parsed.glyph_outline(id, upem).map(|outline| {
        outline.commands().iter().map(|command| match *command {
            Command::MoveTo(p) => PathCommand::MoveTo(p.x, p.y),
            Command::LineTo(p) => PathCommand::LineTo(p.x, p.y),
            Command::QuadTo(c, p) => PathCommand::QuadTo(c.x, c.y, p.x, p.y),
            Command::CurveTo(a, b, p) => PathCommand::CurveTo(a.x, a.y, b.x, b.y, p.x, p.y),
            Command::Close => PathCommand::Close,
        }).collect()
    }).unwrap_or_default();
    Type3Glyph::Outline(commands)
}

/// A stretch of a line in a face: from `x0` to `x1` and between its ascent and descent.
struct LineBox {
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
}

impl LineBox {
    fn new(face: &Face, x: f32, baseline: f32, width: f32, size: f32) -> Self {
        let scale = size / face.upem;
        Self {
            x0: f64::from(x),
            x1: f64::from(x + width),
            y0: f64::from(baseline - face.ascent * scale),
            y1: f64::from(baseline - face.descent * scale),
        }
    }

    fn middle(&self) -> f64 {
        (self.y0 + self.y1) * 0.5
    }

    /// `None` when the stretch is not on the row's page (wholly beside its clip, or its middle
    /// past the clip's top or bottom), else the clip it needs: none when it lies inside.
    fn clip(&self, row: &CapturedRow) -> Option<Option<Rect>> {
        let left = row.clip.pos.x;
        let top = row.clip.pos.y;
        let right = left + row.clip.size.x;
        let bottom = top + row.clip.size.y;
        // A line belongs to the page its glyphs' middle is on: the line boxes of the lines
        // next to a page break reach a little across it.
        let middle = self.middle();
        if right <= left || bottom <= top || self.x1 <= left || self.x0 >= right || middle < top || middle >= bottom {
            return None;
        }
        const EPSILON: f64 = 0.01;
        Some((self.x0 < left - EPSILON || self.x1 > right + EPSILON || self.y0 < top - EPSILON || self.y1 > bottom + EPSILON)
            .then_some(Rect { x: left as f32, y: top as f32, w: row.clip.size.x as f32, h: row.clip.size.y as f32 }))
    }
}

/// A glyph as written: its run, its direction and its cluster's text range.
struct Placed {
    run: usize,
    rtl: bool,
    text: Range<usize>,
}

/// Gives each right-to-left stretch of a line (consecutive runs whose glyphs all read right to
/// left, often in several fonts: a fallback face for the letters, the main one for the spaces) one
/// replacement text, the text it reads in reading order. The glyphs stay in visual order and keep
/// their own text, so a viewer that orders a line itself reads them as before, while a reader of
/// replacement text (a structure reader, a screen reader) gets the logical text. A stretch whose
/// clusters do not tile one range of the row's text (a glyph left out inside it, an ellipsis) or
/// that is one cluster (its glyphs already share its text) gets none. Answers how many got one.
fn logical_rtl_text(runs: &mut [TextRun], placed: &[Placed], text: &str) -> usize {
    let mut joined = 0;
    let mut start = 0;
    while start < placed.len() {
        let rtl = placed[start].rtl;
        let mut end = start + 1;
        while end < placed.len() && placed[end].rtl == rtl {
            end += 1;
        }
        let stretch = &placed[start..end];
        start = end;
        if !rtl || stretch.iter().any(|placed| placed.text.is_empty()) {
            continue;
        }
        let mut ranges: Vec<Range<usize>> = stretch.iter().map(|placed| placed.text.clone()).collect();
        ranges.sort_by_key(|range| (range.start, range.end));
        ranges.dedup();
        if ranges.len() < 2 || ranges.windows(2).any(|pair| pair[0].end != pair[1].start) {
            continue;
        }
        let (first, last) = (stretch[0].run, stretch[stretch.len() - 1].run);
        runs[first].replacement = Replacement::Start(text[ranges[0].start..ranges[ranges.len() - 1].end].to_string());
        for run in &mut runs[first + 1..=last] {
            run.replacement = Replacement::Continue;
        }
        joined += 1;
    }
    joined
}

fn image_glyph(face: &ttf_parser::Face<'_>, id: u16, upem: f32) -> Option<Type3Glyph> {
    let raster = GlyphRasterImage::from_raster_glyph_image(face.glyph_raster_image(ttf_parser::GlyphId(id), u16::MAX)?)?;
    let size = raster.decode_size();
    if size.width == 0 || size.height == 0 || raster.dpxs_per_em() <= 0.0 {
        return None;
    }
    let mut image = Image::<Bgra>::new(size);
    raster.decode(&mut image.subimage_mut(size.into()));
    // decode_png copies PNG channels directly into Bgra: they are straight,
    // not premultiplied. The PDF writer supplies their alpha as an SMask.
    let rgba = image.as_pixels().iter().flat_map(|pixel| [pixel.r(), pixel.g(), pixel.b(), pixel.a()]).collect();
    let origin = raster.origin_in_dpxs();
    let scale = upem / raster.dpxs_per_em();
    // Matches draw_rasterized_glyph's `-origin_in_dpxs.y - atlas_image_size.height`
    // once transparent atlas padding is removed: origin.y is the bottom in y-up units.
    Some(Type3Glyph::Image {
        rgba, width_px: size.width as u32, height_px: size.height as u32,
        rect: [origin.x * scale, origin.y * scale, (origin.x + size.width as f32) * scale, (origin.y + size.height as f32) * scale],
    })
}
