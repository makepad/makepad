use crate::types::Id;
use crate::FastMap;

/// A row means the retained window image is stale. Keep the size the frame drew
/// because the shell may reuse a larger target across resizes.
pub(super) struct DirectFrames<I: Clone> {
    rows: FastMap<(u32, u32), (u32, Option<I>, [u32; 2])>,
}

impl<I: Clone> DirectFrames<I> {
    pub(super) fn new() -> Self {
        Self { rows: FastMap::default() }
    }

    pub(super) fn record(&mut self, (sid, id): (u32, Id), image: I, size: [u32; 2]) {
        self.rows.insert((sid, id.index), (id.gen, Some(image), size));
    }

    pub(super) fn newest(&self, (sid, id): (u32, Id)) -> Option<(I, [u32; 2])> {
        let (gen, image, size) = self.rows.get(&(sid, id.index))?;
        if *gen != id.gen { return None; }
        Some((image.clone()?, *size))
    }

    pub(super) fn stale(&self, key: (u32, u32)) -> bool {
        self.rows.contains_key(&key)
    }

    /// The shell is overwriting the target before the retained image could take it back.
    pub(super) fn drop_image(&mut self, key: (u32, u32)) {
        if let Some((_, image, _)) = self.rows.get_mut(&key) {
            *image = None;
        }
    }

    pub(super) fn take(&mut self, key: (u32, u32)) {
        self.rows.remove(&key);
    }

    pub(super) fn retained_is_newest(&mut self, key: (u32, u32)) {
        self.take(key);
    }

    pub(super) fn close_window(&mut self, key: (u32, u32)) {
        self.take(key);
    }

    pub(super) fn close_session(&mut self, sid: u32) {
        self.rows.retain(|key, _| key.0 != sid);
    }

    pub(super) fn reset(&mut self) {
        self.rows.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::HOST;

    #[test]
    fn newest_frames_follow_each_sessions_windows() {
        let mut frames = DirectFrames::new();
        let id = Id::new(3, 1);
        let host = (HOST, id);
        let app = (1, id);
        frames.record(host, 10u32, [20, 30]);
        frames.record(app, 11, [40, 50]);
        assert_eq!(frames.newest(app), Some((11, [40, 50])), "a non-host window reads its direct image and drawn size");

        frames.drop_image((1, id.index));
        assert!(frames.stale((1, id.index)), "dropping the direct image keeps the retained image stale");
        assert_eq!(frames.newest(app), None, "the overwritten direct image can no longer be read");
        frames.record(app, 18, [50, 40]);
        assert_eq!(frames.newest(app), Some((18, [50, 40])), "recording again restores the direct image and its size");

        frames.retained_is_newest((1, id.index));
        assert!(!frames.stale((1, id.index)), "a retained app frame releases its direct row");
        assert_eq!(frames.newest(host), Some((10, [20, 30])), "an app frame leaves the host's same-index frame alone");
        frames.record(app, 12, [60, 70]);
        frames.retained_is_newest((HOST, id.index));
        assert_eq!(frames.newest(app), Some((12, [60, 70])), "a host frame leaves the app's same-index frame alone");

        let reused = (1, Id::new(id.index, id.gen + 1));
        frames.record(reused, 13, [80, 90]);
        assert_eq!(frames.newest(app), None, "an old generation cannot read the reused slot");
        assert_eq!(frames.newest(reused), Some((13, [80, 90])), "the reused slot reads its new frame and size");

        let other = (1, Id::new(4, 1));
        frames.record(other, 14, [40, 30]);
        frames.record(host, 15, [30, 20]);
        frames.close_window((1, id.index));
        assert!(!frames.stale((1, id.index)), "closing a window removes its row");
        assert!(frames.stale((1, other.1.index)), "closing one window preserves its sibling");
        assert!(frames.stale((HOST, id.index)), "closing an app window preserves the same host slot");
        frames.record(app, 16, [20, 30]);
        frames.close_session(1);
        assert!(!frames.stale((1, id.index)) && !frames.stale((1, other.1.index)), "closing a session releases all its rows");
        assert!(frames.stale((HOST, id.index)), "closing a session preserves another session's row");
        frames.record(app, 17, [30, 40]);
        frames.reset();
        assert!(!frames.stale((HOST, id.index)) && !frames.stale((1, id.index)), "reset releases rows across sessions");
    }
}
