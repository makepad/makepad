//! Tesla Fleet API client for makepad apps, built on the makepad network
//! layer (Cx::http_request / Event::NetworkResponses). Data-oriented: made to
//! poll battery + charge status of your own car for charger-aware routing.
//!
//! Credentials setup (one-time, per car owner): see libs/tesla/README.md.
//!
//! ```ignore
//! // in your App
//! #[rust] tesla: Option<TeslaClient>,
//!
//! // startup
//! match TeslaClient::load_default() {
//!     Ok(client) => self.tesla = Some(client),
//!     Err(e) => log!("{}", e),
//! }
//! // kick off a poll (auto-refreshes the oauth token as needed)
//! if let Some(tesla) = &mut self.tesla {
//!     tesla.request_charge_state(cx, "LRW3E7EK4NC000000");
//! }
//!
//! // in AppMain::handle_event
//! if let Some(tesla) = &mut self.tesla {
//!     for action in tesla.handle_event(cx, event) {
//!         match action {
//!             TeslaAction::VehicleData { vin, data } => {
//!                 if let Some(charge) = &data.charge_state {
//!                     log!("{}: {}% / {} km", vin,
//!                         charge.usable_battery_level.unwrap_or(0),
//!                         charge.battery_range_km().unwrap_or(0.0) as i64);
//!                 }
//!             }
//!             TeslaAction::VehicleAsleep { vin } => { /* wake or retry later */ }
//!             TeslaAction::Error(e) => log!("{}", e),
//!             _ => {}
//!         }
//!     }
//! }
//! ```

pub mod client;
pub mod data;

pub use client::{
    TeslaAction, TeslaClient, TeslaCredentials, TeslaError, TeslaRegion, AUTH_TOKEN_URL,
};
pub use data::{
    ChargeState, DriveState, Vehicle, VehicleData, VehicleDataEndpoint, MILES_TO_KM,
};
