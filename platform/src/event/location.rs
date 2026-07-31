//! Geolocation events, delivered after `Cx::start_location_updates()`.
//!
//! Backends: macOS/iOS CoreLocation, Android LocationManager, web
//! `navigator.geolocation`. Other platforms emit `LocationError::Unavailable`.

/// One position fix from the platform location service.
#[derive(Clone, Debug, PartialEq)]
pub struct LocationUpdateEvent {
    /// Longitude in degrees (WGS84).
    pub lon: f64,
    /// Latitude in degrees (WGS84).
    pub lat: f64,
    /// Horizontal accuracy radius in meters.
    pub accuracy_m: f64,
    /// Altitude above sea level in meters, when the fix has one.
    pub altitude_m: Option<f64>,
    /// Ground speed in m/s, when known.
    pub speed_mps: Option<f64>,
    /// Course over ground in degrees (0 = north, clockwise), when known.
    pub heading_deg: Option<f64>,
    /// Fix time as unix seconds (fractional).
    pub time: f64,
}

/// Location updates cannot be delivered (once per start attempt or on
/// permission loss). Updates may still resume later, e.g. when the user
/// grants permission through system settings.
#[derive(Clone, Debug, PartialEq)]
pub enum LocationErrorEvent {
    /// The user denied location permission.
    PermissionDenied,
    /// No location service on this platform/device, or an OS error.
    Unavailable(String),
}
