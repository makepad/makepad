//! Which hub routes each fleet peer reaches through the TLS front
//! (makepad_network::fleet::front applies it; anything refused is a 403).
//! Peers come from makepad_network::fleet::id (tools/fleet-identity.md):
//! - the admin (the Mac's device credential from its bound address) reaches
//!   everything: control (incl. switching the Claude service), the panel,
//!   every job;
//! - a fleet machine (a node, or the Mac's credential from elsewhere) reaches
//!   what fleet job routing needs: the job API (the Claude service included,
//!   while the user has it on), kernels, model blobs and inventories;
//! - this machine's own credential (the WM hosting the hub, proven by the
//!   hub's own key) also reaches the machine-local control: the panel's
//!   picture and watch, local use, GPU yield and model load/unload. Admin
//!   settings (`/llm/context`, the panel pages) stay with the admin;
//! - a paired phone reaches its own jobs and the on-device model files.

use makepad_network::fleet::id::{Peer, Role};

/// The job API: submit, poll, fetch, cancel; listing models and voices.
fn job_route(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("GET", "/health") | ("GET", "/models") | ("GET", "/voices") | ("POST", "/generate") | ("POST", "/realtime") | ("POST", "/bye")
    ) || (method == "GET" && (path.starts_with("/job/") || path.starts_with("/artifact/") || path.starts_with("/realtime/")))
        || (method == "POST" && path.starts_with("/job/") && (path.ends_with("/cancel") || path.ends_with("/keepalive")))
}

/// What the WM hosting this hub needs of it: the control picture (`GET /control`, its
/// watch), local use, GPU memory back for the UI, and loading or unloading a model.
fn machine_control_route(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("GET", "/control") | ("GET", "/control/watch") | ("POST", "/control/local-use") | ("POST", "/control/yield") | ("POST", "/control/model")
    )
}

fn blob_route(method: &str, path: &str) -> bool {
    method == "GET" && path.starts_with(crate::peer_serve::BLOB_PATH_PREFIX)
}

/// Whether `peer` may reach `method path`. `edge` (the internet front)
/// narrows everyone to the job API.
pub fn route_allowed(peer: &Peer, edge: bool, method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    if edge {
        return job_route(method, path);
    }
    if peer.admin() {
        return true;
    }
    if peer.this_machine() && machine_control_route(method, path) {
        return true;
    }
    match peer.role {
        Role::Node | Role::Device => {
            job_route(method, path)
                || blob_route(method, path)
                || (method == "GET" && matches!(path, "/v1/model_inventory" | "/jobs" | "/loras"))
                || path == "/kernel"
                || path.starts_with("/kernel/")
        }
        Role::Paired => job_route(method, path) || blob_route(method, path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn peer(role: Role, addr: &str, from: Option<&str>) -> Peer {
        Peer { name: "x".into(), role, addr: addr.parse().unwrap(), from: from.map(|f| f.parse::<IpAddr>().unwrap()), point: [4; 65], this_machine: false }
    }

    #[test]
    fn route_policy() {
        let admin = peer(Role::Device, "10.0.0.222", Some("10.0.0.222"));
        let mac_elsewhere = peer(Role::Device, "10.0.0.50", Some("10.0.0.222"));
        let node = peer(Role::Node, "10.0.0.217", None);
        let phone = peer(Role::Paired, "10.0.0.60", None);
        // Admin: control, panel, everything.
        for (m, p) in [("GET", "/control"), ("POST", "/control/model"), ("GET", "/control/watch?since=3"), ("POST", "/llm/context"), ("GET", "/index.html"), ("POST", "/control/yield")] {
            assert!(route_allowed(&admin, false, m, p), "{m} {p}");
            for other in [&mac_elsewhere, &node, &phone] {
                assert!(!route_allowed(other, false, m, p), "{m} {p} for {:?}", other.role);
            }
        }
        // Fleet machines: the job API, kernels, blobs, inventory.
        for who in [&node, &mac_elsewhere] {
            assert!(route_allowed(who, false, "POST", "/generate"));
            assert!(route_allowed(who, false, "GET", "/job/abc"));
            assert!(route_allowed(who, false, "POST", "/kernel/jobs"));
            assert!(route_allowed(who, false, "GET", "/v1/model_blob/00"));
            assert!(route_allowed(who, false, "GET", "/v1/model_inventory"));
            assert!(!route_allowed(who, false, "POST", "/v1/model_blob/00"));
        }
        // The hub's own machine (its WM): machine-local control, not admin settings or pages.
        let own = Peer { this_machine: true, ..node.clone() };
        for (m, p) in [("GET", "/control"), ("GET", "/control/watch?since=3"), ("POST", "/control/local-use"), ("POST", "/control/yield"), ("POST", "/control/model")] {
            assert!(route_allowed(&own, false, m, p), "{m} {p}");
            assert!(!route_allowed(&own, true, m, p), "edge {m} {p}");
        }
        for (m, p) in [("POST", "/llm/context"), ("GET", "/llm/context"), ("GET", "/index.html"), ("POST", "/control")] {
            assert!(!route_allowed(&own, false, m, p), "{m} {p}");
        }
        assert!(route_allowed(&own, false, "POST", "/generate"));
        // Local use is this machine's only: another node or the Mac away is refused.
        for other in [&node, &mac_elsewhere, &phone] {
            assert!(!route_allowed(other, false, "POST", "/control/local-use"));
        }
        // Phones: their jobs and model files.
        assert!(route_allowed(&phone, false, "POST", "/generate"));
        assert!(route_allowed(&phone, false, "GET", "/v1/model_blob/00"));
        assert!(!route_allowed(&phone, false, "GET", "/v1/model_inventory"));
        assert!(!route_allowed(&phone, false, "POST", "/kernel/jobs"));
        assert!(!route_allowed(&phone, false, "GET", "/jobs"));
        // The edge narrows even the admin to the job API.
        assert!(!route_allowed(&admin, true, "GET", "/control"));
        assert!(!route_allowed(&admin, true, "GET", "/v1/model_blob/00"));
        assert!(route_allowed(&admin, true, "GET", "/models"));
        assert!(route_allowed(&phone, true, "POST", "/generate"));
    }
}
