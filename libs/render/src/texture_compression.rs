//! Adapt the caller's task pool to the texture encoder's chunk executor.

use makepad_draw::makepad_platform::thread::{checkpoint_when_due, yield_to_waiting, TaskPool};
use std::time::{Duration, Instant};

/// About how long one round of chunks runs before the encode lets waiting jobs go first.
const ROUND: Duration = Duration::from_millis(40);

/// Run all chunks through the pool, returning after every chunk finishes.
/// Call from a worker, as required by [`TaskPool::fan_out`].
///
/// The chunks go out in rounds of about [`ROUND`], sized from the last round's time. Between
/// rounds the encode gives its CPU permit back, behind every job that asked for one meanwhile
/// ([`yield_to_waiting`]). The budget is FIFO, so one large encode would otherwise keep its
/// class's permits to its last chunk. A 4k sky's stored form held the only Background permit of a
/// four-core budget for 11.6 s, while a game's voxel car and a requested still waited behind it.
pub fn pool_executor(pool: &TaskPool) -> impl Fn(usize, &(dyn Fn(usize) + Sync)) + '_ {
    // One chunk is the caller's own item: there is nothing to share, and a
    // fan-out of one still costs a helper hand-off and a budget checkpoint
    // (about 12 ms each, measured over 234 single-mip encodes of one model).
    move |chunks, work| {
        // Even a stale encode needs every chunk to produce its result.
        let _ = checkpoint_when_due();
        let (mut next, mut round) = (0, 1);
        while next < chunks {
            let (base, len) = (next, round.min(chunks - next));
            let started = Instant::now();
            if len == 1 { work(base) } else { pool.fan_out(len, |i| work(base + i)) }
            next += len;
            if next == chunks {
                break;
            }
            // The round's own time: the wait below is other jobs' turns.
            let took = started.elapsed();
            yield_to_waiting();
            if took < ROUND / 2 {
                round *= 2;
            } else if took > ROUND * 2 {
                round = (round / 2).max(1);
            }
        }
    }
}
