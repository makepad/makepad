use super::*;

pub(super) const XR_ALIGNMENT_CALLBACK_BUDGET_MILLIS: u64 = 25;
pub(super) const XR_ALIGNMENT_CALLBACK_MAX_STEPS: usize = 4096;
pub(super) const XR_ALIGNMENT_PROGRESS_SIGNAL_INTERVAL_MILLIS: u64 = 100;

#[derive(Debug)]
pub(super) struct AlignmentWorkerPeerState {
    pub(super) peer: XrNetPeer,
    pub(super) latest_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    pub(super) remote_to_local: Option<Mat4f>,
    pub(super) last_accepted_solution: Option<XrDepthAlignSolution>,
    pub(super) last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    pub(super) last_solve_ms: f64,
    pub(super) last_solved_local_descriptor_version: Option<(u64, u64)>,
    pub(super) last_solved_remote_descriptor_seq: Option<u32>,
    pub(super) active_local_descriptor_version: Option<(u64, u64)>,
    pub(super) active_remote_descriptor_seq: Option<u32>,
    pub(super) queued_rerun: bool,
    pub(super) matcher: Option<XrDepthAlignMatcher>,
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
pub(super) struct AlignmentWorkerPeerResult {
    pub(super) remote_to_local: Option<Mat4f>,
    pub(super) last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    pub(super) last_solve_ms: f64,
    pub(super) last_solved_local_descriptor_version: Option<(u64, u64)>,
    pub(super) last_solved_remote_descriptor_seq: Option<u32>,
    pub(super) worker_progress: Option<XrDepthAlignMatcherProgress>,
}

#[derive(Clone, Debug)]
pub(super) struct XrPeopleAlignmentWorkerResult {
    pub(super) peer_results: HashMap<XrNetPeerId, AlignmentWorkerPeerResult>,
    pub(super) alignment_debug_text: String,
}

#[derive(Clone)]
pub(super) enum PendingLocalDescriptorUpdate {
    Set {
        frame: XrNetAlignmentDescriptorFrame,
        version: (u64, u64),
    },
    Clear,
}

#[derive(Clone)]
pub(super) enum PendingPeerDescriptorUpdate {
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

pub(super) struct XrPeopleAlignmentWorker {
    store: XrTsdfStore,
    mailbox: Arc<Mutex<XrPeopleAlignmentWorkerMailbox>>,
    latest_result: Arc<Mutex<Option<XrPeopleAlignmentWorkerResult>>>,
}

impl XrPeopleAlignmentWorker {
    pub(super) fn new(store: XrTsdfStore) -> Self {
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

    pub(super) fn set_local_descriptor(
        &mut self,
        frame: XrNetAlignmentDescriptorFrame,
        version: (u64, u64),
    ) {
        self.send_mailbox_update(|mailbox| {
            mailbox.pending_local_descriptor =
                Some(PendingLocalDescriptorUpdate::Set { frame, version });
        });
    }

    pub(super) fn clear_local_descriptor(&mut self) {
        self.send_mailbox_update(|mailbox| {
            mailbox.pending_local_descriptor = Some(PendingLocalDescriptorUpdate::Clear);
        });
    }

    pub(super) fn set_peer_descriptor(
        &mut self,
        peer: XrNetPeer,
        frame: XrNetAlignmentDescriptorFrame,
    ) {
        self.send_mailbox_update(move |mailbox| {
            mailbox
                .pending_peer_updates
                .insert(peer.id, PendingPeerDescriptorUpdate::Set { peer, frame });
        });
    }

    pub(super) fn remove_peer(&mut self, peer_id: XrNetPeerId) {
        self.send_mailbox_update(move |mailbox| {
            mailbox
                .pending_peer_updates
                .insert(peer_id, PendingPeerDescriptorUpdate::Remove);
        });
    }

    pub(super) fn take_latest_result(&mut self) -> Option<XrPeopleAlignmentWorkerResult> {
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
pub(super) struct AlignmentWorkerState {
    pub(super) peers: HashMap<XrNetPeerId, AlignmentWorkerPeerState>,
    pub(super) local_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    pub(super) local_descriptor_version: Option<(u64, u64)>,
    pub(super) last_progress_publish_at: Option<Instant>,
}

impl AlignmentWorkerState {
    pub(super) fn apply_local_descriptor_update(
        &mut self,
        update: PendingLocalDescriptorUpdate,
    ) -> bool {
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

    pub(super) fn apply_peer_update(
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

    pub(super) fn has_pending_work(&self) -> bool {
        self.peers
            .values()
            .any(|peer_state| peer_state.matcher.is_some())
    }

    pub(super) fn advance_pending_alignments(
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
pub(super) struct AlignmentWorkerStepOutcome {
    pub(super) did_work: bool,
    pub(super) completed_cycle: bool,
    pub(super) result_changed: bool,
    pub(super) has_more_work: bool,
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

pub(super) fn descriptor_change_score(
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

pub(super) fn descriptor_pair_ready_for_solve(
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

pub(super) fn choose_stable_alignment_solution(
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
