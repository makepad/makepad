use {
    super::{
        font_face::FontFace,
        geom::{Point, Rect},
        glyph_outline,
        glyph_outline::GlyphOutline,
        glyph_raster_image::GlyphRasterImage,
        intern::Intern,
        loader::FontData,
        rasterizer::{RasterizedGlyph, Rasterizer},
    },
    fxhash::FxHashMap,
    rustybuzz,
    rustybuzz::ttf_parser,
    std::{
        cell::RefCell,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

/// Cap on the per-font glyph-outline cache (distinct glyphs). Large enough that typical Latin
/// usage never reaches it, but bounds CJK/emoji-heavy sessions from growing without limit.
const MAX_CACHED_GLYPH_OUTLINES: usize = 8192;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FontId(u64);

impl From<u64> for FontId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<&str> for FontId {
    fn from(value: &str) -> Self {
        Self(value.intern().as_ptr() as u64)
    }
}

#[derive(Debug)]
pub struct Font {
    id: FontId,
    rasterizer: Rc<RefCell<Rasterizer>>,
    face: FontFace,
    units_per_em: f32,
    ascender_in_ems: f32,
    descender_in_ems: f32,
    line_gap_in_ems: f32,
    cached_glyph_outlines: RefCell<FxHashMap<GlyphId, Option<Rc<GlyphOutline>>>>,
    /// Apple `hvgl` renderer for iOS/macOS system fonts whose composite-part
    /// outlines `ttf_parser` can't decode. Lazily initialized on first outline:
    /// outer `None` = not yet attempted; inner `None` = font has no `hvgl` table
    /// or `libhvf` is unavailable (so fall back to `ttf_parser`).
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    hvgl_renderer: RefCell<Option<Option<super::hvgl_render::HvglRenderer>>>,
    /// CoreText color-emoji rasterizer for iOS/tvOS/macOS color-emoji fonts.
    /// Their color bitmaps (Apple's private `emjc` `sbix` format) can't be
    /// decoded by `ttf_parser`, and on all three platforms the `sbix` table is
    /// stripped from the reassembled sfnt before parsing (see
    /// `sfnt_bytes_from_ctfont`) to avoid ~179MB of resident bitmap data — so we
    /// draw glyphs via a re-queried CoreText font instead. Same lazy-init
    /// convention as `hvgl_renderer`: outer `None` = not yet attempted; inner
    /// `None` = not a color-emoji font or CoreText is unavailable (fall back to
    /// the outline path).
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    color_emoji_renderer:
        RefCell<Option<Option<super::color_emoji_render::ColorEmojiRenderer>>>,
    /// Whether this font was resolved as a color-emoji font. Set at construction
    /// from `FontDefinition::is_color_emoji` (which `ensure_fallback_for_emoji`
    /// sets). Gates the CoreText raster path in place of a physical `sbix` probe,
    /// because the `sbix` table is stripped before parsing on Apple platforms.
    /// Only read on Apple platforms (via `is_color_emoji()`); elsewhere it's an
    /// inert flag, so silence the dead-code lint on non-Apple targets.
    #[cfg_attr(
        not(any(target_os = "ios", target_os = "tvos", target_os = "macos")),
        allow(dead_code)
    )]
    is_color_emoji: bool,
}

impl Font {
    pub fn new(
        id: FontId,
        rasterizer: Rc<RefCell<Rasterizer>>,
        face: FontFace,
        ascender_fudge_in_ems: f32,
        descender_fudge_in_ems: f32,
        is_color_emoji: bool,
    ) -> Self {
        let (units_per_em, ascender_in_ems, descender_in_ems, line_gap_in_ems) = face
            .with_ttf_parser_face(|face| {
                let units_per_em = face.units_per_em() as f32;
                (
                    units_per_em,
                    face.ascender() as f32 / units_per_em + ascender_fudge_in_ems,
                    face.descender() as f32 / units_per_em + descender_fudge_in_ems,
                    face.line_gap() as f32 / units_per_em,
                )
            });
        Self {
            id,
            rasterizer,
            face,
            units_per_em,
            ascender_in_ems,
            descender_in_ems,
            line_gap_in_ems,
            cached_glyph_outlines: RefCell::new(FxHashMap::default()),
            #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
            hvgl_renderer: RefCell::new(None),
            #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
            color_emoji_renderer: RefCell::new(None),
            is_color_emoji,
        }
    }

    pub fn id(&self) -> FontId {
        self.id
    }

    pub fn data(&self) -> &FontData {
        self.face.data()
    }

    pub(super) fn with_ttf_parser_face<R>(&self, f: impl FnOnce(&ttf_parser::Face<'_>) -> R) -> R {
        self.face.with_ttf_parser_face(f)
    }

    pub(super) fn with_rustybuzz_face<R>(&self, f: impl FnOnce(&rustybuzz::Face<'_>) -> R) -> R {
        self.face.with_rustybuzz_face(f)
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    pub fn ascender_in_ems(&self) -> f32 {
        self.ascender_in_ems
    }

    pub fn descender_in_ems(&self) -> f32 {
        self.descender_in_ems
    }

    pub fn line_gap_in_ems(&self) -> f32 {
        self.line_gap_in_ems
    }

    pub fn glyph_outline(&self, glyph_id: GlyphId) -> Option<GlyphOutline> {
        self.glyph_outline_rc(glyph_id)
            .map(|outline| (*outline).clone())
    }

    /// Like [`Self::glyph_outline`], but returns the cache's shared `Rc` so callers
    /// on hot per-frame paths avoid deep-copying the outline's command list.
    pub fn glyph_outline_rc(&self, glyph_id: GlyphId) -> Option<Rc<GlyphOutline>> {
        if let Some(outline) = self.cached_glyph_outlines.borrow().get(&glyph_id) {
            return outline.clone();
        }

        let units_per_em = self.units_per_em;

        // Apple `hvgl` system fonts (iOS/macOS): composite-part outlines that
        // `ttf_parser` can't decode — render them via Apple's `libhvf`.
        #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
        if let Some(outline) = self.hvgl_glyph_outline(glyph_id, units_per_em) {
            let outline = Rc::new(outline);
            self.cache_glyph_outline(glyph_id, Some(outline.clone()));
            return Some(outline);
        }

        let outline = self.with_ttf_parser_face(|face| {
            let gid = ttf_parser::GlyphId(glyph_id);
            let mut builder = glyph_outline::Builder::new();
            let bounds = face.outline_glyph(gid, &mut builder);

            let bounds = bounds?;
            let min = Point::new(bounds.x_min as f32, bounds.y_min as f32);
            let max = Point::new(bounds.x_max as f32, bounds.y_max as f32);
            Some(Rc::new(builder.finish(Rect::new(min, max - min), units_per_em)))
        });

        self.cache_glyph_outline(glyph_id, outline.clone());
        outline
    }

    fn cache_glyph_outline(&self, glyph_id: GlyphId, outline: Option<Rc<GlyphOutline>>) {
        let mut cache = self.cached_glyph_outlines.borrow_mut();
        // Bound the per-font outline cache. The cap is generous, so this only triggers
        // for scripts with thousands of distinct glyphs (e.g. CJK); clearing simply forces
        // the currently-visible glyphs to be re-extracted from the font face on next use.
        if cache.len() >= MAX_CACHED_GLYPH_OUTLINES {
            cache.clear();
        }
        cache.insert(glyph_id, outline);
    }

    /// Render a glyph via Apple's `libhvf`, if this is an `hvgl` system font and
    /// `libhvf` is available. `None` otherwise (caller falls back to `ttf_parser`).
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    fn hvgl_glyph_outline(&self, glyph_id: GlyphId, units_per_em: f32) -> Option<GlyphOutline> {
        use super::hvgl_render::HvglRenderer;

        // Lazily open the renderer (copies the raw `hvgl` bytes into an aligned
        // buffer, so nothing borrows the face afterwards).
        {
            let mut slot = self.hvgl_renderer.borrow_mut();
            if slot.is_none() {
                let opened = self.with_ttf_parser_face(|face| {
                    let hvgl = face.tables().hvgl;
                    hvgl.and_then(|t| HvglRenderer::open(t.data()))
                });
                *slot = Some(opened);
            }
        }

        let slot = self.hvgl_renderer.borrow();
        let renderer = slot.as_ref().unwrap().as_ref()?;

        let mut builder = glyph_outline::Builder::new();
        let bounds = renderer.outline(ttf_parser::GlyphId(glyph_id), &mut builder)?;
        let outline = builder.finish(bounds, units_per_em);
        Some(outline)
    }

    pub fn glyph_outline_bounds_in_ems(
        &self,
        glyph_id: GlyphId,
        out_outline: &mut Option<Rc<GlyphOutline>>,
    ) -> Option<Rect<f32>> {
        // Check the outline cache first — it stores the full outline,
        // from which we can derive bounds.
        if let Some(cached) = self.cached_glyph_outlines.borrow().get(&glyph_id) {
            *out_outline = cached.clone();
            return cached.as_ref().map(|o| o.bounds_in_ems());
        }

        // Not cached yet — compute via glyph_outline_rc() which will populate the cache.
        if let Some(outline) = self.glyph_outline_rc(glyph_id) {
            let bounds_in_ems = outline.bounds_in_ems();
            *out_outline = Some(outline);
            Some(bounds_in_ems)
        } else {
            None
        }
    }

    pub fn with_glyph_raster_image<R>(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
        f: impl FnOnce(GlyphRasterImage<'_>) -> R,
    ) -> Option<R> {
        self.with_ttf_parser_face(|face| {
            let glyph_id = ttf_parser::GlyphId(glyph_id);
            let image = face.glyph_raster_image(glyph_id, dpxs_per_em as u16)?;
            let raster = GlyphRasterImage::from_raster_glyph_image(image)?;
            Some(f(raster))
        })
    }

    pub fn has_glyph_raster_image(&self, glyph_id: GlyphId, dpxs_per_em: f32) -> bool {
        self.with_ttf_parser_face(|face| {
            let glyph_id = ttf_parser::GlyphId(glyph_id);
            face.glyph_raster_image(glyph_id, dpxs_per_em as u16)
                .is_some()
        })
    }

    /// Whether this font was resolved as a color-emoji font (Apple's
    /// `AppleColorEmoji`). On iOS/tvOS/macOS this gates the CoreText color-emoji
    /// path: its glyphs (Apple's private `emjc` `sbix` bitmaps) must stay on the
    /// raster path rather than the slug/outline path, and only these fonts open a
    /// `ColorEmojiRenderer`. We can't probe for a physical `sbix` table because
    /// it's stripped before parsing (see `sfnt_bytes_from_ctfont`), so the flag
    /// is threaded in from `FontDefinition::is_color_emoji`.
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    pub fn is_color_emoji(&self) -> bool {
        self.is_color_emoji
    }

    /// Rasterize a color-emoji glyph via CoreText (`CTFontDrawGlyphs`) if this is
    /// a color-emoji font and CoreText is available; run `f` on the resulting
    /// raster. `None` otherwise (caller falls back to the outline path). Lazily
    /// opens the renderer by re-querying CoreText for the font by its PostScript
    /// name — the reassembled sfnt bytes no longer carry the color bitmaps (the
    /// `sbix` table is stripped to save ~179MB) — see `color_emoji_render`.
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    pub fn with_color_emoji_raster<R>(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
        f: impl FnOnce(&super::color_emoji_render::ColorRaster) -> R,
    ) -> Option<R> {
        use super::color_emoji_render::ColorEmojiRenderer;

        // Lazily open the renderer. Only color-emoji fonts carry color bitmaps,
        // so skip (and remember) anything else — this keeps the CoreText re-query
        // off the common path for the Latin/CJK/etc. text fonts.
        {
            let mut slot = self.color_emoji_renderer.borrow_mut();
            if slot.is_none() {
                let opened = if self.is_color_emoji {
                    // Re-query CoreText for the font by its PostScript name (name
                    // ID 6, retained in the reassembled sfnt). This pins the same
                    // TTC face and gives a lazily-mmapped CTFont with the color
                    // bitmaps intact, without materializing the ~179MB `sbix`.
                    // The glyph count is passed for a defence-in-depth check: if
                    // CoreText resolves a different face (mismatched count), the
                    // renderer declines and we fall back to the outline path
                    // rather than draw wrong glyphs.
                    self.with_ttf_parser_face(|face| {
                        let ps_name = face.names().into_iter().find_map(|name| {
                            if name.name_id != ttf_parser::name_id::POST_SCRIPT_NAME {
                                return None;
                            }
                            // `ttf_parser`'s `to_string()` only decodes Unicode /
                            // Windows platform records (UTF-16BE). Apple Color
                            // Emoji stores its names only on the Macintosh platform
                            // (Roman encoding), which `to_string()` rejects — so we
                            // decode that record's ASCII bytes ourselves. PostScript
                            // names are constrained to printable ASCII by spec.
                            name.to_string().or_else(|| {
                                let raw = name.name;
                                raw.iter().all(|&b| (0x20..0x7f).contains(&b))
                                    .then(|| String::from_utf8_lossy(raw).into_owned())
                            })
                        });
                        let expected_glyph_count = face.number_of_glyphs();
                        ps_name.and_then(|ps_name| {
                            ColorEmojiRenderer::open_by_name(&ps_name, expected_glyph_count)
                        })
                    })
                } else {
                    None
                };
                *slot = Some(opened);
            }
        }

        let slot = self.color_emoji_renderer.borrow();
        let renderer = slot.as_ref().unwrap().as_ref()?;
        let raster = renderer.rasterize(glyph_id, dpxs_per_em)?;
        Some(f(&raster))
    }

    pub fn rasterize_glyph(&self, glyph_id: GlyphId, dpxs_per_em: f32) -> Option<RasterizedGlyph> {
        self.rasterizer
            .borrow_mut()
            .rasterize_glyph(self, glyph_id, dpxs_per_em)
    }

    pub fn rasterize_glyph_stable_fallback(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        self.rasterizer
            .borrow_mut()
            .rasterize_glyph_stable_fallback(self, glyph_id, dpxs_per_em)
    }
}

impl Eq for Font {}

impl Hash for Font {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PartialEq for Font {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

pub type GlyphId = u16;
