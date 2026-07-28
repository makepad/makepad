use makepad_micro_serde::*;

pub const MILES_TO_KM: f64 = 1.609344;

/// Subset of the Fleet API `vehicle_data` endpoints query values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VehicleDataEndpoint {
    ChargeState,
    ClimateState,
    DriveState,
    LocationData,
    VehicleState,
    VehicleConfig,
    GuiSettings,
    ChargeScheduleData,
}

impl VehicleDataEndpoint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChargeState => "charge_state",
            Self::ClimateState => "climate_state",
            Self::DriveState => "drive_state",
            Self::LocationData => "location_data",
            Self::VehicleState => "vehicle_state",
            Self::VehicleConfig => "vehicle_config",
            Self::GuiSettings => "gui_settings",
            Self::ChargeScheduleData => "charge_schedule_data",
        }
    }
}

#[derive(DeJson, Debug, Clone)]
pub struct VehiclesResponse {
    pub response: Option<Vec<Vehicle>>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(DeJson, Debug, Clone)]
pub struct Vehicle {
    pub vin: Option<String>,
    pub display_name: Option<String>,
    /// "online" | "asleep" | "offline"
    pub state: Option<String>,
    pub in_service: Option<bool>,
}

impl Vehicle {
    pub fn is_online(&self) -> bool {
        self.state.as_deref() == Some("online")
    }
}

#[derive(DeJson, Debug, Clone)]
pub struct VehicleDataResponse {
    pub response: Option<VehicleData>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(DeJson, Debug, Clone)]
pub struct VehicleData {
    pub vin: Option<String>,
    pub state: Option<String>,
    pub charge_state: Option<ChargeState>,
    pub drive_state: Option<DriveState>,
}

/// The `charge_state` block of `vehicle_data`. Everything routing cares about.
/// All fields optional: Tesla adds/removes fields between firmware versions.
#[derive(DeJson, Debug, Clone, Default)]
pub struct ChargeState {
    /// Displayed percent 0-100.
    pub battery_level: Option<i32>,
    /// Percent corrected for cold battery; use this for range planning.
    pub usable_battery_level: Option<i32>,
    /// Rated range in miles.
    pub battery_range: Option<f64>,
    /// Range estimated from recent consumption, miles.
    pub est_battery_range: Option<f64>,
    /// "Charging" | "Complete" | "Disconnected" | "Stopped" | "NoPower" | "Starting"
    pub charging_state: Option<String>,
    /// Charge limit percent.
    pub charge_limit_soc: Option<i32>,
    /// mi of range added per hour while charging.
    pub charge_rate: Option<f64>,
    /// Current charging power in kW.
    pub charger_power: Option<i32>,
    pub charger_voltage: Option<i32>,
    pub charger_actual_current: Option<i32>,
    pub minutes_to_full_charge: Option<i32>,
    /// Hours, fractional.
    pub time_to_full_charge: Option<f64>,
    pub fast_charger_present: Option<bool>,
    pub fast_charger_type: Option<String>,
    /// Cable type when plugged in, e.g. "IEC" / "SAE"; "<invalid>" when not.
    pub conn_charge_cable: Option<String>,
    /// kWh added this session.
    pub charge_energy_added: Option<f64>,
    pub battery_heater_on: Option<bool>,
    /// Milliseconds since epoch, set by the car.
    pub timestamp: Option<u64>,
}

impl ChargeState {
    pub fn battery_range_km(&self) -> Option<f64> {
        self.battery_range.map(|mi| mi * MILES_TO_KM)
    }

    pub fn est_battery_range_km(&self) -> Option<f64> {
        self.est_battery_range.map(|mi| mi * MILES_TO_KM)
    }

    pub fn is_charging(&self) -> bool {
        self.charging_state.as_deref() == Some("Charging")
    }

    pub fn is_plugged_in(&self) -> bool {
        matches!(
            self.charging_state.as_deref(),
            Some("Charging") | Some("Complete") | Some("Stopped") | Some("NoPower") | Some("Starting")
        )
    }
}

/// Only populated with lat/long when `location_data` is in the requested endpoints
/// (plain `drive_state` no longer carries the position).
#[derive(DeJson, Debug, Clone, Default)]
pub struct DriveState {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub heading: Option<i32>,
    /// mph; null when parked.
    pub speed: Option<f64>,
    /// kW; negative while regen/charging.
    pub power: Option<i32>,
    pub shift_state: Option<String>,
    pub timestamp: Option<u64>,
}

#[derive(DeJson, Debug, Clone)]
pub struct WakeUpResponse {
    pub response: Option<Vehicle>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Response of the oauth2 token endpoint (refresh flow).
#[derive(DeJson, Debug, Clone)]
pub struct TokenResponse {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Seconds.
    pub expires_in: Option<u64>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Generic Fleet API error body, for calls that failed with a non-2xx status.
#[derive(DeJson, Debug, Clone)]
pub struct ErrorBody {
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_charge_state_ignores_unknown_fields() {
        let json = r#"{
            "response": {
                "vin": "LRW3E7EK4NC000000",
                "state": "online",
                "charge_state": {
                    "battery_level": 72,
                    "usable_battery_level": 70,
                    "battery_range": 224.47,
                    "est_battery_range": 171.24,
                    "charging_state": "Charging",
                    "charge_limit_soc": 80,
                    "charge_rate": 27.1,
                    "charger_power": 11,
                    "charger_voltage": 229,
                    "charger_actual_current": 16,
                    "minutes_to_full_charge": 42,
                    "time_to_full_charge": 0.7,
                    "fast_charger_present": false,
                    "fast_charger_type": "<invalid>",
                    "conn_charge_cable": "IEC",
                    "charge_energy_added": 12.3,
                    "battery_heater_on": false,
                    "charge_port_door_open": true,
                    "scheduled_charging_mode": "Off",
                    "timestamp": 1604977209418
                },
                "drive_state": {
                    "latitude": 52.379189,
                    "longitude": 4.899431,
                    "heading": 194,
                    "speed": null,
                    "power": -11,
                    "shift_state": null,
                    "timestamp": 1604977209418
                }
            }
        }"#;
        let parsed = VehicleDataResponse::deserialize_json_lenient(json).unwrap();
        let data = parsed.response.unwrap();
        let charge = data.charge_state.unwrap();
        assert_eq!(charge.battery_level, Some(72));
        assert_eq!(charge.usable_battery_level, Some(70));
        assert!(charge.is_charging());
        assert!(charge.is_plugged_in());
        assert!((charge.battery_range_km().unwrap() - 361.25).abs() < 0.1);
        let drive = data.drive_state.unwrap();
        assert!((drive.latitude.unwrap() - 52.379189).abs() < 1e-9);
        assert_eq!(drive.speed, None);
    }

    #[test]
    fn parse_error_body() {
        let json = r#"{"response":null,"error":"vehicle unavailable: vehicle is offline or asleep","error_description":""}"#;
        let parsed = VehicleDataResponse::deserialize_json_lenient(json).unwrap();
        assert!(parsed.response.is_none());
        assert!(parsed.error.unwrap().contains("unavailable"));
    }
}
