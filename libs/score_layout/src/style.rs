//! The versioned engraving style sheet.
//!
//! Every numeric decision the kernel makes comes from here, so a caller can
//! override any of them. Values are in staff spaces unless noted. Defaults
//! come from three places: the SMuFL `engravingDefaults` convention (font
//! metadata contract — a real font should override these at load time),
//! widely published house-style starting values, and this project's own
//! tuning constants (marked "ours"), which exist to be calibrated against a
//! corpus and are deliberately explicit rather than buried in code.
//!
//! The kernel itself never loads fonts: the style sheet is where a font
//! layer injects font-provided values before layout runs.

use crate::sp::Sp;

/// Bump when the meaning or default of any style field changes, so cached
/// layouts and golden tests can detect a style-schema move.
pub const STYLE_VERSION: u32 = 1;

/// Horizontal spacing constants for the spring-and-rod model.
#[derive(Clone, PartialEq, Debug)]
pub struct SpacingStyle {
    /// Space quanta given to the reference duration ("S"). The natural width
    /// of a column of the reference duration is `spacing_increment * S` plus
    /// its collision headroom. Rationale: anchors the whole duration curve.
    pub shortest_duration_space: f64,
    /// Additive width per duration doubling ("I"). Above the reference
    /// duration, each doubling adds one increment — logarithmic, so a whole
    /// note does not consume eight times an eighth note.
    pub spacing_increment: Sp,
    /// Reference duration `d0` as a fraction of a whole note (1/8 = eighth
    /// note). Above it the curve is logarithmic; below it linear, which
    /// prevents wildly compressed 32nd/64th widths going negative.
    pub reference_duration: f64,
    /// Lower bound for a column's stretch flexibility. Default flexibility
    /// is the natural length itself (longer notes absorb more stretch);
    /// this floor keeps very short columns from becoming rigid.
    pub min_stretch_flex: f64,
    /// Lower bound for a column's shrink flexibility. Default flexibility is
    /// `natural - minimum` (a column compresses toward its rod); this floor
    /// keeps the compression side numerically well-behaved.
    pub min_shrink_flex: f64,
    /// Weight of the equal-duration regularizer. It pulls the whitespace of
    /// rhythmically equivalent neighbour columns together after the force
    /// solve, fixing the classic polyphonic irregularity where equal notes
    /// get unequal gaps. Ours.
    pub regularize_lambda: f64,
    /// Hard cap on how far the regularizer may move any single column, so it
    /// can never erase intentional optical spacing. Ours.
    pub regularize_cap: Sp,
    /// Optical correction when an up-stem column precedes a down-stem one
    /// (stems lean apart; the gap looks too tight). Ours, needs corpus
    /// calibration.
    pub optical_up_down: Sp,
    /// Optical correction for down-stem before up-stem (stems lean together;
    /// the gap looks too wide). Ours.
    pub optical_down_up: Sp,
    /// Cap for the same-direction correction driven by relative notehead
    /// heights. Ours.
    pub optical_same_max: Sp,
    /// Clamp on the total optical correction. Corrections adjust natural
    /// lengths only, never rod minima, so legality is never at stake. Ours.
    pub optical_clamp: Sp,
}

impl Default for SpacingStyle {
    fn default() -> SpacingStyle {
        SpacingStyle {
            shortest_duration_space: 2.0,
            spacing_increment: Sp(1.2),
            reference_duration: 0.125,
            min_stretch_flex: 0.5,
            min_shrink_flex: 0.05,
            regularize_lambda: 2.0,
            regularize_cap: Sp(0.25),
            optical_up_down: Sp(0.15),
            optical_down_up: Sp(-0.10),
            optical_same_max: Sp(0.08),
            optical_clamp: Sp(0.20),
        }
    }
}

/// Line- and page-breaking cost constants.
///
/// The badness machinery follows the Knuth-Plass optimal-fit device (cubic
/// badness, squared demerits); the music-specific penalties are this
/// project's tuning constants.
#[derive(Clone, PartialEq, Debug)]
pub struct BreakStyle {
    /// Scale on cubic stretch badness: `b = min(cap, scale * |r|^3)`.
    pub badness_scale: f64,
    /// Badness saturation, also used as the "infeasible" badness in the
    /// emergency pass.
    pub badness_cap: f64,
    /// Base offset in the demerits `(offset + b)^2`, so even a perfect line
    /// pays a small constant and the DP prefers fewer systems overall.
    pub demerit_offset: f64,
    /// Reject a system compressed past its shrink capacity (`r < -1`)
    /// outside the emergency pass.
    pub min_ratio: f64,
    /// Reject a system stretched past this multiple of its capacity
    /// (`r > 3`) outside the emergency pass.
    pub max_ratio: f64,
    /// Weight of the density-continuity term `(r - r_prev)^2`: adjacent
    /// systems should look equally dense. This is what produces even
    /// gray-level down a page; greedy breaking cannot.
    pub continuity_weight: f64,
    /// Extra quadratic penalty once `|r| > 1` — visible over/under-fullness
    /// costs more than the cubic badness alone conveys. Ours.
    pub overstretch_weight: f64,
    /// Penalty for breaking immediately before a one-measure final system
    /// (widow). Ours.
    pub widow_break_penalty: f64,
    /// Penalty for a final system containing a single measure. Ours.
    pub orphan_measure_penalty: f64,
    /// Final-system fill (`natural / usable`) at or above which the last
    /// system is justified like any other.
    pub last_fill_justify: f64,
    /// Fill below which a ragged last system starts to look wrong; the gap
    /// below this level is charged quadratically.
    pub last_fill_min: f64,
    /// Weight on `max(0, last_fill_min - fill)^2` for sparse last systems.
    pub last_fill_weight: f64,
    /// Representative adjustment ratio for each of the four fitness classes
    /// `(-inf,-0.5) [-0.5,0.5) [0.5,1) [1,inf)`. The continuity term uses
    /// the representative of the previous class so the DP state stays small
    /// and provably optimal for the quantized cost.
    pub class_reps: [f64; 4],
    /// Hard cap on measures per system, bounding DP edge scans even in the
    /// emergency pass. Ours (well above any real system).
    pub max_measures_per_system: usize,
    /// Page: weight of vertical continuity `(r_y - r_y_prev)^2`.
    pub page_continuity_weight: f64,
    /// Page: bonus (negative cost) for turning at a caller-marked good turn,
    /// e.g. after rests.
    pub good_turn_bonus: f64,
    /// Page: penalty for a single system alone on the last page.
    pub page_orphan_penalty: f64,
    /// Page: final-page fill below which it is left at natural height
    /// instead of being stretched.
    pub page_last_fill_min: f64,
    /// Page: weight on squared emptiness `(1 - fill)^2` of an underfull
    /// *non-final* page that cannot (or may not) stretch. A half-empty
    /// middle page is a hard defect; only the last page may legitimately
    /// run short. Ours.
    pub page_sparse_weight: f64,
    /// Page: hard cap on systems per page, bounding DP edge scans. Ours.
    pub max_systems_per_page: usize,
}

impl Default for BreakStyle {
    fn default() -> BreakStyle {
        BreakStyle {
            badness_scale: 100.0,
            badness_cap: 10_000.0,
            demerit_offset: 10.0,
            min_ratio: -1.0,
            max_ratio: 3.0,
            continuity_weight: 40.0,
            overstretch_weight: 250.0,
            widow_break_penalty: 800.0,
            orphan_measure_penalty: 2_000.0,
            last_fill_justify: 0.70,
            last_fill_min: 0.35,
            last_fill_weight: 8_000.0,
            class_reps: [-0.75, 0.0, 0.75, 1.5],
            max_measures_per_system: 64,
            page_continuity_weight: 30.0,
            good_turn_bonus: -200.0,
            page_orphan_penalty: 2_000.0,
            page_last_fill_min: 0.80,
            page_sparse_weight: 8_000.0,
            max_systems_per_page: 32,
        }
    }
}

/// Vertical placement constants (skylines, staff and system distances).
#[derive(Clone, PartialEq, Debug)]
pub struct VerticalStyle {
    /// General clearance added between opposing skylines when no
    /// class-specific clearance applies.
    pub general_clearance: Sp,
    /// Horizontal look-around when querying a skyline: an object also clears
    /// ink slightly beside its own x range, which reads better optically.
    pub skyline_padding: Sp,
    /// Preferred distance between adjacent staves of one system.
    pub staff_distance: Sp,
    /// Minimum distance between adjacent systems.
    pub system_distance_min: Sp,
    /// Maximum distance systems may be stretched apart during page justify.
    pub system_distance_max: Sp,
    /// Space reserved above the top staff of a page/system.
    pub staff_upper_border: Sp,
    /// Space reserved below the bottom staff of a page/system.
    pub staff_lower_border: Sp,
    /// Minimum distance from staff to a lyric line above it.
    pub lyric_top_distance: Sp,
    /// Minimum distance from staff to a lyric line below it.
    pub lyric_bottom_distance: Sp,
    /// Minimum distance between stacked lyric verses.
    pub lyric_verse_distance: Sp,
    /// Geometric tolerance for collision shapes; differences below this are
    /// visually meaningless and treated as equal.
    pub shape_tolerance: Sp,
}

impl Default for VerticalStyle {
    fn default() -> VerticalStyle {
        VerticalStyle {
            general_clearance: Sp(0.5),
            skyline_padding: Sp(0.25),
            staff_distance: Sp(6.5),
            system_distance_min: Sp(8.5),
            system_distance_max: Sp(15.0),
            staff_upper_border: Sp(7.0),
            staff_lower_border: Sp(7.0),
            lyric_top_distance: Sp(1.0),
            lyric_bottom_distance: Sp(1.5),
            lyric_verse_distance: Sp(0.25),
            shape_tolerance: Sp(0.02),
        }
    }
}

/// Slur/tie curve-fitting constants.
///
/// The scoring weights are this project's tuning constants; their essential,
/// tested property is categorical — a collision-free curve always beats a
/// colliding pretty one.
#[derive(Clone, PartialEq, Debug)]
pub struct CurveStyle {
    /// Ribbon thickness at the endpoints (SMuFL `engravingDefaults`
    /// convention; a font layer should override).
    pub thickness_end: Sp,
    /// Ribbon thickness at the midpoint.
    pub thickness_mid: Sp,
    /// Slur preferred height: `clamp(a + b*L, min, max)` where `L` is the
    /// endpoint distance. Ours.
    pub slur_height_base: f64,
    /// Slope of the slur height formula per staff space of length.
    pub slur_height_per_len: f64,
    /// Minimum preferred slur height.
    pub slur_height_min: Sp,
    /// Maximum preferred slur height.
    pub slur_height_max: Sp,
    /// Tie preferred height base (ties are flatter than slurs).
    pub tie_height_base: f64,
    /// Slope of the tie height formula per staff space of length.
    pub tie_height_per_len: f64,
    /// Minimum preferred tie height.
    pub tie_height_min: Sp,
    /// Maximum preferred tie height.
    pub tie_height_max: Sp,
    /// Candidate control-arm fractions of chord length.
    pub arm_fractions: [f64; 4],
    /// Candidate multipliers of the preferred height.
    pub height_multipliers: [f64; 4],
    /// Candidate endpoint offsets along the placement normal, per end.
    pub end_offsets: [Sp; 3],
    /// Preferred control-arm fraction (zero of the arm penalty).
    pub arm_preferred: f64,
    /// Number of arc samples used to score one candidate.
    pub samples: usize,
    /// Soft clearance: penetration is also charged against obstacles
    /// inflated by this margin, so curves keep daylight around ink.
    pub clearance: Sp,
    /// Weight on approximate collision area — must dominate everything.
    pub weight_collision_area: f64,
    /// Weight on squared penetration depth into the clearance zone.
    pub weight_penetration: f64,
    /// Weight on squared endpoint motion away from requested attachments.
    pub weight_end_motion: f64,
    /// Weight on squared deviation from the preferred height.
    pub weight_height: f64,
    /// Weight on squared deviation from the preferred arm fraction.
    pub weight_arm: f64,
    /// Weight on squared excess endpoint tangent beyond the limit (degrees).
    pub weight_tangent: f64,
    /// Endpoint tangent limit in degrees from horizontal.
    pub tangent_limit_deg: f64,
    /// Weight on staff-line nearness (curve segments hugging a staff line).
    pub weight_line_nearness: f64,
    /// Weight on squared endpoint-offset asymmetry.
    pub weight_asymmetry: f64,
    /// Penetration weight multiplier for note cores (heads/stems/accidentals).
    pub obstacle_weight_note: f64,
    /// Penetration weight multiplier for articulations and text.
    pub obstacle_weight_marking: f64,
    /// Penetration weight multiplier for lines and other light ink.
    pub obstacle_weight_line: f64,
}

impl Default for CurveStyle {
    fn default() -> CurveStyle {
        CurveStyle {
            thickness_end: Sp(0.10),
            thickness_mid: Sp(0.22),
            slur_height_base: 0.55,
            slur_height_per_len: 0.12,
            slur_height_min: Sp(0.8),
            slur_height_max: Sp(3.2),
            tie_height_base: 0.30,
            tie_height_per_len: 0.08,
            tie_height_min: Sp(0.30),
            tie_height_max: Sp(1.20),
            arm_fractions: [0.20, 0.25, 0.30, 0.35],
            height_multipliers: [0.75, 1.0, 1.25, 1.5],
            end_offsets: [Sp(0.0), Sp(0.20), Sp(0.40)],
            arm_preferred: 0.30,
            samples: 33,
            clearance: Sp(0.5),
            weight_collision_area: 1_000_000.0,
            weight_penetration: 400.0,
            weight_end_motion: 80.0,
            weight_height: 12.0,
            weight_arm: 8.0,
            weight_tangent: 100.0,
            tangent_limit_deg: 35.0,
            weight_line_nearness: 40.0,
            weight_asymmetry: 25.0,
            obstacle_weight_note: 4.0,
            obstacle_weight_marking: 2.0,
            obstacle_weight_line: 1.0,
        }
    }
}

/// Line/rule thicknesses (SMuFL `engravingDefaults` convention; a music font
/// normally overrides all of these from its own metadata).
#[derive(Clone, PartialEq, Debug)]
pub struct StrokeStyle {
    /// Staff line thickness.
    pub staff_line: Sp,
    /// Stem thickness.
    pub stem: Sp,
    /// Beam thickness.
    pub beam: Sp,
    /// Clear gap between beams (centerlines are `beam + beam_gap` apart).
    pub beam_gap: Sp,
    /// Thin barline thickness.
    pub barline_thin: Sp,
    /// Thick barline thickness.
    pub barline_thick: Sp,
    /// Separation between thin and thick barlines of a final/repeat group.
    pub barline_separation: Sp,
    /// Separation between repeat dots and a barline.
    pub repeat_dot_separation: Sp,
    /// Dashed barline dash length.
    pub barline_dash_length: Sp,
    /// Dashed barline gap length.
    pub barline_dash_gap: Sp,
    /// Ledger line thickness.
    pub ledger: Sp,
    /// Ledger line extension beyond the notehead union on each side.
    pub ledger_extension: Sp,
    /// Hairpin line thickness.
    pub hairpin: Sp,
    /// Tuplet bracket thickness.
    pub tuplet_bracket: Sp,
    /// Group bracket thickness.
    pub bracket: Sp,
    /// Sub-bracket thickness.
    pub sub_bracket: Sp,
    /// Text enclosure thickness.
    pub text_enclosure: Sp,
    /// Lyric extender (melisma line) thickness.
    pub lyric_extender: Sp,
    /// Octave line thickness.
    pub octave_line: Sp,
    /// Pedal line thickness.
    pub pedal_line: Sp,
    /// Multi-measure rest H-bar thickness.
    pub mm_rest_hbar: Sp,
}

impl Default for StrokeStyle {
    fn default() -> StrokeStyle {
        StrokeStyle {
            staff_line: Sp(0.13),
            stem: Sp(0.12),
            beam: Sp(0.50),
            beam_gap: Sp(0.25),
            barline_thin: Sp(0.16),
            barline_thick: Sp(0.50),
            barline_separation: Sp(0.40),
            repeat_dot_separation: Sp(0.16),
            barline_dash_length: Sp(0.50),
            barline_dash_gap: Sp(0.25),
            ledger: Sp(0.16),
            ledger_extension: Sp(0.40),
            hairpin: Sp(0.16),
            tuplet_bracket: Sp(0.16),
            bracket: Sp(0.50),
            sub_bracket: Sp(0.16),
            text_enclosure: Sp(0.16),
            lyric_extender: Sp(0.16),
            octave_line: Sp(0.16),
            pedal_line: Sp(0.16),
            mm_rest_hbar: Sp(1.00),
        }
    }
}

/// House-style distances between objects (published starting values; a
/// publisher style may legitimately override any of them).
#[derive(Clone, PartialEq, Debug)]
pub struct DistanceStyle {
    /// Minimum width of any measure, however empty.
    pub min_measure_width: Sp,
    /// Left margin before a clef at a system start.
    pub clef_left_margin: Sp,
    /// Left margin before a key signature.
    pub key_left_margin: Sp,
    /// Left margin before a time signature.
    pub time_left_margin: Sp,
    /// Distance from clef to key signature.
    pub clef_to_key: Sp,
    /// Distance from key signature to time signature.
    pub key_to_time: Sp,
    /// Minimum whitespace between successive notes.
    pub note_to_note_min: Sp,
    /// Distance from a barline to the first note of a measure.
    pub barline_to_note: Sp,
    /// Distance from the last note of a measure to its barline.
    pub note_to_barline: Sp,
    /// Distance from a barline to a following accidental.
    pub barline_to_accidental: Sp,
    /// Distance between packed accidental columns.
    pub accidental_column: Sp,
    /// Distance from an accidental to its note.
    pub accidental_to_note: Sp,
    /// Default stem length.
    pub stem_default: Sp,
    /// Shortest allowed unbeamed stem.
    pub stem_min_unbeamed: Sp,
    /// Minimum beam/beamlet length.
    pub beamlet_min: Sp,
    /// Distance from a note to its augmentation dot.
    pub note_to_dot: Sp,
    /// Distance from a rest to its augmentation dot.
    pub rest_to_dot: Sp,
    /// Distance between successive augmentation dots.
    pub dot_to_dot: Sp,
    /// Hairpin opening height.
    pub hairpin_height: Sp,
    /// Height of a hairpin continued from a previous system.
    pub hairpin_continued_height: Sp,
    /// Scale factor applied to grace-note glyph and stem geometry.
    pub grace_scale: f64,
    /// Gap between successive grace chords.
    pub grace_to_grace: Sp,
    /// Gap between the last grace chord and the main chord.
    pub grace_to_main: Sp,
    /// Distance from an arpeggio line to the closest notehead.
    pub arpeggio_to_note: Sp,
    /// Minimum clearance kept around slurs and ties.
    pub slur_clearance: Sp,
}

impl Default for DistanceStyle {
    fn default() -> DistanceStyle {
        DistanceStyle {
            min_measure_width: Sp(8.0),
            clef_left_margin: Sp(0.75),
            key_left_margin: Sp(0.50),
            time_left_margin: Sp(0.63),
            clef_to_key: Sp(0.75),
            key_to_time: Sp(1.00),
            note_to_note_min: Sp(0.35),
            barline_to_note: Sp(1.25),
            note_to_barline: Sp(1.50),
            barline_to_accidental: Sp(0.65),
            accidental_column: Sp(0.25),
            accidental_to_note: Sp(0.25),
            stem_default: Sp(3.50),
            stem_min_unbeamed: Sp(2.50),
            beamlet_min: Sp(1.10),
            note_to_dot: Sp(0.50),
            rest_to_dot: Sp(0.25),
            dot_to_dot: Sp(0.65),
            hairpin_height: Sp(1.15),
            hairpin_continued_height: Sp(0.50),
            grace_scale: 0.70,
            grace_to_grace: Sp(0.30),
            grace_to_main: Sp(0.45),
            arpeggio_to_note: Sp(0.40),
            slur_clearance: Sp(0.50),
        }
    }
}

/// The complete versioned style sheet.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LayoutStyle {
    /// Horizontal spacing constants.
    pub spacing: SpacingStyle,
    /// Line/page breaking cost constants.
    pub breaking: BreakStyle,
    /// Vertical placement constants.
    pub vertical: VerticalStyle,
    /// Slur/tie fitting constants.
    pub curve: CurveStyle,
    /// Line/rule thicknesses.
    pub stroke: StrokeStyle,
    /// Object distance defaults.
    pub distance: DistanceStyle,
}

impl LayoutStyle {
    /// The style schema version this build implements ([`STYLE_VERSION`]).
    pub fn version(&self) -> u32 {
        STYLE_VERSION
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = LayoutStyle::default();
        assert_eq!(s.version(), STYLE_VERSION);
        // Spot-check the load-bearing spacing anchors.
        assert_eq!(s.spacing.shortest_duration_space, 2.0);
        assert_eq!(s.spacing.spacing_increment, Sp(1.2));
        assert_eq!(s.spacing.reference_duration, 0.125);
        // Fitness classes must be ordered and bracket the feasible range.
        let reps = s.breaking.class_reps;
        assert!(reps[0] < reps[1] && reps[1] < reps[2] && reps[2] < reps[3]);
        assert!(s.breaking.min_ratio < 0.0 && s.breaking.max_ratio > 1.0);
        // Curve thickness: midpoint thicker than endpoints (ribbon, not
        // stroke).
        assert!(s.curve.thickness_mid > s.curve.thickness_end);
        // Collision must dominate every aesthetic weight by orders of
        // magnitude.
        for w in [
            s.curve.weight_end_motion,
            s.curve.weight_height,
            s.curve.weight_arm,
            s.curve.weight_tangent,
            s.curve.weight_line_nearness,
            s.curve.weight_asymmetry,
        ] {
            assert!(s.curve.weight_collision_area > 1_000.0 * w);
        }
    }
}
