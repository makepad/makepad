use super::graph::Score;
use super::id::MeasureId;
use super::time::{RationalError, ScoreTime};
use makepad_micro_serde::{DeBin, DeBinErr, SerBin};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, Default, Eq, PartialEq, SerBin, DeBin)]
pub struct FlowGraph {
    pub nodes: Vec<FlowNode>,
    pub repeats: Vec<RepeatSection>,
    pub voltas: Vec<VoltaEnding>,
    pub markers: Vec<FlowMarker>,
    pub jumps: Vec<JumpInstruction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct FlowNode {
    pub measure: MeasureId,
    pub ordinal: u32,
}

/// Inclusive node bounds for a nested repeat.
#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct RepeatSection {
    pub start: u32,
    pub end: u32,
    pub times: u16,
}

/// Inclusive node bounds shown only on selected passes of a repeat.
#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct VoltaEnding {
    pub start: u32,
    pub end: u32,
    pub repeat_start: u32,
    pub passes: Vec<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct FlowMarker {
    pub at: u32,
    pub kind: MarkerKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum MarkerKind {
    Segno,
    Coda,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct JumpInstruction {
    pub at: u32,
    pub kind: JumpKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum JumpKind {
    DaCapo,
    DalSegno,
    ToCoda,
    Fine,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaybackVisit {
    pub source_measure: MeasureId,
    pub pass: u16,
    pub play_start: ScoreTime,
    pub score_start: ScoreTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaybackPlanError {
    VisitLimit { limit: usize },
    CyclicNavigation { node: u32 },
    InvalidNode { node: u32 },
    MissingMeasure(MeasureId),
    InvalidRepeat { start: u32, end: u32, times: u16 },
    MissingMarker(MarkerKind),
    AmbiguousMarker(MarkerKind),
    Arithmetic(RationalError),
}

impl fmt::Display for PlaybackPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PlaybackPlanError {}

impl From<RationalError> for PlaybackPlanError {
    fn from(value: RationalError) -> Self {
        Self::Arithmetic(value)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PlaybackState {
    node: u32,
    repeat_passes: Vec<(u32, u16)>,
    used_jumps: Vec<u32>,
    navigation_armed: bool,
}

impl FlowGraph {
    /// Compiles notation navigation to bounded visits without changing the score graph.
    pub fn unfold(
        &self,
        score: &Score,
        max_visits: usize,
    ) -> Result<Vec<PlaybackVisit>, PlaybackPlanError> {
        if self.nodes.is_empty() {
            return Ok(Vec::new());
        }
        self.validate_structure()?;

        let mut visits = Vec::new();
        let mut node = 0_u32;
        let mut play_start = ScoreTime::ZERO;
        let mut repeat_passes = BTreeMap::<u32, u16>::new();
        let mut used_jumps = BTreeSet::<u32>::new();
        let mut navigation_armed = false;
        let mut states = BTreeSet::new();

        while usize::try_from(node).ok().is_some_and(|index| index < self.nodes.len()) {
            let state = PlaybackState {
                node,
                repeat_passes: repeat_passes.iter().map(|(a, b)| (*a, *b)).collect(),
                used_jumps: used_jumps.iter().copied().collect(),
                navigation_armed,
            };
            if !states.insert(state) {
                return Err(PlaybackPlanError::CyclicNavigation { node });
            }

            let suppressed = self.voltas.iter().any(|volta| {
                (volta.start..=volta.end).contains(&node)
                    && !volta
                        .passes
                        .contains(repeat_passes.get(&volta.repeat_start).unwrap_or(&1))
            });

            if !suppressed {
                if visits.len() >= max_visits {
                    return Err(PlaybackPlanError::VisitLimit { limit: max_visits });
                }
                let flow_node = self.nodes[node as usize];
                let measure = score
                    .measures
                    .get(&flow_node.measure)
                    .ok_or(PlaybackPlanError::MissingMeasure(flow_node.measure))?;
                let pass = repeat_passes.values().copied().max().unwrap_or(1);
                visits.push(PlaybackVisit {
                    source_measure: flow_node.measure,
                    pass,
                    play_start,
                    score_start: measure.start,
                });
                play_start = play_start.checked_add(measure.extent)?;
            }

            let mut jumped = false;
            for (jump_index, jump) in self.jumps.iter().enumerate() {
                if jump.at != node || suppressed {
                    continue;
                }
                let jump_index = u32::try_from(jump_index)
                    .map_err(|_| PlaybackPlanError::InvalidNode { node })?;
                match jump.kind {
                    JumpKind::DaCapo if used_jumps.insert(jump_index) => {
                        navigation_armed = true;
                        node = 0;
                        jumped = true;
                    }
                    JumpKind::DalSegno if used_jumps.insert(jump_index) => {
                        navigation_armed = true;
                        node = self.unique_marker(MarkerKind::Segno)?;
                        jumped = true;
                    }
                    JumpKind::ToCoda if navigation_armed && used_jumps.insert(jump_index) => {
                        node = self.unique_marker(MarkerKind::Coda)?;
                        jumped = true;
                    }
                    JumpKind::Fine if navigation_armed => return Ok(visits),
                    _ => {}
                }
                if jumped {
                    repeat_passes.clear();
                    break;
                }
            }
            if jumped {
                continue;
            }

            if let Some(repeat) = self
                .repeats
                .iter()
                .filter(|repeat| repeat.end == node)
                .max_by_key(|repeat| repeat.start)
            {
                let pass = repeat_passes.entry(repeat.start).or_insert(1);
                if *pass < repeat.times {
                    *pass += 1;
                    repeat_passes.retain(|start, _| *start <= repeat.start);
                    node = repeat.start;
                    continue;
                }
                repeat_passes.remove(&repeat.start);
            }
            node = node
                .checked_add(1)
                .ok_or(PlaybackPlanError::InvalidNode { node })?;
        }
        Ok(visits)
    }

    fn validate_structure(&self) -> Result<(), PlaybackPlanError> {
        let count = u32::try_from(self.nodes.len())
            .map_err(|_| PlaybackPlanError::InvalidNode { node: u32::MAX })?;
        for repeat in &self.repeats {
            if repeat.start > repeat.end || repeat.end >= count || repeat.times == 0 {
                return Err(PlaybackPlanError::InvalidRepeat {
                    start: repeat.start,
                    end: repeat.end,
                    times: repeat.times,
                });
            }
        }
        for volta in &self.voltas {
            if volta.start > volta.end || volta.end >= count || volta.passes.is_empty() {
                return Err(PlaybackPlanError::InvalidNode { node: volta.end });
            }
        }
        for marker in &self.markers {
            if marker.at >= count {
                return Err(PlaybackPlanError::InvalidNode { node: marker.at });
            }
        }
        for jump in &self.jumps {
            if jump.at >= count {
                return Err(PlaybackPlanError::InvalidNode { node: jump.at });
            }
        }
        Ok(())
    }

    fn unique_marker(&self, kind: MarkerKind) -> Result<u32, PlaybackPlanError> {
        let mut matching = self.markers.iter().filter(|marker| marker.kind == kind);
        let first = matching
            .next()
            .ok_or(PlaybackPlanError::MissingMarker(kind))?;
        if matching.next().is_some() {
            return Err(PlaybackPlanError::AmbiguousMarker(kind));
        }
        Ok(first.at)
    }
}
