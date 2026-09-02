//! sheets on the AI bus: one read tool, `summary`.
//!
//! The pilot for the Window overlay: the app opens one port with
//! [`AiServicePort::open`] — hosted by the window manager that is the bus,
//! standalone it is an in-process link the F10 overlay adopts — and
//! answers `sheets.summary` with what is on screen. The tool only looks.

use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};

/// The manifest: who the app is and the one tool it exposes.
pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "sheets",
        "Sheets",
        "The spreadsheet on screen. Its one tool only reads: the sheet's name, \
         its used range, the header row and the current selection.",
    )
    .with_tool(ToolDef::new(
        "summary",
        "The sheet on screen: its name, how many sheets the workbook has, the used range, the header row and the selection.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Read,
    ))
}

/// Answer one call. `summary` is read from the view at call time; every
/// other name is refused with the names that exist.
pub fn answer(call: &ServiceCall, summary: impl FnOnce() -> String) -> ToolResult {
    match call.tool.as_str() {
        "summary" => ToolResult::ok(&call.call_id, summary(), "the sheet on screen"),
        other => ToolResult::refused(&call.call_id, format!("sheets has no tool `{other}`; it has summary")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::wire::ToolOutcome;

    #[test]
    fn the_manifest_validates_and_only_reads() {
        let m = manifest();
        assert_eq!(m.id, "sheets");
        m.validate().expect("a manifest the wire accepts");
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].name, "summary");
        assert_eq!(m.tools[0].risk, Risk::Read);
    }

    #[test]
    fn summary_answers_and_unknown_tools_are_refused() {
        let call = ServiceCall { call_id: "c1".into(), tool: "summary".into(), args: "{}".into() };
        let result = answer(&call, || "Sheet \"Budget\" (1 of 1), used A1:D4".into());
        assert_eq!(result.outcome, ToolOutcome::Ok);
        assert!(result.text.starts_with("Sheet \"Budget\""));
        let call = ServiceCall { call_id: "c2".into(), tool: "write_cell".into(), args: "{}".into() };
        let result = answer(&call, || unreachable!("never read for a refused call"));
        assert_eq!(result.outcome, ToolOutcome::Refused);
        assert!(result.text.contains("summary"));
    }
}
