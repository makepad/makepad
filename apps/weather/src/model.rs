//! The weather model: the selectable cities, the Open-Meteo forecast shape,
//! WMO weather-code wording, and the fetch state machine (loading, failed,
//! stale-but-kept) that both faces render from.
use makepad_widgets::*;
use makepad_widgets::makepad_micro_serde::*;
use std::time::{Duration, Instant};

pub struct City {
    pub name: &'static str,
    pub country: &'static str,
    pub lat: f64,
    pub lon: f64,
}

pub const CITIES: [City; 4] = [
    City { name: "Amsterdam", country: "Netherlands", lat: 52.37, lon: 4.90 },
    City { name: "London", country: "United Kingdom", lat: 51.51, lon: -0.13 },
    City { name: "New York", country: "United States", lat: 40.71, lon: -74.01 },
    City { name: "Tokyo", country: "Japan", lat: 35.68, lon: 139.65 },
];

pub const DEFAULT_CITY: usize = 0;
pub const REFRESH_EVERY: Duration = Duration::from_secs(15 * 60);
/// A forecast body is a few kilobytes; anything past this is not one.
pub const MAX_BODY_BYTES: usize = 256 * 1024;
pub const ATTRIBUTION: &str = "Weather data by Open-Meteo.com (CC BY 4.0)";

pub fn forecast_url(city: &City) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.2}&longitude={:.2}\
         &current=temperature_2m,relative_humidity_2m,is_day,precipitation,weather_code,wind_speed_10m\
         &daily=temperature_2m_max,temperature_2m_min,weather_code&timezone=auto&forecast_days=5",
        city.lat, city.lon
    )
}

#[derive(Clone, Debug, Default, DeJson)]
pub struct Forecast {
    pub current: Current,
    pub daily: Daily,
}

#[derive(Clone, Debug, Default, DeJson)]
pub struct Current {
    pub temperature_2m: Option<f64>,
    pub relative_humidity_2m: Option<f64>,
    pub is_day: Option<f64>,
    pub precipitation: Option<f64>,
    pub weather_code: Option<f64>,
    pub wind_speed_10m: Option<f64>,
}

#[derive(Clone, Debug, Default, DeJson)]
pub struct Daily {
    pub time: Vec<String>,
    pub temperature_2m_max: Vec<Option<f64>>,
    pub temperature_2m_min: Vec<Option<f64>>,
    pub weather_code: Vec<Option<f64>>,
}

/// One forecast day, already checked for array-length agreement.
pub struct Day {
    pub date: String,
    pub weekday: &'static str,
    pub code: Option<u32>,
    pub high: Option<f64>,
    pub low: Option<f64>,
}

impl Forecast {
    pub fn parse(json: &str) -> Result<Self, String> {
        // Lenient: Open-Meteo sends fields we do not model (units, timezone,
        // elevation); strict decoding would reject the whole body for them.
        let f: Forecast = DeJson::deserialize_json_lenient(json).map_err(|e| format!("{e:?}"))?;
        if f.current.temperature_2m.is_none() || f.current.weather_code.is_none() {
            return Err("response has no current weather".into());
        }
        Ok(f)
    }

    pub fn days(&self) -> Vec<Day> {
        let d = &self.daily;
        d.time
            .iter()
            .enumerate()
            .take(5)
            .map(|(i, date)| Day {
                date: date.clone(),
                weekday: weekday_of(date),
                code: d.weather_code.get(i).copied().flatten().map(|c| c as u32),
                high: d.temperature_2m_max.get(i).copied().flatten(),
                low: d.temperature_2m_min.get(i).copied().flatten(),
            })
            .collect()
    }

    pub fn code(&self) -> u32 {
        self.current.weather_code.unwrap_or(0.0) as u32
    }

    pub fn is_day(&self) -> bool {
        self.current.is_day.unwrap_or(1.0) >= 0.5
    }

    pub fn today_high_low(&self) -> (Option<f64>, Option<f64>) {
        (
            self.daily.temperature_2m_max.first().copied().flatten(),
            self.daily.temperature_2m_min.first().copied().flatten(),
        )
    }
}

/// The sky artwork's coarse condition classes; see `sky.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyKind {
    Clear = 0,
    Partly = 1,
    Overcast = 2,
    Rain = 3,
    Snow = 4,
    Fog = 5,
    Thunder = 6,
}

/// WMO weather interpretation codes as Open-Meteo documents them.
pub fn describe(code: u32) -> &'static str {
    match code {
        0 => "Clear sky",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 => "Fog",
        48 => "Rime fog",
        51 => "Light drizzle",
        53 => "Drizzle",
        55 => "Dense drizzle",
        56 | 57 => "Freezing drizzle",
        61 => "Light rain",
        63 => "Rain",
        65 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71 => "Light snow",
        73 => "Snow",
        75 => "Heavy snow",
        77 => "Snow grains",
        80 => "Light showers",
        81 => "Showers",
        82 => "Violent showers",
        85 => "Snow showers",
        86 => "Heavy snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown conditions",
    }
}

pub fn sky_kind(code: u32) -> SkyKind {
    match code {
        0 | 1 => SkyKind::Clear,
        2 => SkyKind::Partly,
        3 => SkyKind::Overcast,
        45 | 48 => SkyKind::Fog,
        51..=67 | 80..=82 => SkyKind::Rain,
        71..=77 | 85 | 86 => SkyKind::Snow,
        95..=99 => SkyKind::Thunder,
        _ => SkyKind::Overcast,
    }
}

pub fn temp(v: Option<f64>) -> String {
    match v {
        Some(t) => format!("{}°", t.round() as i64),
        None => "–".into(),
    }
}

/// Weekday of an ISO `YYYY-MM-DD`, or the raw text when it is not one.
fn weekday_of(date: &str) -> &'static str {
    const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let mut it = date.split('-').map(|p| p.parse::<i64>().ok());
    let (Some(Some(y)), Some(Some(m)), Some(Some(d))) = (it.next(), it.next(), it.next()) else {
        return "";
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return "";
    }
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    NAMES[((days + 4).rem_euclid(7)) as usize]
}

/// What the faces show, independent of how it is laid out.
pub struct WeatherState {
    pub city: usize,
    /// The good forecast most recently received, tagged with its city.
    pub data: Option<(usize, Forecast, Instant)>,
    /// A request in flight: its id and the city it was sent for.
    pub pending: Option<(LiveId, usize)>,
    pub last_error: Option<String>,
    seq: u64,
}

impl Default for WeatherState {
    fn default() -> Self {
        Self { city: DEFAULT_CITY, data: None, pending: None, last_error: None, seq: 0 }
    }
}

impl WeatherState {
    pub fn city(&self) -> &'static City {
        &CITIES[self.city.min(CITIES.len() - 1)]
    }

    pub fn loading(&self) -> bool {
        self.pending.is_some()
    }

    /// A fresh request id; any earlier in-flight reply is now superseded.
    pub fn begin_fetch(&mut self) -> (LiveId, String) {
        self.seq += 1;
        let id = LiveId::from_str(&format!("weather_forecast_{}", self.seq));
        self.pending = Some((id, self.city));
        (id, forecast_url(self.city()))
    }

    pub fn select_city(&mut self, city: usize) -> bool {
        let city = city.min(CITIES.len() - 1);
        if city == self.city {
            return false;
        }
        self.city = city;
        // Whatever was in flight answers for the old city: drop it.
        self.pending = None;
        true
    }

    /// True when the id matches the request we are waiting for.
    pub fn owns(&self, id: LiveId) -> bool {
        self.pending.is_some_and(|(p, _)| p == id)
    }

    pub fn complete(&mut self, id: LiveId, result: Result<Forecast, String>) -> bool {
        if !self.owns(id) {
            return false;
        }
        let (_, city) = self.pending.take().unwrap();
        match result {
            Ok(f) => {
                self.data = Some((city, f, Instant::now()));
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e),
        }
        true
    }

    /// The good data to show: only if it belongs to the selected city.
    pub fn current(&self) -> Option<(&Forecast, Instant)> {
        self.data.as_ref().filter(|(c, _, _)| *c == self.city).map(|(_, f, t)| (f, *t))
    }

    pub fn due_for_refresh(&self) -> bool {
        if self.loading() {
            return false;
        }
        match self.current() {
            Some((_, at)) => at.elapsed() >= REFRESH_EVERY,
            None => true,
        }
    }

    pub fn is_stale(&self) -> bool {
        self.current().is_some_and(|(_, at)| at.elapsed() >= REFRESH_EVERY)
            || (self.current().is_some() && self.last_error.is_some())
    }

    /// One line of status for either face.
    pub fn status_text(&self) -> String {
        let age = |at: Instant| {
            let mins = at.elapsed().as_secs() / 60;
            if mins == 0 { "just now".to_string() } else { format!("{} min ago", mins) }
        };
        match (self.loading(), self.current(), &self.last_error) {
            (true, None, _) => "Loading…".into(),
            (true, Some((_, at)), _) => format!("Refreshing · updated {}", age(at)),
            (false, None, Some(e)) => format!("Offline · {}", e),
            (false, None, None) => "No data yet".into(),
            (false, Some((_, at)), Some(_)) => format!("Stale · offline, showing data from {}", age(at)),
            (false, Some((_, at)), None) if self.is_stale() => format!("Stale · updated {}", age(at)),
            (false, Some((_, at)), None) => format!("Updated {}", age(at)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"latitude":52.38,"longitude":4.9,"timezone":"Europe/Amsterdam",
        "current":{"time":"2026-09-06T12:00","interval":900,"temperature_2m":18.4,"relative_humidity_2m":61,
        "is_day":1,"precipitation":0.0,"weather_code":2,"wind_speed_10m":14.7},
        "daily":{"time":["2026-09-06","2026-09-07","2026-09-08","2026-09-09","2026-09-10"],
        "temperature_2m_max":[20.1,21.3,null,19.0,18.2],"temperature_2m_min":[12.0,11.5,10.9,12.2,11.0],
        "weather_code":[2,3,61,80,0]}}"#;

    #[test]
    fn parses_open_meteo_and_tolerates_nulls() {
        let f = Forecast::parse(SAMPLE).unwrap();
        assert_eq!(f.code(), 2);
        assert!(f.is_day());
        let days = f.days();
        assert_eq!(days.len(), 5);
        assert_eq!(days[0].weekday, "Sun");
        assert_eq!(days[2].high, None);
        assert_eq!(temp(days[1].high), "21°");
    }

    #[test]
    fn malformed_bodies_are_errors_not_panics() {
        assert!(Forecast::parse("").is_err());
        assert!(Forecast::parse("<html>").is_err());
        assert!(Forecast::parse(r#"{"error":true,"reason":"nope"}"#).is_err());
        assert!(Forecast::parse(r#"{"current":{},"daily":{}}"#).is_err());
    }

    #[test]
    fn superseded_replies_are_ignored_and_good_data_survives_failure() {
        let mut s = WeatherState::default();
        let (first, _) = s.begin_fetch();
        assert!(s.complete(first, Forecast::parse(SAMPLE)));
        assert!(s.current().is_some());
        let (second, _) = s.begin_fetch();
        assert!(s.select_city(3));
        assert!(!s.complete(second, Err("late".into())), "old city's reply is dropped");
        assert!(s.current().is_none(), "Amsterdam data is not shown as Tokyo");
        let (third, _) = s.begin_fetch();
        assert!(s.complete(third, Err("offline".into())));
        assert!(s.status_text().starts_with("Offline"));
        s.select_city(0);
        assert!(s.current().is_some(), "last good Amsterdam data is still there");
        assert!(s.status_text().starts_with("Stale"));
    }

    #[test]
    fn url_is_the_documented_query() {
        let url = forecast_url(&CITIES[0]);
        assert!(url.starts_with("https://api.open-meteo.com/v1/forecast?latitude=52.37&longitude=4.90&current="));
        assert!(url.ends_with("&timezone=auto&forecast_days=5"));
    }
}
