//! weather on the AI bus: one read tool over the forecast on screen — the
//! same tool the standalone service answers over its port and the module
//! executor answers on the root at call time.

use crate::view::WeatherView;
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};

/// The manifest shared by the standalone service and the module executor.
pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "weather",
        "Weather",
        "The forecast on screen for the selected city: current conditions and the 5-day outlook, from Open-Meteo.",
    )
    .with_tool(ToolDef::new(
        "current",
        "Current conditions and the 5-day forecast for the city selected on screen.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Read,
    ))
}

/// Answer one call against the forecast on screen. Every branch answers;
/// an unknown name is refused with the name that exists.
pub fn answer(view: &WeatherView, call: &ServiceCall) -> ToolResult {
    match call.tool.as_str() {
        "current" => ToolResult::ok(&call.call_id, view.ai_summary(), ""),
        other => ToolResult::failed(&call.call_id, format!("unknown tool `{other}`; this app only has `current`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_declares_one_read_tool() {
        let m = manifest();
        assert_eq!(m.id, "weather");
        assert!(m.validate().is_ok());
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].name, "current");
        assert_eq!(m.tools[0].risk, Risk::Read);
    }
}
