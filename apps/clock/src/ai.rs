//! clock on the AI bus: one read tool over the state on screen — the same
//! tool the standalone service answers over its port and the module
//! executor answers on the root at call time.

use crate::view::ClockView;
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};

/// The manifest shared by the standalone service and the module executor.
pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "clock",
        "Clock",
        "The clock on screen: local time and date, the next alarm, and whether a timer or the stopwatch is running.",
    )
    .with_tool(ToolDef::new(
        "now",
        "The current local time and date, the next alarm, and any running timer or stopwatch.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Read,
    ))
}

/// Answer one call against the clock on screen. Every branch answers; an
/// unknown name is refused with the name that exists.
pub fn answer(view: &ClockView, call: &ServiceCall) -> ToolResult {
    match call.tool.as_str() {
        "now" => ToolResult::ok(&call.call_id, view.ai_summary(), ""),
        other => ToolResult::failed(&call.call_id, format!("unknown tool `{other}`; this app only has `now`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_declares_one_read_tool() {
        let m = manifest();
        assert_eq!(m.id, "clock");
        assert!(m.validate().is_ok());
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].name, "now");
        assert_eq!(m.tools[0].risk, Risk::Read);
    }
}
