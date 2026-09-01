use super::*;
use makepad_asset_client::{
    ChatCreateRequest, ChatEventBodyDto, ChatEventDto, ChatEventsPageDto, ChatProviderDto,
    ChatProviderKind, ChatProviderLocality, ChatProviderStateDto, ChatSendRequest, ChatSessionId,
};
use makepad_score::model::{
    Alter, Change, Duration, EventKind, GlobalMaps, Id, KeySignature, MapScope, Measure, Meter,
    Note, Notehead, Part, Pitch, Score, ScoreTime, Spanner, SpannerEndpoint, SpannerKind, Staff,
    StaffKind, Step, TimedEvent, Transposition, Voice,
};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

fn pitch(step: Step, alter: i64, octave: i8) -> Pitch {
    Pitch::new(step, Alter::new(alter, 1).unwrap(), octave)
}

fn piano_specification(bar_count: u32) -> ScoreSpecification {
    ScoreSpecification {
        meter: MeterSpec::simple(4, 4),
        key: KeySpec {
            fifths: 0,
            mode: "major".to_string(),
        },
        bar_count,
        instruments: vec![InstrumentSpec {
            part_name: "Piano".to_string(),
            instrument_name: "acoustic piano".to_string(),
            written_low: pitch(Step::A, 0, 0),
            written_high: pitch(Step::C, 0, 8),
            keyboard: true,
            max_hand_span_semitones: 12,
        }],
    }
}

fn event(id: u64, note_id: u64, onset: (i64, u64), duration: (i64, u64)) -> TimedEvent {
    TimedEvent {
        id: Id::new(1, id),
        onset: ScoreTime::new(onset.0, onset.1).unwrap(),
        duration: Some(Duration::new(duration.0, duration.1).unwrap()),
        grace: None,
        kind: EventKind::Chord(vec![Note {
            id: Id::new(1, note_id),
            performance: None,
            written_pitch: Some(pitch(Step::C, 0, 4)),
            unpitched_sound: None,
            display_staff: Id::new(1, 2),
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

fn valid_score() -> Score {
    let part_id = Id::new(1, 1);
    let staff_id = Id::new(1, 2);
    let voice_id = Id::new(1, 3);
    let measure_id = Id::new(1, 4);
    let mut score = Score::new([7; 16]);
    score.title = "Test Piece".to_string();
    score.parts.insert(
        part_id,
        Part {
            id: part_id,
            name: "Piano".to_string(),
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
    score.voices.insert(
        voice_id,
        Voice {
            id: voice_id,
            staff: staff_id,
            number: 1,
            events: vec![event(5, 6, (0, 1), (1, 1))],
        },
    );
    score.measures.insert(
        measure_id,
        Measure {
            id: measure_id,
            ordinal: 1,
            label: "1".to_string(),
            start: ScoreTime::ZERO,
            extent: Duration::new(1, 1).unwrap(),
        },
    );
    score.maps = GlobalMaps {
        pedal: Vec::new(),
        tempo: Vec::new(),
        time_signature: vec![Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: Meter::Measured {
                groups: vec![4],
                unit: 4,
            },
        }],
        key: vec![Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: KeySignature::C_MAJOR,
        }],
    };
    score
}

fn two_bar_score() -> Score {
    let mut score = valid_score();
    let voice_id = Id::new(1, 3);
    score
        .voices
        .get_mut(&voice_id)
        .unwrap()
        .events
        .push(event(8, 9, (1, 1), (1, 1)));
    let measure_id = Id::new(1, 7);
    score.measures.insert(
        measure_id,
        Measure {
            id: measure_id,
            ordinal: 2,
            label: "2".to_string(),
            start: ScoreTime::new(1, 1).unwrap(),
            extent: Duration::new(1, 1).unwrap(),
        },
    );
    score
}

fn xml(id: &str) -> String {
    format!(
        "<score-partwise version=\"4.0\" id=\"{id}\"><part-list><score-part id=\"P1\"><part-name>Piano</part-name></score-part></part-list><part id=\"P1\"><measure number=\"1\"/></part></score-partwise>"
    )
}

#[test]
fn extracts_fenced_and_prose_wrapped_musicxml() {
    let document = xml("one");
    let reply = format!("Here is the score.\n```musicxml\n{document}\n```\nHope it helps.");
    let candidates = extract_musicxml_candidates(&reply);
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].complete);
    assert!(matches!(
        candidates[0].source,
        CandidateSource::Fenced { .. }
    ));
    assert_eq!(candidates[0].parse().unwrap().root().attr("id"), Some("one"));

    let prose = format!("prefix words {document} suffix words");
    let candidates = extract_musicxml_candidates(&prose);
    assert_eq!(candidates.len(), 1);
    assert!(matches!(
        candidates[0].source,
        CandidateSource::ConversationalText
    ));
}

#[test]
fn extraction_retains_truncation_and_multiple_candidates() {
    let truncated = "```xml\n<score-partwise version=\"4.0\"><part-list/>";
    let candidates = extract_musicxml_candidates(truncated);
    assert_eq!(candidates.len(), 1);
    assert!(!candidates[0].complete);
    assert!(candidates[0].parse().unwrap_err().contains("truncated"));

    let reply = format!("draft {} corrected {}", xml("draft"), xml("corrected"));
    let candidates = extract_musicxml_candidates(&reply);
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].parse().unwrap().root().attr("id"), Some("draft"));
    assert_eq!(
        candidates[1].parse().unwrap().root().attr("id"),
        Some("corrected")
    );
}

#[test]
fn valid_score_passes_every_musical_check() {
    assert_eq!(validate_score(&valid_score(), &piano_specification(1)), Vec::new());
}

#[test]
fn bar_meter_extent_and_count_failures_are_specific() {
    let mut score = valid_score();
    score
        .voices
        .get_mut(&Id::new(1, 3))
        .unwrap()
        .events[0]
        .duration = Some(Duration::new(5, 8).unwrap());
    score.measures.get_mut(&Id::new(1, 4)).unwrap().extent = Duration::new(3, 4).unwrap();
    score.maps.time_signature[0].value = Meter::Measured {
        groups: vec![3],
        unit: 4,
    };
    let problems = validate_score(&score, &piano_specification(2));
    assert!(problems.iter().any(|problem| matches!(
        problem,
        MusicalProblem::UnexpectedBarCount { expected: 2, actual: 1 }
    )));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::DeclaredMeterMismatch { .. })));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::MeasureExtentMismatch { .. })));
    let duration = problems
        .iter()
        .find(|problem| matches!(problem, MusicalProblem::BarDurationMismatch { .. }))
        .unwrap()
        .to_string();
    assert!(duration.contains("bar 1"));
    assert!(duration.contains("voice 1 sums to 5/8"));
}

#[test]
fn instrumentation_ranges_and_accidentals_are_checked() {
    let mut score = valid_score();
    let EventKind::Chord(notes) = &mut score
        .voices
        .get_mut(&Id::new(1, 3))
        .unwrap()
        .events[0]
        .kind
    else {
        unreachable!()
    };
    notes[0].written_pitch = Some(pitch(Step::C, 3, 9));
    let problems = validate_score(&score, &piano_specification(1));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::PitchOutOfRange { .. })));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::ImplausibleAccidental { .. })));

    let mut wrong_instrument = piano_specification(1);
    wrong_instrument.instruments[0].part_name = "Violin".to_string();
    let problems = validate_score(&valid_score(), &wrong_instrument);
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::MissingInstrumentPart { .. })));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::UnexpectedInstrumentPart { .. })));
}

#[test]
fn voice_collisions_duplicate_numbers_and_crossing_barlines_are_checked() {
    let mut score = valid_score();
    let voice_id = Id::new(1, 3);
    let voice = score.voices.get_mut(&voice_id).unwrap();
    voice.events[0].duration = Some(Duration::new(2, 1).unwrap());
    voice.events.push(event(10, 11, (1, 2), (1, 2)));

    let second_voice_id = Id::new(1, 12);
    score.voices.insert(
        second_voice_id,
        Voice {
            id: second_voice_id,
            staff: Id::new(1, 2),
            number: 1,
            events: vec![event(13, 14, (0, 1), (1, 1))],
        },
    );
    score
        .staves
        .get_mut(&Id::new(1, 2))
        .unwrap()
        .voices
        .push(second_voice_id);
    let problems = validate_score(&score, &piano_specification(1));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::VoiceCollision { .. })));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::EventOutsideBar { .. })));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::DuplicateVoiceNumber { .. })));
}

#[test]
fn every_tie_rule_is_checked() {
    let first_note = Id::new(1, 6);
    let second_note = Id::new(1, 9);

    let mut missing = valid_score();
    let EventKind::Chord(notes) = &mut missing
        .voices
        .get_mut(&Id::new(1, 3))
        .unwrap()
        .events[0]
        .kind
    else {
        unreachable!()
    };
    notes[0].tie_to = Some(Id::new(1, 999));
    assert!(validate_score(&missing, &piano_specification(1))
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::TieTargetMissing { .. })));

    let mut nonreciprocal = two_bar_score();
    let EventKind::Chord(notes) = &mut nonreciprocal
        .voices
        .get_mut(&Id::new(1, 3))
        .unwrap()
        .events[0]
        .kind
    else {
        unreachable!()
    };
    notes[0].tie_to = Some(second_note);
    assert!(validate_score(&nonreciprocal, &piano_specification(2))
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::TieNotReciprocal { .. })));

    let mut mismatch = two_bar_score();
    let voice = mismatch.voices.get_mut(&Id::new(1, 3)).unwrap();
    let EventKind::Chord(first) = &mut voice.events[0].kind else {
        unreachable!()
    };
    first[0].tie_to = Some(second_note);
    let EventKind::Chord(second) = &mut voice.events[1].kind else {
        unreachable!()
    };
    second[0].tie_from = Some(first_note);
    second[0].written_pitch = Some(pitch(Step::D, 0, 4));
    assert!(validate_score(&mismatch, &piano_specification(2))
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::TiePitchMismatch { .. })));

    let mut not_following = valid_score();
    let EventKind::Chord(notes) = &mut not_following
        .voices
        .get_mut(&Id::new(1, 3))
        .unwrap()
        .events[0]
        .kind
    else {
        unreachable!()
    };
    notes[0].tie_to = Some(Id::new(1, 15));
    notes.push(Note {
        performance: None,
        id: Id::new(1, 15),
        written_pitch: Some(pitch(Step::C, 0, 4)),
        unpitched_sound: None,
        display_staff: Id::new(1, 2),
        tie_from: Some(first_note),
        tie_to: None,
        tab: None,
        notehead: Notehead::Normal,
    });
    assert!(validate_score(&not_following, &piano_specification(1))
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::TieTargetNotFollowing { .. })));

    let mut valid = two_bar_score();
    let voice = valid.voices.get_mut(&Id::new(1, 3)).unwrap();
    let EventKind::Chord(first) = &mut voice.events[0].kind else {
        unreachable!()
    };
    first[0].tie_to = Some(second_note);
    let EventKind::Chord(second) = &mut voice.events[1].kind else {
        unreachable!()
    };
    second[0].tie_from = Some(first_note);
    assert_eq!(validate_score(&valid, &piano_specification(2)), Vec::new());
}

#[test]
fn slur_endpoints_must_exist_and_be_ordered() {
    let mut missing = valid_score();
    let spanner_id = Id::new(1, 20);
    missing.spanners.insert(
        spanner_id,
        Spanner {
            id: spanner_id,
            kind: SpannerKind::Slur { placement: None },
            start: SpannerEndpoint::Note(Id::new(1, 6)),
            end: SpannerEndpoint::Note(Id::new(1, 999)),
        },
    );
    assert!(validate_score(&missing, &piano_specification(1))
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::SlurEndpointMissing { .. })));

    let mut reversed = two_bar_score();
    reversed.spanners.insert(
        spanner_id,
        Spanner {
            id: spanner_id,
            kind: SpannerKind::Slur { placement: None },
            start: SpannerEndpoint::Note(Id::new(1, 9)),
            end: SpannerEndpoint::Note(Id::new(1, 6)),
        },
    );
    assert!(validate_score(&reversed, &piano_specification(2))
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::SlurEndpointOrder { .. })));

    reversed.spanners.get_mut(&spanner_id).unwrap().start =
        SpannerEndpoint::Note(Id::new(1, 6));
    reversed.spanners.get_mut(&spanner_id).unwrap().end =
        SpannerEndpoint::Note(Id::new(1, 9));
    assert_eq!(validate_score(&reversed, &piano_specification(2)), Vec::new());
}

#[test]
fn key_signature_and_keyboard_span_are_checked() {
    let mut score = valid_score();
    score.maps.key[0].value = KeySignature {
        fifths: 9,
        custom: vec![
            (Step::C, Alter::new(0, 1).unwrap()),
            (Step::C, Alter::new(3, 1).unwrap()),
        ],
    };
    let EventKind::Chord(notes) = &mut score
        .voices
        .get_mut(&Id::new(1, 3))
        .unwrap()
        .events[0]
        .kind
    else {
        unreachable!()
    };
    notes.push(Note {
        performance: None,
        id: Id::new(1, 30),
        written_pitch: Some(pitch(Step::G, 0, 5)),
        unpitched_sound: None,
        display_staff: Id::new(1, 2),
        tie_from: None,
        tie_to: None,
        tab: None,
        notehead: Notehead::Normal,
    });
    let problems = validate_score(&score, &piano_specification(1));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::DeclaredKeyMismatch { .. })));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::InvalidKeySignature { .. })));
    assert!(problems
        .iter()
        .any(|problem| matches!(problem, MusicalProblem::KeyboardHandSpan { .. })));
}

#[test]
fn selection_context_is_compact_and_all_operations_build_prompts() {
    let context = serialize_selection(
        &valid_score(),
        &ScoreSelection {
            parts: Vec::new(),
            first_bar: 1,
            last_bar: 1,
        },
    )
    .unwrap();
    assert!(context.0.contains("m1 meter=4/4 key=0f"));
    assert!(context.0.contains("p=Piano"));
    assert!(context.0.contains("@0/1+1/1:C4"));

    let operations = vec![
        ScoreOperation::Generate {
            description: "a prelude".to_string(),
        },
        ScoreOperation::Continue {
            description: "continue".to_string(),
            context: context.clone(),
        },
        ScoreOperation::HarmoniseMelody {
            description: "harmonise".to_string(),
            melody: context.clone(),
        },
        ScoreOperation::GenerateSecondPart {
            description: "counterpoint".to_string(),
            existing_part: context.clone(),
        },
        ScoreOperation::RevoiceSelection {
            description: "close voicing".to_string(),
            selection: context,
        },
    ];
    for operation in operations {
        let prompt = build_initial_prompt(&GenerationRequest {
            operation,
            specification: piano_specification(1),
        });
        assert!(prompt.system.contains("exactly 1 measures in 4/4"));
        assert!(prompt.system.contains("Every non-grace voice"));
        assert!(prompt.user.contains("TASK:"));
    }
}

struct FakeImporter {
    scores: BTreeMap<String, Score>,
}

impl ScoreImporter for FakeImporter {
    fn import(&self, document: &makepad_musicxml::MusicXmlDocument) -> Result<Score, ImportError> {
        let id = document
            .root()
            .attr("id")
            .ok_or_else(|| ImportError::new("root id missing"))?;
        self.scores
            .get(id)
            .cloned()
            .ok_or_else(|| ImportError::new(format!("no fixture for {id}")))
    }
}

struct FakeBroker {
    provider_rows: Vec<ChatProviderDto>,
    responses: Mutex<VecDeque<String>>,
    sent: Mutex<Vec<ChatSendRequest>>,
    creates: AtomicUsize,
    cancels: AtomicUsize,
    retires: AtomicUsize,
    sequence: AtomicUsize,
    cancel_during_events: AtomicBool,
    cancellation: Mutex<Option<CancellationToken>>,
}

impl FakeBroker {
    fn new(provider_rows: Vec<ChatProviderDto>, responses: Vec<String>) -> Self {
        Self {
            provider_rows,
            responses: Mutex::new(responses.into()),
            sent: Mutex::new(Vec::new()),
            creates: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            retires: AtomicUsize::new(0),
            sequence: AtomicUsize::new(0),
            cancel_during_events: AtomicBool::new(false),
            cancellation: Mutex::new(None),
        }
    }
}

impl ScoreChatBroker for FakeBroker {
    fn providers(&self) -> Result<Vec<ChatProviderDto>, BrokerError> {
        Ok(self.provider_rows.clone())
    }

    fn create(&self, _request: &ChatCreateRequest) -> Result<ChatSessionId, BrokerError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        Ok(ChatSessionId::parse("chat_0102030405060708").unwrap())
    }

    fn send(
        &self,
        _session: &ChatSessionId,
        request: &ChatSendRequest,
    ) -> Result<u64, BrokerError> {
        self.sent.lock().unwrap().push(request.clone());
        Ok(self.sent.lock().unwrap().len() as u64)
    }

    fn events(
        &self,
        _session: &ChatSessionId,
        _after: u64,
        _wait_ms: u64,
        _limit: u32,
    ) -> Result<ChatEventsPageDto, BrokerError> {
        if self.cancel_during_events.swap(false, Ordering::SeqCst) {
            if let Some(token) = self.cancellation.lock().unwrap().as_ref() {
                token.cancel();
            }
            return Ok(ChatEventsPageDto {
                events: Vec::new(),
                cursor: self.sequence.load(Ordering::SeqCst) as u64,
            });
        }
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| BrokerError::new("no fake response"))?;
        let delta_seq = self.sequence.fetch_add(2, Ordering::SeqCst) as u64 + 1;
        Ok(ChatEventsPageDto {
            events: vec![
                ChatEventDto {
                    seq: delta_seq,
                    body: ChatEventBodyDto::Delta {
                        text: response,
                        serving: None,
                    },
                },
                ChatEventDto {
                    seq: delta_seq + 1,
                    body: ChatEventBodyDto::Done,
                },
            ],
            cursor: delta_seq + 1,
        })
    }

    fn cancel(&self, _session: &ChatSessionId) -> Result<(), BrokerError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn retire(&self, _session: &ChatSessionId) -> Result<(), BrokerError> {
        self.retires.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn available_provider(kind: ChatProviderKind, locality: ChatProviderLocality) -> ChatProviderDto {
    ChatProviderDto {
        kind,
        locality,
        state: ChatProviderStateDto::Available {
            model: "test-model".to_string(),
        },
    }
}

fn generation_request() -> GenerationRequest {
    GenerationRequest {
        operation: ScoreOperation::Generate {
            description: "a small lyrical prelude".to_string(),
        },
        specification: piano_specification(1),
    }
}

#[test]
fn repair_loop_converges_reports_attempts_and_records_provenance() {
    let mut bad = valid_score();
    bad.voices
        .get_mut(&Id::new(1, 3))
        .unwrap()
        .events[0]
        .duration = Some(Duration::new(3, 4).unwrap());
    let importer = FakeImporter {
        scores: BTreeMap::from([
            ("bad".to_string(), bad),
            ("good".to_string(), valid_score()),
        ]),
    };
    let broker = FakeBroker::new(
        vec![available_provider(
            ChatProviderKind::ClaudeCli,
            ChatProviderLocality::Cloud,
        )],
        vec![
            format!("```musicxml\n{}\n```", xml("bad")),
            format!("fixed:\n```xml\n{}\n```", xml("good")),
        ],
    );
    let mut progress = Vec::new();
    let outcome = ScoreAiEngine::new(&broker, &importer)
        .generate(
            &generation_request(),
            ChatProviderKind::ClaudeCli,
            LocalityPolicy::AllowCloud,
            &CancellationToken::default(),
            |event| progress.push(event),
        )
        .unwrap();
    assert_eq!(outcome.attempts.len(), 2);
    assert!(!outcome.attempts[0].candidates[0].problems.is_empty());
    assert!(outcome.attempts[1].candidates[0].problems.is_empty());
    assert!(outcome.remaining_problems.is_empty());
    assert_eq!(outcome.provenance.provider, ChatProviderKind::ClaudeCli);
    assert_eq!(outcome.provenance.attempt, 2);
    assert!(outcome.score.annotations.contains_key(&outcome.provenance_annotation));
    let annotation = outcome
        .score
        .annotations
        .get(&outcome.provenance_annotation)
        .unwrap();
    let makepad_score::model::AnnotationBody::Text(body) = &annotation.body else {
        panic!("provenance should be ordinary text annotation")
    };
    assert!(body.contains("provider=claude-cli"));
    assert!(body.contains("attempt=2"));
    assert!(body.contains("SYSTEM"));
    assert!(progress.iter().any(|event| matches!(
        event,
        ProgressEvent::ModelDelta { attempt: 1, .. }
    )));
    assert_eq!(broker.sent.lock().unwrap().len(), 2);
    assert_eq!(broker.retires.load(Ordering::SeqCst), 1);
}

#[test]
fn repair_loop_is_bounded_and_stops_without_improvement() {
    let mut bad = valid_score();
    bad.voices
        .get_mut(&Id::new(1, 3))
        .unwrap()
        .events[0]
        .duration = Some(Duration::new(1, 2).unwrap());
    let importer = FakeImporter {
        scores: BTreeMap::from([("bad".to_string(), bad)]),
    };
    let broker = FakeBroker::new(
        vec![available_provider(
            ChatProviderKind::ClaudeCli,
            ChatProviderLocality::Cloud,
        )],
        vec![xml("bad"), xml("bad"), xml("bad")],
    );
    let mut progress = Vec::new();
    let outcome = ScoreAiEngine::new(&broker, &importer)
        .with_config(EngineConfig {
            max_attempts: 3,
            ..EngineConfig::default()
        })
        .generate(
            &generation_request(),
            ChatProviderKind::ClaudeCli,
            LocalityPolicy::AllowCloud,
            &CancellationToken::default(),
            |event| progress.push(event),
        )
        .unwrap();
    assert_eq!(outcome.attempts.len(), 2);
    assert!(!outcome.remaining_problems.is_empty());
    assert_eq!(outcome.provenance.attempt, 1);
    assert!(progress.iter().any(|event| matches!(
        event,
        ProgressEvent::RepairStoppedNoImprovement { attempt: 2 }
    )));
    assert_eq!(broker.sent.lock().unwrap().len(), 2);

    let broker = FakeBroker::new(
        vec![available_provider(
            ChatProviderKind::ClaudeCli,
            ChatProviderLocality::Cloud,
        )],
        vec![xml("bad"), xml("bad")],
    );
    let outcome = ScoreAiEngine::new(&broker, &importer)
        .with_config(EngineConfig {
            max_attempts: 1,
            ..EngineConfig::default()
        })
        .generate(
            &generation_request(),
            ChatProviderKind::ClaudeCli,
            LocalityPolicy::AllowCloud,
            &CancellationToken::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(outcome.attempts.len(), 1);
}

#[test]
fn worker_enforces_locality_and_surfaces_unavailable_provider() {
    let importer = FakeImporter {
        scores: BTreeMap::new(),
    };
    let broker = FakeBroker::new(
        vec![available_provider(
            ChatProviderKind::ClaudeCli,
            ChatProviderLocality::Cloud,
        )],
        Vec::new(),
    );
    let failure = ScoreAiEngine::new(&broker, &importer)
        .generate(
            &generation_request(),
            ChatProviderKind::ClaudeCli,
            LocalityPolicy::LocalOnly,
            &CancellationToken::default(),
            |_| {},
        )
        .unwrap_err();
    assert!(matches!(
        failure.error,
        GenerationError::LocalityRefused {
            provider: ChatProviderKind::ClaudeCli
        }
    ));
    assert_eq!(broker.creates.load(Ordering::SeqCst), 0);

    let unavailable = FakeBroker::new(
        vec![ChatProviderDto {
            kind: ChatProviderKind::FleetQwen,
            locality: ChatProviderLocality::Local,
            state: ChatProviderStateDto::Unavailable {
                reason: "fleet offline".to_string(),
            },
        }],
        Vec::new(),
    );
    let engine = ScoreAiEngine::new(&unavailable, &importer);
    assert!(matches!(
        engine.providers().unwrap()[0].state,
        ChatProviderStateDto::Unavailable { .. }
    ));
    let failure = engine
        .generate(
            &generation_request(),
            ChatProviderKind::FleetQwen,
            LocalityPolicy::LocalOnly,
            &CancellationToken::default(),
            |_| {},
        )
        .unwrap_err();
    assert!(matches!(
        failure.error,
        GenerationError::ProviderUnavailable { .. }
    ));
    assert_eq!(unavailable.creates.load(Ordering::SeqCst), 0);
}

#[test]
fn cancellation_reaches_broker_and_retires_session() {
    let importer = FakeImporter {
        scores: BTreeMap::new(),
    };
    let broker = FakeBroker::new(
        vec![available_provider(
            ChatProviderKind::FleetQwen,
            ChatProviderLocality::Local,
        )],
        Vec::new(),
    );
    let token = CancellationToken::default();
    *broker.cancellation.lock().unwrap() = Some(token.clone());
    broker.cancel_during_events.store(true, Ordering::SeqCst);
    let failure = ScoreAiEngine::new(&broker, &importer)
        .generate(
            &generation_request(),
            ChatProviderKind::FleetQwen,
            LocalityPolicy::LocalOnly,
            &token,
            |_| {},
        )
        .unwrap_err();
    assert_eq!(failure.error, GenerationError::Cancelled);
    assert_eq!(failure.attempts.len(), 1);
    assert!(failure.attempts[0]
        .stream_error
        .as_deref()
        .is_some_and(|error| error.contains("cancelled")));
    assert_eq!(broker.creates.load(Ordering::SeqCst), 1);
    assert_eq!(broker.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(broker.retires.load(Ordering::SeqCst), 1);
}
