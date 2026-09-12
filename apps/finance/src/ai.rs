//! Finance on the AI bus: one bounded read of the ledger on screen.
//!
//! The manifest is shared by the module's executor (`module.rs`) and, if a
//! standalone build later opens a service port toward the desktop the way
//! `apps/sheets` does, that port too — the same shape either way, answered
//! against [`crate::view::Finance`] at call time.

use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "finance",
        "Finance",
        "The household ledger on screen. Read net worth, the current range's money in and out, and which screen and account filter are showing.",
    )
    .with_tool(ToolDef::new(
        "summary",
        "Net worth, this range's income/expense/net, and the screen and account filter currently showing.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Read,
    ))
}

/// Run one call against a summary the caller already knows how to build —
/// kept lazy (`summary` is only called for the one tool that needs it) the
/// same way `apps/sheets` does it.
pub fn answer(call: &ServiceCall, summary: impl FnOnce() -> String) -> ToolResult {
    match call.tool.as_str() {
        "summary" => ToolResult::ok(&call.call_id, summary(), ""),
        other => ToolResult::failed(&call.call_id, format!("finance has no tool named `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_declares_the_one_read_tool() {
        let m = manifest();
        assert_eq!(m.id, "finance");
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].name, "summary");
        assert_eq!(m.tools[0].risk, Risk::Read);
    }

    #[test]
    fn an_unknown_tool_fails_instead_of_panicking() {
        let call = ServiceCall {
            call_id: "c1".to_string(),
            tool: "delete_everything".to_string(),
            args: "{}".to_string(),
        };
        let result = answer(&call, || "unreachable".to_string());
        assert!(!result.outcome.is_ok());
    }
}
