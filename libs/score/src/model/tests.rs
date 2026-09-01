use super::*;
use crate::symbol::{Articulation, Placement};

fn id<K>(counter: u64) -> Id<K> {
    Id::new(0xabc, counter)
}

fn whole(value: i64) -> ScoreTime {
    ScoreTime::new(value, 1).unwrap()
}

fn duration(num: i64, den: u64) -> Duration {
    Duration::new(num, den).unwrap()
}

fn pitch(step: Step, alter: i64, octave: i8) -> Pitch {
    Pitch::new(step, Alter::new(alter, 1).unwrap(), octave)
}

fn note_event(
    event_counter: u64,
    note_counter: u64,
    staff: StaffId,
    onset: ScoreTime,
    value: Duration,
    written_pitch: Pitch,
) -> TimedEvent {
    TimedEvent {
        id: id::<EventTag>(event_counter),
        onset,
        duration: Some(value),
        grace: None,
        kind: EventKind::Chord(vec![Note {
            performance: None,
            id: id::<NoteTag>(note_counter),
            written_pitch: Some(written_pitch),
            unpitched_sound: None,
            display_staff: staff,
            tie_from: None,
            tie_to: None,
            tab: None,
            notehead: Notehead::Normal,
        }]),
        beams: Vec::new(),
        tuplets: Vec::new(),
        articulations: Vec::new(),
        ornaments: Vec::new(),
    }
}

fn fixture_score(measure_count: usize) -> Score {
    let part_id = id::<PartTag>(1);
    let staff_id = id::<StaffTag>(2);
    let voice_id = id::<VoiceTag>(3);
    let mut score = Score::new([7; 16]);
    score.title = "Deterministic fixture".to_string();
    score.parts.insert(
        part_id,
        Part {
            id: part_id,
            name: "Clarinet".to_string(),
            staves: vec![staff_id],
            transposition: Transposition::NONE,
        },
    );
    score.staves.insert(
        staff_id,
        Staff {
            id: staff_id,
            part: part_id,
            parent: None,
            kind: StaffKind::Standard,
            voices: vec![voice_id],
        },
    );
    let mut events = Vec::new();
    for index in 0..measure_count {
        let measure_id = id::<MeasureTag>(100 + index as u64);
        score.measures.insert(
            measure_id,
            Measure {
                id: measure_id,
                ordinal: index as u32,
                label: (index + 1).to_string(),
                start: whole(index as i64),
                extent: duration(1, 1),
            },
        );
        score.flow.nodes.push(FlowNode {
            measure: measure_id,
            ordinal: index as u32,
        });
        events.push(note_event(
            200 + index as u64,
            300 + index as u64,
            staff_id,
            whole(index as i64),
            duration(1, 1),
            pitch(Step::C, 0, 4),
        ));
    }
    score.voices.insert(
        voice_id,
        Voice {
            id: voice_id,
            staff: staff_id,
            number: 1,
            events,
        },
    );
    score.maps.time_signature.push(Change {
        at: ScoreTime::ZERO,
        scope: MapScope::Global,
        value: Meter::Measured {
            groups: vec![4],
            unit: 4,
        },
    });
    score
}

#[test]
fn exact_rational_and_nested_tuplet_arithmetic_is_checked() {
    assert_eq!(Rational::new(6, 8).unwrap(), Rational::new(3, 4).unwrap());
    assert_eq!(
        Rational::new(1, 6)
            .unwrap()
            .checked_add(Rational::new(1, 4).unwrap())
            .unwrap(),
        Rational::new(5, 12).unwrap()
    );
    assert!(Rational::new(2, 3).unwrap() > Rational::new(3, 5).unwrap());

    let triplet_eighth = Rational::QUARTER
        .checked_mul(Rational::new(2, 3).unwrap())
        .unwrap();
    assert_eq!(triplet_eighth, Rational::new(1, 6).unwrap());
    let nested_four_in_five = triplet_eighth
        .checked_mul(Rational::new(4, 5).unwrap())
        .unwrap();
    assert_eq!(nested_four_in_five, Rational::new(2, 15).unwrap());
    assert_eq!(
        nested_four_in_five
            .checked_mul(Rational::new(15, 2).unwrap())
            .unwrap(),
        Rational::ONE
    );
    assert_eq!(
        Rational::new(i64::MAX, 1)
            .unwrap()
            .checked_mul(Rational::new(2, 1).unwrap()),
        Err(RationalError::Overflow)
    );
    assert_eq!(Duration::new(0, 1), Err(RationalError::NonPositiveDuration));
}

#[test]
fn ids_survive_edits_and_native_save_load() {
    let score = fixture_score(2);
    let note_id = id::<NoteTag>(300);
    let original_event_id = id::<EventTag>(200);
    let score_bytes = score.to_bytes();
    let loaded = Score::from_bytes(&score_bytes).unwrap();
    assert_eq!(loaded, score);
    assert_eq!(loaded.note(note_id).unwrap().id, note_id);
    assert_eq!(loaded.event(original_event_id).unwrap().id, original_event_id);
    assert_eq!(loaded.to_bytes(), score_bytes);

    let mut workspace = ScoreWorkspace::new(loaded, 44, 2).unwrap();
    workspace
        .transact(vec![EditCommand::ChangePitch {
            note: note_id,
            pitch: pitch(Step::D, 0, 4),
        }])
        .unwrap();
    assert_eq!(workspace.score().note(note_id).unwrap().id, note_id);
    assert_eq!(workspace.score().event(original_event_id).unwrap().id, original_event_id);

    let archive = workspace.to_bytes();
    let restored = ScoreWorkspace::from_bytes(&archive).unwrap();
    assert_eq!(restored, workspace);
    assert_eq!(restored.to_bytes(), archive);
}

#[test]
fn transposition_and_part_views_are_non_mutating_projections() {
    let mut score = fixture_score(1);
    let part_id = id::<PartTag>(1);
    let note_id = id::<NoteTag>(300);
    score.parts.get_mut(&part_id).unwrap().transposition = Transposition {
        diatonic_steps: -1,
        chromatic_semitones: Alter::new(-2, 1).unwrap(),
        octave_shift: 0,
    };
    let written = score
        .pitch_projection(part_id, note_id, false)
        .unwrap()
        .unwrap();
    let concert = score
        .pitch_projection(part_id, note_id, true)
        .unwrap()
        .unwrap();
    assert_eq!(written.displayed, pitch(Step::C, 0, 4));
    assert_eq!(concert.displayed, pitch(Step::B, -1, 3));
    assert_eq!(score.note(note_id).unwrap().written_pitch, Some(written.written));

    let view = PartView {
        id: id::<PartViewTag>(500),
        name: "Clarinet part".to_string(),
        included_parts: vec![part_id],
        layout_overrides: LayoutOverrides::default(),
    };
    {
        let projection = score.project(&view);
        assert!(std::ptr::eq(projection.score(), &score));
        assert_eq!(projection.parts().count(), 1);
        assert_eq!(projection.staves().count(), 1);
        assert_eq!(projection.voices().next().unwrap().events[0].id, id::<EventTag>(200));
    }
    score.note_mut(note_id).unwrap().written_pitch = Some(pitch(Step::E, 0, 4));
    assert_eq!(
        score
            .project(&view)
            .voices()
            .next()
            .unwrap()
            .events[0]
            .chord_notes()[0]
            .written_pitch,
        Some(pitch(Step::E, 0, 4))
    );
}

fn visited_ordinals(score: &Score, visits: &[PlaybackVisit]) -> Vec<u32> {
    visits
        .iter()
        .map(|visit| score.measures[&visit.source_measure].ordinal)
        .collect()
}

#[test]
fn repeat_unfolding_handles_nesting_and_voltas_without_flattening() {
    let mut score = fixture_score(5);
    let original = score.flow.clone();
    score.flow.repeats = vec![
        RepeatSection {
            start: 1,
            end: 2,
            times: 2,
        },
        RepeatSection {
            start: 0,
            end: 3,
            times: 2,
        },
    ];
    let visits = score.flow.unfold(&score, 100).unwrap();
    assert_eq!(
        visited_ordinals(&score, &visits),
        vec![0, 1, 2, 1, 2, 3, 0, 1, 2, 1, 2, 3, 4]
    );
    assert_ne!(score.flow, original);
    assert_eq!(score.measures.len(), 5);

    score.flow.repeats = vec![RepeatSection {
        start: 0,
        end: 3,
        times: 2,
    }];
    score.flow.voltas = vec![
        VoltaEnding {
            start: 2,
            end: 2,
            repeat_start: 0,
            passes: vec![1],
        },
        VoltaEnding {
            start: 3,
            end: 3,
            repeat_start: 0,
            passes: vec![2],
        },
    ];
    let visits = score.flow.unfold(&score, 100).unwrap();
    assert_eq!(visited_ordinals(&score, &visits), vec![0, 1, 2, 0, 1, 3, 4]);
    assert_eq!(
        score.flow.unfold(&score, 3),
        Err(PlaybackPlanError::VisitLimit { limit: 3 })
    );
}

#[test]
fn repeat_unfolding_handles_dal_segno_to_coda_and_fine() {
    let mut score = fixture_score(7);
    score.flow.markers = vec![
        FlowMarker {
            at: 1,
            kind: MarkerKind::Segno,
        },
        FlowMarker {
            at: 5,
            kind: MarkerKind::Coda,
        },
    ];
    score.flow.jumps = vec![
        JumpInstruction {
            at: 3,
            kind: JumpKind::ToCoda,
        },
        JumpInstruction {
            at: 4,
            kind: JumpKind::DalSegno,
        },
        JumpInstruction {
            at: 6,
            kind: JumpKind::Fine,
        },
    ];
    let visits = score.flow.unfold(&score, 100).unwrap();
    assert_eq!(
        visited_ordinals(&score, &visits),
        vec![0, 1, 2, 3, 4, 1, 2, 3, 5, 6]
    );
    assert_eq!(visits[5].play_start, whole(5));
}

fn anchored_annotation(score: &Score, annotation_id: AnnotationId, note_id: NoteId) -> Annotation {
    let (voice_id, event, note) = score.note_context(note_id).unwrap();
    let staff = score.voices[&voice_id].staff;
    Annotation {
        id: annotation_id,
        layer: id::<LayerTag>(800),
        kind: AnnotationKind::Circle,
        anchor: SemanticAnchor {
            primary: AnchorTarget::Note(note.id),
            fallback: BeatRange {
                staff,
                voice: Some(voice_id),
                start: event.onset,
                end: event.end().unwrap(),
            },
            affinity: Affinity::On,
            context_fingerprint: score.note_fingerprint(note_id).unwrap(),
            ink: Some(InkAnchor::ElementLocal {
                target: ElementRef::Note(note_id),
                points: vec![LocalInkPoint {
                    u: Rational::new(1, 2).unwrap(),
                    v: Rational::new(1, 4).unwrap(),
                    pressure: 700,
                    tilt: 0,
                    azimuth: 0,
                    elapsed_micros: 10,
                }],
            }),
        },
        body: AnnotationBody::None,
        style: AnnotationStyle {
            color_rgba: [255, 0, 0, 255],
            width_milli_staff_space: 120,
        },
        action: None,
        author: [9; 16],
        created_lamport: 1,
        modified_lamport: 1,
    }
}

#[test]
fn anchors_resolve_exact_fallback_after_rebar_and_orphan_after_deletion() {
    let note_id = id::<NoteTag>(300);
    let annotation_id = id::<AnnotationTag>(700);
    let mut score = fixture_score(2);
    let annotation = anchored_annotation(&score, annotation_id, note_id);
    score.annotations.insert(annotation_id, annotation.clone());
    assert_eq!(
        score.resolve_anchor(&annotation.anchor),
        AnchorResolution::Exact(ResolvedTarget::Note(note_id))
    );

    let voice_id = id::<VoiceTag>(3);
    let staff_id = id::<StaffTag>(2);
    let replacement = note_event(
        900,
        901,
        staff_id,
        ScoreTime::ZERO,
        duration(1, 1),
        pitch(Step::C, 0, 4),
    );
    let mut workspace = ScoreWorkspace::new(score, 88, 3).unwrap();
    workspace
        .transact(vec![
            EditCommand::DeleteEvent {
                event: id::<EventTag>(200),
            },
            EditCommand::InsertEvent {
                voice: voice_id,
                event: replacement,
            },
        ])
        .unwrap();
    assert_eq!(
        workspace.score().resolve_anchor(&annotation.anchor),
        AnchorResolution::Fallback {
            target: ResolvedTarget::Note(id::<NoteTag>(901)),
            confidence_milli: 1000,
        }
    );

    let old_measure = id::<MeasureTag>(100);
    let rebared = Measure {
        id: id::<MeasureTag>(950),
        ordinal: 0,
        label: "1a".to_string(),
        start: ScoreTime::ZERO,
        extent: duration(1, 1),
    };
    workspace
        .transact(vec![EditCommand::Rebar {
            remove: vec![old_measure],
            replacements: vec![rebared],
        }])
        .unwrap();
    assert!(matches!(
        workspace.score().resolve_anchor(&annotation.anchor),
        AnchorResolution::Fallback {
            target: ResolvedTarget::Note(_),
            ..
        }
    ));

    workspace
        .transact(vec![EditCommand::DeleteEvent {
            event: id::<EventTag>(900),
        }])
        .unwrap();
    assert_eq!(
        workspace.score().resolve_anchor(&annotation.anchor),
        AnchorResolution::Orphaned {
            last_known: annotation.anchor.fallback,
        }
    );
}

#[test]
fn journaled_core_edits_are_atomic_and_compensating() {
    let score = fixture_score(2);
    let initial = score.clone();
    let mut workspace = ScoreWorkspace::new(score, 55, 2).unwrap();
    let event_id = id::<EventTag>(200);
    let articulation = PlacedArticulation {
        kind: Articulation::Staccato,
        placement: Some(Placement::Above),
    };
    workspace
        .transact(vec![
            EditCommand::ChangeDuration {
                event: event_id,
                duration: duration(1, 2),
            },
            EditCommand::SetArticulations {
                event: event_id,
                articulations: vec![articulation],
            },
            EditCommand::AddMeasures {
                measures: vec![Measure {
                    id: id::<MeasureTag>(999),
                    ordinal: 2,
                    label: "3".to_string(),
                    start: whole(2),
                    extent: duration(1, 1),
                }],
            },
        ])
        .unwrap();
    assert_eq!(workspace.score().event(event_id).unwrap().duration, Some(duration(1, 2)));
    assert_eq!(workspace.score().event(event_id).unwrap().articulations, [articulation]);
    assert!(workspace.score().measures.contains_key(&id::<MeasureTag>(999)));

    let before_rejected = workspace.score().clone();
    let error = workspace
        .transact(vec![EditCommand::MoveEvent {
            event: id::<EventTag>(201),
            onset: ScoreTime::new(1, 4).unwrap(),
        }])
        .unwrap_err();
    assert!(matches!(error, EditError::InvariantViolation(_)));
    assert_eq!(workspace.score(), &before_rejected);

    let undo_receipt = workspace.undo().unwrap();
    assert_eq!(workspace.score(), &initial);
    assert_eq!(workspace.journal().last().unwrap().undoes, Some(OpId { actor: 55, counter: 1 }));
    assert_eq!(undo_receipt.transaction, OpId { actor: 55, counter: 2 });
    workspace.redo().unwrap();
    assert_eq!(workspace.score(), &before_rejected);
    assert_eq!(workspace.snapshots().len(), 1);
}

#[test]
fn long_fixed_seed_edit_sequence_undoes_and_redoes_exactly() {
    let score = fixture_score(16);
    let initial = score.clone();
    let mut workspace = ScoreWorkspace::new(score, 0xfeed, 17).unwrap();
    let mut state = 0x1234_5678_9abc_def0_u64;
    const STEPS: [Step; 7] = [Step::C, Step::D, Step::E, Step::F, Step::G, Step::A, Step::B];

    for _ in 0..256 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let index = (state as usize) % 16;
        let step = STEPS[((state >> 12) as usize) % STEPS.len()];
        let alter = match (state >> 24) % 3 {
            0 => -1,
            1 => 0,
            _ => 1,
        };
        let octave = 3 + ((state >> 32) % 4) as i8;
        workspace
            .transact(vec![EditCommand::ChangePitch {
                note: id::<NoteTag>(300 + index as u64),
                pitch: pitch(step, alter, octave),
            }])
            .unwrap();
    }
    let edited = workspace.score().clone();
    for _ in 0..256 {
        workspace.undo().unwrap();
    }
    assert_eq!(workspace.score(), &initial);
    for _ in 0..256 {
        workspace.redo().unwrap();
    }
    assert_eq!(workspace.score(), &edited);
    assert_eq!(workspace.journal().len(), 768);
    assert!(!workspace.snapshots().is_empty());

    let restored = ScoreWorkspace::from_bytes(&workspace.to_bytes()).unwrap();
    assert_eq!(restored, workspace);
}

#[test]
fn validation_reports_malformed_meter_voice_tie_and_slur() {
    let mut score = fixture_score(1);
    let measure_id = id::<MeasureTag>(100);
    score.measures.get_mut(&measure_id).unwrap().extent = duration(3, 4);
    let voice_id = id::<VoiceTag>(3);
    let staff_id = id::<StaffTag>(2);
    score.voices.get_mut(&voice_id).unwrap().events.push(note_event(
        999,
        998,
        staff_id,
        ScoreTime::new(1, 2).unwrap(),
        duration(1, 2),
        pitch(Step::D, 0, 4),
    ));
    score.note_mut(id::<NoteTag>(300)).unwrap().tie_to = Some(id::<NoteTag>(12345));
    let spanner_id = id::<SpannerTag>(777);
    score.spanners.insert(
        spanner_id,
        Spanner {
            id: spanner_id,
            kind: SpannerKind::Slur { placement: None },
            start: SpannerEndpoint::Note(id::<NoteTag>(300)),
            end: SpannerEndpoint::Note(id::<NoteTag>(54321)),
        },
    );
    let problems = score.validate();
    assert!(problems.iter().any(|problem| matches!(
        problem,
        ValidationProblem::MeasureDurationMismatch { measure, .. } if *measure == measure_id
    )));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, ValidationProblem::VoiceOverlap { .. })));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, ValidationProblem::DanglingTie { .. })));
    assert!(problems.iter().any(|problem| matches!(
        problem,
        ValidationProblem::DanglingSpannerEndpoint { spanner } if *spanner == spanner_id
    )));
}
