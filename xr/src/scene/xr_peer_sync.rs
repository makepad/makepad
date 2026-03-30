use super::{hand_is_palm_down_closed_fist, CLOSED_FIST_GESTURE};
use crate::prelude::*;
use makepad_widgets::event::XrSyncAnchor;
use std::{
    collections::HashMap,
    sync::{mpsc::TryRecvError, Arc, Mutex},
    time::{Duration, Instant},
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.XrPeerSyncBase = #(XrPeerSync::register_widget(vm))
    mod.widgets.XrPeerSync = set_type_default() do mod.widgets.XrPeerSyncBase{
        body: mod.widgets.XrBodyKind.Disabled
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum XrPeerSyncAction {
    ActivityChanged(XrActivityId),
    BodySpawn(XrBodySpawn),
    #[default]
    None,
}

const XR_ALIGNMENT_CALLBACK_BUDGET_MILLIS: u64 = 25;
const XR_ALIGNMENT_CALLBACK_MAX_STEPS: usize = 4096;
const XR_ALIGNMENT_PROGRESS_SIGNAL_INTERVAL_MILLIS: u64 = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RemoteTransformSource {
    #[default]
    Raw,
    Anchor,
    Descriptor,
}

#[derive(Clone, Debug)]
struct RemotePeerState {
    peer: XrNetPeer,
    latest_state: Option<XrNetStateFrame>,
    last_state_received_at: f64,
    last_sync_anchor_seen_at: Option<f64>,
    latest_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    has_descriptor: bool,
    anchor_remote_to_local: Option<Mat4f>,
    descriptor_remote_to_local: Option<Mat4f>,
    remote_to_local: Option<Mat4f>,
    transform_source: RemoteTransformSource,
    last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    last_solve_ms: f64,
    last_solved_local_descriptor_version: Option<(u64, u64)>,
    last_solved_remote_descriptor_seq: Option<u32>,
    worker_progress: Option<XrDepthAlignMatcherProgress>,
}

impl RemotePeerState {
    fn new(peer: XrNetPeer) -> Self {
        Self {
            peer,
            latest_state: None,
            last_state_received_at: 0.0,
            last_sync_anchor_seen_at: None,
            latest_descriptor: None,
            has_descriptor: false,
            anchor_remote_to_local: None,
            descriptor_remote_to_local: None,
            remote_to_local: None,
            transform_source: RemoteTransformSource::Raw,
            last_solve_diagnostic: None,
            last_solve_ms: 0.0,
            last_solved_local_descriptor_version: None,
            last_solved_remote_descriptor_seq: None,
            worker_progress: None,
        }
    }
}

#[derive(Debug)]
struct AlignmentWorkerPeerState {
    peer: XrNetPeer,
    latest_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    remote_to_local: Option<Mat4f>,
    last_accepted_solution: Option<XrDepthAlignSolution>,
    last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    last_solve_ms: f64,
    last_solved_local_descriptor_version: Option<(u64, u64)>,
    last_solved_remote_descriptor_seq: Option<u32>,
    active_local_descriptor_version: Option<(u64, u64)>,
    active_remote_descriptor_seq: Option<u32>,
    queued_rerun: bool,
    matcher: Option<XrDepthAlignMatcher>,
}

impl AlignmentWorkerPeerState {
    fn new(peer: XrNetPeer) -> Self {
        Self {
            peer,
            latest_descriptor: None,
            remote_to_local: None,
            last_accepted_solution: None,
            last_solve_diagnostic: None,
            last_solve_ms: 0.0,
            last_solved_local_descriptor_version: None,
            last_solved_remote_descriptor_seq: None,
            active_local_descriptor_version: None,
            active_remote_descriptor_seq: None,
            queued_rerun: false,
            matcher: None,
        }
    }

    fn to_result(&self) -> AlignmentWorkerPeerResult {
        AlignmentWorkerPeerResult {
            remote_to_local: self.remote_to_local,
            last_solve_diagnostic: self.last_solve_diagnostic,
            last_solve_ms: self.last_solve_ms,
            last_solved_local_descriptor_version: self.last_solved_local_descriptor_version,
            last_solved_remote_descriptor_seq: self.last_solved_remote_descriptor_seq,
            worker_progress: self.matcher.as_ref().map(XrDepthAlignMatcher::progress),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AlignmentWorkerPeerResult {
    remote_to_local: Option<Mat4f>,
    last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    last_solve_ms: f64,
    last_solved_local_descriptor_version: Option<(u64, u64)>,
    last_solved_remote_descriptor_seq: Option<u32>,
    worker_progress: Option<XrDepthAlignMatcherProgress>,
}

#[derive(Clone, Debug)]
struct XrPeopleAlignmentWorkerResult {
    peer_results: HashMap<XrNetPeerId, AlignmentWorkerPeerResult>,
    alignment_debug_text: String,
}

#[derive(Clone)]
enum PendingLocalDescriptorUpdate {
    Set {
        frame: XrNetAlignmentDescriptorFrame,
        version: (u64, u64),
    },
    Clear,
}

#[derive(Clone)]
enum PendingPeerDescriptorUpdate {
    Set {
        peer: XrNetPeer,
        frame: XrNetAlignmentDescriptorFrame,
    },
    Remove,
}

#[derive(Default)]
struct XrPeopleAlignmentWorkerMailbox {
    pending_local_descriptor: Option<PendingLocalDescriptorUpdate>,
    pending_peer_updates: HashMap<XrNetPeerId, PendingPeerDescriptorUpdate>,
}

struct XrPeopleAlignmentWorker {
    store: XrTsdfStore,
    mailbox: Arc<Mutex<XrPeopleAlignmentWorkerMailbox>>,
    latest_result: Arc<Mutex<Option<XrPeopleAlignmentWorkerResult>>>,
}

impl XrPeopleAlignmentWorker {
    fn new(store: XrTsdfStore) -> Self {
        let mailbox = Arc::new(Mutex::new(XrPeopleAlignmentWorkerMailbox::default()));
        let latest_result = Arc::new(Mutex::new(None));
        let runtime = Arc::new(Mutex::new(AlignmentWorkerState::default()));
        let mailbox_callback = mailbox.clone();
        let latest_result_callback = latest_result.clone();
        let runtime_callback = runtime.clone();
        store.set_cooperative_step_callback(Some(Box::new(move || {
            xr_people_alignment_worker_step(
                &mailbox_callback,
                &runtime_callback,
                &latest_result_callback,
            )
        })));

        Self {
            store,
            mailbox,
            latest_result,
        }
    }

    fn set_local_descriptor(&mut self, frame: XrNetAlignmentDescriptorFrame, version: (u64, u64)) {
        self.send_mailbox_update(|mailbox| {
            mailbox.pending_local_descriptor =
                Some(PendingLocalDescriptorUpdate::Set { frame, version });
        });
    }

    fn clear_local_descriptor(&mut self) {
        self.send_mailbox_update(|mailbox| {
            mailbox.pending_local_descriptor = Some(PendingLocalDescriptorUpdate::Clear);
        });
    }

    fn set_peer_descriptor(&mut self, peer: XrNetPeer, frame: XrNetAlignmentDescriptorFrame) {
        self.send_mailbox_update(move |mailbox| {
            mailbox
                .pending_peer_updates
                .insert(peer.id, PendingPeerDescriptorUpdate::Set { peer, frame });
        });
    }

    fn remove_peer(&mut self, peer_id: XrNetPeerId) {
        self.send_mailbox_update(move |mailbox| {
            mailbox
                .pending_peer_updates
                .insert(peer_id, PendingPeerDescriptorUpdate::Remove);
        });
    }

    fn take_latest_result(&mut self) -> Option<XrPeopleAlignmentWorkerResult> {
        self.latest_result
            .lock()
            .ok()
            .and_then(|mut result| result.take())
    }

    fn send_mailbox_update<F>(&mut self, update: F)
    where
        F: FnOnce(&mut XrPeopleAlignmentWorkerMailbox),
    {
        if let Ok(mut mailbox) = self.mailbox.lock() {
            update(&mut mailbox);
        }
    }
}

impl Drop for XrPeopleAlignmentWorker {
    fn drop(&mut self) {
        self.store.set_cooperative_step_callback(None);
    }
}

#[derive(Default)]
struct AlignmentWorkerState {
    peers: HashMap<XrNetPeerId, AlignmentWorkerPeerState>,
    local_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    local_descriptor_version: Option<(u64, u64)>,
    last_progress_publish_at: Option<Instant>,
}

impl AlignmentWorkerState {
    fn apply_local_descriptor_update(&mut self, update: PendingLocalDescriptorUpdate) -> bool {
        match update {
            PendingLocalDescriptorUpdate::Set { frame, version } => {
                if descriptor_frames_equal(self.local_descriptor.as_ref(), Some(&frame)) {
                    return false;
                }
                self.local_descriptor = Some(frame);
                self.local_descriptor_version = Some(version);
                self.refresh_all_peer_alignments()
            }
            PendingLocalDescriptorUpdate::Clear => {
                if self.local_descriptor.is_none() {
                    return false;
                }
                self.local_descriptor = None;
                self.local_descriptor_version = None;
                self.refresh_all_peer_alignments()
            }
        }
    }

    fn apply_peer_update(
        &mut self,
        peer_id: XrNetPeerId,
        update: PendingPeerDescriptorUpdate,
    ) -> bool {
        match update {
            PendingPeerDescriptorUpdate::Set { peer, frame } => {
                let peer_state = self
                    .peers
                    .entry(peer_id)
                    .or_insert_with(|| AlignmentWorkerPeerState::new(peer));
                peer_state.peer = peer;
                if descriptor_frames_equal(peer_state.latest_descriptor.as_ref(), Some(&frame)) {
                    return false;
                }
                peer_state.latest_descriptor = Some(frame);
                self.refresh_peer_alignment(peer_id)
            }
            PendingPeerDescriptorUpdate::Remove => self.peers.remove(&peer_id).is_some(),
        }
    }

    fn refresh_all_peer_alignments(&mut self) -> bool {
        let local_descriptor = self.local_descriptor.clone();
        let local_descriptor_version = self.local_descriptor_version;
        let mut changed = false;
        for peer_state in self.peers.values_mut() {
            changed |= schedule_alignment_worker_peer(
                peer_state,
                local_descriptor.as_ref(),
                local_descriptor_version,
            );
        }
        changed
    }

    fn refresh_peer_alignment(&mut self, peer_id: XrNetPeerId) -> bool {
        let local_descriptor = self.local_descriptor.clone();
        let local_descriptor_version = self.local_descriptor_version;
        let Some(peer_state) = self.peers.get_mut(&peer_id) else {
            return false;
        };
        schedule_alignment_worker_peer(
            peer_state,
            local_descriptor.as_ref(),
            local_descriptor_version,
        )
    }

    fn next_pending_peer_id(&self) -> Option<XrNetPeerId> {
        self.peers
            .iter()
            .find_map(|(peer_id, peer_state)| peer_state.matcher.is_some().then_some(*peer_id))
    }

    fn has_pending_work(&self) -> bool {
        self.peers
            .values()
            .any(|peer_state| peer_state.matcher.is_some())
    }

    fn advance_pending_alignments(
        &mut self,
        budget: Duration,
        max_steps: usize,
    ) -> AlignmentWorkerStepOutcome {
        let started = std::time::Instant::now();
        let mut aggregate = AlignmentWorkerStepOutcome::default();
        let mut steps = 0usize;
        while steps < max_steps {
            let Some(peer_id) = self.next_pending_peer_id() else {
                break;
            };
            let outcome = self.step_peer_alignment(peer_id);
            aggregate.did_work |= outcome.did_work;
            aggregate.completed_cycle |= outcome.completed_cycle;
            aggregate.result_changed |= outcome.result_changed;
            if !outcome.did_work {
                break;
            }
            steps += 1;
            if !outcome.has_more_work {
                break;
            }
            if steps < max_steps && !budget.is_zero() && started.elapsed() >= budget {
                break;
            }
        }
        aggregate.has_more_work = self.has_pending_work();
        aggregate
    }

    fn step_peer_alignment(&mut self, peer_id: XrNetPeerId) -> AlignmentWorkerStepOutcome {
        let local_descriptor = self.local_descriptor.clone();
        let local_descriptor_version = self.local_descriptor_version;
        let Some(peer_state) = self.peers.get_mut(&peer_id) else {
            return AlignmentWorkerStepOutcome::default();
        };
        let previous_transform = peer_state.remote_to_local;
        let previous_solution = peer_state.last_accepted_solution;
        let previous_diagnostic = peer_state.last_solve_diagnostic;
        let Some(matcher) = peer_state.matcher.as_mut() else {
            return AlignmentWorkerStepOutcome::default();
        };
        let did_work = matcher.step();
        if !did_work && !matcher.is_finished() {
            return AlignmentWorkerStepOutcome::default();
        }
        if !matcher.is_finished() {
            return AlignmentWorkerStepOutcome {
                did_work: true,
                has_more_work: true,
                ..AlignmentWorkerStepOutcome::default()
            };
        }

        let diagnostic = peer_state
            .matcher
            .take()
            .and_then(|matcher| matcher.diagnostic())
            .expect("finished matcher should produce a diagnostic");
        peer_state.last_solved_local_descriptor_version =
            peer_state.active_local_descriptor_version;
        peer_state.last_solved_remote_descriptor_seq = peer_state.active_remote_descriptor_seq;
        peer_state.active_local_descriptor_version = None;
        peer_state.active_remote_descriptor_seq = None;
        peer_state.last_solve_ms = diagnostic.total_compute_ms as f64;
        peer_state.last_solve_diagnostic = Some(diagnostic);

        let previous_scored_on_current = previous_solution.and_then(|solution| {
            let local_descriptor = local_descriptor.as_ref()?;
            let remote_descriptor = peer_state.latest_descriptor.as_ref()?;
            Some(
                local_descriptor
                    .descriptor
                    .rescore_remote_to_local(&remote_descriptor.descriptor, solution),
            )
        });
        let next_solution = choose_stable_alignment_solution(
            peer_state.last_accepted_solution,
            previous_scored_on_current,
            &diagnostic,
        );
        peer_state.last_accepted_solution = next_solution;
        peer_state.remote_to_local =
            next_solution.map(|solution| solution.remote_to_local_transform());
        let queued_rerun = peer_state.queued_rerun;
        peer_state.queued_rerun = false;
        let rerun_changed = if queued_rerun {
            schedule_alignment_worker_peer(
                peer_state,
                local_descriptor.as_ref(),
                local_descriptor_version,
            )
        } else {
            false
        };
        AlignmentWorkerStepOutcome {
            did_work: true,
            completed_cycle: true,
            result_changed: previous_transform != peer_state.remote_to_local
                || previous_solution != peer_state.last_accepted_solution
                || previous_diagnostic != peer_state.last_solve_diagnostic
                || rerun_changed,
            has_more_work: self.has_pending_work(),
        }
    }

    fn make_result(&self) -> XrPeopleAlignmentWorkerResult {
        XrPeopleAlignmentWorkerResult {
            peer_results: self
                .peers
                .iter()
                .map(|(peer_id, peer_state)| (*peer_id, peer_state.to_result()))
                .collect(),
            alignment_debug_text: make_alignment_debug_text(LocalSceneState::Ready, &self.peers),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AlignmentWorkerStepOutcome {
    did_work: bool,
    completed_cycle: bool,
    result_changed: bool,
    has_more_work: bool,
}

fn xr_people_alignment_worker_step(
    mailbox: &Arc<Mutex<XrPeopleAlignmentWorkerMailbox>>,
    runtime: &Arc<Mutex<AlignmentWorkerState>>,
    latest_result: &Arc<Mutex<Option<XrPeopleAlignmentWorkerResult>>>,
) -> XrTsdfCooperativeStepResult {
    let (local_update, peer_updates) = match mailbox.lock() {
        Ok(mut mailbox) => (
            mailbox.pending_local_descriptor.take(),
            std::mem::take(&mut mailbox.pending_peer_updates),
        ),
        Err(_) => return XrTsdfCooperativeStepResult::default(),
    };

    let (step_outcome, publish_result) = match runtime.lock() {
        Ok(mut state) => {
            let mut result_changed = false;
            let mut did_work = false;
            if let Some(local_update) = local_update {
                let changed = state.apply_local_descriptor_update(local_update);
                result_changed |= changed;
                did_work |= changed;
            }
            for (peer_id, update) in peer_updates {
                let changed = state.apply_peer_update(peer_id, update);
                result_changed |= changed;
                did_work |= changed;
            }
            let mut step_outcome = state.advance_pending_alignments(
                Duration::from_millis(XR_ALIGNMENT_CALLBACK_BUDGET_MILLIS),
                XR_ALIGNMENT_CALLBACK_MAX_STEPS,
            );
            step_outcome.did_work |= did_work;
            step_outcome.result_changed |= result_changed;
            step_outcome.has_more_work |= state.has_pending_work();
            let publish_result = (result_changed
                || step_outcome.result_changed
                || step_outcome.completed_cycle
                || (step_outcome.did_work
                    && state.last_progress_publish_at.is_none_or(|last_publish| {
                        last_publish.elapsed()
                            >= Duration::from_millis(XR_ALIGNMENT_PROGRESS_SIGNAL_INTERVAL_MILLIS)
                    })))
            .then(|| {
                state.last_progress_publish_at = Some(Instant::now());
                state.make_result()
            });
            if !step_outcome.has_more_work {
                state.last_progress_publish_at = None;
            }
            (step_outcome, publish_result)
        }
        Err(_) => return XrTsdfCooperativeStepResult::default(),
    };

    if let Some(result) = publish_result {
        if let Ok(mut slot) = latest_result.lock() {
            *slot = Some(result);
        }
        SignalToUI::set_ui_signal();
    }

    XrTsdfCooperativeStepResult {
        did_work: step_outcome.did_work,
        has_more_work: step_outcome.has_more_work,
        completed_cycle: step_outcome.completed_cycle,
    }
}

fn descriptor_frames_equal(
    left: Option<&XrNetAlignmentDescriptorFrame>,
    right: Option<&XrNetAlignmentDescriptorFrame>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.descriptor == right.descriptor,
        (None, None) => true,
        _ => false,
    }
}

fn height_map_change_score(previous: &XrDepthAlignHeightMap, next: &XrDepthAlignHeightMap) -> f32 {
    if previous.size_x != next.size_x
        || previous.size_z != next.size_z
        || (previous.origin_x - next.origin_x).abs() > 1.0e-4
        || (previous.origin_z - next.origin_z).abs() > 1.0e-4
        || (previous.cell_size_meters - next.cell_size_meters).abs() > 1.0e-5
        || (previous.bottom_y_meters - next.bottom_y_meters).abs() > 1.0e-4
        || (previous.top_y_meters - next.top_y_meters).abs() > 1.0e-4
        || (previous.floor_y_meters - next.floor_y_meters).abs() > 1.0e-4
    {
        return 1.0;
    }
    let total = previous
        .heights_meters
        .len()
        .min(next.heights_meters.len())
        .max(1);
    let mut changed = 0usize;
    for (left, right) in previous
        .heights_meters
        .iter()
        .zip(next.heights_meters.iter())
    {
        if left.is_finite() != right.is_finite()
            || (left.is_finite() && right.is_finite() && (*left - *right).abs() >= 0.05)
        {
            changed += 1;
        }
    }
    changed as f32 / total as f32
}

fn descriptor_change_score(
    previous: &XrDepthAlignDescriptor,
    next: &XrDepthAlignDescriptor,
) -> f32 {
    match (previous.height_map.as_ref(), next.height_map.as_ref()) {
        (Some(previous), Some(next)) => height_map_change_score(previous, next),
        (None, None) => 0.0,
        (Some(_), None) | (None, Some(_)) => 1.0,
    }
}

fn schedule_alignment_worker_peer(
    peer_state: &mut AlignmentWorkerPeerState,
    local_descriptor: Option<&XrNetAlignmentDescriptorFrame>,
    local_descriptor_version: Option<(u64, u64)>,
) -> bool {
    let (Some(local_descriptor), Some(local_descriptor_version), Some(remote_descriptor)) = (
        local_descriptor,
        local_descriptor_version,
        peer_state.latest_descriptor.as_ref(),
    ) else {
        return clear_alignment_worker_peer(peer_state);
    };

    if peer_state.matcher.is_some() {
        let solving_current_pair = peer_state.active_local_descriptor_version
            == Some(local_descriptor_version)
            && peer_state.active_remote_descriptor_seq == Some(remote_descriptor.seq);
        if solving_current_pair {
            return false;
        }
        let changed = !peer_state.queued_rerun;
        peer_state.queued_rerun = true;
        return changed;
    }

    if peer_state.last_solved_local_descriptor_version == Some(local_descriptor_version)
        && peer_state.last_solved_remote_descriptor_seq == Some(remote_descriptor.seq)
    {
        return false;
    }

    if !descriptor_pair_ready_for_solve(
        peer_state.last_solved_local_descriptor_version,
        peer_state.last_solved_remote_descriptor_seq,
        local_descriptor_version,
        remote_descriptor.seq,
    ) {
        return false;
    }

    peer_state.last_solve_diagnostic = None;
    peer_state.last_solve_ms = 0.0;
    peer_state.active_local_descriptor_version = Some(local_descriptor_version);
    peer_state.active_remote_descriptor_seq = Some(remote_descriptor.seq);
    peer_state.queued_rerun = false;
    peer_state.matcher = Some(XrDepthAlignMatcher::new(
        &local_descriptor.descriptor,
        &remote_descriptor.descriptor,
        peer_state.last_accepted_solution,
    ));
    true
}

fn descriptor_pair_ready_for_solve(
    last_solved_local_descriptor_version: Option<(u64, u64)>,
    last_solved_remote_descriptor_seq: Option<u32>,
    local_descriptor_version: (u64, u64),
    remote_descriptor_seq: u32,
) -> bool {
    match (
        last_solved_local_descriptor_version,
        last_solved_remote_descriptor_seq,
    ) {
        (Some(last_local), Some(last_remote)) => {
            local_descriptor_version != last_local || remote_descriptor_seq != last_remote
        }
        _ => true,
    }
}

fn clear_alignment_worker_peer(peer_state: &mut AlignmentWorkerPeerState) -> bool {
    let changed = peer_state.remote_to_local.is_some()
        || peer_state.last_solve_diagnostic.is_some()
        || peer_state.matcher.is_some();
    peer_state.remote_to_local = None;
    peer_state.last_accepted_solution = None;
    peer_state.last_solve_diagnostic = None;
    peer_state.last_solve_ms = 0.0;
    peer_state.last_solved_local_descriptor_version = None;
    peer_state.last_solved_remote_descriptor_seq = None;
    peer_state.active_local_descriptor_version = None;
    peer_state.active_remote_descriptor_seq = None;
    peer_state.queued_rerun = false;
    peer_state.matcher = None;
    changed
}

fn choose_stable_alignment_solution(
    previous: Option<XrDepthAlignSolution>,
    previous_scored_on_current: Option<XrDepthAlignSolution>,
    diagnostic: &XrDepthAlignSolveDiagnostic,
) -> Option<XrDepthAlignSolution> {
    let candidate = diagnostic.accepted_solution();
    let Some(previous) = previous else {
        return candidate;
    };
    let previous_on_current = previous_scored_on_current.unwrap_or(previous);
    let previous_still_supported = previous_on_current.is_accepted(diagnostic);
    let Some(candidate) = candidate else {
        return previous_still_supported.then_some(previous);
    };
    let previous_score = alignment_lock_score(previous_on_current, Some(*diagnostic));
    let candidate_score = alignment_lock_score(candidate, Some(*diagnostic));
    let score_delta = candidate_score - previous_score;
    if previous_still_supported && score_delta < -0.02 {
        return Some(previous);
    }
    if previous_still_supported
        && is_large_alignment_jump(previous, candidate)
        && score_delta < 0.08
    {
        return Some(previous);
    }
    if previous_still_supported
        && score_delta < 0.03
        && candidate.residual_meters >= previous_on_current.residual_meters - 0.02
        && candidate.symmetry_confidence <= previous_on_current.symmetry_confidence + 0.03
        && candidate.matched_samples <= previous_on_current.matched_samples
    {
        return Some(previous);
    }
    Some(candidate)
}

#[cfg(test)]
mod descriptor_pair_tests {
    use super::descriptor_pair_ready_for_solve;

    #[test]
    fn initial_descriptor_pair_solves_immediately() {
        assert!(descriptor_pair_ready_for_solve(None, None, (1, 0), 7));
    }

    #[test]
    fn one_sided_descriptor_updates_resolve_immediately() {
        assert!(descriptor_pair_ready_for_solve(
            Some((1, 0)),
            Some(7),
            (2, 0),
            7,
        ));
        assert!(descriptor_pair_ready_for_solve(
            Some((1, 0)),
            Some(7),
            (1, 0),
            8,
        ));
        assert!(descriptor_pair_ready_for_solve(
            Some((1, 0)),
            Some(7),
            (2, 0),
            8,
        ));
    }
}

fn alignment_lock_score(
    solution: XrDepthAlignSolution,
    diagnostic: Option<XrDepthAlignSolveDiagnostic>,
) -> f32 {
    let (signal_factor, overlap_factor) = diagnostic
        .map(|diagnostic| {
            let signal_factor = (diagnostic
                .local_wall_samples
                .min(diagnostic.remote_wall_samples) as f32
                / 12.0)
                .clamp(0.0, 1.0);
            let overlap_ratio = (solution.matched_samples as f32
                / diagnostic.remote_wall_samples.max(1) as f32)
                .clamp(0.0, 1.0);
            (signal_factor, overlap_ratio * signal_factor.sqrt())
        })
        .unwrap_or_else(|| {
            let matched = (solution.matched_samples as f32 / 4.0).clamp(0.0, 1.0);
            (0.5, matched)
        });
    (solution.ranking_confidence() * 0.50
        + solution.symmetry_confidence * 0.20
        + signal_factor * 0.15
        + overlap_factor * 0.15)
        .clamp(0.0, 1.0)
}

fn wrap_alignment_angle(mut angle: f32) -> f32 {
    while angle <= -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    angle
}

fn is_large_alignment_jump(left: XrDepthAlignSolution, right: XrDepthAlignSolution) -> bool {
    let yaw_delta = wrap_alignment_angle(left.yaw_radians - right.yaw_radians).abs();
    let translation_delta = (left.translation - right.translation).length();
    yaw_delta > 0.22 || translation_delta > 0.18
}

fn transform_source_label(source: RemoteTransformSource) -> &'static str {
    match source {
        RemoteTransformSource::Raw => "raw",
        RemoteTransformSource::Anchor => "anchor",
        RemoteTransformSource::Descriptor => "solved",
    }
}

fn descriptor_version_label(version: Option<(u64, u64)>) -> String {
    version
        .map(|(mesh_generation, update_sequence)| format!("{mesh_generation}/{update_sequence}"))
        .unwrap_or_else(|| "-".to_string())
}

fn descriptor_seq_label(seq: Option<u32>) -> String {
    seq.map(|seq| seq.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn descriptor_contour_sample_count(descriptor: &XrDepthAlignDescriptor) -> usize {
    let sample_count = descriptor
        .samples
        .iter()
        .filter(|sample| sample.kind == XrDepthAlignSampleKind::Wall)
        .count();
    if sample_count != 0 {
        sample_count
    } else if let Some(height_map) = descriptor.height_map.as_ref() {
        let valid_cells = height_map
            .heights_meters
            .iter()
            .filter(|value| value.is_finite())
            .count();
        ((valid_cells + 127) / 128).max(1)
    } else {
        0
    }
}

fn descriptor_height_map_filled_cells(descriptor: &XrDepthAlignDescriptor) -> usize {
    descriptor
        .height_map
        .as_ref()
        .map(|height_map| {
            height_map
                .heights_meters
                .iter()
                .filter(|value| value.is_finite())
                .count()
        })
        .unwrap_or(0)
}

fn descriptor_height_map_status(descriptor: &XrDepthAlignDescriptor) -> String {
    descriptor
        .height_map
        .as_ref()
        .map(|height_map| {
            format!(
                "{}x{} fill {}",
                height_map.size_x,
                height_map.size_z,
                descriptor_height_map_filled_cells(descriptor),
            )
        })
        .unwrap_or_else(|| "missing".to_string())
}

fn solve_outcome_label(diagnostic: Option<XrDepthAlignSolveDiagnostic>) -> &'static str {
    let Some(diagnostic) = diagnostic else {
        return "pending";
    };
    match diagnostic.outcome() {
        XrDepthAlignSolveOutcome::MissingSamples => "need-signal",
        XrDepthAlignSolveOutcome::NoCandidate => "no-candidate",
        XrDepthAlignSolveOutcome::Rejected => "rejected",
        XrDepthAlignSolveOutcome::Accepted => "accepted",
    }
}

fn make_alignment_state_text(
    local_scene_state: LocalSceneState,
    local_descriptor_version: Option<(u64, u64)>,
    peers: &HashMap<XrNetPeerId, RemotePeerState>,
) -> String {
    let local_descriptor_ready = matches!(local_scene_state, LocalSceneState::Ready);
    let local_version = descriptor_version_label(local_descriptor_version);
    let Some((peer_id, peer_state)) = peers.iter().max_by_key(|(peer_id, peer_state)| {
        (
            peer_state.worker_progress.is_some(),
            peer_state.last_solve_diagnostic.is_some(),
            peer_state.latest_descriptor.is_some(),
            peer_state.latest_state.is_some(),
            std::cmp::Reverse(peer_id.0),
        )
    }) else {
        return format!(
            "AlignState: local map {} v{} | waiting for peer map",
            if local_descriptor_ready { "yes" } else { "no" },
            local_version,
        );
    };
    let peer_label = format!("{:08x}", peer_id.0);
    let remote_descriptor = peer_state.latest_descriptor.as_ref();
    if peer_state.worker_progress.is_some() {
        return format!("AlignState {peer_label}: solving");
    }
    format!(
        "AlignState {peer_label}: local map {} v{} | remote map {} seq {} | worker lv{} rv{} {} match {:.1}ms | pose {}",
        if local_descriptor_ready { "yes" } else { "no" },
        local_version,
        if remote_descriptor.is_some() { "yes" } else { "no" },
        descriptor_seq_label(remote_descriptor.map(|frame| frame.seq)),
        descriptor_version_label(peer_state.last_solved_local_descriptor_version),
        descriptor_seq_label(peer_state.last_solved_remote_descriptor_seq),
        solve_outcome_label(peer_state.last_solve_diagnostic),
        peer_state.last_solve_ms,
        transform_source_label(peer_state.transform_source),
    )
}

fn make_peer_scene_debug_text(
    has_local_descriptor: bool,
    peers: &HashMap<XrNetPeerId, RemotePeerState>,
) -> String {
    let Some((peer_id, peer_state)) = peers.iter().max_by_key(|(peer_id, peer_state)| {
        (
            peer_state.last_solve_diagnostic.is_some(),
            peer_state.latest_descriptor.is_some(),
            peer_state.latest_state.is_some(),
            std::cmp::Reverse(peer_id.0),
        )
    }) else {
        return "PeerMap: waiting for peer".to_string();
    };
    let peer_label = format!("{:08x}", peer_id.0);
    let state_text = if peer_state.latest_state.is_some() {
        "yes"
    } else {
        "no"
    };
    let remote_seq =
        descriptor_seq_label(peer_state.latest_descriptor.as_ref().map(|frame| frame.seq));
    if let Some(diagnostic) = peer_state.last_solve_diagnostic {
        return format!(
            "PeerMap {peer_label}: state {state_text} | map {} seq {} | signal {} | pose {}",
            if peer_state.has_descriptor {
                "yes"
            } else {
                "no"
            },
            remote_seq,
            diagnostic.remote_wall_samples,
            transform_source_label(peer_state.transform_source),
        );
    }
    if let Some(descriptor) = peer_state.latest_descriptor.as_ref() {
        return format!(
            "PeerMap {peer_label}: state {state_text} | map yes seq {} {} | pose {}{}",
            descriptor_seq_label(Some(descriptor.seq)),
            descriptor_height_map_status(&descriptor.descriptor),
            transform_source_label(peer_state.transform_source),
            if has_local_descriptor {
                " | solve pending"
            } else {
                ""
            },
        );
    }
    format!(
        "PeerMap {peer_label}: state {state_text} | map {} seq {} | pose {}{}",
        if peer_state.has_descriptor {
            "yes"
        } else {
            "no"
        },
        remote_seq,
        transform_source_label(peer_state.transform_source),
        if has_local_descriptor && peer_state.has_descriptor {
            " | solve pending"
        } else {
            ""
        },
    )
}

fn make_pending_alignment_debug_text(
    local_descriptor_text: &str,
    peers: &HashMap<XrNetPeerId, RemotePeerState>,
) -> String {
    let Some((peer_id, peer_state)) = peers.iter().max_by_key(|(peer_id, peer_state)| {
        (
            peer_state.last_solve_diagnostic.is_some(),
            peer_state.latest_descriptor.is_some(),
            peer_state.latest_state.is_some(),
            std::cmp::Reverse(peer_id.0),
        )
    }) else {
        return format!("{local_descriptor_text} | waiting for peer heightmap");
    };
    if peer_state.last_solve_diagnostic.is_some() {
        return local_descriptor_text.to_string();
    }
    let peer_label = format!("{:08x}", peer_id.0);
    if let Some(descriptor) = peer_state.latest_descriptor.as_ref() {
        format!(
            "{local_descriptor_text} | {peer_label}: remote map seq {} {} | solve pending",
            descriptor.seq,
            descriptor_height_map_status(&descriptor.descriptor),
        )
    } else if peer_state.has_descriptor {
        format!("{local_descriptor_text} | {peer_label}: solve pending")
    } else {
        format!("{local_descriptor_text} | {peer_label}: waiting for peer heightmap")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalSceneState {
    Missing,
    PublishPending,
    Ready,
}

fn make_alignment_debug_text(
    local_scene_state: LocalSceneState,
    peers: &HashMap<XrNetPeerId, AlignmentWorkerPeerState>,
) -> String {
    let Some((peer_id, peer_state)) = peers.iter().max_by_key(|(peer_id, peer_state)| {
        (
            peer_state.matcher.is_some(),
            peer_state.last_solve_diagnostic.is_some(),
            peer_state.latest_descriptor.is_some(),
            std::cmp::Reverse(peer_id.0),
        )
    }) else {
        return match local_scene_state {
            LocalSceneState::Ready => "AlignDbg: waiting for peer heightmap".to_string(),
            LocalSceneState::PublishPending => {
                "AlignDbg: local heightmap ready | waiting to publish".to_string()
            }
            LocalSceneState::Missing => "AlignDbg: waiting for local heightmap".to_string(),
        };
    };
    let peer_label = format!("{:08x}", peer_id.0);
    if local_scene_state == LocalSceneState::Missing {
        return format!("AlignDbg {peer_label}: waiting for local heightmap");
    }
    if local_scene_state == LocalSceneState::PublishPending {
        return format!("AlignDbg {peer_label}: local heightmap ready | waiting to publish");
    }
    let Some(diagnostic) = peer_state.last_solve_diagnostic else {
        if peer_state.latest_descriptor.is_some() {
            return format!(
                "AlignDbg {peer_label}: {}",
                if peer_state.matcher.is_some() {
                    "solving"
                } else {
                    "waiting for solve"
                },
            );
        }
        return format!("AlignDbg {peer_label}: waiting for peer heightmap");
    };
    match diagnostic.outcome() {
        XrDepthAlignSolveOutcome::MissingSamples => format!("AlignDbg {peer_label}: need-signal"),
        XrDepthAlignSolveOutcome::NoCandidate => format!("AlignDbg {peer_label}: no-candidate"),
        XrDepthAlignSolveOutcome::Rejected => format!("AlignDbg {peer_label}: rejected"),
        XrDepthAlignSolveOutcome::Accepted => format!("AlignDbg {peer_label}: aligned"),
    }
}

#[derive(Default)]
struct XrPeerSyncLocalState {
    state_time: f64,
    anchor: Option<XrAnchor>,
    anchor_override: Option<XrAnchor>,
    sync_anchor: Option<XrSyncAnchor>,
    fist_hold_anchor: Option<XrAnchor>,
    descriptor: Option<XrNetAlignmentDescriptorFrame>,
    descriptor_version: Option<(u64, u64)>,
    slice_preview: Option<XrDepthAlignSlicePreview>,
    last_sent_descriptor_signature: Option<(u64, u64)>,
    last_sent_descriptor: Option<XrDepthAlignDescriptor>,
    last_sent_descriptor_at: Option<f64>,
}

impl XrPeerSyncLocalState {
    fn effective_anchor(&self) -> Option<XrAnchor> {
        self.anchor_override.or(self.anchor)
    }

    fn active_sync_anchor(&self) -> Option<XrSyncAnchor> {
        self.sync_anchor.filter(|sync| {
            self.state_time - sync.captured_at <= XrPeerSync::SYNC_MATCH_ACTIVE_WINDOW_SECONDS
        })
    }

    fn scene_state(&self) -> LocalSceneState {
        if self.descriptor.is_some() {
            LocalSceneState::Ready
        } else if self.contour_sample_count() != 0 {
            LocalSceneState::PublishPending
        } else {
            LocalSceneState::Missing
        }
    }

    fn contour_sample_count(&self) -> usize {
        self.descriptor
            .as_ref()
            .map(|frame| descriptor_contour_sample_count(&frame.descriptor))
            .unwrap_or(0)
    }
}

#[derive(Default)]
struct XrPeerSyncMetrics {
    tx_state_count: u64,
    tx_descriptor_count: u64,
    tx_activity_count: u64,
    tx_body_spawn_count: u64,
    rx_join_count: u64,
    rx_leave_count: u64,
    rx_state_count: u64,
    rx_descriptor_count: u64,
    rx_activity_count: u64,
    rx_body_spawn_count: u64,
    last_event_text: String,
}

impl XrPeerSyncMetrics {
    fn record_node_started(&mut self) {
        self.last_event_text = "node started".to_string();
    }

    fn record_join(&mut self, peer_id: XrNetPeerId) {
        self.rx_join_count = self.rx_join_count.saturating_add(1);
        self.last_event_text = format!("join {}", XrPeerSync::peer_label(peer_id));
    }

    fn record_leave(&mut self, peer_id: XrNetPeerId) {
        self.rx_leave_count = self.rx_leave_count.saturating_add(1);
        self.last_event_text = format!("leave {}", XrPeerSync::peer_label(peer_id));
    }

    fn record_state(&mut self, peer_id: XrNetPeerId, seq: u32) {
        self.rx_state_count = self.rx_state_count.saturating_add(1);
        self.last_event_text = format!("state {} seq {}", XrPeerSync::peer_label(peer_id), seq);
    }

    fn record_descriptor(&mut self, peer_id: XrNetPeerId, seq: u32) {
        self.rx_descriptor_count = self.rx_descriptor_count.saturating_add(1);
        self.last_event_text = format!("desc {} seq {}", XrPeerSync::peer_label(peer_id), seq);
    }

    fn record_activity_tx(&mut self, activity: XrNetActivityState) {
        self.tx_activity_count = self.tx_activity_count.saturating_add(1);
        self.last_event_text = format!(
            "tx activity {} tick {}",
            activity.activity_id.to_live_id().0,
            activity.changed_tick
        );
    }

    fn record_activity_rx(&mut self, peer_id: XrNetPeerId, activity: XrNetActivityState) {
        self.rx_activity_count = self.rx_activity_count.saturating_add(1);
        self.last_event_text = format!(
            "activity {} {} tick {}",
            XrPeerSync::peer_label(peer_id),
            activity.activity_id.to_live_id().0,
            activity.changed_tick
        );
    }

    fn record_body_spawn_tx(&mut self, spawn_label: u64) {
        self.tx_body_spawn_count = self.tx_body_spawn_count.saturating_add(1);
        self.last_event_text = format!("tx spawn {:016x}", spawn_label);
    }

    fn record_body_spawn_rx(&mut self, peer_id: XrNetPeerId, spawn_label: u64) {
        self.rx_body_spawn_count = self.rx_body_spawn_count.saturating_add(1);
        self.last_event_text = format!(
            "spawn {} {:016x}",
            XrPeerSync::peer_label(peer_id),
            spawn_label
        );
    }

    fn last_event_label(&self) -> &str {
        if self.last_event_text.is_empty() {
            "none"
        } else {
            &self.last_event_text
        }
    }
}

#[derive(Default)]
struct XrPeerRegistry {
    peers: HashMap<XrNetPeerId, RemotePeerState>,
    accepted_sync_ids: HashMap<XrNetPeerId, (u32, u32)>,
}

impl XrPeerRegistry {
    fn len(&self) -> usize {
        self.peers.len()
    }

    fn peer_ids(&self) -> Vec<XrNetPeerId> {
        self.peers.keys().copied().collect()
    }

    fn preferred_peer(&self) -> Option<(XrNetPeerId, RemotePeerState)> {
        self.peers
            .iter()
            .max_by_key(|(peer_id, peer_state)| {
                (
                    peer_state.remote_to_local.is_some(),
                    peer_state.latest_state.is_some(),
                    std::cmp::Reverse(peer_id.0),
                )
            })
            .map(|(peer_id, peer_state)| (*peer_id, peer_state.clone()))
    }

    fn track_join(&mut self, peer: XrNetPeer) {
        self.peers
            .entry(peer.id)
            .or_insert_with(|| RemotePeerState::new(peer));
    }

    fn track_leave(&mut self, peer_id: XrNetPeerId) {
        self.peers.remove(&peer_id);
        self.accepted_sync_ids.remove(&peer_id);
    }

    fn track_state(&mut self, peer: XrNetPeer, frame: XrNetStateFrame, local_state_time: f64) {
        let peer_state = self
            .peers
            .entry(peer.id)
            .or_insert_with(|| RemotePeerState::new(peer));
        peer_state.peer = peer;
        peer_state.last_state_received_at = local_state_time;
        peer_state.last_sync_anchor_seen_at = frame.state.sync_anchor.map(|_| local_state_time);
        peer_state.latest_state = Some(frame);
    }

    fn track_descriptor(&mut self, peer: XrNetPeer, frame: XrNetAlignmentDescriptorFrame) {
        let peer_state = self
            .peers
            .entry(peer.id)
            .or_insert_with(|| RemotePeerState::new(peer));
        peer_state.peer = peer;
        peer_state.latest_descriptor = Some(frame);
        peer_state.has_descriptor = true;
    }

    fn apply_alignment_results(
        &mut self,
        peer_results: HashMap<XrNetPeerId, AlignmentWorkerPeerResult>,
    ) {
        for peer_state in self.peers.values_mut() {
            peer_state.descriptor_remote_to_local = None;
        }
        for (peer_id, peer_result) in peer_results {
            if let Some(peer_state) = self.peers.get_mut(&peer_id) {
                peer_state.descriptor_remote_to_local = peer_result.remote_to_local;
                peer_state.last_solve_diagnostic = peer_result.last_solve_diagnostic;
                peer_state.last_solve_ms = peer_result.last_solve_ms;
                peer_state.last_solved_local_descriptor_version =
                    peer_result.last_solved_local_descriptor_version;
                peer_state.last_solved_remote_descriptor_seq =
                    peer_result.last_solved_remote_descriptor_seq;
                peer_state.worker_progress = peer_result.worker_progress;
                peer_state.has_descriptor =
                    peer_state.has_descriptor || peer_result.last_solve_diagnostic.is_some();
            }
        }
    }

    fn refresh_transforms(
        &mut self,
        cx: &mut Cx,
        local_anchor: Option<XrAnchor>,
        local_sync_anchor: Option<XrSyncAnchor>,
        local_fist_hold_anchor: Option<XrAnchor>,
        local_anchor_override: &mut Option<XrAnchor>,
        now: f64,
    ) -> bool {
        let mut changed = false;

        for (peer_id, peer_state) in self.peers.iter_mut() {
            peer_state.anchor_remote_to_local = None;
            if let (Some(local_anchor), Some(state_frame)) =
                (local_anchor, peer_state.latest_state.as_ref())
            {
                if let Some(remote_anchor) = state_frame.state.anchor {
                    peer_state.anchor_remote_to_local =
                        Some(remote_anchor.mapping_to(&local_anchor));
                }
            }

            if peer_state.anchor_remote_to_local.is_none() {
                if let Some(state_frame) = peer_state.latest_state.as_ref() {
                    let remote_sync_anchor = state_frame.state.sync_anchor.filter(|_| {
                        peer_state
                            .last_sync_anchor_seen_at
                            .is_some_and(|last_seen_at| {
                                now - last_seen_at <= XrPeerSync::SYNC_MATCH_RECEIVE_WINDOW_SECONDS
                            })
                    });
                    let remote_fist_hold_anchor =
                        XrPeerSync::state_fist_ack_anchor(&state_frame.state);

                    if let Some(local_sync_anchor) = local_sync_anchor {
                        if let Some(remote_anchor) = remote_sync_anchor
                            .map(|sync| (sync.anchor, sync.id))
                            .or_else(|| remote_fist_hold_anchor.map(|anchor| (anchor, u32::MAX)))
                        {
                            let sync_ids = (local_sync_anchor.id, remote_anchor.1);
                            if self.accepted_sync_ids.get(peer_id) != Some(&sync_ids) {
                                self.accepted_sync_ids.insert(*peer_id, sync_ids);
                                *local_anchor_override = Some(local_sync_anchor.anchor);
                                cx.xr_set_local_anchor(local_sync_anchor.anchor);
                            }
                            peer_state.anchor_remote_to_local =
                                Some(remote_anchor.0.mapping_to(&local_sync_anchor.anchor));
                        }
                    } else if let (Some(local_fist_hold_anchor), Some(remote_sync_anchor)) =
                        (local_fist_hold_anchor, remote_sync_anchor)
                    {
                        let sync_ids = (u32::MAX, remote_sync_anchor.id);
                        if self.accepted_sync_ids.get(peer_id) != Some(&sync_ids) {
                            self.accepted_sync_ids.insert(*peer_id, sync_ids);
                            *local_anchor_override = Some(local_fist_hold_anchor);
                            cx.xr_set_local_anchor(local_fist_hold_anchor);
                        }
                        peer_state.anchor_remote_to_local = Some(
                            remote_sync_anchor
                                .anchor
                                .mapping_to(&local_fist_hold_anchor),
                        );
                    }
                }
            }

            let next_transform = peer_state
                .anchor_remote_to_local
                .or(peer_state.descriptor_remote_to_local);
            let next_source = if peer_state.anchor_remote_to_local.is_some() {
                RemoteTransformSource::Anchor
            } else if peer_state.descriptor_remote_to_local.is_some() {
                RemoteTransformSource::Descriptor
            } else {
                RemoteTransformSource::Raw
            };
            if peer_state.remote_to_local != next_transform
                || peer_state.transform_source != next_source
            {
                peer_state.remote_to_local = next_transform;
                peer_state.transform_source = next_source;
                changed = true;
            }
        }

        changed
    }
}

#[derive(Default)]
struct XrPeerSyncRuntime {
    net_node: Option<XrNetNode>,
    alignment_worker: Option<XrPeopleAlignmentWorker>,
    local: XrPeerSyncLocalState,
    registry: XrPeerRegistry,
    shared_objects: XrSharedObjectRegistry,
    accepted_activity: Option<XrNetActivityState>,
    metrics: XrPeerSyncMetrics,
}

#[derive(Default)]
struct XrPeerSyncDiagnostics {
    status: String,
    network_status: String,
    peer_scene_status: String,
    alignment_debug_status: String,
    alignment_state_status: String,
}

impl XrPeerSyncDiagnostics {
    fn status_text(&self) -> &str {
        if self.status.is_empty() {
            "AlignSync: off"
        } else {
            &self.status
        }
    }

    fn network_status_text(&self) -> &str {
        if self.network_status.is_empty() {
            "Network: off"
        } else {
            &self.network_status
        }
    }

    fn alignment_debug_text(&self) -> &str {
        if self.alignment_debug_status.is_empty() {
            "AlignDbg: off"
        } else {
            &self.alignment_debug_status
        }
    }

    fn alignment_state_text(&self) -> &str {
        if self.alignment_state_status.is_empty() {
            "AlignState: off"
        } else {
            &self.alignment_state_status
        }
    }

    fn peer_scene_text(&self) -> &str {
        if self.peer_scene_status.is_empty() {
            "PeerMap: off"
        } else {
            &self.peer_scene_status
        }
    }

    fn set_enabled_defaults(&mut self, auto_alignment_enabled: bool, network_ready: bool) {
        if network_ready {
            self.status = "AlignSync: waiting for peer heightmap".to_string();
            self.network_status = "Network: bridge ready | waiting for local XR frames".to_string();
        } else {
            self.status = "AlignSync: network unavailable".to_string();
            self.network_status = "Network: bind failed".to_string();
        }
        self.peer_scene_status = "PeerMap: waiting for peer".to_string();
        self.alignment_debug_status = if auto_alignment_enabled {
            "AlignDbg: waiting for local heightmap".to_string()
        } else {
            "AlignDbg: manual sync idle".to_string()
        };
        self.alignment_state_status = if auto_alignment_enabled {
            "AlignState: local map no v- | waiting for peer map".to_string()
        } else {
            "AlignState: local anchor no | sync idle | waiting for peer".to_string()
        };
    }

    fn set_disabled(&mut self) {
        self.status = "AlignSync: off".to_string();
        self.network_status = "Network: off".to_string();
        self.peer_scene_status = "PeerMap: off".to_string();
        self.alignment_debug_status = "AlignDbg: off".to_string();
        self.alignment_state_status = "AlignState: off".to_string();
    }

    fn set_network_bind_failed(&mut self, err: &str) {
        self.status = format!("AlignSync: network bind failed ({err})");
        self.network_status = format!("Network: bind failed ({err})");
    }

    fn set_network_disconnected(&mut self) {
        self.status = "AlignSync: network worker disconnected, retrying".to_string();
        self.network_status = "Network: worker disconnected".to_string();
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrPeerSync {
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[live(false)]
    auto_alignment_enabled: bool,
    #[rust]
    enabled: bool,
    #[rust]
    runtime: XrPeerSyncRuntime,
    #[rust]
    diagnostics: XrPeerSyncDiagnostics,
    #[cast]
    #[deref]
    node: XrNode,
}

impl XrPeerSync {
    const HEADSET_SIZE: Vec3f = Vec3f {
        x: 0.18,
        y: 0.11,
        z: 0.14,
    };
    const HAND_SIZE: Vec3f = Vec3f {
        x: 0.08,
        y: 0.05,
        z: 0.10,
    };
    const ANCHOR_MARKER_SIZE: f32 = 0.060;
    const REMOTE_ANCHOR_MARKER_SIZE: f32 = 0.050;
    const SYNC_MATCH_RECEIVE_WINDOW_SECONDS: f64 = 0.45;
    // Fraction of heightmap cells that must change by at least 5 cm before we republish.
    const DESCRIPTOR_SEND_MIN_CHANGE_PERCENT: f32 = 4.0;
    const SYNC_MATCH_ACTIVE_WINDOW_SECONDS: f64 = 1.35;
    const FIST_ACK_MAX_VERTICAL_DELTA_METERS: f32 = 0.22;
    const FIST_ACK_MAX_DEPTH_DELTA_METERS: f32 = 0.22;
    const FIST_ACK_MIN_HAND_GAP_METERS: f32 = 0.06;
    const FIST_ACK_MAX_HAND_GAP_METERS: f32 = 0.78;
    const FIST_ACK_MIN_CHEST_DISTANCE_METERS: f32 = 0.10;
    const FIST_ACK_MAX_CHEST_DISTANCE_METERS: f32 = 1.05;
    const DESCRIPTOR_MAX_HEIGHT_METERS: f32 = 2.00;
    const DESCRIPTOR_MIN_HEIGHT_METERS: f32 = 0.08;
    const DESCRIPTOR_CELL_FOOTPRINT: f32 = 0.62;
    const SHOW_LOCAL_DESCRIPTOR_DEBUG: bool = false;

    pub fn status_text(&self) -> &str {
        self.diagnostics.status_text()
    }

    pub fn connected_peer_count(&self) -> usize {
        self.runtime.registry.len()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn current_activity(&self) -> Option<XrActivityId> {
        Some(self.runtime.accepted_activity?.activity_id)
    }

    pub fn spawnable_activity(&self) -> Option<XrActivityId> {
        self.runtime.shared_objects.activity_id()
    }

    pub fn shared_object_count(&self) -> usize {
        self.runtime.shared_objects.len()
    }

    pub fn network_status_text(&self) -> &str {
        self.diagnostics.network_status_text()
    }

    pub fn alignment_debug_text(&self) -> &str {
        self.diagnostics.alignment_debug_text()
    }

    pub fn alignment_state_text(&self) -> &str {
        self.diagnostics.alignment_state_text()
    }

    pub fn peer_scene_text(&self) -> &str {
        self.diagnostics.peer_scene_text()
    }

    pub fn aligned_peer_height_map(&self) -> Option<XrDepthAlignHeightMap> {
        let (_, peer_state) = self.runtime.registry.preferred_peer()?;
        let transform = peer_state.descriptor_remote_to_local.or_else(|| {
            peer_state
                .last_solve_diagnostic
                .and_then(|diagnostic| diagnostic.best_solution)
                .map(|solution| solution.remote_to_local_transform())
        })?;
        let descriptor = peer_state.latest_descriptor?.descriptor;
        descriptor.transformed(&transform).height_map
    }

    pub fn raw_peer_alignment_descriptor(
        &self,
    ) -> Option<(XrNetPeerId, XrNetAlignmentDescriptorFrame)> {
        let (peer_id, peer_state) = self.runtime.registry.preferred_peer()?;
        Some((peer_id, peer_state.latest_descriptor?))
    }

    pub fn raw_peer_height_map(&self) -> Option<XrDepthAlignHeightMap> {
        let (_, descriptor) = self.raw_peer_alignment_descriptor()?;
        descriptor.descriptor.height_map
    }

    pub fn raw_alignment_dump_pair(&self) -> Option<XrNetAlignmentDescriptorDumpPair> {
        let local_descriptor = self.runtime.local.descriptor.clone()?;
        let (peer_id, remote_descriptor) = self.raw_peer_alignment_descriptor()?;
        if local_descriptor.descriptor.height_map.is_none()
            || remote_descriptor.descriptor.height_map.is_none()
        {
            return None;
        }
        Some(XrNetAlignmentDescriptorDumpPair::new(
            peer_id,
            local_descriptor,
            remote_descriptor,
        ))
    }

    pub fn local_slice_preview(&self) -> Option<XrDepthAlignSlicePreview> {
        self.runtime.local.slice_preview.clone()
    }

    pub fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) -> bool {
        if self.enabled == enabled {
            return self.enabled;
        }
        self.enabled = enabled;
        cx.xr_tsdf().set_surface_analysis_enabled(enabled);
        self.runtime = XrPeerSyncRuntime::default();
        self.diagnostics = XrPeerSyncDiagnostics::default();

        if enabled {
            if self.auto_alignment_enabled {
                self.runtime.alignment_worker = Some(XrPeopleAlignmentWorker::new(cx.xr_tsdf()));
            }
            self.ensure_net_node();
            self.diagnostics
                .set_enabled_defaults(self.auto_alignment_enabled, self.runtime.net_node.is_some());
        } else {
            self.diagnostics.set_disabled();
        }
        self.redraw(cx);
        self.enabled
    }

    pub fn set_local_activity(
        &mut self,
        _cx: &mut Cx,
        activity_id: XrActivityId,
    ) -> Option<XrNetActivityState> {
        if !self.enabled {
            return None;
        }
        self.ensure_net_node();
        let changed_at = if self.runtime.local.state_time != 0.0 {
            self.runtime.local.state_time
        } else {
            Cx::time_now()
        };
        let local_node_id = self.runtime.net_node.as_ref()?.node_id();
        if self.runtime.accepted_activity.is_some_and(|current| {
            current.activity_id == activity_id && current.changed_by == local_node_id
        }) {
            return self.runtime.accepted_activity;
        }
        let state = self
            .runtime
            .net_node
            .as_mut()?
            .send_activity(activity_id, changed_at);
        self.runtime.accepted_activity = Some(state);
        self.runtime.metrics.record_activity_tx(state);
        Some(state)
    }

    pub fn set_spawnable_objects<I>(&mut self, activity_id: XrActivityId, bindings: I) -> usize
    where
        I: IntoIterator<Item = XrSpawnableObjectBinding>,
    {
        self.runtime
            .shared_objects
            .replace_spawnables(activity_id, bindings);
        self.runtime.shared_objects.len()
    }

    pub fn send_local_body_spawn(
        &mut self,
        spawn: XrBodySpawn,
    ) -> Option<XrNetSharedObjectControl> {
        if !self.enabled {
            return None;
        }
        self.ensure_net_node();
        let activity_id = self.runtime.shared_objects.activity_id()?;
        let allocation = self
            .runtime
            .shared_objects
            .allocate_local_shared_object(activity_id, spawn.widget_uid)?;
        let authority = self.runtime.shared_objects.local_peer_id()?;
        let control = XrNetSharedObjectControl::XrSpawnObject {
            object_id: allocation.shared_object_id,
            epoch: 0,
            authority,
            fidelity: XrSharedObjectFidelity::ImpactCritical,
            shape: XrSharedObjectShape::ActivitySpawnable {
                activity_id,
                spawnable_id: allocation.spawnable_object_id,
            },
            pose: spawn.pose,
            linvel: spawn.linvel,
            angvel: spawn.angvel,
        };
        self.runtime
            .net_node
            .as_mut()?
            .send_shared_object_control(control.clone());
        self.runtime
            .metrics
            .record_body_spawn_tx(allocation.shared_object_id.0);
        Some(control)
    }

    fn ensure_net_node(&mut self) {
        if self.runtime.net_node.is_some() {
            return;
        }
        match XrNetNode::new() {
            Ok(node) => {
                self.runtime
                    .shared_objects
                    .set_local_peer_id(node.shared_object_peer_id());
                self.runtime.net_node = Some(node);
                self.runtime.local.last_sent_descriptor_signature = None;
                self.runtime.local.last_sent_descriptor = None;
                self.runtime.local.last_sent_descriptor_at = None;
                self.runtime.metrics.record_node_started();
            }
            Err(err) => {
                self.diagnostics.set_network_bind_failed(&err.to_string());
            }
        }
    }

    fn refresh_from_local_state(&mut self, cx: &mut Cx, state: &XrState) {
        if !self.enabled {
            return;
        }
        self.ensure_net_node();
        self.runtime.local.state_time = state.time;
        self.runtime.local.anchor = state.anchor;
        self.runtime.local.sync_anchor = state.sync_anchor;
        self.runtime.local.fist_hold_anchor = Self::state_fist_preview_anchor(state);
        if let (Some(local_anchor), Some(override_anchor)) = (
            self.runtime.local.anchor,
            self.runtime.local.anchor_override,
        ) {
            if Self::anchors_match(local_anchor, override_anchor) {
                self.runtime.local.anchor_override = None;
            }
        }
        let effective_local_anchor = self.runtime.local.effective_anchor();
        let Some(net_node) = self.runtime.net_node.as_mut() else {
            return;
        };

        let mut broadcast_state = state.clone();
        broadcast_state.anchor = effective_local_anchor;
        net_node.send_state(broadcast_state);
        self.runtime.metrics.tx_state_count = self.runtime.metrics.tx_state_count.saturating_add(1);

        if !self.auto_alignment_enabled {
            self.runtime.local.descriptor = None;
            self.runtime.local.descriptor_version = None;
            self.runtime.local.slice_preview = None;
            self.runtime.local.last_sent_descriptor_signature = None;
            self.runtime.local.last_sent_descriptor = None;
            self.runtime.local.last_sent_descriptor_at = None;
            if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                worker.clear_local_descriptor();
            }
            self.refresh_peer_transforms(cx);
            return;
        }

        let next_snapshot = cx.xr_tsdf().latest_tsdf_snapshot();
        let next_signature = next_snapshot
            .as_ref()
            .map(|snapshot| (snapshot.generation, snapshot.update_sequence));
        let next_slice_preview = next_snapshot
            .as_ref()
            .and_then(|snapshot| XrDepthAlignSlicePreview::from_tsdf_snapshot(snapshot.as_ref()));
        let next_descriptor = next_snapshot.as_ref().and_then(|snapshot| {
            XrNetAlignmentDescriptorFrame::from_tsdf_snapshot(snapshot.as_ref(), state.time)
        });

        if let (Some(signature), Some(frame)) = (next_signature, next_descriptor) {
            let change_score = self
                .runtime
                .local
                .last_sent_descriptor
                .as_ref()
                .map(|previous| descriptor_change_score(previous, &frame.descriptor))
                .unwrap_or(1.0);
            let should_publish = self.runtime.local.last_sent_descriptor_signature
                != Some(signature)
                && (self.runtime.local.last_sent_descriptor.is_none()
                    || change_score * 100.0 >= Self::DESCRIPTOR_SEND_MIN_CHANGE_PERCENT);
            if should_publish {
                self.runtime.local.descriptor = Some(frame.clone());
                self.runtime.local.descriptor_version = Some(signature);
                self.runtime.local.slice_preview = next_slice_preview;
                if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                    worker.set_local_descriptor(frame.clone(), signature);
                }
                net_node.send_alignment_descriptor(frame);
                self.runtime.local.last_sent_descriptor_signature = Some(signature);
                self.runtime.local.last_sent_descriptor = self
                    .runtime
                    .local
                    .descriptor
                    .as_ref()
                    .map(|frame| frame.descriptor.clone());
                self.runtime.local.last_sent_descriptor_at = Some(state.time);
                self.runtime.metrics.tx_descriptor_count =
                    self.runtime.metrics.tx_descriptor_count.saturating_add(1);
            }
        } else {
            self.runtime.local.descriptor = None;
            self.runtime.local.descriptor_version = None;
            self.runtime.local.slice_preview = None;
            self.runtime.local.last_sent_descriptor_signature = None;
            self.runtime.local.last_sent_descriptor = None;
            self.runtime.local.last_sent_descriptor_at = None;
            if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                worker.clear_local_descriptor();
            }
        }
        self.refresh_peer_transforms(cx);
    }

    fn poll_network(&mut self, cx: &mut Cx) {
        if !self.enabled {
            return;
        }

        let mut disconnected = false;
        let mut received_message = false;
        loop {
            let result = match self.runtime.net_node.as_mut() {
                Some(net_node) => net_node.incoming_receiver.try_recv(),
                None => break,
            };
            match result {
                Ok(message) => {
                    received_message = true;
                    self.handle_network_message(cx, message);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if disconnected {
            self.runtime.net_node = None;
            self.runtime.accepted_activity = None;
            self.runtime.local.last_sent_descriptor_signature = None;
            self.runtime.local.last_sent_descriptor = None;
            self.runtime.local.last_sent_descriptor_at = None;
            self.diagnostics.set_network_disconnected();
        } else if received_message {
            self.refresh_peer_transforms(cx);
        }
    }

    fn handle_network_message(&mut self, cx: &mut Cx, message: XrNetIncoming) {
        match message {
            XrNetIncoming::Join { peer } => {
                self.runtime.metrics.record_join(peer.id);
                self.runtime.registry.track_join(peer);
            }
            XrNetIncoming::Leave { peer, .. } => {
                self.runtime.metrics.record_leave(peer.id);
                self.runtime.registry.track_leave(peer.id);
                if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                    worker.remove_peer(peer.id);
                }
            }
            XrNetIncoming::State { peer, frame } => {
                self.runtime.metrics.record_state(peer.id, frame.seq);
                self.runtime
                    .registry
                    .track_state(peer, frame, self.runtime.local.state_time);
            }
            XrNetIncoming::AlignmentDescriptor { peer, frame } => {
                self.runtime.metrics.record_descriptor(peer.id, frame.seq);
                self.runtime.registry.track_descriptor(peer, frame.clone());
                if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                    worker.set_peer_descriptor(peer, frame);
                }
            }
            XrNetIncoming::Activity { peer, control } => {
                let activity = control.state();
                self.runtime.metrics.record_activity_rx(peer.id, activity);
                if self
                    .runtime
                    .accepted_activity
                    .is_none_or(|current| activity.is_newer_than(&current))
                {
                    let activity_changed = self
                        .runtime
                        .accepted_activity
                        .is_none_or(|current| current != activity);
                    self.runtime.accepted_activity = Some(activity);
                    if activity_changed {
                        cx.widget_action(
                            self.widget_uid(),
                            XrPeerSyncAction::ActivityChanged(activity.activity_id),
                        );
                    }
                }
            }
            XrNetIncoming::BodySpawn { peer, spawn } => {
                self.runtime
                    .metrics
                    .record_body_spawn_rx(peer.id, spawn.object_id.0);
                let Some(widget_uid) = self
                    .runtime
                    .shared_objects
                    .resolve_widget_uid(spawn.activity_id, spawn.object_id)
                else {
                    return;
                };
                cx.widget_action(
                    self.widget_uid(),
                    XrPeerSyncAction::BodySpawn(XrBodySpawn {
                        widget_uid,
                        pose: spawn.pose,
                        linvel: spawn.linvel,
                        angvel: spawn.angvel,
                    }),
                );
            }
            XrNetIncoming::SharedObjectControl { peer, control } => match control {
                XrNetSharedObjectControl::XrSpawnObject {
                    object_id,
                    shape:
                        XrSharedObjectShape::ActivitySpawnable {
                            activity_id,
                            spawnable_id,
                        },
                    pose,
                    linvel,
                    angvel,
                    ..
                } => {
                    self.runtime
                        .metrics
                        .record_body_spawn_rx(peer.id, object_id.0);
                    let Some(widget_uid) = self.runtime.shared_objects.resolve_remote_widget_uid(
                        activity_id,
                        object_id,
                        spawnable_id,
                    ) else {
                        return;
                    };
                    cx.widget_action(
                        self.widget_uid(),
                        XrPeerSyncAction::BodySpawn(XrBodySpawn {
                            widget_uid,
                            pose,
                            linvel,
                            angvel,
                        }),
                    );
                }
                _ => {}
            },
            XrNetIncoming::Alignment { .. } => {}
        }
    }

    fn apply_alignment_results(&mut self, cx: &mut Cx) {
        let Some(worker) = self.runtime.alignment_worker.as_mut() else {
            return;
        };
        let Some(result) = worker.take_latest_result() else {
            return;
        };

        self.runtime
            .registry
            .apply_alignment_results(result.peer_results);
        self.diagnostics.alignment_debug_status = result.alignment_debug_text;
        self.refresh_peer_transforms(cx);
    }

    fn local_sync_status_text(&self) -> String {
        self.runtime
            .local
            .active_sync_anchor()
            .map(|sync| format!("armed {}", sync.id))
            .unwrap_or_else(|| "idle".to_string())
    }

    fn manual_peer_scene_text(&self) -> String {
        let Some((peer_id, peer_state)) = self.runtime.registry.preferred_peer() else {
            return "PeerScene: waiting for peer".to_string();
        };
        let state_text = if peer_state.latest_state.is_some() {
            "yes"
        } else {
            "no"
        };
        let anchor_text = peer_state
            .latest_state
            .as_ref()
            .and_then(|state| state.state.anchor)
            .map(|_| "yes")
            .unwrap_or("no");
        let sync_text = peer_state
            .latest_state
            .as_ref()
            .and_then(|state| state.state.sync_anchor)
            .filter(|_| {
                peer_state.last_sync_anchor_seen_at.is_some_and(|seen_at| {
                    self.runtime.local.state_time - seen_at
                        <= Self::SYNC_MATCH_RECEIVE_WINDOW_SECONDS
                })
            })
            .map(|sync| format!("yes {}", sync.id))
            .unwrap_or_else(|| "no".to_string());
        format!(
            "PeerScene {:08x}: state {} | anchor {} | sync {} | pose {}",
            peer_id.0,
            state_text,
            anchor_text,
            sync_text,
            transform_source_label(peer_state.transform_source),
        )
    }

    fn manual_alignment_state_text(&self) -> String {
        let local_anchor_text = if self.runtime.local.effective_anchor().is_some() {
            "yes"
        } else {
            "no"
        };
        let local_sync_text = self.local_sync_status_text();
        let Some((peer_id, peer_state)) = self.runtime.registry.preferred_peer() else {
            return format!(
                "AlignState: local anchor {} | sync {} | waiting for peer",
                local_anchor_text, local_sync_text
            );
        };
        let remote_anchor_text = peer_state
            .latest_state
            .as_ref()
            .and_then(|state| state.state.anchor)
            .map(|_| "yes")
            .unwrap_or("no");
        let remote_sync_text = peer_state
            .latest_state
            .as_ref()
            .and_then(|state| state.state.sync_anchor)
            .filter(|_| {
                peer_state.last_sync_anchor_seen_at.is_some_and(|seen_at| {
                    self.runtime.local.state_time - seen_at
                        <= Self::SYNC_MATCH_RECEIVE_WINDOW_SECONDS
                })
            })
            .map(|sync| format!("armed {}", sync.id))
            .unwrap_or_else(|| "idle".to_string());
        format!(
            "AlignState {:08x}: local anchor {} | local sync {} | remote anchor {} | remote sync {} | pose {}",
            peer_id.0,
            local_anchor_text,
            local_sync_text,
            remote_anchor_text,
            remote_sync_text,
            transform_source_label(peer_state.transform_source),
        )
    }

    fn manual_alignment_debug_text(&self) -> String {
        let Some((peer_id, peer_state)) = self.runtime.registry.preferred_peer() else {
            return match (
                self.runtime.local.active_sync_anchor(),
                self.runtime.local.fist_hold_anchor,
            ) {
                (Some(sync), _) => format!(
                    "AlignDbg: local sync {} armed | waiting for peer fistbump",
                    sync.id
                ),
                (None, Some(_)) => "AlignDbg: local fists ready".to_string(),
                _ => "AlignDbg: manual sync idle".to_string(),
            };
        };
        if peer_state.transform_source == RemoteTransformSource::Anchor {
            if peer_state
                .latest_state
                .as_ref()
                .is_some_and(|state| state.state.anchor.is_some())
            {
                return format!("AlignDbg {:08x}: using saved anchors", peer_id.0);
            }
            if let Some(sync) = self.runtime.local.active_sync_anchor() {
                return format!(
                    "AlignDbg {:08x}: sync matched local {} -> persistent anchor requested",
                    peer_id.0, sync.id
                );
            }
            return format!("AlignDbg {:08x}: anchor transform active", peer_id.0);
        }
        if let Some(sync) = peer_state
            .latest_state
            .as_ref()
            .and_then(|state| state.state.sync_anchor)
        {
            return format!(
                "AlignDbg {:08x}: remote sync {} seen | hold both fists or perform local bump",
                peer_id.0, sync.id
            );
        }
        if peer_state
            .latest_state
            .as_ref()
            .and_then(|state| Self::state_fist_ack_anchor(&state.state))
            .is_some()
        {
            return format!(
                "AlignDbg {:08x}: remote fists ready | perform local bump",
                peer_id.0
            );
        }
        if let Some(sync) = self.runtime.local.active_sync_anchor() {
            return format!(
                "AlignDbg {:08x}: local sync {} armed | waiting for remote sync or fists",
                peer_id.0, sync.id
            );
        }
        if self.runtime.local.fist_hold_anchor.is_some() {
            return format!("AlignDbg {:08x}: local fists ready", peer_id.0);
        }
        format!("AlignDbg {:08x}: manual sync idle", peer_id.0)
    }

    fn refresh_status(&mut self) {
        if !self.enabled {
            self.diagnostics.set_disabled();
            return;
        }
        if self.runtime.net_node.is_none() {
            if self.diagnostics.status.is_empty() {
                self.diagnostics.status = "AlignSync: network unavailable".to_string();
            }
            if self.diagnostics.network_status.is_empty() {
                self.diagnostics.network_status = "Network: unavailable".to_string();
            }
            if self.diagnostics.peer_scene_status.is_empty() {
                self.diagnostics.peer_scene_status = "PeerMap: network unavailable".to_string();
            }
            return;
        }

        let peer_count = self.runtime.registry.len();
        let visible_count = self
            .runtime
            .registry
            .peers
            .values()
            .filter(|peer| peer.latest_state.is_some())
            .count();
        let descriptor_count = self
            .runtime
            .registry
            .peers
            .values()
            .filter(|peer| peer.has_descriptor)
            .count();
        let aligned_count = self
            .runtime
            .registry
            .peers
            .values()
            .filter(|peer| peer.latest_state.is_some() && peer.remote_to_local.is_some())
            .count();
        let anchor_aligned_count = self
            .runtime
            .registry
            .peers
            .values()
            .filter(|peer| {
                peer.latest_state.is_some()
                    && peer.transform_source == RemoteTransformSource::Anchor
            })
            .count();
        let local_scene_state = self.runtime.local.scene_state();

        if !self.auto_alignment_enabled {
            self.diagnostics.status = if peer_count == 0 {
                "Peers: scanning LAN for clients".to_string()
            } else {
                format!(
                    "Peers: {peer_count} seen | {visible_count} state | {anchor_aligned_count} anchor-aligned"
                )
            };
            let last_event = self.runtime.metrics.last_event_label();
            self.diagnostics.network_status = format!(
                "Network: tx s{} d{} a{} b{} | rx j{} l{} s{} d{} a{} b{} | peers {} vis {} anchor {} | local anchor {} sync {} | objects {} | last {}",
                self.runtime.metrics.tx_state_count,
                self.runtime.metrics.tx_descriptor_count,
                self.runtime.metrics.tx_activity_count,
                self.runtime.metrics.tx_body_spawn_count,
                self.runtime.metrics.rx_join_count,
                self.runtime.metrics.rx_leave_count,
                self.runtime.metrics.rx_state_count,
                self.runtime.metrics.rx_descriptor_count,
                self.runtime.metrics.rx_activity_count,
                self.runtime.metrics.rx_body_spawn_count,
                peer_count,
                visible_count,
                anchor_aligned_count,
                if self.runtime.local.effective_anchor().is_some() {
                    "yes"
                } else {
                    "no"
                },
                self.local_sync_status_text(),
                self.runtime.shared_objects.len(),
                last_event,
            );
            self.diagnostics.peer_scene_status = self.manual_peer_scene_text();
            self.diagnostics.alignment_state_status = self.manual_alignment_state_text();
            self.diagnostics.alignment_debug_status = self.manual_alignment_debug_text();
            return;
        }

        self.diagnostics.status = if peer_count == 0 {
            "AlignSync: waiting for peer heightmap".to_string()
        } else if local_scene_state == LocalSceneState::Ready {
            format!(
                "AlignSync: peers {peer_count} | visible {visible_count} | remote maps {descriptor_count} | solved {aligned_count}"
            )
        } else if local_scene_state == LocalSceneState::PublishPending {
            format!(
                "AlignSync: peers {peer_count} | local map signal {} ready | publish pending",
                self.runtime.local.contour_sample_count()
            )
        } else {
            format!("AlignSync: peers {peer_count} | waiting for local heightmap")
        };

        let last_event = self.runtime.metrics.last_event_label();
        self.diagnostics.network_status = format!(
            "Network: tx state {} map {} activity {} spawn {} | rx join {} leave {} state {} map {} activity {} spawn {} | peers {} vis {} maps {} solved {} | local map {} signal {} | objects {} | last {}",
            self.runtime.metrics.tx_state_count,
            self.runtime.metrics.tx_descriptor_count,
            self.runtime.metrics.tx_activity_count,
            self.runtime.metrics.tx_body_spawn_count,
            self.runtime.metrics.rx_join_count,
            self.runtime.metrics.rx_leave_count,
            self.runtime.metrics.rx_state_count,
            self.runtime.metrics.rx_descriptor_count,
            self.runtime.metrics.rx_activity_count,
            self.runtime.metrics.rx_body_spawn_count,
            peer_count,
            visible_count,
            descriptor_count,
            aligned_count,
            match local_scene_state {
                LocalSceneState::Ready => "yes",
                LocalSceneState::PublishPending => "pending",
                LocalSceneState::Missing => "no",
            },
            self.runtime.local.contour_sample_count(),
            self.runtime.shared_objects.len(),
            last_event,
        );
        self.diagnostics.peer_scene_status = make_peer_scene_debug_text(
            local_scene_state == LocalSceneState::Ready,
            &self.runtime.registry.peers,
        );
        self.diagnostics.alignment_state_status = make_alignment_state_text(
            local_scene_state,
            self.runtime.local.descriptor_version,
            &self.runtime.registry.peers,
        );
        let has_alignment_diagnostic = self
            .runtime
            .registry
            .peers
            .values()
            .any(|peer| peer.last_solve_diagnostic.is_some());
        let has_alignment_worker_progress = self
            .runtime
            .registry
            .peers
            .values()
            .any(|peer| peer.worker_progress.is_some());
        if local_scene_state != LocalSceneState::Ready
            || (!has_alignment_diagnostic && !has_alignment_worker_progress)
        {
            let local_descriptor_text = self.local_descriptor_debug_text();
            self.diagnostics.alignment_debug_status = if local_scene_state == LocalSceneState::Ready
            {
                make_pending_alignment_debug_text(
                    &local_descriptor_text,
                    &self.runtime.registry.peers,
                )
            } else {
                local_descriptor_text
            };
        }
    }

    fn peer_base_color(peer_id: XrNetPeerId) -> Vec4f {
        match (peer_id.0 % 5) as usize {
            0 => vec4f(0.92, 0.38, 0.31, 1.0),
            1 => vec4f(0.24, 0.74, 0.58, 1.0),
            2 => vec4f(0.35, 0.58, 0.98, 1.0),
            3 => vec4f(0.93, 0.70, 0.28, 1.0),
            _ => vec4f(0.80, 0.48, 0.94, 1.0),
        }
    }

    fn peer_label(peer_id: XrNetPeerId) -> String {
        format!("{:08x}", peer_id.0)
    }

    fn peer_transform(peer: &RemotePeerState) -> Mat4f {
        peer.remote_to_local.unwrap_or_default()
    }

    fn peer_alpha(peer: &RemotePeerState) -> f32 {
        match peer.transform_source {
            RemoteTransformSource::Anchor => 1.0,
            RemoteTransformSource::Descriptor => 1.0,
            RemoteTransformSource::Raw => 0.42,
        }
    }

    fn flat_forward(orientation: Quat) -> Vec3f {
        let mut forward = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-6 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        }
    }

    fn hand_is_closed_fist(hand: &XrHand, is_left: bool) -> bool {
        hand_is_palm_down_closed_fist(hand, is_left, CLOSED_FIST_GESTURE)
    }

    fn hand_fist_anchor_point(hand: &XrHand, forward: Vec3f, is_left: bool) -> Option<Vec3f> {
        if !Self::hand_is_closed_fist(hand, is_left) {
            return None;
        }
        let mut best_point = None;
        let mut best_projection = f32::NEG_INFINITY;
        for joint_index in [
            XrHand::INDEX_KNUCKLE3,
            XrHand::MIDDLE_KNUCKLE3,
            XrHand::RING_KNUCKLE3,
            XrHand::LITTLE_KNUCKLE3,
        ] {
            let point = hand.joints[joint_index].position;
            let projection = point.dot(forward);
            if projection > best_projection {
                best_projection = projection;
                best_point = Some(point);
            }
        }
        best_point
    }

    fn state_fist_preview_anchor(state: &XrState) -> Option<XrAnchor> {
        let forward = Self::flat_forward(state.head_pose.orientation);
        let left_point = Self::hand_fist_anchor_point(&state.left_hand, forward, true)?;
        let right_point = Self::hand_fist_anchor_point(&state.right_hand, forward, false)?;
        Some(XrAnchor {
            left: left_point,
            right: right_point,
        })
    }

    fn state_fist_ack_anchor(state: &XrState) -> Option<XrAnchor> {
        let preview = Self::state_fist_preview_anchor(state)?;
        let forward = Self::flat_forward(state.head_pose.orientation);
        let mut right = state
            .head_pose
            .orientation
            .rotate_vec3(&vec3f(1.0, 0.0, 0.0));
        right.y = 0.0;
        right = if right.length() <= 1.0e-6 {
            vec3f(1.0, 0.0, 0.0)
        } else {
            right.normalize()
        };

        let left_point = preview.left;
        let right_point = preview.right;
        let left_local = left_point - state.head_pose.position;
        let right_local = right_point - state.head_pose.position;
        let left_forward = left_local.dot(forward);
        let right_forward = right_local.dot(forward);
        let left_lateral = left_local.dot(right);
        let right_lateral = right_local.dot(right);
        let hand_gap = (right_point - left_point).length();
        if left_lateral >= right_lateral
            || (left_point.y - right_point.y).abs() > Self::FIST_ACK_MAX_VERTICAL_DELTA_METERS
            || (left_forward - right_forward).abs() > Self::FIST_ACK_MAX_DEPTH_DELTA_METERS
            || hand_gap < Self::FIST_ACK_MIN_HAND_GAP_METERS
            || hand_gap > Self::FIST_ACK_MAX_HAND_GAP_METERS
        {
            return None;
        }
        let forward_distance = (left_forward + right_forward) * 0.5;
        if !(Self::FIST_ACK_MIN_CHEST_DISTANCE_METERS..=Self::FIST_ACK_MAX_CHEST_DISTANCE_METERS)
            .contains(&forward_distance)
        {
            return None;
        }
        Some(XrAnchor {
            left: left_point,
            right: right_point,
        })
    }

    fn anchors_match(left: XrAnchor, right: XrAnchor) -> bool {
        (left.left - right.left).length() <= 0.025 && (left.right - right.right).length() <= 0.025
    }

    fn refresh_peer_transforms(&mut self, cx: &mut Cx) {
        if let (Some(local_anchor), Some(override_anchor)) = (
            self.runtime.local.anchor,
            self.runtime.local.anchor_override,
        ) {
            if Self::anchors_match(local_anchor, override_anchor) {
                self.runtime.local.anchor_override = None;
            }
        }

        let local_anchor = self.runtime.local.effective_anchor();
        let local_sync_anchor = self.runtime.local.active_sync_anchor();
        let local_fist_hold_anchor = self.runtime.local.fist_hold_anchor;
        let now = self.runtime.local.state_time;
        let changed = {
            let (registry, local) = (&mut self.runtime.registry, &mut self.runtime.local);
            registry.refresh_transforms(
                cx,
                local_anchor,
                local_sync_anchor,
                local_fist_hold_anchor,
                &mut local.anchor_override,
                now,
            )
        };

        if changed {
            self.redraw(cx);
        }
    }

    fn peer_marker_anchor(peer: &RemotePeerState) -> Option<XrAnchor> {
        peer.latest_state
            .as_ref()
            .and_then(|state| state.state.anchor)
            .or_else(|| {
                peer.latest_state
                    .as_ref()
                    .and_then(|state| state.state.sync_anchor.map(|sync| sync.anchor))
            })
    }

    fn draw_cube_at(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        pose_transform: &Mat4f,
        size: Vec3f,
        color: Vec4f,
    ) {
        self.draw_cube.transform = Mat4f::mul(world, pose_transform);
        self.draw_cube.cube_pos = vec3(0.0, 0.0, 0.0);
        self.draw_cube.cube_size = size;
        self.draw_cube.color = color;
        self.draw_cube.depth_clip = 1.0;
        self.draw_cube.draw(cx);
    }

    fn transform_point(transform: &Mat4f, point: Vec3f) -> Vec3f {
        let point = transform.transform_vec4(vec4f(point.x, point.y, point.z, 1.0));
        if point.w.abs() > 1.0e-6 {
            vec3f(point.x / point.w, point.y / point.w, point.z / point.w)
        } else {
            point.to_vec3f()
        }
    }

    fn local_descriptor_debug_text(&self) -> String {
        let Some(descriptor) = self
            .runtime
            .local
            .descriptor
            .as_ref()
            .map(|frame| &frame.descriptor)
        else {
            return match self.runtime.local.scene_state() {
                LocalSceneState::Missing => "AlignDbg: waiting for local heightmap".to_string(),
                LocalSceneState::PublishPending => {
                    "AlignDbg: local heightmap ready | publish pending".to_string()
                }
                LocalSceneState::Ready => "AlignDbg: local map missing".to_string(),
            };
        };
        let map_status = descriptor_height_map_status(descriptor);
        match self.runtime.local.scene_state() {
            LocalSceneState::Missing => "AlignDbg: waiting for local heightmap".to_string(),
            LocalSceneState::PublishPending => format!(
                "AlignDbg: local map {} | signal {} | publish pending",
                map_status,
                self.runtime.local.contour_sample_count(),
            ),
            LocalSceneState::Ready => format!(
                "AlignDbg: local map {} | signal {}",
                map_status,
                self.runtime.local.contour_sample_count(),
            ),
        }
    }

    fn descriptor_height_meters(height_u8: u8) -> f32 {
        if height_u8 == 0 {
            Self::DESCRIPTOR_MIN_HEIGHT_METERS
        } else {
            ((height_u8 as f32 / 255.0) * Self::DESCRIPTOR_MAX_HEIGHT_METERS).clamp(
                Self::DESCRIPTOR_MIN_HEIGHT_METERS,
                Self::DESCRIPTOR_MAX_HEIGHT_METERS,
            )
        }
    }

    fn draw_local_descriptor(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let Some(vertical) = self
            .runtime
            .local
            .descriptor
            .as_ref()
            .and_then(|frame| frame.descriptor.vertical_descriptor.as_ref())
            .cloned()
        else {
            return;
        };
        let size = vertical.size as usize;
        if size == 0
            || vertical.vertical_surface_masks.len() != size * size
            || vertical.clutter_surface_masks.len() != size * size
            || vertical.height_u8.len() != size * size
        {
            return;
        }

        let cell_size = vertical.cell_size_meters;
        let footprint = cell_size * Self::DESCRIPTOR_CELL_FOOTPRINT;
        for z in 0..size {
            for x in 0..size {
                let index = x + z * size;
                let vertical_count = vertical.vertical_surface_masks[index].count_ones() as f32;
                let clutter_count = vertical.clutter_surface_masks[index].count_ones() as f32;
                if vertical_count <= 0.0 && clutter_count <= 0.0 {
                    continue;
                }
                let center_x = vertical.origin_x + (x as f32 + 0.5) * cell_size;
                let center_z = vertical.origin_z + (z as f32 + 0.5) * cell_size;
                let height = Self::descriptor_height_meters(vertical.height_u8[index]);
                let weight = (vertical_count + clutter_count).max(1.0);
                let vertical_mix = vertical_count / weight;
                let clutter_mix = clutter_count / weight;
                let alpha = (0.16 + 0.06 * weight.min(4.0)).clamp(0.16, 0.40);
                let color = vec4f(
                    0.14 + 0.18 * clutter_mix,
                    0.42 + 0.36 * clutter_mix,
                    0.96 - 0.48 * clutter_mix,
                    alpha,
                );
                let transform =
                    Pose::new(Quat::default(), vec3(center_x, height * 0.5, center_z)).to_mat4();
                self.draw_cube_at(
                    cx,
                    world,
                    &transform,
                    vec3(footprint, height, footprint),
                    vec4f(
                        color.x + 0.06 * vertical_mix,
                        color.y,
                        color.z + 0.04 * vertical_mix,
                        color.w,
                    ),
                );
            }
        }
    }

    fn draw_anchor_markers(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        anchor: XrAnchor,
        size: f32,
        left_color: Vec4f,
        right_color: Vec4f,
    ) {
        let left_transform = Pose::new(anchor.to_quat(), anchor.left).to_mat4();
        self.draw_cube_at(
            cx,
            world,
            &left_transform,
            vec3(size, size, size),
            left_color,
        );
        let right_transform = Pose::new(anchor.to_quat_rev(), anchor.right).to_mat4();
        self.draw_cube_at(
            cx,
            world,
            &right_transform,
            vec3(size, size, size),
            right_color,
        );
    }

    fn draw_local_anchor_markers(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let Some(anchor) = self.runtime.local.effective_anchor() else {
            return;
        };
        self.draw_anchor_markers(
            cx,
            world,
            anchor,
            Self::ANCHOR_MARKER_SIZE,
            vec4f(1.0, 0.15, 0.10, 0.96),
            vec4f(0.18, 0.46, 1.0, 0.96),
        );
    }

    fn draw_local_pending_sync_markers(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        if let Some(sync_anchor) = self.runtime.local.active_sync_anchor() {
            self.draw_anchor_markers(
                cx,
                world,
                sync_anchor.anchor,
                Self::ANCHOR_MARKER_SIZE * 0.82,
                vec4f(1.0, 0.48, 0.38, 0.82),
                vec4f(0.46, 0.72, 1.0, 0.82),
            );
            return;
        }
        let Some(preview_anchor) = self.runtime.local.fist_hold_anchor else {
            return;
        };
        self.draw_anchor_markers(
            cx,
            world,
            preview_anchor,
            Self::ANCHOR_MARKER_SIZE * 0.72,
            vec4f(1.0, 0.62, 0.24, 0.62),
            vec4f(0.36, 0.78, 1.0, 0.62),
        );
    }

    fn draw_remote_anchor_markers(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let peer_ids = self.runtime.registry.peer_ids();
        for peer_id in peer_ids {
            let Some(peer) = self.runtime.registry.peers.get(&peer_id).cloned() else {
                continue;
            };
            let Some(remote_to_local) = peer.remote_to_local else {
                continue;
            };
            let Some(remote_anchor) = Self::peer_marker_anchor(&peer) else {
                continue;
            };
            let solved_anchor = XrAnchor {
                left: Self::transform_point(&remote_to_local, remote_anchor.left),
                right: Self::transform_point(&remote_to_local, remote_anchor.right),
            };
            let alpha = if peer.transform_source == RemoteTransformSource::Anchor {
                0.98
            } else {
                0.55
            };
            self.draw_anchor_markers(
                cx,
                world,
                solved_anchor,
                Self::REMOTE_ANCHOR_MARKER_SIZE,
                vec4f(1.0, 0.44, 0.34, alpha),
                vec4f(0.42, 0.68, 1.0, alpha),
            );
        }
    }

    fn draw_peer_hand(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        root_transform: &Mat4f,
        hand: &XrHand,
        controller: &XrController,
        color: Vec4f,
    ) {
        let pose = if hand.in_view() {
            Some(hand.joints[XrHand::CENTER])
        } else if controller.active() {
            Some(controller.grip_pose)
        } else {
            None
        };
        let Some(pose) = pose else {
            return;
        };
        let transform = Mat4f::mul(root_transform, &pose.to_mat4());
        self.draw_cube_at(cx, world, &transform, Self::HAND_SIZE, color);
    }

    fn draw_remote_peers(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let peer_ids = self.runtime.registry.peer_ids();
        for peer_id in peer_ids {
            let Some(peer) = self.runtime.registry.peers.get(&peer_id).cloned() else {
                continue;
            };
            let Some(state_frame) = peer.latest_state.as_ref() else {
                continue;
            };

            let alpha = Self::peer_alpha(&peer);
            let base = Self::peer_base_color(peer.peer.id);
            let root_transform = Self::peer_transform(&peer);
            let head_color = vec4f(base.x, base.y, base.z, alpha);
            let left_color = vec4f(
                (base.x * 0.72).min(1.0),
                (base.y * 1.05).min(1.0),
                1.0,
                alpha,
            );
            let right_color = vec4f(
                1.0,
                (base.y * 0.82).min(1.0),
                (base.z * 0.72).min(1.0),
                alpha,
            );

            let head_transform =
                Mat4f::mul(&root_transform, &state_frame.state.head_pose.to_mat4());
            self.draw_cube_at(cx, world, &head_transform, Self::HEADSET_SIZE, head_color);
            self.draw_peer_hand(
                cx,
                world,
                &root_transform,
                &state_frame.state.left_hand,
                &state_frame.state.left_controller,
                left_color,
            );
            self.draw_peer_hand(
                cx,
                world,
                &root_transform,
                &state_frame.state.right_hand,
                &state_frame.state.right_controller,
                right_color,
            );
        }
    }
}

impl Widget for XrPeerSync {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_enabled) {
            let mut enabled = self.enabled;
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                enabled = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            let enabled = vm.with_cx_mut(|cx| self.set_enabled(cx, enabled));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(toggle_enabled) || method == live_id!(toggle_test) {
            let enabled = vm.with_cx_mut(|cx| self.set_enabled(cx, !self.enabled));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(enabled) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.enabled));
        }
        if method == live_id!(set_auto_alignment_enabled) {
            let mut enabled = self.auto_alignment_enabled;
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                enabled = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            let restart = self.auto_alignment_enabled != enabled && self.enabled;
            self.auto_alignment_enabled = enabled;
            vm.with_cx_mut(|cx| {
                if restart {
                    self.set_enabled(cx, false);
                    self.set_enabled(cx, true);
                } else {
                    cx.xr_tsdf().set_surface_analysis_enabled(self.enabled);
                }
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(auto_alignment_enabled) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.auto_alignment_enabled));
        }
        self.node.script_call(vm, method, args)
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        self.node.script_result(vm, id, result);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.enabled {
            if let Event::XrUpdate(update) = event {
                self.refresh_from_local_state(cx, update.state.as_ref());
            }
            self.poll_network(cx);
            self.apply_alignment_results(cx);
            self.refresh_status();
        }
        self.node.handle_event(cx, event, scope);
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.enabled {
            return self.node.draw_3d(cx, scope);
        }
        if cx.scene_state_3d().is_none() {
            return DrawStep::done();
        }
        let world = if cx.cx.in_xr_mode() {
            // Depth mesh and remote peers live in raw OpenXR local-space.
            // Keep debug geometry in that same space so
            // root/content recentering does not skew or shift them.
            Mat4f::identity()
        } else {
            self.node.local_transform()
        };
        self.draw_cube.begin_many_instances(cx);
        self.draw_local_pending_sync_markers(cx, &world);
        self.draw_local_anchor_markers(cx, &world);
        self.draw_remote_anchor_markers(cx, &world);
        if self.auto_alignment_enabled && Self::SHOW_LOCAL_DESCRIPTOR_DEBUG {
            self.draw_local_descriptor(cx, &world);
        }
        self.draw_remote_peers(cx, &world);
        self.draw_cube.end_many_instances(cx);
        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_solution(
        yaw_radians: f32,
        translation: Vec3f,
        confidence: f32,
        residual_meters: f32,
        matched_samples: usize,
    ) -> XrDepthAlignSolution {
        XrDepthAlignSolution {
            yaw_radians,
            translation,
            confidence,
            symmetry_confidence: 1.0,
            residual_meters,
            matched_samples,
        }
    }

    fn make_diagnostic(solution: XrDepthAlignSolution) -> XrDepthAlignSolveDiagnostic {
        XrDepthAlignSolveDiagnostic {
            local_wall_samples: 8,
            remote_wall_samples: 8,
            best_solution: Some(solution),
            ..XrDepthAlignSolveDiagnostic::default()
        }
    }

    #[test]
    fn stable_alignment_prefers_existing_solution_over_flip() {
        let previous = make_solution(0.42, vec3(0.28, 0.0, -0.64), 0.41, 0.03, 8);
        let flipped = make_solution(-2.71, vec3(-0.34, 0.0, 0.71), 0.44, 0.03, 8);

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(previous),
            &make_diagnostic(flipped),
        )
        .unwrap();

        assert_eq!(chosen, previous);
    }

    #[test]
    fn stable_alignment_accepts_small_refinement() {
        let previous = make_solution(0.42, vec3(0.28, 0.0, -0.64), 0.28, 0.06, 6);
        let refined = make_solution(0.46, vec3(0.24, 0.0, -0.60), 0.35, 0.03, 8);

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(previous),
            &make_diagnostic(refined),
        )
        .unwrap();

        assert_eq!(chosen, refined);
    }

    #[test]
    fn stable_alignment_holds_stronger_solution_over_weaker_reacquisition() {
        let previous = XrDepthAlignSolution {
            yaw_radians: 0.38,
            translation: vec3(0.18, 0.0, -0.54),
            confidence: 0.74,
            symmetry_confidence: 0.82,
            residual_meters: 0.03,
            matched_samples: 8,
        };
        let weaker = XrDepthAlignSolution {
            yaw_radians: 0.52,
            translation: vec3(0.34, 0.0, -0.30),
            confidence: 0.59,
            symmetry_confidence: 0.21,
            residual_meters: 0.08,
            matched_samples: 2,
        };
        let mut weaker_diag = make_diagnostic(weaker);
        weaker_diag.local_wall_samples = 2;
        weaker_diag.remote_wall_samples = 2;

        let chosen =
            choose_stable_alignment_solution(Some(previous), Some(previous), &weaker_diag).unwrap();

        assert_eq!(chosen, previous);
    }

    #[test]
    fn stable_alignment_switches_when_previous_pose_no_longer_scores_on_current_descriptor() {
        let previous = make_solution(-0.41, vec3(0.58, 0.0, -0.44), 0.42, 0.03, 8);
        let candidate = make_solution(1.18, vec3(-0.62, 0.0, 0.71), 0.39, 0.03, 8);
        let stale_on_current = XrDepthAlignSolution {
            yaw_radians: previous.yaw_radians,
            translation: previous.translation,
            confidence: 0.06,
            symmetry_confidence: 0.01,
            residual_meters: 0.21,
            matched_samples: 1,
        };

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(stale_on_current),
            &make_diagnostic(candidate),
        )
        .unwrap();

        assert_eq!(chosen, candidate);
    }

    #[test]
    fn stable_alignment_clears_previous_when_current_descriptor_no_longer_supports_it() {
        let previous = make_solution(-0.41, vec3(0.58, 0.0, -0.44), 0.42, 0.03, 8);
        let stale_on_current = XrDepthAlignSolution {
            yaw_radians: previous.yaw_radians,
            translation: previous.translation,
            confidence: 0.05,
            symmetry_confidence: 0.0,
            residual_meters: 0.24,
            matched_samples: 0,
        };
        let rejected = XrDepthAlignSolveDiagnostic {
            local_wall_samples: 4,
            remote_wall_samples: 4,
            local_vertical_descriptor: true,
            remote_vertical_descriptor: true,
            best_solution: Some(XrDepthAlignSolution {
                yaw_radians: 1.18,
                translation: vec3(-0.62, 0.0, 0.71),
                confidence: 0.10,
                symmetry_confidence: 0.03,
                residual_meters: 0.18,
                matched_samples: 1,
            }),
            ..XrDepthAlignSolveDiagnostic::default()
        };

        let chosen =
            choose_stable_alignment_solution(Some(previous), Some(stale_on_current), &rejected);

        assert_eq!(chosen, None);
    }

    fn make_peer(peer_id: u64) -> XrNetPeer {
        XrNetPeer {
            id: XrNetPeerId(peer_id),
            addr: "127.0.0.1:41547".parse().unwrap(),
        }
    }

    fn make_peer_descriptor(
        wall_count: usize,
        has_vertical_descriptor: bool,
    ) -> XrNetAlignmentDescriptorFrame {
        let samples = (0..wall_count)
            .map(|index| XrDepthAlignSample {
                kind: XrDepthAlignSampleKind::Wall,
                point: vec3(index as f32 * 0.2, 0.0, 0.0),
                normal: vec3(1.0, 0.0, 0.0),
                weight: 1.0,
            })
            .collect();
        XrNetAlignmentDescriptorFrame {
            seq: 7,
            sent_at: 1.0,
            descriptor: XrDepthAlignDescriptor {
                samples,
                vertical_descriptor: has_vertical_descriptor.then_some(
                    XrDepthAlignVerticalDescriptor {
                        origin_x: 0.0,
                        origin_z: 0.0,
                        cell_size_meters: 0.25,
                        size: 1,
                        vertical_surface_masks: vec![1],
                        clutter_surface_masks: vec![0],
                        free_space_masks: vec![0],
                        height_u8: vec![128],
                    },
                ),
                ..XrDepthAlignDescriptor::default()
            },
        }
    }

    fn reference_dump_pair() -> XrNetAlignmentDescriptorDumpPair {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("dump/dumps/align-pair-226a39e4b300-r0097-1774792873191.bin");
        let bytes = std::fs::read(path).expect("reference dump should exist");
        XrNetAlignmentDescriptorDumpPair::from_file_bytes(&bytes)
            .expect("reference dump should decode")
    }

    #[test]
    fn worker_queues_new_local_descriptor_without_interrupting_active_solver() {
        let pair = reference_dump_pair();
        let peer = XrNetPeer {
            id: pair.remote_peer_id,
            addr: "127.0.0.1:41547".parse().unwrap(),
        };
        let mut state = AlignmentWorkerState::default();
        let mut updated_local = pair.local_descriptor.clone();
        let updated_height = updated_local
            .descriptor
            .height_map
            .as_mut()
            .and_then(|height_map| {
                height_map
                    .heights_meters
                    .iter_mut()
                    .find(|height| height.is_finite())
            })
            .expect("reference dump should contain a finite local height sample");
        *updated_height += 0.06;

        state.apply_local_descriptor_update(PendingLocalDescriptorUpdate::Set {
            frame: pair.local_descriptor.clone(),
            version: (1, 0),
        });
        assert!(state.apply_peer_update(
            peer.id,
            PendingPeerDescriptorUpdate::Set {
                peer,
                frame: pair.remote_descriptor.clone(),
            },
        ));

        let peer_state = state.peers.get(&peer.id).unwrap();
        assert!(peer_state.matcher.is_some());
        assert_eq!(peer_state.active_local_descriptor_version, Some((1, 0)));
        assert_eq!(
            peer_state.active_remote_descriptor_seq,
            Some(pair.remote_descriptor.seq)
        );
        assert!(!peer_state.queued_rerun);

        assert!(
            state.apply_local_descriptor_update(PendingLocalDescriptorUpdate::Set {
                frame: updated_local,
                version: (2, 0),
            })
        );

        let peer_state = state.peers.get(&peer.id).unwrap();
        assert!(peer_state.matcher.is_some());
        assert_eq!(peer_state.active_local_descriptor_version, Some((1, 0)));
        assert_eq!(
            peer_state.active_remote_descriptor_seq,
            Some(pair.remote_descriptor.seq)
        );
        assert!(peer_state.queued_rerun);

        let mut guard = 0usize;
        while state.has_pending_work() && guard < 16 {
            let outcome =
                state.advance_pending_alignments(Duration::ZERO, XR_ALIGNMENT_CALLBACK_MAX_STEPS);
            assert!(outcome.did_work);
            guard += 1;
        }

        let peer_state = state.peers.get(&peer.id).unwrap();
        assert_eq!(
            peer_state.last_solved_local_descriptor_version,
            Some((2, 0))
        );
        assert_eq!(
            peer_state.last_solved_remote_descriptor_seq,
            Some(pair.remote_descriptor.seq)
        );
        assert!(peer_state.matcher.is_none());
        assert!(!peer_state.queued_rerun);
    }

    #[test]
    fn pending_alignment_debug_reports_local_descriptor_before_peer_arrives() {
        let text = make_pending_alignment_debug_text(
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0",
            &HashMap::new(),
        );
        assert_eq!(
            text,
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0 | waiting for peer heightmap"
        );
    }

    #[test]
    fn pending_alignment_debug_reports_solve_pending_once_peer_descriptor_arrives() {
        let mut peers = HashMap::new();
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.latest_descriptor = Some(make_peer_descriptor(2, true));
        peer.has_descriptor = true;
        peers.insert(peer.peer.id, peer);

        let text = make_pending_alignment_debug_text(
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0",
            &peers,
        );
        assert_eq!(
            text,
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0 | 0000002a: remote map seq 7 missing | solve pending"
        );
    }

    #[test]
    fn peer_scene_debug_uses_descriptor_payload_before_solver_runs() {
        let mut peers = HashMap::new();
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.latest_descriptor = Some(make_peer_descriptor(2, true));
        peer.has_descriptor = true;
        peers.insert(peer.peer.id, peer);

        let text = make_peer_scene_debug_text(true, &peers);
        assert_eq!(
            text,
            "PeerMap 0000002a: state no | map yes seq 7 missing | pose raw | solve pending"
        );
    }

    #[test]
    fn peer_scene_debug_prefers_peer_with_descriptor_over_stale_waiting_peer() {
        let mut peers = HashMap::new();
        peers.insert(make_peer(0x01).id, RemotePeerState::new(make_peer(0x01)));

        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.latest_descriptor = Some(make_peer_descriptor(2, true));
        peer.has_descriptor = true;
        peers.insert(peer.peer.id, peer);

        let text = make_peer_scene_debug_text(true, &peers);
        assert_eq!(
            text,
            "PeerMap 0000002a: state no | map yes seq 7 missing | pose raw | solve pending"
        );
    }

    #[test]
    fn alignment_state_reports_local_remote_worker_versions() {
        let mut peers = HashMap::new();
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.latest_descriptor = Some(make_peer_descriptor(2, true));
        peer.last_solve_ms = 1.7;
        peer.last_solved_local_descriptor_version = Some((4, 9));
        peer.last_solved_remote_descriptor_seq = Some(7);
        peer.last_solve_diagnostic = Some(XrDepthAlignSolveDiagnostic {
            local_wall_samples: 8,
            remote_wall_samples: 8,
            local_vertical_descriptor: true,
            remote_vertical_descriptor: true,
            best_solution: Some(make_solution(0.15, vec3(0.2, 0.0, -0.1), 0.42, 0.03, 8)),
            ..XrDepthAlignSolveDiagnostic::default()
        });
        peers.insert(peer.peer.id, peer);

        let text = make_alignment_state_text(LocalSceneState::Ready, Some((4, 9)), &peers);
        assert_eq!(
            text,
            "AlignState 0000002a: local map yes v4/9 | remote map yes seq 7 | worker lv4/9 rv7 accepted match 1.7ms | pose raw"
        );
    }

    #[test]
    fn pending_alignment_debug_keeps_worker_diagnostic_text_when_available() {
        let mut peers = HashMap::new();
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.has_descriptor = true;
        peer.last_solve_diagnostic = Some(XrDepthAlignSolveDiagnostic::default());
        peers.insert(peer.peer.id, peer);

        let text = make_pending_alignment_debug_text(
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0",
            &peers,
        );
        assert_eq!(text, "AlignDbg: local slice 2 | desc occ 0 v 0 c 0");
    }
}
