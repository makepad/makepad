//! The host API of the hardware stream decoders: what `makepad_video::VideoStreamDecoder` drives
//! on Android (MediaCodec, `backend::android::media::stream_decode`). JIT-compiled code reaches
//! the phone's decoders only through these functions. A decoder is named by a number; a
//! failure's text is the calling thread's last error (`hal_stream_decoder_error`).

#[cfg(target_os = "android")]
use crate::backend::android::media::stream_decode::{Picture, StreamDecoder};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
#[cfg(any(target_os = "android", test))]
use std::time::{Duration, Instant};

/// Flag of `hal_stream_decoder_open`: H.265 instead of H.264.
pub const HEVC: u32 = 2;
/// Flag of `hal_stream_decoder_open`: pictures stay on the GPU (a picture is read as a texture
/// import ticket, `texture_handle::VIDEO`, released with `hal_stream_decoder_release`).
pub const GPU: u32 = 4;

/// GPU pictures read and not released, by ticket: the decoder's image stays out of its reader
/// (its buffer is not decoded into again) until the ticket is released.
#[cfg(target_os = "android")]
static HELD: Mutex<Option<HashMap<u64, crate::backend::android::media::stream_decode::GpuPicture>>> = Mutex::new(None);

#[cfg(target_os = "android")]
struct Open {
    decoder: StreamDecoder,
    /// Pictures decoded and not read yet, oldest first.
    out: VecDeque<Picture>,
}
#[cfg(not(target_os = "android"))]
struct Open {
    out: VecDeque<()>,
}

static DECODERS: Mutex<Option<HashMap<u64, Arc<Mutex<Open>>>>> = Mutex::new(None);
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// (`hal::end_run`) The ended run's decoders and their pictures.
pub(crate) fn end_run() {
    let open = crate::lock(&DECODERS).take();
    drop(open);
}

fn get(decoder: u64) -> Option<Arc<Mutex<Open>>> {
    crate::lock(&DECODERS).as_ref()?.get(&decoder).cloned()
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn failed(error: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = error);
}

/// The text of this thread's last failed stream-decoder op (its first `out.len()` bytes); its
/// length is returned.
#[relax_api::relax_api]
pub fn hal_stream_decoder_error(out: &mut [u8]) -> u64 {
    LAST_ERROR.with(|e| {
        let e = e.borrow();
        let n = e.len().min(out.len());
        out[..n].copy_from_slice(&e.as_bytes()[..n]);
        e.len() as u64
    })
}

/// Opens a decoder for a stream of about `width` x `height` pictures (`flags`: `HEVC`). Its
/// number, 0 when this machine cannot (the error says why).
#[relax_api::relax_api]
pub fn hal_stream_decoder_open(width: u32, height: u32, flags: u32) -> u64 {
    #[cfg(target_os = "android")]
    let opened = StreamDecoder::open(width, height, flags & HEVC != 0, flags & GPU != 0).map(|decoder| Open { decoder, out: VecDeque::new() });
    #[cfg(not(target_os = "android"))]
    let opened: Result<Open, String> = {
        let _ = (width, height, flags);
        Err("the HAL stream decoder is Android's".into())
    };
    match opened {
        Ok(open) => {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::lock(&DECODERS).get_or_insert_with(HashMap::new).insert(id, Arc::new(Mutex::new(open)));
            id
        }
        Err(e) => {
            failed(e);
            0
        }
    }
}

/// The decoder's name (its first `out.len()` bytes); its length.
#[relax_api::relax_api]
pub fn hal_stream_decoder_name(decoder: u64, out: &mut [u8]) -> u64 {
    let Some(open) = get(decoder) else { return 0 };
    #[cfg(target_os = "android")]
    let name = crate::lock(&open).decoder.info.clone();
    #[cfg(not(target_os = "android"))]
    let name = { let _ = open; String::new() };
    let n = name.len().min(out.len());
    out[..n].copy_from_slice(&name.as_bytes()[..n]);
    name.len() as u64
}

/// Result of a non-blocking pass over the decoder's image reader.
#[cfg(any(target_os = "android", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReaderStop {
    Done,
    NoBuffer,
    FencePending,
    MaxAcquired,
}

#[cfg(any(target_os = "android", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FenceWait {
    Signaled(Duration),
    TimedOut,
}

/// Frames in surface release order, plus the single acquired image waiting on its fence.
#[cfg(any(target_os = "android", test))]
pub(crate) struct PendingGpu<A = ()> {
    entries: VecDeque<(i64, Instant)>,
    acquired: VecDeque<(A, i64, Instant)>,
}

#[cfg(any(target_os = "android", test))]
impl<A> Default for PendingGpu<A> {
    fn default() -> Self {
        Self { entries: VecDeque::new(), acquired: VecDeque::new() }
    }
}

#[cfg(any(target_os = "android", test))]
impl<A> PendingGpu<A> {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.acquired.is_empty()
    }

    pub(crate) fn has_released(&self) -> bool {
        !self.entries.is_empty()
    }

    pub(crate) fn push(&mut self, pts_us: i64, released_at: Instant) {
        self.entries.push_back((pts_us, released_at));
    }

    pub(crate) fn front_acquired(&self) -> Option<&A> {
        self.acquired.front().map(|(image, _, _)| image)
    }

    pub(crate) fn hold_acquired(&mut self, image: A, pts_us: i64, released_at: Instant) {
        debug_assert!(self.acquired.is_empty());
        self.acquired.push_back((image, pts_us, released_at));
    }

    pub(crate) fn take_acquired(&mut self) -> Option<(A, i64, Instant)> {
        self.acquired.pop_front()
    }

    pub(crate) fn fresh_wait(&self, now: Instant, max_wait: Duration) -> Duration {
        let released = self.entries.back().map(|entry| entry.1);
        let acquired = self.acquired.back().map(|entry| entry.2);
        match released.max(acquired) {
            Some(latest) => max_wait.saturating_sub(now.saturating_duration_since(latest)),
            None => Duration::ZERO,
        }
    }

    pub(crate) fn match_image(&mut self, image_pts_us: i64) -> Option<(i64, Instant)> {
        if let Some(pos) = self.entries.iter().position(|&(pts, _)| pts == image_pts_us) {
            // The surface may have skipped earlier releases. Pick the first exact match.
            let matched = self.entries[pos];
            self.entries.drain(..=pos);
            return Some(matched);
        }
        if image_pts_us < self.entries.front()?.0 {
            return None; // An older image left in the reader.
        }
        // Some drivers stamp a different time on the buffer. Preserve the released PTS.
        self.entries.pop_front()
    }
}

/// A zero budget tries once. Waiting pushes share one budget across reader retries and fences.
/// At push entry, `no_buffer_budget` limits how long to wait for an old release to reach the
/// reader. A pending fence or a full reader can still use the whole remaining budget.
#[cfg(any(target_os = "android", test))]
pub(crate) fn drain_reader_with_budget<C, E>(
    ctx: &mut C,
    budget: &mut Duration,
    no_buffer_budget: Duration,
    step_max: Duration,
    stop_on_progress: bool,
    mut try_drain: impl FnMut(&mut C) -> Result<(ReaderStop, usize), E>,
    mut sleep: impl FnMut(Duration) -> Duration,
    mut wait_fence: impl FnMut(&mut C, Duration) -> Result<FenceWait, E>,
) -> Result<ReaderStop, E> {
    let mut no_buffer_left = no_buffer_budget;
    loop {
        let (stop, acquired) = try_drain(ctx)?;
        if budget.is_zero() || (stop_on_progress && acquired > 0) {
            return Ok(stop);
        }
        let spent = match stop {
            ReaderStop::Done => return Ok(stop),
            ReaderStop::FencePending => match wait_fence(ctx, *budget)? {
                FenceWait::Signaled(elapsed) => elapsed,
                FenceWait::TimedOut => {
                    *budget = Duration::ZERO;
                    return Ok(stop);
                }
            },
            ReaderStop::MaxAcquired => {
                let step = step_max.min(*budget);
                sleep(step).max(step)
            }
            ReaderStop::NoBuffer => {
                let cap = no_buffer_left.min(*budget);
                if cap.is_zero() {
                    return Ok(stop);
                }
                let step = step_max.min(cap);
                sleep(step).max(step)
            }
        };
        *budget = budget.saturating_sub(spent);
        no_buffer_left = no_buffer_left.saturating_sub(spent);
    }
}

#[cfg(any(target_os = "android", test))]
fn collect_into<T>(out: &mut VecDeque<T>, result: Result<Vec<T>, String>) -> i64 {
    match result {
        Ok(pictures) => {
            out.extend(pictures);
            out.len() as i64
        }
        Err(error) => {
            failed(error);
            -1
        }
    }
}

/// Queues one Annex-B access unit with presentation time `pts_us` and collects the pictures
/// decoded, waiting up to `wait_us` for one. The number of pictures waiting to be read, -1 when
/// it failed.
#[relax_api::relax_api]
pub fn hal_stream_decoder_push(decoder: u64, annex_b: &[u8], pts_us: i64, wait_us: u32) -> i64 {
    let Some(open) = get(decoder) else {
        failed("no such decoder".into());
        return -1;
    };
    let mut open = crate::lock(&open);
    #[cfg(target_os = "android")]
    {
        let result = open.decoder.push(annex_b, pts_us, Duration::from_micros(wait_us as u64));
        collect_into(&mut open.out, result)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (annex_b, pts_us, wait_us, &mut open);
        -1
    }
}

/// Collects any pictures the decoder has finished without waiting. The number of pictures
/// waiting to be read, -1 when it failed.
#[relax_api::relax_api]
pub fn hal_stream_decoder_poll(decoder: u64) -> i64 {
    let Some(open) = get(decoder) else {
        failed("no such decoder".into());
        return -1;
    };
    #[cfg(target_os = "android")]
    {
        let mut open = crate::lock(&open);
        let result = open.decoder.pictures(Duration::ZERO);
        collect_into(&mut open.out, result)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = open;
        failed("the HAL stream decoder is Android's".into());
        -1
    }
}

/// The oldest picture waiting: a 24-byte header (width u32, height u32, pts_us i64, ticket u64)
/// then, for a CPU picture (ticket 0), its tightly packed NV12, into `out`. A GPU picture's
/// ticket names it to the texture import (`texture_handle::VIDEO`) until it is released
/// (`hal_stream_decoder_release`). Its length; 0 when none waits, more than `out.len()` when
/// `out` is short (the picture stays).
#[relax_api::relax_api]
pub fn hal_stream_decoder_read(decoder: u64, out: &mut [u8]) -> u64 {
    let Some(open) = get(decoder) else { return 0 };
    let mut open = crate::lock(&open);
    #[cfg(target_os = "android")]
    {
        let Some(p) = open.out.front() else { return 0 };
        let need = 24 + p.nv12.len() as u64;
        if (out.len() as u64) < need {
            return need;
        }
        let p = open.out.pop_front().unwrap();
        let ticket = match p.gpu {
            Some(gpu) => match crate::backend::vulkan::picture_ticket(gpu.buffer) {
                Ok(ticket) => {
                    crate::lock(&HELD).get_or_insert_with(HashMap::new).insert(ticket, gpu);
                    ticket
                }
                Err(e) => {
                    failed(e);
                    return 0;
                }
            },
            None => 0,
        };
        out[0..4].copy_from_slice(&p.width.to_le_bytes());
        out[4..8].copy_from_slice(&p.height.to_le_bytes());
        out[8..16].copy_from_slice(&p.pts_us.to_le_bytes());
        out[16..24].copy_from_slice(&ticket.to_le_bytes());
        out[24..24 + p.nv12.len()].copy_from_slice(&p.nv12);
        need
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (out, open.out.pop_front());
        0
    }
}

/// A GPU picture is done with (every frame that drew it finished): its image goes back to the
/// decoder.
#[relax_api::relax_api]
pub fn hal_stream_decoder_release(ticket: u64) {
    #[cfg(target_os = "android")]
    {
        let held = crate::lock(&HELD).as_mut().and_then(|h| h.remove(&ticket));
        crate::backend::vulkan::release_picture_ticket(ticket);
        drop(held);
    }
    #[cfg(not(target_os = "android"))]
    let _ = ticket;
}

#[relax_api::relax_api]
pub fn hal_stream_decoder_close(decoder: u64) {
    let open = crate::lock(&DECODERS).as_mut().and_then(|d| d.remove(&decoder));
    drop(open);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_last_error(expected: &str) {
        let mut buf = [0; 128];
        let n = hal_stream_decoder_error(&mut buf) as usize;
        assert_eq!(&buf[..n], expected.as_bytes());
    }

    #[test]
    fn hal_stream_decoder_poll_reports_missing_decoder() {
        for id in [0, u64::MAX] {
            assert_eq!(hal_stream_decoder_poll(id), -1);
            assert_last_error("no such decoder");
        }
    }

    #[test]
    fn collect_into_appends_fifo_and_records_errors() {
        let mut q = VecDeque::<u32>::new();
        assert_eq!(collect_into(&mut q, Ok(vec![])), 0);
        assert!(q.is_empty());
        assert_eq!(collect_into(&mut q, Ok(vec![10, 20])), 2);
        assert_eq!(q, [10, 20]);
        assert_eq!(collect_into(&mut q, Ok(vec![30])), 3);
        assert_eq!(q, [10, 20, 30]);
        let error = "MediaCodec: dequeue output buffer: -10000";
        assert_eq!(collect_into(&mut q, Err(error.into())), -1);
        assert_last_error(error);
        assert_eq!(q, [10, 20, 30]);
    }

    #[test]
    fn pending_gpu_matches_fifo_skips_dropped_and_drops_older_orphans() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut pending = PendingGpu::<u32>::default();
        assert!(pending.is_empty());
        assert_eq!(pending.match_image(100), None);
        assert_eq!(pending.fresh_wait(t0, ms(20)), Duration::ZERO);

        pending.push(100, t0);
        pending.push(200, t0 + ms(5));
        assert_eq!(pending.match_image(50), None);
        assert_eq!(pending.match_image(100), Some((100, t0)));
        assert_eq!(pending.match_image(200), Some((200, t0 + ms(5))));
        assert!(pending.is_empty());

        pending.push(100, t0);
        pending.push(200, t0 + ms(1));
        pending.push(300, t0 + ms(2));
        assert_eq!(pending.match_image(200), Some((200, t0 + ms(1))));
        assert_eq!(pending.match_image(300), Some((300, t0 + ms(2))));
        assert!(pending.is_empty(), "later exact match skips a dropped surface frame");

        for (released, stamped) in [
            ([0, 0], [0, 0]),
            ([5000, 100], [5000, 100]),
            ([100, 200], [9_000_000, 9_016_000]),
        ] {
            pending.push(released[0], t0);
            pending.push(released[1], t0 + ms(1));
            assert_eq!(pending.match_image(stamped[0]), Some((released[0], t0)));
            assert_eq!(pending.match_image(stamped[1]), Some((released[1], t0 + ms(1))));
            assert!(pending.is_empty());
        }

        pending.push(100, t0);
        assert_eq!(pending.match_image(100), Some((100, t0)));
        pending.hold_acquired(42, 100, t0);
        assert!(!pending.has_released());
        assert!(!pending.is_empty());
        assert_eq!(pending.front_acquired(), Some(&42));
        assert_eq!(pending.fresh_wait(t0 + ms(5), ms(20)), ms(15));
        pending.push(200, t0 + ms(10));
        assert_eq!(pending.fresh_wait(t0 + ms(12), ms(20)), ms(18));
        assert_eq!(pending.take_acquired(), Some((42, 100, t0)));
        assert_eq!(pending.front_acquired(), None);
        assert!(!pending.is_empty());
        assert_eq!(pending.match_image(200), Some((200, t0 + ms(10))));
        assert!(pending.is_empty());

        pending.push(100, t0);
        pending.push(200, t0 + ms(25));
        assert_eq!(pending.fresh_wait(t0 + ms(26), ms(20)), ms(19));
        assert_eq!(pending.fresh_wait(t0 + ms(50), ms(20)), Duration::ZERO);
        assert_eq!(pending.fresh_wait(t0 + ms(26), Duration::ZERO), Duration::ZERO);
    }

    // Each callback consumes one expected call. A sleep records (requested, elapsed) in µs.
    fn run_reader_drain(
        budget_us: u64,
        no_buffer_us: u64,
        stop_on_progress: bool,
        drains: &[Result<(ReaderStop, usize), &'static str>],
        sleeps_us: &[(u64, u64)],
        fences: &[(u64, Result<FenceWait, &'static str>)],
    ) -> (Result<ReaderStop, &'static str>, u128) {
        let mut budget = Duration::from_micros(budget_us);
        let mut drains = drains.iter();
        let mut sleeps = sleeps_us.iter();
        let mut fences = fences.iter();
        let result = drain_reader_with_budget(
            &mut (), &mut budget, Duration::from_micros(no_buffer_us), Duration::from_micros(300), stop_on_progress,
            |_| *drains.next().expect("unexpected reader drain"),
            |step| {
                let &(requested_us, elapsed_us) = sleeps.next().expect("unexpected sleep");
                assert_eq!(step, Duration::from_micros(requested_us));
                Duration::from_micros(elapsed_us)
            },
            |_, limit| {
                let &(expected_us, result) = fences.next().expect("unexpected fence wait");
                assert_eq!(limit, Duration::from_micros(expected_us));
                result
            },
        );
        assert_eq!(drains.len(), 0, "missing reader drain");
        assert_eq!(sleeps.len(), 0, "missing sleep");
        assert_eq!(fences.len(), 0, "missing fence wait");
        (result, budget.as_micros())
    }

    #[test]
    fn drain_reader_with_budget_never_sleeps_at_zero_and_bounds_push_wait() {
        use ReaderStop::*;
        let run = |budget, stop_on_progress, drains: &[_], sleeps: &[_], fences: &[_]| {
            run_reader_drain(budget, budget, stop_on_progress, drains, sleeps, fences)
        };
        for stop in [NoBuffer, MaxAcquired, FencePending] {
            assert_eq!(run(0, false, &[Ok((stop, 0))], &[], &[]), (Ok(stop), 0));
        }
        let empty = Ok((NoBuffer, 0));
        let fenced = Ok((FencePending, 0));
        let done = Ok((Done, 1));
        assert_eq!(run(20_000, false, &[empty, empty, done], &[(300, 300); 2], &[]), (Ok(Done), 19_400));
        let signaled = Ok(FenceWait::Signaled(Duration::from_micros(400)));
        assert_eq!(run(20_000, false, &[empty, fenced, done], &[(300, 300)], &[(19_700, signaled)]), (Ok(Done), 19_300));
        assert_eq!(run(20_000, false, &[fenced], &[], &[(20_000, Ok(FenceWait::TimedOut))]), (Ok(FencePending), 0));
        assert_eq!(run(20_000, false, &[fenced], &[], &[(20_000, Err("fence failed"))]).0, Err("fence failed"));
        for stop in [NoBuffer, FencePending] {
            assert_eq!(run(20_000, true, &[Ok((stop, 1))], &[], &[]), (Ok(stop), 20_000));
        }
        let sleeps = [(300, 300), (300, 300), (300, 300), (100, 100)];
        assert_eq!(run(1000, false, &[Ok((MaxAcquired, 0)); 5], &sleeps, &[]), (Ok(MaxAcquired), 0));
        assert_eq!(run(20_000, false, &[Err("reader failed")], &[], &[]).0, Err("reader failed"));
    }

    #[test]
    fn drain_reader_with_budget_splits_no_buffer_cap_and_charges_oversleep() {
        use ReaderStop::*;
        let empty = Ok((NoBuffer, 0));
        let fenced = Ok((FencePending, 0));
        let full = Ok((MaxAcquired, 0));
        let done = Ok((Done, 1));

        assert_eq!(run_reader_drain(20_000, 0, true, &[empty], &[], &[]), (Ok(NoBuffer), 20_000));
        let signaled = Ok(FenceWait::Signaled(Duration::from_micros(4000)));
        assert_eq!(run_reader_drain(20_000, 0, true, &[fenced, done], &[], &[(20_000, signaled)]), (Ok(Done), 16_000));
        assert_eq!(run_reader_drain(20_000, 0, true, &[fenced], &[], &[(20_000, Ok(FenceWait::TimedOut))]), (Ok(FencePending), 0));

        let sleeps = [(300, 300), (300, 300), (300, 300), (100, 100)];
        assert_eq!(run_reader_drain(1000, 0, true, &[full; 5], &sleeps, &[]), (Ok(MaxAcquired), 0));
        assert_eq!(run_reader_drain(1000, 0, true, &[full, empty], &[(300, 300)], &[]), (Ok(NoBuffer), 700));

        assert_eq!(run_reader_drain(20_000, 1000, true, &[empty, empty], &[(300, 4000)], &[]), (Ok(NoBuffer), 16_000));
        let late = Ok(FenceWait::Signaled(Duration::from_micros(1500)));
        assert_eq!(run_reader_drain(1000, 1000, false, &[fenced, done], &[], &[(1000, late)]), (Ok(Done), 0));
    }
}
