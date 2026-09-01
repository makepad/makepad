use super::annotation::Annotation;
use super::graph::{Measure, PlacedArticulation, Score, Spanner, TimedEvent};
use super::id::{AnnotationId, EventId, MeasureId, NoteId, SpannerId, VoiceId};
use super::pitch::Pitch;
use super::playback::FlowGraph;
use super::time::{Duration, ScoreTime};
use super::validation::{validate_edit_invariants, ValidationProblem};
use makepad_micro_serde::{DeBin, DeBinErr, SerBin};
use std::fmt;

const SCORE_MAGIC: &[u8; 8] = b"MPSCORE\0";
const WORKSPACE_MAGIC: &[u8; 8] = b"MPWORKS\0";
const NATIVE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, SerBin, DeBin)]
pub struct OpId {
    pub actor: u64,
    pub counter: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct EditTxn {
    pub id: OpId,
    pub causal_parents: Vec<OpId>,
    pub undoes: Option<OpId>,
    pub ops: Vec<EditOp>,
}

/// Fully invertible primitive operations retained in the append-only journal.
#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum EditOp {
    InsertEvent {
        voice: VoiceId,
        event: TimedEvent,
    },
    DeleteEvent {
        voice: VoiceId,
        prior: TimedEvent,
    },
    SetPitch {
        note: NoteId,
        before: Pitch,
        after: Pitch,
    },
    SetDuration {
        event: EventId,
        before: Option<Duration>,
        after: Option<Duration>,
    },
    MoveEvent {
        event: EventId,
        before: ScoreTime,
        after: ScoreTime,
    },
    ReplaceMeasures {
        before: Vec<Measure>,
        after: Vec<Measure>,
    },
    SetArticulations {
        event: EventId,
        before: Vec<PlacedArticulation>,
        after: Vec<PlacedArticulation>,
    },
    PutSpanner {
        id: SpannerId,
        before: Option<Spanner>,
        after: Option<Spanner>,
    },
    PutAnnotation {
        id: AnnotationId,
        before: Option<Annotation>,
        after: Option<Annotation>,
    },
    SetFlow {
        before: FlowGraph,
        after: FlowGraph,
    },
}

impl EditOp {
    pub fn inverse(&self) -> Self {
        match self {
            Self::InsertEvent { voice, event } => Self::DeleteEvent {
                voice: *voice,
                prior: event.clone(),
            },
            Self::DeleteEvent { voice, prior } => Self::InsertEvent {
                voice: *voice,
                event: prior.clone(),
            },
            Self::SetPitch {
                note,
                before,
                after,
            } => Self::SetPitch {
                note: *note,
                before: *after,
                after: *before,
            },
            Self::SetDuration {
                event,
                before,
                after,
            } => Self::SetDuration {
                event: *event,
                before: *after,
                after: *before,
            },
            Self::MoveEvent {
                event,
                before,
                after,
            } => Self::MoveEvent {
                event: *event,
                before: *after,
                after: *before,
            },
            Self::ReplaceMeasures { before, after } => Self::ReplaceMeasures {
                before: after.clone(),
                after: before.clone(),
            },
            Self::SetArticulations {
                event,
                before,
                after,
            } => Self::SetArticulations {
                event: *event,
                before: after.clone(),
                after: before.clone(),
            },
            Self::PutSpanner { id, before, after } => Self::PutSpanner {
                id: *id,
                before: after.clone(),
                after: before.clone(),
            },
            Self::PutAnnotation { id, before, after } => Self::PutAnnotation {
                id: *id,
                before: after.clone(),
                after: before.clone(),
            },
            Self::SetFlow { before, after } => Self::SetFlow {
                before: after.clone(),
                after: before.clone(),
            },
        }
    }
}

/// Intent-level edit commands compiled atomically into journal operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditCommand {
    InsertEvent { voice: VoiceId, event: TimedEvent },
    DeleteEvent { event: EventId },
    ChangePitch { note: NoteId, pitch: Pitch },
    ChangeDuration { event: EventId, duration: Duration },
    MoveEvent { event: EventId, onset: ScoreTime },
    AddMeasures { measures: Vec<Measure> },
    RemoveMeasures { measures: Vec<MeasureId> },
    Rebar {
        remove: Vec<MeasureId>,
        replacements: Vec<Measure>,
    },
    SetArticulations {
        event: EventId,
        articulations: Vec<PlacedArticulation>,
    },
    PutSpanner(Option<Spanner>, SpannerId),
    PutAnnotation(Option<Annotation>, AnnotationId),
    SetFlow(FlowGraph),
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct JournalSnapshot {
    pub at: OpId,
    pub score: Score,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct ScoreWorkspace {
    score: Score,
    journal: Vec<EditTxn>,
    snapshots: Vec<JournalSnapshot>,
    actor: u64,
    next_op_counter: u64,
    snapshot_interval: usize,
    undo_stack: Vec<OpId>,
    redo_stack: Vec<OpId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditReceipt {
    pub transaction: OpId,
    pub revision: u64,
    pub problems: Vec<ValidationProblem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditError {
    EmptyTransaction,
    CounterExhausted,
    NotFound(&'static str),
    DuplicateId(&'static str),
    PreconditionFailed(&'static str),
    InvalidEntityId(&'static str),
    InvariantViolation(Vec<ValidationProblem>),
    NothingToUndo,
    NothingToRedo,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for EditError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeFormatError {
    BadMagic,
    UnsupportedVersion(u32),
    Truncated,
    TrailingBytes,
    Decode(String),
    InvalidModel(Vec<ValidationProblem>),
    InvalidSnapshotInterval,
}

impl fmt::Display for NativeFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for NativeFormatError {}

impl ScoreWorkspace {
    pub fn new(
        score: Score,
        actor: u64,
        snapshot_interval: usize,
    ) -> Result<Self, EditError> {
        let problems = validate_edit_invariants(&score);
        if !problems.is_empty() {
            return Err(EditError::InvariantViolation(problems));
        }
        Ok(Self {
            score,
            journal: Vec::new(),
            snapshots: Vec::new(),
            actor,
            next_op_counter: 1,
            snapshot_interval: snapshot_interval.max(1),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        })
    }

    pub const fn score(&self) -> &Score {
        &self.score
    }

    /// Whether [`Self::undo`] would do anything. A UI that offers Undo with
    /// an empty stack is offering an error message.
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Whether [`Self::redo`] would do anything.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn journal(&self) -> &[EditTxn] {
        &self.journal
    }

    pub fn snapshots(&self) -> &[JournalSnapshot] {
        &self.snapshots
    }

    pub fn transact(&mut self, commands: Vec<EditCommand>) -> Result<EditReceipt, EditError> {
        if commands.is_empty() {
            return Err(EditError::EmptyTransaction);
        }
        let mut working = self.score.clone();
        let mut ops = Vec::with_capacity(commands.len());
        for command in commands {
            let op = compile_command(&working, command)?;
            apply_op(&mut working, &op)?;
            ops.push(op);
        }
        let problems = validate_edit_invariants(&working);
        if !problems.is_empty() {
            return Err(EditError::InvariantViolation(problems));
        }
        let receipt = self.commit(working, ops, None)?;
        self.undo_stack.push(receipt.transaction);
        self.redo_stack.clear();
        Ok(receipt)
    }

    pub fn undo(&mut self) -> Result<EditReceipt, EditError> {
        let target = *self.undo_stack.last().ok_or(EditError::NothingToUndo)?;
        let txn = self
            .journal
            .iter()
            .find(|txn| txn.id == target)
            .ok_or(EditError::NotFound("transaction"))?
            .clone();
        let ops: Vec<_> = txn.ops.iter().rev().map(EditOp::inverse).collect();
        let working = apply_ops_atomically(&self.score, &ops)?;
        let receipt = self.commit(working, ops, Some(target))?;
        self.undo_stack.pop();
        self.redo_stack.push(receipt.transaction);
        Ok(receipt)
    }

    pub fn redo(&mut self) -> Result<EditReceipt, EditError> {
        let undo_id = *self.redo_stack.last().ok_or(EditError::NothingToRedo)?;
        let undo_txn = self
            .journal
            .iter()
            .find(|txn| txn.id == undo_id)
            .ok_or(EditError::NotFound("undo transaction"))?
            .clone();
        let original = undo_txn
            .undoes
            .ok_or(EditError::PreconditionFailed("redo target is not an undo"))?;
        let ops: Vec<_> = undo_txn.ops.iter().rev().map(EditOp::inverse).collect();
        let working = apply_ops_atomically(&self.score, &ops)?;
        let receipt = self.commit(working, ops, Some(undo_id))?;
        self.redo_stack.pop();
        self.undo_stack.push(original);
        Ok(receipt)
    }

    /// Selectively compensates a retained operation when its preconditions still hold.
    pub fn selective_undo(&mut self, target: OpId) -> Result<EditReceipt, EditError> {
        let txn = self
            .journal
            .iter()
            .find(|txn| txn.id == target)
            .ok_or(EditError::NotFound("transaction"))?
            .clone();
        let ops: Vec<_> = txn.ops.iter().rev().map(EditOp::inverse).collect();
        let working = apply_ops_atomically(&self.score, &ops)?;
        let receipt = self.commit(working, ops, Some(target))?;
        self.undo_stack.retain(|id| *id != target);
        Ok(receipt)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        encode_native(WORKSPACE_MAGIC, self)
    }

    pub fn from_bytes(input: &[u8]) -> Result<Self, NativeFormatError> {
        let workspace: Self = decode_native(WORKSPACE_MAGIC, input)?;
        if workspace.snapshot_interval == 0 {
            return Err(NativeFormatError::InvalidSnapshotInterval);
        }
        let problems = validate_edit_invariants(&workspace.score);
        if !problems.is_empty() {
            return Err(NativeFormatError::InvalidModel(problems));
        }
        Ok(workspace)
    }

    fn commit(
        &mut self,
        score: Score,
        ops: Vec<EditOp>,
        undoes: Option<OpId>,
    ) -> Result<EditReceipt, EditError> {
        let id = OpId {
            actor: self.actor,
            counter: self.next_op_counter,
        };
        self.next_op_counter = self
            .next_op_counter
            .checked_add(1)
            .ok_or(EditError::CounterExhausted)?;
        let causal_parents = self.journal.last().map(|txn| vec![txn.id]).unwrap_or_default();
        self.score = score;
        self.journal.push(EditTxn {
            id,
            causal_parents,
            undoes,
            ops,
        });
        if self.journal.len() % self.snapshot_interval == 0 {
            self.snapshots.push(JournalSnapshot {
                at: id,
                score: self.score.clone(),
            });
        }
        Ok(EditReceipt {
            transaction: id,
            revision: self.journal.len() as u64,
            problems: self.score.validate(),
        })
    }
}

impl Score {
    pub fn to_bytes(&self) -> Vec<u8> {
        encode_native(SCORE_MAGIC, self)
    }

    pub fn from_bytes(input: &[u8]) -> Result<Self, NativeFormatError> {
        let score: Self = decode_native(SCORE_MAGIC, input)?;
        let problems = validate_edit_invariants(&score);
        if problems.is_empty() {
            Ok(score)
        } else {
            Err(NativeFormatError::InvalidModel(problems))
        }
    }
}

fn compile_command(score: &Score, command: EditCommand) -> Result<EditOp, EditError> {
    match command {
        EditCommand::InsertEvent { voice, event } => Ok(EditOp::InsertEvent { voice, event }),
        EditCommand::DeleteEvent { event } => {
            let voice = score.event_owner(event).ok_or(EditError::NotFound("event"))?;
            let prior = score
                .event(event)
                .ok_or(EditError::NotFound("event"))?
                .clone();
            Ok(EditOp::DeleteEvent { voice, prior })
        }
        EditCommand::ChangePitch { note, pitch } => {
            let before = score
                .note(note)
                .ok_or(EditError::NotFound("note"))?
                .written_pitch
                .ok_or(EditError::PreconditionFailed("unpitched note"))?;
            Ok(EditOp::SetPitch {
                note,
                before,
                after: pitch,
            })
        }
        EditCommand::ChangeDuration { event, duration } => Ok(EditOp::SetDuration {
            event,
            before: score
                .event(event)
                .ok_or(EditError::NotFound("event"))?
                .duration,
            after: Some(duration),
        }),
        EditCommand::MoveEvent { event, onset } => Ok(EditOp::MoveEvent {
            event,
            before: score
                .event(event)
                .ok_or(EditError::NotFound("event"))?
                .onset,
            after: onset,
        }),
        EditCommand::AddMeasures { measures } => Ok(EditOp::ReplaceMeasures {
            before: Vec::new(),
            after: measures,
        }),
        EditCommand::RemoveMeasures { measures } => Ok(EditOp::ReplaceMeasures {
            before: collect_measures(score, &measures)?,
            after: Vec::new(),
        }),
        EditCommand::Rebar {
            remove,
            replacements,
        } => Ok(EditOp::ReplaceMeasures {
            before: collect_measures(score, &remove)?,
            after: replacements,
        }),
        EditCommand::SetArticulations {
            event,
            articulations,
        } => Ok(EditOp::SetArticulations {
            event,
            before: score
                .event(event)
                .ok_or(EditError::NotFound("event"))?
                .articulations
                .clone(),
            after: articulations,
        }),
        EditCommand::PutSpanner(after, id) => Ok(EditOp::PutSpanner {
            id,
            before: score.spanners.get(&id).cloned(),
            after,
        }),
        EditCommand::PutAnnotation(after, id) => Ok(EditOp::PutAnnotation {
            id,
            before: score.annotations.get(&id).cloned(),
            after,
        }),
        EditCommand::SetFlow(after) => Ok(EditOp::SetFlow {
            before: score.flow.clone(),
            after,
        }),
    }
}

fn collect_measures(score: &Score, ids: &[MeasureId]) -> Result<Vec<Measure>, EditError> {
    ids.iter()
        .map(|id| {
            score
                .measures
                .get(id)
                .cloned()
                .ok_or(EditError::NotFound("measure"))
        })
        .collect()
}

fn apply_ops_atomically(score: &Score, ops: &[EditOp]) -> Result<Score, EditError> {
    let mut working = score.clone();
    for op in ops {
        apply_op(&mut working, op)?;
    }
    let problems = validate_edit_invariants(&working);
    if problems.is_empty() {
        Ok(working)
    } else {
        Err(EditError::InvariantViolation(problems))
    }
}

fn apply_op(score: &mut Score, op: &EditOp) -> Result<(), EditError> {
    match op {
        EditOp::InsertEvent { voice, event } => {
            if score.event(event.id).is_some() {
                return Err(EditError::DuplicateId("event"));
            }
            let voice = score
                .voices
                .get_mut(voice)
                .ok_or(EditError::NotFound("voice"))?;
            voice.events.push(event.clone());
            Score::sort_voice(voice);
        }
        EditOp::DeleteEvent { voice, prior } => {
            let voice = score
                .voices
                .get_mut(voice)
                .ok_or(EditError::NotFound("voice"))?;
            let index = voice
                .events
                .iter()
                .position(|event| event.id == prior.id)
                .ok_or(EditError::NotFound("event"))?;
            if voice.events[index] != *prior {
                return Err(EditError::PreconditionFailed("event changed"));
            }
            voice.events.remove(index);
        }
        EditOp::SetPitch {
            note,
            before,
            after,
        } => {
            let note = score.note_mut(*note).ok_or(EditError::NotFound("note"))?;
            if note.written_pitch != Some(*before) {
                return Err(EditError::PreconditionFailed("pitch changed"));
            }
            note.written_pitch = Some(*after);
        }
        EditOp::SetDuration {
            event,
            before,
            after,
        } => {
            let owner = score.event_owner(*event).ok_or(EditError::NotFound("event"))?;
            let voice = score
                .voices
                .get_mut(&owner)
                .ok_or(EditError::NotFound("voice"))?;
            let event = voice
                .events
                .iter_mut()
                .find(|candidate| candidate.id == *event)
                .ok_or(EditError::NotFound("event"))?;
            if event.duration != *before {
                return Err(EditError::PreconditionFailed("duration changed"));
            }
            event.duration = *after;
            Score::sort_voice(voice);
        }
        EditOp::MoveEvent {
            event,
            before,
            after,
        } => {
            let owner = score.event_owner(*event).ok_or(EditError::NotFound("event"))?;
            let voice = score
                .voices
                .get_mut(&owner)
                .ok_or(EditError::NotFound("voice"))?;
            let event = voice
                .events
                .iter_mut()
                .find(|candidate| candidate.id == *event)
                .ok_or(EditError::NotFound("event"))?;
            if event.onset != *before {
                return Err(EditError::PreconditionFailed("onset changed"));
            }
            event.onset = *after;
            Score::sort_voice(voice);
        }
        EditOp::ReplaceMeasures { before, after } => {
            for measure in before {
                if score.measures.get(&measure.id) != Some(measure) {
                    return Err(EditError::PreconditionFailed("measure changed"));
                }
            }
            for measure in before {
                score.measures.remove(&measure.id);
            }
            for measure in after {
                if score.measures.contains_key(&measure.id) {
                    return Err(EditError::DuplicateId("measure"));
                }
                score.measures.insert(measure.id, measure.clone());
            }
            let has_navigation = !score.flow.repeats.is_empty()
                || !score.flow.voltas.is_empty()
                || !score.flow.markers.is_empty()
                || !score.flow.jumps.is_empty();
            if has_navigation {
                if before.len() == after.len()
                    && before
                        .iter()
                        .zip(after)
                        .all(|(old, new)| old.ordinal == new.ordinal)
                {
                    for (old, new) in before.iter().zip(after) {
                        for node in &mut score.flow.nodes {
                            if node.measure == old.id {
                                node.measure = new.id;
                            }
                        }
                    }
                }
            } else {
                let mut measures: Vec<_> = score.measures.values().collect();
                measures.sort_by_key(|measure| (measure.ordinal, measure.start, measure.id));
                score.flow.nodes = measures
                    .into_iter()
                    .enumerate()
                    .map(|(ordinal, measure)| super::playback::FlowNode {
                        measure: measure.id,
                        ordinal: ordinal as u32,
                    })
                    .collect();
            }
        }
        EditOp::SetArticulations {
            event,
            before,
            after,
        } => {
            let event = score
                .event_mut(*event)
                .ok_or(EditError::NotFound("event"))?;
            if event.articulations != *before {
                return Err(EditError::PreconditionFailed("articulations changed"));
            }
            event.articulations.clone_from(after);
        }
        EditOp::PutSpanner { id, before, after } => {
            if score.spanners.get(id).cloned() != *before {
                return Err(EditError::PreconditionFailed("spanner changed"));
            }
            if let Some(value) = after {
                if value.id != *id {
                    return Err(EditError::InvalidEntityId("spanner"));
                }
                score.spanners.insert(*id, value.clone());
            } else {
                score.spanners.remove(id);
            }
        }
        EditOp::PutAnnotation { id, before, after } => {
            if score.annotations.get(id).cloned() != *before {
                return Err(EditError::PreconditionFailed("annotation changed"));
            }
            if let Some(value) = after {
                if value.id != *id {
                    return Err(EditError::InvalidEntityId("annotation"));
                }
                score.annotations.insert(*id, value.clone());
            } else {
                score.annotations.remove(id);
            }
        }
        EditOp::SetFlow { before, after } => {
            if score.flow != *before {
                return Err(EditError::PreconditionFailed("flow changed"));
            }
            score.flow.clone_from(after);
        }
    }
    Ok(())
}

fn encode_native<T: SerBin>(magic: &[u8; 8], value: &T) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(magic);
    NATIVE_VERSION.ser_bin(&mut output);
    value.ser_bin(&mut output);
    output
}

fn decode_native<T: DeBin>(magic: &[u8; 8], input: &[u8]) -> Result<T, NativeFormatError> {
    if input.len() < 12 {
        return Err(NativeFormatError::Truncated);
    }
    if &input[..8] != magic {
        return Err(NativeFormatError::BadMagic);
    }
    let mut offset = 8;
    let version = u32::de_bin(&mut offset, input).map_err(decode_error)?;
    if version != NATIVE_VERSION {
        return Err(NativeFormatError::UnsupportedVersion(version));
    }
    let value = T::de_bin(&mut offset, input).map_err(decode_error)?;
    if offset != input.len() {
        return Err(NativeFormatError::TrailingBytes);
    }
    Ok(value)
}

fn decode_error(error: DeBinErr) -> NativeFormatError {
    NativeFormatError::Decode(error.to_string())
}
