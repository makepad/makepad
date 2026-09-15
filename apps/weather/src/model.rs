//! The weather model: the selectable cities, the Open-Meteo forecast shape
//! (current conditions, 24 hourly steps, ten daily rows, solar events),
//! WMO weather-code wording, the sky's inputs (condition class, solar phase,
//! cloud cover, precipitation intensity), the ten-day range domain and its
//! temperature palette, and the fetch state machine (loading, failed,
//! stale-but-kept) that both faces render from.
//!
//! Timestamps are Open-Meteo's ISO strings in the CITY'S zone (`timezone=
//! auto`): every stamp is already the local wall time at that instant, DST
//! included, so the labels read straight off the string and no offset is
//! ever applied twice. Arrays are matched by index against `time` and
//! tolerate nulls and unequal lengths.
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
/// After this the tile shows when the data was last good instead of H/L.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
/// A ten-day forecast with 240 hourly rows is ~60 KB; anything past this
/// is not one.
pub const MAX_BODY_BYTES: usize = 512 * 1024;
pub const ATTRIBUTION: &str = "Weather data by Open-Meteo.com (CC BY 4.0)";
/// How many hourly cells the strip shows, "Now" first.
pub const HOURLY_CELLS: usize = 24;
pub const DAILY_ROWS: usize = 10;

pub fn forecast_url(city: &City) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.2}&longitude={:.2}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,precipitation,\
         weather_code,cloud_cover,wind_speed_10m,wind_direction_10m,wind_gusts_10m\
         &hourly=temperature_2m,weather_code,is_day,precipitation_probability,precipitation,\
         rain,snowfall,cloud_cover,uv_index\
         &daily=temperature_2m_max,temperature_2m_min,weather_code,sunrise,sunset,uv_index_max,\
         precipitation_probability_max,precipitation_sum\
         &timezone=auto&forecast_days=10",
        city.lat, city.lon
    )
}

#[derive(Clone, Debug, Default, DeJson)]
pub struct Forecast {
    pub timezone: Option<String>,
    pub utc_offset_seconds: Option<f64>,
    pub current: Current,
    pub hourly: Hourly,
    pub daily: Daily,
}

#[derive(Clone, Debug, Default, DeJson)]
pub struct Current {
    pub time: Option<String>,
    pub interval: Option<f64>,
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

#[derive(Clone, Debug, Default, DeJson)]
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

#[derive(Clone, Debug, Default, DeJson)]
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
    pub fn parse(json: &str) -> Result<Self, String> {
        // Lenient: Open-Meteo sends fields we do not model (units, elevation,
        // generation time); strict decoding would reject the whole body.
        let f: Forecast = DeJson::deserialize_json_lenient(json).map_err(|e| format!("{e:?}"))?;
        if f.current.temperature_2m.is_none() || f.current.weather_code.is_none() {
            return Err("response has no current weather".into());
        }
        Ok(f)
    }

    /// The ten daily rows, in order; fewer when the service sent fewer.
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

/// WMO weather interpretation codes as Open-Meteo documents them.
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

pub fn temp(v: Option<f64>) -> String {
    match v {
        Some(t) => format!("{}°", t.round() as i64),
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

    /// The tile swaps its H/L line for this after a failed refresh or
    /// thirty minutes of age: when the data was last good.
    pub fn tile_freshness(&self) -> Option<String> {
        let (forecast, at) = self.current()?;
        if at.elapsed() < STALE_AFTER && self.last_error.is_none() {
            return None;
        }
        Some(format!("Updated {}", forecast.current.time.as_deref().map(hhmm_of).unwrap_or_else(|| "earlier".into())))
    }

    /// One line of status for either face.
    pub fn status_text(&self) -> String {
        let age = |at: Instant| {
            let mins = at.elapsed().as_secs() / 60;
            if mins == 0 { "just now".to_string() } else { format!("{} min ago", mins) }
        };
        match (self.loading(), self.current(), &self.last_error) {
            (true, None, _) => "Loading".into(),
            (true, Some((_, at)), _) => format!("Refreshing · updated {}", age(at)),
            (false, None, Some(_)) => "Unavailable".into(),
            (false, None, None) => "No data yet".into(),
            (false, Some((_, at)), Some(_)) => format!("Offline · showing data from {}", age(at)),
            (false, Some((_, at)), None) if self.is_stale() => format!("Stale · updated {}", age(at)),
            (false, Some((_, at)), None) => format!("Updated {}", age(at)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"latitude":52.38,"longitude":4.9,"timezone":"Europe/Amsterdam","utc_offset_seconds":7200,
        "current":{"time":"2026-09-06T12:10","interval":900,"temperature_2m":18.4,"relative_humidity_2m":61,
        "apparent_temperature":17.1,"is_day":1,"precipitation":0.0,"weather_code":2,"cloud_cover":40,
        "wind_speed_10m":14.7,"wind_direction_10m":247,"wind_gusts_10m":31.0},
        "hourly":{"time":["2026-09-06T11:00","2026-09-06T12:00","2026-09-06T13:00","2026-09-06T14:00","2026-09-06T19:00","2026-09-06T20:00"],
        "temperature_2m":[17.0,18.0,19.2,null,15.0,14.0],"weather_code":[2,2,3,61,0,0],"is_day":[1,1,1,1,1,0],
        "precipitation_probability":[0,5,20,60,0,0],"precipitation":[0,0,0.2,1.4,0,0],"rain":[0,0,0.2,1.4,0,0],
        "snowfall":[0,0,0,0,0,0],"cloud_cover":[30,40,80,100,10,5],"uv_index":[3.1,4.4,4.0,1.2,0.3,0]},
        "daily":{"time":["2026-09-06","2026-09-07","2026-09-08","2026-09-09","2026-09-10","2026-09-11","2026-09-12","2026-09-13","2026-09-14","2026-09-15"],
        "temperature_2m_max":[20.1,21.3,null,19.0,18.2,22.0,24.5,23.1,20.0,18.0],
        "temperature_2m_min":[12.0,11.5,10.9,12.2,11.0,12.5,13.0,14.0,11.0,9.5],
        "weather_code":[2,3,61,80,0,1,0,2,3,61],
        "sunrise":["2026-09-06T07:02","2026-09-07T07:04",null,"2026-09-09T07:07","2026-09-10T07:09","2026-09-11T07:10","2026-09-12T07:12","2026-09-13T07:14","2026-09-14T07:15","2026-09-15T07:17"],
        "sunset":["2026-09-06T19:35","2026-09-07T19:33",null,"2026-09-09T19:28","2026-09-10T19:26","2026-09-11T19:24","2026-09-12T19:21","2026-09-13T19:19","2026-09-14T19:17","2026-09-15T19:14"],
        "uv_index_max":[4.4,5.0,2.1,3.0,4.0,5.5,6.0,5.8,3.2,2.0],
        "precipitation_probability_max":[5,20,80,60,0,0,0,10,30,70],
        "precipitation_sum":[0.0,0.4,6.2,2.1,0,0,0,0.1,1.0,4.4]}}"#;

    #[test]
    fn parses_ten_days_and_hourly_and_tolerates_nulls() {
        let f = Forecast::parse(SAMPLE).unwrap();
        assert_eq!(f.code(), 2);
        assert!(f.is_day());
        let days = f.days();
        assert_eq!(days.len(), 10);
        assert_eq!(days[0].weekday, "Sun");
        assert_eq!(days[2].high, None);
        assert_eq!(days[2].sunrise, None);
        assert_eq!(days[1].sunset.as_deref(), Some("2026-09-07T19:33"));
        assert_eq!(temp(days[1].high), "21°");
        assert_eq!(days[0].uv_max, Some(4.4));
        assert_eq!(days[0].precip_sum, Some(0.0));
        assert_eq!(f.current.apparent_temperature, Some(17.1));
        assert_eq!(f.current.wind_direction_10m, Some(247.0));
        assert_eq!(compass(247.0), "WSW");
    }

    #[test]
    fn the_strip_starts_now_and_inserts_solar_events() {
        let f = Forecast::parse(SAMPLE).unwrap();
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
        let f = Forecast::parse(SAMPLE).unwrap();
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
        let f = Forecast::parse(SAMPLE).unwrap();
        let days = f.days();
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
        assert_eq!(s.tile_freshness(), None, "fresh data keeps the H/L line");
        let (second, _) = s.begin_fetch();
        assert!(s.select_city(3));
        assert!(!s.complete(second, Err("late".into())), "old city's reply is dropped");
        assert!(s.current().is_none(), "Amsterdam data is not shown as Tokyo");
        let (third, _) = s.begin_fetch();
        assert!(s.complete(third, Err("offline".into())));
        assert_eq!(s.status_text(), "Unavailable");
        s.select_city(0);
        assert!(s.current().is_some(), "last good Amsterdam data is still there");
        assert!(s.status_text().starts_with("Offline"));
        assert_eq!(s.tile_freshness().as_deref(), Some("Updated 12:10"), "a failed refresh shows when the data was good");
    }

    #[test]
    fn url_is_the_documented_query() {
        let url = forecast_url(&CITIES[0]);
        assert!(url.starts_with("https://api.open-meteo.com/v1/forecast?latitude=52.37&longitude=4.90&current="));
        assert!(url.contains("&hourly=temperature_2m,weather_code,is_day,precipitation_probability"));
        assert!(url.contains("&daily=temperature_2m_max,temperature_2m_min,weather_code,sunrise,sunset,uv_index_max"));
        assert!(url.ends_with("&timezone=auto&forecast_days=10"));
    }
}
