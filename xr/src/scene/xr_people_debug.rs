use crate::*;
use std::{
    collections::HashMap,
    sync::{mpsc::TryRecvError, Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Instant,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.XrPeopleDebugBase = #(XrPeopleDebug::register_widget(vm))
    mod.widgets.XrPeopleDebug = set_type_default() do mod.widgets.XrPeopleDebugBase{
        body: mod.widgets.XrBodyKind.Disabled
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RemoteTransformSource {
    #[default]
    Raw,
    Descriptor,
}

#[derive(Clone, Debug)]
struct RemotePeerState {
    peer: XrNetPeer,
    latest_state: Option<XrNetStateFrame>,
    latest_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    has_descriptor: bool,
    remote_to_local: Option<Mat4f>,
    transform_source: RemoteTransformSource,
    last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    last_solve_ms: f64,
    last_solved_local_descriptor_version: Option<(u64, u64)>,
    last_solved_remote_descriptor_seq: Option<u32>,
}

impl RemotePeerState {
    fn new(peer: XrNetPeer) -> Self {
        Self {
            peer,
            latest_state: None,
            latest_descriptor: None,
            has_descriptor: false,
            remote_to_local: None,
            transform_source: RemoteTransformSource::Raw,
            last_solve_diagnostic: None,
            last_solve_ms: 0.0,
            last_solved_local_descriptor_version: None,
            last_solved_remote_descriptor_seq: None,
        }
    }
}

#[derive(Clone, Debug)]
struct AlignmentWorkerPeerState {
    peer: XrNetPeer,
    latest_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    remote_to_local: Option<Mat4f>,
    last_accepted_solution: Option<XrDepthAlignSolution>,
    last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    last_solve_ms: f64,
    last_solved_local_descriptor_version: Option<(u64, u64)>,
    last_solved_remote_descriptor_seq: Option<u32>,
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
        }
    }

    fn to_result(&self) -> AlignmentWorkerPeerResult {
        AlignmentWorkerPeerResult {
            remote_to_local: self.remote_to_local,
            transform_source: if self.remote_to_local.is_some() {
                RemoteTransformSource::Descriptor
            } else {
                RemoteTransformSource::Raw
            },
            last_solve_diagnostic: self.last_solve_diagnostic,
            last_solve_ms: self.last_solve_ms,
            last_solved_local_descriptor_version: self.last_solved_local_descriptor_version,
            last_solved_remote_descriptor_seq: self.last_solved_remote_descriptor_seq,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AlignmentWorkerPeerResult {
    remote_to_local: Option<Mat4f>,
    transform_source: RemoteTransformSource,
    last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    last_solve_ms: f64,
    last_solved_local_descriptor_version: Option<(u64, u64)>,
    last_solved_remote_descriptor_seq: Option<u32>,
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
    version: u64,
    shutdown: bool,
    pending_local_descriptor: Option<PendingLocalDescriptorUpdate>,
    pending_peer_updates: HashMap<XrNetPeerId, PendingPeerDescriptorUpdate>,
}

struct XrPeopleAlignmentWorker {
    mailbox: Arc<(Mutex<XrPeopleAlignmentWorkerMailbox>, Condvar)>,
    latest_result: Arc<Mutex<Option<XrPeopleAlignmentWorkerResult>>>,
    join_handle: Option<JoinHandle<()>>,
}

impl XrPeopleAlignmentWorker {
    fn new() -> Self {
        let mailbox = Arc::new((
            Mutex::new(XrPeopleAlignmentWorkerMailbox::default()),
            Condvar::new(),
        ));
        let latest_result = Arc::new(Mutex::new(None));
        let mailbox_thread = mailbox.clone();
        let result_thread = latest_result.clone();
        let join_handle = Some(thread::spawn(move || {
            xr_people_alignment_worker_loop(mailbox_thread, result_thread)
        }));

        Self {
            mailbox,
            latest_result,
            join_handle,
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
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            update(&mut mailbox);
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
    }
}

impl Drop for XrPeopleAlignmentWorker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.shutdown = true;
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

#[derive(Default)]
struct AlignmentWorkerState {
    peers: HashMap<XrNetPeerId, AlignmentWorkerPeerState>,
    local_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    local_descriptor_version: Option<(u64, u64)>,
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
            changed |= refresh_alignment_worker_peer(
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
        refresh_alignment_worker_peer(
            peer_state,
            local_descriptor.as_ref(),
            local_descriptor_version,
        )
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

fn xr_people_alignment_worker_loop(
    mailbox: Arc<(Mutex<XrPeopleAlignmentWorkerMailbox>, Condvar)>,
    latest_result: Arc<Mutex<Option<XrPeopleAlignmentWorkerResult>>>,
) {
    let mut seen_version = 0u64;
    let mut state = AlignmentWorkerState::default();

    loop {
        let (local_update, peer_updates) = {
            let (lock, wake) = &*mailbox;
            let mut guard = match lock.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            while !guard.shutdown && guard.version == seen_version {
                guard = match wake.wait(guard) {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
            }
            if guard.shutdown {
                return;
            }
            seen_version = guard.version;
            (
                guard.pending_local_descriptor.take(),
                std::mem::take(&mut guard.pending_peer_updates),
            )
        };

        let mut dirty = false;
        if let Some(local_update) = local_update {
            dirty |= state.apply_local_descriptor_update(local_update);
        }
        for (peer_id, update) in peer_updates {
            dirty |= state.apply_peer_update(peer_id, update);
        }
        if dirty {
            if let Ok(mut result_slot) = latest_result.lock() {
                *result_slot = Some(state.make_result());
            }
        }
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

fn refresh_alignment_worker_peer(
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

    if peer_state.last_solved_local_descriptor_version == Some(local_descriptor_version)
        && peer_state.last_solved_remote_descriptor_seq == Some(remote_descriptor.seq)
    {
        return false;
    }

    peer_state.last_solved_local_descriptor_version = Some(local_descriptor_version);
    peer_state.last_solved_remote_descriptor_seq = Some(remote_descriptor.seq);

    let previous_transform = peer_state.remote_to_local;
    let previous_solution = peer_state.last_accepted_solution;
    let previous_diagnostic = peer_state.last_solve_diagnostic;
    let solve_started = Instant::now();
    let diagnostic = xr_depth_align_analyze_remote_to_local(
        &local_descriptor.descriptor,
        &remote_descriptor.descriptor,
    );
    peer_state.last_solve_ms = solve_started.elapsed().as_secs_f64() * 1000.0;
    peer_state.last_solve_diagnostic = Some(diagnostic);

    let next_solution = choose_stable_alignment_solution(
        peer_state.last_accepted_solution,
        previous_diagnostic,
        &diagnostic,
    );
    peer_state.last_accepted_solution = next_solution;
    let next_transform = next_solution.map(|solution| solution.remote_to_local_transform());
    peer_state.remote_to_local = next_transform;
    previous_transform != peer_state.remote_to_local
        || previous_solution != peer_state.last_accepted_solution
        || previous_diagnostic != peer_state.last_solve_diagnostic
}

fn clear_alignment_worker_peer(peer_state: &mut AlignmentWorkerPeerState) -> bool {
    let changed =
        peer_state.remote_to_local.is_some() || peer_state.last_solve_diagnostic.is_some();
    peer_state.remote_to_local = None;
    peer_state.last_accepted_solution = None;
    peer_state.last_solve_diagnostic = None;
    peer_state.last_solve_ms = 0.0;
    peer_state.last_solved_local_descriptor_version = None;
    peer_state.last_solved_remote_descriptor_seq = None;
    changed
}

fn choose_stable_alignment_solution(
    previous: Option<XrDepthAlignSolution>,
    previous_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    diagnostic: &XrDepthAlignSolveDiagnostic,
) -> Option<XrDepthAlignSolution> {
    let candidate = diagnostic.accepted_solution();
    let Some(previous) = previous else {
        return candidate;
    };
    let Some(candidate) = candidate else {
        return Some(previous);
    };
    let previous_score = alignment_lock_score(previous, previous_diagnostic);
    let candidate_score = alignment_lock_score(candidate, Some(*diagnostic));
    let score_delta = candidate_score - previous_score;
    if score_delta < -0.02 {
        return Some(previous);
    }
    if is_large_alignment_jump(previous, candidate) && score_delta < 0.08 {
        return Some(previous);
    }
    if score_delta < 0.03
        && candidate.residual_meters >= previous.residual_meters - 0.02
        && candidate.symmetry_confidence <= previous.symmetry_confidence + 0.03
        && candidate.matched_samples <= previous.matched_samples
    {
        return Some(previous);
    }
    Some(candidate)
}

fn alignment_lock_score(
    solution: XrDepthAlignSolution,
    diagnostic: Option<XrDepthAlignSolveDiagnostic>,
) -> f32 {
    let wall_factor = diagnostic
        .map(|diagnostic| {
            (diagnostic
                .local_wall_features
                .min(diagnostic.remote_wall_features) as f32
                / 4.0)
                .clamp(0.0, 1.0)
        })
        .unwrap_or(0.5);
    let matched_factor = (solution.matched_samples as f32 / 4.0).clamp(0.0, 1.0);
    (solution.ranking_confidence() * 0.55
        + solution.symmetry_confidence * 0.25
        + wall_factor * 0.15
        + matched_factor * 0.05)
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

fn solve_outcome_label(diagnostic: Option<XrDepthAlignSolveDiagnostic>) -> &'static str {
    match diagnostic.map(|diagnostic| diagnostic.outcome()) {
        Some(XrDepthAlignSolveOutcome::MissingSamples) => "need-walls",
        Some(XrDepthAlignSolveOutcome::NoCandidate) => "no-candidate",
        Some(XrDepthAlignSolveOutcome::Rejected) => "rejected",
        Some(XrDepthAlignSolveOutcome::Accepted) => "accepted",
        None => "pending",
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
            peer_state.last_solve_diagnostic.is_some(),
            peer_state.latest_descriptor.is_some(),
            peer_state.latest_state.is_some(),
            std::cmp::Reverse(peer_id.0),
        )
    }) else {
        return format!(
            "AlignState: local {} v{} | waiting for peer",
            if local_descriptor_ready { "yes" } else { "no" },
            local_version,
        );
    };
    let peer_label = format!("{:08x}", peer_id.0);
    let remote_descriptor = peer_state.latest_descriptor.as_ref();
    let remote_wall_count = remote_descriptor
        .map(|frame| frame.descriptor.wall_features.len())
        .unwrap_or(0);
    let remote_vdesc = remote_descriptor.is_some_and(|frame| {
        frame
            .descriptor
            .vertical_descriptor
            .as_ref()
            .is_some_and(|vertical| !vertical.is_empty())
    });
    format!(
        "AlignState {peer_label}: local {} v{} | remote {} seq {} walls {} vdesc {} | worker lv{} rv{} {} {:.1}ms | pose {}",
        if local_descriptor_ready { "yes" } else { "no" },
        local_version,
        if remote_descriptor.is_some() { "yes" } else { "no" },
        descriptor_seq_label(remote_descriptor.map(|frame| frame.seq)),
        remote_wall_count,
        if remote_vdesc { "yes" } else { "no" },
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
        return "PeerScene: waiting for peer".to_string();
    };
    let peer_label = format!("{:08x}", peer_id.0);
    let state_text = if peer_state.latest_state.is_some() {
        "yes"
    } else {
        "no"
    };
    if let Some(diagnostic) = peer_state.last_solve_diagnostic {
        return format!(
            "PeerScene {peer_label}: state {state_text} | desc {} | walls {} | vdesc {} | pose {}",
            if peer_state.has_descriptor {
                "yes"
            } else {
                "no"
            },
            diagnostic.remote_wall_features,
            if diagnostic.remote_vertical_descriptor {
                "yes"
            } else {
                "no"
            },
            transform_source_label(peer_state.transform_source),
        );
    }
    if let Some(descriptor) = peer_state.latest_descriptor.as_ref() {
        return format!(
            "PeerScene {peer_label}: state {state_text} | desc yes | walls {} | vdesc {} | pose {}{}",
            descriptor.descriptor.wall_features.len(),
            if descriptor
                .descriptor
                .vertical_descriptor
                .as_ref()
                .is_some_and(|vertical| !vertical.is_empty())
            {
                "yes"
            } else {
                "no"
            },
            transform_source_label(peer_state.transform_source),
            if has_local_descriptor {
                " | solve pending"
            } else {
                ""
            },
        );
    }
    format!(
        "PeerScene {peer_label}: state {state_text} | desc {} | walls ? | vdesc ? | pose {}{}",
        if peer_state.has_descriptor {
            "yes"
        } else {
            "no"
        },
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
        return format!("{local_descriptor_text} | waiting for peer descriptor");
    };
    if peer_state.last_solve_diagnostic.is_some() {
        return local_descriptor_text.to_string();
    }
    let peer_label = format!("{:08x}", peer_id.0);
    if let Some(descriptor) = peer_state.latest_descriptor.as_ref() {
        format!(
            "{local_descriptor_text} | {peer_label}: remote walls {} | remote vdesc {} | solve pending",
            descriptor.descriptor.wall_features.len(),
            if descriptor
                .descriptor
                .vertical_descriptor
                .as_ref()
                .is_some_and(|vertical| !vertical.is_empty())
            {
                "yes"
            } else {
                "no"
            },
        )
    } else if peer_state.has_descriptor {
        format!("{local_descriptor_text} | {peer_label}: solve pending")
    } else {
        format!("{local_descriptor_text} | {peer_label}: waiting for peer descriptor")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalSceneState {
    Missing,
    PublishPending,
    Ready,
}

fn alignment_rejection_reason(
    diagnostic: &XrDepthAlignSolveDiagnostic,
    best: XrDepthAlignSolution,
) -> String {
    if best.matched_samples < 2 {
        format!("matched {}<2", best.matched_samples)
    } else if diagnostic.local_vertical_descriptor
        && diagnostic.remote_vertical_descriptor
        && best.symmetry_confidence <= 0.10
    {
        format!("symmetry {:.2}<=0.10", best.symmetry_confidence)
    } else if best.confidence <= 0.18 {
        format!("confidence {:.2}<=0.18", best.confidence)
    } else if !best.residual_meters.is_finite() {
        "vertical overlap missing".to_string()
    } else if best.residual_meters >= 0.18 {
        format!("residual {:.2}m>=0.18m", best.residual_meters)
    } else if diagnostic.remote_wall_features < 2 {
        format!("remote walls {}<2", diagnostic.remote_wall_features)
    } else if !diagnostic.remote_vertical_descriptor {
        "remote compact scene missing".to_string()
    } else {
        "score below accept threshold".to_string()
    }
}

fn make_alignment_debug_text(
    local_scene_state: LocalSceneState,
    peers: &HashMap<XrNetPeerId, AlignmentWorkerPeerState>,
) -> String {
    let Some((peer_id, peer_state)) = peers.iter().max_by_key(|(peer_id, peer_state)| {
        (
            peer_state.last_solve_diagnostic.is_some(),
            peer_state.latest_descriptor.is_some(),
            std::cmp::Reverse(peer_id.0),
        )
    }) else {
        return match local_scene_state {
            LocalSceneState::Ready => "AlignDbg: waiting for peer descriptor".to_string(),
            LocalSceneState::PublishPending => {
                "AlignDbg: local walls ready | waiting to publish local descriptor".to_string()
            }
            LocalSceneState::Missing => "AlignDbg: waiting for local scene descriptor".to_string(),
        };
    };
    let peer_label = format!("{:08x}", peer_id.0);
    if local_scene_state == LocalSceneState::Missing {
        return format!("AlignDbg {peer_label}: waiting for local scene descriptor");
    }
    if local_scene_state == LocalSceneState::PublishPending {
        return format!(
            "AlignDbg {peer_label}: local walls ready | waiting to publish local descriptor"
        );
    }
    let Some(diagnostic) = peer_state.last_solve_diagnostic else {
        if let Some(remote_descriptor) = peer_state.latest_descriptor.as_ref() {
            return format!(
                "AlignDbg {peer_label}: remote desc seq {} walls {} vdesc {} | waiting for solve",
                remote_descriptor.seq,
                remote_descriptor.descriptor.wall_features.len(),
                if remote_descriptor
                    .descriptor
                    .vertical_descriptor
                    .as_ref()
                    .is_some_and(|vertical| !vertical.is_empty())
                {
                    "yes"
                } else {
                    "no"
                },
            );
        }
        return format!("AlignDbg {peer_label}: waiting for peer descriptor");
    };

    let context = format!(
        "local walls {} | remote walls {} | local vdesc {} | remote vdesc {} | yaw {} | pose {}",
        diagnostic.local_wall_features,
        diagnostic.remote_wall_features,
        if diagnostic.local_vertical_descriptor {
            "yes"
        } else {
            "no"
        },
        if diagnostic.remote_vertical_descriptor {
            "yes"
        } else {
            "no"
        },
        diagnostic.yaw_candidate_count,
        diagnostic.pose_candidate_count,
    );
    match diagnostic.outcome() {
        XrDepthAlignSolveOutcome::MissingSamples => {
            format!("AlignDbg {peer_label}: need >=2 room walls on both sides | {context}")
        }
        XrDepthAlignSolveOutcome::NoCandidate => {
            format!(
                "AlignDbg {peer_label}: no candidate t{:.1}ms | {context}",
                peer_state.last_solve_ms,
            )
        }
        XrDepthAlignSolveOutcome::Rejected => {
            let Some(best) = diagnostic.best_solution else {
                return format!("AlignDbg {peer_label}: reject | {context}");
            };
            format!(
                "AlignDbg {peer_label}: reject {} | cw{:.2} cs{:.2} cr{:.2} m{} r{:.2} t{:.1}ms | {context}",
                alignment_rejection_reason(&diagnostic, best),
                best.confidence,
                best.symmetry_confidence,
                best.ranking_confidence(),
                best.matched_samples,
                best.residual_meters,
                peer_state.last_solve_ms,
            )
        }
        XrDepthAlignSolveOutcome::Accepted => {
            let Some(best) = diagnostic.best_solution else {
                return format!("AlignDbg {peer_label}: aligned | {context}");
            };
            format!(
                "AlignDbg {peer_label}: ok cw{:.2} cs{:.2} cr{:.2} m{} r{:.2} t{:.1}ms yaw{:.0} tx{:.2} ty{:.2} tz{:.2} | {context}",
                best.confidence,
                best.symmetry_confidence,
                best.ranking_confidence(),
                best.matched_samples,
                best.residual_meters,
                peer_state.last_solve_ms,
                best.yaw_radians.to_degrees(),
                best.translation.x,
                best.translation.y,
                best.translation.z,
            )
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrPeopleDebug {
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[rust]
    enabled: bool,
    #[rust]
    last_status: String,
    #[rust]
    last_network_status: String,
    #[rust]
    last_peer_scene_status: String,
    #[rust]
    last_alignment_debug_status: String,
    #[rust]
    last_alignment_state_status: String,
    #[rust]
    last_plane_scan_status: String,
    #[rust]
    net_node: Option<XrNetNode>,
    #[rust]
    alignment_worker: Option<XrPeopleAlignmentWorker>,
    #[rust]
    peers: HashMap<XrNetPeerId, RemotePeerState>,
    #[rust]
    local_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    #[rust]
    local_descriptor_version: Option<(u64, u64)>,
    #[rust]
    local_plane_patches: Vec<XrDepthPlanePatch>,
    #[rust]
    local_alignment_wall_features: Vec<XrDepthAlignWallFeature>,
    #[rust]
    local_alignment_debug: XrDepthAlignDebug,
    #[rust]
    last_sent_descriptor_signature: Option<(u64, u64)>,
    #[rust]
    tx_state_count: u64,
    #[rust]
    tx_descriptor_count: u64,
    #[rust]
    rx_join_count: u64,
    #[rust]
    rx_leave_count: u64,
    #[rust]
    rx_state_count: u64,
    #[rust]
    rx_descriptor_count: u64,
    #[rust]
    last_event_text: String,
    #[cast]
    #[deref]
    node: XrNode,
}

impl XrPeopleDebug {
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
    const ALIGN_WALL_BEAM_HEIGHT: f32 = 0.10;
    const ALIGN_WALL_BEAM_THICKNESS: f32 = 0.045;
    const ALIGN_WALL_BEAM_NORMAL_OFFSET: f32 = 0.05;
    const PEER_ALIGN_WALL_BEAM_HEIGHT: f32 = 0.075;
    const PEER_ALIGN_WALL_BEAM_THICKNESS: f32 = 0.030;
    const PEER_ALIGN_WALL_BEAM_NORMAL_OFFSET: f32 = -0.05;
    const PEER_ALIGN_MARKER_SIZE: f32 = 0.055;
    const DESCRIPTOR_MAX_HEIGHT_METERS: f32 = 2.00;
    const DESCRIPTOR_MIN_HEIGHT_METERS: f32 = 0.08;
    const DESCRIPTOR_CELL_FOOTPRINT: f32 = 0.62;
    const SHOW_LOCAL_DESCRIPTOR_DEBUG: bool = false;

    pub fn status_text(&self) -> &str {
        if self.last_status.is_empty() {
            "Peers: off"
        } else {
            &self.last_status
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn network_status_text(&self) -> &str {
        if self.last_network_status.is_empty() {
            "Network: off"
        } else {
            &self.last_network_status
        }
    }

    pub fn alignment_debug_text(&self) -> &str {
        if self.last_alignment_debug_status.is_empty() {
            "AlignDbg: off"
        } else {
            &self.last_alignment_debug_status
        }
    }

    pub fn alignment_state_text(&self) -> &str {
        if self.last_alignment_state_status.is_empty() {
            "AlignState: off"
        } else {
            &self.last_alignment_state_status
        }
    }

    pub fn peer_scene_text(&self) -> &str {
        if self.last_peer_scene_status.is_empty() {
            "PeerScene: off"
        } else {
            &self.last_peer_scene_status
        }
    }

    pub fn plane_scan_text(&self) -> &str {
        if self.last_plane_scan_status.is_empty() {
            "PlaneScan: off"
        } else {
            &self.last_plane_scan_status
        }
    }

    pub fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) -> bool {
        if self.enabled == enabled {
            return self.enabled;
        }
        self.enabled = enabled;
        cx.xr_depth_mesh().set_plane_scan_enabled(enabled);
        cx.xr_depth_mesh().set_surface_analysis_enabled(enabled);
        self.net_node = None;
        self.alignment_worker = None;
        self.peers.clear();
        self.local_descriptor = None;
        self.local_descriptor_version = None;
        self.local_plane_patches.clear();
        self.local_alignment_wall_features.clear();
        self.local_alignment_debug = XrDepthAlignDebug::default();
        self.last_sent_descriptor_signature = None;
        self.tx_state_count = 0;
        self.tx_descriptor_count = 0;
        self.rx_join_count = 0;
        self.rx_leave_count = 0;
        self.rx_state_count = 0;
        self.rx_descriptor_count = 0;
        self.last_event_text.clear();
        self.last_peer_scene_status.clear();
        self.last_plane_scan_status.clear();

        if enabled {
            self.alignment_worker = Some(XrPeopleAlignmentWorker::new());
            self.ensure_net_node();
            if self.net_node.is_some() {
                self.last_status = "Peers: scanning LAN for clients".to_string();
                self.last_network_status =
                    "Network: bridge ready | waiting for local XR frames".to_string();
            } else {
                self.last_status = "Peers: network unavailable".to_string();
                self.last_network_status = "Network: bind failed".to_string();
            }
            self.last_peer_scene_status = "PeerScene: waiting for peer".to_string();
            self.last_alignment_debug_status =
                "AlignDbg: waiting for local scene descriptor".to_string();
            self.last_alignment_state_status =
                "AlignState: local no v- | waiting for peer".to_string();
            self.last_plane_scan_status = "PlaneScan: waiting for TSDF scan".to_string();
        } else {
            self.last_status = "Peers: off".to_string();
            self.last_network_status = "Network: off".to_string();
            self.last_peer_scene_status = "PeerScene: off".to_string();
            self.last_alignment_debug_status = "AlignDbg: off".to_string();
            self.last_alignment_state_status = "AlignState: off".to_string();
            self.last_plane_scan_status = "PlaneScan: off".to_string();
        }
        self.redraw(cx);
        self.enabled
    }

    fn ensure_net_node(&mut self) {
        if self.net_node.is_some() {
            return;
        }
        match XrNetNode::new() {
            Ok(node) => {
                self.net_node = Some(node);
                self.last_sent_descriptor_signature = None;
                self.last_event_text = "node started".to_string();
            }
            Err(err) => {
                self.last_status = format!("Peers: network bind failed ({err})");
                self.last_network_status = format!("Network: bind failed ({err})");
            }
        }
    }

    fn refresh_from_local_state(&mut self, cx: &mut Cx, state: &XrState) {
        if !self.enabled {
            return;
        }
        self.ensure_net_node();
        let Some(net_node) = self.net_node.as_mut() else {
            return;
        };

        net_node.send_state(state.clone());
        self.tx_state_count = self.tx_state_count.saturating_add(1);

        let next_mesh = cx.xr_depth_mesh().latest_mesh();
        self.local_alignment_debug = next_mesh
            .as_ref()
            .map(|mesh| mesh.alignment_debug)
            .unwrap_or_default();
        self.local_plane_patches = next_mesh
            .as_ref()
            .map(|mesh| mesh.plane_patches.clone())
            .unwrap_or_default();
        self.local_alignment_wall_features = next_mesh
            .as_ref()
            .and_then(|mesh| {
                mesh.alignment_descriptor
                    .as_ref()
                    .map(|descriptor| descriptor.wall_features.clone())
            })
            .unwrap_or_default();
        let next_signature = next_mesh
            .as_ref()
            .map(|mesh| (mesh.mesh_generation, mesh.update_sequence));
        let next_descriptor = next_mesh.as_ref().and_then(|mesh| {
            XrNetAlignmentDescriptorFrame::from_depth_mesh(mesh.as_ref(), state.time)
        });
        self.local_descriptor_version = if next_descriptor.is_some() {
            next_signature
        } else {
            None
        };

        if !descriptor_frames_equal(self.local_descriptor.as_ref(), next_descriptor.as_ref()) {
            self.local_descriptor = next_descriptor.clone();
            if let Some(worker) = self.alignment_worker.as_mut() {
                if let (Some(frame), Some(version)) = (next_descriptor.clone(), next_signature) {
                    worker.set_local_descriptor(frame, version);
                } else {
                    worker.clear_local_descriptor();
                }
            }
        }

        if let (Some(signature), Some(frame)) = (next_signature, next_descriptor) {
            if self.last_sent_descriptor_signature != Some(signature) {
                net_node.send_alignment_descriptor(frame);
                self.last_sent_descriptor_signature = Some(signature);
                self.tx_descriptor_count = self.tx_descriptor_count.saturating_add(1);
            }
        } else {
            self.last_sent_descriptor_signature = None;
        }
    }

    fn poll_network(&mut self) {
        if !self.enabled {
            return;
        }

        let mut disconnected = false;
        loop {
            let result = match self.net_node.as_mut() {
                Some(net_node) => net_node.incoming_receiver.try_recv(),
                None => break,
            };
            match result {
                Ok(message) => self.handle_network_message(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if disconnected {
            self.net_node = None;
            self.last_sent_descriptor_signature = None;
            self.last_status = "Peers: network worker disconnected, retrying".to_string();
            self.last_network_status = "Network: worker disconnected".to_string();
        }
    }

    fn handle_network_message(&mut self, message: XrNetIncoming) {
        match message {
            XrNetIncoming::Join { peer } => {
                self.rx_join_count = self.rx_join_count.saturating_add(1);
                self.last_event_text = format!("join {}", Self::peer_label(peer.id));
                self.peers
                    .entry(peer.id)
                    .or_insert_with(|| RemotePeerState::new(peer));
            }
            XrNetIncoming::Leave { peer, .. } => {
                self.rx_leave_count = self.rx_leave_count.saturating_add(1);
                self.last_event_text = format!("leave {}", Self::peer_label(peer.id));
                self.peers.remove(&peer.id);
                if let Some(worker) = self.alignment_worker.as_mut() {
                    worker.remove_peer(peer.id);
                }
            }
            XrNetIncoming::State { peer, frame } => {
                self.rx_state_count = self.rx_state_count.saturating_add(1);
                self.last_event_text =
                    format!("state {} seq {}", Self::peer_label(peer.id), frame.seq);
                let peer_state = self
                    .peers
                    .entry(peer.id)
                    .or_insert_with(|| RemotePeerState::new(peer));
                peer_state.peer = peer;
                peer_state.latest_state = Some(frame);
            }
            XrNetIncoming::AlignmentDescriptor { peer, frame } => {
                self.rx_descriptor_count = self.rx_descriptor_count.saturating_add(1);
                self.last_event_text =
                    format!("desc {} seq {}", Self::peer_label(peer.id), frame.seq);
                let peer_state = self
                    .peers
                    .entry(peer.id)
                    .or_insert_with(|| RemotePeerState::new(peer));
                peer_state.peer = peer;
                peer_state.latest_descriptor = Some(frame.clone());
                peer_state.has_descriptor = true;
                if let Some(worker) = self.alignment_worker.as_mut() {
                    worker.set_peer_descriptor(peer, frame);
                }
            }
            XrNetIncoming::Alignment { .. } => {}
        }
    }

    fn apply_alignment_results(&mut self, cx: &mut Cx) {
        let Some(worker) = self.alignment_worker.as_mut() else {
            return;
        };
        let Some(result) = worker.take_latest_result() else {
            return;
        };

        for peer_state in self.peers.values_mut() {
            peer_state.remote_to_local = None;
            peer_state.transform_source = RemoteTransformSource::Raw;
        }
        for (peer_id, peer_result) in result.peer_results {
            if let Some(peer_state) = self.peers.get_mut(&peer_id) {
                peer_state.remote_to_local = peer_result.remote_to_local;
                peer_state.transform_source = peer_result.transform_source;
                peer_state.last_solve_diagnostic = peer_result.last_solve_diagnostic;
                peer_state.last_solve_ms = peer_result.last_solve_ms;
                peer_state.last_solved_local_descriptor_version =
                    peer_result.last_solved_local_descriptor_version;
                peer_state.last_solved_remote_descriptor_seq =
                    peer_result.last_solved_remote_descriptor_seq;
                peer_state.has_descriptor =
                    peer_state.has_descriptor || peer_result.last_solve_diagnostic.is_some();
            }
        }
        self.last_alignment_debug_status = result.alignment_debug_text;
        self.redraw(cx);
    }

    fn refresh_status(&mut self) {
        if !self.enabled {
            self.last_status = "Peers: off".to_string();
            self.last_network_status = "Network: off".to_string();
            self.last_peer_scene_status = "PeerScene: off".to_string();
            self.last_alignment_debug_status = "AlignDbg: off".to_string();
            self.last_alignment_state_status = "AlignState: off".to_string();
            self.last_plane_scan_status = "PlaneScan: off".to_string();
            return;
        }
        if self.net_node.is_none() {
            if self.last_status.is_empty() {
                self.last_status = "Peers: network unavailable".to_string();
            }
            if self.last_network_status.is_empty() {
                self.last_network_status = "Network: unavailable".to_string();
            }
            if self.last_peer_scene_status.is_empty() {
                self.last_peer_scene_status = "PeerScene: network unavailable".to_string();
            }
            return;
        }

        let peer_count = self.peers.len();
        let visible_count = self
            .peers
            .values()
            .filter(|peer| peer.latest_state.is_some())
            .count();
        let descriptor_count = self
            .peers
            .values()
            .filter(|peer| peer.has_descriptor)
            .count();
        let aligned_count = self
            .peers
            .values()
            .filter(|peer| peer.latest_state.is_some() && peer.remote_to_local.is_some())
            .count();
        let local_scene_state = self.local_scene_state();

        self.last_status = if peer_count == 0 {
            "Peers: scanning LAN for clients".to_string()
        } else if local_scene_state == LocalSceneState::Ready {
            format!("Peers: {peer_count} seen | {visible_count} state | {aligned_count} descriptor-solved")
        } else if local_scene_state == LocalSceneState::PublishPending {
            format!(
                "Peers: {peer_count} seen | {visible_count} state | local walls {} ready | waiting to publish local descriptor",
                self.local_alignment_wall_features.len()
            )
        } else {
            format!("Peers: {peer_count} seen | {visible_count} state | waiting for local scene descriptor")
        };

        let last_event = if self.last_event_text.is_empty() {
            "none"
        } else {
            &self.last_event_text
        };
        self.last_network_status = format!(
            "Network: tx s{} d{} | rx j{} l{} s{} d{} | peers {} vis {} desc {} align {} | local desc {} walls {} | last {}",
            self.tx_state_count,
            self.tx_descriptor_count,
            self.rx_join_count,
            self.rx_leave_count,
            self.rx_state_count,
            self.rx_descriptor_count,
            peer_count,
            visible_count,
            descriptor_count,
            aligned_count,
            match local_scene_state {
                LocalSceneState::Ready => "yes",
                LocalSceneState::PublishPending => "pending",
                LocalSceneState::Missing => "no",
            },
            self.local_alignment_wall_features.len(),
            last_event,
        );
        self.last_peer_scene_status =
            make_peer_scene_debug_text(local_scene_state == LocalSceneState::Ready, &self.peers);
        self.last_alignment_state_status = make_alignment_state_text(
            local_scene_state,
            self.local_descriptor_version,
            &self.peers,
        );
        self.last_plane_scan_status = self.local_plane_scan_text();
        let has_alignment_diagnostic = self
            .peers
            .values()
            .any(|peer| peer.last_solve_diagnostic.is_some());
        if local_scene_state != LocalSceneState::Ready || !has_alignment_diagnostic {
            let local_descriptor_text = self.local_descriptor_debug_text();
            self.last_alignment_debug_status = if local_scene_state == LocalSceneState::Ready {
                make_pending_alignment_debug_text(&local_descriptor_text, &self.peers)
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
            RemoteTransformSource::Descriptor => 1.0,
            RemoteTransformSource::Raw => 0.42,
        }
    }

    fn local_scene_state(&self) -> LocalSceneState {
        if self.local_descriptor.is_some() {
            LocalSceneState::Ready
        } else if !self.local_alignment_wall_features.is_empty() {
            LocalSceneState::PublishPending
        } else {
            LocalSceneState::Missing
        }
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

    fn draw_alignment_wall_features(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        walls: &[XrDepthAlignWallFeature],
        beam_height: f32,
        beam_thickness: f32,
        normal_offset: f32,
        color: Vec4f,
    ) {
        for wall in walls.iter().copied() {
            let center = vec3(
                wall.center.x + wall.normal.x * normal_offset,
                wall.max_y - beam_height * 0.5,
                wall.center.z + wall.normal.z * normal_offset,
            );
            let transform = Mat4f {
                v: [
                    wall.along_axis.x,
                    wall.along_axis.y,
                    wall.along_axis.z,
                    0.0,
                    0.0,
                    1.0,
                    0.0,
                    0.0,
                    wall.normal.x,
                    wall.normal.y,
                    wall.normal.z,
                    0.0,
                    center.x,
                    center.y,
                    center.z,
                    1.0,
                ],
            };
            self.draw_cube_at(
                cx,
                world,
                &transform,
                vec3(wall.half_extent_along * 2.0, beam_height, beam_thickness),
                color,
            );
        }
    }

    fn local_descriptor_debug_text(&self) -> String {
        let (descriptor_cells, vertical_cells, clutter_cells) = self
            .local_descriptor
            .as_ref()
            .and_then(|frame| frame.descriptor.vertical_descriptor.as_ref())
            .map(|descriptor| {
                let size = descriptor.size as usize;
                let mut occupied = 0usize;
                let mut vertical = 0usize;
                let mut clutter = 0usize;
                if size != 0
                    && descriptor.vertical_surface_masks.len() == size * size
                    && descriptor.clutter_surface_masks.len() == size * size
                {
                    for index in 0..size * size {
                        if descriptor.vertical_surface_masks[index] != 0
                            || descriptor.clutter_surface_masks[index] != 0
                        {
                            occupied += 1;
                        }
                        if descriptor.vertical_surface_masks[index] != 0 {
                            vertical += 1;
                        }
                        if descriptor.clutter_surface_masks[index] != 0 {
                            clutter += 1;
                        }
                    }
                }
                (occupied, vertical, clutter)
            })
            .unwrap_or((0, 0, 0));
        match self.local_scene_state() {
            LocalSceneState::Missing => "AlignDbg: waiting for local scene descriptor".to_string(),
            LocalSceneState::PublishPending => format!(
                "AlignDbg: local walls {} ready | descriptor publish pending",
                self.local_alignment_wall_features.len(),
            ),
            LocalSceneState::Ready => format!(
                "AlignDbg: local wall feats {} | desc occ {} v {} c {}",
                self.local_alignment_wall_features.len(),
                descriptor_cells,
                vertical_cells,
                clutter_cells,
            ),
        }
    }

    fn local_plane_scan_text(&self) -> String {
        let mut wall_count = 0usize;
        let mut other_count = 0usize;
        for patch in &self.local_plane_patches {
            match patch.kind {
                XrDepthPlaneKind::Wall => wall_count += 1,
                XrDepthPlaneKind::Floor
                | XrDepthPlaneKind::Ceiling
                | XrDepthPlaneKind::Table
                | XrDepthPlaneKind::Unknown => other_count += 1,
            }
        }
        let debug = self.local_alignment_debug;
        if debug.near_surface_voxel_count == 0 && self.local_plane_patches.is_empty() {
            "PlaneScan: waiting for TSDF scan".to_string()
        } else {
            format!(
                "PlaneScan: patches {} wall {} room {} other {} | near {} wall vox {}",
                self.local_plane_patches.len(),
                wall_count,
                self.local_alignment_wall_features.len(),
                other_count,
                debug.near_surface_voxel_count,
                debug.wall_candidate_count,
            )
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
            .local_descriptor
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

    fn draw_local_alignment_walls(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let walls = self.local_alignment_wall_features.clone();
        self.draw_alignment_wall_features(
            cx,
            world,
            &walls,
            Self::ALIGN_WALL_BEAM_HEIGHT,
            Self::ALIGN_WALL_BEAM_THICKNESS,
            Self::ALIGN_WALL_BEAM_NORMAL_OFFSET,
            vec4f(1.0, 0.18, 0.14, 0.92),
        );
    }

    fn draw_remote_alignment_walls(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let peer_ids = self.peers.keys().copied().collect::<Vec<_>>();
        for peer_id in peer_ids {
            let Some(peer) = self.peers.get(&peer_id).cloned() else {
                continue;
            };
            let (Some(remote_to_local), Some(descriptor_frame)) =
                (peer.remote_to_local, peer.latest_descriptor.as_ref())
            else {
                continue;
            };

            let solved_frame = descriptor_frame.transformed(&remote_to_local);
            if solved_frame.descriptor.wall_features.is_empty() {
                continue;
            }

            let base = Self::peer_base_color(peer_id);
            let wall_color = vec4f(
                (base.x * 0.82 + 0.18).min(1.0),
                (base.y * 0.82 + 0.18).min(1.0),
                (base.z * 0.82 + 0.18).min(1.0),
                0.86,
            );
            self.draw_alignment_wall_features(
                cx,
                world,
                &solved_frame.descriptor.wall_features,
                Self::PEER_ALIGN_WALL_BEAM_HEIGHT,
                Self::PEER_ALIGN_WALL_BEAM_THICKNESS,
                Self::PEER_ALIGN_WALL_BEAM_NORMAL_OFFSET,
                wall_color,
            );

            if let Some(markers) = solved_frame.test_markers() {
                let marker_color = vec4f(
                    (base.x * 0.78 + 0.22).min(1.0),
                    (base.y * 0.78 + 0.22).min(1.0),
                    1.0,
                    0.94,
                );
                for marker in markers {
                    let transform = Pose::new(Quat::default(), marker).to_mat4();
                    self.draw_cube_at(
                        cx,
                        world,
                        &transform,
                        vec3(
                            Self::PEER_ALIGN_MARKER_SIZE,
                            Self::PEER_ALIGN_MARKER_SIZE,
                            Self::PEER_ALIGN_MARKER_SIZE,
                        ),
                        marker_color,
                    );
                }
            }
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
        let peer_ids = self.peers.keys().copied().collect::<Vec<_>>();
        for peer_id in peer_ids {
            let Some(peer) = self.peers.get(&peer_id).cloned() else {
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

impl Widget for XrPeopleDebug {
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
            self.poll_network();
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
            // Depth mesh and plane scan patches live in raw OpenXR local-space.
            // Keep debug planes and remote peers in that same space so
            // root/content recentering does not skew or shift them.
            Mat4f::identity()
        } else {
            self.node.local_transform()
        };
        self.draw_cube.begin_many_instances(cx);
        if Self::SHOW_LOCAL_DESCRIPTOR_DEBUG {
            self.draw_local_descriptor(cx, &world);
        }
        self.draw_local_alignment_walls(cx, &world);
        self.draw_remote_alignment_walls(cx, &world);
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
            used_wall_features: true,
            local_wall_features: 4,
            remote_wall_features: 4,
            best_solution: Some(solution),
            ..XrDepthAlignSolveDiagnostic::default()
        }
    }

    #[test]
    fn stable_alignment_prefers_existing_solution_over_flip() {
        let previous = make_solution(0.42, vec3(0.28, 0.0, -0.64), 0.41, 0.03, 4);
        let flipped = make_solution(-2.71, vec3(-0.34, 0.0, 0.71), 0.44, 0.03, 4);

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(make_diagnostic(previous)),
            &make_diagnostic(flipped),
        )
        .unwrap();

        assert_eq!(chosen, previous);
    }

    #[test]
    fn stable_alignment_accepts_small_refinement() {
        let previous = make_solution(0.42, vec3(0.28, 0.0, -0.64), 0.28, 0.06, 3);
        let refined = make_solution(0.46, vec3(0.24, 0.0, -0.60), 0.35, 0.03, 4);

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(make_diagnostic(previous)),
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
            matched_samples: 4,
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
        weaker_diag.local_wall_features = 2;
        weaker_diag.remote_wall_features = 2;

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(make_diagnostic(previous)),
            &weaker_diag,
        )
        .unwrap();

        assert_eq!(chosen, previous);
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
        XrNetAlignmentDescriptorFrame {
            seq: 7,
            sent_at: 1.0,
            descriptor: XrDepthAlignDescriptor {
                wall_features: vec![XrDepthAlignWallFeature::default(); wall_count],
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

    #[test]
    fn pending_alignment_debug_reports_local_descriptor_before_peer_arrives() {
        let text = make_pending_alignment_debug_text(
            "AlignDbg: local wall feats 2 | desc occ 0 v 0 c 0",
            &HashMap::new(),
        );
        assert_eq!(
            text,
            "AlignDbg: local wall feats 2 | desc occ 0 v 0 c 0 | waiting for peer descriptor"
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
            "AlignDbg: local wall feats 2 | desc occ 0 v 0 c 0",
            &peers,
        );
        assert_eq!(
            text,
            "AlignDbg: local wall feats 2 | desc occ 0 v 0 c 0 | 0000002a: remote walls 2 | remote vdesc yes | solve pending"
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
            "PeerScene 0000002a: state no | desc yes | walls 2 | vdesc yes | pose raw | solve pending"
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
            "PeerScene 0000002a: state no | desc yes | walls 2 | vdesc yes | pose raw | solve pending"
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
            used_wall_features: true,
            local_wall_features: 2,
            remote_wall_features: 2,
            local_vertical_descriptor: true,
            remote_vertical_descriptor: true,
            best_solution: Some(make_solution(0.15, vec3(0.2, 0.0, -0.1), 0.42, 0.03, 3)),
            ..XrDepthAlignSolveDiagnostic::default()
        });
        peers.insert(peer.peer.id, peer);

        let text = make_alignment_state_text(LocalSceneState::Ready, Some((4, 9)), &peers);
        assert_eq!(
            text,
            "AlignState 0000002a: local yes v4/9 | remote yes seq 7 walls 2 vdesc yes | worker lv4/9 rv7 accepted 1.7ms | pose raw"
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
            "AlignDbg: local wall feats 2 | desc occ 0 v 0 c 0",
            &peers,
        );
        assert_eq!(text, "AlignDbg: local wall feats 2 | desc occ 0 v 0 c 0");
    }
}
