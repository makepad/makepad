//! Shaped, embedded vector text for material textures and vector canvases.
//! RGB is a distance field, never colour; A is an independent badge SDF.
use makepad_font_core::{
    font::{Font, FontId},
    font_face::{FontData, FontFace},
    geom::Size,
    glyph_outline::GlyphOutline,
    image::{Bgra, Image},
    msdfer::Msdfer,
    raster::{render_msdf, Settings},
    rustybuzz::UnicodeBuffer,
};
use std::{cell::RefCell, collections::{BTreeMap, HashMap, VecDeque}, rc::Rc};

/// What a glyph's distance field depends on: the face and its variations,
/// the glyph, the texels per em and the field's range (its padding and
/// image size follow from these).
#[derive(Clone, PartialEq, Eq, Hash)]
struct GlyphFieldKey {
    font: String,
    weight: Option<u32>,
    width: Option<u32>,
    glyph_id: u16,
    resolution: u32,
    radius: u32,
    padding: usize,
    size: (usize, usize),
}

/// Most glyph fields kept for later bakes (the oldest goes first).
const GLYPH_FIELDS_KEPT: usize = 1024;

thread_local! {
    /// Distance fields of glyphs already rendered, shared by every bake: a
    /// label, a plate's digit strip and the next sign reuse their letters.
    /// Only a cache: eviction never refuses text or changes its bytes.
    static GLYPH_FIELDS: RefCell<(HashMap<GlyphFieldKey, Rc<Image<Bgra>>>, VecDeque<GlyphFieldKey>)> = RefCell::new((HashMap::new(), VecDeque::new()));
}

fn glyph_field(key: GlyphFieldKey, render: impl FnOnce() -> Image<Bgra>) -> Rc<Image<Bgra>> {
    if let Some(field) = GLYPH_FIELDS.with(|f| f.borrow().0.get(&key).cloned()) { return field; }
    let field = Rc::new(render());
    GLYPH_FIELDS.with(|f| {
        let (fields, order) = &mut *f.borrow_mut();
        fields.insert(key.clone(), field.clone());
        order.push_back(key);
        while order.len() > GLYPH_FIELDS_KEPT {
            if let Some(old) = order.pop_front() { fields.remove(&old); }
        }
    });
    field
}

/// The same embedded faces serve both material text and vector canvases.
pub fn text_face(name: &str, weight: Option<f32>, width: Option<f32>) -> Result<FontFace, String> {
    let bytes: &'static [u8] = match name {
        "plex" => include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf"),
        "plex-semibold" => include_bytes!("../../../widgets/resources/IBMPlexSans-SemiBold.ttf"),
        "plex-italic" => include_bytes!("../../../widgets/resources/IBMPlexSans-Italic.ttf"),
        "plex-bold-italic" => include_bytes!("../../../widgets/resources/IBMPlexSans-BoldItalic.ttf"),
        "inter" => include_bytes!("../../../widgets/resources/Inter.ttf"),
        "roboto-flex" => include_bytes!("../../../widgets/resources/RobotoFlex.ttf"),
        _ => return Err(format!("unknown text face '{name}'")),
    };
    thread_local! {
        static FACES: RefCell<BTreeMap<String, FontFace>> = RefCell::new(BTreeMap::new());
    }
    let mut face = FACES.with(|faces| {
        let mut faces = faces.borrow_mut();
        if let Some(face) = faces.get(name) { return Ok(face.clone()); }
        let face = FontFace::from_data_and_index(FontData::from_vec(bytes.to_vec()), 0)
            .ok_or_else(|| format!("cannot parse embedded face '{name}'"))?;
        faces.insert(name.into(), face.clone());
        Ok::<_, String>(face)
    })?;
    let mut variations = Vec::new();
    for (option, tag, value, min, max) in [
        ("weight", *b"wght", weight, 100.0, 1000.0),
        ("width", *b"wdth", width, 25.0, 151.0),
    ] {
        if let Some(value) = value {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(format!("{option} on face '{name}' must be {min}..={max}"));
            }
            let supported = face.with_ttf_parser_face(|f| f.variation_axes().into_iter().any(|a| a.tag.0 == u32::from_be_bytes(tag)));
            if !supported { return Err(format!("face '{name}' has no {option} axis ({})", std::str::from_utf8(&tag).unwrap())); }
            variations.push((u32::from_be_bytes(tag), value));
        }
    }
    face.set_variations(&variations);
    Ok(face)
}

#[derive(Clone, Debug)]
pub struct TextGlyph {
    pub outline: Rc<GlyphOutline>,
    pub glyph_id: u16,
    /// Baseline origin in ems, with Y pointing up (as in font_core).
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug)]
pub struct TextLine {
    pub glyphs: Vec<TextGlyph>,
    pub advance: f32,
    /// Left, bottom, right, top, including advances and outline overhangs.
    pub bounds: [f32; 4],
}

/// Shape once, including ligatures, kerning, glyph offsets and variations.
/// The caller chooses its line-length policy (the material bake allows 64).
pub fn shape_text(face: &FontFace, text: &str, letter_spacing: f32) -> TextLine {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    let shaped = face.shape(&[], buffer);
    let units = face.with_ttf_parser_face(|f| f.units_per_em() as f32);
    let font = Font::new(FontId::from(0u64), face.clone(), 0.0, 0.0);
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut bounds = [0.0f32, f32::INFINITY, 0.0, f32::NEG_INFINITY];
    let mut glyphs = Vec::new();
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        let (gx, gy) = (x + pos.x_offset as f32 / units, y + pos.y_offset as f32 / units);
        if let Some(outline) = font.glyph_outline_rc(info.glyph_id as u16) {
            let b = outline.bounds_in_ems();
            bounds[0] = bounds[0].min(gx + b.origin.x);
            bounds[1] = bounds[1].min(gy + b.origin.y);
            bounds[2] = bounds[2].max(gx + b.origin.x + b.size.width);
            bounds[3] = bounds[3].max(gy + b.origin.y + b.size.height);
            glyphs.push(TextGlyph { outline, glyph_id: info.glyph_id as u16, x: gx, y: gy });
        }
        x += pos.x_advance as f32 / units + letter_spacing;
        y += pos.y_advance as f32 / units;
        bounds[0] = bounds[0].min(x);
        bounds[2] = bounds[2].max(x);
    }
    if glyphs.is_empty() { bounds[1] = 0.0; bounds[3] = 1.0; }
    TextLine { glyphs, advance: x, bounds }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MsdfTextSpec {
    pub text: String,
    pub font: String,
    pub weight: Option<f32>,
    pub width: Option<f32>,
    pub letter_spacing: f32,
    /// Texels per em, 16..=256.
    pub resolution: u32,
    pub badge: Option<String>,
    /// Additional margin in ems (rect/text margin by default).
    pub padding: f32,
    /// In ems; reserves enough SDF range to resolve the inner border edge.
    pub border_width: f32,
    pub plate: bool,
}

impl Default for MsdfTextSpec {
    fn default() -> Self {
        Self { text: String::new(), font: "plex-semibold".into(), weight: None, width: None,
            letter_spacing: 0.0, resolution: 64, badge: None, padding: 1.0 / 7.0,
            border_width: 0.0, plate: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MsdfTextImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// Full encoded distance range in texels (0.5 is the contour).
    pub range: f32,
    /// First blank U, blank cell width U, first strip U, strip cell width U.
    /// Exactly three blank cells above, ten digit cells below. Both rows
    /// use identical texel density, so the remap preserves signed distances.
    pub plate_cells: Option<[f32; 4]>,
}

pub fn msdf_text(spec: &MsdfTextSpec) -> Result<MsdfTextImage, String> {
    let count = spec.text.chars().count();
    if count > 64 || spec.text.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')) {
        return Err("text needs one line of at most 64 characters".into());
    }
    if spec.plate && count > 4 { return Err("plate prefix needs at most 4 characters".into()); }
    if !(16..=256).contains(&spec.resolution) { return Err("resolution must be 16..=256 texels per em".into()); }
    if !spec.letter_spacing.is_finite() || !spec.padding.is_finite() || spec.padding < 0.0
        || !spec.border_width.is_finite() || spec.border_width < 0.0 {
        return Err("letter_spacing must be finite; padding and border_width must be finite, nonnegative ems".into());
    }
    let badge = if spec.plate { "plate" } else { spec.badge.as_deref().unwrap_or("") };
    if !["", "circle", "plate", "rect"].contains(&badge) { return Err(format!("unknown badge '{badge}' (circle, plate, rect)")); }
    let face = text_face(&spec.font, spec.weight, spec.width)?;
    let mut top = shape_text(&face, &spec.text, spec.letter_spacing);
    let mut strip = Vec::new();
    let mut cell = 0.0f32;
    let mut blank_x = 0.0;
    if spec.plate {
        let digits: Vec<_> = "0123456789".chars().map(|c| shape_text(&face, &c.to_string(), 0.0)).collect();
        cell = digits.iter().map(|d| d.advance).fold(0.0, f32::max);
        // Keep all ten cells at the same scale as the top line. The shorter
        // prefix + three blanks is centred within that common atlas width.
        let top_width = (top.bounds[2] - top.bounds[0]) + 3.0 * cell;
        let width = top_width.max(10.0 * cell);
        let shift = (width - top_width) * 0.5 - top.bounds[0];
        for g in &mut top.glyphs { g.x += shift; }
        blank_x = shift + top.bounds[2];
        top.bounds[0] = 0.0;
        top.bounds[2] = width;
        for (i, digit) in digits.into_iter().enumerate() {
            top.bounds[1] = top.bounds[1].min(digit.bounds[1]);
            top.bounds[3] = top.bounds[3].max(digit.bounds[3]);
            for mut g in digit.glyphs {
                g.x += i as f32 * cell + (cell - digit.advance) * 0.5;
                strip.push(g);
            }
        }
    }
    let res = spec.resolution as f32;
    let mut settings = Settings::default().msdfer;
    settings.cutoff = 0.5;
    settings.radius = settings.radius.max(2.0 * (spec.border_width * res + 1.0));
    // The guard alone must fit the texture limit before converting to usize.
    if !settings.radius.is_finite() || settings.radius + 2.0 > 4096.0 {
        return Err(format!("resolution {} with text length {count} exceeds the 4096 texel texture side", spec.resolution));
    }
    settings.padding = (settings.radius * 0.5).ceil() as usize + 1;
    let tw = (top.bounds[2] - top.bounds[0]).max(1.0 / res);
    let th = (top.bounds[3] - top.bounds[1]).max(1.0 / res);
    let (mut bw, mut bh) = match badge {
        "circle" => { let d = (tw.max(th) * 1.35).max(9.0 / 7.0) + 2.0 * spec.padding; (d, d) }
        "plate" => (tw + 4.0 / 7.0 + 2.0 * spec.padding, th + 3.2 / 7.0 + 2.0 * spec.padding),
        _ => (tw + 2.0 * spec.padding, th + 2.0 * spec.padding),
    };
    // A guard surrounds the shape and the text even when padding is zero.
    // This preserves the zero contour at texture edges and in lower mips.
    let guard = settings.padding as f32;
    bw *= res;
    bh *= res;
    let width = (bw + guard * 2.0).ceil();
    let row_height = (bh + guard * 2.0).ceil();
    let height = row_height * if spec.plate { 2.0 } else { 1.0 };
    if !width.is_finite() || !height.is_finite() || width.max(height) > 4096.0 {
        return Err(format!("resolution {} with text length {count} exceeds the 4096 texel texture side", spec.resolution));
    }
    let (w, rh, h) = (width as u32, row_height as u32, height as u32);
    let ox = (width - tw * res) * 0.5 - top.bounds[0] * res;
    let baseline = (row_height - th * res) * 0.5 + top.bounds[3] * res;
    let mut image = MsdfTextImage { width: w, height: h, pixels: vec![0; w as usize * h as usize * 4], range: settings.radius,
        plate_cells: spec.plate.then_some([(ox + blank_x * res) / width, cell * res / width, ox / width, cell * res / width]) };
    let mut msdfer = Msdfer::new(settings);
    // Repeated glyphs (including prefix digits reused in the strip) render
    // only once, and a glyph an earlier bake rendered is not rendered again.
    // The caches affect neither order nor output bytes.
    let mut fields = BTreeMap::new();
    for (glyphs, row) in [(&top.glyphs, 0), (&strip, rh)] {
        for glyph in glyphs {
            let b = glyph.outline.bounds_in_ems();
            let iw = (b.size.width * res).ceil() as usize;
            let ih = (b.size.height * res).ceil() as usize;
            let size = Size::new(iw + 2 * settings.padding, ih + 2 * settings.padding);
            let field = fields.entry(glyph.glyph_id).or_insert_with(|| {
                let key = GlyphFieldKey {
                    font: spec.font.clone(), weight: spec.weight.map(f32::to_bits), width: spec.width.map(f32::to_bits),
                    glyph_id: glyph.glyph_id, resolution: spec.resolution, radius: settings.radius.to_bits(),
                    padding: settings.padding, size: (size.width, size.height),
                };
                glyph_field(key, || render_msdf(&mut msdfer, &glyph.outline, res, size))
            });
            let x0 = (ox + (glyph.x + b.origin.x) * res - guard).round() as i32;
            // font_core flips about inner_height - 1 (not the outline top).
            let y0 = (baseline - (glyph.y + b.origin.y) * res - (ih as f32 - 1.0) - guard).round() as i32 + row as i32;
            for y in 0..size.height {
                let dy = y0 + y as i32;
                if dy < row as i32 || dy >= (row + rh) as i32 { continue; }
                for x in 0..size.width {
                    let dx = x0 + x as i32;
                    if dx < 0 || dx >= w as i32 { continue; }
                    let p = field.as_pixels()[y * size.width + x];
                    let rgb = [p.r(), p.g(), p.b()];
                    let at = (dy as usize * w as usize + dx as usize) * 4;
                    // Union by the median; never combine unrelated channels
                    // from adjacent glyphs into a spurious corner.
                    if median(&rgb) > median(&image.pixels[at..at + 3]) {
                        image.pixels[at..at + 3].copy_from_slice(&rgb);
                    }
                }
            }
        }
    }
    for y in 0..h { for x in 0..w {
        let px = (x as f32 + 0.5 - width * 0.5).abs();
        let py = ((y % rh) as f32 + 0.5 - row_height * 0.5).abs();
        let distance = match badge {
            "circle" => bw.min(bh) * 0.5 - (px * px + py * py).sqrt(),
            "plate" | "rect" => {
                let r = if badge == "plate" { (1.2 / 7.0 * res).min(bw.min(bh) * 0.5) } else { 0.0 };
                let (qx, qy) = (px - bw * 0.5 + r, py - bh * 0.5 + r);
                r - (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() - qx.max(qy).min(0.0)
            }
            _ => f32::INFINITY,
        };
        image.pixels[((y * w + x) * 4 + 3) as usize] = ((0.5 + distance / image.range).clamp(0.0, 1.0) * 255.0).round() as u8;
    }}
    Ok(image)
}

fn median(c: &[u8]) -> u8 { c[0].min(c[1]).max(c[0].max(c[1]).min(c[2])) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shaped_text_distances_resolution_badge_and_variations() {
        let spec = MsdfTextSpec { text: "27".into(), resolution: 32, badge: Some("circle".into()), ..Default::default() };
        let a = msdf_text(&spec).unwrap();
        let b = msdf_text(&MsdfTextSpec { resolution: 64, ..spec.clone() }).unwrap();
        assert!((b.width as i32 - 2 * a.width as i32).abs() <= 12, "fixed distance guard is the only nonscaling margin");
        assert_eq!(a.pixels, msdf_text(&spec).unwrap().pixels, "repeat bake is byte identical");
        assert!(median(&a.pixels[..3]) < 128, "corner is outside the glyphs");
        assert!(a.pixels.chunks_exact(4).any(|p| median(p) > 160), "a stroke interior has positive distance");
        assert!(a.pixels[((a.height / 2 * a.width + a.width / 2) * 4 + 3) as usize] > 128, "badge centre is inside");
        assert!(a.pixels[3] < 128, "badge corner is outside");
        let face = text_face("roboto-flex", None, None).unwrap();
        let narrow = text_face("roboto-flex", None, Some(25.0)).unwrap();
        assert!(shape_text(&narrow, "27", 0.0).advance < shape_text(&face, "27", 0.0).advance, "wdth changes shaped advance");
        let variable = MsdfTextSpec { text: "27".into(), font: "roboto-flex".into(), resolution: 16, ..Default::default() };
        let normal = msdf_text(&variable).unwrap();
        let condensed = msdf_text(&MsdfTextSpec { width: Some(25.0), ..variable }).unwrap();
        assert!(condensed.width < normal.width, "wdth narrows the baked texture");
        assert!(normal.pixels.chunks_exact(4).all(|p| p[3] == 255), "no badge covers the entire texture");
        for name in ["plex", "plex-semibold", "plex-italic", "plex-bold-italic", "inter", "roboto-flex"] {
            let face = text_face(name, None, None).unwrap();
            assert_eq!(shape_text(&face, "27", 0.0).glyphs.len(), 2, "embedded face {name} supplies digit outlines");
        }
        let err = text_face("plex", None, Some(75.0)).unwrap_err();
        assert!(err.contains("plex") && err.contains("width"));
    }

    #[test]
    fn plate_has_three_blank_cells_and_ten_drawn_digits() {
        let image = msdf_text(&MsdfTextSpec { text: "AB".into(), resolution: 16, plate: true, ..Default::default() }).unwrap();
        let [blank, cell, strip, strip_cell] = image.plate_cells.unwrap();
        let has_ink = |start: f32, end: f32, row: u32| {
            let lo = (start * image.width as f32).ceil() as u32;
            let hi = (end * image.width as f32).floor() as u32;
            (row..row + image.height / 2).any(|y| (lo..hi).any(|x| median(&image.pixels[((y * image.width + x) * 4) as usize..][..3]) > 128))
        };
        for i in 0..3 { assert!(!has_ink(blank + i as f32 * cell, blank + (i + 1) as f32 * cell, 0), "blank cell {i}"); }
        for i in 0..10 { assert!(has_ink(strip + i as f32 * strip_cell, strip + (i + 1) as f32 * strip_cell, image.height / 2), "digit {i}"); }
        assert!(msdf_text(&MsdfTextSpec { text: "W".repeat(64), resolution: 256, ..Default::default() }).unwrap_err().contains("resolution 256 with text length 64"));
    }
}
