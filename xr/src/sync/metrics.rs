use super::*;

#[derive(Default)]
pub(super) struct XrPeerSyncMetrics {
    pub(super) tx_state_count: u64,
    pub(super) tx_descriptor_count: u64,
    pub(super) tx_activity_count: u64,
    pub(super) tx_body_spawn_count: u64,
    pub(super) tx_shared_object_state_count: u64,
    pub(super) tx_clock_ping_count: u64,
    pub(super) tx_clock_pong_count: u64,
    pub(super) rx_join_count: u64,
    pub(super) rx_leave_count: u64,
    pub(super) rx_state_count: u64,
    pub(super) rx_descriptor_count: u64,
    pub(super) rx_activity_count: u64,
    pub(super) rx_body_spawn_count: u64,
    pub(super) rx_shared_object_state_count: u64,
    pub(super) rx_clock_ping_count: u64,
    pub(super) rx_clock_pong_count: u64,
    pub(super) non_xr_draw_clock_count: u64,
    pub(super) remote_shadow_apply_count: u64,
    last_event_text: String,
}

impl XrPeerSyncMetrics {
    pub(super) fn record_node_started(&mut self) {
        self.last_event_text = "node started".to_string();
    }

    pub(super) fn record_join(&mut self, peer_id: XrNetPeerId) {
        self.rx_join_count = self.rx_join_count.saturating_add(1);
        self.last_event_text = format!("join {}", XrPeerSync::peer_label(peer_id));
    }

    pub(super) fn record_leave(&mut self, peer_id: XrNetPeerId) {
        self.rx_leave_count = self.rx_leave_count.saturating_add(1);
        self.last_event_text = format!("leave {}", XrPeerSync::peer_label(peer_id));
    }

    pub(super) fn record_state(&mut self, peer_id: XrNetPeerId, seq: u32) {
        self.rx_state_count = self.rx_state_count.saturating_add(1);
        self.last_event_text = format!("state {} seq {}", XrPeerSync::peer_label(peer_id), seq);
    }

    pub(super) fn record_descriptor(&mut self, peer_id: XrNetPeerId, seq: u32) {
        self.rx_descriptor_count = self.rx_descriptor_count.saturating_add(1);
        self.last_event_text = format!("desc {} seq {}", XrPeerSync::peer_label(peer_id), seq);
    }

    pub(super) fn record_activity_tx(&mut self, activity: XrNetActivityState) {
        self.tx_activity_count = self.tx_activity_count.saturating_add(1);
        self.last_event_text = format!(
            "tx activity {} tick {}",
            activity.activity_id.to_live_id().0,
            activity.changed_tick
        );
    }

    pub(super) fn record_activity_rx(&mut self, peer_id: XrNetPeerId, activity: XrNetActivityState) {
        self.rx_activity_count = self.rx_activity_count.saturating_add(1);
        self.last_event_text = format!(
            "activity {} {} tick {}",
            XrPeerSync::peer_label(peer_id),
            activity.activity_id.to_live_id().0,
            activity.changed_tick
        );
    }

    pub(super) fn record_body_spawn_tx(&mut self, spawn_label: u64) {
        self.tx_body_spawn_count = self.tx_body_spawn_count.saturating_add(1);
        self.last_event_text = format!("tx spawn {:016x}", spawn_label);
    }

    pub(super) fn record_shared_object_state_tx(&mut self, object_id: XrSharedObjectId, seq: u32) {
        self.tx_shared_object_state_count = self.tx_shared_object_state_count.saturating_add(1);
        self.last_event_text = format!("tx shared {:016x} seq {seq}", object_id.0);
    }

    pub(super) fn record_body_spawn_rx(&mut self, peer_id: XrNetPeerId, spawn_label: u64) {
        self.rx_body_spawn_count = self.rx_body_spawn_count.saturating_add(1);
        self.last_event_text = format!(
            "spawn {} {:016x}",
            XrPeerSync::peer_label(peer_id),
            spawn_label
        );
    }

    pub(super) fn record_shared_object_state_rx(
        &mut self,
        peer_id: XrNetPeerId,
        object_id: XrSharedObjectId,
        seq: u32,
    ) {
        self.rx_shared_object_state_count = self.rx_shared_object_state_count.saturating_add(1);
        self.last_event_text = format!(
            "shared {} {:016x} seq {seq}",
            XrPeerSync::peer_label(peer_id),
            object_id.0
        );
    }

    pub(super) fn record_clock_ping_tx(&mut self, seq: u32) {
        self.tx_clock_ping_count = self.tx_clock_ping_count.saturating_add(1);
        self.last_event_text = format!("tx clock ping {seq}");
    }

    pub(super) fn record_clock_ping_rx(&mut self, peer_id: XrNetPeerId, seq: u32) {
        self.rx_clock_ping_count = self.rx_clock_ping_count.saturating_add(1);
        self.last_event_text = format!("clock ping {} {seq}", XrPeerSync::peer_label(peer_id));
    }

    pub(super) fn record_clock_pong_tx(&mut self, seq: u32) {
        self.tx_clock_pong_count = self.tx_clock_pong_count.saturating_add(1);
        self.last_event_text = format!("tx clock pong {seq}");
    }

    pub(super) fn record_clock_pong_rx(&mut self, peer_id: XrNetPeerId, seq: u32) {
        self.rx_clock_pong_count = self.rx_clock_pong_count.saturating_add(1);
        self.last_event_text = format!("clock pong {} {seq}", XrPeerSync::peer_label(peer_id));
    }

    pub(super) fn record_non_xr_draw_clock(&mut self) {
        self.non_xr_draw_clock_count = self.non_xr_draw_clock_count.saturating_add(1);
    }

    pub(super) fn record_remote_shadow_apply(
        &mut self,
        object_id: XrSharedObjectId,
        seq: Option<u32>,
    ) {
        self.remote_shadow_apply_count = self.remote_shadow_apply_count.saturating_add(1);
        self.last_event_text = if let Some(seq) = seq {
            format!("shadow {:016x} seq {seq}", object_id.0)
        } else {
            format!("shadow {:016x}", object_id.0)
        };
    }

    pub(super) fn last_event_label(&self) -> &str {
        if self.last_event_text.is_empty() {
            "none"
        } else {
            &self.last_event_text
        }
    }
}
