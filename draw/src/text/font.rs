use {
    super::{
        font_face::FontFace,
        rasterizer::{RasterizedGlyph, Rasterizer},
    },
    makepad_font_core::font::Font as CoreFont,
    std::{
        cell::RefCell,
        hash::{Hash, Hasher},
        ops::Deref,
        rc::Rc,
    },
};

pub use makepad_font_core::font::{FontId, GlyphId};

/// A font as the text stack uses it: the shared font tables and outlines
/// ([`makepad_font_core::font::Font`], reached through `Deref`) and the atlas its glyphs are
/// rasterised into.
#[derive(Debug)]
pub struct Font {
    core: CoreFont,
    rasterizer: Rc<RefCell<Rasterizer>>,
}

impl Deref for Font {
    type Target = CoreFont;

    fn deref(&self) -> &CoreFont {
        &self.core
    }
}

impl Font {
    pub(super) fn worker_definition(&self) -> super::async_labels::FontSnapshot {
        let (data, index, variations) = self.core.face().worker_source();
        let units_per_em = self.core.units_per_em();
        let (ascender, descender) = self.core.with_ttf_parser_face(|f| {
            (f.ascender() as f32 / units_per_em, f.descender() as f32 / units_per_em)
        });
        super::async_labels::FontSnapshot {
            id: self.core.id(),
            data,
            index,
            variations,
            ascender_fudge: self.core.ascender_in_ems() - ascender,
            descender_fudge: self.core.descender_in_ems() - descender,
        }
    }

    pub fn new(
        id: FontId,
        rasterizer: Rc<RefCell<Rasterizer>>,
        face: FontFace,
        ascender_fudge_in_ems: f32,
        descender_fudge_in_ems: f32,
    ) -> Self {
        Self {
            core: CoreFont::new(id, face, ascender_fudge_in_ems, descender_fudge_in_ems),
            rasterizer,
        }
    }

    pub fn rasterize_glyph(&self, glyph_id: GlyphId, dpxs_per_em: f32) -> Option<RasterizedGlyph> {
        self.rasterizer
            .borrow_mut()
            .rasterize_glyph(self, glyph_id, dpxs_per_em)
    }
}

impl Eq for Font {}

impl Hash for Font {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.core.id().hash(state);
    }
}

impl PartialEq for Font {
    fn eq(&self, other: &Self) -> bool {
        self.core.id() == other.core.id()
    }
}

#[cfg(test)]
mod tests {
    use super::{Font, FontId};
    use crate::{
        makepad_platform::SharedBytes,
        text::{
            font_face::FontFace,
            layouter,
            loader::FontData,
            rasterizer::Rasterizer,
        },
    };
    #[cfg(target_arch = "wasm32")]
    use crate::text::rasterizer::AtlasKind;
    use std::{cell::RefCell, path::PathBuf, rc::Rc};

    fn bundled_emoji_font_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../widgets/resources/NotoColorEmoji.ttf")
    }

    fn bundled_font_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../widgets/resources")
            .join(name)
    }

    fn load_font_data(path: PathBuf) -> FontData {
        SharedBytes::from_file_mmap_or_read(path).expect("font bytes should load")
    }

    fn make_font(path: PathBuf) -> Font {
        Font::new(
            FontId::from(0xE0E1_u64),
            Rc::new(RefCell::new(Rasterizer::new(
                layouter::Settings::default().loader.rasterizer,
            ))),
            FontFace::from_data_and_index(load_font_data(path), 0).expect("font face should load"),
            0.0,
            0.0,
        )
    }

    /// The theme's text faces have to hand out a cap height, or every boxed
    /// label falls back to line-box centering and sits high again.
    #[test]
    fn text_faces_report_a_cap_height() {
        for name in [
            "IBMPlexSans-Text.ttf",
            "IBMPlexSans-SemiBold.ttf",
            "LiberationMono-Regular.ttf",
            "NotoSans-Regular.ttf",
        ] {
            let cap = make_font(bundled_font_path(name)).cap_height_in_ems();
            assert!(
                (0.5..0.85).contains(&cap),
                "{name}: cap height {cap} ems is not a plausible capital"
            );
        }
    }

    /// The fudge the themes apply to the ascender is a line-box tweak; the ink
    /// metric must not move with it, or the centering it feeds would chase its
    /// own tail.
    #[test]
    fn cap_height_ignores_the_ascender_fudge() {
        let path = bundled_font_path("IBMPlexSans-Text.ttf");
        let plain = make_font(path.clone());
        let fudged = Font::new(
            FontId::from(0xE0E2_u64),
            Rc::new(RefCell::new(Rasterizer::new(
                layouter::Settings::default().loader.rasterizer,
            ))),
            FontFace::from_data_and_index(load_font_data(path), 0).expect("font face should load"),
            -0.1,
            0.0,
        );
        assert_eq!(plain.cap_height_in_ems(), fudged.cap_height_in_ems());
        assert!((plain.ascender_in_ems() - fudged.ascender_in_ems() - 0.1).abs() < 1e-6);
    }

    #[test]
    fn noto_color_emoji_prefers_raster_images() {
        let font = make_font(bundled_emoji_font_path());
        let glyph_id = font
            .with_ttf_parser_face(|face| face.glyph_index('😀').map(|glyph| glyph.0))
            .expect("emoji glyph should exist");
        let dpxs_per_em = 128.0;

        assert!(
            font.has_glyph_raster_image(glyph_id, dpxs_per_em),
            "emoji glyph should expose a raster image"
        );

        // The rasteriser draws the bitmap into the atlas it owns; under the HAL the host's font
        // service does (its tests render this same glyph).
        #[cfg(target_arch = "wasm32")]
        {
            let rasterized = font
                .rasterize_glyph(glyph_id, dpxs_per_em)
                .expect("emoji glyph should rasterize");
            assert_eq!(rasterized.atlas_kind, AtlasKind::Color);
        }
    }

    #[test]
    fn jetbrains_ui_symbol_fallback_has_permissive_license_and_required_cmap() {
        let chars = ['⌘', '⇧', '⌥', '⌃', '⏎', '←', '→', '↑', '↓', '•', '…'];
        let font = make_font(bundled_font_path("jetbrains_mono_variable.ttf"));
        font.with_ttf_parser_face(|face| {
            let license = face
                .names()
                .into_iter()
                .find(|record| record.name_id == 13)
                .and_then(|record| record.to_string())
                .expect("UI fallback must carry its license in the font name table");
            assert!(license.contains("SIL Open Font License, Version 1.1"));
            for ch in chars {
                assert!(face.glyph_index(ch).is_some(), "UI fallback is missing {ch:?}");
            }
        });
    }
}
