//! makepad-mixer library: protocol, safety, model and the session client.
//! The UI binary lives in main.rs; everything here is plain std so the
//! whole safety surface is testable headless.
//!
//! There is NO fake/offline mode in the application: it talks to the real
//! console or to nothing. The in-process fake console survives only as a
//! TEST fixture (`#[cfg(test)] mod fake`) — it is what the live-wire tests
//! push every forbidden address at.
//!
//! START AT src/safety.rs — it explains the two-layer guarantee (closed
//! whitelist enum + deny list at the single socket write) that makes
//! dangerous console messages unconstructable.

pub mod client;
pub mod model;
pub mod osc;
pub mod safety;
pub mod units;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod livewire;
