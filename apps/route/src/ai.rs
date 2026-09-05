//! Route's tools on the desktop assistant bus.

use crate::{broker, App};
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_widgets::Cx;

/// The bus already supplies the `route.` namespace, so only route's own
/// prefix is removed. All descriptions and schemas still come from the one
/// registry used by the in-app assistant.
fn bus_name(internal: &str) -> &str {
    match internal {
        "route_plan" => "plan",
        "route_add_stop" => "add_stop",
        "route_remove_stop" => "remove_stop",
        "route_status" => "status",
        "route_along" => "along",
        _ => internal,
    }
}

fn risk(internal: &str) -> Risk {
    match internal {
        "geo_search" | "route_status" | "weather_now" | "trip_history"
        | "images_search" | "cloud_ask" => Risk::Read,
        "map_fly_to" | "map_show_trip" | "map_set_layer" | "map_set_theme"
        | "map_set_markers" | "route_plan" | "route_add_stop"
        | "route_remove_stop" | "route_along" | "nav_start" | "nav_stop" => Risk::Act,
        name => panic!("route tool `{name}` has no AI-bus risk class"),
    }
}

pub fn manifest() -> ServiceManifest {
    let mut manifest = ServiceManifest::new(
        "route",
        "Route",
        "The map on screen, including its centre, zoom, layers, markers, weather and current trip. Route-planning tools change the trip drawn on screen.",
    );
    for definition in broker::tool_definitions() {
        let tool_name = bus_name(&definition.name);
        manifest = manifest.with_tool(ToolDef::new(
            tool_name,
            definition.description,
            definition.parameters,
            risk(&definition.name),
        ));
    }
    manifest
}

fn internal_name(short_name: &str) -> Option<String> {
    broker::tool_definitions()
        .into_iter()
        .find(|definition| bus_name(&definition.name) == short_name)
        .map(|definition| definition.name)
}

fn result(call: &ServiceCall, outcome: Result<String, String>) -> ToolResult {
    match outcome {
        Ok(text) => ToolResult::ok(
            &call.call_id,
            text,
            format!("route.{} completed", call.tool),
        ),
        Err(error) if error.starts_with("unknown tool:") => {
            let names = manifest()
                .tools
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
                .join(", ");
            ToolResult::refused(
                &call.call_id,
                format!("unknown route tool `{}`; available tools: {names}", call.tool),
            )
        }
        Err(error) => ToolResult::failed(&call.call_id, error),
    }
}

/// Execute one desktop-bus call through the exact dispatcher used by route's
/// own assistant.
fn answer_with(
    call: &ServiceCall,
    execute: impl FnOnce(&str, &str) -> Result<String, String>,
) -> ToolResult {
    let Some(name) = internal_name(&call.tool) else {
        return result(call, Err(format!("unknown tool: {}", call.tool)));
    };
    result(call, execute(&name, &call.args))
}

pub fn answer(cx: &mut Cx, app: &mut App, call: &ServiceCall) -> ToolResult {
    answer_with(call, |name, args| app.execute_tool(cx, name, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::wire::ToolOutcome;

    #[test]
    fn manifest_is_valid_and_matches_the_local_tool_registry() {
        let manifest = manifest();
        manifest.validate().unwrap();
        let local_names = broker::tool_definitions()
            .iter()
            .map(|definition| bus_name(&definition.name).to_string())
            .collect::<Vec<_>>();
        let bus_names = manifest
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(bus_names, local_names);
    }

    #[test]
    fn unknown_calls_are_refused_with_the_real_names() {
        let call = ServiceCall {
            call_id: "call-unknown".into(),
            tool: "not_a_route_tool".into(),
            args: "{}".into(),
        };
        let answer = answer_with(&call, |_, _| panic!("unknown tools must not execute"));
        assert_eq!(answer.outcome, ToolOutcome::Refused);
        for tool in manifest().tools {
            assert!(answer.text.contains(&tool.name), "missing {} in {}", tool.name, answer.text);
        }
    }

    #[test]
    fn risk_classes_match_route_state_effects() {
        let manifest = manifest();
        for name in [
            "geo_search",
            "status",
            "weather_now",
            "trip_history",
            "images_search",
            "cloud_ask",
        ] {
            assert_eq!(manifest.tool(name).unwrap().risk, Risk::Read, "{name}");
        }
        for name in [
            "map_fly_to",
            "map_show_trip",
            "map_set_layer",
            "map_set_theme",
            "map_set_markers",
            "plan",
            "add_stop",
            "remove_stop",
            "along",
            "nav_start",
            "nav_stop",
        ] {
            assert_eq!(manifest.tool(name).unwrap().risk, Risk::Act, "{name}");
        }
    }
}
