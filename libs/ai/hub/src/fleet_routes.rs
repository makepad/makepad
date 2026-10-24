//! Which hub routes each fleet role reaches through the TLS front
//! (makepad_network::fleet::front applies it; anything refused is a 403).

use makepad_network::fleet::auth::Role;

/// Which routes a role may reach (method, path). `edge` narrows every role
/// to the device set.
pub fn route_allowed(role: Role, edge: bool, method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    let device = matches!(
        (method, path),
        ("GET", "/health") | ("GET", "/models") | ("GET", "/voices") | ("POST", "/generate") | ("POST", "/realtime")
    ) || (method == "GET" && (path.starts_with("/job/") || path.starts_with("/artifact/") || path.starts_with("/realtime/")))
        || (method == "POST" && path.starts_with("/job/") && (path.ends_with("/cancel") || path.ends_with("/keepalive")))
        || (method == "POST" && path == "/bye");
    if edge {
        return device;
    }
    match role {
        Role::Lan => true,
        // Paired devices also fetch the on-device model files: read-only,
        // hash-addressed (the blob endpoint), on the LAN front only.
        // And the control panel's routes (CONTROL_API.md): paired devices
        // may watch and steer the node they are paired with.
        Role::Device => device
            || (method == "GET" && path.starts_with(crate::peer_serve::BLOB_PATH_PREFIX))
            || path == "/control"
            || path == "/control/watch"
            || path == "/control/model"
            || path == "/llm/context",
        // Nodes fetch model blobs from each other (and check health).
        Role::Node => (method == "GET" && path.starts_with(crate::peer_serve::BLOB_PATH_PREFIX)) || (method == "GET" && path == "/health"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_policy() {
        assert!(route_allowed(Role::Lan, false, "GET", "/jobs"));
        assert!(!route_allowed(Role::Device, false, "GET", "/jobs"));
        assert!(route_allowed(Role::Device, false, "GET", "/control/watch?since=3"));
        assert!(route_allowed(Role::Device, false, "POST", "/control/model"));
        assert!(!route_allowed(Role::Device, true, "GET", "/control"));
        assert!(!route_allowed(Role::Node, false, "GET", "/control"));
        assert!(route_allowed(Role::Device, false, "POST", "/generate"));
        assert!(route_allowed(Role::Device, false, "GET", "/voices"));
        assert!(route_allowed(Role::Device, false, "GET", "/job/abc"));
        assert!(route_allowed(Role::Device, false, "GET", "/v1/model_blob/00"));
        assert!(!route_allowed(Role::Device, false, "POST", "/v1/model_blob/00"));
        assert!(!route_allowed(Role::Device, true, "GET", "/v1/model_blob/00"));
        assert!(!route_allowed(Role::Device, false, "GET", "/v1/model_inventory"));
        assert!(route_allowed(Role::Node, false, "GET", "/v1/model_blob/00"));
        assert!(!route_allowed(Role::Node, false, "POST", "/generate"));
        // The edge narrows even a LAN credential to the device set.
        assert!(!route_allowed(Role::Lan, true, "GET", "/jobs"));
        assert!(!route_allowed(Role::Lan, true, "GET", "/v1/model_inventory"));
        assert!(route_allowed(Role::Lan, true, "GET", "/models"));
    }
}
