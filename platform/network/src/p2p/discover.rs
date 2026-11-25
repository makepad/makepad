//! Choose the address to read to a peer, and assess the direct path from both peers' hints.

use super::portmap::PortMapping;
use super::reflect::Mapping;
use std::net::SocketAddrV4;

/// What discovery established about reaching this socket. The invite carries these two-bit values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Hint {
    Unknown = 0,
    /// The router granted a publicly reachable port.
    Easy = 1,
    /// Two reflectors saw the same endpoint. The firewall's filtering is still unknown.
    ConeUnknownFiltering = 2,
    /// Reflectors at different destinations saw different endpoints.
    Symmetric = 3,
}

#[derive(Debug, Default)]
pub struct Found {
    /// The lease granted by PCP, NAT-PMP or UPnP. Keep it even if it only opens the inner router.
    pub router: Option<PortMapping>,
    /// The reflected endpoint and the mapping behaviour observed across destinations.
    pub reflected: Option<(SocketAddrV4, Mapping)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub address: Option<SocketAddrV4>,
    pub hint: Hint,
    /// The router reported a non-public outside address, indicating another NAT upstream.
    pub double_nat: bool,
}

impl Choice {
    /// A granted inner port stays open even when only a reflector can see the outer port.
    fn with_grant(self, public: Option<SocketAddrV4>) -> Self {
        match public {
            Some(address) => Self { address: Some(address), hint: Hint::Easy, double_nat: false },
            None => Self { double_nat: true, ..self },
        }
    }

    #[cfg(test)]
    pub(super) fn with_simulated_grant(self, external: SocketAddrV4) -> Self {
        // The simulator grants an endpoint without creating a router lease handle.
        self.with_grant(super::portmap::is_public_v4(*external.ip()).then_some(external))
    }
}

/// Select the endpoint for the words while leaving the router lease with its owner.
pub fn choose(found: &Found) -> Choice {
    let hint = match found.reflected {
        None | Some((_, Mapping::Unknown)) => Hint::Unknown,
        Some((_, Mapping::EndpointIndependent)) => Hint::ConeUnknownFiltering,
        Some((_, Mapping::DestinationDependent)) => Hint::Symmetric,
    };
    let choice = Choice { address: found.reflected.map(|(addr, _)| addr), hint, double_nat: false };
    match &found.router {
        Some(lease) => choice.with_grant(lease.public()),
        None => choice,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assessment {
    Likely,
    StillTrying,
    NotPossible,
}

impl Assessment {
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::StillTrying => Some("the firewalls may be strict"),
            _ => None,
        }
    }
}

/// Two destination-dependent mappings rule out the direct path. With a cone opposite one,
/// success depends on filtering and the ports the routers allocate.
pub fn assess(local: Hint, peer: Hint) -> Assessment {
    use Hint::{ConeUnknownFiltering, Symmetric};
    match (local, peer) {
        (Symmetric, Symmetric) => Assessment::NotPossible,
        (Symmetric, ConeUnknownFiltering) | (ConeUnknownFiltering, Symmetric) => Assessment::StillTrying,
        _ => Assessment::Likely,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_public_router_grant_takes_precedence_over_reflection() {
        let public = "192.0.2.7:41000".parse().unwrap();
        let outer = "198.51.100.23:50000".parse().unwrap();
        let easy = Choice { address: Some(public), hint: Hint::Easy, double_nat: false };
        let unknown = Choice { address: None, hint: Hint::Unknown, double_nat: false };
        assert_eq!(choose(&Found::default()), unknown);
        assert_eq!(unknown.with_simulated_grant(public), easy);

        for (mapping, hint) in [
            (Mapping::Unknown, Hint::Unknown),
            (Mapping::EndpointIndependent, Hint::ConeUnknownFiltering),
            (Mapping::DestinationDependent, Hint::Symmetric),
        ] {
            let reflected = Some((outer, mapping));
            let choice = choose(&Found { router: None, reflected });
            assert_eq!(choice, Choice { address: Some(outer), hint, double_nat: false });
            assert_eq!(choice.with_simulated_grant(public), easy);
            for inside in ["192.168.77.2:41000", "100.64.0.2:41000"] {
                let grant = inside.parse().unwrap();
                assert_eq!(choice.with_simulated_grant(grant), Choice { double_nat: true, ..choice }, "{inside}: use the reflected port");
                assert_eq!(unknown.with_simulated_grant(grant), Choice { double_nat: true, ..unknown }, "{inside}: wait for reflection");
            }
        }
    }

    #[test]
    fn assess_all_hint_pairs() {
        use Assessment::{Likely, NotPossible, StillTrying};
        let hints = [Hint::Unknown, Hint::Easy, Hint::ConeUnknownFiltering, Hint::Symmetric];
        let verdicts = [
            [Likely; 4],
            [Likely; 4],
            [Likely, Likely, Likely, StillTrying],
            [Likely, Likely, StillTrying, NotPossible],
        ];
        for (local, row) in hints.into_iter().zip(verdicts) {
            for (peer, expected) in hints.into_iter().zip(row) {
                assert_eq!(assess(local, peer), expected, "{local:?} facing {peer:?}");
            }
        }
        assert_eq!(StillTrying.note(), Some("the firewalls may be strict"));
    }
}
