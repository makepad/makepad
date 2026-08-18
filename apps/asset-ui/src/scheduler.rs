//! Bounded fleet-aware run scheduler — the pure planning core.
//!
//! SLOT IDENTITY: an endpoint is NOT a GPU slot. The deployment invariant
//! is exactly ONE makepad-asset-ai service process per physical PC, with
//! any additional GPUs advertised as slots BY that one service — never as
//! extra ports/processes. Host bucketing here is defense in depth: a live
//! probe once found a second service (10.0.0.169:8080) beside :8123 on the
//! same 96GB GPU, and scheduling the two independently would have
//! double-booked the card. If a duplicate/stale entry or a rogue second
//! process ever reappears, [`slot_key`] still collapses it: an advertised
//! stable gpu id when one exists, else the host IP. (`node_key` must never
//! be the key — it is per SERVICE INSTANCE, one per port.) A bucket admits
//! one run unless a service in it explicitly advertises more GPUs
//! (`capacity` > 1 — never assumed).
//!
//! Honesty rules:
//!
//! - capability comes from the same advertised-model affinity functions the
//!   per-stage router uses — never from a hardcoded box list;
//! - per endpoint, busy = max(service-reported queue depth, our committed
//!   stages) so a submission that has not yet appeared in a health poll is
//!   neither missed nor double-counted; a bucket SUMS its endpoints (each
//!   job queues on exactly one endpoint);
//! - a fresh run STARTS only onto a free compatible slot, WAITS in the app
//!   queue while all compatible capacity is occupied, and fails visibly
//!   (service gap) when no live endpoint has the capability at all.
//!
//! Repeated Generate clicks therefore fan out across distinct free GPU
//! slots, and a single-capacity GPU is never handed two of our runs — not
//! even through two different service ports.

/// Upper bound on concurrently active runs, above per-slot capacity. The
/// demo target is five simultaneous video jobs across five GPU slots; the
/// bound leaves headroom without ever being the effective limiter (slot
/// capacity is).
pub const MAX_ACTIVE_RUNS: usize = 8;

/// Physical-GPU slot identity for one endpoint. Preference order: a stable
/// advertised gpu id (none exists in `/health` today — the seam is here for
/// when one does), else the endpoint's host IP — two ports on one host are
/// assumed to share one GPU until the service says otherwise.
pub fn slot_key(base_url: &str, advertised_gpu_id: Option<&str>) -> String {
    if let Some(gpu_id) = advertised_gpu_id {
        return format!("gpu:{gpu_id}");
    }
    let stripped = base_url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host = stripped
        .split(['/', ':'])
        .next()
        .filter(|host| !host.is_empty())
        .unwrap_or(stripped);
    format!("host:{host}")
}

/// Expand occupied endpoint urls to every endpoint sharing their GPU slot.
/// `pairs` = (base_url, slot_key) for the whole fleet. The occupied urls
/// themselves are always included, even when no longer in the fleet list.
pub fn expand_occupied(pairs: &[(String, String)], occupied: &[String]) -> Vec<String> {
    let occupied_keys: Vec<&str> = pairs
        .iter()
        .filter(|(url, _)| occupied.iter().any(|occ| occ == url))
        .map(|(_, key)| key.as_str())
        .collect();
    let mut expanded: Vec<String> = occupied.to_vec();
    for (url, key) in pairs {
        if occupied_keys.iter().any(|occ| occ == key)
            && !expanded.iter().any(|existing| existing == url)
        {
            expanded.push(url.clone());
        }
    }
    expanded
}

/// Honest load picture of one service endpoint for one prospective run.
#[derive(Clone, Debug)]
pub struct EndpointLoad {
    pub base_url: String,
    /// Physical-GPU bucket this endpoint belongs to (see [`slot_key`]).
    pub slot_key: String,
    /// `/health` answered on the last poll.
    pub up: bool,
    /// The run's FIRST stage can execute here (advertised models via the
    /// affinity scorer, after the run's own box pin), and the GPU's TOTAL
    /// memory can fit that model plus the service reserve.
    pub capable: bool,
    /// `capable`, but the latest health snapshot reports too little FREE
    /// memory to admit the model now. This is transient pressure: the run
    /// stays queued and is reconsidered on the next fleet poll.
    pub vram_waiting: bool,
    /// Service-reported queue depth (includes other clients' jobs and any
    /// of ours the service has already seen).
    pub reported_pending: u64,
    /// Our runs whose CURRENT stage is committed to this endpoint —
    /// authoritative between health polls.
    pub ours_active: u32,
    /// GPU slots this endpoint's service advertises for its bucket. 1
    /// unless the service EXPLICITLY says it drives multiple GPUs.
    pub capacity: u32,
}

/// This endpoint's own occupied-count: max() covers the window where our
/// committed job has not yet appeared in the endpoint's reported queue.
fn endpoint_busy(load: &EndpointLoad) -> u64 {
    load.reported_pending.max(load.ours_active as u64)
}

/// Free capacity of the GPU slot (bucket) containing `member`. Busy sums
/// per-endpoint (each job queues on exactly one endpoint of the GPU);
/// capacity is the largest a member service advertises, floor 1, and a
/// bucket with no live member has none.
fn slot_free(loads: &[EndpointLoad], member: &EndpointLoad) -> u32 {
    let bucket: Vec<&EndpointLoad> = loads
        .iter()
        .filter(|load| load.slot_key == member.slot_key)
        .collect();
    if !bucket.iter().any(|load| load.up) {
        return 0;
    }
    let busy: u64 = bucket.iter().map(|load| endpoint_busy(load)).sum();
    let capacity = bucket
        .iter()
        .map(|load| load.capacity.max(1))
        .max()
        .unwrap_or(1);
    (capacity as u64).saturating_sub(busy) as u32
}

#[derive(Debug, PartialEq, Eq)]
pub enum DispatchPlan {
    /// Start now. `avoid` = every endpoint of every full GPU slot; the
    /// per-stage affinity router picks the best among the rest.
    Start { avoid: Vec<String> },
    /// Every compatible GPU slot is occupied — hold in the app-side queue.
    Wait { reason: String },
    /// No live endpoint advertises the capability: start anyway so the
    /// stage fails visibly as a service gap instead of parking silently.
    NoCapability,
}

pub fn plan_run(domain: &str, loads: &[EndpointLoad]) -> DispatchPlan {
    let capable: Vec<&EndpointLoad> = loads
        .iter()
        .filter(|load| load.up && load.capable)
        .collect();
    if capable.is_empty() {
        return DispatchPlan::NoCapability;
    }
    let admitted: Vec<&EndpointLoad> = capable
        .iter()
        .copied()
        .filter(|load| !load.vram_waiting)
        .collect();
    if admitted.is_empty() {
        return DispatchPlan::Wait {
            reason: format!(
                "all {} {domain}-capable GPU slot{} waiting for VRAM",
                capable_slots(loads).len(),
                if capable_slots(loads).len() == 1 { "" } else { "s" }
            ),
        };
    }
    // Count SLOTS, not endpoints: two capable ports on one GPU are one unit
    // of capacity in both the free check and the wait reason.
    let mut capable_slots: Vec<&str> = Vec::new();
    for load in &capable {
        if !capable_slots.contains(&load.slot_key.as_str()) {
            capable_slots.push(load.slot_key.as_str());
        }
    }
    let free = admitted
        .iter()
        .filter(|load| slot_free(loads, load) > 0)
        .count();
    if free == 0 {
        return DispatchPlan::Wait {
            reason: format!(
                "all {} {domain}-capable GPU slot{} busy",
                capable_slots.len(),
                if capable_slots.len() == 1 { "" } else { "s" }
            ),
        };
    }
    // Avoid EVERY endpoint of every full slot — including a free-looking
    // sibling port whose GPU is already taken. Free slots' endpoints stay
    // unavoided even when not first-stage-capable: later chain stages may
    // need exactly them.
    let avoid = loads
        .iter()
        .filter(|load| slot_free(loads, load) == 0)
        .map(|load| load.base_url.clone())
        .collect();
    DispatchPlan::Start { avoid }
}

fn capable_slots(loads: &[EndpointLoad]) -> Vec<&str> {
    let mut slots = Vec::new();
    for load in loads.iter().filter(|load| load.up && load.capable) {
        if !slots.contains(&load.slot_key.as_str()) {
            slots.push(load.slot_key.as_str());
        }
    }
    slots
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(url: &str, up: bool, capable: bool, reported: u64, ours: u32) -> EndpointLoad {
        EndpointLoad {
            base_url: url.to_string(),
            slot_key: slot_key(url, None),
            up,
            capable,
            vram_waiting: false,
            reported_pending: reported,
            ours_active: ours,
            capacity: 1,
        }
    }

    #[test]
    fn slot_key_prefers_advertised_gpu_id_then_host_ip() {
        assert_eq!(
            slot_key("http://10.0.0.169:8123", Some("GPU-9f3a")),
            "gpu:GPU-9f3a"
        );
        assert_eq!(slot_key("http://10.0.0.169:8123", None), "host:10.0.0.169");
        assert_eq!(slot_key("http://10.0.0.169:8080", None), "host:10.0.0.169");
        assert_eq!(slot_key("https://box.local/", None), "host:box.local");
        assert_eq!(slot_key("10.0.0.5:9000", None), "host:10.0.0.5");
    }

    /// Host/node identity dedupe regression. The deployment invariant is
    /// one service per PC (root removed the duplicate .169:8080 fleet
    /// entry), but a stale entry or rogue second process must STILL never
    /// double-book the card: committing a run through one port consumes the
    /// whole slot and the second video run waits.
    #[test]
    fn two_ports_on_one_host_are_one_gpu_slot() {
        let loads = vec![
            endpoint("http://10.0.0.169:8123", true, true, 0, 1),
            endpoint("http://10.0.0.169:8080", true, true, 0, 0),
        ];
        assert_eq!(
            plan_run("video", &loads),
            DispatchPlan::Wait {
                reason: "all 1 video-capable GPU slot busy".into()
            }
        );

        // With a second HOST free, dispatch avoids BOTH .169 ports — the
        // free-looking sibling included.
        let loads = vec![
            endpoint("http://10.0.0.169:8123", true, true, 0, 1),
            endpoint("http://10.0.0.169:8080", true, true, 0, 0),
            endpoint("http://10.0.0.217:8767", true, true, 0, 0),
        ];
        match plan_run("video", &loads) {
            DispatchPlan::Start { avoid } => {
                assert!(avoid.contains(&"http://10.0.0.169:8123".to_string()));
                assert!(avoid.contains(&"http://10.0.0.169:8080".to_string()));
                assert!(!avoid.contains(&"http://10.0.0.217:8767".to_string()));
            }
            other => panic!("expected Start, got {other:?}"),
        }

        // Same story when the sibling port is the busy one via an EXTERNAL
        // job and only the capable port looks idle.
        let loads = vec![
            endpoint("http://10.0.0.169:8123", true, true, 0, 0),
            endpoint("http://10.0.0.169:8080", true, false, 1, 0),
        ];
        assert!(matches!(plan_run("video", &loads), DispatchPlan::Wait { .. }));
    }

    /// The demo acceptance shape: five healthy slots on five DISTINCT hosts
    /// take five clicks onto five distinct GPUs; the sixth waits.
    #[test]
    fn five_distinct_hosts_run_five_concurrent_videos_then_queue() {
        let urls = [
            "http://10.0.0.11:8767",
            "http://10.0.0.12:8767",
            "http://10.0.0.13:8767",
            "http://10.0.0.14:8767",
            "http://10.0.0.15:8767",
        ];
        let mut ours = vec![0u32; urls.len()];
        for click in 0..5 {
            let loads: Vec<EndpointLoad> = urls
                .iter()
                .zip(&ours)
                .map(|(url, ours)| endpoint(url, true, true, 0, *ours))
                .collect();
            match plan_run("video", &loads) {
                DispatchPlan::Start { avoid } => {
                    // Exactly the committed hosts are avoided, so this run
                    // lands on a fresh GPU.
                    assert_eq!(avoid.len(), click, "click {click}");
                    let target = urls
                        .iter()
                        .position(|url| !avoid.iter().any(|a| a == url))
                        .expect("a free host remains");
                    ours[target] += 1;
                }
                other => panic!("click {click}: expected Start, got {other:?}"),
            }
        }
        assert!(ours.iter().all(|count| *count == 1), "no double-booking");

        let loads: Vec<EndpointLoad> = urls
            .iter()
            .zip(&ours)
            .map(|(url, ours)| endpoint(url, true, true, 0, *ours))
            .collect();
        assert_eq!(
            plan_run("video", &loads),
            DispatchPlan::Wait {
                reason: "all 5 video-capable GPU slots busy".into()
            }
        );
    }

    #[test]
    fn advertised_multi_gpu_capacity_is_the_only_same_host_concurrency() {
        // Conservative default: same host, no advertisement → one slot.
        let default_cap = vec![
            endpoint("http://10.0.0.169:8123", true, true, 0, 1),
            endpoint("http://10.0.0.169:8080", true, true, 0, 0),
        ];
        assert!(matches!(
            plan_run("video", &default_cap),
            DispatchPlan::Wait { .. }
        ));

        // A service EXPLICITLY advertising two GPUs unlocks a second run…
        let mut two_gpu = default_cap.clone();
        two_gpu[0].capacity = 2;
        match plan_run("video", &two_gpu) {
            DispatchPlan::Start { avoid } => assert!(avoid.is_empty()),
            other => panic!("expected Start, got {other:?}"),
        }
        // …and exactly two: the bucket then fills.
        two_gpu[1].ours_active = 1;
        assert!(matches!(
            plan_run("video", &two_gpu),
            DispatchPlan::Wait { .. }
        ));
    }

    #[test]
    fn bucket_busy_sums_ports_but_never_double_counts_one_job() {
        // Window A: our job committed via :8123 AND already visible in that
        // port's reported queue — max() per endpoint counts it once.
        let seen_both_ways = vec![
            EndpointLoad {
                capacity: 2,
                ..endpoint("http://10.0.0.169:8123", true, true, 1, 1)
            },
            endpoint("http://10.0.0.169:8080", true, true, 0, 0),
        ];
        match plan_run("video", &seen_both_ways) {
            DispatchPlan::Start { avoid } => assert!(avoid.is_empty()),
            other => panic!("expected Start, got {other:?}"),
        }

        // Window B: an external job reported on :8123 plus our fresh commit
        // on :8080 are DIFFERENT jobs — they sum to a full 2-GPU bucket.
        let two_distinct_jobs = vec![
            EndpointLoad {
                capacity: 2,
                ..endpoint("http://10.0.0.169:8123", true, true, 1, 0)
            },
            endpoint("http://10.0.0.169:8080", true, true, 0, 1),
        ];
        assert!(matches!(
            plan_run("video", &two_distinct_jobs),
            DispatchPlan::Wait { .. }
        ));
    }

    #[test]
    fn capability_gaps_and_down_endpoints_are_honest() {
        // Capability nowhere live: dispatch-to-visible-failure, never park.
        let loads = vec![
            endpoint("http://10.0.0.11:8767", true, false, 0, 0),
            endpoint("http://10.0.0.12:8767", false, true, 0, 0),
        ];
        assert_eq!(plan_run("world", &loads), DispatchPlan::NoCapability);
        // A slot whose only member is down has no free capacity.
        let down = endpoint("http://10.0.0.12:8767", false, true, 0, 0);
        assert_eq!(slot_free(std::slice::from_ref(&down), &down), 0);
    }

    #[test]
    fn full_h3_waits_on_96gb_pressure_and_excludes_24_32gb_nodes() {
        // `capable` already incorporates TOTAL-VRAM fit from fleet.rs. The
        // 24/32 GiB endpoints therefore cannot masquerade as H3 slots; .169
        // is the sole compatible slot but its latest FREE-VRAM admission is
        // temporarily blocked.
        let mut loads = vec![
            EndpointLoad {
                vram_waiting: true,
                ..endpoint("http://10.0.0.169:8123", true, true, 0, 0)
            },
            endpoint("http://10.0.0.217:8767", true, false, 0, 0),
            endpoint("http://10.0.0.100:8767", true, false, 0, 0),
        ];
        assert_eq!(
            plan_run("video", &loads),
            DispatchPlan::Wait {
                reason: "all 1 video-capable GPU slot waiting for VRAM".into()
            }
        );

        // A later health poll reports enough free memory. The same queued
        // run is now dispatchable on .169; undersized nodes remain excluded.
        loads[0].vram_waiting = false;
        assert_eq!(
            plan_run("video", &loads),
            DispatchPlan::Start { avoid: Vec::new() }
        );
    }

    #[test]
    fn expand_occupied_covers_slot_mates_and_unknown_urls() {
        let pairs: Vec<(String, String)> = [
            "http://10.0.0.169:8123",
            "http://10.0.0.169:8080",
            "http://10.0.0.217:8767",
        ]
        .iter()
        .map(|url| (url.to_string(), slot_key(url, None)))
        .collect();
        // Occupying one .169 port expands to its sibling, not to .217.
        let expanded = expand_occupied(&pairs, &["http://10.0.0.169:8123".to_string()]);
        assert!(expanded.contains(&"http://10.0.0.169:8123".to_string()));
        assert!(expanded.contains(&"http://10.0.0.169:8080".to_string()));
        assert!(!expanded.contains(&"http://10.0.0.217:8767".to_string()));
        // An occupied url no longer in the fleet list is still avoided.
        let gone = expand_occupied(&pairs, &["http://10.0.0.99:1".to_string()]);
        assert_eq!(gone, vec!["http://10.0.0.99:1".to_string()]);
    }
}
