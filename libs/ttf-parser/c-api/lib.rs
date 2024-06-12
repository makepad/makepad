#![allow(non_camel_case_types)]

use std::convert::TryFrom;
use std::os::raw::{c_void, c_char};

use ttf_parser::GlyphId;
#[cfg(feature = "variable-fonts")] use ttf_parser::Tag;

/// @brief An opaque pointer to the font face structure.
pub struct ttfp_face {
    _unused: [u8; 0],
}

/// @brief An outline building interface.
#[repr(C)]
pub struct ttfp_outline_builder {
    pub move_to: unsafe extern "C" fn(x: f32, y: f32, data: *mut c_void),
    pub line_to: unsafe extern "C" fn(x: f32, y: f32, data: *mut c_void),
    pub quad_to: unsafe extern "C" fn(x1: f32, y1: f32, x: f32, y: f32, data: *mut c_void),
    pub curve_to: unsafe extern "C" fn(x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32, data: *mut c_void),
    pub close_path: unsafe extern "C" fn(data: *mut c_void),
}

struct Builder(ttfp_outline_builder, *mut c_void);

impl ttf_parser::OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        unsafe { (self.0.move_to)(x, y, self.1) }
    }

    fn line_to(&mut self, x: f32, y: f32) {
        unsafe { (self.0.line_to)(x, y, self.1) }
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        unsafe { (self.0.quad_to)(x1, y1, x, y, self.1) }
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        unsafe { (self.0.curve_to)(x1, y1, x2, y2, x, y, self.1) }
    }

    fn close(&mut self) {
        unsafe { (self.0.close_path)(self.1) }
    }
}

/// @brief A name record.
///
/// https://docs.microsoft.com/en-us/typography/opentype/spec/name#name-records
#[repr(C)]
pub struct ttfp_name_record {
    pub platform_id: u16,
    pub encoding_id: u16,
    pub language_id: u16,
    pub name_id: u16,
    pub name_size: u16,
}

/// @brief A glyph image format.
#[repr(C)]
pub enum ttfp_raster_image_format {
    /// @brief A PNG.
    PNG = 0,

    /// @brief A monochrome bitmap.
    ///
    /// The most significant bit of the first byte corresponds to the top-left pixel, proceeding
    /// through succeeding bits moving left to right. The data for each row is padded to a byte
    /// boundary, so the next row begins with the most significant bit of a new byte. 1 corresponds
    /// to black, and 0 to white.
    BITMAP_MONO = 1,

    /// @brief A packed monochrome bitmap.
    ///
    /// The most significant bit of the first byte corresponds to the top-left pixel, proceeding
    /// through succeeding bits moving left to right. Data is tightly packed with no padding. 1
    /// corresponds to black, and 0 to white.
    BITMAP_MONO_PACKED = 2,

    /// @brief A grayscale bitmap with 2 bits per pixel.
    ///
    /// The most significant bits of the first byte corresponds to the top-left pixel, proceeding
    /// through succeeding bits moving left to right. The data for each row is padded to a byte
    /// boundary, so the next row begins with the most significant bit of a new byte.
    BITMAP_GRAY_2 = 3,

    /// @brief A packed grayscale bitmap with 2 bits per pixel.
    ///
    /// The most significant bits of the first byte corresponds to the top-left pixel, proceeding
    /// through succeeding bits moving left to right. Data is tightly packed with no padding.
    BITMAP_GRAY_2_PACKED = 4,

    /// @brief A grayscale bitmap with 4 bits per pixel.
    ///
    /// The most significant bits of the first byte corresponds to the top-left pixel, proceeding
    /// through succeeding bits moving left to right. The data for each row is padded to a byte
    /// boundary, so the next row begins with the most significant bit of a new byte.
    BITMAP_GRAY_4 = 5,

    /// @brief A packed grayscale bitmap with 4 bits per pixel.
    ///
    /// The most significant bits of the first byte corresponds to the top-left pixel, proceeding
    /// through succeeding bits moving left to right. Data is tightly packed with no padding.
    BITMAP_GRAY_4_PACKED = 6,

    /// @brief A grayscale bitmap with 8 bits per pixel.
    ///
    /// The first byte corresponds to the top-left pixel, proceeding through succeeding bytes
    /// moving left to right.
    BITMAP_GRAY_8 = 7,

    /// @brief A color bitmap with 32 bits per pixel.
    ///
    /// The first group of four bytes corresponds to the top-left pixel, proceeding through
    /// succeeding pixels moving left to right. Each byte corresponds to a color channel and the
    /// channels within a pixel are in blue, green, red, alpha order. Color values are
    /// pre-multiplied by the alpha. For example, the color "full-green with half translucency"
    /// is encoded as `\x00\x80\x00\x80`, and not `\x00\xFF\x00\x80`.
    BITMAP_PREMUL_BGRA_32 = 8,
}

/// @brief A glyph image.
///
/// An image offset and size isn't defined in all tables, so `x`, `y`, `width` and `height`
/// can be set to 0.
#[repr(C)]
pub struct ttfp_glyph_raster_image {
    /// Horizontal offset.
    pub x: i16,

    /// Vertical offset.
    pub y: i16,

    /// Image width.
    ///
    /// It doesn't guarantee that this value is the same as set in the `data`.
    pub width: u16,

    /// Image height.
    ///
    /// It doesn't guarantee that this value is the same as set in the `data`.
    pub height: u16,

    /// A pixels per em of the selected strike.
    pub pixels_per_em: u16,

    /// An image format.
    pub format: ttfp_raster_image_format,

    /// A raw image data as is. It's up to the caller to decode PNG, JPEG, etc.
    pub data: *const c_char,

    /// A raw image data size.
    pub len: u32,
}

fn face_from_ptr(face: *const ttfp_face) -> &'static ttf_parser::Face<'static> {
    unsafe { &*(face as *const ttf_parser::Face) }
}

#[cfg(feature = "variable-fonts")]
fn face_from_mut_ptr(face: *const ttfp_face) -> &'static mut ttf_parser::Face<'static> {
    unsafe { &mut *(face as *mut ttf_parser::Face) }
}

/// @brief Returns the number of fonts stored in a TrueType font collection.
///
/// @param data The font data.
/// @param len The size of the font data.
/// @return Number of fonts or -1 when provided data is not a TrueType font collection
///         or when number of fonts is larger than INT_MAX.
#[no_mangle]
pub extern "C" fn ttfp_fonts_in_collection(data: *const c_char, len: usize) -> i32 {
    let data = unsafe { std::slice::from_raw_parts(data as *const _, len) };
    match ttf_parser::fonts_in_collection(data) {
        Some(n) => i32::try_from(n).unwrap_or(-1),
        None => -1,
    }
}

/// @brief Creates a new font face parser.
///
/// Since #ttfp_face is an opaque pointer, a caller should allocate it manually
/// using #ttfp_face_size_of.
/// Deallocation is also handled by a caller.
/// #ttfp_face doesn't use heap internally, so we can simply `free()` it without
/// a dedicated `ttfp_face_deinit` function.
///
/// @param data A font binary data. Must outlive the #ttfp_face.
/// @param len Size of the font data.
/// @param index The font face index in a collection (typically *.ttc). 0 should be used for basic fonts.
/// @param face A pointer to a #ttfp_face object.
/// @return `true` on success.
#[no_mangle]
pub extern "C" fn ttfp_face_init(data: *const c_char, len: usize, index: u32, face: *mut c_void) -> bool {
    // This method invokes a lot of parsing, so let's catch any panics just in case.
    std::panic::catch_unwind(|| {
        let data = unsafe { std::slice::from_raw_parts(data as *const _, len) };
        let face_rs = match ttf_parser::Face::parse(data, index) {
            Ok(v) => v,
            Err(_) => return false,
        };
        unsafe {
            std::ptr::copy(
                &face_rs as *const ttf_parser::Face as _,
                face,
                ttfp_face_size_of(),
            );
        }

        true
    }).unwrap_or(false)
}

/// @brief Returns the size of `ttfp_face`.
#[no_mangle]
pub extern "C" fn ttfp_face_size_of() -> usize {
    std::mem::size_of::<ttf_parser::Face>()
}

/// @brief Returns the number of name records in the face.
#[no_mangle]
pub extern "C" fn ttfp_get_name_records_count(face: *const ttfp_face) -> u16 {
    face_from_ptr(face).names().len()
}

/// @brief Returns a name record.
///
/// @param Record's index. The total amount can be obtained via #ttfp_get_name_records_count.
/// @return `false` when `index` is out of range or `platform_id` is invalid.
#[no_mangle]
pub extern "C" fn ttfp_get_name_record(
    face: *const ttfp_face,
    index: u16,
    record: *mut ttfp_name_record,
) -> bool {
    match face_from_ptr(face).names().get(index) {
        Some(rec) => {
            unsafe {
                (*record).platform_id = match rec.platform_id {
                    ttf_parser::PlatformId::Unicode => 0,
                    ttf_parser::PlatformId::Macintosh => 1,
                    ttf_parser::PlatformId::Iso => 2,
                    ttf_parser::PlatformId::Windows => 3,
                    ttf_parser::PlatformId::Custom => 4,
                };

                (*record).encoding_id = rec.encoding_id;
                (*record).language_id = rec.language_id;
                (*record).name_id = rec.name_id;
                (*record).name_size = rec.name.len() as u16;
            }

            true
        }
        None => false,
    }
}

/// @brief Returns a name record's string.
///
/// @param index Record's index.
/// @param name A string buffer that will be filled with the record's name.
///             Remember that a name will use encoding specified in `ttfp_name_record.encoding_id`
///             Because of that, the name will not be null-terminated.
/// @param len The size of a string buffer. Must be equal to `ttfp_name_record.name_size`.
/// @return `false` when `index` is out of range or string buffer is not equal
///         `ttfp_name_record.name_size`.
#[no_mangle]
pub extern "C" fn ttfp_get_name_record_string(
    face: *const ttfp_face,
    index: u16,
    name: *mut c_char,
    len: usize,
) -> bool {
    match face_from_ptr(face).names().get(index) {
        Some(r) => {
            let r_name = r.name;
            if r_name.len() != len {
                return false;
            }

            // TODO: memcpy?
            let name = unsafe { std::slice::from_raw_parts_mut(name, len) };
            for (i, c) in r_name.iter().enumerate() {
                name[i] = *c as c_char;
            }

            true
        }
        None => false,
    }
}

/// @brief Checks that face is marked as *Regular*.
///
/// @return `false` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_is_regular(face: *const ttfp_face) -> bool {
    face_from_ptr(face).is_regular()
}

/// @brief Checks that face is marked as *Italic*.
///
/// @return `false` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_is_italic(face: *const ttfp_face) -> bool {
    face_from_ptr(face).is_italic()
}

/// @brief Checks that face is marked as *Bold*.
///
/// @return `false` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_is_bold(face: *const ttfp_face) -> bool {
    face_from_ptr(face).is_bold()
}

/// @brief Checks that face is marked as *Oblique*.
///
/// @return `false` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_is_oblique(face: *const ttfp_face) -> bool {
    face_from_ptr(face).is_oblique()
}

/// @brief Checks that face is marked as *Monospaced*.
///
/// @return `false` when `post` table is not present.
#[no_mangle]
pub extern "C" fn ttfp_is_monospaced(face: *const ttfp_face) -> bool {
    face_from_ptr(face).is_monospaced()
}

/// @brief Checks that face is variable.
///
/// Simply checks the presence of a `fvar` table.
#[no_mangle]
pub extern "C" fn ttfp_is_variable(face: *const ttfp_face) -> bool {
    face_from_ptr(face).is_variable()
}

/// @brief Returns face's weight.
///
/// @return Face's weight or `400` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_weight(face: *const ttfp_face) -> u16 {
    face_from_ptr(face).weight().to_number()
}

/// @brief Returns face's width.
///
/// @return Face's width in a 1..9 range or `5` when OS/2 table is not present
///         or when value is invalid.
#[no_mangle]
pub extern "C" fn ttfp_get_width(face: *const ttfp_face) -> u16 {
    face_from_ptr(face).width().to_number()
}

/// @brief Returns face's italic angle.
///
/// @return Face's italic angle or `0.0` when `post` table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_italic_angle(face: *const ttfp_face) -> f32 {
    face_from_ptr(face).italic_angle().unwrap_or(0.0)
}

/// @brief Returns a horizontal face ascender.
///
/// This function is affected by variation axes.
#[no_mangle]
pub extern "C" fn ttfp_get_ascender(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).ascender()
}

/// @brief Returns a horizontal face descender.
///
/// This function is affected by variation axes.
#[no_mangle]
pub extern "C" fn ttfp_get_descender(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).descender()
}

/// @brief Returns a horizontal face height.
///
/// This function is affected by variation axes.
#[no_mangle]
pub extern "C" fn ttfp_get_height(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).height()
}

/// @brief Returns a horizontal face line gap.
///
/// This function is affected by variation axes.
#[no_mangle]
pub extern "C" fn ttfp_get_line_gap(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).line_gap()
}

/// @brief Returns a horizontal typographic face ascender.
///
/// Prefer `ttfp_get_ascender` unless you explicitly want this. This is a more
/// low-level alternative.
///
/// This function is affected by variation axes.
///
/// @return `0` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_typographic_ascender(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).typographic_ascender().unwrap_or(0)
}

/// @brief Returns a horizontal typographic face descender.
///
/// Prefer `ttfp_get_descender` unless you explicitly want this. This is a more
/// low-level alternative.
///
/// This function is affected by variation axes.
///
/// @return `0` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_typographic_descender(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).typographic_descender().unwrap_or(0)
}

/// @brief Returns a horizontal typographic face line gap.
///
/// Prefer `ttfp_get_line_gap` unless you explicitly want this. This is a more
/// low-level alternative.
///
/// This function is affected by variation axes.
///
/// @return `0` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_typographic_line_gap(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).typographic_line_gap().unwrap_or(0)
}

/// @brief Returns a vertical face ascender.
///
/// This function is affected by variation axes.
///
/// @return `0` when `vhea` table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_vertical_ascender(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).vertical_ascender().unwrap_or(0)
}

/// @brief Returns a vertical face descender.
///
/// This function is affected by variation axes.
///
/// @return `0` when `vhea` table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_vertical_descender(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).vertical_descender().unwrap_or(0)
}

/// @brief Returns a vertical face height.
///
/// This function is affected by variation axes.
///
/// @return `0` when `vhea` table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_vertical_height(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).vertical_height().unwrap_or(0)
}

/// @brief Returns a vertical face line gap.
///
/// This function is affected by variation axes.
///
/// @return `0` when `vhea` table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_vertical_line_gap(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).vertical_line_gap().unwrap_or(0)
}

/// @brief Returns face's units per EM.
///
/// @return Units in a 16..16384 range or `0` otherwise.
#[no_mangle]
pub extern "C" fn ttfp_get_units_per_em(face: *const ttfp_face) -> u16 {
    face_from_ptr(face).units_per_em()
}

/// @brief Returns face's x height.
///
/// This function is affected by variation axes.
///
/// @return x height or 0 when OS/2 table is not present or when its version is < 2.
#[no_mangle]
pub extern "C" fn ttfp_get_x_height(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).x_height().unwrap_or(0)
}

/// @brief Returns face's capital height.
///
/// This function is affected by variation axes.
///
/// @return capital height or 0 when OS/2 table is not present or when its version is < 2.
#[no_mangle]
pub extern "C" fn ttfp_get_capital_height(face: *const ttfp_face) -> i16 {
    face_from_ptr(face).capital_height().unwrap_or(0)
}

/// @brief Returns face's underline metrics.
///
/// This function is affected by variation axes.
///
/// @return `false` when `post` table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_underline_metrics(
    face: *const ttfp_face,
    metrics: *mut ttf_parser::LineMetrics,
) -> bool {
    match face_from_ptr(face).underline_metrics() {
        Some(m) => {
            unsafe { *metrics = m; }
            true
        }
        None => false,
    }
}

/// @brief Returns face's strikeout metrics.
///
/// This function is affected by variation axes.
///
/// @return `false` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_strikeout_metrics(
    face: *const ttfp_face,
    metrics: *mut ttf_parser::LineMetrics,
) -> bool {
    match face_from_ptr(face).strikeout_metrics() {
        Some(m) => {
            unsafe { *metrics = m; }
            true
        }
        None => false,
    }
}

/// @brief Returns font's subscript metrics.
///
/// This function is affected by variation axes.
///
/// @return `false` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_subscript_metrics(
    face: *const ttfp_face,
    metrics: *mut ttf_parser::ScriptMetrics,
) -> bool {
    match face_from_ptr(face).subscript_metrics() {
        Some(m) => {
            unsafe { *metrics = m; }
            true
        }
        None => false,
    }
}

/// @brief Returns face's superscript metrics.
///
/// This function is affected by variation axes.
///
/// @return `false` when OS/2 table is not present.
#[no_mangle]
pub extern "C" fn ttfp_get_superscript_metrics(
    face: *const ttfp_face,
    metrics: *mut ttf_parser::ScriptMetrics,
) -> bool {
    match face_from_ptr(face).superscript_metrics() {
        Some(m) => {
            unsafe { *metrics = m; }
            true
        }
        None => false,
    }
}

/// @brief Returns a total number of glyphs in the face.
///
/// @return The number of glyphs which is never zero.
#[no_mangle]
pub extern "C" fn ttfp_get_number_of_glyphs(face: *const ttfp_face) -> u16 {
    face_from_ptr(face).number_of_glyphs()
}

/// @brief Resolves a Glyph ID for a code point.
///
/// All subtable formats except Mixed Coverage (8) are supported.
///
/// @param codepoint A valid Unicode codepoint. Otherwise 0 will be returned.
/// @return Returns 0 when glyph is not present or parsing is failed.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_index(face: *const ttfp_face, codepoint: u32) -> u16 {
    // This method invokes a lot of parsing, so let's catch any panics just in case.
    std::panic::catch_unwind(|| {
        let get = || {
            let c = char::try_from(codepoint).ok()?;
            face_from_ptr(face).glyph_index(c).map(|gid| gid.0)
        };

        get().unwrap_or(0)
    }).unwrap_or(0)
}

/// @brief Resolves a variation of a Glyph ID from two code points.
///
/// @param codepoint A valid Unicode codepoint. Otherwise 0 will be returned.
/// @param variation A valid Unicode codepoint. Otherwise 0 will be returned.
/// @return Returns 0 when glyph is not present or parsing is failed.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_var_index(
    face: *const ttfp_face,
    codepoint: u32,
    variation: u32,
) -> u16 {
    // This method invokes a lot of parsing, so let's catch any panics just in case.
    std::panic::catch_unwind(|| {
        let get = || {
            let c = char::try_from(codepoint).ok()?;
            let v = char::try_from(variation).ok()?;
            face_from_ptr(face).glyph_variation_index(c, v).map(|gid| gid.0)
        };

        get().unwrap_or(0)
    }).unwrap_or(0)
}

/// @brief Returns glyph's horizontal advance.
///
/// @return Glyph's advance or 0 when not set.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_hor_advance(face: *const ttfp_face, glyph_id: GlyphId) -> u16 {
    face_from_ptr(face).glyph_hor_advance(glyph_id).unwrap_or(0)
}

/// @brief Returns glyph's vertical advance.
///
/// This function is affected by variation axes.
///
/// @return Glyph's advance or 0 when not set.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_ver_advance(face: *const ttfp_face, glyph_id: GlyphId) -> u16 {
    face_from_ptr(face).glyph_ver_advance(glyph_id).unwrap_or(0)
}

/// @brief Returns glyph's horizontal side bearing.
///
/// @return Glyph's side bearing or 0 when not set.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_hor_side_bearing(face: *const ttfp_face, glyph_id: GlyphId) -> i16 {
    face_from_ptr(face).glyph_hor_side_bearing(glyph_id).unwrap_or(0)
}

/// @brief Returns glyph's vertical side bearing.
///
/// This function is affected by variation axes.
///
/// @return Glyph's side bearing or 0 when not set.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_ver_side_bearing(face: *const ttfp_face, glyph_id: GlyphId) -> i16 {
    face_from_ptr(face).glyph_ver_side_bearing(glyph_id).unwrap_or(0)
}

/// @brief Returns glyph's vertical origin.
///
/// @return Glyph's vertical origin or 0 when not set.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_y_origin(face: *const ttfp_face, glyph_id: GlyphId) -> i16 {
    face_from_ptr(face).glyph_y_origin(glyph_id).unwrap_or(0)
}

/// @brief Returns glyph's name.
///
/// Uses the `post` and `CFF` tables as sources.
///
/// A glyph name cannot be larger than 255 bytes + 1 byte for '\0'.
///
/// @param name A char buffer larger than 256 bytes.
/// @return `true` on success.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_name(
    face: *const ttfp_face,
    glyph_id: GlyphId,
    name: *mut c_char,
) -> bool {
    match face_from_ptr(face).glyph_name(glyph_id) {
        Some(n) => {
            // TODO: memcpy?
            let name = unsafe { std::slice::from_raw_parts_mut(name as *mut _, 256) };
            for (i, c) in n.bytes().enumerate() {
                name[i] = c;
            }

            name[n.len()] = 0;

            true
        }
        None => false,
    }
}

/// @brief Outlines a glyph and returns its tight bounding box.
///
/// **Warning**: since `ttf-parser` is a pull parser,
/// `OutlineBuilder` will emit segments even when outline is partially malformed.
/// You must check #ttfp_outline_glyph() result before using
/// #ttfp_outline_builder 's output.
///
/// `glyf`, `gvar`, `CFF` and `CFF2` tables are supported.
///
/// This function is affected by variation axes.
///
/// @return `false` when glyph has no outline or on error.
#[no_mangle]
pub extern "C" fn ttfp_outline_glyph(
    face: *const ttfp_face,
    builder: ttfp_outline_builder,
    user_data: *mut c_void,
    glyph_id: GlyphId,
    bbox: *mut ttf_parser::Rect,
) -> bool {
    // This method invokes a lot of parsing, so let's catch any panics just in case.
    std::panic::catch_unwind(|| {
        let mut b = Builder(builder, user_data);
        match face_from_ptr(face).outline_glyph(glyph_id, &mut b) {
            Some(bb) => {
                unsafe { *bbox = bb }
                true
            }
            None => false,
        }
    }).unwrap_or(false)
}

/// @brief Returns a tight glyph bounding box.
///
/// Unless the current face has a `glyf` table, this is just a shorthand for `outline_glyph()`
/// since only the `glyf` table stores a bounding box. In case of CFF and variable fonts
/// we have to actually outline a glyph to find it's bounding box.
///
/// This function is affected by variation axes.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_bbox(
    face: *const ttfp_face,
    glyph_id: GlyphId,
    bbox: *mut ttf_parser::Rect,
) -> bool {
    // This method invokes a lot of parsing, so let's catch any panics just in case.
    std::panic::catch_unwind(|| {
        match face_from_ptr(face).glyph_bounding_box(glyph_id) {
            Some(bb) => {
                unsafe { *bbox = bb }
                true
            }
            None => false,
        }
    }).unwrap_or(false)
}

/// @brief Returns a bounding box that large enough to enclose any glyph from the face.
#[no_mangle]
pub extern "C" fn ttfp_get_global_bounding_box(
    face: *const ttfp_face,
) -> ttf_parser::Rect {
    face_from_ptr(face).global_bounding_box()
}

/// @brief Returns a reference to a glyph's raster image.
///
/// A font can define a glyph using a raster or a vector image instead of a simple outline.
/// Which is primarily used for emojis. This method should be used to access raster images.
///
/// `pixels_per_em` allows selecting a preferred image size. The chosen size will
/// be closer to an upper one. So when font has 64px and 96px images and `pixels_per_em`
/// is set to 72, 96px image will be returned.
/// To get the largest image simply use `SHRT_MAX`.
///
/// Note that this method will return an encoded image. It should be decoded
/// by the caller. We don't validate or preprocess it in any way.
///
/// Also, a font can contain both: images and outlines. So when this method returns `None`
/// you should also try `ttfp_outline_glyph()` afterwards.
///
/// There are multiple ways an image can be stored in a TrueType font
/// and this method supports most of them.
/// This includes `sbix`, `bloc` + `bdat`, `EBLC` + `EBDT`, `CBLC` + `CBDT`.
/// And font's tables will be accesses in this specific order.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_raster_image(
    face: *const ttfp_face,
    glyph_id: GlyphId,
    pixels_per_em: u16,
    glyph_image: *mut ttfp_glyph_raster_image,
) -> bool {
    match face_from_ptr(face).glyph_raster_image(glyph_id, pixels_per_em) {
        Some(image) => {
            unsafe {
                *glyph_image = ttfp_glyph_raster_image {
                    x: image.x,
                    y: image.y,
                    width: image.width,
                    height: image.height,
                    pixels_per_em: image.pixels_per_em,
                    format: match image.format {
                        ttf_parser::RasterImageFormat::PNG => ttfp_raster_image_format::PNG,
                        ttf_parser::RasterImageFormat::BitmapMono => {
                            ttfp_raster_image_format::BITMAP_MONO
                        }
                        ttf_parser::RasterImageFormat::BitmapMonoPacked => {
                            ttfp_raster_image_format::BITMAP_MONO_PACKED
                        }
                        ttf_parser::RasterImageFormat::BitmapGray2 => {
                            ttfp_raster_image_format::BITMAP_GRAY_2
                        }
                        ttf_parser::RasterImageFormat::BitmapGray2Packed => {
                            ttfp_raster_image_format::BITMAP_GRAY_2_PACKED
                        }
                        ttf_parser::RasterImageFormat::BitmapGray4 => {
                            ttfp_raster_image_format::BITMAP_GRAY_4
                        }
                        ttf_parser::RasterImageFormat::BitmapGray4Packed => {
                            ttfp_raster_image_format::BITMAP_GRAY_4_PACKED
                        }
                        ttf_parser::RasterImageFormat::BitmapGray8 => {
                            ttfp_raster_image_format::BITMAP_GRAY_8
                        }
                        ttf_parser::RasterImageFormat::BitmapPremulBgra32 => {
                            ttfp_raster_image_format::BITMAP_PREMUL_BGRA_32
                        }
                    },
                    data: image.data.as_ptr() as _,
                    len: image.data.len() as u32,
                };
            }

            true
        }
        None => false,
    }
}

/// @brief Returns a reference to a glyph's SVG image.
///
/// A font can define a glyph using a raster or a vector image instead of a simple outline.
/// Which is primarily used for emojis. This method should be used to access SVG images.
///
/// Note that this method will return just an SVG data. It should be rendered
/// or even decompressed (in case of SVGZ) by the caller.
/// We don't validate or preprocess it in any way.
///
/// Also, a font can contain both: images and outlines. So when this method returns `false`
/// you should also try `ttfp_outline_glyph()` afterwards.
#[no_mangle]
pub extern "C" fn ttfp_get_glyph_svg_image(
    face: *const ttfp_face,
    glyph_id: GlyphId,
    svg: *mut *const c_char,
    len: *mut u32,
) -> bool {
    match face_from_ptr(face).glyph_svg_image(glyph_id) {
        Some(image) => {
            unsafe {
                *svg = image.data.as_ptr() as *const c_char;
                *len = image.data.len() as u32;
            }

            true
        }
        None => false,
    }
}

/// @brief Returns the amount of variation axes.
#[cfg(feature = "variable-fonts")]
#[no_mangle]
pub extern "C" fn ttfp_get_variation_axes_count(face: *const ttfp_face) -> u16 {
    face_from_ptr(face).variation_axes().len()
}

/// @brief Returns a variation axis by index.
#[cfg(feature = "variable-fonts")]
#[no_mangle]
pub extern "C" fn ttfp_get_variation_axis(
    face: *const ttfp_face,
    index: u16,
    axis: *mut ttf_parser::VariationAxis,
) -> bool {
    match face_from_ptr(face).variation_axes().get(index) {
        Some(a) => {
            unsafe { *axis = a };
            true
        }
        None => false,
    }
}

/// @brief Returns a variation axis by tag.
#[cfg(feature = "variable-fonts")]
#[no_mangle]
pub extern "C" fn ttfp_get_variation_axis_by_tag(
    face: *const ttfp_face,
    tag: ttf_parser::Tag,
    axis: *mut ttf_parser::VariationAxis,
) -> bool {
    match face_from_ptr(face).variation_axes().into_iter().find(|axis| axis.tag == tag) {
        Some(a) => {
            unsafe { *axis = a };
            true
        }
        None => false,
    }
}

/// @brief Sets a variation axis coordinate.
///
/// This is the only mutable function in the library.
/// We can simplify the API a lot by storing the variable coordinates
/// in the face object itself.
///
/// This function is reentrant.
///
/// Since coordinates are stored on the stack, we allow only 32 of them.
///
/// @return `false` when face is not variable or doesn't have such axis.
#[cfg(feature = "variable-fonts")]
#[no_mangle]
pub extern "C" fn ttfp_set_variation(face: *mut ttfp_face, axis: Tag, value: f32) -> bool {
    face_from_mut_ptr(face).set_variation(axis, value).is_some()
}

/// @brief Returns the current normalized variation coordinates.
///
/// Values represented as f2.16
#[cfg(feature = "variable-fonts")]
#[no_mangle]
pub extern "C" fn ttfp_get_variation_coordinates(face: *const ttfp_face) -> *const i16 {
    face_from_ptr(face).variation_coordinates().as_ptr() as _
}

/// @brief Checks that face has non-default variation coordinates.
#[cfg(feature = "variable-fonts")]
#[no_mangle]
pub extern "C" fn ttfp_has_non_default_variation_coordinates(face: *const ttfp_face) -> bool {
    face_from_ptr(face).has_non_default_variation_coordinates()
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes() {
        assert_eq!(std::mem::size_of::<ttf_parser::Rect>(), 8);
        assert_eq!(std::mem::size_of::<ttf_parser::LineMetrics>(), 4);
        assert_eq!(std::mem::size_of::<ttf_parser::ScriptMetrics>(), 8);
    }
}
