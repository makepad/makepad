use crate::*;
use makepad_pdf_parse::{ObjRef, PdfOp};
use makepad_score::model::{Alter, Pitch, Step};
use makepad_score::symbol::Clef;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

fn source(operator: u32) -> SourceSpan {
    SourceSpan {
        object: ObjRef { num: 4, gen: 0 },
        stream_index: 0,
        decoded_bytes: operator * 10..operator * 10 + 9,
        operator_index: operator,
        subpath_index: Some(0),
        form_chain: Vec::new(),
    }
}

fn stroked_line(id: u64, start: Point, end: Point, width: f64) -> (PrimitiveId, DisplayPrimitive) {
    let primitive = PrimitiveId(id);
    (
        primitive,
        DisplayPrimitive::Path(PdfPath {
            commands: vec![PathCommand::Move(start), PathCommand::Line(end)],
            bounds: Rect::from_points(start, end),
            paint: PathPaint::Stroke,
            clip: None,
            line_width: width,
            stroke_color: Color::Gray(0.0),
            fill_color: Color::Gray(0.0),
            command_sources: vec![source(id as u32), source(id as u32)],
            paint_source: source(id as u32),
        }),
    )
}

fn synthetic_display(primitives: Vec<(PrimitiveId, DisplayPrimitive)>) -> DisplayList {
    DisplayList {
        page: PageIndex(0),
        page_object: ObjRef { num: 3, gen: 0 },
        media_box: Rect::new(0.0, 0.0, 200.0, 200.0),
        crop_box: Rect::new(0.0, 0.0, 200.0, 200.0),
        rotation: 0,
        content_streams: Vec::new(),
        operators: vec![OperatorRecord {
            source: source(0),
            raw: b"q".to_vec(),
            operation: RetainedOperator::Parsed(PdfOp::SaveState),
        }],
        primitives,
        fonts: HashMap::new(),
    }
}

#[test]
fn staff_clustering_rejects_unrelated_long_horizontal() {
    let mut primitives = Vec::new();
    let mut id = 1;
    for base in [50.0, 110.0] {
        for line in 0..5 {
            primitives.push(stroked_line(
                id,
                Point::new(20.0, base + line as f64 * 5.0),
                Point::new(180.0, base + line as f64 * 5.0),
                0.5,
            ));
            id += 1;
        }
    }
    primitives.push(stroked_line(
        id,
        Point::new(5.0, 190.0),
        Point::new(195.0, 190.0),
        0.5,
    ));
    let geometry = recover_page_geometry(&synthetic_display(primitives));
    assert_eq!(geometry.staves.len(), 2);
    assert!(geometry
        .staves
        .iter()
        .all(|staff| (staff.staff_space - 5.0).abs() < 1e-9));
}

#[test]
fn barline_requires_endpoints_on_two_different_staves() {
    let mut primitives = Vec::new();
    let mut id = 1;
    for base in [50.0, 100.0] {
        for line in 0..5 {
            primitives.push(stroked_line(
                id,
                Point::new(20.0, base + line as f64 * 5.0),
                Point::new(180.0, base + line as f64 * 5.0),
                0.5,
            ));
            id += 1;
        }
    }
    primitives.push(stroked_line(
        id,
        Point::new(50.0, 70.0),
        Point::new(50.0, 100.0),
        0.5,
    ));
    id += 1;
    // A stem entirely within one staff is deliberately longer than normal.
    primitives.push(stroked_line(
        id,
        Point::new(80.0, 50.0),
        Point::new(80.0, 70.0),
        0.5,
    ));
    let geometry = recover_page_geometry(&synthetic_display(primitives));
    assert_eq!(geometry.barlines.len(), 1);
    assert!((geometry.barlines[0].x - 50.0).abs() < 1e-9);
}

#[test]
fn clef_lookup_uses_the_last_clef_to_the_left() {
    let clefs = vec![
        ClefPlacement {
            primitive: PrimitiveId(1),
            staff: 0,
            x: 10.0,
            reference_staff_step: 2,
            clef: Estimate::certain(Clef::G, Evidence::SmuflName("gClef".to_string())),
        },
        ClefPlacement {
            primitive: PrimitiveId(2),
            staff: 0,
            x: 80.0,
            reference_staff_step: 6,
            clef: Estimate::certain(Clef::F, Evidence::SmuflName("fClef".to_string())),
        },
    ];
    assert_eq!(clef_in_force(&clefs, 0, 79.0).unwrap().clef.value, Clef::G);
    assert_eq!(clef_in_force(&clefs, 0, 80.0).unwrap().clef.value, Clef::F);
}

#[test]
fn accidental_scope_resets_at_measure_boundary() {
    let mut notes = vec![
        ScopeNote {
            measure: 0,
            x_milli: 20_000,
            staff_step: 4,
            diatonic_absolute: 30,
            alter: 0,
        },
        ScopeNote {
            measure: 0,
            x_milli: 40_000,
            staff_step: 4,
            diatonic_absolute: 30,
            alter: 0,
        },
        ScopeNote {
            measure: 1,
            x_milli: 20_000,
            staff_step: 4,
            diatonic_absolute: 30,
            alter: 0,
        },
    ];
    let accidental = ScopeAccidental {
        primitive: PrimitiveId(9),
        measure: 0,
        x_milli: 10_000,
        staff_step: 4,
        kind: AccidentalKind::Sharp,
    };
    apply_accidental_scope(&mut notes, &[accidental], [0; 7]);
    assert_eq!(notes.iter().map(|note| note.alter).collect::<Vec<_>>(), vec![1, 1, 0]);
}

#[test]
fn tie_and_slur_are_separated_by_endpoint_pitch() {
    let c4 = Pitch::new(Step::C, Alter::NATURAL, 4);
    let d4 = Pitch::new(Step::D, Alter::NATURAL, 4);
    assert_eq!(
        classify_curve_relation(Some(c4), Some(c4), 3.0, 0.4).value,
        CurveKind::Tie
    );
    assert_eq!(
        classify_curve_relation(Some(c4), Some(d4), 3.0, 0.4).value,
        CurveKind::Slur
    );
}

#[test]
fn splice_bounds_are_the_union_of_dependency_primitives() {
    let display = synthetic_display(vec![
        stroked_line(1, Point::new(10.0, 20.0), Point::new(15.0, 25.0), 0.5),
        stroked_line(2, Point::new(30.0, 40.0), Point::new(35.0, 45.0), 0.5),
    ]);
    let primitives = BTreeSet::from([PrimitiveId(1), PrimitiveId(2)]);
    let style = StyleProfile {
        staff_line_thickness: 0.5,
        ..StyleProfile::default()
    };
    assert_eq!(
        minimal_patch_bounds(&display, &primitives, &style),
        Some(Rect::new(9.0, 19.0, 36.0, 46.0))
    );
}

#[test]
fn incremental_export_preserves_the_original_prefix_and_reparses() {
    let original: Arc<[u8]> = Arc::from(makepad_pdf_parse::generate_test_pdf(1));
    let mut document = ingest_pdf(original.clone(), &PdfIngestOptions::default()).unwrap();
    let duration = DurationValue::new(1, 4, 0);
    let plan = SplicePlan {
        page: PageIndex(0),
        note: SemanticId(1),
        scope: ReflowScope::Glyph,
        patch_bounds: Rect::new(90.0, 90.0, 110.0, 110.0),
        erase: ErasePlan::ClippedOriginalForm {
            replacement: vec![PaintCommand::Dot {
                center: Point::new(100.0, 100.0),
                diameter: 2.0,
            }],
        },
        style: StyleProfile::default(),
        font: FontUseDecision::FallbackOfl {
            family: "test geometry".to_string(),
            license: "OFL-1.1".to_string(),
            visual_delta: 0.0,
        },
        affected_notes: vec![SemanticId(1)],
        affected_primitives: Vec::new(),
        onset_shifts: Vec::new(),
        before_pitch: None,
        after_pitch: None,
        before_duration: duration,
        after_duration: duration,
        warnings: vec![SpliceWarning::FontSubstituted],
        requires_explicit_approval: false,
    };
    apply_plan(&mut document, plan).unwrap();
    let exported = export_pdf(&document, &PdfExportOptions::default()).unwrap();
    assert_eq!(&exported.bytes[..original.len()], original.as_ref());
    assert!(exported.validation.original_prefix_identical);
    assert!(exported.validation.incremental_revision);
    assert!(exported.validation.reparsed);
    assert_eq!(exported.validation.page_count, 1);
    assert_eq!(revert_pdf(&document).as_ref(), original.as_ref());
}
