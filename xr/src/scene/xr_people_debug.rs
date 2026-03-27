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
    has_descriptor: bool,
    remote_to_local: Option<Mat4f>,
    transform_source: RemoteTransformSource,
    last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
}

impl RemotePeerState {
    fn new(peer: XrNetPeer) -> Self {
        Self {
            peer,
            latest_state: None,
            has_descriptor: false,
            remote_to_local: None,
            transform_source: RemoteTransformSource::Raw,
            last_solve_diagnostic: None,
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
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AlignmentWorkerPeerResult {
    remote_to_local: Option<Mat4f>,
    transform_source: RemoteTransformSource,
    last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
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
            alignment_debug_text: make_alignment_debug_text(
                self.local_descriptor.is_some(),
                &self.peers,
            ),
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

    let solve_started = Instant::now();
    let diagnostic = xr_depth_align_analyze_remote_to_local(
        &local_descriptor.descriptor,
        &remote_descriptor.descriptor,
    );
    peer_state.last_solve_ms = solve_started.elapsed().as_secs_f64() * 1000.0;
    peer_state.last_solve_diagnostic = Some(diagnostic);

    let next_solution =
        choose_stable_alignment_solution(peer_state.last_accepted_solution, &diagnostic);
    peer_state.last_accepted_solution = next_solution;
    let next_transform = next_solution.map(|solution| solution.remote_to_local_transform());
    let changed = peer_state.remote_to_local != next_transform;
    peer_state.remote_to_local = next_transform;
    changed
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
    diagnostic: &XrDepthAlignSolveDiagnostic,
) -> Option<XrDepthAlignSolution> {
    let candidate = diagnostic.accepted_solution();
    let Some(previous) = previous else {
        return candidate;
    };
    let Some(candidate) = candidate else {
        return Some(previous);
    };
    if is_large_alignment_jump(previous, candidate)
        && candidate.confidence < previous.confidence + 0.18
        && candidate.matched_samples <= previous.matched_samples + 1
        && candidate.residual_meters >= previous.residual_meters - 0.03
    {
        return Some(previous);
    }
    Some(candidate)
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

fn make_alignment_debug_text(
    has_local_descriptor: bool,
    peers: &HashMap<XrNetPeerId, AlignmentWorkerPeerState>,
) -> String {
    let Some((peer_id, peer_state)) = peers.iter().min_by_key(|(peer_id, _)| peer_id.0) else {
        return if has_local_descriptor {
            "AlignDbg: waiting for peer descriptor".to_string()
        } else {
            "AlignDbg: waiting for local scene descriptor".to_string()
        };
    };
    let peer_label = format!("{:08x}", peer_id.0);
    if !has_local_descriptor {
        return format!("AlignDbg {peer_label}: waiting for local scene descriptor");
    }
    let Some(diagnostic) = peer_state.last_solve_diagnostic else {
        return format!("AlignDbg {peer_label}: waiting for peer descriptor");
    };

    let samples = format!(
        "lw{} rw{} y{} p{}",
        diagnostic.local_wall_samples,
        diagnostic.remote_wall_samples,
        diagnostic.yaw_candidate_count,
        diagnostic.pose_candidate_count,
    );
    match diagnostic.outcome() {
        XrDepthAlignSolveOutcome::MissingSamples => {
            format!("AlignDbg {peer_label}: need samples {samples}")
        }
        XrDepthAlignSolveOutcome::NoCandidate => {
            format!(
                "AlignDbg {peer_label}: no candidate t{:.1}ms {samples}",
                peer_state.last_solve_ms,
            )
        }
        XrDepthAlignSolveOutcome::Rejected => {
            let Some(best) = diagnostic.best_solution else {
                return format!("AlignDbg {peer_label}: reject {samples}");
            };
            format!(
                "AlignDbg {peer_label}: reject c{:.2} m{} r{:.2} t{:.1}ms {samples}",
                best.confidence,
                best.matched_samples,
                best.residual_meters,
                peer_state.last_solve_ms,
            )
        }
        XrDepthAlignSolveOutcome::Accepted => {
            let Some(best) = diagnostic.best_solution else {
                return format!("AlignDbg {peer_label}: aligned {samples}");
            };
            format!(
                "AlignDbg {peer_label}: ok c{:.2} m{} r{:.2} t{:.1}ms yaw{:.0} tx{:.2} ty{:.2} tz{:.2}",
                best.confidence,
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
    last_alignment_debug_status: String,
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
    const PLANE_THICKNESS: f32 = 0.032;
    const ALIGN_WALL_BEAM_HEIGHT: f32 = 0.10;
    const ALIGN_WALL_BEAM_THICKNESS: f32 = 0.045;
    const ALIGN_WALL_BEAM_NORMAL_OFFSET: f32 = 0.05;
    const PLANE_MARKER_SIZE: Vec3f = Vec3f {
        x: 0.055,
        y: 0.055,
        z: 0.055,
    };

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
            self.last_alignment_debug_status =
                "AlignDbg: waiting for local scene descriptor".to_string();
            self.last_plane_scan_status = "PlaneScan: waiting for TSDF scan".to_string();
        } else {
            self.last_status = "Peers: off".to_string();
            self.last_network_status = "Network: off".to_string();
            self.last_alignment_debug_status = "AlignDbg: off".to_string();
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
            self.last_alignment_debug_status = "AlignDbg: off".to_string();
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

        self.last_status = if peer_count == 0 {
            "Peers: scanning LAN for clients".to_string()
        } else if self.local_descriptor.is_some() {
            format!("Peers: {peer_count} seen | {visible_count} state | {aligned_count} descriptor-solved")
        } else {
            format!("Peers: {peer_count} seen | {visible_count} state | waiting for local scene descriptor")
        };

        let last_event = if self.last_event_text.is_empty() {
            "none"
        } else {
            &self.last_event_text
        };
        self.last_network_status = format!(
            "Network: tx s{} d{} | rx j{} l{} s{} d{} | peers {} vis {} desc {} align {} | local desc {} | last {}",
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
            if self.local_descriptor.is_some() { "yes" } else { "no" },
            last_event,
        );
        self.last_plane_scan_status = self.local_plane_scan_text();
        if self.local_descriptor.is_none() {
            self.last_alignment_debug_status = self.local_descriptor_debug_text();
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

    fn local_descriptor_debug_text(&self) -> String {
        let debug = self.local_alignment_debug;
        if debug.near_surface_voxel_count == 0 {
            "AlignDbg: waiting for local scene descriptor".to_string()
        } else {
            format!(
                "AlignDbg: local near {} wall {}->{}",
                debug.near_surface_voxel_count, debug.wall_candidate_count, debug.wall_sample_count,
            )
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

    fn draw_local_plane_patches(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        for patch in self.local_plane_patches.clone() {
            let color = match patch.kind {
                XrDepthPlaneKind::Floor => vec4f(0.18, 0.96, 0.34, 0.42),
                XrDepthPlaneKind::Wall => vec4f(0.18, 0.66, 1.0, 0.36),
                XrDepthPlaneKind::Ceiling => vec4f(1.0, 0.62, 0.28, 0.32),
                XrDepthPlaneKind::Table => vec4f(0.92, 0.84, 0.18, 0.34),
                XrDepthPlaneKind::Unknown => vec4f(0.72, 0.72, 0.72, 0.24),
            };
            let center = patch.center + patch.normal.scale(0.024);
            let transform = Mat4f {
                v: [
                    patch.tangent.x,
                    patch.tangent.y,
                    patch.tangent.z,
                    0.0,
                    patch.normal.x,
                    patch.normal.y,
                    patch.normal.z,
                    0.0,
                    patch.bitangent.x,
                    patch.bitangent.y,
                    patch.bitangent.z,
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
                vec3(
                    patch.half_extent_tangent * 2.0,
                    Self::PLANE_THICKNESS,
                    patch.half_extent_bitangent * 2.0,
                ),
                color,
            );
            let marker_transform =
                Pose::new(Quat::default(), center + patch.normal.scale(0.03)).to_mat4();
            self.draw_cube_at(
                cx,
                world,
                &marker_transform,
                Self::PLANE_MARKER_SIZE,
                vec4f(color.x, color.y, color.z, 0.92),
            );
        }
    }

    fn draw_local_alignment_walls(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        for wall in self.local_alignment_wall_features.clone() {
            let center = vec3(
                wall.center.x + wall.normal.x * Self::ALIGN_WALL_BEAM_NORMAL_OFFSET,
                wall.max_y - Self::ALIGN_WALL_BEAM_HEIGHT * 0.5,
                wall.center.z + wall.normal.z * Self::ALIGN_WALL_BEAM_NORMAL_OFFSET,
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
                vec3(
                    wall.half_extent_along * 2.0,
                    Self::ALIGN_WALL_BEAM_HEIGHT,
                    Self::ALIGN_WALL_BEAM_THICKNESS,
                ),
                vec4f(1.0, 0.18, 0.14, 0.92),
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
        self.draw_local_plane_patches(cx, &world);
        self.draw_local_alignment_walls(cx, &world);
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

        let chosen =
            choose_stable_alignment_solution(Some(previous), &make_diagnostic(flipped)).unwrap();

        assert_eq!(chosen, previous);
    }

    #[test]
    fn stable_alignment_accepts_small_refinement() {
        let previous = make_solution(0.42, vec3(0.28, 0.0, -0.64), 0.28, 0.06, 3);
        let refined = make_solution(0.46, vec3(0.24, 0.0, -0.60), 0.35, 0.03, 4);

        let chosen =
            choose_stable_alignment_solution(Some(previous), &make_diagnostic(refined)).unwrap();

        assert_eq!(chosen, refined);
    }
}
