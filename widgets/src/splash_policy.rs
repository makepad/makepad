//! What an isolate is allowed, enforced where the runtime acts.
//!
//! The `host` bridge reports a capability list and the storage jail enforces
//! a quota, but until now nothing REFUSED a request on the strength of that
//! list, and nothing bounded how much script an isolate could run over its
//! life. This module holds the per-heap policy for both, and the rest of the
//! splash runtime asks it:
//!
//! - [`service_allowed`] — before a `host.request` is queued;
//! - [`url_allowed`] — from every network path, through the gate installed
//!   in `makepad-script-std`;
//! - [`charge`] — after every evaluation and callback, against a cumulative
//!   instruction budget.
//!
//! Like the jail and the bridge, state is host-side and keyed by heap, where
//! script can neither read nor raise it. An isolate with no policy set keeps
//! the behaviour every existing host relied on: services pass through to the
//! host, URLs are allowed, and only the per-evaluation cap applies.
use crate::makepad_draw::makepad_platform::makepad_script_std;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct HeapPolicy {
    /// Granted capability names. A service `a.b.c` needs capability `a`, or
    /// the exact name `a.b.c`.
    pub capabilities: Vec<String>,
    /// Hosts this heap may reach, lowercased, exact. Empty means none.
    pub hosts: Vec<String>,
    /// Script instructions this heap may run over its life. None = unbounded
    /// (the per-evaluation cap still applies).
    pub instruction_budget: Option<u64>,
    pub instructions_used: u64,
    /// Set once the budget is spent; nothing runs in this heap afterwards.
    pub exhausted: bool,
}

thread_local! {
    static POLICIES: RefCell<HashMap<usize, HeapPolicy>> = RefCell::new(HashMap::new());
}

/// Set (or replace) the policy for a heap. Enforcement starts here: a heap
/// that never had this called is not enforced.
pub fn set_policy_for_heap(heap_key: usize, capabilities: Vec<String>, hosts: Vec<String>, instruction_budget: Option<u64>) {
    POLICIES.with(|p| {
        let mut p = p.borrow_mut();
        let used = p.get(&heap_key).map(|old| old.instructions_used).unwrap_or(0);
        let exhausted = instruction_budget.map(|b| used >= b).unwrap_or(false);
        p.insert(
            heap_key,
            HeapPolicy {
                capabilities,
                hosts: hosts.into_iter().map(|h| h.to_ascii_lowercase()).collect(),
                instruction_budget,
                instructions_used: used,
                exhausted,
            },
        );
    });
}

/// Drop policies for reclaimed isolates (from the isolate GC).
pub(crate) fn gc_policies(dead_heaps: &[usize]) {
    POLICIES.with(|p| {
        let mut p = p.borrow_mut();
        for heap in dead_heaps {
            p.remove(heap);
        }
    });
}

/// Whether this heap is under an enforced policy at all.
pub fn is_enforced(heap_key: usize) -> bool {
    POLICIES.with(|p| p.borrow().contains_key(&heap_key))
}

/// May this heap ask the host for `service`? `Ok` when no policy is set (the
/// host decides, as before) or when the policy grants it; `Err` names what is
/// missing, in words meant for a log.
pub fn service_allowed(heap_key: usize, service: &str) -> Result<(), String> {
    POLICIES.with(|p| {
        let p = p.borrow();
        let Some(policy) = p.get(&heap_key) else { return Ok(()) };
        if policy.exhausted {
            return Err("this app's instruction budget is spent".into());
        }
        let family = service.split('.').next().unwrap_or(service);
        if policy.capabilities.iter().any(|c| c == service || c == family) {
            Ok(())
        } else {
            Err(format!("this app was not granted {family:?}, which {service:?} needs"))
        }
    })
}

/// May this heap reach `url`? True when no policy is set. With a policy, the
/// URL's host must be listed exactly; a URL with no host is refused.
pub fn url_allowed(heap_key: usize, url: &str) -> bool {
    POLICIES.with(|p| {
        let p = p.borrow();
        let Some(policy) = p.get(&heap_key) else { return true };
        if policy.exhausted {
            return false;
        }
        // A listed `host` matches any port; a listed `host:port` matches
        // only that port, which is how a host lists its own asset origin.
        let host = makepad_script_std::url_host(url);
        let host_port = makepad_script_std::url_host_port(url);
        policy.hosts.iter().any(|h| Some(h) == host.as_ref() || Some(h) == host_port.as_ref())
    })
}

/// Record `instructions` run by this heap. Returns false once the budget is
/// spent, and stays false: the heap is exhausted from then on.
pub fn charge(heap_key: usize, instructions: u64) -> bool {
    POLICIES.with(|p| {
        let mut p = p.borrow_mut();
        let Some(policy) = p.get_mut(&heap_key) else { return true };
        policy.instructions_used = policy.instructions_used.saturating_add(instructions);
        if let Some(budget) = policy.instruction_budget {
            if policy.instructions_used >= budget {
                policy.exhausted = true;
            }
        }
        !policy.exhausted
    })
}

/// Whether this heap may still run anything.
pub fn may_run(heap_key: usize) -> bool {
    POLICIES.with(|p| p.borrow().get(&heap_key).map(|policy| !policy.exhausted).unwrap_or(true))
}

/// Instructions used so far, for a host that wants to show it.
pub fn instructions_used(heap_key: usize) -> u64 {
    POLICIES.with(|p| p.borrow().get(&heap_key).map(|policy| policy.instructions_used).unwrap_or(0))
}

/// The gate installed into `makepad-script-std`, once per thread, so every
/// request path consults the table above.
pub(crate) fn install_url_gate() {
    thread_local! { static INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    INSTALLED.with(|installed| {
        if !installed.get() {
            makepad_script_std::set_script_url_gate(Some(url_allowed));
            installed.set(true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpoliced_heap_behaves_as_before() {
        gc_policies(&[900]);
        assert!(service_allowed(900, "location.get").is_ok());
        assert!(url_allowed(900, "https://anything.example/x"));
        assert!(charge(900, 1_000_000));
        assert!(may_run(900));
    }

    #[test]
    fn a_service_needs_its_capability_family_or_its_exact_name() {
        set_policy_for_heap(901, vec!["location".into(), "ledger.read".into()], vec![], None);
        assert!(service_allowed(901, "location.get").is_ok());
        assert!(service_allowed(901, "location.watch").is_ok());
        assert!(service_allowed(901, "ledger.read").is_ok());
        assert!(service_allowed(901, "ledger.write").is_err(), "ledger.read does not cover ledger.write");
        assert!(service_allowed(901, "clipboard.read").is_err());
        gc_policies(&[901]);
    }

    #[test]
    fn a_url_must_name_a_listed_host_exactly() {
        set_policy_for_heap(902, vec!["net".into()], vec!["Api.Weather.Example".into()], None);
        assert!(url_allowed(902, "https://api.weather.example/v1"));
        assert!(url_allowed(902, "HTTPS://API.WEATHER.EXAMPLE:443/"));
        assert!(!url_allowed(902, "https://weather.example/"), "a parent domain is not the listed host");
        assert!(!url_allowed(902, "https://evil.api.weather.example/"), "nor is a subdomain");
        assert!(!url_allowed(902, "http://127.0.0.1:8170/ux-images/x.svg"), "the artwork server that leaked");
        assert!(!url_allowed(902, "garbage"));
        gc_policies(&[902]);
    }

    #[test]
    fn a_listed_host_port_matches_only_that_port() {
        set_policy_for_heap(907, vec![], vec!["127.0.0.1:5000".into()], None);
        assert!(url_allowed(907, "http://127.0.0.1:5000/assets/a.svg"));
        assert!(!url_allowed(907, "http://127.0.0.1:8170/ux-images/a.svg"), "another loopback service");
        assert!(!url_allowed(907, "http://127.0.0.1/a.svg"), "nor the bare host");
        gc_policies(&[907]);
    }

    #[test]
    fn an_empty_host_list_reaches_nothing_even_with_the_capability() {
        set_policy_for_heap(903, vec!["net".into()], vec![], None);
        assert!(!url_allowed(903, "https://api.weather.example/"));
        gc_policies(&[903]);
    }

    #[test]
    fn the_budget_is_cumulative_and_exhaustion_is_permanent() {
        set_policy_for_heap(904, vec!["location".into()], vec!["a.example".into()], Some(1000));
        assert!(charge(904, 400));
        assert!(charge(904, 400));
        assert!(may_run(904));
        assert!(!charge(904, 400), "the third charge crosses the budget");
        assert!(!may_run(904));
        assert!(service_allowed(904, "location.get").is_err(), "an exhausted heap gets no services");
        assert!(!url_allowed(904, "https://a.example/"), "nor network");
        assert!(!charge(904, 1), "and stays exhausted");
        assert_eq!(instructions_used(904), 1201);
        gc_policies(&[904]);
    }

    #[test]
    fn replacing_a_policy_keeps_what_was_already_spent() {
        set_policy_for_heap(905, vec![], vec![], Some(100));
        charge(905, 90);
        set_policy_for_heap(905, vec![], vec![], Some(100));
        assert_eq!(instructions_used(905), 90, "a re-push of the same policy is not a refill");
        set_policy_for_heap(905, vec![], vec![], Some(50));
        assert!(!may_run(905), "lowering the budget below what is spent exhausts the heap");
        gc_policies(&[905]);
    }

    #[test]
    fn gc_forgets_a_heap_entirely() {
        set_policy_for_heap(906, vec![], vec![], Some(1));
        charge(906, 5);
        gc_policies(&[906]);
        assert!(!is_enforced(906));
        assert!(may_run(906));
    }
}
