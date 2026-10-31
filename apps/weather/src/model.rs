//! The weather model: the city catalogue (every city the app knows, with
//! the position, wall clock and climate its sample forecast is generated
//! from), the forecast shape (current conditions, 24 hourly steps, ten
//! daily rows, solar events), WMO weather-code wording, the sky's inputs
//! (condition class, solar phase, cloud cover, precipitation intensity),
//! and the ten-day range domain and its temperature palette that both
//! faces render from.
//!
//! The app is offline: the forecast is sample data generated on the
//! device (`sample.rs`), never fetched. Timestamps are ISO strings in the
//! CITY'S wall time: every stamp is already the local time at that
//! instant, daylight saving included, so the labels read straight off the
//! string and no offset is ever applied twice. Arrays are matched by index
//! against `time` and tolerate gaps and unequal lengths.
use crate::sample::sample_forecast;
use makepad_widgets::*;
use std::time::{SystemTime, UNIX_EPOCH};

/// A city the app knows: its place, its wall clock and the climate its
/// sample forecast is drawn from. Every city comes from the catalogue
/// below, so the persisted list only needs names (`serialize_cities`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct City {
    pub name: &'static str,
    pub country: &'static str,
    pub lat: f64,
    pub lon: f64,
    /// Standard time's offset from UTC, in minutes.
    pub utc_offset_min: i32,
    pub dst: Dst,
    pub climate: Climate,
}

/// When a city's clocks go forward an hour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dst {
    None,
    /// The last Sunday of March to the last Sunday of October.
    Eu,
    /// The second Sunday of March to the first Sunday of November.
    Us,
    /// The southern summer: the first Sunday of October to the first of April.
    South,
}

/// The few numbers a city's sample weather is drawn from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Climate {
    /// Annual mean temperature, °C.
    pub mean: f64,
    /// Half the gap between the warmest and the coldest month, °C.
    pub swing: f64,
    /// 0 = desert dry, 1 = rain most days.
    pub wet: f64,
    /// Typical wind speed, km/h.
    pub wind: f64,
}

const fn city(name: &'static str, country: &'static str, lat: f64, lon: f64, utc_offset_min: i32, dst: Dst, mean: f64, swing: f64, wet: f64, wind: f64) -> City {
    City { name, country, lat, lon, utc_offset_min, dst, climate: Climate { mean, swing, wet, wind } }
}

/// Every city the Locations search can find; the first four are the list
/// a fresh install starts from.
const CATALOGUE: &[City] = &[
    // name, country, lat, lon, standard UTC offset (min), DST rule,
    // annual mean °C, seasonal swing °C, wetness 0..1, typical wind km/h.
    city("Amsterdam", "Netherlands", 52.37, 4.90, 60, Dst::Eu, 10.5, 7.5, 0.65, 18.0),
    city("London", "United Kingdom", 51.51, -0.13, 0, Dst::Eu, 11.5, 7.0, 0.55, 15.0),
    city("New York", "United States", 40.71, -74.01, -300, Dst::Us, 13.0, 12.0, 0.5, 16.0),
    city("Tokyo", "Japan", 35.68, 139.65, 540, Dst::None, 16.0, 10.5, 0.55, 12.0),
    city("Paris", "France", 48.86, 2.35, 60, Dst::Eu, 12.5, 8.0, 0.45, 13.0),
    city("Berlin", "Germany", 52.52, 13.40, 60, Dst::Eu, 10.0, 9.5, 0.45, 14.0),
    city("Zurich", "Switzerland", 47.37, 8.54, 60, Dst::Eu, 9.5, 9.0, 0.6, 9.0),
    city("Madrid", "Spain", 40.42, -3.70, 60, Dst::Eu, 15.0, 10.5, 0.25, 11.0),
    city("Lisbon", "Portugal", 38.72, -9.14, 0, Dst::Eu, 17.5, 6.0, 0.35, 15.0),
    city("Rome", "Italy", 41.90, 12.50, 60, Dst::Eu, 16.0, 8.5, 0.35, 10.0),
    city("Athens", "Greece", 37.98, 23.73, 120, Dst::Eu, 18.5, 9.5, 0.2, 14.0),
    city("Edinburgh", "United Kingdom", 55.95, -3.19, 0, Dst::Eu, 9.5, 5.5, 0.6, 20.0),
    city("Oslo", "Norway", 59.91, 10.75, 60, Dst::Eu, 6.5, 10.5, 0.5, 11.0),
    city("Helsinki", "Finland", 60.17, 24.94, 120, Dst::Eu, 6.5, 11.5, 0.5, 16.0),
    city("Reykjavik", "Iceland", 64.15, -21.94, 0, Dst::None, 5.0, 5.5, 0.7, 25.0),
    city("Istanbul", "Türkiye", 41.01, 28.98, 180, Dst::None, 15.0, 9.5, 0.45, 17.0),
    city("Cairo", "Egypt", 30.04, 31.24, 120, Dst::None, 22.5, 7.5, 0.02, 13.0),
    city("Nairobi", "Kenya", -1.29, 36.82, 180, Dst::None, 18.5, 1.5, 0.45, 12.0),
    city("Cape Town", "South Africa", -33.92, 18.42, 120, Dst::None, 17.0, 4.5, 0.35, 22.0),
    city("Dubai", "United Arab Emirates", 25.20, 55.27, 240, Dst::None, 28.0, 7.5, 0.03, 14.0),
    city("Mumbai", "India", 19.08, 72.88, 330, Dst::None, 27.5, 2.5, 0.5, 12.0),
    city("Singapore", "Singapore", 1.35, 103.82, 480, Dst::None, 27.5, 0.8, 0.8, 9.0),
    city("Hong Kong", "China", 22.32, 114.17, 480, Dst::None, 23.5, 6.0, 0.6, 14.0),
    city("Beijing", "China", 39.90, 116.40, 480, Dst::None, 13.0, 15.5, 0.25, 11.0),
    city("Seoul", "South Korea", 37.57, 126.98, 540, Dst::None, 12.5, 14.0, 0.45, 10.0),
    city("Sydney", "Australia", -33.87, 151.21, 600, Dst::South, 18.5, 5.0, 0.5, 16.0),
    city("Melbourne", "Australia", -37.81, 144.96, 600, Dst::South, 15.5, 5.5, 0.5, 18.0),
    city("Auckland", "New Zealand", -36.85, 174.76, 720, Dst::South, 15.5, 4.5, 0.6, 20.0),
    city("Anchorage", "United States", 61.22, -149.90, -540, Dst::Us, 3.0, 11.5, 0.45, 12.0),
    city("Vancouver", "Canada", 49.28, -123.12, -480, Dst::Us, 11.0, 7.0, 0.7, 12.0),
    city("San Francisco", "United States", 37.77, -122.42, -480, Dst::Us, 14.5, 3.0, 0.3, 18.0),
    city("Los Angeles", "United States", 34.05, -118.24, -480, Dst::Us, 18.5, 4.5, 0.1, 10.0),
    city("Chicago", "United States", 41.88, -87.63, -360, Dst::Us, 10.5, 14.5, 0.5, 18.0),
    city("Toronto", "Canada", 43.65, -79.38, -300, Dst::Us, 9.0, 13.5, 0.5, 16.0),
    city("Mexico City", "Mexico", 19.43, -99.13, -360, Dst::None, 17.0, 3.0, 0.4, 9.0),
    city("São Paulo", "Brazil", -23.55, -46.63, -180, Dst::None, 20.5, 3.5, 0.6, 10.0),
    city("Buenos Aires", "Argentina", -34.60, -58.38, -180, Dst::None, 18.0, 6.0, 0.45, 15.0),
];

pub fn city_catalogue() -> &'static [City] {
    CATALOGUE
}

/// The list a fresh install (or a wiped one) starts from.
pub fn default_cities() -> Vec<City> {
    CATALOGUE[..4].to_vec()
}

/// The Locations search: catalogue cities whose name (or, failing that,
/// country) starts a word with the query, names first.
pub fn search_cities(query: &str) -> Vec<City> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let starts_a_word = |text: &str| text.to_lowercase().split([' ', '-']).any(|w| w.starts_with(&q)) || text.to_lowercase().starts_with(&q);
    let mut out: Vec<City> = CATALOGUE.iter().filter(|c| starts_a_word(c.name)).copied().collect();
    out.extend(CATALOGUE.iter().filter(|c| !starts_a_word(c.name) && starts_a_word(c.country)).copied());
    out.truncate(MAX_CITIES);
    out
}

pub const DEFAULT_CITY: usize = 0;
/// A generous personal list, not a hard platform limit: the sheet's row
/// slots are declared up to this count (view.rs).
pub const MAX_CITIES: usize = 12;
/// Said wherever the forecast's source would be credited: none of it is
/// real weather.
pub const SAMPLE_NOTE: &str = "Sample forecast, generated on this device. Not real weather data.";
/// How many hourly cells the strip shows, "Now" first.
pub const HOURLY_CELLS: usize = 24;
pub const DAILY_ROWS: usize = 10;

/// `name\tcountry` per line: the catalogue holds everything else.
pub fn serialize_cities(cities: &[City]) -> Vec<u8> {
    let mut out = String::new();
    for city in cities {
        out.push_str(city.name);
        out.push('\t');
        out.push_str(city.country);
        out.push('\n');
    }
    out.into_bytes()
}

/// The persisted list, each line looked up in the catalogue; a line that
/// names no catalogue city is dropped.
pub fn parse_cities(bytes: &[u8]) -> Vec<City> {
    let Ok(text) = std::str::from_utf8(bytes) else { return Vec::new() };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let (name, country) = (parts.next()?, parts.next()?);
            CATALOGUE.iter().find(|c| c.name == name && c.country == country).copied()
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct Forecast {
    pub utc_offset_seconds: Option<f64>,
    pub current: Current,
    pub hourly: Hourly,
    pub daily: Daily,
}

#[derive(Clone, Debug, Default)]
pub struct Current {
    pub time: Option<String>,
    pub temperature_2m: Option<f64>,
    pub relative_humidity_2m: Option<f64>,
    pub apparent_temperature: Option<f64>,
    pub is_day: Option<f64>,
    pub precipitation: Option<f64>,
    pub weather_code: Option<f64>,
    pub cloud_cover: Option<f64>,
    pub wind_speed_10m: Option<f64>,
    pub wind_direction_10m: Option<f64>,
    pub wind_gusts_10m: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Hourly {
    pub time: Vec<String>,
    pub temperature_2m: Vec<Option<f64>>,
    pub weather_code: Vec<Option<f64>>,
    pub is_day: Vec<Option<f64>>,
    pub precipitation_probability: Vec<Option<f64>>,
    pub precipitation: Vec<Option<f64>>,
    pub rain: Vec<Option<f64>>,
    pub snowfall: Vec<Option<f64>>,
    pub cloud_cover: Vec<Option<f64>>,
    pub uv_index: Vec<Option<f64>>,
}

#[derive(Clone, Debug, Default)]
pub struct Daily {
    pub time: Vec<String>,
    pub temperature_2m_max: Vec<Option<f64>>,
    pub temperature_2m_min: Vec<Option<f64>>,
    pub weather_code: Vec<Option<f64>>,
    pub sunrise: Vec<Option<String>>,
    pub sunset: Vec<Option<String>>,
    pub uv_index_max: Vec<Option<f64>>,
    pub precipitation_probability_max: Vec<Option<f64>>,
    pub precipitation_sum: Vec<Option<f64>>,
}


/// One forecast day, already checked for array-length agreement.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: String,
    pub weekday: &'static str,
    pub code: Option<u32>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub sunrise: Option<String>,
    pub sunset: Option<String>,
    pub uv_max: Option<f64>,
    pub precip_chance: Option<f64>,
    pub precip_sum: Option<f64>,
}

/// One cell of the hourly strip: an hour, or a sunrise/sunset event
/// inserted between two hours.
#[derive(Clone, Debug, PartialEq)]
pub enum HourCell {
    Hour { label: String, code: u32, is_day: bool, chance: Option<f64>, temp: Option<f64>, is_now: bool },
    Sunrise { label: String },
    Sunset { label: String },
}

impl HourCell {
    pub fn label(&self) -> &str {
        match self {
            HourCell::Hour { label, .. } | HourCell::Sunrise { label } | HourCell::Sunset { label } => label,
        }
    }
}

impl Forecast {
    /// The ten daily rows, in order; fewer when the arrays are shorter.
    pub fn days(&self) -> Vec<Day> {
        let d = &self.daily;
        d.time
            .iter()
            .enumerate()
            .take(DAILY_ROWS)
            .map(|(i, date)| Day {
                date: date.clone(),
                weekday: weekday_of(date),
                code: d.weather_code.get(i).copied().flatten().map(|c| c as u32),
                high: d.temperature_2m_max.get(i).copied().flatten(),
                low: d.temperature_2m_min.get(i).copied().flatten(),
                sunrise: d.sunrise.get(i).cloned().flatten(),
                sunset: d.sunset.get(i).cloned().flatten(),
                uv_max: d.uv_index_max.get(i).copied().flatten(),
                precip_chance: d.precipitation_probability_max.get(i).copied().flatten(),
                precip_sum: d.precipitation_sum.get(i).copied().flatten(),
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

    /// The index of the hourly row the current observation falls in: the
    /// last row whose stamp is at or before `current.time`.
    pub fn now_index(&self) -> usize {
        let Some(now) = self.current.time.as_deref() else { return 0 };
        let mut index = 0;
        for (i, stamp) in self.hourly.time.iter().enumerate() {
            if stamp.as_str() <= now {
                index = i;
            } else {
                break;
            }
        }
        index
    }

    /// The strip: "Now" first, then the following hours, with sunrise and
    /// sunset cells inserted where they fall.
    pub fn hour_cells(&self) -> Vec<HourCell> {
        let h = &self.hourly;
        let start = self.now_index();
        let mut events: Vec<(String, bool)> = Vec::new();
        for day in self.days() {
            if let Some(s) = day.sunrise {
                events.push((s, true));
            }
            if let Some(s) = day.sunset {
                events.push((s, false));
            }
        }
        events.sort();
        let now_stamp = self.current.time.clone().unwrap_or_default();
        let mut out = Vec::new();
        let mut event_index = events.iter().position(|(stamp, _)| stamp.as_str() > now_stamp.as_str()).unwrap_or(events.len());
        for i in start..h.time.len() {
            if out.len() >= HOURLY_CELLS {
                break;
            }
            let stamp = &h.time[i];
            let is_now = i == start;
            // Events between the previous cell and this hour come first.
            while event_index < events.len() && !is_now && events[event_index].0.as_str() <= stamp.as_str() {
                let (when, rise) = &events[event_index];
                let label = hhmm_of(when);
                out.push(if *rise { HourCell::Sunrise { label } } else { HourCell::Sunset { label } });
                event_index += 1;
            }
            out.push(HourCell::Hour {
                label: if is_now { "Now".into() } else { hour_label(stamp) },
                code: h.weather_code.get(i).copied().flatten().unwrap_or(0.0) as u32,
                is_day: h.is_day.get(i).copied().flatten().unwrap_or(1.0) >= 0.5,
                chance: h.precipitation_probability.get(i).copied().flatten(),
                temp: h.temperature_2m.get(i).copied().flatten(),
                is_now,
            });
        }
        out
    }

    /// The current hour's UV index (never the daily maximum).
    pub fn current_uv(&self) -> Option<f64> {
        self.hourly.uv_index.get(self.now_index()).copied().flatten()
    }

    /// The current hour's precipitation intensity in mm/h, for the sky.
    pub fn current_precip_intensity(&self) -> f64 {
        let i = self.now_index();
        let rain = self.hourly.rain.get(i).copied().flatten().unwrap_or(0.0);
        let snow = self.hourly.snowfall.get(i).copied().flatten().unwrap_or(0.0);
        self.current.precipitation.unwrap_or(rain + snow).max(rain + snow)
    }

    /// What the sky paints from: the condition class, the solar phase
    /// (0 = night, 1 = day, in between = dawn or dusk), the warmth of a
    /// dawn/dusk (1 = clear horizon), cloud cover and precipitation.
    pub fn sky_inputs(&self) -> SkyInputs {
        let code = self.code();
        let kind = sky_kind(code);
        let cover = self.current.cloud_cover.unwrap_or(match kind {
            SkyKind::Clear => 10.0,
            SkyKind::Partly => 50.0,
            _ => 95.0,
        }) / 100.0;
        let today = self.days().into_iter().next();
        let phase = match (&self.current.time, &today) {
            (Some(now), Some(day)) => solar_phase(now, day.sunrise.as_deref(), day.sunset.as_deref(), self.is_day()),
            _ => if self.is_day() { 1.0 } else { 0.0 },
        };
        SkyInputs {
            kind,
            phase,
            cover: cover.clamp(0.0, 1.0),
            precip: (self.current_precip_intensity() / 4.0).clamp(0.0, 1.0),
        }
    }
}

/// The sky artwork's inputs; see `sky.rs`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyInputs {
    pub kind: SkyKind,
    /// 0 = night, 1 = day; the ±45 minute blend around sunrise and sunset.
    pub phase: f64,
    pub cover: f64,
    pub precip: f64,
}

/// Where `now` sits relative to the day's sunrise and sunset, blended over
/// ±45 minutes around each. Falls back to `is_day` when the events are
/// absent (polar days and nights have none).
pub fn solar_phase(now: &str, sunrise: Option<&str>, sunset: Option<&str>, is_day: bool) -> f64 {
    let (Some(rise), Some(set)) = (sunrise.and_then(minute_of_day), sunset.and_then(minute_of_day)) else {
        return if is_day { 1.0 } else { 0.0 };
    };
    let Some(t) = minute_of_day(now) else {
        return if is_day { 1.0 } else { 0.0 };
    };
    const BLEND: f64 = 45.0;
    let up = ((t - rise + BLEND) / (2.0 * BLEND)).clamp(0.0, 1.0);
    let down = ((set + BLEND - t) / (2.0 * BLEND)).clamp(0.0, 1.0);
    if rise <= set { up.min(down) } else { up.max(down) }
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
    Unknown = 7,
}

/// WMO weather interpretation codes.
pub fn describe(code: u32) -> &'static str {
    match code {
        0 => "Clear",
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
        _ => "Unknown",
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
        _ => SkyKind::Unknown,
    }
}

/// Which condition glyph a code gets, day or night: sun/moon, the partly
/// pair, cloud, drizzle, rain, snow, fog, thunder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Sun,
    Moon,
    PartlyDay,
    PartlyNight,
    Cloud,
    Drizzle,
    Rain,
    Snow,
    Fog,
    Thunder,
}

pub fn glyph(code: u32, is_day: bool) -> Glyph {
    match code {
        0 | 1 => if is_day { Glyph::Sun } else { Glyph::Moon },
        2 => if is_day { Glyph::PartlyDay } else { Glyph::PartlyNight },
        3 => Glyph::Cloud,
        45 | 48 => Glyph::Fog,
        51..=57 => Glyph::Drizzle,
        61..=67 | 80..=82 => Glyph::Rain,
        71..=77 | 85 | 86 => Glyph::Snow,
        95..=99 => Glyph::Thunder,
        _ => Glyph::Cloud,
    }
}

/// The display unit for a temperature. Every value the model holds (the
/// forecast, the range domain, the fixed Celsius palette below) stays in
/// Celsius regardless; only `temp()`'s formatted text ever converts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TempUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

impl TempUnit {
    pub fn toggle(self) -> Self {
        match self {
            TempUnit::Celsius => TempUnit::Fahrenheit,
            TempUnit::Fahrenheit => TempUnit::Celsius,
        }
    }

    /// The toggle's own label: the unit shown is the one currently active.
    pub fn label(self) -> &'static str {
        match self {
            TempUnit::Celsius => "°C",
            TempUnit::Fahrenheit => "°F",
        }
    }

    fn convert(self, celsius: f64) -> f64 {
        match self {
            TempUnit::Celsius => celsius,
            TempUnit::Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
        }
    }

    pub fn storage_byte(self) -> u8 {
        match self {
            TempUnit::Celsius => b'C',
            TempUnit::Fahrenheit => b'F',
        }
    }

    pub fn from_storage(bytes: &[u8]) -> Option<Self> {
        match bytes.first() {
            Some(b'F') => Some(TempUnit::Fahrenheit),
            Some(b'C') => Some(TempUnit::Celsius),
            _ => None,
        }
    }
}

pub fn temp(v: Option<f64>, unit: TempUnit) -> String {
    match v {
        Some(t) => format!("{}°", unit.convert(t).round() as i64),
        None => "—°".into(),
    }
}

/// UV index wording as the WHO scale names it.
pub fn uv_text(uv: Option<f64>) -> (String, &'static str) {
    match uv {
        None => ("—".into(), "Unavailable"),
        Some(v) => {
            let n = v.round().max(0.0) as i64;
            let word = match n {
                0..=2 => "Low",
                3..=5 => "Moderate",
                6..=7 => "High",
                8..=10 => "Very high",
                _ => "Extreme",
            };
            (format!("{n}"), word)
        }
    }
}

/// A compass name for a wind direction in degrees.
pub fn compass(deg: f64) -> &'static str {
    const NAMES: [&str; 16] = ["N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW", "NNW"];
    NAMES[(((deg.rem_euclid(360.0) + 11.25) / 22.5).floor() as usize) % 16]
}

/// The one domain every ten-day range bar shares: the coldest low to the
/// warmest high across all rows, widened for today's observation.
pub fn range_domain(days: &[Day], observed: Option<f64>) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for d in days {
        if let Some(v) = d.low {
            lo = lo.min(v);
        }
        if let Some(v) = d.high {
            hi = hi.max(v);
        }
    }
    if let Some(v) = observed {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0);
    }
    (lo, hi)
}

/// Where a temperature sits in the domain, 0..1; the design's
/// `x = 154 + 134 * clamp((T-a)/max(b-a,1), 0, 1)` without the pixels.
pub fn range_fraction(t: f64, domain: (f64, f64)) -> f64 {
    ((t - domain.0) / (domain.1 - domain.0).max(1.0)).clamp(0.0, 1.0)
}

/// The fixed Celsius palette the range bars are coloured with.
pub const RANGE_PALETTE: [(f64, u32); 7] = [
    (-10.0, 0x9C9AFF),
    (0.0, 0x6BAEFF),
    (10.0, 0x64D3E9),
    (20.0, 0x8BDA9A),
    (25.0, 0xF0D36E),
    (30.0, 0xF5A35B),
    (35.0, 0xED7868),
];

/// The palette colour for a temperature, blended between stops.
pub fn range_color(t: f64) -> Vec4f {
    let rgb = |c: u32| vec4(((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, 1.0);
    if t <= RANGE_PALETTE[0].0 {
        return rgb(RANGE_PALETTE[0].1);
    }
    for w in RANGE_PALETTE.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let f = ((t - t0) / (t1 - t0)) as f32;
            let (a, b) = (rgb(c0), rgb(c1));
            return a + (b - a) * f;
        }
    }
    rgb(RANGE_PALETTE[RANGE_PALETTE.len() - 1].1)
}

/// "14" from an ISO stamp's hour; the raw text when it is not one.
pub fn hour_label(stamp: &str) -> String {
    match stamp.split('T').nth(1).and_then(|t| t.get(0..2)).and_then(|h| h.parse::<u32>().ok()) {
        Some(h) => format!("{h}"),
        None => stamp.to_string(),
    }
}

/// "06:52" from an ISO stamp.
pub fn hhmm_of(stamp: &str) -> String {
    stamp.split('T').nth(1).map(|t| t.chars().take(5).collect()).unwrap_or_else(|| stamp.to_string())
}

/// The wall clock right now at a city, from the forecast's own
/// `utc_offset_seconds` (daylight saving included): it keeps ticking
/// instead of freezing at the minute the forecast was generated.
pub fn local_hhmm(utc_offset_seconds: Option<f64>) -> String {
    let utc_now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let secs_of_day = (utc_now + utc_offset_seconds.unwrap_or(0.0) as i64).rem_euclid(86_400);
    format!("{:02}:{:02}", secs_of_day / 3600, (secs_of_day % 3600) / 60)
}

fn minute_of_day(stamp: &str) -> Option<f64> {
    let time = stamp.split('T').nth(1)?;
    let mut parts = time.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.get(0..2)?.parse().ok()?;
    Some(h * 60.0 + m)
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
    /// A person's own list: the defaults, plus anything added from the
    /// Locations sheet's search (capped at `MAX_CITIES`, persisted under
    /// storage's "cities" key).
    pub cities: Vec<City>,
    pub city: usize,
    pub unit: TempUnit,
}

impl Default for WeatherState {
    fn default() -> Self {
        Self { cities: default_cities(), city: DEFAULT_CITY, unit: TempUnit::default() }
    }
}

impl WeatherState {
    pub fn city(&self) -> &City {
        &self.cities[self.city.min(self.cities.len() - 1)]
    }

    /// The selected city's sample forecast as of right now: generated,
    /// never fetched, so it is there from the first frame.
    pub fn forecast(&self) -> Forecast {
        let utc_now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        sample_forecast(self.city(), utc_now)
    }

    pub fn select_city(&mut self, city: usize) -> bool {
        let city = city.min(self.cities.len() - 1);
        if city == self.city {
            return false;
        }
        self.city = city;
        true
    }

    /// Add a city from a search result and return its index — the
    /// existing one's, unchanged, if it is already on the list, so
    /// searching for a city already added just selects it rather than
    /// duplicating it. `None` only when the list is full of cities that
    /// are not this one.
    pub fn add_city(&mut self, city: City) -> Option<usize> {
        if let Some(i) = self.cities.iter().position(|c| c.name == city.name && c.country == city.country) {
            return Some(i);
        }
        if self.cities.len() >= MAX_CITIES {
            return None;
        }
        self.cities.push(city);
        Some(self.cities.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-written forecast with gaps (`NAN` = missing), the shape the
    /// views must tolerate.
    fn fixture() -> Forecast {
        let o = |v: &[f64]| v.iter().map(|x| if x.is_nan() { None } else { Some(*x) }).collect::<Vec<_>>();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let so = |v: &[&str]| v.iter().map(|x| if x.is_empty() { None } else { Some(x.to_string()) }).collect::<Vec<_>>();
        const NAN: f64 = f64::NAN;
        Forecast {
            utc_offset_seconds: Some(7200.0),
            current: Current {
                time: Some("2026-09-06T12:10".into()),
                temperature_2m: Some(18.4),
                relative_humidity_2m: Some(61.0),
                apparent_temperature: Some(17.1),
                is_day: Some(1.0),
                precipitation: Some(0.0),
                weather_code: Some(2.0),
                cloud_cover: Some(40.0),
                wind_speed_10m: Some(14.7),
                wind_direction_10m: Some(247.0),
                wind_gusts_10m: Some(31.0),
            },
            hourly: Hourly {
                time: s(&["2026-09-06T11:00", "2026-09-06T12:00", "2026-09-06T13:00", "2026-09-06T14:00", "2026-09-06T19:00", "2026-09-06T20:00"]),
                temperature_2m: o(&[17.0, 18.0, 19.2, NAN, 15.0, 14.0]),
                weather_code: o(&[2.0, 2.0, 3.0, 61.0, 0.0, 0.0]),
                is_day: o(&[1.0, 1.0, 1.0, 1.0, 1.0, 0.0]),
                precipitation_probability: o(&[0.0, 5.0, 20.0, 60.0, 0.0, 0.0]),
                precipitation: o(&[0.0, 0.0, 0.2, 1.4, 0.0, 0.0]),
                rain: o(&[0.0, 0.0, 0.2, 1.4, 0.0, 0.0]),
                snowfall: o(&[0.0; 6]),
                cloud_cover: o(&[30.0, 40.0, 80.0, 100.0, 10.0, 5.0]),
                uv_index: o(&[3.1, 4.4, 4.0, 1.2, 0.3, 0.0]),
            },
            daily: Daily {
                time: s(&["2026-09-06", "2026-09-07", "2026-09-08", "2026-09-09", "2026-09-10", "2026-09-11", "2026-09-12", "2026-09-13", "2026-09-14", "2026-09-15"]),
                temperature_2m_max: o(&[20.1, 21.3, NAN, 19.0, 18.2, 22.0, 24.5, 23.1, 20.0, 18.0]),
                temperature_2m_min: o(&[12.0, 11.5, 10.9, 12.2, 11.0, 12.5, 13.0, 14.0, 11.0, 9.5]),
                weather_code: o(&[2.0, 3.0, 61.0, 80.0, 0.0, 1.0, 0.0, 2.0, 3.0, 61.0]),
                sunrise: so(&["2026-09-06T07:02", "2026-09-07T07:04", "", "2026-09-09T07:07", "2026-09-10T07:09", "2026-09-11T07:10", "2026-09-12T07:12", "2026-09-13T07:14", "2026-09-14T07:15", "2026-09-15T07:17"]),
                sunset: so(&["2026-09-06T19:35", "2026-09-07T19:33", "", "2026-09-09T19:28", "2026-09-10T19:26", "2026-09-11T19:24", "2026-09-12T19:21", "2026-09-13T19:19", "2026-09-14T19:17", "2026-09-15T19:14"]),
                uv_index_max: o(&[4.4, 5.0, 2.1, 3.0, 4.0, 5.5, 6.0, 5.8, 3.2, 2.0]),
                precipitation_probability_max: o(&[5.0, 20.0, 80.0, 60.0, 0.0, 0.0, 0.0, 10.0, 30.0, 70.0]),
                precipitation_sum: o(&[0.0, 0.4, 6.2, 2.1, 0.0, 0.0, 0.0, 0.1, 1.0, 4.4]),
            },
        }
    }

    #[test]
    fn ten_days_and_hourly_tolerate_gaps() {
        let f = fixture();
        assert_eq!(f.code(), 2);
        assert!(f.is_day());
        let days = f.days();
        assert_eq!(days.len(), 10);
        assert_eq!(days[0].weekday, "Sun");
        assert_eq!(days[2].high, None);
        assert_eq!(days[2].sunrise, None);
        assert_eq!(days[1].sunset.as_deref(), Some("2026-09-07T19:33"));
        assert_eq!(temp(days[1].high, TempUnit::Celsius), "21°");
        assert_eq!(days[0].uv_max, Some(4.4));
        assert_eq!(days[0].precip_sum, Some(0.0));
        assert_eq!(compass(247.0), "WSW");
    }

    #[test]
    fn the_strip_starts_now_and_inserts_solar_events() {
        let f = fixture();
        assert_eq!(f.now_index(), 1, "12:10 falls in the 12:00 row");
        let cells = f.hour_cells();
        assert!(matches!(&cells[0], HourCell::Hour { label, is_now: true, temp: Some(t), .. } if label == "Now" && *t == 18.0));
        assert!(matches!(&cells[1], HourCell::Hour { label, chance: Some(c), .. } if label == "13" && *c == 20.0));
        assert!(matches!(&cells[2], HourCell::Hour { label, temp: None, code: 61, .. } if label == "14"));
        // Sunset at 19:35 falls between the 19:00 and 20:00 rows.
        assert!(matches!(&cells[3], HourCell::Hour { label, .. } if label == "19"));
        assert!(matches!(&cells[4], HourCell::Sunset { label } if label == "19:35"));
        assert!(matches!(&cells[5], HourCell::Hour { label, is_day: false, .. } if label == "20"));
        assert_eq!(cells.len(), 6);
        assert_eq!(f.current_uv(), Some(4.4), "the current hour's UV, not the daily maximum");
    }

    #[test]
    fn the_sky_keys_on_city_time_solar_phase_and_class() {
        let f = fixture();
        let sky = f.sky_inputs();
        assert_eq!(sky.kind, SkyKind::Partly);
        assert_eq!(sky.phase, 1.0, "noon is full day");
        assert!((sky.cover - 0.4).abs() < 1e-9);
        assert_eq!(sky.precip, 0.0);
        // Dawn and dusk blend over ±45 minutes; the OS appearance plays no part.
        assert_eq!(solar_phase("2026-09-06T06:17", Some("2026-09-06T07:02"), Some("2026-09-06T19:35"), false), 0.0);
        assert!((solar_phase("2026-09-06T07:02", Some("2026-09-06T07:02"), Some("2026-09-06T19:35"), true) - 0.5).abs() < 1e-9);
        assert_eq!(solar_phase("2026-09-06T20:30", Some("2026-09-06T07:02"), Some("2026-09-06T19:35"), false), 0.0);
        assert!((solar_phase("2026-09-06T19:35", Some("2026-09-06T07:02"), Some("2026-09-06T19:35"), true) - 0.5).abs() < 1e-9);
        // No solar events (polar): is_day decides.
        assert_eq!(solar_phase("2026-09-06T12:00", None, None, false), 0.0);
        assert_eq!(sky_kind(999), SkyKind::Unknown);
        assert_eq!(glyph(2, false), Glyph::PartlyNight);
        assert_eq!(glyph(53, true), Glyph::Drizzle);
    }

    #[test]
    fn the_range_domain_is_shared_and_the_palette_blends() {
        let days = fixture().days();
        let domain = range_domain(&days, Some(18.4));
        assert_eq!(domain, (9.5, 24.5));
        assert_eq!(range_fraction(9.5, domain), 0.0);
        assert_eq!(range_fraction(24.5, domain), 1.0);
        assert!((range_fraction(17.0, domain) - 0.5).abs() < 1e-9);
        // Today's observation widens the domain when it lies outside it.
        assert_eq!(range_domain(&days, Some(30.0)).1, 30.0);
        assert_eq!(range_domain(&[], None), (0.0, 1.0));
        let c = range_color(35.0);
        assert!((c.x - 0xED as f32 / 255.0).abs() < 1e-4);
        let mid = range_color(5.0);
        assert!(mid.x > 0.0 && mid.x < 0.5, "between the 0 and 10 stops");
        let (uv, word) = uv_text(Some(6.4));
        assert_eq!((uv.as_str(), word), ("6", "High"));
        assert_eq!(uv_text(None).1, "Unavailable");
    }

    #[test]
    fn the_city_list_round_trips_through_the_catalogue() {
        let mut cities = default_cities();
        cities.push(search_cities("Paulo")[0]);
        cities.push(search_cities("Cape")[0]);
        let back = parse_cities(&serialize_cities(&cities));
        assert_eq!(back, cities);
        assert!(parse_cities(b"Atlantis\tNowhere\n").is_empty(), "a city the catalogue lacks is dropped");
    }

    #[test]
    fn the_search_finds_catalogue_cities_by_name_then_country() {
        let names = |q: &str| search_cities(q).iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("tok"), ["Tokyo"]);
        assert_eq!(names("york"), ["New York"], "any word of the name");
        assert_eq!(names("Austral"), ["Sydney", "Melbourne"], "a country finds its cities");
        assert!(names("  ").is_empty());
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn the_state_has_a_forecast_from_the_start_and_adds_without_duplicates() {
        let mut s = WeatherState::default();
        assert_eq!(s.forecast().days().len(), DAILY_ROWS, "no fetch, no wait");
        let tokyo = *s.city_by_name("Tokyo");
        assert_eq!(s.add_city(tokyo), Some(3), "already on the list: selected, not added");
        let paris = search_cities("Paris")[0];
        assert_eq!(s.add_city(paris), Some(4));
        assert!(s.select_city(4));
        assert_eq!(s.city().name, "Paris");
    }

    impl WeatherState {
        fn city_by_name(&self, name: &str) -> &City {
            self.cities.iter().find(|c| c.name == name).unwrap()
        }
    }
}
