//! Native flow-server host: the bounded HTTP planes, the flow files and
//! their watcher, and the JSON event journal, over one engine (`runtime`).

mod backend;
mod config;
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
pub use state::{Definition, NodeRow, RunRow};
