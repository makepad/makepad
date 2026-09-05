//! Tool broker — one registry of typed tools (route.md §2.1).
//!
//! Each tool has a name, a JSON schema, and an executor. Definitions render
//! to `makepad_ai::ToolDefinition` for the cloud backend (and later to Qwen
//! ChatML `<tools>` blocks for the local model). Executors are deterministic
//! Rust; results are compact digests sized for small contexts.
//!
//! Note: Claude tool names must match `[a-zA-Z0-9_-]+`, so the conceptual
//! `route.plan` from route.md becomes `route_plan` on the wire.

use makepad_widgets::*;
use makepad_converse::agent_seam::*;
use makepad_widgets::makepad_micro_serde::*;

use crate::nav::native::NavData;
use crate::tools;
use crate::trip::TripModel;

/// Ad-hoc marker spec (search results, sights) kept so trip redraws don't
/// wipe them; rebuilt into `MapMarker`s on every sync.
#[derive(Clone, Debug)]
pub struct MarkerSpec {
    pub lon: f64,
    pub lat: f64,
    pub label: String,
    pub kind: String,
}

/// Marker bookkeeping: stable ids + id→label legend for `marker_clicked`.
#[derive(Default)]
pub struct MarkerLegend {
    next_id: u64,
    pub names: Vec<(u64, String)>,
    pub adhoc: Vec<MarkerSpec>,
}

impl MarkerLegend {
    pub fn alloc(&mut self, label: &str) -> u64 {
        self.next_id += 1;
        self.names.push((self.next_id, label.to_string()));
        self.next_id
    }
    pub fn clear_names(&mut self) {
        self.names.clear();
    }
    pub fn name_of(&self, id: u64) -> Option<&str> {
        self.names
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str())
    }
}

/// Everything a tool executor may touch. Borrowed from App per call.
pub struct ToolCtx<'a> {
    pub cx: &'a mut Cx,
    pub map: &'a MapViewRef,
    pub trip: &'a mut TripModel,
    pub nav: Option<&'a mut NavData>,
    pub radar: Option<&'a crate::nav::native::RadarData>,
    pub markers: &'a mut MarkerLegend,
    /// Latest GPS fix (lon, lat) from the platform geo service, if any.
    pub position: Option<(f64, f64)>,
    pub layers: &'a mut crate::layers::LayerState,
    /// Per-leg routed Routes (with maneuvers) for turn-by-turn; filled by
    /// route planning, consumed by nav_start.
    pub leg_routes: &'a mut Vec<makepad_map_nav::graph::Route>,
    /// Set by nav tools; the app starts/stops navigation after the run.
    pub nav_action: &'a mut Option<crate::nav::NavAction>,
}

impl<'a> ToolCtx<'a> {
    /// Current map center, falling back to Amsterdam if unresolved.
    pub fn map_center(&self) -> (f64, f64) {
        self.map.center().unwrap_or((4.8952, 52.3702))
    }

    /// Where "here" is: the GPS fix when we have one, else the map center.
    pub fn here(&self) -> (f64, f64) {
        self.position.unwrap_or_else(|| self.map_center())
    }
    pub fn nav(&mut self) -> Result<&mut NavData, String> {
        match &mut self.nav {
            Some(nav) => Ok(nav),
            None => Err("navigation data is still loading — try again in a few seconds".into()),
        }
    }
}

pub fn def(name: &str, description: &str, parameters: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters: parameters.to_string(),
    }
}

/// The full v1 tool registry, rendered for the agent backend.
pub fn tool_definitions() -> Vec<ToolDefinition> {
    let mut out = Vec::new();
    out.extend(tools::map::defs());
    out.extend(tools::geo::defs());
    out.extend(tools::route::defs());
    out.extend(tools::weather::defs());
    out.push(def(
        "trip_history",
        "List recent recorded drives (date, distance, trip) from the on-disk drive history.",
        r#"{"type":"object","properties":{
            "limit":{"type":"integer","description":"max entries, default 10"}},
          "required":[]}"#,
    ));
    out.push(def(
        "nav_start",
        "Start turn-by-turn navigation on the current trip: maneuver banner, follow camera, live progress. Uses real GPS when a fix exists, otherwise a simulated drive.",
        r#"{"type":"object","properties":{
            "simulate":{"type":"boolean","description":"force the simulated drive even with GPS"}},
          "required":[]}"#,
    ));
    out.push(def(
        "nav_stop",
        "End turn-by-turn navigation (keeps the planned trip).",
        r#"{"type":"object","properties":{}}"#,
    ));
    out.push(def(
        "images_search",
        "Search DuckDuckGo images and show up to 4 thumbnails as cards (sights, places, buildings). Returns the image titles. Rate limited; needs network.",
        r#"{"type":"object","properties":{
            "query":{"type":"string"}},
          "required":["query"]}"#,
    ));
    out.push(def(
        "cloud_ask",
        "Escalate to the cloud model for world knowledge you don't have: what places are famous for, rankings, reviews, opening-hour customs, history. Costs a network round-trip and is unavailable offline — use only when local tools cannot answer.",
        r#"{"type":"object","properties":{
            "question":{"type":"string"},
            "context":{"type":"string","description":"compact digest of relevant app state (trip, candidates)"}},
          "required":["question"]}"#,
    ));
    out
}

/// One string field out of raw tool-args JSON.
pub fn parse_field(input: &str, key: &str) -> Option<String> {
    let args = parse_args(input).ok()?;
    arg_str(&args, key).map(|s| s.to_string())
}

/// Question text for cloud_ask (question + optional context digest).
pub fn parse_question(input: &str) -> String {
    let Ok(args) = parse_args(input) else {
        return input.to_string();
    };
    let question = arg_str(&args, "question").unwrap_or(input).to_string();
    match arg_str(&args, "context") {
        Some(context) if !context.is_empty() => format!("{question}\n\nContext:\n{context}"),
        _ => question,
    }
}

/// Dispatch one tool call. `input` is the raw JSON args text from the model.
pub fn execute(ctx: &mut ToolCtx, name: &str, input: &str) -> Result<String, String> {
    let args = parse_args(input)?;
    match name {
        "map_fly_to" => tools::map::fly_to(ctx, &args),
        "map_show_trip" => tools::map::show_trip(ctx, &args),
        "map_set_layer" => tools::map::set_layer(ctx, &args),
        "map_set_theme" => tools::map::set_theme(ctx, &args),
        "map_set_markers" => tools::map::set_markers(ctx, &args),
        "geo_search" => tools::geo::search(ctx, &args),
        "route_plan" => tools::route::plan(ctx, &args),
        "route_add_stop" => tools::route::add_stop(ctx, &args),
        "route_remove_stop" => tools::route::remove_stop(ctx, &args),
        "route_status" => tools::route::status(ctx, &args),
        "route_along" => tools::route::along(ctx, &args),
        "nav_start" => tools::route::nav_start(ctx, &args),
        "nav_stop" => tools::route::nav_stop(ctx, &args),
        // Async — intercepted by the app before reaching here; this arm is
        // the offline/unavailable fallback.
        "images_search" => Err("image search unavailable right now".into()),
        "weather_now" => tools::weather::now(ctx, &args),
        "trip_history" => Ok(crate::history::list_drives(
            arg_usize(&args, "limit").unwrap_or(10).clamp(1, 50),
        )),
        // Reached only when the app has no cloud agent (no key / offline).
        "cloud_ask" => Err("cloud unavailable (offline or no API key) — answer from local data or say you don't know".into()),
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn parse_args(input: &str) -> Result<JsonValue, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(JsonValue::Object(Default::default()));
    }
    JsonValue::deserialize_json(input).map_err(|e| format!("bad tool args: {e:?}"))
}

// --- JsonValue arg helpers (micro_serde has no serde_json-style accessors) ---

pub fn arg_f64(args: &JsonValue, key: &str) -> Option<f64> {
    num_f64(args.key(key)?)
}

pub fn num_f64(v: &JsonValue) -> Option<f64> {
    match v {
        JsonValue::F64(f) => Some(*f),
        JsonValue::I64(i) => Some(*i as f64),
        JsonValue::U64(u) => Some(*u as f64),
        JsonValue::I128(i) => Some(*i as f64),
        JsonValue::U128(u) => Some(*u as f64),
        _ => None,
    }
}

pub fn arg_str<'a>(args: &'a JsonValue, key: &str) -> Option<&'a str> {
    args.key(key)?.string().map(|s| s.as_str())
}

pub fn arg_usize(args: &JsonValue, key: &str) -> Option<usize> {
    arg_f64(args, key).map(|f| f.max(0.0) as usize)
}

pub fn arg_str_list(args: &JsonValue, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(JsonValue::Array(items)) = args.key(key) {
        for item in items {
            if let Some(s) = item.string() {
                out.push(s.clone());
            }
        }
    }
    out
}

pub fn arg_array<'a>(args: &'a JsonValue, key: &str) -> Option<&'a Vec<JsonValue>> {
    match args.key(key)? {
        JsonValue::Array(items) => Some(items),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema strings are spliced verbatim into the API request body —
    /// a typo would only surface as an HTTP 400. Validate them here.
    #[test]
    fn tool_schemas_are_valid_json() {
        let defs = tool_definitions();
        assert!(defs.len() >= 10);
        for def in &defs {
            assert!(
                def.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "tool name '{}' not API-safe",
                def.name
            );
            let parsed = JsonValue::deserialize_json(&def.parameters);
            assert!(
                matches!(parsed, Ok(JsonValue::Object(_))),
                "schema for '{}' is not a JSON object: {:?}",
                def.name,
                parsed
            );
        }
    }

    #[test]
    fn args_parse_and_accessors() {
        let args = parse_args(
            r#"{"query":"pizza","limit":5,"near_lon":4.9,"kinds":["a","b"]}"#,
        )
        .unwrap();
        assert_eq!(arg_str(&args, "query"), Some("pizza"));
        assert_eq!(arg_usize(&args, "limit"), Some(5));
        assert_eq!(arg_f64(&args, "near_lon"), Some(4.9));
        assert_eq!(arg_str_list(&args, "kinds"), vec!["a".to_string(), "b".to_string()]);
        assert!(arg_str(&args, "missing").is_none());
        // empty input → empty object
        assert!(parse_args("").is_ok());
    }
}
