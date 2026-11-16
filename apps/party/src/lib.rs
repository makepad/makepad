//! party: guests' phones send short camera loops, album photos and prompts to
//! the party box, which redraws them with its AI hub for the big screen.
//!
//! - `server`: the one HTTPS service guests reach (TLS, bounded HTTP, caps).
//! - `cert`: the box's self-signed identity on its own addresses.
//! - `api`: what guests' phones may ask, and the party it changes.
//! - `store`: guests, their sources and takes, keep levels.
//! - `queue`: the takes waiting for the hub worker.
//! - `worker`: the one thread that takes them to the hub.
//! - `start`: starting a party (identity, store, worker, service), shared
//!   by the party server and the big screen.
//! - `wall_plan`: the big screen's layout and the size its pictures are
//!   decoded at.
//! - `camtest`: the phase-0 camera check page.
//! - `jpeg`: what the server checks in a guest's picture, and the metadata it
//!   strips.
//! - `image`: pixel conversions between guests' JPEGs and the models' PNGs.
//! - `hub`: the client of the box's own AI hub (jobs and live sessions).
//! - `live`: one guest at a time live on the big screen: the queue, the
//!   slot, and the WebRTC calls (Linux).

pub mod api;
pub mod camtest;
pub mod cert;
pub mod code;
pub mod hub;
pub mod image;
pub mod jpeg;
pub mod live;
pub mod queue;
pub mod server;
pub mod start;
pub mod store;
pub mod wall_plan;
pub mod worker;
