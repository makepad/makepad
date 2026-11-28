use relax_api::relax_api;
use crate::makepad_network::{NetworkRuntime, NetworkRuntimeHandle};
use crate::{net::*, run::*, task::*};
use std::sync::Arc;

/// Capabilities granted by the embedding host. Platform sinks can add bits here
/// as they adopt the same policy. Only persistent grants pass to child VMs.
#[relax_api(plain, run)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostGrants(u32);

impl HostGrants {
    pub const NONE: Self = Self(0);
    pub const NETWORK: Self = Self(1 << 0);
    pub const LOCAL_RESOURCES: Self = Self(1 << 1);
    pub const CLIPBOARD: Self = Self(1 << 2);
    pub const OPEN_URL: Self = Self(1 << 3);
    pub const BROWSER_NAV: Self = Self(1 << 4);
    pub const DESKTOP_SHELL: Self = Self(1 << 5);
    pub const DRAG: Self = Self(1 << 6);
    pub const NATIVE_LAYERS: Self = Self(1 << 7);
    pub const FILE_DIALOGS: Self = Self(1 << 8);
    pub const DEVICE_IO: Self = Self(1 << 9);
    pub const DESKTOP_ALL: Self = Self(
        Self::CLIPBOARD.0 | Self::OPEN_URL.0 | Self::BROWSER_NAV.0 | Self::DESKTOP_SHELL.0
            | Self::DRAG.0 | Self::NATIVE_LAYERS.0 | Self::FILE_DIALOGS.0 | Self::DEVICE_IO.0,
    );
    pub const ALL: Self = Self(Self::NETWORK.0 | Self::LOCAL_RESOURCES.0 | Self::DESKTOP_ALL.0);

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, grant: Self) -> bool {
        self.0 & grant.0 == grant.0
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

#[relax_api(handle, default, run)]
pub struct ScriptStd {
    pub net: Option<NetworkRuntimeHandle>,
    pub data: ScriptData,
    grants: HostGrants,
    scoped_grants: HostGrants,
}

impl Default for ScriptStd {
    fn default() -> Self {
        Self {
            net: None,
            data: ScriptData::default(),
            grants: HostGrants::ALL,
            scoped_grants: HostGrants::NONE,
        }
    }
}

#[relax_api(run)]
impl ScriptStd {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn with_network_runtime(net: Arc<NetworkRuntime>) -> Self {
        Self::with_network_handle(net.into())
    }

    #[relax_api(run)]
    pub fn with_network_handle(net: NetworkRuntimeHandle) -> Self {
        Self {
            net: Some(net),
            ..Self::default()
        }
    }

    #[inline(always)]
    pub fn set_network_runtime(&mut self, net: Arc<NetworkRuntime>) {
        self.set_network_handle(net.into());
    }

    pub fn set_network_handle(&mut self, net: NetworkRuntimeHandle) {
        if self.network_allowed() {
            self.net = Some(net);
        }
    }

    pub fn grants(&self) -> HostGrants {
        self.grants
    }

    #[inline]
    pub fn has_grant(&self, grant: HostGrants) -> bool {
        self.grants.contains(grant)
    }

    pub fn network_allowed(&self) -> bool {
        self.has_grant(HostGrants::NETWORK)
    }

    pub fn set_grants(&mut self, grants: HostGrants) {
        self.grants = grants;
        if !grants.contains(HostGrants::NETWORK) {
            self.net = None;
        }
    }

    pub fn restrict_grants(&mut self, mask: HostGrants) {
        self.set_grants(self.grants.intersection(mask));
    }

    /// Scoped grants additionally require a trusted script call site. They
    /// never contribute to a guest prelude or a nested isolate's authority.
    pub fn scoped_grants(&self) -> HostGrants {
        self.scoped_grants
    }

    pub fn set_scoped_grants(&mut self, grants: HostGrants) -> HostGrants {
        std::mem::replace(&mut self.scoped_grants, grants)
    }
}

#[relax_api(handle, run)]
#[derive(Default)]
pub struct ScriptData {
    pub tasks: ScriptTasks,
    pub child_processes: Vec<ScriptChildProcessState>,
    pub web_sockets: Vec<ScriptWebSocket>,
    pub socket_streams: std::rc::Rc<std::cell::RefCell<Vec<ScriptSocketStream>>>,
    pub http_requests: Vec<ScriptHttp>,
    pub http_servers: Vec<ScriptHttpServer>,
}

unsafe impl makepad_script::ScriptHostCapability for ScriptStd {
    const ID: makepad_script::LiveId = makepad_script::LiveId::from_str("makepad.script.std.v1");
}
