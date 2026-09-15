//! The browser on the desktop's AI bus.
//!
//! The current CEF wrapper mirrors navigation metadata but does not bind its
//! frame text/source callbacks, so `page` reports the active title and URL.

use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_strict_json::{self as json, Value};

pub struct PageState {
    pub title: String,
    pub url: String,
}

pub struct TabState {
    pub title: String,
    pub url: String,
    pub active: bool,
}

/// The bus-facing subset of the webview. Keeping it as a trait makes the
/// closed dispatcher testable without starting CEF or a window.
pub trait BrowserTarget {
    fn page(&self) -> Option<PageState>;
    fn tabs(&self) -> Vec<TabState>;
    fn navigate(&mut self, url: &str) -> bool;
    fn new_tab(&mut self, url: &str);
}

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "browser",
        "Browser",
        "The live web browser. Its read tools report the active page and all tabs; its action tools steer the active tab or open a new one.",
    )
    .with_tool(ToolDef::new(
        "page",
        "Read the active tab's displayed title and URL. The current CEF binding does not expose visible page text or source, so this tool cannot return page text.",
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "tabs",
        "Read every open tab's displayed title and URL, with the active tab marked.",
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "navigate",
        "Navigate the active tab to an http:// or https:// URL, or to about:blank.",
        r#"{"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "new_tab",
        "Open and activate a new tab at an http:// or https:// URL, or at about:blank.",
        r#"{"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}"#,
        Risk::Act,
    ))
}

/// Answer one browser call through a closed match over the four advertised
/// names. URL actions use the webview's existing navigation methods.
pub fn answer(call: &ServiceCall, target: &mut impl BrowserTarget) -> ToolResult {
    match call.tool.as_str() {
        "page" => {
            if let Err(error) = empty_args(&call.args) {
                return ToolResult::refused(&call.call_id, error);
            }
            match target.page() {
                Some(page) => ToolResult::ok(
                    &call.call_id,
                    format!("title: {}\nurl: {}", page.title, page.url),
                    format!("{} — {}", page.title, page.url),
                ),
                None => ToolResult::unavailable(&call.call_id, "there is no active browser tab"),
            }
        }
        "tabs" => {
            if let Err(error) = empty_args(&call.args) {
                return ToolResult::refused(&call.call_id, error);
            }
            let tabs = target.tabs();
            let mut text = String::new();
            for (index, tab) in tabs.iter().enumerate() {
                if index > 0 {
                    text.push('\n');
                }
                text.push_str(if tab.active { "[active] " } else { "         " });
                text.push_str(&tab.title);
                text.push_str(" — ");
                text.push_str(&tab.url);
            }
            if text.is_empty() {
                text.push_str("no tabs open");
            }
            ToolResult::ok(&call.call_id, text, format!("{} tabs", tabs.len()))
        }
        "navigate" => {
            let url = match url_arg(&call.args, "navigate") {
                Ok(url) => url,
                Err(error) => return ToolResult::refused(&call.call_id, error),
            };
            if target.navigate(&url) {
                ToolResult::ok(&call.call_id, format!("navigating to {url}"), "navigating")
            } else {
                ToolResult::unavailable(&call.call_id, "there is no active browser tab")
            }
        }
        "new_tab" => {
            let url = match url_arg(&call.args, "new_tab") {
                Ok(url) => url,
                Err(error) => return ToolResult::refused(&call.call_id, error),
            };
            target.new_tab(&url);
            ToolResult::ok(&call.call_id, format!("opened a new tab at {url}"), "navigating")
        }
        other => ToolResult::refused(
            &call.call_id,
            format!("browser has no tool `{other}`; it has page, tabs, navigate, new_tab"),
        ),
    }
}

fn empty_args(args: &str) -> Result<(), String> {
    let fields = object_args(args)?;
    if let Some((key, _)) = fields.first() {
        return Err(format!("unknown argument `{key}`"));
    }
    Ok(())
}

fn url_arg(args: &str, tool: &str) -> Result<String, String> {
    let fields = object_args(args)?;
    if let Some((key, _)) = fields.iter().find(|(key, _)| key != "url") {
        return Err(format!("unknown argument `{key}`"));
    }
    let Some(url) = fields
        .iter()
        .find(|(key, _)| key == "url")
        .and_then(|(_, value)| value.as_str())
    else {
        return Err(format!("{tool}.url must be a string"));
    };
    if !allowed_url(url) {
        return Err(format!(
            "{tool}.url must start with http:// or https://, or be exactly about:blank"
        ));
    }
    Ok(url.to_string())
}

fn object_args(args: &str) -> Result<Vec<(String, Value)>, String> {
    match json::parse(args.as_bytes()) {
        Ok(Value::Obj(fields)) => Ok(fields),
        Ok(_) => Err("tool arguments must be a JSON object".to_string()),
        Err(error) => Err(format!("invalid tool arguments: {error}")),
    }
}

fn allowed_url(url: &str) -> bool {
    if url == "about:blank" {
        return true;
    }
    if url.chars().any(char::is_whitespace) || url.chars().any(char::is_control) {
        return false;
    }
    url.strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .is_some_and(|rest| !rest.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::wire::ToolOutcome;

    #[derive(Default)]
    struct FakeTarget {
        navigated: Vec<String>,
    }

    impl BrowserTarget for FakeTarget {
        fn page(&self) -> Option<PageState> {
            None
        }

        fn tabs(&self) -> Vec<TabState> {
            Vec::new()
        }

        fn navigate(&mut self, url: &str) -> bool {
            self.navigated.push(url.to_string());
            true
        }

        fn new_tab(&mut self, url: &str) {
            self.navigated.push(url.to_string());
        }
    }

    #[test]
    fn browser_manifest_validates() {
        manifest().validate().expect("a valid browser manifest");
    }

    #[test]
    fn navigate_refuses_non_http_urls() {
        let mut target = FakeTarget::default();
        for url in ["file:///etc/passwd", "javascript:alert(1)", "data:text/plain,no"] {
            let call = ServiceCall {
                call_id: "c1".into(),
                tool: "navigate".into(),
                args: format!(r#"{{"url":"{url}"}}"#),
            };
            let result = answer(&call, &mut target);
            assert_eq!(result.outcome, ToolOutcome::Refused);
        }
        assert!(target.navigated.is_empty());
    }
}
