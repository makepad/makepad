//! Services exposed to another person's authenticated link.

use crate::carrier::Carrier;
use crate::conn::{Conn, ConnOptions, Hooks, Io, Open, ServiceLookup, ServiceOpen, CLOSE_REFUSED};
use crate::endpoint::ServiceLimits;
use crate::hello::{self, HandshakeError, Hello};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Supplied by the carrier after it authenticates this link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    pub device_key: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerService {
    Ping,
}

impl PeerService {
    pub const ALL: &'static [Self] = &[Self::Ping];

    pub fn name(self) -> &'static str {
        match self {
            Self::Ping => "ping",
        }
    }

    fn limits(self) -> ServiceLimits {
        match self {
            Self::Ping => ServiceLimits { max_message: 64 << 10, max_channels: 1 },
        }
    }

    fn caps(self) -> String {
        format!("{}/1", self.name())
    }

    fn open(self, identity: &PeerIdentity, admit: &Admit, open: Open) -> Result<String, (u16, String)> {
        admit(identity).map_err(|reason| (CLOSE_REFUSED, reason))?;
        match self {
            Self::Ping => open.channel.echo_on_reader(),
        }
        Ok(self.caps())
    }
}

type Admit = dyn Fn(&PeerIdentity) -> Result<(), String> + Send + Sync;

/// A peer registry, independent of the process's local inspection services.
#[derive(Clone)]
pub struct PeerEndpoint {
    name: String,
    services: Vec<(PeerService, Arc<Admit>)>,
}

impl PeerEndpoint {
    pub fn new(name: &str) -> Self {
        Self { name: name.into(), services: Vec::new() }
    }

    /// Registers the service's built-in handler. Its admission callback runs on the reader
    /// with the authenticated identity of this link. It must return promptly.
    pub fn register(&mut self, service: PeerService, admit: impl Fn(&PeerIdentity) -> Result<(), String> + Send + Sync + 'static) {
        self.services.retain(|(s, _)| *s != service);
        self.services.push((service, Arc::new(admit)));
    }

    pub fn caps(&self) -> Vec<String> {
        self.services.iter().map(|(service, _)| service.caps()).collect()
    }

    fn lookup(&self, identity: PeerIdentity) -> ServiceLookup {
        let identity = Arc::new(identity);
        let services: Vec<_> = self.services.iter().map(|(service, admit)| {
            let (service, admit, identity) = (*service, admit.clone(), identity.clone());
            let handler = Arc::new(move |_: &Conn, open| service.open(&identity, &*admit, open));
            (service.name(), ServiceOpen { limits: service.limits(), handler })
        }).collect();
        Arc::new(move |name| services.iter().find(|(s, _)| *s == name).map(|(_, open)| open.clone()))
    }

    /// Accepts an authenticated carrier and serves just this registry.
    pub fn serve(&self, carrier: Carrier, identity: PeerIdentity) -> Result<Conn, HandshakeError> {
        self.start(carrier, identity, false)
    }

    /// Dials over an authenticated carrier, offering the same peer registry to the other end.
    pub fn connect(&self, carrier: Carrier, identity: PeerIdentity) -> Result<Conn, HandshakeError> {
        self.start(carrier, identity, true)
    }

    fn start(&self, mut carrier: Carrier, identity: PeerIdentity, initiator: bool) -> Result<Conn, HandshakeError> {
        let mut options = ConnOptions::strict();
        options.open_max_message = |name| PeerService::ALL.iter().find(|s| s.name() == name).map_or(0, |s| s.limits().max_message);
        carrier.reader.deadline = Some(Instant::now() + Duration::from_secs(10));
        let own = Hello { name: self.name.clone(), caps: self.caps(), ..Hello::default() };
        let handshake = if initiator { hello::client_handshake_with_options } else { hello::server_handshake_with_options };
        let peer = match handshake(&mut carrier.reader, &mut *carrier.writer, &own, options) {
            Ok(peer) => peer,
            Err(err) => {
                (carrier.shutdown)();
                return Err(err);
            }
        };
        let io = Io { reader: carrier.reader, writer: carrier.writer, shutdown: carrier.shutdown };
        Ok(Conn::start_registry(io, peer, initiator, Some(self.lookup(identity)), Hooks::default(), options))
    }
}
