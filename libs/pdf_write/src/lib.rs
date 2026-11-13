//! In-memory PDF 1.7 writing with streamed raster pages and embedded vector text.
//! Coordinates are points from the page's top-left, with y increasing downward.
//! Images are compressed before they reach the writer (`RgbImage`), on any
//! thread, so that pages compress in parallel and adding one only copies it in.

mod font;
mod object;
mod outline;
mod type3;

#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};

use font::Font;
use makepad_fast_inflate::ZlibCompressor;
use object::{number, string, uri, utf16_hex, Objects};

/// Document metadata.
pub struct DocInfo {
    /// Document title.
    pub title: String,
    /// Software that produced the document.
    pub producer: String,
}

/// A rectangle in points, measured from the page's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// An RGB8 image, Flate-compressed and ready for a page.
pub struct RgbImage {
    width_px: u32,
    height_px: u32,
    flate: Vec<u8>,
}

/// Compresses an `RgbImage` from its tightly packed RGB8 rows as they come,
/// top to bottom, in pieces of any size, so the whole image never has to be
/// held uncompressed.
pub struct RgbImageEncoder {
    width_px: u32,
    height_px: u32,
    written: u64,
    compressor: ZlibCompressor,
    flate: Vec<u8>,
}

impl RgbImageEncoder {
    /// Starts an image of the given size.
    pub fn new(width_px: u32, height_px: u32) -> Self {
        Self { width_px, height_px, written: 0, compressor: ZlibCompressor::new(6), flate: Vec::new() }
    }

    /// Compresses the next RGB8 bytes of the image.
    pub fn write(&mut self, rgb: &[u8]) {
        self.written += rgb.len() as u64;
        self.compressor.write(rgb, &mut self.flate);
    }

    /// Ends the image. Panics unless exactly width_px * height_px * 3 bytes were written.
    pub fn finish(mut self) -> RgbImage {
        let expected = u64::from(self.width_px)
            .checked_mul(u64::from(self.height_px))
            .and_then(|pixels| pixels.checked_mul(3));
        assert!(
            expected == Some(self.written),
            "an RgbImage takes width_px * height_px * 3 bytes"
        );
        self.compressor.finish(&mut self.flate);
        RgbImage { width_px: self.width_px, height_px: self.height_px, flate: self.flate }
    }
}

/// An image and its placement on a page.
pub struct PageImage {
    /// The compressed pixels.
    pub image: RgbImage,
    /// Where the image is drawn on the page.
    pub rect: Rect,
}

/// A font registered with this writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontId(pub usize);

/// Font metrics supplied by the caller, independently of the font program.
pub struct FontMetrics {
    /// PostScript name, sanitised to ASCII letters, digits and hyphens and given a subset tag.
    pub postscript_name: String,
    /// Font units per em; must be nonzero.
    pub units_per_em: u16,
    /// Ascent in font units.
    pub ascent: i16,
    /// Descent in font units, normally negative.
    pub descent: i16,
    /// Capital height in font units.
    pub cap_height: i16,
    /// Font bounding box [x_min, y_min, x_max, y_max] in font units.
    pub bbox: [i16; 4],
}

/// A Type 3 glyph drawn in font units, with y increasing upward.
pub enum Type3Glyph {
    /// An uncoloured outline, filled with the run's colour using the nonzero winding rule.
    Outline(Vec<PathCommand>),
    /// A colour image, including per-pixel alpha.
    Image {
        /// Tightly packed RGBA8 rows, top to bottom.
        rgba: Vec<u8>,
        /// Image width in pixels; must be nonzero.
        width_px: u32,
        /// Image height in pixels; must be nonzero.
        height_px: u32,
        /// Placement [x_min, y_min, x_max, y_max] in font units.
        rect: [f32; 4],
    },
}

/// An outline operation in font units, with y increasing upward.
#[derive(Clone, Copy, Debug)]
pub enum PathCommand {
    /// Starts a contour at (x, y).
    MoveTo(f32, f32),
    /// Draws a line to (x, y).
    LineTo(f32, f32),
    /// Draws a quadratic through control (cx, cy) to (x, y).
    QuadTo(f32, f32, f32, f32),
    /// Draws a cubic through controls (cx1, cy1), (cx2, cy2) to (x, y).
    CurveTo(f32, f32, f32, f32, f32, f32),
    /// Closes the current contour.
    Close,
}

/// A positioned glyph with its extraction text.
pub struct Glyph {
    /// Glyph id in the registered font.
    pub id: u16,
    /// Pen x in points relative to the run origin.
    pub x: f32,
    /// Offset from the run baseline in points, positive downward.
    pub y: f32,
    /// Advance in font units, used in the PDF width array (the first glyph with an id gives its width).
    pub advance: u16,
    /// Cluster text on its first glyph, or an empty string on the remaining glyphs.
    pub text: String,
}

/// How a text run is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextMode {
    /// Paint filled glyphs.
    Fill,
    /// Searchable text with PDF render mode 3; paints nothing.
    Invisible,
}

/// Positioned text anchored at a baseline, with optional per-glyph offsets.
pub struct TextRun {
    /// Registered font to use.
    pub font: FontId,
    /// Font size in points.
    pub size: f32,
    /// RGBA fill components in 0..1.
    pub color: [f32; 4],
    /// Baseline start in top-left page coordinates.
    pub origin: [f32; 2],
    /// Optional clipping rectangle in top-left page coordinates.
    pub clip: Option<Rect>,
    /// Visible fill or invisible searchable text.
    pub mode: TextMode,
    /// Glyphs in drawing order.
    pub glyphs: Vec<Glyph>,
}

/// A link's external or in-document destination.
pub enum LinkTarget {
    /// A URI; non-ASCII UTF-8 bytes are percent-encoded.
    Uri(String),
    /// A location in this document, including a page added later.
    Page {
        /// Zero-based page index.
        index: usize,
        /// Distance from the page top in points.
        y: f32,
    },
}

/// A rectangular link annotation.
pub struct Link {
    /// Clickable area in top-left page coordinates.
    pub rect: Rect,
    /// Destination of the link.
    pub target: LinkTarget,
}

/// A page, painted as images first, then glyph runs, with link annotations.
pub struct Page {
    /// Page width in points.
    pub width: f32,
    /// Page height in points.
    pub height: f32,
    /// Raster layers in drawing order.
    pub images: Vec<PageImage>,
    /// Vector text in drawing order; empty for a raster-only page.
    pub text: Vec<TextRun>,
    /// Link annotations.
    pub links: Vec<Link>,
}

/// A document bookmark; skipped levels nest under the nearest shallower entry.
pub struct OutlineEntry {
    /// Bookmark title.
    pub title: String,
    /// Nesting level, with 1 at the top.
    pub level: u8,
    /// Zero-based destination page index.
    pub page: usize,
    /// Destination distance from the page top in points.
    pub y: f32,
}

struct PageRecord {
    object: usize,
    height: f32,
}

struct PendingLink {
    object: usize,
    rect: String,
    target: LinkTarget,
}

/// An in-memory writer retaining compressed pages and document metadata.
pub struct PdfWriter {
    objects: Objects,
    info: DocInfo,
    fonts: Vec<Font>,
    page_objects: HashMap<usize, usize>,
    pages: Vec<PageRecord>,
    links: Vec<PendingLink>,
    outline: Vec<OutlineEntry>,
}

impl PdfWriter {
    /// Starts a PDF document with the given metadata.
    pub fn new(info: DocInfo) -> Self {
        Self {
            objects: Objects::new(),
            info,
            fonts: Vec::new(),
            page_objects: HashMap::new(),
            pages: Vec::new(),
            links: Vec::new(),
            outline: Vec::new(),
        }
    }

    /// Registers an sfnt font whose program is supplied before finish().
    /// Unused fonts allocate no PDF objects and need no program.
    pub fn add_font(&mut self, metrics: FontMetrics) -> FontId {
        let id = FontId(self.fonts.len());
        self.fonts.push(Font::new(metrics, false));
        id
    }

    /// Registers a Type 3 font whose used glyph procedures are supplied before finish().
    pub fn add_type3_font(&mut self, metrics: FontMetrics) -> FontId {
        let id = FontId(self.fonts.len());
        self.fonts.push(Font::new(metrics, true));
        id
    }

    /// Returns the glyph ids used so far by a registered font, in ascending order.
    pub fn used_glyphs(&self, font: FontId) -> Vec<u16> {
        self.fonts
            .get(font.0)
            .expect("font must be registered")
            .used_glyphs()
    }

    /// Sets or replaces a registered sfnt font's TrueType `glyf` or OpenType `CFF ` program.
    /// The caller may subset it, preserving the glyph ids used by runs.
    pub fn set_font_program(&mut self, font: FontId, data: Vec<u8>) {
        self.fonts
            .get_mut(font.0)
            .expect("font must be registered")
            .set_program(data);
    }

    /// Sets or replaces a glyph procedure in a registered Type 3 font.
    pub fn set_type3_glyph(&mut self, font: FontId, id: u16, glyph: Type3Glyph) {
        self.fonts
            .get_mut(font.0)
            .expect("font must be registered")
            .set_glyph(id, glyph);
    }

    /// Writes the page's content and images immediately: the images are
    /// compressed already, and the content stream is compressed here.
    pub fn add_page(&mut self, page: Page) {
        let page_object = self.reserve_page(self.pages.len());
        let mut content = String::new();
        let mut images = String::new();
        for (index, image) in page.images.into_iter().enumerate() {
            let object = self.objects.reserve();
            self.objects.flate_stream(
                object,
                &format!(
                    "/Type /XObject /Subtype /Image /Width {} /Height {} \
                     /ColorSpace /DeviceRGB /BitsPerComponent 8",
                    image.image.width_px, image.image.height_px
                ),
                &image.image.flate,
            );
            images.push_str(&format!("/Im{index} {object} 0 R "));
            content.push_str(&format!(
                "q {} 0 0 {} {} {} cm /Im{index} Do Q\n",
                number(f64::from(image.rect.w)),
                number(f64::from(image.rect.h)),
                number(f64::from(image.rect.x)),
                number(f64::from(page.height) - f64::from(image.rect.y) - f64::from(image.rect.h)),
            ));
        }

        let mut used_fonts = BTreeSet::new();
        let mut alphas = HashMap::new();
        let mut states = String::new();
        for run in page.text {
            let font = self
                .fonts
                .get_mut(run.font.0)
                .expect("run font must be registered");
            if run.glyphs.is_empty() {
                continue;
            }
            assert!(
                run.color.iter().all(|value| (0.0..=1.0).contains(value)),
                "text colour components must be in 0..1"
            );
            let alpha = run.color[3];
            let saved = alpha < 1.0 || run.clip.is_some();
            if saved {
                content.push_str("q ");
            }
            if alpha < 1.0 {
                // Canonicalise negative zero so equal alphas share one state.
                let key = if alpha == 0.0 { 0 } else { alpha.to_bits() };
                let next = alphas.len();
                let state = alphas.entry(key).or_insert_with(|| {
                    let object = self.objects.reserve();
                    self.objects.write(
                        object,
                        &format!("<< /Type /ExtGState /ca {} >>", number(f64::from(alpha))),
                    );
                    states.push_str(&format!("/GS{next} {object} 0 R "));
                    next
                });
                content.push_str(&format!("/GS{state} gs "));
            }
            if let Some(clip) = run.clip {
                content.push_str(&format!(
                    "{} {} {} {} re W n ",
                    number(f64::from(clip.x)),
                    number(f64::from(page.height) - f64::from(clip.y) - f64::from(clip.h)),
                    number(f64::from(clip.w)),
                    number(f64::from(clip.h)),
                ));
            }
            for subfont in write_run(&mut content, &run, font, &mut self.objects, page.height) {
                used_fonts.insert((run.font.0, subfont));
            }
            if saved {
                content.push_str("Q\n");
            }
        }
        let mut fonts = String::new();
        for (index, subfont) in used_fonts {
            let font = &self.fonts[index];
            fonts.push_str(&format!(
                "/{} {} 0 R ",
                font.resource_name(index, subfont),
                font.objects[subfont]
            ));
        }
        let states = if states.is_empty() {
            String::new()
        } else {
            format!("/ExtGState << {states} >>")
        };

        let mut annotations = String::new();
        for link in page.links {
            if let LinkTarget::Page { index, .. } = &link.target {
                self.reserve_page(*index);
            }
            let object = self.objects.reserve();
            annotations.push_str(&format!("{object} 0 R "));
            let rect = format!(
                "[{} {} {} {}]",
                number(f64::from(link.rect.x)),
                number(f64::from(page.height) - f64::from(link.rect.y) - f64::from(link.rect.h)),
                number(f64::from(link.rect.x) + f64::from(link.rect.w)),
                number(f64::from(page.height) - f64::from(link.rect.y)),
            );
            self.links.push(PendingLink {
                object,
                rect,
                target: link.target,
            });
        }
        let contents = self.objects.reserve();
        self.objects.stream(contents, "", content.as_bytes());
        self.objects.write(
            page_object,
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] \
                 /Resources << /XObject << {images} >> /Font << {fonts} >> {states} >> \
                 /Contents {contents} 0 R /Annots [{annotations}] >>",
                number(f64::from(page.width)),
                number(f64::from(page.height)),
            ),
        );
        self.pages.push(PageRecord {
            object: page_object,
            height: page.height,
        });
    }

    /// Replaces the bookmarks at any time before finish(), reserving destination pages.
    pub fn set_outline(&mut self, entries: Vec<OutlineEntry>) {
        for entry in &entries {
            self.reserve_page(entry.page);
        }
        self.outline = entries;
    }

    /// Returns the number of pages already added.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Writes fonts, destinations, page tree and xref, returning the completed PDF.
    /// All in-document destinations must refer to pages added before this call.
    /// Panics with the font name if a used font program or Type 3 glyph is missing.
    pub fn finish(mut self) -> Vec<u8> {
        for (index, font) in self.fonts.into_iter().enumerate() {
            font.write(&mut self.objects, index);
        }
        for link in self.links {
            let target = match link.target {
                LinkTarget::Uri(value) => format!("/A << /S /URI /URI {} >>", uri(&value)),
                LinkTarget::Page { index, y } => {
                    format!("/Dest {}", destination(&self.pages, index, y))
                }
            };
            self.objects.write(
                link.object,
                &format!(
                    "<< /Type /Annot /Subtype /Link /Border [0 0 0] /Rect {} {target} >>",
                    link.rect
                ),
            );
        }
        let outline = outline::write(&mut self.objects, &self.pages, self.outline);
        self.objects
            .write(1, &format!("<< /Type /Catalog /Pages 2 0 R {outline} >>"));
        let mut kids = String::new();
        for page in &self.pages {
            kids.push_str(&format!("{} 0 R ", page.object));
        }
        self.objects.write(
            2,
            &format!(
                "<< /Type /Pages /Kids [{kids}] /Count {} >>",
                self.pages.len()
            ),
        );
        let info = self.objects.reserve();
        self.objects.write(
            info,
            &format!(
                "<< /Title {} /Producer {} >>",
                string(&self.info.title),
                string(&self.info.producer)
            ),
        );
        self.objects.finish(info)
    }

    fn reserve_page(&mut self, index: usize) -> usize {
        *self
            .page_objects
            .entry(index)
            .or_insert_with(|| self.objects.reserve())
    }
}

fn destination(pages: &[PageRecord], index: usize, y: f32) -> String {
    let page = pages
        .get(index)
        .expect("destination page must be added before finish");
    format!(
        "[{} 0 R /XYZ 0 {} null]",
        page.object,
        number(f64::from(page.height) - f64::from(y))
    )
}

fn write_run(
    content: &mut String,
    run: &TextRun,
    font: &mut Font,
    objects: &mut Objects,
    height: f32,
) -> BTreeSet<usize> {
    let em = f64::from(font.metrics.units_per_em);
    let size = f64::from(run.size);
    let mut subfonts = BTreeSet::new();
    // Text render mode survives ET: set it explicitly so Fill follows Invisible correctly.
    content.push_str(&format!(
        "BT {} {} {} rg {} Tr ",
        number(f64::from(run.color[0])),
        number(f64::from(run.color[1])),
        number(f64::from(run.color[2])),
        if run.mode == TextMode::Invisible {
            3
        } else {
            0
        },
    ));
    let mut selected = None;
    let mut previous: Option<&Glyph> = None;
    let mut start = 0;
    while start < run.glyphs.len() {
        let first = &run.glyphs[start];
        let mut end = start + 1;
        if !first.text.is_empty() {
            while end < run.glyphs.len() && run.glyphs[end].text.is_empty() {
                end += 1;
            }
        }
        let actual = !first.text.is_empty() && (end - start > 1 || font.conflicts(first));
        if actual {
            close_text_array(content, &mut previous);
            content.push_str(&format!(
                "/Span << /ActualText <FEFF{}> >> BDC ",
                utf16_hex(&first.text)
            ));
        }
        for glyph in &run.glyphs[start..end] {
            let (subfont, code) = font.record(glyph, objects);
            subfonts.insert(subfont);
            if selected != Some(subfont) {
                close_text_array(content, &mut previous);
                content.push_str(&format!(
                    "/{} {} Tf ",
                    font.resource_name(run.font.0, subfont),
                    number(size)
                ));
                selected = Some(subfont);
            }
            if previous.is_some_and(|prev| prev.y != glyph.y) {
                close_text_array(content, &mut previous);
            }
            if let Some(prev) = previous {
                // A zero-size run paints no glyphs; avoid division by zero in its TJ array.
                if size != 0.0 {
                    let adjustment = -(f64::from(glyph.x)
                        - (f64::from(prev.x) + f64::from(prev.advance) * size / em))
                        * 1000.0
                        / size;
                    if adjustment.abs() >= 0.01 {
                        content.push_str(&format!("{} ", number(adjustment)));
                    }
                }
            } else {
                content.push_str(&format!(
                    "1 0 0 1 {} {} Tm [",
                    number(f64::from(run.origin[0]) + f64::from(glyph.x)),
                    number(f64::from(height) - f64::from(run.origin[1]) - f64::from(glyph.y)),
                ));
            }
            content.push_str(&code);
            content.push(' ');
            previous = Some(glyph);
        }
        if actual {
            close_text_array(content, &mut previous);
            content.push_str("EMC ");
        }
        start = end;
    }
    close_text_array(content, &mut previous);
    content.push_str("ET\n");
    subfonts
}

fn close_text_array(content: &mut String, previous: &mut Option<&Glyph>) {
    if previous.take().is_some() {
        content.push_str("] TJ ");
    }
}
