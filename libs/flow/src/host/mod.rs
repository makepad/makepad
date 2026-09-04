//! Native flow-server host: state owner, file watcher and bounded HTTP planes.

mod config;
mod batches;
mod events;
mod models;
mod parallelism;
mod routes;
mod server;
mod state;
mod util;
mod watcher;

pub use config::{DiscoveryConfig, FlowServerConfig};
pub use parallelism::estimate_parallelism;
pub use events::{EventCursor, EventHub, FlowEvent};
pub use server::{Endpoints, FlowServer, ServerError};
pub use state::{Definition, FlowState, NodeRow, RunRow, StateHandle};
