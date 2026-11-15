//! Per-address limits for an internet-facing server. A limit never loses a
//! guest's data: an upload over its rate is answered 429 with a Retry-After,
//! and the phone page sends it again then.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The machine's memory, for budgets that scale with it (Linux
/// `/proc/meminfo`; elsewhere a laptop's 16 GiB).
pub fn memory_bytes() -> usize {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|info| {
            let line = info.lines().find(|l| l.starts_with("MemTotal:"))?;
            let kib: usize = line.split_whitespace().nth(1)?.parse().ok()?;
            Some(kib * 1024)
        })
        .unwrap_or(16 << 30)
}

/// Bytes that may be held at once across every connection (request bodies
/// read whole and checked): over it, a request is told to come back, never
/// cut off halfway.
pub struct ByteBudget {
    capacity: usize,
    used: AtomicUsize,
}

/// A part of a [`ByteBudget`], given back when dropped.
pub struct Reserved<'a> {
    budget: &'a ByteBudget,
    bytes: usize,
}

impl ByteBudget {
    pub fn new(capacity: usize) -> Self {
        Self { capacity, used: AtomicUsize::new(0) }
    }

    pub fn reserve(&self, bytes: usize) -> Option<Reserved<'_>> {
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            let total = used.checked_add(bytes).filter(|&total| total <= self.capacity)?;
            match self.used.compare_exchange_weak(used, total, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Reserved { budget: self, bytes }),
                Err(now) => used = now,
            }
        }
    }

    pub fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
}

impl Drop for Reserved<'_> {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Wrong party codes from one address within [`CodeGuard::WINDOW`] before it
/// is refused for the rest of the window: a guest mistypes a few times, a
/// scanner guesses thousands.
const WRONG_CODES: u32 = 20;

#[derive(Default)]
pub struct CodeGuard {
    wrong: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

impl CodeGuard {
    pub const WINDOW: Duration = Duration::from_secs(600);

    pub fn wrong(&self, ip: IpAddr) {
        self.wrong_at(ip, Instant::now());
    }

    pub fn refused(&self, ip: IpAddr) -> bool {
        self.refused_at(ip, Instant::now())
    }

    fn wrong_at(&self, ip: IpAddr, now: Instant) {
        let mut wrong = self.wrong.lock().unwrap_or_else(|p| p.into_inner());
        if wrong.len() > 4096 {
            wrong.retain(|_, (_, since)| now.duration_since(*since) < Self::WINDOW);
        }
        let entry = wrong.entry(ip).or_insert((0, now));
        if now.duration_since(entry.1) >= Self::WINDOW {
            *entry = (0, now);
        }
        entry.0 += 1;
    }

    fn refused_at(&self, ip: IpAddr, now: Instant) -> bool {
        let wrong = self.wrong.lock().unwrap_or_else(|p| p.into_inner());
        wrong.get(&ip).is_some_and(|(count, since)| *count >= WRONG_CODES && now.duration_since(*since) < Self::WINDOW)
    }
}

/// A token bucket per key: `burst` at once, then `per_second`.
pub struct RateLimiter<K> {
    burst: f64,
    per_second: f64,
    buckets: Mutex<HashMap<K, (f64, Instant)>>,
}

impl<K: std::hash::Hash + Eq + Clone> RateLimiter<K> {
    pub fn new(burst: f64, per_second: f64) -> Self {
        Self { burst, per_second, buckets: Mutex::new(HashMap::new()) }
    }

    /// Takes `cost` tokens for `key`, or says how long until they are there.
    pub fn take(&self, key: &K, cost: f64) -> Result<(), Duration> {
        self.take_at(key, cost, Instant::now())
    }

    fn take_at(&self, key: &K, cost: f64, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        if buckets.len() > 4096 {
            let full = Duration::from_secs_f64(self.burst / self.per_second);
            buckets.retain(|_, (_, at)| now.duration_since(*at) < full);
        }
        let (tokens, at) = buckets.entry(key.clone()).or_insert((self.burst, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * self.per_second).min(self.burst);
        *at = now;
        if *tokens >= cost {
            *tokens -= cost;
            Ok(())
        } else {
            Err(Duration::from_secs_f64(((cost - *tokens) / self.per_second).max(0.05)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_codes_refuse_an_address_for_the_window_only() {
        let guard = CodeGuard::default();
        let (a, b): (IpAddr, IpAddr) = ("192.0.2.1".parse().unwrap(), "192.0.2.2".parse().unwrap());
        let t0 = Instant::now();
        for _ in 0..WRONG_CODES - 1 {
            guard.wrong_at(a, t0);
        }
        assert!(!guard.refused_at(a, t0), "a few mistakes are fine");
        guard.wrong_at(a, t0);
        assert!(guard.refused_at(a, t0), "a guesser is refused");
        assert!(!guard.refused_at(b, t0), "other addresses are not");
        assert!(!guard.refused_at(a, t0 + CodeGuard::WINDOW), "the refusal ends with the window");
    }

    #[test]
    fn rate_limiter_bursts_then_refills() {
        let limiter = RateLimiter::new(3.0, 1.0);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(limiter.take_at(&1u32, 1.0, t0).is_ok());
        }
        let wait = limiter.take_at(&1, 1.0, t0).unwrap_err();
        assert!((wait.as_secs_f64() - 1.0).abs() < 0.01, "retry after the refill: {wait:?}");
        assert!(limiter.take_at(&2, 1.0, t0).is_ok(), "keys are separate");
        assert!(limiter.take_at(&1, 1.0, t0 + Duration::from_secs(1)).is_ok());

        let budget = ByteBudget::new(100);
        let a = budget.reserve(60).expect("room for the first");
        assert!(budget.reserve(41).is_none(), "the budget is shared: no room for a second over it");
        let b = budget.reserve(40).expect("what fits still fits");
        drop(a);
        assert_eq!(budget.used(), 40, "a finished request gives its part back");
        drop(b);
        assert!(budget.reserve(100).is_some() && budget.reserve(usize::MAX).is_none(), "the whole budget, never more");
    }
}
