use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LocalSceneState {
    Missing,
    PublishPending,
    Ready,
}

pub(super) fn transform_source_label(source: RemoteTransformSource) -> &'static str {
    match source {
        RemoteTransformSource::Raw => "raw",
        RemoteTransformSource::Anchor => "anchor",
        RemoteTransformSource::Descriptor => "solved",
    }
}

pub(super) fn bool_digit(value: bool) -> char {
    if value {
        '1'
    } else {
        '0'
    }
}

pub(super) fn sync_extrema_label(extrema: XrSyncAnchorExtrema) -> &'static str {
    match extrema {
        XrSyncAnchorExtrema::Low => "low",
        XrSyncAnchorExtrema::High => "high",
    }
}

fn option_f32_text(value: Option<f32>, decimals: usize) -> String {
    value
        .map(|value| format!("{value:.decimals$}"))
        .unwrap_or_else(|| "--".to_string())
}

fn finger_name(index: usize) -> &'static str {
    match index {
        0 => "idx",
        1 => "mid",
        2 => "rng",
        3 => "lit",
        _ => "?",
    }
}

pub(super) fn finger_pass_bits_text(fingers: &[Option<f32>; 4]) -> String {
    fingers
        .iter()
        .map(|finger| {
            bool_digit(finger.is_some_and(|bend| bend <= XrHand::OPEN_MAX_FINGER_BEND_DEGREES))
        })
        .collect()
}

pub(super) fn finger_bends_text(fingers: &[Option<f32>; 4]) -> String {
    fingers
        .iter()
        .map(|finger| option_f32_text(*finger, 0))
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn average_bend_text(fingers: &[Option<f32>; 4]) -> String {
    let bends = fingers
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()
        .map(|bends| bends.into_iter().sum::<f32>() / fingers.len() as f32);
    option_f32_text(bends, 0)
}

pub(super) fn open_fail_reason(fingers: &[Option<f32>; 4]) -> String {
    let mut reasons = Vec::new();
    for (index, finger) in fingers.iter().enumerate() {
        if finger.is_none() {
            reasons.push(format!("{}:bend?", finger_name(index)));
        } else if finger.is_some_and(|bend| bend > XrHand::OPEN_MAX_FINGER_BEND_DEGREES) {
            reasons.push(format!("{}:bend", finger_name(index)));
        }
    }
    if let Some(average_bend) = fingers
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()
        .map(|bends| bends.into_iter().sum::<f32>() / fingers.len() as f32)
    {
        if average_bend > XrHand::OPEN_MAX_AVERAGE_FINGER_BEND_DEGREES {
            reasons.push("avg".to_string());
        }
    } else {
        reasons.push("avg?".to_string());
    }
    if reasons.is_empty() {
        "ok".to_string()
    } else {
        reasons.join(",")
    }
}

pub(super) fn descriptor_version_label(version: Option<(u64, u64)>) -> String {
    version
        .map(|(mesh_generation, update_sequence)| format!("{mesh_generation}/{update_sequence}"))
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn descriptor_seq_label(seq: Option<u32>) -> String {
    seq.map(|seq| seq.to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn descriptor_contour_sample_count(descriptor: &XrDepthAlignDescriptor) -> usize {
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

pub(super) fn descriptor_height_map_status(descriptor: &XrDepthAlignDescriptor) -> String {
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

pub(super) fn make_alignment_state_text(
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

pub(super) fn make_peer_scene_debug_text(
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

pub(super) fn make_pending_alignment_debug_text(
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

pub(super) fn make_alignment_debug_text(
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
pub(super) struct XrPeerSyncDiagnostics {
    pub(super) status: String,
    pub(super) network_status: String,
    pub(super) peer_scene_status: String,
    pub(super) alignment_debug_status: String,
    pub(super) alignment_state_status: String,
}

impl XrPeerSyncDiagnostics {
    pub(super) fn status_text(&self) -> &str {
        if self.status.is_empty() {
            "AlignSync: off"
        } else {
            &self.status
        }
    }

    pub(super) fn network_status_text(&self) -> &str {
        if self.network_status.is_empty() {
            "Network: off"
        } else {
            &self.network_status
        }
    }

    pub(super) fn alignment_debug_text(&self) -> &str {
        if self.alignment_debug_status.is_empty() {
            "AlignDbg: off"
        } else {
            &self.alignment_debug_status
        }
    }

    pub(super) fn alignment_state_text(&self) -> &str {
        if self.alignment_state_status.is_empty() {
            "AlignState: off"
        } else {
            &self.alignment_state_status
        }
    }

    pub(super) fn peer_scene_text(&self) -> &str {
        if self.peer_scene_status.is_empty() {
            "PeerMap: off"
        } else {
            &self.peer_scene_status
        }
    }

    pub(super) fn set_enabled_defaults(
        &mut self,
        auto_alignment_enabled: bool,
        network_ready: bool,
    ) {
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

    pub(super) fn set_disabled(&mut self) {
        self.status = "AlignSync: off".to_string();
        self.network_status = "Network: off".to_string();
        self.peer_scene_status = "PeerMap: off".to_string();
        self.alignment_debug_status = "AlignDbg: off".to_string();
        self.alignment_state_status = "AlignState: off".to_string();
    }

    pub(super) fn set_network_bind_failed(&mut self, err: &str) {
        self.status = format!("AlignSync: network bind failed ({err})");
        self.network_status = format!("Network: bind failed ({err})");
    }

    pub(super) fn set_network_disconnected(&mut self) {
        self.status = "AlignSync: network worker disconnected, retrying".to_string();
        self.network_status = "Network: worker disconnected".to_string();
    }
}
