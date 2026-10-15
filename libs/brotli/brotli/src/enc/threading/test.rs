#![cfg(test)]
#![cfg(feature = "std")]
// Unit tests for the parent `threading` module. These exercise CompressMulti's
// error-draining behavior
use super::*;
use alloc::SliceWrapper;
use alloc_stdlib::StandardAlloc;
use core::sync::atomic::{AtomicUsize, Ordering};

static JOINED_AFTER_JOIN_ERROR: AtomicUsize = AtomicUsize::new(0);
static JOINED_AFTER_SETUP_ERROR: AtomicUsize = AtomicUsize::new(0);
static DRAINED_AFTER_STREAM_PHASE: AtomicUsize = AtomicUsize::new(0);

struct TestSlice(&'static [u8]);

impl SliceWrapper<u8> for TestSlice {
    fn slice(&self) -> &[u8] {
        self.0
    }
}

struct CountingJoinable {
    result: Option<Result<CompressionThreadResult<StandardAlloc>, BrotliEncoderThreadError>>,
    joined_count: &'static AtomicUsize,
}

impl Joinable<CompressionThreadResult<StandardAlloc>, BrotliEncoderThreadError>
    for CountingJoinable
{
    fn join(mut self) -> Result<CompressionThreadResult<StandardAlloc>, BrotliEncoderThreadError> {
        self.joined_count.fetch_add(1, Ordering::SeqCst);
        self.result.take().unwrap()
    }
}

struct TestOwnedRetriever<U: Send + 'static> {
    input: Option<U>,
    fail_views: bool,
}

impl<U: Send + 'static> OwnedRetriever<U> for TestOwnedRetriever<U> {
    fn view<Output, Func: FnOnce(&U) -> Output>(
        &self,
        func: Func,
    ) -> Result<Output, PoisonedThreadError> {
        if self.fail_views {
            Err(PoisonedThreadError::default())
        } else {
            Ok(func(self.input.as_ref().unwrap()))
        }
    }

    fn unwrap(self) -> Result<U, PoisonedThreadError> {
        Ok(self.input.unwrap())
    }
}

struct CountingSpawner {
    joined_count: &'static AtomicUsize,
    join_error_index: Option<usize>,
    fail_views: bool,
}

impl
    BatchSpawnableLite<
        CompressionThreadResult<StandardAlloc>,
        UnionHasher<StandardAlloc>,
        StandardAlloc,
        (TestSlice, BrotliEncoderParams),
    > for CountingSpawner
{
    type JoinHandle = CountingJoinable;
    type FinalJoinHandle = TestOwnedRetriever<(TestSlice, BrotliEncoderParams)>;

    fn make_spawner(
        &mut self,
        input: &mut Owned<(TestSlice, BrotliEncoderParams)>,
    ) -> Self::FinalJoinHandle {
        TestOwnedRetriever {
            input: Some(mem::replace(input, Owned(InternalOwned::Borrowed)).unwrap()),
            fail_views: self.fail_views,
        }
    }

    fn spawn(
        &mut self,
        _handle: &mut Self::FinalJoinHandle,
        alloc_per_thread: &mut SendAlloc<
            CompressionThreadResult<StandardAlloc>,
            UnionHasher<StandardAlloc>,
            StandardAlloc,
            Self::JoinHandle,
        >,
        index: usize,
        _num_threads: usize,
        _func: fn(
            UnionHasher<StandardAlloc>,
            usize,
            usize,
            &(TestSlice, BrotliEncoderParams),
            StandardAlloc,
        ) -> CompressionThreadResult<StandardAlloc>,
    ) {
        let (alloc, _extra_input) = alloc_per_thread.replace_with_default();
        let result = if self.join_error_index == Some(index) {
            Err(BrotliEncoderThreadError::OtherThreadPanic)
        } else {
            Ok(CompressionThreadResult {
                compressed: Err(BrotliEncoderThreadError::InsufficientOutputSpace),
                alloc,
            })
        };
        alloc_per_thread.0 = InternalSendAlloc::Join(CountingJoinable {
            result: Some(result),
            joined_count: self.joined_count,
        });
    }
}

type TestSendAlloc = SendAlloc<
    CompressionThreadResult<StandardAlloc>,
    UnionHasher<StandardAlloc>,
    StandardAlloc,
    CountingJoinable,
>;

fn test_alloc() -> TestSendAlloc {
    SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit)
}

/// Spawner that produces genuine, concatenatable chunks via the real
/// `compress_part` for every worker except `compressed_error_index`, whose
/// worker instead reports a compression error. This lets a test put a real
/// (streamable) result *after* an errored worker so the drain loop reaches
/// its BroCatli streaming arms, exercising the "first error wins" behavior.
struct RealChunkSpawner {
    joined_count: &'static AtomicUsize,
    compressed_error_index: Option<usize>,
}

impl
    BatchSpawnableLite<
        CompressionThreadResult<StandardAlloc>,
        UnionHasher<StandardAlloc>,
        StandardAlloc,
        (TestSlice, BrotliEncoderParams),
    > for RealChunkSpawner
{
    type JoinHandle = CountingJoinable;
    type FinalJoinHandle = TestOwnedRetriever<(TestSlice, BrotliEncoderParams)>;

    fn make_spawner(
        &mut self,
        input: &mut Owned<(TestSlice, BrotliEncoderParams)>,
    ) -> Self::FinalJoinHandle {
        TestOwnedRetriever {
            input: Some(mem::replace(input, Owned(InternalOwned::Borrowed)).unwrap()),
            fail_views: false,
        }
    }

    fn spawn(
        &mut self,
        handle: &mut Self::FinalJoinHandle,
        alloc_per_thread: &mut SendAlloc<
            CompressionThreadResult<StandardAlloc>,
            UnionHasher<StandardAlloc>,
            StandardAlloc,
            Self::JoinHandle,
        >,
        index: usize,
        num_threads: usize,
        _func: fn(
            UnionHasher<StandardAlloc>,
            usize,
            usize,
            &(TestSlice, BrotliEncoderParams),
            StandardAlloc,
        ) -> CompressionThreadResult<StandardAlloc>,
    ) {
        let (alloc, _extra_input) = alloc_per_thread.replace_with_default();
        let result = if self.compressed_error_index == Some(index) {
            Ok(CompressionThreadResult {
                compressed: Err(BrotliEncoderThreadError::ConcatenationDidNotProcessFullFile),
                alloc,
            })
        } else {
            // Compress synchronously exactly as the production spawner would,
            // yielding a real chunk that BroCatli can concatenate.
            Ok(handle
                .view(|input_and_params| {
                    compress_part(
                        UnionHasher::Uninit,
                        index,
                        num_threads,
                        input_and_params,
                        alloc,
                    )
                })
                .unwrap())
        };
        alloc_per_thread.0 = InternalSendAlloc::Join(CountingJoinable {
            result: Some(result),
            joined_count: self.joined_count,
        });
    }
}

#[test]
fn compress_multi_joins_remaining_workers_after_join_error() {
    static INPUT: &[u8] = b"join all workers before returning";
    JOINED_AFTER_JOIN_ERROR.store(0, Ordering::SeqCst);
    let mut spawner = CountingSpawner {
        joined_count: &JOINED_AFTER_JOIN_ERROR,
        join_error_index: Some(0),
        fail_views: false,
    };
    let mut alloc_per_thread = [test_alloc(), test_alloc(), test_alloc(), test_alloc()];
    let mut params = BrotliEncoderParams::default();
    params.quality = 1;
    let mut owned_input = Owned::new(TestSlice(INPUT));
    let mut output = [0u8; 256];

    let result = CompressMulti(
        &params,
        &mut owned_input,
        &mut output,
        &mut alloc_per_thread[..],
        &mut spawner,
    );

    assert!(matches!(
        result,
        Err(BrotliEncoderThreadError::OtherThreadPanic)
    ));
    assert_eq!(JOINED_AFTER_JOIN_ERROR.load(Ordering::SeqCst), 3);
    assert_eq!(owned_input.view().slice(), INPUT);
}

#[test]
fn compress_multi_joins_spawned_worker_after_setup_view_error() {
    static INPUT: &[u8] = b"restore input after setup failure";
    JOINED_AFTER_SETUP_ERROR.store(0, Ordering::SeqCst);
    let mut spawner = CountingSpawner {
        joined_count: &JOINED_AFTER_SETUP_ERROR,
        join_error_index: None,
        fail_views: true,
    };
    let mut alloc_per_thread = [test_alloc(), test_alloc()];
    let mut params = BrotliEncoderParams::default();
    params.favor_cpu_efficiency = true;
    let mut owned_input = Owned::new(TestSlice(INPUT));
    let mut output = [0u8; 256];

    let result = CompressMulti(
        &params,
        &mut owned_input,
        &mut output,
        &mut alloc_per_thread[..],
        &mut spawner,
    );

    assert!(matches!(
        result,
        Err(BrotliEncoderThreadError::OtherThreadPanic)
    ));
    assert_eq!(JOINED_AFTER_SETUP_ERROR.load(Ordering::SeqCst), 1);
    assert_eq!(owned_input.view().slice(), INPUT);
}

/// Test case:
/// Worker 0 erorrs
/// Workers 1.. return valid results.
///
/// Ensure Worker 0's error is returned irrespective of latter success values.
#[test]
fn compress_multi_preserves_first_worker_error_through_stream_phase() {
    static INPUT: &[u8] =
        b"a sufficiently long body of text so every worker produces a real brotli chunk";
    DRAINED_AFTER_STREAM_PHASE.store(0, Ordering::SeqCst);
    let mut spawner = RealChunkSpawner {
        joined_count: &DRAINED_AFTER_STREAM_PHASE,
        compressed_error_index: Some(0),
    };
    let mut alloc_per_thread = [test_alloc(), test_alloc(), test_alloc()];
    let mut params = BrotliEncoderParams::default();
    params.quality = 1;
    params.magic_number = true;
    let mut owned_input = Owned::new(TestSlice(INPUT));
    // Enough output that the healthy chunks could have concatted ok.
    let mut output = [0u8; 4096];

    let result = CompressMulti(
        &params,
        &mut owned_input,
        &mut output,
        &mut alloc_per_thread[..],
        &mut spawner,
    );

    assert!(matches!(
        result,
        Err(BrotliEncoderThreadError::ConcatenationDidNotProcessFullFile)
    ));
    // Both spawned workers (indices 0 and 1) are joined; index 2 is the
    // synchronous last thread.
    assert_eq!(DRAINED_AFTER_STREAM_PHASE.load(Ordering::SeqCst), 2);
    assert_eq!(owned_input.view().slice(), INPUT);
}
