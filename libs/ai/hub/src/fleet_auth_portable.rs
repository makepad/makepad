//! Fleet credentials on the web: a browser serves nothing and reaches no
//! fleet node directly (see fleet_auth.rs for the native model).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Lan,
    Node,
    Device,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Lan => "lan",
            Role::Node => "node",
            Role::Device => "device",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        match text {
            "lan" => Some(Role::Lan),
            "node" => Some(Role::Node),
            "device" => Some(Role::Device),
            _ => None,
        }
    }
}

pub fn mark_fleet_endpoint(_host_port: &str) {}

pub fn is_fleet_endpoint(_host_port: &str) -> bool {
    false
}
