use std::collections::HashMap;
use std::sync::Arc;

use makepad_widgets::{DVec2, Vec4f};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpImageKey(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpFontKey(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpExternalImageKey(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpGlyphRunKey(pub u64);

pub type MpTextRunKey = MpGlyphRunKey;

#[derive(Clone, Debug)]
pub enum MpResourceUpdate {
    UpsertImage {
        key: MpImageKey,
        image: MpImageResource,
    },
    DeleteImage(MpImageKey),
    UpsertFont {
        key: MpFontKey,
        font: MpFontResource,
    },
    DeleteFont(MpFontKey),
    UpsertGlyphRun {
        key: MpGlyphRunKey,
        glyph_run: MpGlyphRunResource,
    },
    DeleteGlyphRun(MpGlyphRunKey),
    UpsertExternalImage {
        key: MpExternalImageKey,
        image: MpExternalImageResource,
    },
    DeleteExternalImage(MpExternalImageKey),
}

#[derive(Clone, Debug)]
pub struct MpImageResource {
    pub size: DVec2,
    pub rgba8: Arc<[u8]>,
}

#[derive(Clone, Debug)]
pub struct MpFontResource {
    pub bytes: Arc<[u8]>,
    pub face_index: u32,
}

#[derive(Clone, Debug)]
pub struct MpGlyphRunResource {
    pub text: String,
    pub font_keys: Vec<MpFontKey>,
    pub glyphs: Vec<MpPositionedGlyph>,
    pub metrics: MpGlyphRunMetrics,
    pub decorations: MpTextDecorations,
}

#[derive(Clone, Copy, Debug)]
pub struct MpPositionedGlyph {
    pub glyph_id: u32,
    pub font_size_px: f32,
    pub origin: DVec2,
    pub font_slot: u16,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MpGlyphRunMetrics {
    pub advance_width_px: f32,
    pub baseline_ascent_px: f32,
    pub underline_offset_px: f32,
    pub underline_thickness_px: f32,
    pub strikeout_offset_px: f32,
    pub strikeout_thickness_px: f32,
}

#[derive(Clone, Debug, Default)]
pub struct MpTextDecorations {
    pub background_color: Option<Vec4f>,
    pub decoration_color: Option<Vec4f>,
    pub underline: bool,
    pub overline: bool,
    pub line_through: bool,
    pub shadows: Vec<MpTextShadow>,
}

#[derive(Clone, Copy, Debug)]
pub struct MpTextShadow {
    pub offset: DVec2,
    pub blur_radius_px: f32,
    pub color: Vec4f,
}

#[derive(Clone, Debug)]
pub struct MpExternalImageResource {
    pub size: DVec2,
}

#[derive(Clone, Debug, Default)]
pub struct ResourceRegistry {
    generation: u64,
    pub images: HashMap<MpImageKey, MpImageResource>,
    pub fonts: HashMap<MpFontKey, MpFontResource>,
    pub external_images: HashMap<MpExternalImageKey, MpExternalImageResource>,
}

impl ResourceRegistry {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn upsert_image(&mut self, key: MpImageKey, image: MpImageResource) {
        let changed = self
            .images
            .get(&key)
            .map(|existing| {
                existing.size != image.size
                    || !std::sync::Arc::ptr_eq(&existing.rgba8, &image.rgba8)
            })
            .unwrap_or(true);
        self.images.insert(key, image);
        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub fn upsert_font(&mut self, key: MpFontKey, font: MpFontResource) {
        let changed = self
            .fonts
            .get(&key)
            .map(|existing| {
                existing.face_index != font.face_index
                    || !std::sync::Arc::ptr_eq(&existing.bytes, &font.bytes)
            })
            .unwrap_or(true);
        self.fonts.insert(key, font);
        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub fn upsert_external_image(
        &mut self,
        key: MpExternalImageKey,
        image: MpExternalImageResource,
    ) {
        let changed = self
            .external_images
            .get(&key)
            .map(|existing| existing.size != image.size)
            .unwrap_or(true);
        self.external_images.insert(key, image);
        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
    }
}

impl From<&MpResourceStore> for ResourceRegistry {
    fn from(store: &MpResourceStore) -> Self {
        Self {
            generation: 1,
            images: store.images.clone(),
            fonts: store.fonts.clone(),
            external_images: store.external_images.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MpResourceStore {
    pub images: HashMap<MpImageKey, MpImageResource>,
    pub fonts: HashMap<MpFontKey, MpFontResource>,
    pub glyph_runs: HashMap<MpGlyphRunKey, MpGlyphRunResource>,
    pub external_images: HashMap<MpExternalImageKey, MpExternalImageResource>,
}

impl MpResourceStore {
    pub fn apply(&mut self, updates: impl IntoIterator<Item = MpResourceUpdate>) {
        for update in updates {
            match update {
                MpResourceUpdate::UpsertImage { key, image } => {
                    self.images.insert(key, image);
                }
                MpResourceUpdate::DeleteImage(key) => {
                    self.images.remove(&key);
                }
                MpResourceUpdate::UpsertFont { key, font } => {
                    self.fonts.insert(key, font);
                }
                MpResourceUpdate::DeleteFont(key) => {
                    self.fonts.remove(&key);
                }
                MpResourceUpdate::UpsertGlyphRun { key, glyph_run } => {
                    self.glyph_runs.insert(key, glyph_run);
                }
                MpResourceUpdate::DeleteGlyphRun(key) => {
                    self.glyph_runs.remove(&key);
                }
                MpResourceUpdate::UpsertExternalImage { key, image } => {
                    self.external_images.insert(key, image);
                }
                MpResourceUpdate::DeleteExternalImage(key) => {
                    self.external_images.remove(&key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::dvec2;
    use std::sync::Arc;

    #[test]
    fn apply_updates_glyph_run_resources() {
        let mut store = MpResourceStore::default();
        store.apply([
            MpResourceUpdate::UpsertFont {
                key: MpFontKey(1),
                font: MpFontResource {
                    bytes: Arc::from(vec![1, 2, 3]),
                    face_index: 0,
                },
            },
            MpResourceUpdate::UpsertGlyphRun {
                key: MpGlyphRunKey(2),
                glyph_run: MpGlyphRunResource {
                    text: "abc".to_string(),
                    font_keys: vec![MpFontKey(1)],
                    glyphs: vec![MpPositionedGlyph {
                        glyph_id: 7,
                        font_size_px: 12.0,
                        origin: dvec2(0.0, 8.0),
                        font_slot: 0,
                    }],
                    metrics: MpGlyphRunMetrics {
                        advance_width_px: 12.0,
                        baseline_ascent_px: 8.0,
                        underline_offset_px: 2.0,
                        underline_thickness_px: 1.0,
                        strikeout_offset_px: 4.0,
                        strikeout_thickness_px: 1.0,
                    },
                    decorations: MpTextDecorations::default(),
                },
            },
        ]);

        assert!(store.fonts.contains_key(&MpFontKey(1)));
        assert_eq!(store.glyph_runs[&MpGlyphRunKey(2)].text, "abc");
    }

    #[test]
    fn resource_registry_bumps_generation_on_identity_change() {
        let mut registry = ResourceRegistry::default();
        let initial = registry.generation();
        let bytes: Arc<[u8]> = Arc::from(vec![1, 2, 3]);
        registry.upsert_font(
            MpFontKey(1),
            MpFontResource {
                bytes: bytes.clone(),
                face_index: 0,
            },
        );
        let after_first_insert = registry.generation();
        registry.upsert_font(
            MpFontKey(1),
            MpFontResource {
                bytes: bytes.clone(),
                face_index: 0,
            },
        );
        let after_same_insert = registry.generation();
        registry.upsert_font(
            MpFontKey(1),
            MpFontResource {
                bytes: Arc::from(vec![4, 5, 6]),
                face_index: 0,
            },
        );

        assert!(after_first_insert > initial);
        assert_eq!(after_same_insert, after_first_insert);
        assert!(registry.generation() > after_same_insert);
    }
}
