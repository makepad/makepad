//! Lane E: the sun study.
//!
//! `api::SkyState` owns the NOAA solar model; this module owns the study UI:
//! date/time/site controls, the day scrub, the sun-path arc the overlay draws, and the
//! readout. Lane B takes `SunSettings::direction()` for the key light and the
//! shadow maps, lane F for the sky — so a scrub here moves the shadows there.

use crate::api::*;
use makepad_widgets::*;

pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Hours the day scrub runs between (civil daylight either side of the
/// solstices at temperate latitudes) and how fast it plays.
pub const PLAY_FROM: f32 = 4.0;
pub const PLAY_TO: f32 = 22.0;
pub const PLAY_HOURS_PER_SECOND: f32 = 3.0;

/// Compass bearing of the sun, degrees clockwise from project north (+Y).
pub fn azimuth_deg(sun: &SunSettings) -> f32 {
    sun.azimuth_deg()
}

pub fn compass_point(deg: f32) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let i = (((deg / 45.0).round() as i32) % 8 + 8) % 8;
    POINTS[i as usize]
}

pub fn clock(hour: f32) -> String {
    let h = hour.rem_euclid(24.0);
    let hh = h.floor() as i32;
    let mm = ((h - hh as f32) * 60.0).round() as i32;
    let (hh, mm) = if mm == 60 { (hh + 1, 0) } else { (hh, mm) };
    format!("{:02}:{:02}", hh % 24, mm)
}

pub fn date(sun: &SunSettings) -> String {
    let month = sun.date.month.clamp(1, 12) as usize - 1;
    format!(
        "{} {} {}",
        sun.date
            .day
            .clamp(1, days_in_month(sun.date.year, sun.date.month)),
        MONTHS[month],
        sun.date.year
    )
}

/// The one-line readout under the view label.
pub fn describe(sun: &SunSettings) -> String {
    let elev = sun.elevation_deg();
    if elev <= 0.0 {
        format!(
            "{} {} · alt {:.1}° · az {:.1}° · night",
            date(sun),
            clock(sun.time_local),
            elev,
            sun.azimuth_deg()
        )
    } else {
        let az = azimuth_deg(sun);
        format!(
            "{} {} · alt {:.0}° · az {:.0}° {}",
            date(sun),
            clock(sun.time_local),
            elev,
            az,
            compass_point(az)
        )
    }
}

/// One step of the day scrub. Wraps back to dawn at dusk.
pub fn advance(sun: &SunSettings, dt_seconds: f32, hours_per_second: f32) -> SunSettings {
    let mut out = *sun;
    let mut h = sun.time_local + dt_seconds * hours_per_second;
    if h >= PLAY_TO {
        h = PLAY_FROM + (h - PLAY_TO);
        if h >= PLAY_TO {
            h = PLAY_FROM;
        }
    }
    out.time_local = h.clamp(0.0, 24.0);
    out
}

/// The sun's track across this day, sampled every `step` hours; only the
/// above-horizon part, which is what the compass rose draws.
pub fn day_path(sun: &SunSettings, step: f32) -> Vec<(f32, Vec3f)> {
    let mut out = Vec::new();
    let step = step.max(0.05);
    let mut h = 0.0f32;
    while h <= 24.0 {
        let s = SunSettings {
            time_local: h,
            ..*sun
        };
        let d = s.direction();
        if d.z > 0.0 {
            out.push((h, d));
        }
        h += step;
    }
    out
}

/// Sunrise / sunset for this day and latitude, by bisection on elevation.
/// `None` for polar day or polar night.
pub fn sun_times(sun: &SunSettings) -> Option<(f32, f32)> {
    let up = |h: f32| {
        SunSettings {
            time_local: h,
            ..*sun
        }
        .elevation_deg()
            > 0.0
    };
    let mut rise = None;
    let mut set = None;
    let mut prev = up(0.0);
    let mut h = 0.05f32;
    while h <= 24.0 {
        let now = up(h);
        if now != prev {
            let (mut lo, mut hi) = (h - 0.05, h);
            for _ in 0..24 {
                let mid = (lo + hi) * 0.5;
                if up(mid) == prev {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let t = (lo + hi) * 0.5;
            if now {
                rise = Some(t);
            } else {
                set = Some(t);
            }
            prev = now;
        }
        h += 0.05;
    }
    match (rise, set) {
        (Some(r), Some(s)) => Some((r, s)),
        _ => None,
    }
}

/// Where a shadow of a point falls on the ground plane, or `None` when the
/// sun is down. Used by the overlay to show the sun direction honestly.
pub fn ground_shadow(sun: &SunSettings, p: Vec3f) -> Option<Vec3f> {
    let d = sun.direction();
    if d.z <= 0.05 || p.z <= 0.0 {
        return None;
    }
    let t = p.z / d.z;
    Some(vec3(p.x - d.x * t, p.y - d.y * t, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amsterdam_midsummer_noon_is_south_and_high() {
        let s = SunSettings {
            date: SkyDate {
                year: 2024,
                month: 6,
                day: 21,
            },
            time_local: 12.0,
            latitude: 52.37,
            longitude: 0.0,
            tz_offset: 0.0,
            north_deg: 0.0,
            ..Default::default()
        };
        let az = azimuth_deg(&s);
        assert!(compass_point(az) == "S", "azimuth {az} at solar noon");
        assert!(s.elevation_deg() > 55.0, "{}", s.elevation_deg());
        let (rise, set) = sun_times(&s).expect("the sun rises in June in Amsterdam");
        // Solar time, so symmetric about noon: ~04:15 to ~19:45.
        assert!(rise > 3.0 && rise < 6.0, "sunrise {rise}");
        assert!(set > 18.0 && set < 21.0, "sunset {set}");
        assert!(((rise + set) * 0.5 - 12.0).abs() < 0.2, "noon not centred");
    }

    #[test]
    fn midwinter_is_lower_than_midsummer() {
        let summer = SunSettings {
            date: SkyDate {
                year: 2024,
                month: 6,
                day: 21,
            },
            time_local: 12.0,
            longitude: 0.0,
            tz_offset: 0.0,
            north_deg: 0.0,
            ..Default::default()
        };
        let winter = SunSettings {
            date: SkyDate {
                year: 2024,
                month: 12,
                day: 21,
            },
            ..summer
        };
        assert!(winter.elevation_deg() < summer.elevation_deg() - 30.0);
        assert!(winter.elevation_deg() > 0.0, "still daylight at noon");
    }

    /// The play clock is wall-time by construction: hours advanced are
    /// exactly `dt * speed`, so a stalled frame followed by a long one
    /// still lands the sun where the elapsed time says.
    #[test]
    fn the_day_scrub_advances_by_elapsed_time() {
        let s = SunSettings {
            time_local: 10.0,
            ..Default::default()
        };
        let next = advance(&s, 2.0, PLAY_HOURS_PER_SECOND);
        assert!((next.time_local - 16.0).abs() < 1.0e-4, "{}", next.time_local);
        // Zero elapsed time moves nothing, half the dt moves half as far.
        assert_eq!(advance(&s, 0.0, PLAY_HOURS_PER_SECOND).time_local, 10.0);
        let half = advance(&s, 1.0, PLAY_HOURS_PER_SECOND);
        assert!((half.time_local - 13.0).abs() < 1.0e-4);
    }

    #[test]
    fn the_day_scrub_wraps() {
        let s = SunSettings {
            time_local: PLAY_TO - 0.1,
            ..Default::default()
        };
        let next = advance(&s, 1.0, PLAY_HOURS_PER_SECOND);
        assert!(
            next.time_local >= PLAY_FROM && next.time_local < PLAY_TO,
            "{}",
            next.time_local
        );
    }

    #[test]
    fn a_shadow_points_away_from_the_sun() {
        let s = SunSettings {
            time_local: 9.0,
            ..Default::default()
        };
        let p = vec3(0.0, 0.0, 3.0);
        let g = ground_shadow(&s, p).unwrap();
        assert!(g.z.abs() < 1e-5);
        let d = s.direction();
        assert!((g - p).dot(d) < 0.0, "shadow fell toward the sun");
    }
}
