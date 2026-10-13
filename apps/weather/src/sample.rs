//! The sample forecast: the app runs entirely offline, so every forecast
//! it shows is generated here, on the device, from a city's position and
//! a few climate numbers — never fetched, never pasted from a service.
//!
//! The generator is deterministic per city and per local date: the same
//! city on the same day always reads the same, and a given calendar day
//! keeps its weather as it moves from day 9 to "today" (each day's draws
//! are keyed on the absolute day number, not on the row it sits in).
//! What makes it believable:
//! - the season from the latitude and the city's annual mean and swing,
//!   with a multi-day warm/cool drift on top;
//! - a diurnal curve (coldest at sunrise, warmest mid-afternoon, cooling
//!   through the evening) that cloud and rain flatten;
//! - wet spells from slowly varying noise, scaled by how wet the city is,
//!   with the rain falling inside a window of hours, light at its edges;
//! - snow when it is cold enough, thunder when it is warm and heavy,
//!   morning fog on still, damp days;
//! - sunrise and sunset from the solar declination and the equation of
//!   time, at the city's wall clock (standard offset plus its daylight-
//!   saving rule), with polar days and nights having no events.

use crate::model::{City, Current, Daily, Dst, Forecast, Hourly};

/// Hourly rows generated from the current hour: the strip shows 24 plus
/// the sunrise/sunset cells, so two days leaves room for both.
const HOURS: usize = 48;
const DAYS: usize = 10;

/// The forecast for `city` as seen at the UTC instant `utc_now` (seconds).
pub fn sample_forecast(city: &City, utc_now: i64) -> Forecast {
    let offset = utc_offset_secs(city, utc_now);
    let local = utc_now + offset;
    let today = local.div_euclid(86_400);
    let now_min = local.rem_euclid(86_400) as f64 / 60.0;
    let seed = city_seed(city);

    // One day before today (the evening cooling into this morning) through
    // one day past the last row (the night after it).
    let mut days: Vec<DayGen> = (-1..=DAYS as i64).map(|d| DayGen::new(city, seed, today + d, offset)).collect();
    ensure_wet_spell(city, seed, offset, &mut days[1..=DAYS]);
    let at = |d: usize| &days[d];

    let mut hourly = Hourly::default();
    let first_hour = (now_min / 60.0).floor() as i64;
    for h in 0..HOURS as i64 {
        let abs_hour = first_hour + h;
        let d = 1 + abs_hour.div_euclid(24) as usize;
        let minute = abs_hour.rem_euclid(24) as f64 * 60.0;
        let hour = HourGen::new(&days, d, minute, seed, today * 24 + abs_hour);
        hourly.time.push(stamp(at(d).z, minute));
        hourly.temperature_2m.push(Some(round1(hour.temp)));
        hourly.weather_code.push(Some(hour.code as f64));
        hourly.is_day.push(Some(if hour.is_day { 1.0 } else { 0.0 }));
        hourly.precipitation_probability.push(Some(hour.chance));
        hourly.precipitation.push(Some(round1(hour.rain + hour.snow)));
        hourly.rain.push(Some(round1(hour.rain)));
        hourly.snowfall.push(Some(round1(hour.snow)));
        hourly.cloud_cover.push(Some((hour.cloud * 100.0).round()));
        hourly.uv_index.push(Some(round1(hour.uv)));
    }

    let mut daily = Daily::default();
    for day in &days[1..=DAYS] {
        daily.time.push(date(day.z));
        daily.temperature_2m_max.push(Some(round1(day.high)));
        daily.temperature_2m_min.push(Some(round1(day.low)));
        daily.weather_code.push(Some(day.code as f64));
        daily.sunrise.push(day.sun.map(|(rise, _)| stamp(day.z, rise)));
        daily.sunset.push(day.sun.map(|(_, set)| stamp(day.z, set)));
        daily.uv_index_max.push(Some(round1(day.uv_max)));
        daily.precipitation_probability_max.push(Some(day.chance));
        daily.precipitation_sum.push(Some(round1(day.sum)));
    }

    // The current observation: the current hour's conditions at the
    // current minute's temperature.
    let now = HourGen::new(&days, 1, now_min, seed, today * 24 + first_hour);
    let hours_abs = (utc_now as f64) / 3600.0;
    let c = city.climate;
    let wind = c.wind * (0.55 + 0.9 * smooth(seed ^ 0x77, hours_abs / 9.0)) + 10.0 * now.intensity;
    let gust = wind * (1.4 + 0.35 * unit(seed ^ 0x78, (hours_abs / 3.0) as i64 as u64));
    let direction = (200.0 + 260.0 * (smooth(seed ^ 0x79, hours_abs / 30.0) - 0.5)).rem_euclid(360.0);
    let humidity = (48.0 + 30.0 * now.cloud + 18.0 * now.intensity.min(1.0) + 20.0 * c.wet - 0.6 * (now.temp - 15.0).max(0.0)).clamp(22.0, 98.0);
    let apparent = if now.temp <= 12.0 {
        now.temp - (wind / 10.0) * (1.0 + (12.0 - now.temp) / 12.0)
    } else if now.temp >= 24.0 {
        now.temp + (humidity - 40.0).max(0.0) * 0.08
    } else {
        now.temp - wind / 25.0
    };
    let current = Current {
        time: Some(stamp(today, now_min.floor())),
        temperature_2m: Some(round1(now.temp)),
        relative_humidity_2m: Some(humidity.round()),
        apparent_temperature: Some(round1(apparent)),
        is_day: Some(if now.is_day { 1.0 } else { 0.0 }),
        precipitation: Some(round1(now.rain + now.snow)),
        weather_code: Some(now.code as f64),
        cloud_cover: Some((now.cloud * 100.0).round()),
        wind_speed_10m: Some(round1(wind)),
        wind_direction_10m: Some(direction.round()),
        wind_gusts_10m: Some(round1(gust)),
    };

    Forecast { utc_offset_seconds: Some(offset as f64), current, hourly, daily }
}

/// One generated day: its absolute day number, temperatures, weather,
/// the wet spell's hours and the solar events (minutes of the local day).
struct DayGen {
    z: i64,
    high: f64,
    low: f64,
    cloud: f64,
    /// 0 = dry; 0..1 how hard it rains (or snows) inside the window.
    intensity: f64,
    /// The wet spell's start and length in minutes of the local day.
    window: (f64, f64),
    fog: bool,
    code: u32,
    /// Sunrise and sunset; `None` on a polar day or night.
    sun: Option<(f64, f64)>,
    /// A polar day (the sun never sets) when `sun` is `None`.
    polar_day: bool,
    uv_max: f64,
    chance: f64,
    sum: f64,
}

impl DayGen {
    fn new(city: &City, seed: u64, z: i64, offset: i64) -> Self {
        let c = city.climate;
        let doy = day_of_year(z) as f64;
        // Coldest around 20 January in the north, 20 July in the south.
        let season = (2.0 * std::f64::consts::PI * (doy - 20.0) / 365.25).cos();
        let seasonal = if city.lat >= 0.0 { c.mean - c.swing * season } else { c.mean + c.swing * season };
        let drift = (smooth(seed ^ 0x11, z as f64 / 3.0) - 0.5) * 7.0 + (unit(seed ^ 0x12, z as u64) - 0.5) * 2.0;

        let wet_score = 0.75 * smooth(seed ^ 0x21, z as f64 / 2.2) + 0.25 * unit(seed ^ 0x22, z as u64);
        let threshold = 1.0 - c.wet * 0.55;
        let intensity = if wet_score > threshold { ((wet_score - threshold) / (1.0 - threshold)).clamp(0.15, 1.0) } else { 0.0 };
        let mut cloud = (0.1 + 0.9 * smooth(seed ^ 0x31, z as f64 / 1.7) * (0.45 + c.wet)).clamp(0.0, 1.0);
        if intensity > 0.0 {
            cloud = cloud.max(0.8 + 0.2 * intensity);
        }

        let range = (4.0 + 8.0 * (1.0 - cloud)) * (1.0 - 0.35 * intensity) * (0.6 + 0.4 * (c.swing / 10.0).min(1.2));
        let mid = seasonal + drift - 1.5 * intensity;
        let high = mid + range / 2.0;
        let low = mid - range / 2.0;

        let start = 6.0 * 60.0 + unit(seed ^ 0x41, z as u64) * 14.0 * 60.0;
        let len = (2.0 + 10.0 * intensity) * 60.0;
        let fog = intensity == 0.0 && c.wet >= 0.4 && (-2.0..12.0).contains(&low) && unit(seed ^ 0x51, z as u64) < 0.14;

        let mut day = DayGen {
            z,
            high,
            low,
            cloud,
            intensity,
            window: (start, len),
            fog,
            code: 0,
            sun: None,
            polar_day: false,
            uv_max: 0.0,
            chance: 0.0,
            sum: 0.0,
        };
        day.settle(city, offset);
        day
    }

    /// Everything that follows from the drawn values: the day's code, its
    /// sun, UV, rain chance and total. Re-run when a wet spell is forced.
    fn settle(&mut self, city: &City, offset: i64) {
        let doy = day_of_year(self.z) as f64;
        self.code = if self.intensity > 0.0 {
            wet_code(self.intensity, self.high, unit(self.z as u64, 0x61) < 0.5)
        } else if self.fog {
            45
        } else {
            dry_code(self.cloud)
        };
        let (sun, polar_day, noon_elevation) = sun_times(city.lat, city.lon, offset, doy);
        self.sun = sun;
        self.polar_day = polar_day;
        let clear_uv = 11.0 * noon_elevation.to_radians().sin().max(0.0).powf(2.8);
        self.uv_max = clear_uv * (1.0 - 0.55 * self.cloud) * (1.0 - 0.3 * self.intensity);
        self.chance = if self.intensity > 0.0 {
            round5(55.0 + 40.0 * self.intensity)
        } else {
            round5(self.cloud * 25.0)
        };
        self.sum = if self.intensity > 0.0 {
            self.intensity * (1.5 + 14.0 * self.intensity) * (self.window.1 / 360.0)
        } else {
            0.0
        };
    }

    fn rise(&self) -> f64 {
        self.sun.map(|(rise, _)| rise).unwrap_or(6.0 * 60.0)
    }
}

/// The ten rows always carry a wet spell somewhere, unless the city is
/// properly dry: when the draws gave none, the wettest-scoring day from
/// the third row on becomes one.
fn ensure_wet_spell(city: &City, seed: u64, offset: i64, days: &mut [DayGen]) {
    if city.climate.wet < 0.15 || days.iter().any(|d| d.intensity > 0.0) {
        return;
    }
    let pick = (2..days.len()).max_by(|&a, &b| {
        let score = |i: usize| unit(seed ^ 0x23, days[i].z as u64);
        score(a).total_cmp(&score(b))
    });
    if let Some(i) = pick {
        let day = &mut days[i];
        day.intensity = 0.45;
        day.cloud = day.cloud.max(0.9);
        day.high -= 1.5;
        day.fog = false;
        day.settle(city, offset);
    }
}

/// One generated hour (or the current minute) of a day.
struct HourGen {
    temp: f64,
    code: u32,
    is_day: bool,
    cloud: f64,
    intensity: f64,
    chance: f64,
    rain: f64,
    snow: f64,
    uv: f64,
}

impl HourGen {
    /// `days[d]` is the day the minute falls in; `days[d - 1]` and
    /// `days[d + 1]` must exist. `key` varies the hour-to-hour draws.
    fn new(days: &[DayGen], d: usize, minute: f64, seed: u64, key: i64) -> Self {
        let day = &days[d];
        let temp = temp_at(&days[d - 1], day, &days[d + 1], minute);
        let is_day = match day.sun {
            Some((rise, set)) => minute >= rise && minute < set,
            None => day.polar_day,
        };

        let (start, len) = day.window;
        // The wet spell may run past midnight: the previous day's tail
        // counts too.
        let prev = &days[d - 1];
        let in_today = day.intensity > 0.0 && minute >= start && minute < start + len;
        let in_prev = prev.intensity > 0.0 && minute + 1440.0 < prev.window.0 + prev.window.1;
        let (wet, edge) = if in_today {
            (day.intensity, (minute - start).min(start + len - minute) / 60.0)
        } else if in_prev {
            (prev.intensity, (prev.window.0 + prev.window.1 - minute - 1440.0) / 60.0)
        } else {
            (0.0, 0.0)
        };
        // Light at the spell's edges, heaviest in its middle.
        let intensity = if wet > 0.0 { wet * (0.45 + 0.55 * (edge / 2.0).min(1.0)) } else { 0.0 };
        let near_spell = day.intensity > 0.0 && minute >= start - 180.0 && minute < start + len + 120.0;

        let jitter = (unit(seed ^ 0x81, key as u64) - 0.5) * 0.3;
        let cloud = if intensity > 0.0 {
            (0.85 + 0.15 * intensity).min(1.0)
        } else if near_spell {
            (day.cloud + 0.15 + jitter).clamp(0.6, 1.0)
        } else {
            (day.cloud * 0.8 + jitter).clamp(0.0, 1.0)
        };
        let fog = day.fog && (4.0 * 60.0..10.0 * 60.0).contains(&minute);
        let code = if intensity > 0.0 {
            wet_code(intensity, temp, unit(day.z as u64, 0x61) < 0.5)
        } else if fog {
            45
        } else {
            dry_code(cloud)
        };
        let chance = if intensity > 0.0 {
            round5(60.0 + 35.0 * intensity)
        } else if near_spell {
            round5(25.0 + 25.0 * day.intensity)
        } else {
            round5(cloud * 15.0)
        };
        let mm = if intensity > 0.0 { intensity * (0.3 + 3.5 * intensity) } else { 0.0 };
        let (rain, snow) = if matches!(code, 71..=77 | 85 | 86) { (0.0, mm * 0.8) } else { (mm, 0.0) };
        let uv = match day.sun {
            Some((rise, set)) if is_day => day.uv_max * ((minute - rise) / (set - rise) * std::f64::consts::PI).sin().max(0.0).powf(1.5),
            None if is_day => day.uv_max * 0.6,
            _ => 0.0,
        };
        HourGen { temp, code, is_day, cloud, intensity, chance, rain, snow, uv }
    }
}

/// The diurnal curve: the low at sunrise, the high at 15:00, cooling from
/// the previous afternoon into this sunrise and from this afternoon toward
/// the next sunrise.
fn temp_at(prev: &DayGen, day: &DayGen, next: &DayGen, minute: f64) -> f64 {
    const PEAK: f64 = 15.0 * 60.0;
    let ease = |f: f64| (1.0 - (std::f64::consts::PI * f.clamp(0.0, 1.0)).cos()) / 2.0;
    let rise = day.rise().min(PEAK - 60.0);
    if minute < rise {
        let from = PEAK - 1440.0;
        prev.high + (day.low - prev.high) * ease((minute - from) / (rise - from))
    } else if minute < PEAK {
        day.low + (day.high - day.low) * ease((minute - rise) / (PEAK - rise))
    } else {
        let to = next.rise().min(PEAK - 60.0) + 1440.0;
        day.high + (next.low - day.high) * ease((minute - PEAK) / (to - PEAK))
    }
}

/// A wet code: snow when cold, thunder when warm and heavy, showers or
/// steady rain otherwise, drizzle at the lightest.
fn wet_code(intensity: f64, temp: f64, showery: bool) -> u32 {
    if temp <= 1.5 {
        return if intensity < 0.35 { 71 } else if intensity < 0.7 { 73 } else { 75 };
    }
    if temp >= 23.0 && intensity > 0.55 {
        return 95;
    }
    match intensity {
        i if i > 0.75 => if showery { 82 } else { 65 },
        i if i > 0.45 => if showery { 81 } else { 63 },
        i if i > 0.25 => if showery { 80 } else { 61 },
        _ => 53,
    }
}

fn dry_code(cloud: f64) -> u32 {
    match cloud {
        c if c < 0.2 => 0,
        c if c < 0.4 => 1,
        c if c < 0.72 => 2,
        _ => 3,
    }
}

/// Sunrise and sunset in minutes of the local day (`offset` seconds from
/// UTC), whether a missing pair is a polar day, and the sun's elevation at
/// noon in degrees.
fn sun_times(lat: f64, lon: f64, offset: i64, doy: f64) -> (Option<(f64, f64)>, bool, f64) {
    use std::f64::consts::PI;
    let declination = 23.44 * (2.0 * PI * (284.0 + doy) / 365.0).sin();
    let noon_elevation = 90.0 - (lat - declination).abs();
    let (phi, delta) = (lat.to_radians(), declination.to_radians());
    let cos_w = ((-0.833f64).to_radians().sin() - phi.sin() * delta.sin()) / (phi.cos() * delta.cos());
    if cos_w <= -1.0 {
        return (None, true, noon_elevation);
    }
    if cos_w >= 1.0 {
        return (None, false, noon_elevation);
    }
    let half = cos_w.acos().to_degrees() * 4.0;
    let b = 2.0 * PI * (doy - 81.0) / 364.0;
    let equation_of_time = 9.87 * (2.0 * b).sin() - 7.53 * b.cos() - 1.5 * b.sin();
    let noon = 720.0 - 4.0 * lon - equation_of_time + offset as f64 / 60.0;
    (Some(((noon - half).max(0.0), (noon + half).min(1439.0))), false, noon_elevation)
}

/// The city's wall-clock offset from UTC at `utc` (seconds): its standard
/// offset plus an hour while its daylight-saving rule is in force.
pub fn utc_offset_secs(city: &City, utc: i64) -> i64 {
    let standard = city.utc_offset_min as i64 * 60;
    let local = utc + standard;
    let (year, _, _) = civil_from_days(local.div_euclid(86_400));
    let at = |z: i64, hour: i64| z * 86_400 + hour * 3600;
    let summer = match city.dst {
        Dst::None => false,
        // 01:00 UTC on the last Sundays of March and October.
        Dst::Eu => (at(last_sunday(year, 3), 1)..at(last_sunday(year, 10), 1)).contains(&utc),
        // 02:00 local, the second Sunday of March to the first of November.
        Dst::Us => (at(nth_sunday(year, 3, 2), 2)..at(nth_sunday(year, 11, 1), 1)).contains(&local),
        // 02:00 local, the first Sunday of October to the first of April.
        Dst::South => local >= at(nth_sunday(year, 10, 1), 2) || local < at(nth_sunday(year, 4, 1), 2),
    };
    standard + if summer { 3600 } else { 0 }
}

fn last_sunday(year: i64, month: i64) -> i64 {
    let next = if month == 12 { days_from_civil(year + 1, 1, 1) } else { days_from_civil(year, month + 1, 1) };
    let last = next - 1;
    last - weekday(last)
}

fn nth_sunday(year: i64, month: i64, n: i64) -> i64 {
    let first = days_from_civil(year, month, 1);
    first + (7 - weekday(first)) % 7 + 7 * (n - 1)
}

/// 0 = Sunday.
fn weekday(z: i64) -> i64 {
    (z + 4).rem_euclid(7)
}

fn day_of_year(z: i64) -> i64 {
    let (y, _, _) = civil_from_days(z);
    z - days_from_civil(y, 1, 1) + 1
}

/// Howard Hinnant's days_from_civil / civil_from_days.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// "2026-10-12".
fn date(z: i64) -> String {
    let (y, m, d) = civil_from_days(z);
    format!("{y:04}-{m:02}-{d:02}")
}

/// "2026-10-12T14:05", the local wall time the model's stamps use.
fn stamp(z: i64, minute: f64) -> String {
    let m = minute.round() as i64;
    format!("{}T{:02}:{:02}", date(z), m / 60, m % 60)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round5(v: f64) -> f64 {
    ((v / 5.0).round() * 5.0).clamp(0.0, 100.0)
}

/// FNV-1a over the city's name: each city its own weather.
fn city_seed(city: &City) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in city.name.bytes().chain(city.country.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A uniform draw in [0, 1) for (`seed`, `key`).
fn unit(seed: u64, key: u64) -> f64 {
    (splitmix(seed ^ splitmix(key)) >> 11) as f64 / (1u64 << 53) as f64
}

/// Value noise in [0, 1): uniform draws on the integers, eased between.
fn smooth(seed: u64, x: f64) -> f64 {
    let i = x.floor();
    let f = x - i;
    let f = f * f * (3.0 - 2.0 * f);
    let a = unit(seed, i as i64 as u64);
    let b = unit(seed, (i as i64 + 1) as u64);
    a + (b - a) * f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{city_catalogue, default_cities, sky_kind, HourCell, SkyKind, DAILY_ROWS, HOURLY_CELLS};

    /// 2026-10-12 10:30 UTC.
    const NOON_ISH: i64 = 1_791_801_000;

    #[test]
    fn the_calendar_helpers_agree() {
        assert_eq!(date(days_from_civil(2026, 10, 12)), "2026-10-12");
        assert_eq!(weekday(days_from_civil(2026, 10, 11)), 0, "2026-10-11 is a Sunday");
        assert_eq!(date(last_sunday(2026, 3)), "2026-03-29");
        assert_eq!(date(last_sunday(2026, 10)), "2026-10-25");
        assert_eq!(date(nth_sunday(2026, 3, 2)), "2026-03-08");
        assert_eq!(date(nth_sunday(2026, 11, 1)), "2026-11-01");
        assert_eq!(day_of_year(days_from_civil(2026, 12, 31)), 365);
    }

    #[test]
    fn wall_clocks_follow_daylight_saving() {
        let cities = city_catalogue();
        let find = |name: &str| cities.iter().find(|c| c.name == name).copied().unwrap();
        let summer = 1_783_000_000; // early July 2026
        let winter = 1_768_000_000; // mid January 2026
        assert_eq!(utc_offset_secs(&find("Amsterdam"), summer), 7200);
        assert_eq!(utc_offset_secs(&find("Amsterdam"), winter), 3600);
        assert_eq!(utc_offset_secs(&find("New York"), summer), -4 * 3600);
        assert_eq!(utc_offset_secs(&find("New York"), winter), -5 * 3600);
        assert_eq!(utc_offset_secs(&find("Sydney"), summer), 10 * 3600);
        assert_eq!(utc_offset_secs(&find("Sydney"), winter), 11 * 3600);
        assert_eq!(utc_offset_secs(&find("Tokyo"), summer), 9 * 3600);
    }

    #[test]
    fn every_city_gets_a_full_plausible_forecast() {
        for city in city_catalogue() {
            let f = sample_forecast(city, NOON_ISH);
            assert_eq!(f.days().len(), DAILY_ROWS, "{}", city.name);
            assert!(f.hour_cells().len() >= HOURLY_CELLS, "{}", city.name);
            assert_eq!(f.now_index(), 0, "the strip starts at the current hour");
            for day in f.days() {
                let (hi, lo) = (day.high.unwrap(), day.low.unwrap());
                assert!(hi > lo && hi - lo < 16.0, "{} {}: {lo}..{hi}", city.name, day.date);
                assert!((-35.0..48.0).contains(&lo) && (-30.0..50.0).contains(&hi), "{} {}: {lo}..{hi}", city.name, day.date);
                assert!(sky_kind(day.code.unwrap()) != SkyKind::Unknown, "{} {}", city.name, day.date);
                if let (Some(rise), Some(set)) = (&day.sunrise, &day.sunset) {
                    assert!(rise < set, "{} {}: {rise} {set}", city.name, day.date);
                }
            }
            let t = f.current.temperature_2m.unwrap();
            let (hi, lo) = f.today_high_low();
            assert!(t <= hi.unwrap() + 8.0 && t >= lo.unwrap() - 8.0, "{}: now {t} vs {lo:?}..{hi:?}", city.name);
        }
    }

    #[test]
    fn the_same_city_and_date_read_the_same_and_cities_differ() {
        let cities = default_cities();
        let a = sample_forecast(&cities[0], NOON_ISH);
        let b = sample_forecast(&cities[0], NOON_ISH + 600);
        assert_eq!(a.days(), b.days(), "ten minutes later, the same days");
        let highs = |c: &City| sample_forecast(c, NOON_ISH).days().iter().map(|d| d.high.unwrap()).collect::<Vec<_>>();
        for pair in cities.windows(2) {
            assert_ne!(highs(&pair[0]), highs(&pair[1]), "{} vs {}", pair[0].name, pair[1].name);
        }
        // A calendar day keeps its weather as it comes closer.
        let tomorrow = sample_forecast(&cities[0], NOON_ISH + 86_400);
        assert_eq!(a.days()[3].high, tomorrow.days()[2].high);
    }

    #[test]
    fn wet_cities_get_a_rain_spell_and_the_curve_peaks_in_the_afternoon() {
        for city in default_cities() {
            let f = sample_forecast(&city, NOON_ISH);
            assert!(
                f.days().iter().any(|d| sky_kind(d.code.unwrap()) == SkyKind::Rain || sky_kind(d.code.unwrap()) == SkyKind::Snow || sky_kind(d.code.unwrap()) == SkyKind::Thunder),
                "{} has no wet day",
                city.name
            );
            let cells = f.hour_cells();
            assert!(cells.iter().any(|c| matches!(c, HourCell::Sunrise { .. } | HourCell::Sunset { .. })), "{}: no solar events", city.name);
            assert!(cells.iter().any(|c| matches!(c, HourCell::Hour { is_day: false, .. })), "{}: no night", city.name);
        }
        // The curve peaks at 15:00 on the day's high.
        let f = sample_forecast(&default_cities()[3], NOON_ISH);
        let tomorrow = &f.days()[1];
        let i = f.hourly.time.iter().position(|t| *t == format!("{}T15:00", tomorrow.date)).unwrap();
        assert_eq!(f.hourly.temperature_2m[i], tomorrow.high);
    }
}
