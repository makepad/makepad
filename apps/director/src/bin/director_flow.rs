//! Terminal transport for the flow assigned by Director's launch environment.
use makepad_strict_json::{self as json, Value};
use makepad_director::iteration_worker::{
    cli_capability_url, cli_environment_scope, cli_poll_reply, cli_request_id, cli_service_call,
    cli_submit, cli_tool_name,
};
use std::path::Path;
use std::time::{Duration, Instant};

const HELP: &str = "director-flow [--request-id <id>] <tool-or-alias> '<JSON object>'\ndirector-flow --result <id>\ndirector-flow --tools | --url | --help\n\nAliases: inspect, todos, requirement, prepared, build, freeze, feedback, git_inspect, diff.\nRun director-flow --tools for current argument schemas. Run director-flow --url for the current lane's local callback URL. Keep that capability private.\n\n--request-id <id> fixes the request identity so an interrupted call can be recovered with --result <id> (an identical repeat reuses the retained reply; changed arguments are refused).\n\nManaged screen sessions resolve their current owner from MAKEPAD_SCREEN_SESSION and MAKEPAD_SCREEN_STATE_DIR on every call, including after a split. Mirrored displays do not change ownership. Other terminals use their Director launch environment. Other flows and global tools are refused.\n\nRead apps/director/AGENTS.md. Inspect first; report compact todo deltas with expected revision v. Record requirements as they arrive. Prepared is an agent report; build admission is not a passing build. The human must close the prior app before the next compile. Never repeat an uncertain mutation blindly.\n\nReplies wait up to 30 seconds. Requests and immutable receipts are retained across UI restarts; interrupted requests are not replayed. Control history is bounded to 4096 receipts and 32 MiB of replies; full spools refuse new requests until reviewed cleanup.\n\nExamples:\n  director-flow todos '{\"v\":0,\"u\":[[\"fix-size\",\"w\",\"Fix window size\"]]}'";

fn print_tools() {
    let tools: Vec<Value> = makepad_director::iteration_tools::tool_defs()
        .into_iter()
        .filter(|tool| cli_tool_name(&tool.name).is_ok())
        .map(|tool| {
            json::obj(vec![
                ("tool", json::s(&tool.name)),
                ("description", json::s(&tool.description)),
                (
                    "args",
                    json::parse(tool.parameters.as_bytes()).unwrap_or(Value::Null),
                ),
            ])
        })
        .collect();
    println!("{}", Value::Arr(tools).to_json());
}

/// Submit one call and wait for its durable reply (at most 30 s).
fn call(flow: &str, control: &Path, id: &str, tool: &str, args: Value) -> Result<Value, String> {
    let call = cli_service_call(id, flow, tool, args)?;
    let reply = cli_submit(control, flow, &call)?;
    wait(flow, &reply, id)
}

fn wait(flow: &str, reply: &Path, id: &str) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = cli_poll_reply(reply, flow, id)? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Ok(json::obj(vec![("request_id", json::s(id)), ("flow_id", json::s(flow)), ("status", json::s("uncertain")),
                ("error", json::s("Timed out after 30 seconds. The request may still complete after Studio reconnects. Keep this request ID and read it with studio-flow --result <id> before repeating a mutation."))]));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn exit_code(value: &Value) -> i32 {
    match value.get("status").and_then(Value::as_str) {
        Some("ok") => 0,
        Some("uncertain") => 2,
        _ => 1,
    }
}

fn parse_args(body: &str) -> Result<Value, String> {
    if body.len() > 16 * 1024 {
        return Err("Tool arguments exceed 16 KiB".into());
    }
    let args = json::parse_depth(body.as_bytes(), 16).map_err(str::to_owned)?;
    if !matches!(args, Value::Obj(_)) {
        return Err("Tool arguments must be a JSON object".into());
    }
    Ok(args)
}

fn run() -> Result<i32, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().cloned().unwrap_or_else(|| "--help".into());
    if matches!(first.as_str(), "--help" | "-h" | "help") {
        println!("{HELP}");
        return Ok(0);
    }
    if first == "--tools" {
        print_tools();
        return Ok(0);
    }
    if first == "--url" {
        if args.len() > 1 {
            return Err("--url takes no arguments".into());
        }
        let (flow, control) = cli_environment_scope()?;
        println!("{}", cli_capability_url(&control, &flow)?);
        return Ok(0);
    }
    if first == "--result" {
        if args.len() != 2 {
            return Err("--result takes exactly one request id".into());
        }
        let id = args[1].clone();
        let (flow, control) = cli_environment_scope()?;
        let reply = control.join("replies").join(format!("{id}.json"));
        if cli_poll_reply(&reply, &flow, &id)?.is_none() {
            let known = ["requests", "processing", "receipts"]
                .iter()
                .any(|directory| control.join(directory).join(format!("{id}.json")).is_file());
            if !known {
                return Err("Unknown request id for this lane".into());
            }
        }
        let value = wait(&flow, &reply, &id)?;
        println!("{}", value.to_json());
        return Ok(exit_code(&value));
    }
    let mut request_id: Option<String> = None;
    if first == "--request-id" {
        if args.len() < 2 {
            return Err("--request-id needs an id, then the tool and its arguments".into());
        }
        request_id = Some(args[1].clone());
        args.drain(0..2);
    }
    let Some(tool) = args.first().cloned() else {
        println!("{HELP}");
        return Ok(0);
    };
    let body = args.get(1).cloned().unwrap_or_else(|| "{}".into());
    if args.len() > 2 {
        return Err(
            "Pass exactly one JSON argument object; quote it as a single shell argument".into(),
        );
    }
    let (flow, control) = cli_environment_scope()?;
    let id = request_id.unwrap_or_else(cli_request_id);
    let value = call(&flow, &control, &id, &tool, parse_args(&body)?)?;
    println!("{}", value.to_json());
    Ok(exit_code(&value))
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!(
                "{}",
                json::obj(vec![
                    ("status", json::s("refused")),
                    ("error", json::s(error))
                ])
                .to_json()
            );
            std::process::exit(1);
        }
    }
}
