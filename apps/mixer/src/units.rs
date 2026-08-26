//! Wire-float <-> engineering-unit maps.
//!
//! Every continuous parameter travels as float 0.0..=1.0. The maps here are
//! the documented client-side conversions: the four-segment level taper
//! (unity 0 dB at 0.75), linear maps (gain, EQ gain, thresholds, pan) and
//! log maps (frequency, Q, hold/release). Meters arrive as signed int16 at
//! 1/256 dB.

/// Four-segment fader taper, wire float -> dB. 0.0 is treated as -inf
/// (returned as f32::NEG_INFINITY).
pub fn level_to_db(f: f32) -> f32 {
    if f <= 0.0 {
        f32::NEG_INFINITY
    } else if f >= 0.5 {
        f * 40.0 - 30.0
    } else if f >= 0.25 {
        f * 80.0 - 50.0
    } else if f >= 0.0625 {
        f * 160.0 - 70.0
    } else {
        f * 480.0 - 90.0
    }
}

/// dB -> wire float for the level taper. Anything below -90 dB maps to 0.
pub fn db_to_level(d: f32) -> f32 {
    if d >= 10.0 {
        1.0
    } else if d >= -10.0 {
        (d + 30.0) / 40.0
    } else if d >= -30.0 {
        (d + 50.0) / 80.0
    } else if d >= -60.0 {
        (d + 70.0) / 160.0
    } else if d >= -90.0 {
        (d + 90.0) / 480.0
    } else {
        0.0
    }
}

/// Linear wire map: f 0..1 -> [min, max].
pub fn lin_to_unit(min: f32, max: f32, f: f32) -> f32 {
    min + (max - min) * f
}

pub fn unit_to_lin(min: f32, max: f32, x: f32) -> f32 {
    ((x - min) / (max - min)).clamp(0.0, 1.0)
}

/// Log wire map: f 0..1 -> [min, max] geometrically.
pub fn log_to_unit(min: f32, max: f32, f: f32) -> f32 {
    min * (max / min).powf(f)
}

pub fn unit_to_log(min: f32, max: f32, x: f32) -> f32 {
    ((x / min).ln() / (max / min).ln()).clamp(0.0, 1.0)
}

/// Meter sample: signed int16, 1/256 dB. 0x8000 (i16::MIN) is silence/-inf.
pub fn meter_i16_to_db(v: i16) -> f32 {
    if v == i16::MIN {
        f32::NEG_INFINITY
    } else {
        v as f32 / 256.0
    }
}

/// Preamp gain map for the 18-input rack units: -12..+60 dB linear.
pub const HEADAMP_DB_MIN: f32 = -12.0;
pub const HEADAMP_DB_MAX: f32 = 60.0;

/// Gate threshold: -80..0 dB linear.
pub const GATE_THR_MIN: f32 = -80.0;
pub const GATE_THR_MAX: f32 = 0.0;

/// Dyn threshold: -60..0 dB linear.
pub const DYN_THR_MIN: f32 = -60.0;
pub const DYN_THR_MAX: f32 = 0.0;

/// EQ gain: -15..+15 dB linear.
pub const EQ_GAIN_MIN: f32 = -15.0;
pub const EQ_GAIN_MAX: f32 = 15.0;

/// Dyn ratio enum index -> numeric ratio.
pub const DYN_RATIOS: [f32; 12] = [
    1.1, 1.3, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 7.0, 10.0, 20.0, 100.0,
];

/// Fader readout: "-9.1", "0.0", "-∞".
pub fn format_level_db(f: f32) -> String {
    let db = level_to_db(f);
    if db.is_infinite() {
        "-∞".to_string()
    } else {
        format!("{:.1}", db)
    }
}

/// Gain-style readout with sign: "+26.0 dB".
pub fn format_signed_db(db: f32) -> String {
    if db.is_infinite() {
        "-∞ dB".to_string()
    } else {
        format!("{:+.1} dB", db)
    }
}

/// Pan readout from wire float (0.5 = center): "C", "L45", "R100".
pub fn format_pan(f: f32) -> String {
    let pct = ((f - 0.5) * 200.0).round() as i32;
    if pct == 0 {
        "C".to_string()
    } else if pct < 0 {
        format!("L{}", -pct)
    } else {
        format!("R{}", pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taper_corners() {
        // Documented segment corners: 0/0.0625/0.25/0.5/1.0 <-> -90/-60/-30/-10/+10.
        assert_eq!(level_to_db(1.0), 10.0);
        assert_eq!(level_to_db(0.75), 0.0); // unity
        assert_eq!(level_to_db(0.5), -10.0);
        assert_eq!(level_to_db(0.25), -30.0);
        assert_eq!(level_to_db(0.0625), -60.0);
        assert!(level_to_db(0.0).is_infinite());
        assert_eq!(db_to_level(0.0), 0.75);
        assert_eq!(db_to_level(-10.0), 0.5);
        assert_eq!(db_to_level(-30.0), 0.25);
        assert_eq!(db_to_level(-60.0), 0.0625);
        assert_eq!(db_to_level(10.0), 1.0);
        assert_eq!(db_to_level(-120.0), 0.0);
    }

    #[test]
    fn taper_roundtrip() {
        for i in 1..=100 {
            let f = i as f32 / 100.0;
            let back = db_to_level(level_to_db(f));
            assert!((back - f).abs() < 1e-4, "f={} back={}", f, back);
        }
    }

    #[test]
    fn printed_scale_is_equidistant() {
        // The reason the fader scale labels can be evenly spaced: these dB
        // marks land exactly 0.125 apart on the wire float.
        let marks = [10.0, 5.0, 0.0, -5.0, -10.0, -20.0, -30.0, -50.0];
        for (i, db) in marks.iter().enumerate() {
            let f = db_to_level(*db);
            let expect = 1.0 - 0.125 * i as f32;
            assert!((f - expect).abs() < 1e-6, "db={} f={}", db, f);
        }
    }

    #[test]
    fn meter_samples() {
        assert!(meter_i16_to_db(i16::MIN).is_infinite());
        assert_eq!(meter_i16_to_db(0), 0.0); // clip
        let v = -23837; // 0xA2E3 as signed
        assert!((meter_i16_to_db(v) + 93.1).abs() < 0.1);
    }

    #[test]
    fn pan_format() {
        assert_eq!(format_pan(0.5), "C");
        assert_eq!(format_pan(0.0), "L100");
        assert_eq!(format_pan(1.0), "R100");
        assert_eq!(format_pan(0.75), "R50");
    }

    #[test]
    fn lin_log_roundtrip() {
        let g = unit_to_lin(HEADAMP_DB_MIN, HEADAMP_DB_MAX, 26.0);
        assert!((lin_to_unit(HEADAMP_DB_MIN, HEADAMP_DB_MAX, g) - 26.0).abs() < 1e-4);
        let f = unit_to_log(20.0, 20000.0, 1000.0);
        assert!((log_to_unit(20.0, 20000.0, f) - 1000.0).abs() < 0.5);
    }
}
