use ttf_parser::GlyphId;
use ttf_parser::gdef::GlyphClass;
use ttf_parser::opentype_layout::LayoutTable;

use crate::Variation;
use crate::ot::{TableIndex, PositioningTable, SubstitutionTable};
use crate::buffer::GlyphPropsFlags;


// https://docs.microsoft.com/en-us/typography/opentype/spec/cmap#windows-platform-platform-id--3
const WINDOWS_SYMBOL_ENCODING: u16 = 0;
const WINDOWS_UNICODE_BMP_ENCODING: u16 = 1;
const WINDOWS_UNICODE_FULL_ENCODING: u16 = 10;

// https://docs.microsoft.com/en-us/typography/opentype/spec/name#platform-specific-encoding-and-language-ids-unicode-platform-platform-id--0
const UNICODE_1_0_ENCODING: u16 = 0;
const UNICODE_1_1_ENCODING: u16 = 1;
const UNICODE_ISO_ENCODING: u16 = 2;
const UNICODE_2_0_BMP_ENCODING: u16 = 3;
const UNICODE_2_0_FULL_ENCODING: u16 = 4;
//const UNICODE_VARIATION_ENCODING: u16 = 5;
const UNICODE_FULL_ENCODING: u16 = 6;


/// A font face handle.
#[derive(Clone)]
pub struct Face<'a> {
    pub(crate) ttfp_face: ttf_parser::Face<'a>,
    pub(crate) units_per_em: u16,
    pixels_per_em: Option<(u16, u16)>,
    pub(crate) points_per_em: Option<f32>,
    prefered_cmap_encoding_subtable: Option<u16>,
    pub(crate) gsub: Option<SubstitutionTable<'a>>,
    pub(crate) gpos: Option<PositioningTable<'a>>,
}

impl<'a> AsRef<ttf_parser::Face<'a>> for Face<'a> {
    #[inline]
    fn as_ref(&self) -> &ttf_parser::Face<'a> {
        &self.ttfp_face
    }
}

impl<'a> AsMut<ttf_parser::Face<'a>> for Face<'a> {
    #[inline]
    fn as_mut(&mut self) -> &mut ttf_parser::Face<'a> {
        &mut self.ttfp_face
    }
}

impl<'a> core::ops::Deref for Face<'a> {
    type Target = ttf_parser::Face<'a>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.ttfp_face
    }
}

impl<'a> core::ops::DerefMut for Face<'a> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ttfp_face
    }
}

impl<'a> Face<'a> {
    /// Creates a new `Face` from data.
    ///
    /// Data will be referenced, not owned.
    pub fn from_slice(data: &'a [u8], face_index: u32) -> Option<Self> {
        let face = ttf_parser::Face::parse(data, face_index).ok()?;
        Some(Self::from_face(face))
    }

    /// Creates a new [`Face`] from [`ttf_parser::Face`].
    ///
    /// Data will be referenced, not owned.
    pub fn from_face(face: ttf_parser::Face<'a>) -> Self {
        Face {
            units_per_em: face.units_per_em(),
            pixels_per_em: None,
            points_per_em: None,
            prefered_cmap_encoding_subtable: find_best_cmap_subtable(&face),
            gsub: face.tables().gsub.map(SubstitutionTable::new),
            gpos: face.tables().gpos.map(PositioningTable::new),
            ttfp_face: face,
        }
    }

    // TODO: remove
    /// Returns face’s units per EM.
    #[inline]
    pub fn units_per_em(&self) -> i32 {
        self.units_per_em as i32
    }

    #[inline]
    pub(crate) fn pixels_per_em(&self) -> Option<(u16, u16)> {
        self.pixels_per_em
    }

    /// Sets pixels per EM.
    ///
    /// Used during raster glyphs processing and hinting.
    ///
    /// `None` by default.
    #[inline]
    pub fn set_pixels_per_em(&mut self, ppem: Option<(u16, u16)>) {
        self.pixels_per_em = ppem;
    }

    /// Sets point size per EM.
    ///
    /// Used for optical-sizing in Apple fonts.
    ///
    /// `None` by default.
    #[inline]
    pub fn set_points_per_em(&mut self, ptem: Option<f32>) {
        self.points_per_em = ptem;
    }

    /// Sets font variations.
    pub fn set_variations(&mut self, variations: &[Variation]) {
        for variation in variations {
            self.set_variation(variation.tag, variation.value);
        }
    }

    pub(crate) fn has_glyph(&self, c: u32) -> bool {
        self.glyph_index(c).is_some()
    }

    pub(crate) fn glyph_index(&self, c: u32) -> Option<GlyphId> {
        let subtable_idx = self.prefered_cmap_encoding_subtable?;
        let subtable = self.tables().cmap?.subtables.get(subtable_idx)?;
        match subtable.glyph_index(c) {
            Some(gid) => Some(gid),
            None => {
                // Special case for Windows Symbol fonts.
                // TODO: add tests
                if  subtable.platform_id == ttf_parser::PlatformId::Windows &&
                    subtable.encoding_id == WINDOWS_SYMBOL_ENCODING
                {
                    if c <= 0x00FF {
                        // For symbol-encoded OpenType fonts, we duplicate the
                        // U+F000..F0FF range at U+0000..U+00FF.  That's what
                        // Windows seems to do, and that's hinted about at:
                        // https://docs.microsoft.com/en-us/typography/opentype/spec/recom
                        // under "Non-Standard (Symbol) Fonts".
                        return self.glyph_index(0xF000 + c);
                    }
                }

                None
            }
        }
    }

    pub(crate) fn glyph_h_advance(&self, glyph: GlyphId) -> i32 {
        self.glyph_advance(glyph, false) as i32
    }

    pub(crate) fn glyph_v_advance(&self, glyph: GlyphId) -> i32 {
        -(self.glyph_advance(glyph, true) as i32)
    }

    fn glyph_advance(&self, glyph: GlyphId, is_vertical: bool) -> u32 {
        let face = &self.ttfp_face;
        if face.is_variable() &&
           face.has_non_default_variation_coordinates() &&
           face.tables().hvar.is_none() &&
           face.tables().vvar.is_none()
        {
            return match face.glyph_bounding_box(glyph) {
                Some(bbox) => {
                    (if is_vertical {
                        bbox.y_max + bbox.y_min
                    } else {
                        bbox.x_max + bbox.x_min
                    }) as u32
                }
                None => 0,
            };
        }

        if is_vertical && face.tables().vmtx.is_some() {
            face.glyph_ver_advance(glyph).unwrap_or(0) as u32
        } else if !is_vertical && face.tables().hmtx.is_some() {
            face.glyph_hor_advance(glyph).unwrap_or(0) as u32
        } else {
            face.units_per_em() as u32
        }
    }

    pub(crate) fn glyph_h_origin(&self, glyph: GlyphId) -> i32 {
        self.glyph_h_advance(glyph) / 2
    }

    pub(crate) fn glyph_v_origin(&self, glyph: GlyphId) -> i32 {
        match self.ttfp_face.glyph_y_origin(glyph) {
            Some(y) => i32::from(y),
            None => self.glyph_extents(glyph).map_or(0, |ext| ext.y_bearing)
                + self.glyph_side_bearing(glyph, true)
        }
    }

    pub(crate) fn glyph_side_bearing(&self, glyph: GlyphId, is_vertical: bool) -> i32 {
        let face = &self.ttfp_face;
        if  face.is_variable() &&
            face.tables().hvar.is_none() &&
            face.tables().vvar.is_none()
        {
            return match face.glyph_bounding_box(glyph) {
                Some(bbox) => (if is_vertical { bbox.x_min } else { bbox.y_min }) as i32,
                None => 0,
            }
        }

        if is_vertical {
            face.glyph_ver_side_bearing(glyph).unwrap_or(0) as i32
        } else {
            face.glyph_hor_side_bearing(glyph).unwrap_or(0) as i32
        }
    }

    pub(crate) fn glyph_extents(&self, glyph: GlyphId) -> Option<GlyphExtents> {
        let pixels_per_em = match self.pixels_per_em {
            Some(ppem) => ppem.0,
            None => core::u16::MAX,
        };

        if let Some(img) = self.ttfp_face.glyph_raster_image(glyph, pixels_per_em) {
            // HarfBuzz also supports only PNG.
            if img.format == ttf_parser::RasterImageFormat::PNG {
                let scale = self.units_per_em as f32 / img.pixels_per_em as f32;
                return Some(GlyphExtents {
                    x_bearing: crate::round(f32::from(img.x) * scale) as i32,
                    y_bearing: crate::round((f32::from(img.y) + f32::from(img.height)) * scale) as i32,
                    width: crate::round(f32::from(img.width) * scale) as i32,
                    height: crate::round(-f32::from(img.height) * scale) as i32,
                });
            }
        }

        let bbox = self.ttfp_face.glyph_bounding_box(glyph)?;
        Some(GlyphExtents {
            x_bearing: i32::from(bbox.x_min),
            y_bearing: i32::from(bbox.y_max),
            width: i32::from(bbox.width()),
            height: i32::from(bbox.y_min - bbox.y_max),
        })
    }

    pub(crate) fn glyph_name(&self, glyph: GlyphId) -> Option<&str> {
        self.ttfp_face.glyph_name(glyph)
    }

    pub(crate) fn glyph_props(&self, glyph: GlyphId) -> u16 {
        let table = match self.tables().gdef {
            Some(v) => v,
            None => return 0,
        };

        match table.glyph_class(glyph) {
            Some(GlyphClass::Base) => GlyphPropsFlags::BASE_GLYPH.bits(),
            Some(GlyphClass::Ligature) => GlyphPropsFlags::LIGATURE.bits(),
            Some(GlyphClass::Mark) => {
                let class = table.glyph_mark_attachment_class(glyph);
                (class << 8) | GlyphPropsFlags::MARK.bits()
            }
            _ => 0,
        }
    }

    pub(crate) fn layout_table(&self, table_index: TableIndex) -> Option<&LayoutTable<'a>> {
        match table_index {
            TableIndex::GSUB => self.gsub.as_ref().map(|table| &table.inner),
            TableIndex::GPOS => self.gpos.as_ref().map(|table| &table.inner),
        }
    }

    pub(crate) fn layout_tables(&self) -> impl Iterator<Item = (TableIndex, &LayoutTable<'a>)> + '_ {
        TableIndex::iter().filter_map(move |idx| self.layout_table(idx).map(|table| (idx, table)))
    }
}

#[derive(Clone, Copy, Default)]
pub struct GlyphExtents {
    pub x_bearing: i32,
    pub y_bearing: i32,
    pub width: i32,
    pub height: i32,
}

fn find_best_cmap_subtable(face: &ttf_parser::Face) -> Option<u16> {
    use ttf_parser::PlatformId;

    // Symbol subtable.
    // Prefer symbol if available.
    // https://github.com/harfbuzz/harfbuzz/issues/1918
    find_cmap_subtable(face, PlatformId::Windows, WINDOWS_SYMBOL_ENCODING)
        // 32-bit subtables:
        .or_else(|| find_cmap_subtable(face, PlatformId::Windows, WINDOWS_UNICODE_FULL_ENCODING))
        .or_else(|| find_cmap_subtable(face, PlatformId::Unicode, UNICODE_FULL_ENCODING))
        .or_else(|| find_cmap_subtable(face, PlatformId::Unicode, UNICODE_2_0_FULL_ENCODING))
        // 16-bit subtables:
        .or_else(|| find_cmap_subtable(face, PlatformId::Windows, WINDOWS_UNICODE_BMP_ENCODING))
        .or_else(|| find_cmap_subtable(face, PlatformId::Unicode, UNICODE_2_0_BMP_ENCODING))
        .or_else(|| find_cmap_subtable(face, PlatformId::Unicode, UNICODE_ISO_ENCODING))
        .or_else(|| find_cmap_subtable(face, PlatformId::Unicode, UNICODE_1_1_ENCODING))
        .or_else(|| find_cmap_subtable(face, PlatformId::Unicode, UNICODE_1_0_ENCODING))
}

fn find_cmap_subtable(
    face: &ttf_parser::Face,
    platform_id: ttf_parser::PlatformId,
    encoding_id: u16,
) -> Option<u16> {
    for (i, subtable) in face.tables().cmap?.subtables.into_iter().enumerate() {
        if subtable.platform_id == platform_id && subtable.encoding_id == encoding_id {
            return Some(i as u16)
        }
    }

    None
}
