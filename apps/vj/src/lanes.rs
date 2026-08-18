//! Media-lane planning: which bounded runtime serves which fetch, and the
//! cancel-on-supersede bookkeeping for latest-click-wins lanes.
//!
//! Four session lanes keep classes of media from starving each other:
//! video cues alternate between TWO lanes (a fresh click starts its
//! transfer immediately while the superseded one aborts), decks+SFX share
//! an audio lane a video download can never block, and the mesh program has
//! its own. Pure and clock-free; `main.rs` maps decisions onto the session
//! runtimes.

pub const VIDEO_LANE_A: usize = 0;
pub const VIDEO_LANE_B: usize = 1;
pub const AUDIO_LANE: usize = 2;
pub const MESH_LANE: usize = 3;
pub const LANE_COUNT: usize = 4;

/// The lane leaf names handed to `SessionConfig::media_lanes` (index ==
/// lane id; each is its own single-owner bounded cache root).
pub fn lane_leaves() -> Vec<String> {
    vec![
        "media-video-a".to_string(),
        "media-video-b".to_string(),
        "media-audio".to_string(),
        "media-mesh".to_string(),
    ]
}

/// One in-flight latest-wins fetch: `(lane, request_id, generation)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InflightFetch {
    pub lane: usize,
    pub request: u64,
    pub gen: u64,
}

/// Latest-click-wins fetch planner for one media class (video cues, mesh).
/// `begin` picks the lane (alternating when two are configured) and reports
/// which older request must be cancelled; completions clear the slot only
/// when they belong to the current generation.
#[derive(Debug)]
pub struct LatestWins {
    lanes: Vec<usize>,
    next: usize,
    inflight: Option<InflightFetch>,
}

impl LatestWins {
    pub fn new(lanes: Vec<usize>) -> LatestWins {
        assert!(!lanes.is_empty());
        LatestWins { lanes, next: 0, inflight: None }
    }

    /// The video-cue planner over both alternating video lanes.
    pub fn video() -> LatestWins {
        Self::new(vec![VIDEO_LANE_A, VIDEO_LANE_B])
    }

    /// The mesh-program planner (single lane, still latest-wins).
    pub fn mesh() -> LatestWins {
        Self::new(vec![MESH_LANE])
    }

    /// Start a new fetch generation: returns `(lane_to_use, to_cancel)`.
    /// The superseded in-flight request (if any) must be cancelled on ITS
    /// lane; the new fetch goes to the next lane in rotation, which is by
    /// construction not the one still winding down when two are configured.
    pub fn begin(&mut self) -> (usize, Option<InflightFetch>) {
        let cancel = self.inflight.take();
        let lane = self.lanes[self.next % self.lanes.len()];
        self.next = (self.next + 1) % self.lanes.len();
        (lane, cancel)
    }

    /// Record the submitted request id for the generation begun above.
    pub fn submitted(&mut self, lane: usize, request: u64, gen: u64) {
        self.inflight = Some(InflightFetch { lane, request, gen });
    }

    /// A completion/failure arrived on `lane` for `request`. Returns true
    /// when it was the CURRENT fetch (callers act on it); stale completions
    /// return false and must be ignored.
    pub fn finished(&mut self, lane: usize, request: u64) -> bool {
        match self.inflight {
            Some(cur) if cur.lane == lane && cur.request == request => {
                self.inflight = None;
                true
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub fn inflight(&self) -> Option<InflightFetch> {
        self.inflight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_lanes_alternate_and_cancel_the_superseded_fetch() {
        let mut plan = LatestWins::new(vec![VIDEO_LANE_A, VIDEO_LANE_B]);
        let (lane1, cancel) = plan.begin();
        assert_eq!(lane1, VIDEO_LANE_A);
        assert!(cancel.is_none());
        plan.submitted(lane1, 10, 1);
        // A rapid second click: goes to the OTHER lane and cancels click 1.
        let (lane2, cancel) = plan.begin();
        assert_eq!(lane2, VIDEO_LANE_B);
        assert_eq!(cancel, Some(InflightFetch { lane: VIDEO_LANE_A, request: 10, gen: 1 }));
        plan.submitted(lane2, 4, 2);
        // The CANCELLED fetch's completion is stale; the winner's is not.
        assert!(!plan.finished(VIDEO_LANE_A, 10));
        assert!(plan.finished(VIDEO_LANE_B, 4));
        assert!(plan.inflight().is_none());
        // Third click rotates back to lane A (now free).
        let (lane3, cancel) = plan.begin();
        assert_eq!(lane3, VIDEO_LANE_A);
        assert!(cancel.is_none());
    }

    #[test]
    fn single_lane_class_still_supersedes() {
        let mut plan = LatestWins::new(vec![MESH_LANE]);
        let (lane, _) = plan.begin();
        plan.submitted(lane, 7, 1);
        let (lane2, cancel) = plan.begin();
        assert_eq!(lane2, MESH_LANE);
        assert_eq!(cancel.map(|c| c.request), Some(7));
    }

    #[test]
    fn audio_lane_is_disjoint_from_video_and_mesh_lanes() {
        // The fairness invariant the session config encodes: deck/SFX media
        // never share a queue with video or mesh transfers.
        let leaves = lane_leaves();
        assert_eq!(leaves.len(), LANE_COUNT);
        for lane in [VIDEO_LANE_A, VIDEO_LANE_B, MESH_LANE] {
            assert_ne!(lane, AUDIO_LANE);
        }
        // Leaves are distinct single-owner roots.
        let mut sorted = leaves.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), leaves.len());
    }
}
