//! The live generation-capability registry behind `GET /v1/job-profiles`.
//!
//! Deployment config ([`ServerConfig::job_profiles`]) is the STATIC
//! advertisement — what an operator says this deployment is for. It cannot
//! know what the GPU fleet can execute right now: weights get pulled and
//! evicted, boxes come and go, and a profile nothing can run is worse than
//! no profile at all (a client enqueues, and the job sits pending forever —
//! exactly the failure this module exists to end).
//!
//! So a worker that actually claims jobs announces what it can execute:
//!
//! ```text
//! PUT /v1/job-profiles
//! {"worker":"w1","ttl_ms":90000,"domains":["image","video"],"profiles":[…]}
//! ```
//!
//! and the GET merges. The announcement is AUTHORITATIVE FOR ITS DOMAINS:
//! a worker that covers `video` but announces no video profile hides the
//! deployment's static video profiles, because the only process that could
//! run them just said it cannot. Domains nobody covers keep their config
//! advertisement untouched, so a deployment with no worker yet still
//! documents its intent.
//!
//! Announcements are in-memory and LEASED: an entry older than its ttl is
//! dropped on the next read or write, so a worker that dies stops
//! advertising by itself within one ttl. Nothing here is persisted — a
//! restarted server re-learns the truth from the next announcement, which
//! is the only source that was ever authoritative.

use super::config::JobProfile;
use super::json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Most workers that may announce at once.
pub const MAX_WORKERS: usize = 32;
/// Most profiles one announcement may carry, and the most `GET` will ever
/// emit. Both are the client's `wire::MAX_JOB_PROFILES` — a longer list is
/// one no client can parse, so the merge truncates rather than producing an
/// answer the reader must refuse.
pub const MAX_PROFILES: usize = 64;
/// Most domains one announcement may cover.
pub const MAX_DOMAINS: usize = 32;
/// Announcement lease bounds. Short enough that a dead worker's profiles
/// disappear promptly; long enough that a 30 s re-announce cadence is safe.
pub const MIN_TTL_MS: u64 = 5_000;
pub const MAX_TTL_MS: u64 = 10 * 60 * 1000;

/// One worker's live advertisement.
#[derive(Clone, Debug)]
struct Announcement {
    /// Domains this worker covers — it is authoritative for all of them,
    /// including the ones it currently has no executable profile for.
    domains: Vec<String>,
    profiles: Vec<JobProfile>,
    expires_ms: u64,
}

/// In-memory, lease-expiring worker announcements. Cheap to lock: every
/// operation is a small vector walk under one mutex, never a store call.
#[derive(Debug, Default)]
pub struct ProfileRegistry {
    workers: Mutex<BTreeMap<String, Announcement>>,
}

/// Why an announcement was refused. Every variant is a client error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceError {
    TooManyWorkers,
    TooManyProfiles,
    TooManyDomains,
    BadTtl,
}

impl AnnounceError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooManyWorkers => "too many announcing workers",
            Self::TooManyProfiles => "too many profiles",
            Self::TooManyDomains => "too many domains",
            Self::BadTtl => "ttl out of bounds",
        }
    }
}

impl ProfileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or replace) one worker's advertisement. Re-announcing under
    /// the same worker id is the renewal path, so a live worker never
    /// counts twice against [`MAX_WORKERS`].
    pub fn announce(
        &self,
        worker: &str,
        ttl_ms: u64,
        domains: Vec<String>,
        profiles: Vec<JobProfile>,
        now_ms: u64,
    ) -> Result<(), AnnounceError> {
        if !(MIN_TTL_MS..=MAX_TTL_MS).contains(&ttl_ms) {
            return Err(AnnounceError::BadTtl);
        }
        if profiles.len() > MAX_PROFILES {
            return Err(AnnounceError::TooManyProfiles);
        }
        if domains.len() > MAX_DOMAINS {
            return Err(AnnounceError::TooManyDomains);
        }
        let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        workers.retain(|_, a| a.expires_ms > now_ms);
        if !workers.contains_key(worker) && workers.len() >= MAX_WORKERS {
            return Err(AnnounceError::TooManyWorkers);
        }
        workers.insert(
            worker.to_string(),
            Announcement {
                domains,
                profiles,
                expires_ms: now_ms.saturating_add(ttl_ms),
            },
        );
        Ok(())
    }

    /// Drop one worker's advertisement (clean shutdown). Unknown ids are a
    /// no-op — retracting twice must not be an error.
    pub fn retract(&self, worker: &str, now_ms: u64) {
        let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        workers.remove(worker);
        workers.retain(|_, a| a.expires_ms > now_ms);
    }

    /// Every live worker id (for diagnostics/tests).
    pub fn live_workers(&self, now_ms: u64) -> Vec<String> {
        let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        workers.retain(|_, a| a.expires_ms > now_ms);
        workers.keys().cloned().collect()
    }

    /// The advertisement clients see: config profiles for domains no live
    /// worker covers, plus every live worker's profiles. Ids are unique in
    /// the result — a later announcement of an id already present is
    /// dropped, so two workers advertising the same tier list it once.
    pub fn advertised(&self, config: &[JobProfile], now_ms: u64) -> Vec<JobProfile> {
        let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        workers.retain(|_, a| a.expires_ms > now_ms);
        let covered: Vec<&str> = workers
            .values()
            .flat_map(|a| a.domains.iter().map(String::as_str))
            .collect();
        let mut out: Vec<JobProfile> = Vec::new();
        let push = |profile: &JobProfile, out: &mut Vec<JobProfile>| {
            if out.len() < MAX_PROFILES && !out.iter().any(|p| p.id == profile.id) {
                out.push(profile.clone());
            }
        };
        for profile in workers.values().flat_map(|a| a.profiles.iter()) {
            push(profile, &mut out);
        }
        for profile in config {
            if covered.contains(&profile.domain.as_str()) {
                continue;
            }
            push(profile, &mut out);
        }
        out
    }
}

/// Parse the announcement body. Shape errors are the caller's 400s; this
/// validates exactly what [`ServerConfig::validate`] validates for config
/// profiles, so an announced profile can never be less well-formed than a
/// configured one.
pub fn parse_announced_profiles(rows: &[Value]) -> Result<Vec<JobProfile>, &'static str> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let text = |key: &str| row.get(key).and_then(Value::as_str).unwrap_or_default();
        let profile = JobProfile {
            id: text("id").to_string(),
            domain: text("domain").to_string(),
            label: text("label").to_string(),
            kind: text("kind").to_string(),
            namespace: text("namespace").to_string(),
            defaults: match row.get("defaults") {
                Some(d @ Value::Obj(_)) => d.clone(),
                _ => return Err("profile defaults must be an object"),
            },
        };
        validate_profile(&profile)?;
        if out.iter().any(|p: &JobProfile| p.id == profile.id) {
            return Err("duplicate profile id");
        }
        out.push(profile);
    }
    Ok(out)
}

fn text_ok(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

/// The same shape contract config profiles must satisfy.
pub fn validate_profile(profile: &JobProfile) -> Result<(), &'static str> {
    let id_ok = !profile.id.is_empty()
        && profile.id.len() <= 64
        && profile
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !id_ok {
        return Err("profile id");
    }
    if !text_ok(&profile.domain, 32) {
        return Err("profile domain");
    }
    if !text_ok(&profile.label, 128) {
        return Err("profile label");
    }
    if !text_ok(&profile.kind, 64) {
        return Err("profile kind");
    }
    if !text_ok(&profile.namespace, 64) {
        return Err("profile namespace");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::json::{obj, s};

    fn profile(id: &str, domain: &str, kind: &str) -> JobProfile {
        JobProfile {
            id: id.to_string(),
            domain: domain.to_string(),
            label: format!("label {id}"),
            kind: kind.to_string(),
            namespace: "gen".to_string(),
            defaults: obj(vec![("model", s("m"))]),
        }
    }

    #[test]
    fn config_shows_until_a_worker_covers_the_domain() {
        let registry = ProfileRegistry::new();
        let config = vec![
            profile("h3-standard", "video", "video.generate"),
            profile("stock-audio", "audio", "audio.generate"),
        ];
        // Nothing announced: the config advertisement stands.
        let ids: Vec<String> = registry
            .advertised(&config, 1_000)
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec!["h3-standard", "stock-audio"]);

        // A worker covers `video` but can execute nothing there (H3 weights
        // absent fleet-wide) — the stock video profiles must disappear
        // rather than accept jobs nobody can run. `audio` is untouched.
        registry
            .announce("w1", 60_000, vec!["video".to_string()], vec![], 1_000)
            .unwrap();
        let ids: Vec<String> = registry
            .advertised(&config, 1_000)
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec!["stock-audio"]);

        // Now it can: the announced profile replaces the stock one.
        registry
            .announce(
                "w1",
                60_000,
                vec!["video".to_string()],
                vec![profile("h3-q4", "video", "video.generate")],
                2_000,
            )
            .unwrap();
        let ids: Vec<String> = registry
            .advertised(&config, 2_000)
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec!["h3-q4", "stock-audio"]);
    }

    #[test]
    fn a_dead_worker_stops_advertising_within_one_lease() {
        let registry = ProfileRegistry::new();
        let config = vec![profile("stock-video", "video", "video.generate")];
        registry
            .announce(
                "w1",
                10_000,
                vec!["image".to_string()],
                vec![profile("flux-fast", "image", "image.generate")],
                1_000,
            )
            .unwrap();
        assert_eq!(registry.live_workers(5_000), vec!["w1".to_string()]);
        assert_eq!(registry.advertised(&config, 5_000).len(), 2);
        // Lease expired: only the config advertisement remains, and the
        // worker is gone from the roster without anyone sweeping it.
        assert!(registry.live_workers(11_001).is_empty());
        let ids: Vec<String> = registry
            .advertised(&config, 11_001)
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec!["stock-video"]);
    }

    #[test]
    fn two_workers_on_the_same_tier_advertise_it_once_and_retract_cleanly() {
        let registry = ProfileRegistry::new();
        for worker in ["box-a", "box-b"] {
            registry
                .announce(
                    worker,
                    60_000,
                    vec!["image".to_string()],
                    vec![profile("flux-fast", "image", "image.generate")],
                    1_000,
                )
                .unwrap();
        }
        assert_eq!(registry.advertised(&[], 1_000).len(), 1);
        registry.retract("box-a", 1_000);
        assert_eq!(registry.live_workers(1_000), vec!["box-b".to_string()]);
        assert_eq!(registry.advertised(&[], 1_000).len(), 1);
        registry.retract("box-b", 1_000);
        assert!(registry.advertised(&[], 1_000).is_empty());
        // Retracting an unknown worker is a no-op, never an error.
        registry.retract("box-a", 1_000);
    }

    #[test]
    fn announcements_are_bounded_and_renewals_do_not_consume_slots() {
        let registry = ProfileRegistry::new();
        for i in 0..MAX_WORKERS {
            registry
                .announce(&format!("w{i}"), 60_000, vec![], vec![], 1_000)
                .unwrap();
        }
        assert_eq!(
            registry.announce("one-too-many", 60_000, vec![], vec![], 1_000),
            Err(AnnounceError::TooManyWorkers)
        );
        // A renewal of an existing id is always accepted.
        registry.announce("w0", 60_000, vec![], vec![], 1_000).unwrap();
        assert_eq!(
            registry.announce("w0", MIN_TTL_MS - 1, vec![], vec![], 1_000),
            Err(AnnounceError::BadTtl)
        );
        assert_eq!(
            registry.announce("w0", MAX_TTL_MS + 1, vec![], vec![], 1_000),
            Err(AnnounceError::BadTtl)
        );
        let many = vec![profile("p", "image", "image.generate"); MAX_PROFILES + 1];
        assert_eq!(
            registry.announce("w0", 60_000, vec![], many, 1_000),
            Err(AnnounceError::TooManyProfiles)
        );
        let domains: Vec<String> = (0..MAX_DOMAINS + 1).map(|i| format!("d{i}")).collect();
        assert_eq!(
            registry.announce("w0", 60_000, domains, vec![], 1_000),
            Err(AnnounceError::TooManyDomains)
        );
    }

    #[test]
    fn announced_profiles_must_be_as_well_formed_as_configured_ones() {
        let good = obj(vec![
            ("id", s("flux-fast")),
            ("domain", s("image")),
            ("label", s("FLUX.1 schnell")),
            ("kind", s("image.generate")),
            ("namespace", s("gen")),
            ("defaults", obj(vec![("model", s("flux1-schnell"))])),
        ]);
        assert_eq!(parse_announced_profiles(&[good.clone()]).unwrap().len(), 1);
        assert!(parse_announced_profiles(&[good.clone(), good]).is_err());

        let mut bad_id = obj(vec![
            ("id", s("Flux Fast")),
            ("domain", s("image")),
            ("label", s("l")),
            ("kind", s("image.generate")),
            ("namespace", s("gen")),
            ("defaults", obj(vec![])),
        ]);
        assert!(parse_announced_profiles(&[bad_id.clone()]).is_err());
        bad_id = obj(vec![
            ("id", s("flux-fast")),
            ("domain", s("image")),
            ("label", s("l")),
            ("kind", s("image.generate")),
            ("namespace", s("gen")),
            ("defaults", s("not an object")),
        ]);
        assert!(parse_announced_profiles(&[bad_id]).is_err());
    }
}
