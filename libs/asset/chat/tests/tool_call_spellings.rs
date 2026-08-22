//! Every tool-call spelling a served model has actually produced here.
//!
//! The extractor is the one place that decides whether a turn does work or
//! dies. Each line below cost a real conversation before it parsed: the
//! `tool_name` variant left the model retrying the same block forever while
//! the user watched an empty reply, and the namespaced `world` + `action`
//! shape got a bare "unknown tool" that convinced it it had no tools at all.

use makepad_asset_chat::toolcall::{extract, Extract};

#[test]
fn the_observed_tool_call_spellings_all_parse() {
    let bodies = [
        // The generation surface's own marker.
        r#"<<tool>>{"name":"world.get_source","args":{}}"#,
        // Qwen on the fleet box, verbatim.
        "<tool_call>\n{\"tool_name\": \"world.get_source\", \"arguments\": {}}\n</tool_call>",
        "<tool_call>\n{\"function\": \"world.get_source\", \"arguments\": {}}\n</tool_call>",
        "<tool_call>\n{\"name\": \"world.get_source\", \"parameters\": {}}\n</tool_call>",
        // The trained template the agentic surface teaches.
        "<tool_call>\n<function=world.get_source>\n</function>\n</tool_call>",
        // Same, with the args as a bare JSON body.
        "<tool_call>\n<function=world.get_source>\n{}\n</function>\n</tool_call>",
    ];
    for body in bodies {
        match extract(body) {
            Extract::Call { name, .. } => assert_eq!(name, "world.get_source", "{body}"),
            other => panic!("{body} did not parse: {other:?}"),
        }
    }
}

/// The arguments survive the spelling, not just the name.
#[test]
fn arguments_arrive_whatever_the_key_is_called() {
    for body in [
        "<tool_call>\n{\"tool_name\": \"world.spawn\", \"arguments\": {\"model\": \"kenney/x\"}}\n</tool_call>",
        "<tool_call>\n{\"name\": \"world.spawn\", \"parameters\": {\"model\": \"kenney/x\"}}\n</tool_call>",
        "<tool_call>\n<function=world.spawn>\n<parameter=model>\nkenney/x\n</parameter>\n</function>\n</tool_call>",
    ] {
        match extract(body) {
            Extract::Call { name, args, .. } => {
                assert_eq!(name, "world.spawn", "{body}");
                assert_eq!(
                    args.get("model").and_then(makepad_asset_client::json::Value::as_str),
                    Some("kenney/x"),
                    "{body}"
                );
            }
            other => panic!("{body} did not parse: {other:?}"),
        }
    }
}
