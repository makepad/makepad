//! In-memory PDF 1.7 writing with streamed raster pages and embedded vector text.
//! Coordinates are points from the page's top-left, with y increasing downward.

mod font;
mod object;
mod outline;

#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};

use font::Font;
use object::{number, string, uri, Objects};

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

/// An RGB image and its placement on a page.
pub struct PageImage {
    /// Pixel width.
    pub width_px: u32,
    /// Pixel height.
    pub height_px: u32,
    /// Tightly packed RGB8, width_px * height_px * 3 bytes, rows top to bottom.
    pub rgb: Vec<u8>,
    /// Where the image is drawn on the page.
    pub rect: Rect,
}

/// A font registered with this writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontId(pub usize);

/// An sfnt font with metrics supplied by the caller.
pub struct FontProgram {
    /// Complete TrueType `glyf` or OpenType `CFF ` sfnt, optionally subset by the caller.
    /// Run glyph ids refer to this font; the writer never renumbers them.
    pub data: Vec<u8>,
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

/// A positioned glyph with its extraction text.
pub struct Glyph {
    /// Glyph id in the registered font.
    pub id: u16,
    /// Pen x in points relative to the run origin.
    pub x: f32,
    /// Advance in font units from hmtx, used in the PDF width array.
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

/// A single baseline of positioned text.
pub struct TextRun {
    /// Registered font to use.
    pub font: FontId,
    /// Font size in points.
    pub size: f32,
    /// RGB fill components in 0..1.
    pub color: [f32; 3],
    /// Baseline start in top-left page coordinates.
    pub origin: [f32; 2],
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

    /// Registers a font for later runs. Its objects are written in finish().
    pub fn add_font(&mut self, font: FontProgram) -> FontId {
        let id = FontId(self.fonts.len());
        let object = self.objects.reserve();
        self.fonts.push(Font::new(object, font));
        id
    }

    /// Writes the page's compressed content and images immediately, dropping its pixels.
    /// Panics if an image's RGB length does not match its dimensions.
    pub fn add_page(&mut self, page: Page) {
        let page_object = self.reserve_page(self.pages.len());
        let mut content = String::new();
        let mut images = String::new();
        for (index, image) in page.images.into_iter().enumerate() {
            let expected = u64::from(image.width_px)
                .checked_mul(u64::from(image.height_px))
                .and_then(|pixels| pixels.checked_mul(3));
            assert!(
                expected == Some(image.rgb.len() as u64),
                "PageImage rgb length must equal width_px * height_px * 3"
            );
            let object = self.objects.reserve();
            self.objects.stream(
                object,
                &format!(
                    "/Type /XObject /Subtype /Image /Width {} /Height {} \
                     /ColorSpace /DeviceRGB /BitsPerComponent 8",
                    image.width_px, image.height_px
                ),
                &image.rgb,
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
        for run in page.text {
            let font = self
                .fonts
                .get_mut(run.font.0)
                .expect("run font must be registered");
            used_fonts.insert(run.font.0);
            write_run(&mut content, &run, font, page.height);
        }
        let mut fonts = String::new();
        for index in used_fonts {
            fonts.push_str(&format!("/F{index} {} 0 R ", self.fonts[index].object));
        }

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
                 /Resources << /XObject << {images} >> /Font << {fonts} >> >> \
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

fn write_run(content: &mut String, run: &TextRun, font: &mut Font, height: f32) {
    let em = f64::from(font.program.units_per_em);
    let size = f64::from(run.size);
    // Text render mode survives ET: set it explicitly so Fill follows Invisible correctly.
    content.push_str(&format!(
        "BT /F{} {} Tf {} {} {} rg {} Tr 1 0 0 1 {} {} Tm [",
        run.font.0,
        number(size),
        number(f64::from(run.color[0])),
        number(f64::from(run.color[1])),
        number(f64::from(run.color[2])),
        if run.mode == TextMode::Invisible {
            3
        } else {
            0
        },
        number(
            f64::from(run.origin[0]) + run.glyphs.first().map_or(0.0, |glyph| f64::from(glyph.x))
        ),
        number(f64::from(height) - f64::from(run.origin[1])),
    ));
    let mut previous: Option<&Glyph> = None;
    for glyph in &run.glyphs {
        font.record(glyph);
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
        }
        content.push_str(&format!("<{:04X}> ", glyph.id));
        previous = Some(glyph);
    }
    content.push_str("] TJ ET\n");
}
