use crate::smufl::{FontMetadata, GlyphClasses, GlyphRanges, GlyphRegistry, SmuflError};
use crate::symbol::{
    Accidental, Articulation, Clef, Digit, Direction, DynamicMark, FermataShape, FlagDuration,
    NoteheadDuration, NoteheadShape, Ornament, Placement, RestDuration, Symbol, TremoloStrokes,
};
use crate::units::{
    DesignUnits, FontMetrics, LayoutPoint, StaffPoint, StaffSize, StaffSpaces, StaffStep,
};

const ENGRAVING_DEFAULTS: &str = r#"
    "engravingDefaults": {
        "staffLineThickness": 0.13,
        "stemThickness": 0.12,
        "beamThickness": 0.5,
        "beamSpacing": 0.25,
        "legerLineThickness": 0.16,
        "legerLineExtension": 0.4,
        "slurEndpointThickness": 0.1,
        "slurMidpointThickness": 0.22,
        "tieEndpointThickness": 0.1,
        "tieMidpointThickness": 0.22,
        "thinBarlineThickness": 0.16,
        "thickBarlineThickness": 0.5,
        "barlineSeparation": 0.4,
        "repeatBarlineDotSeparation": 0.16,
        "bracketThickness": 0.5,
        "subBracketThickness": 0.16,
        "hairpinThickness": 0.16,
        "octaveLineThickness": 0.16,
        "pedalLineThickness": 0.16,
        "tupletBracketThickness": 0.16,
        "textEnclosureThickness": 0.16,
        "arrowShaftThickness": 0.16,
        "hBarThickness": 1.0,
        "dashedBarlineThickness": 0.16,
        "dashedBarlineDashLength": 0.5,
        "dashedBarlineGapLength": 0.25,
        "lyricLineThickness": 0.16,
        "repeatEndingLineThickness": 0.16,
        "thinThickBarlineSeparation": 0.4,
        "textFontFamily": ["Example Serif", "serif"],
        "futureThickness": 0.7,
        "futureStructuredValue": {"ignored": true}
    }
"#;

fn metadata_fixture() -> Vec<u8> {
    format!(
        r#"{{
            {ENGRAVING_DEFAULTS},
            "fontName": "Example Music",
            "fontVersion": 1.25,
            "glyphAdvanceWidths": {{"noteheadBlack": 1.18}},
            "glyphBBoxes": {{
                "noteheadBlack": {{"bBoxNE": [1.18, 0.5], "bBoxSW": [0, -0.5]}}
            }},
            "glyphsWithAnchors": {{
                "noteheadBlack": {{
                    "stemUpSE": [1.18, 0.168],
                    "stemDownNW": [0, -0.168],
                    "numeralTop": [0.5, 1.0],
                    "futureAnchor": [0.25, -0.75]
                }}
            }},
            "glyphsWithAlternates": {{
                "noteheadBlack": {{
                    "alternates": [{{"name": "noteheadBlackSmall", "codepoint": "U+F46A"}}]
                }}
            }},
            "ligatures": {{
                "noteheadBlackParens": {{
                    "codepoint": "U+F5E6",
                    "componentGlyphs": ["noteheadParenthesisLeft", "noteheadBlack", "noteheadParenthesisRight"],
                    "description": "Parenthesised black notehead"
                }}
            }},
            "optionalGlyphs": {{
                "noteheadBlackSmall": {{
                    "classes": ["noteheads"],
                    "codepoint": "U+F46A",
                    "description": "Small black notehead"
                }}
            }},
            "sets": {{
                "ss01": {{
                    "description": "Small staff",
                    "glyphs": [{{
                        "alternateFor": "noteheadBlack",
                        "codepoint": "U+F46A",
                        "description": "Small black notehead",
                        "name": "noteheadBlackSmall"
                    }}]
                }}
            }},
            "futureSection": {{"anything": [1, 2, 3]}}
        }}"#
    )
    .into_bytes()
}

#[test]
fn staff_space_conversions_use_four_spaces_per_em() {
    let metrics = FontMetrics::from_units_per_em(1000).unwrap();
    assert_eq!(
        StaffSpaces::from_design_units(DesignUnits::new(250.0), metrics),
        StaffSpaces::new(1.0)
    );
    assert_eq!(
        StaffSpaces::new(1.5).to_design_units(metrics),
        DesignUnits::new(375.0)
    );

    let staff_size = StaffSize::new(28.0).unwrap();
    assert_eq!(StaffSpaces::new(1.5).to_layout_points(staff_size), 10.5);
    assert_eq!(
        StaffSpaces::from_layout_points(10.5, staff_size),
        StaffSpaces::new(1.5)
    );
    assert!(FontMetrics::from_units_per_em(0).is_none());
    assert!(StaffSize::new(0.0).is_none());
}

#[test]
fn coordinates_and_staff_steps_follow_the_documented_axes() {
    let size = StaffSize::from_points_per_space(6.0).unwrap();
    let score = StaffPoint::new(StaffSpaces::new(2.0), StaffSpaces::new(1.5));
    let layout = score.to_layout_point(size);
    assert_eq!(layout, LayoutPoint { x: 12.0, y: -9.0 });
    assert_eq!(StaffPoint::from_layout_point(layout, size), score);
    assert_eq!(StaffStep::new(3).to_staff_spaces(), StaffSpaces::new(1.5));
    assert_eq!(StaffStep::new(-2).to_staff_spaces(), StaffSpaces::new(-1.0));
}

#[test]
fn glyph_registry_is_bidirectional() {
    let bytes = br#"{
        "noteheadBlack": {"codepoint": "U+E0A4", "description": "Black notehead"},
        "restQuarter": {"codepoint": "U+E4E5", "description": "Quarter rest"}
    }"#;
    let registry = GlyphRegistry::from_bytes(bytes).unwrap();
    assert_eq!(registry.len(), 2);
    assert_eq!(registry.codepoint_for_name("noteheadBlack"), Some('\u{e0a4}'));
    assert_eq!(registry.name_for_codepoint('\u{e4e5}'), Some("restQuarter"));
    assert_eq!(
        registry.glyph_for_codepoint('\u{e0a4}').unwrap().description,
        "Black notehead"
    );
}

#[test]
fn ranges_and_classes_support_semantic_reverse_lookups() {
    let ranges = GlyphRanges::from_bytes(
        br#"{
            "noteheads": {
                "description": "Noteheads",
                "glyphs": ["noteheadBlack", "noteheadHalf"],
                "range_start": "U+E0A0",
                "range_end": "U+E0FF"
            }
        }"#,
    )
    .unwrap();
    assert!(ranges.contains("noteheads", "noteheadBlack"));
    assert_eq!(ranges.ranges_for_glyph("noteheadHalf"), &["noteheads"]);
    assert!(ranges.get("noteheads").unwrap().contains_codepoint('\u{e0a4}'));

    let classes = GlyphClasses::from_bytes(
        br#"{
            "noteheads": ["noteheadBlack", "noteheadHalf"],
            "rests": ["restQuarter"]
        }"#,
    )
    .unwrap();
    assert!(classes.is_notehead("noteheadBlack"));
    assert!(!classes.is_notehead("restQuarter"));
    assert_eq!(classes.classes_for_glyph("restQuarter"), &["rests"]);
}

#[test]
fn font_metadata_loads_typed_measurements_and_collections() {
    let metadata = FontMetadata::from_bytes(&metadata_fixture()).unwrap();
    assert_eq!(metadata.font_name.as_deref(), Some("Example Music"));
    assert_eq!(metadata.font_version, Some(1.25));
    assert_eq!(
        metadata.engraving_defaults.staff_line_thickness,
        StaffSpaces::new(0.13)
    );
    assert_eq!(
        metadata.engraving_defaults.other_measurements["futureThickness"],
        StaffSpaces::new(0.7)
    );
    assert_eq!(
        metadata.glyph_advance_widths["noteheadBlack"],
        StaffSpaces::new(1.18)
    );

    let bounds = metadata.glyph_bboxes["noteheadBlack"];
    assert_eq!(bounds.width(), StaffSpaces::new(1.18));
    assert_eq!(bounds.height(), StaffSpaces::new(1.0));
    let anchors = &metadata.glyphs_with_anchors["noteheadBlack"];
    assert_eq!(
        anchors.stem_up_se,
        Some(StaffPoint::new(
            StaffSpaces::new(1.18),
            StaffSpaces::new(0.168)
        ))
    );
    assert_eq!(
        anchors.other["futureAnchor"],
        StaffPoint::new(StaffSpaces::new(0.25), StaffSpaces::new(-0.75))
    );
    assert_eq!(
        metadata.glyphs_with_alternates["noteheadBlack"].alternates[0].codepoint,
        '\u{f46a}'
    );
    assert_eq!(
        metadata.ligatures["noteheadBlackParens"].component_glyphs.len(),
        3
    );
    assert_eq!(
        metadata.optional_glyphs["noteheadBlackSmall"].classes,
        ["noteheads"]
    );
    assert_eq!(metadata.sets["ss01"].glyphs[0].alternate_for.as_deref(), Some("noteheadBlack"));
}

#[test]
fn font_metadata_allows_omitted_optional_sections() {
    let fixture = format!(r#"{{{ENGRAVING_DEFAULTS}, "unknown": true}}"#);
    let metadata = FontMetadata::from_bytes(fixture.as_bytes()).unwrap();
    assert!(metadata.glyph_advance_widths.is_empty());
    assert!(metadata.glyph_bboxes.is_empty());
    assert!(metadata.glyphs_with_anchors.is_empty());
    assert!(metadata.glyphs_with_alternates.is_empty());
    assert!(metadata.ligatures.is_empty());
    assert!(metadata.optional_glyphs.is_empty());
    assert!(metadata.sets.is_empty());
}

#[test]
fn malformed_and_truncated_json_are_errors() {
    let error = FontMetadata::from_bytes(br#"{"engravingDefaults":{"staffLineThickness":0.1"#)
        .unwrap_err();
    assert!(matches!(error, SmuflError::Json { .. }));

    let error = GlyphRegistry::from_bytes(
        br#"{"one":{"codepoint":"U+E000","description":"one"}} trailing"#,
    )
    .unwrap_err();
    assert!(matches!(error, SmuflError::Json { .. }));
}

#[test]
fn wrong_types_and_missing_required_sections_are_errors() {
    let missing = FontMetadata::from_bytes(br#"{}"#).unwrap_err();
    assert!(matches!(
        missing,
        SmuflError::MissingField { ref path } if path == "fontMetadata.engravingDefaults"
    ));

    let wrong_type = format!(
        r#"{{{}}}"#,
        ENGRAVING_DEFAULTS.replace("\"staffLineThickness\": 0.13", "\"staffLineThickness\": \"thin\"")
    );
    let error = FontMetadata::from_bytes(wrong_type.as_bytes()).unwrap_err();
    assert!(matches!(
        error,
        SmuflError::WrongType { ref path, .. }
            if path == "fontMetadata.engravingDefaults.staffLineThickness"
    ));

    let error = GlyphClasses::from_bytes(br#"{"noteheads": "not an array"}"#).unwrap_err();
    assert!(matches!(error, SmuflError::WrongType { .. }));
    assert!(matches!(
        GlyphRegistry::from_bytes(&[0xff]).unwrap_err(),
        SmuflError::Utf8
    ));
}

#[test]
fn invalid_and_duplicate_codepoints_are_reported() {
    let invalid = GlyphRegistry::from_bytes(
        br#"{"bad":{"codepoint":"E000","description":"bad"}}"#,
    )
    .unwrap_err();
    assert!(matches!(invalid, SmuflError::InvalidCodepoint { .. }));

    let duplicate = GlyphRegistry::from_bytes(
        br#"{
            "one":{"codepoint":"U+E000","description":"one"},
            "two":{"codepoint":"U+E000","description":"two"}
        }"#,
    )
    .unwrap_err();
    assert!(matches!(duplicate, SmuflError::DuplicateCodepoint { .. }));
}

#[test]
fn symbols_round_trip_through_canonical_names() {
    let symbols = [
        Symbol::Notehead {
            duration: NoteheadDuration::DoubleWhole,
            shape: NoteheadShape::Diamond,
        },
        Symbol::Notehead {
            duration: NoteheadDuration::Black,
            shape: NoteheadShape::Normal,
        },
        Symbol::Rest(RestDuration::OneThousandTwentyFourth),
        Symbol::Accidental(Accidental::ThreeQuarterTonesSharp),
        Symbol::Clef(Clef::G8vb),
        Symbol::Flag {
            duration: FlagDuration::ThirtySecond,
            direction: Direction::Down,
        },
        Symbol::Articulation {
            articulation: Articulation::MarcatoTenuto,
            placement: Placement::Above,
        },
        Symbol::Dynamic(DynamicMark::SforzandoPiano),
        Symbol::TimeSignatureDigit(Digit::Seven),
        Symbol::TimeSignatureCommon,
        Symbol::TupletDigit(Digit::Three),
        Symbol::Ornament(Ornament::InvertedTurn),
        Symbol::Fermata {
            shape: FermataShape::VeryLong,
            placement: Placement::Below,
        },
        Symbol::Tremolo(TremoloStrokes::Three),
        Symbol::AugmentationDot,
        Symbol::Arpeggio(Direction::Up),
        Symbol::Other("futureCanonicalGlyph".to_string()),
    ];

    for symbol in symbols {
        let name = symbol.canonical_name().to_string();
        assert_eq!(Symbol::from_canonical_name(&name), symbol, "{name}");
    }
}

#[test]
fn unknown_symbol_names_remain_lossless() {
    let symbol = Symbol::from_canonical_name("vendorIndependentFutureGlyph");
    assert_eq!(
        symbol,
        Symbol::Other("vendorIndependentFutureGlyph".to_string())
    );
    assert_eq!(symbol.canonical_name(), "vendorIndependentFutureGlyph");
}
