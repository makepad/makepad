//! Fleet identity on the web: a browser serves nothing and reaches no fleet
//! machine directly (see id.rs for the native model).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Device,
    Node,
    Paired,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Device => "device",
            Role::Node => "node",
            Role::Paired => "paired",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        match text {
            "device" => Some(Role::Device),
            "node" => Some(Role::Node),
            "paired" => Some(Role::Paired),
            _ => None,
        }
    }
}

pub fn mark_fleet_endpoint(_host_port: &str) {}

pub fn is_fleet_endpoint(_host_port: &str) -> bool {
    false
}

pub fn same_node(_a: &str, _b: &str) -> bool {
    false
}
