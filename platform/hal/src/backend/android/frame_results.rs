//! Which camera image goes on next, and with which capture result (JNI-free, so it is tested on
//! any host). Camera2 gives an image and its capture result separately, in either order; they
//! match by sensor timestamp. An image waits for its result until the first of: its result, the
//! next image, its own capture's failure (whose result never comes), or its deadline (about two
//! frames after it was taken from the reader, on the HAL clock the worker polls with); then it
//! goes without one. Nothing here waits: the camera worker polls every few milliseconds.

use std::collections::VecDeque;

/// The image waiting for its capture result: its sensor timestamp, and when it may wait until
/// (HAL clock).
pub struct FrameResults<I> {
    pending: Option<(I, i64, u64)>,
}

impl<I> Default for FrameResults<I> {
    fn default() -> Self { Self { pending: None } }
}

/// What a poll knows besides the images: the HAL clock now, and how long an image may wait for
/// its result (about two frames).
#[derive(Clone, Copy)]
pub struct Poll {
    pub now_ns: u64,
    pub wait_ns: u64,
}

/// Which images' results never come. A capture failure names its frame number, an image has a
/// sensor timestamp: the capture-start callbacks tie the two (each frame number's timestamp), so
/// a failure lets go of its own image only. A failure whose start never came had no exposure,
/// so no image of it waits. A few frames are kept: an image waits two at most.
#[derive(Default)]
pub struct FrameFailures {
    started: VecDeque<(i64, i64)>,
    failed: VecDeque<i64>,
}

const FRAMES_KEPT: usize = 8;

impl FrameFailures {
    /// Frame `frame`'s exposure started at sensor time `time`.
    pub fn started(&mut self, frame: i64, time: i64) {
        self.started.push_back((frame, time));
        while self.started.len() > FRAMES_KEPT { self.started.pop_front(); }
    }

    /// Frame `frame`'s capture failed: its result never comes.
    pub fn failed(&mut self, frame: i64) {
        let Some(&(_, time)) = self.started.iter().rev().find(|(f, _)| *f == frame) else { return };
        self.failed.push_back(time);
        while self.failed.len() > FRAMES_KEPT { self.failed.pop_front(); }
    }

    /// Whether the capture of the image taken at `time` failed.
    pub fn has_failed(&self, time: i64) -> bool { self.failed.contains(&time) }
}

impl<I> FrameResults<I> {
    /// The image to hand on now, with its sensor timestamp and its result (None: it went
    /// without), or None when none is due. `result(time)`: the result of that timestamp, once
    /// it came; `failed(time)`: whether that timestamp's capture failed (its result never
    /// comes). `acquire()`: the reader's newest image when a new one came; called only when no
    /// waiting image goes this poll.
    pub fn next<R, E>(&mut self, poll: Poll, result: impl Fn(i64) -> Option<R>, failed: impl Fn(i64) -> bool, acquire: impl FnOnce() -> Result<Option<(I, i64)>, E>) -> Result<Option<(I, i64, Option<R>)>, E> {
        if let Some((image, time, until)) = self.pending.take() {
            match result(time) {
                Some(r) => return Ok(Some((image, time, Some(r)))),
                None if failed(time) || poll.now_ns >= until => return Ok(Some((image, time, None))),
                None => self.pending = Some((image, time, until)),
            }
        }
        let Some((image, time)) = acquire()? else { return Ok(None) };
        let until = poll.now_ns.saturating_add(poll.wait_ns);
        if let Some((older, older_time, _)) = self.pending.replace((image, time, until)) {
            return Ok(Some((older, older_time, None)));
        }
        match result(time) {
            Some(r) => Ok(self.pending.take().map(|(image, time, _)| (image, time, Some(r)))),
            None if failed(time) => Ok(self.pending.take().map(|(image, time, _)| (image, time, None))),
            None => Ok(None),
        }
    }

    /// Lets go of the waiting image (before its reader goes).
    pub fn clear(&mut self) { self.pending = None; }
}

#[cfg(test)]
mod tests {
    use super::{FrameFailures, FrameResults, Poll};
    use std::collections::{HashSet, VecDeque};

    const FRAME_NS: u64 = 33_000_000;

    /// Polls with the results that came so far and the images still to come (one per poll), at
    /// `now_ns`.
    struct Feed { results: HashSet<i64>, images: VecDeque<i64>, acquires: usize, now_ns: u64, failures: FrameFailures }
    impl Feed {
        fn new(results: impl IntoIterator<Item = i64>, images: impl IntoIterator<Item = i64>) -> Feed {
            Feed { results: results.into_iter().collect(), images: images.into_iter().collect(), acquires: 0, now_ns: 0, failures: FrameFailures::default() }
        }
        fn poll(&mut self, m: &mut FrameResults<u32>) -> Option<(u32, i64, Option<i64>)> {
            let (results, failures) = (&self.results, &self.failures);
            let (images, acquires) = (&mut self.images, &mut self.acquires);
            let poll = Poll { now_ns: self.now_ns, wait_ns: 2 * FRAME_NS };
            m.next(poll, |t| results.contains(&t).then_some(t), |t| failures.has_failed(t), || -> Result<_, ()> {
                *acquires += 1;
                Ok(images.pop_front().map(|t| (t as u32, t)))
            }).unwrap()
        }
    }

    #[test]
    fn camera_image_waits_for_its_result_only_until_the_next_image() {
        // the result first: the image goes at once, with it
        let mut m = FrameResults::default();
        let mut f = Feed::new([10], [10]);
        assert_eq!(f.poll(&mut m), Some((10, 10, Some(10))));
        // the image first: it waits; its result comes; it goes without a new image
        let mut f = Feed::new([], [20]);
        assert_eq!(f.poll(&mut m), None);
        f.results.insert(20);
        assert_eq!(f.poll(&mut m), Some((20, 20, Some(20))));
        assert_eq!(f.acquires, 1, "a waiting image whose result came needs no new image");
        // a result that never comes: the image waits through polls without images, never
        // blocking, and goes (without a result) when the next image comes; that one goes with
        // its own result at the next poll
        let mut f = Feed::new([31], [30]);
        assert_eq!(f.poll(&mut m), None);
        assert_eq!(f.poll(&mut m), None);
        f.images.push_back(31);
        assert_eq!(f.poll(&mut m), Some((30, 30, None)));
        assert_eq!(f.poll(&mut m), Some((31, 31, Some(31))));
        assert_eq!(f.poll(&mut m), None);
    }

    #[test]
    fn camera_image_without_result_goes_at_its_deadline_or_a_failure() {
        // frames stop and the result never comes: the image goes two frames after it was taken
        let mut m = FrameResults::default();
        let mut f = Feed::new([], [40]);
        f.now_ns = 1_000_000_000;
        assert_eq!(f.poll(&mut m), None);
        f.now_ns += 2 * FRAME_NS - 1;
        assert_eq!(f.poll(&mut m), None);
        f.now_ns += 1;
        assert_eq!(f.poll(&mut m), Some((40, 40, None)));
        assert_eq!(f.poll(&mut m), None, "nothing left waiting");
        // its own capture's failure (its result never comes): the waiting image goes at once
        let mut f = Feed::new([], [50]);
        f.now_ns = 2_000_000_000;
        f.failures.started(7, 50);
        assert_eq!(f.poll(&mut m), None);
        f.failures.failed(7);
        assert_eq!(f.poll(&mut m), Some((50, 50, None)));
        // a result that comes before the deadline still goes with its image
        let mut f = Feed::new([], [60]);
        f.now_ns = 3_000_000_000;
        assert_eq!(f.poll(&mut m), None);
        f.now_ns += FRAME_NS;
        f.results.insert(60);
        assert_eq!(f.poll(&mut m), Some((60, 60, Some(60))));
    }

    #[test]
    fn camera_images_without_results_still_go_in_order() {
        // a device that never gives results: each image goes when the next comes, in order,
        // none lost, none held past one image
        let mut m = FrameResults::default();
        let mut f = Feed::new([], 1..=100);
        let mut out = Vec::new();
        for _ in 0..100 {
            if let Some((image, time, result)) = f.poll(&mut m) {
                assert_eq!((image as i64, result), (time, None));
                out.push(time);
            }
        }
        assert_eq!(out, (1..=99).collect::<Vec<_>>());
        // results for some images only: every image still goes once, in order
        let mut m = FrameResults::default();
        let mut f = Feed::new((1..=100).filter(|t| t % 3 != 0), 1..=100);
        let mut out = Vec::new();
        for _ in 0..200 {
            if let Some((_, time, result)) = f.poll(&mut m) {
                assert_eq!(result.is_some(), time % 3 != 0, "image {time}");
                out.push(time);
            }
        }
        assert_eq!(out, (1..=100).collect::<Vec<_>>());
    }

    #[test]
    fn camera_failure_lets_go_of_its_own_image_only() {
        // another frame's failure: the waiting image keeps waiting for its result
        let mut m = FrameResults::default();
        let mut f = Feed::new([], [100]);
        f.failures.started(1, 99);
        f.failures.started(2, 100);
        assert_eq!(f.poll(&mut m), None);
        f.failures.failed(1);
        assert_eq!(f.poll(&mut m), None, "frame 1 failed, the image is frame 2's");
        f.results.insert(100);
        assert_eq!(f.poll(&mut m), Some((100, 100, Some(100))));
        // a failure that names a frame whose start never came (no exposure, no image) frees nothing
        let mut m = FrameResults::default();
        let mut f = Feed::new([], [200]);
        f.failures.started(3, 200);
        assert_eq!(f.poll(&mut m), None);
        f.failures.failed(4);
        assert_eq!(f.poll(&mut m), None);
        // a failure before its image is taken from the reader: the image goes at once
        let mut m = FrameResults::default();
        let mut f = Feed::new([], [300]);
        f.failures.started(5, 300);
        f.failures.failed(5);
        assert_eq!(f.poll(&mut m), Some((300, 300, None)));
        // only a few frames are kept: an old failure is forgotten, never matched later
        let mut failures = FrameFailures::default();
        for frame in 0..20 { failures.started(frame, 1000 + frame); }
        failures.failed(0);
        failures.failed(19);
        assert!(!failures.has_failed(1000) && failures.has_failed(1019));
    }

    #[test]
    fn camera_reader_error_keeps_the_waiting_image() {
        let mut m: FrameResults<u32> = FrameResults::default();
        let poll = Poll { now_ns: 0, wait_ns: 2 * FRAME_NS };
        assert_eq!(m.next(poll, |_| None::<()>, |_| false, || Ok::<_, ()>(Some((5, 5)))), Ok(None));
        assert_eq!(m.next(poll, |_| None::<()>, |_| false, || Err(())), Err(()));
        assert_eq!(m.next(poll, |t| (t == 5).then_some(()), |_| false, || Ok::<_, ()>(None)), Ok(Some((5, 5, Some(())))));
    }
}
