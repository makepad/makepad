use crate::{Ink, InkRole, LinearRgba};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleProvenance {
    pub source: Arc<str>,
    pub version: Arc<str>,
}

/// Bravura-compatible fallback values, in staff spaces.
///
/// A real SMuFL font profile should replace these with its own validated
/// `engravingDefaults`. Keeping provenance beside the values prevents an OTF
/// or metadata upgrade from silently moving approved images.
#[derive(Clone, Debug, PartialEq)]
pub struct EngravingDefaults {
    pub provenance: StyleProvenance,
    pub staff_line_thickness: f64,
    pub stem_thickness: f64,
    pub beam_thickness: f64,
    pub beam_spacing: f64,
    pub slur_endpoint_thickness: f64,
    pub slur_midpoint_thickness: f64,
    pub tie_endpoint_thickness: f64,
    pub tie_midpoint_thickness: f64,
    pub thin_barline_thickness: f64,
    pub thick_barline_thickness: f64,
    pub barline_separation: f64,
    pub repeat_dot_barline_separation: f64,
    pub dashed_barline_thickness: f64,
    pub dashed_barline_dash: f64,
    pub dashed_barline_gap: f64,
    pub ledger_line_thickness: f64,
    pub ledger_line_extension: f64,
    pub hairpin_thickness: f64,
    pub tuplet_bracket_thickness: f64,
    pub repeat_ending_thickness: f64,
    pub bracket_thickness: f64,
    pub sub_bracket_thickness: f64,
    pub text_enclosure_thickness: f64,
    pub lyric_extender_thickness: f64,
    pub octave_line_thickness: f64,
    pub pedal_line_thickness: f64,
    pub arrow_shaft_thickness: f64,
    pub multimeasure_rest_thickness: f64,
    pub default_stem_length: f64,
    pub minimum_beamlet_length: f64,
    pub hairpin_opening: f64,
    pub minimum_curve_clearance: f64,
}

impl Default for EngravingDefaults {
    fn default() -> Self {
        Self {
            provenance: StyleProvenance {
                source: Arc::from("Bravura engravingDefaults fallback"),
                version: Arc::from("1.482 / project rule corpus 2026-08-30"),
            },
            staff_line_thickness: 0.13,
            stem_thickness: 0.12,
            beam_thickness: 0.50,
            beam_spacing: 0.25,
            slur_endpoint_thickness: 0.10,
            slur_midpoint_thickness: 0.22,
            tie_endpoint_thickness: 0.10,
            tie_midpoint_thickness: 0.22,
            thin_barline_thickness: 0.16,
            thick_barline_thickness: 0.50,
            barline_separation: 0.40,
            repeat_dot_barline_separation: 0.16,
            dashed_barline_thickness: 0.16,
            dashed_barline_dash: 0.50,
            dashed_barline_gap: 0.25,
            ledger_line_thickness: 0.16,
            ledger_line_extension: 0.40,
            hairpin_thickness: 0.16,
            tuplet_bracket_thickness: 0.16,
            repeat_ending_thickness: 0.16,
            bracket_thickness: 0.50,
            sub_bracket_thickness: 0.16,
            text_enclosure_thickness: 0.16,
            lyric_extender_thickness: 0.16,
            octave_line_thickness: 0.16,
            pedal_line_thickness: 0.16,
            arrow_shaft_thickness: 0.16,
            multimeasure_rest_thickness: 1.00,
            default_stem_length: 3.50,
            minimum_beamlet_length: 1.10,
            hairpin_opening: 1.15,
            minimum_curve_clearance: 0.50,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PaletteId {
    Light,
    Dark,
    Custom(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScorePalette {
    pub id: PaletteId,
    pub paper: LinearRgba,
    pub surround: LinearRgba,
    pub primary_ink: LinearRgba,
    pub staff_ink: LinearRgba,
    pub secondary_ink: LinearRgba,
    pub playback_cursor: LinearRgba,
    pub playback_wash: LinearRgba,
    /// Selection, annotation and hover are *washes*: they are composited under
    /// the notation, once, so the ink they mark stays fully readable through
    /// them. Their alpha is the finished tint, never a per-copy value to stack.
    pub selection: LinearRgba,
    pub annotation: LinearRgba,
    /// The note the pointer is on, which the app is also sounding. Warm and
    /// deliberately weaker than `selection`, so a reading aid never competes
    /// with a real selection or with the playback bar.
    pub hover: LinearRgba,
}

impl ScorePalette {
    pub fn light() -> Self {
        Self {
            id: PaletteId::Light,
            paper: LinearRgba::from_srgb8(0xF7, 0xF4, 0xEC, 0xFF),
            surround: LinearRgba::from_srgb8(0xD8, 0xD4, 0xCC, 0xFF),
            primary_ink: LinearRgba::from_srgb8(0x17, 0x17, 0x13, 0xFF),
            staff_ink: LinearRgba::from_srgb8(0x22, 0x22, 0x1D, 0xF2),
            secondary_ink: LinearRgba::from_srgb8(0x4A, 0x49, 0x43, 0xE8),
            playback_cursor: LinearRgba::from_srgb8(0xC8, 0x6B, 0x4A, 0xFF),
            playback_wash: LinearRgba::from_srgb8(0xC8, 0x6B, 0x4A, 0x0D),
            selection: LinearRgba::from_srgb8(0x4B, 0x8F, 0xD8, 0x3A),
            annotation: LinearRgba::from_srgb8(0xD4, 0x91, 0x3B, 0x32),
            hover: LinearRgba::from_srgb8(0x3F, 0x7C, 0x6A, 0x24),
        }
    }

    /// Designed dark score colors: charcoal paper and warm ink, never RGB inversion.
    pub fn dark() -> Self {
        Self {
            id: PaletteId::Dark,
            paper: LinearRgba::from_srgb8(0x20, 0x21, 0x1F, 0xFF),
            surround: LinearRgba::from_srgb8(0x10, 0x11, 0x0F, 0xFF),
            primary_ink: LinearRgba::from_srgb8(0xE7, 0xE2, 0xD8, 0xFF),
            staff_ink: LinearRgba::from_srgb8(0xBD, 0xBA, 0xB1, 0xE6),
            secondary_ink: LinearRgba::from_srgb8(0xA8, 0xA4, 0x9B, 0xE0),
            playback_cursor: LinearRgba::from_srgb8(0xE0, 0x82, 0x5F, 0xFF),
            playback_wash: LinearRgba::from_srgb8(0xE0, 0x82, 0x5F, 0x10),
            selection: LinearRgba::from_srgb8(0x70, 0xAE, 0xEA, 0x42),
            annotation: LinearRgba::from_srgb8(0xE0, 0xA6, 0x52, 0x3C),
            hover: LinearRgba::from_srgb8(0x86, 0xC9, 0xB2, 0x2C),
        }
    }

    pub fn resolve(self, ink: Ink) -> LinearRgba {
        if let Some(color) = ink.override_color {
            return color;
        }
        match ink.role {
            InkRole::Primary => self.primary_ink,
            InkRole::Staff => self.staff_ink,
            InkRole::Secondary => self.secondary_ink,
            InkRole::Playback => self.playback_cursor,
            InkRole::Selection => self.selection,
            InkRole::Annotation => self.annotation,
            InkRole::Hover => self.hover,
        }
    }
}

impl Default for ScorePalette {
    fn default() -> Self {
        Self::light()
    }
}

/// Dynamic overlays stay outside cached page geometry but address it by stable ID.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayState {
    pub playback_cursor: Option<crate::PlaybackPosition>,
    pub playback_bar: Option<crate::SemanticId>,
    pub playback_bar_transition: Option<PlaybackBarTransition>,
    pub presentation_time_s: f64,
    pub selected: Vec<crate::SemanticId>,
    pub annotated: Vec<crate::SemanticId>,
    /// The element under the pointer. One id: the hover aid marks what sounds.
    pub hovered: Option<crate::SemanticId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackBarTransition {
    pub from: crate::SemanticId,
    pub to: crate::SemanticId,
    pub started_at_s: f64,
}

impl PlaybackBarTransition {
    pub const DURATION_S: f64 = 0.120;

    pub fn weights(self, now_s: f64) -> (f32, f32) {
        let t = ((now_s - self.started_at_s) / Self::DURATION_S).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        ((1.0 - smooth) as f32, smooth as f32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayMetrics {
    pub playback_cursor_px: f32,
    pub selection_halo_px: f32,
    pub annotation_halo_px: f32,
    pub hover_halo_px: f32,
    pub measure_corner_px: f32,
}

impl Default for OverlayMetrics {
    fn default() -> Self {
        Self {
            playback_cursor_px: 1.5,
            // How far the wash spreads past the ink it marks. It is a single
            // dilated copy, so this is a bloom radius, not a stacking count.
            selection_halo_px: 1.6,
            annotation_halo_px: 1.4,
            hover_halo_px: 2.2,
            measure_corner_px: 4.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_palette_is_semantic_not_inverted() {
        let light = ScorePalette::light();
        let dark = ScorePalette::dark();
        assert!(dark.paper.r < 0.1);
        assert!(dark.primary_ink.r > dark.staff_ink.r);
        assert_ne!(dark.primary_ink.r, 1.0 - light.primary_ink.r);
        assert_eq!(dark.resolve(Ink::role(InkRole::Staff)), dark.staff_ink);
    }

    /// The wash marks the music; it must never be able to hide it. Anything
    /// above a light tint reads as an opaque slab painted over the notation.
    #[test]
    fn selection_and_annotation_stay_translucent_washes() {
        for palette in [ScorePalette::light(), ScorePalette::dark()] {
            assert!(
                palette.selection.a > 0.05 && palette.selection.a < 0.32,
                "selection wash alpha {} is not a tint",
                palette.selection.a
            );
            assert!(
                palette.annotation.a > 0.05 && palette.annotation.a < 0.32,
                "annotation wash alpha {} is not a tint",
                palette.annotation.a
            );
            assert!(
                palette.hover.a > 0.03 && palette.hover.a < palette.selection.a,
                "hover wash {} must be lighter than selection {}",
                palette.hover.a,
                palette.selection.a
            );
            assert!(palette.playback_wash.a < 0.15);
        }
    }

    #[test]
    fn numeric_defaults_match_rule_corpus() {
        let defaults = EngravingDefaults::default();
        assert_eq!(defaults.staff_line_thickness, 0.13);
        assert_eq!(defaults.beam_thickness, 0.50);
        assert_eq!(defaults.slur_midpoint_thickness, 0.22);
        assert_eq!(defaults.ledger_line_extension, 0.40);
        assert_eq!(defaults.hairpin_opening, 1.15);
    }

    #[test]
    fn playback_bar_crossfade_is_120_ms_and_normalized() {
        let transition = PlaybackBarTransition {
            from: crate::SemanticId(1),
            to: crate::SemanticId(2),
            started_at_s: 4.0,
        };
        let (from, to) = transition.weights(4.06);
        assert!((from - 0.5).abs() < 1e-5);
        assert!((from + to - 1.0).abs() < 1e-6);
        let (from, to) = transition.weights(4.13);
        assert_eq!((from, to), (0.0, 1.0));
    }
}
