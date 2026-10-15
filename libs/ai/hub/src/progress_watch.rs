//! Inactivity reporting. Elapsed job time is deliberately not a failure signal.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Generous silence threshold for subprocesses and device release work.
pub(crate) const STALL_AFTER: Duration = Duration::from_secs(10 * 60);
pub(crate) const POLL: Duration = Duration::from_millis(200);

pub(crate) struct ProgressWatch {
    last_progress: Instant,
    reported: bool,
}

impl ProgressWatch {
    pub(crate) fn new(now: Instant) -> Self {
        Self { last_progress: now, reported: false }
    }

    /// Returns true when real activity recovers a previously silent job.
    pub(crate) fn progress(&mut self, now: Instant) -> bool {
        self.last_progress = now;
        std::mem::replace(&mut self.reported, false)
    }

    /// Reports a transition once, without cancelling or imposing a total cap.
    pub(crate) fn stalled(&mut self, now: Instant, threshold: Duration) -> bool {
        if !self.reported && now.saturating_duration_since(self.last_progress) >= threshold {
            self.reported = true;
            true
        } else {
            false
        }
    }
}

/// Count raw stdout AND stderr reads before assembling protocol lines. A
/// partial line, carriage-return progress bar, or stderr-only worker is active.
pub(crate) struct ProcessOutput {
    pub(crate) lines: mpsc::Receiver<String>,
    bytes: Arc<AtomicU64>,
}

impl ProcessOutput {
    pub(crate) fn new(stdout: impl Read + Send + 'static, stderr: impl Read + Send + 'static) -> Self {
        let (tx, lines) = mpsc::channel();
        let bytes = Arc::new(AtomicU64::new(0));
        let read_bytes = bytes.clone();
        std::thread::spawn(move || read_output(stdout, read_bytes, Some(tx)));
        let read_bytes = bytes.clone();
        std::thread::spawn(move || read_output(stderr, read_bytes, None));
        Self { lines, bytes }
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
}

fn read_output(mut pipe: impl Read, bytes: Arc<AtomicU64>, lines: Option<mpsc::Sender<String>>) {
    let mut buffer = [0u8; 8192];
    let mut pending = Vec::new();
    loop {
        let count = match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        bytes.fetch_add(count as u64, Ordering::Relaxed);
        if let Some(lines) = &lines {
            for &byte in &buffer[..count] {
                if byte == b'\n' || byte == b'\r' {
                    if !pending.is_empty() {
                        let line = String::from_utf8_lossy(&pending).into_owned();
                        if lines.send(line).is_err() { return; }
                        pending.clear();
                    }
                } else {
                    pending.push(byte);
                }
            }
        } else {
            // Preserve the child's diagnostics without making newline delivery
            // a prerequisite for counting progress.
            use std::io::Write;
            let _ = std::io::stderr().write_all(&buffer[..count]);
        }
    }
    if let Some(lines) = lines {
        if !pending.is_empty() {
            let _ = lines.send(String::from_utf8_lossy(&pending).into_owned());
        }
    }
}

/// Wait for a live worker's release reply. Dropping its reply sender (exit or
/// panic) is the only failure; a long release is waited for and logged as
/// waiting for GPU memory, then as not responding after `STALL_AFTER`.
#[cfg(any(feature = "flux", feature = "rig-native", feature = "motion-native"))]
pub(crate) fn release_reply<T>(
    replies: &mpsc::Receiver<T>,
    worker: &str,
) -> Result<T, mpsc::RecvError> {
    let mut watch = ProgressWatch::new(Instant::now());
    eprintln!("[hub] {worker}: waiting for GPU memory release");
    loop {
        match replies.recv_timeout(POLL) {
            Ok(reply) => return Ok(reply),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(mpsc::RecvError),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if watch.stalled(Instant::now(), STALL_AFTER) {
                    eprintln!("[hub] {worker}: not responding: no GPU memory release reply for {} min", STALL_AFTER.as_secs() / 60);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_progress_resets_silence_and_total_elapsed_never_trips() {
        let start = Instant::now();
        let mut watch = ProgressWatch::new(start);
        for seconds in (0..100_000).step_by(100) {
            let now = start + Duration::from_secs(seconds);
            assert!(!watch.progress(now));
            assert!(!watch.stalled(now + Duration::from_secs(599), STALL_AFTER));
        }
        let silent = start + Duration::from_secs(200_000);
        assert!(watch.stalled(silent, STALL_AFTER));
        assert!(!watch.stalled(silent + STALL_AFTER, STALL_AFTER));
        assert!(watch.progress(silent + STALL_AFTER));
        assert!(!watch.stalled(silent + STALL_AFTER, STALL_AFTER));
    }

    #[test]
    fn partial_lines_count_as_bytes_and_carriage_returns_delimit_progress() {
        let bytes = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        read_output(&b"partial\rnext\nlast"[..], bytes.clone(), Some(tx));
        assert_eq!(bytes.load(Ordering::Relaxed), 17);
        assert_eq!(rx.into_iter().collect::<Vec<_>>(), ["partial", "next", "last"]);
    }

    #[cfg(any(feature = "flux", feature = "rig-native", feature = "motion-native"))]
    #[test]
    fn release_reply_fails_only_when_the_worker_is_gone() {
        let (tx, rx) = mpsc::channel::<u8>();
        tx.send(7).unwrap();
        assert_eq!(release_reply(&rx, "test").unwrap(), 7);
        drop(tx);
        assert!(release_reply(&rx, "test").is_err());
    }
}
