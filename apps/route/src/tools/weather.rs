//! weather.* tools — numeric radar nowcast (never VLM-on-pixels; user rule).

use makepad_ai::makepad_micro_serde::JsonValue;
use makepad_ai::ToolDefinition;

use crate::broker::{arg_f64, def, ToolCtx};
use crate::nav_data::RadarGrid;

pub fn defs() -> Vec<ToolDefinition> {
    vec![def(
        "weather_now",
        "Rain nowcast from KNMI radar at a point: mm/h now and +30/+60/+120 minutes. Covers the Netherlands and nearby; beyond 2 hours there is no data yet.",
        r#"{"type":"object","properties":{
            "lon":{"type":"number","description":"default: current map center"},
            "lat":{"type":"number"}},
          "required":[]}"#,
    )]
}

pub fn now(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    let (center_lon, center_lat) = ctx.map_center();
    let lon = arg_f64(args, "lon").unwrap_or(center_lon);
    let lat = arg_f64(args, "lat").unwrap_or(center_lat);
    let radar = ctx
        .radar
        .ok_or("radar nowcast not available yet (still syncing from KNMI)")?;
    if radar.frames.is_empty() {
        return Err("radar file decoded to zero frames".into());
    }
    let grid = RadarGrid::new();
    let mut out = format!("rain at {lon:.4},{lat:.4} (radar {}):\n", radar.stamp);
    let mut any = false;
    for target_min in [0u32, 30, 60, 120] {
        let Some(frame) = radar
            .frames
            .iter()
            .find(|f| f.minutes_offset == target_min)
        else {
            continue;
        };
        let label = if target_min == 0 {
            "now".to_string()
        } else {
            format!("+{target_min}min")
        };
        match grid.sample_mm_h(frame, lon, lat) {
            Some(mm_h) => {
                any = true;
                out.push_str(&format!("{label}: {:.1} mm/h ({})\n", mm_h, classify(mm_h)));
            }
            None => out.push_str(&format!("{label}: outside radar coverage\n")),
        }
    }
    if !any {
        return Ok(format!(
            "{lon:.4},{lat:.4} is outside KNMI radar coverage (Netherlands + ~100km); no rain data"
        ));
    }
    Ok(out)
}

fn classify(mm_h: f64) -> &'static str {
    if mm_h < 0.1 {
        "dry"
    } else if mm_h < 1.0 {
        "light"
    } else if mm_h < 4.0 {
        "moderate"
    } else if mm_h < 10.0 {
        "heavy"
    } else {
        "intense"
    }
}
