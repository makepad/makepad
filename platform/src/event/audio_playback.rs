use crate::makepad_live_id::LiveId;

#[derive(Clone, Debug)]
pub struct AudioPlaybackPreparedEvent {
    pub player_id: LiveId,
    pub duration_ms: i32,
    pub can_seek: bool,
    pub can_pause: bool,
    pub can_set_volume: bool,
}

#[derive(Clone, Debug)]
pub struct AudioPlaybackStartedEvent {
    pub player_id: LiveId,
}

#[derive(Clone, Debug)]
pub struct AudioPlaybackPausedEvent {
    pub player_id: LiveId,
}

#[derive(Clone, Debug)]
pub struct AudioPlaybackStoppedEvent {
    pub player_id: LiveId,
}

#[derive(Clone, Debug)]
pub struct AudioPlaybackCompletedEvent {
    pub player_id: LiveId,
}

#[derive(Clone, Debug)]
pub struct AudioPlaybackErrorEvent {
    pub player_id: LiveId,
    pub error: String,
}

#[derive(Clone, Debug)]
pub struct AudioPlaybackReleasedEvent {
    pub player_id: LiveId,
}

#[derive(Clone, Debug)]
pub struct AudioPlaybackTimeUpdateEvent { // If we decide to implement JNI path for this
    pub player_id: LiveId,
    pub current_time_ms: u64,
}
