//! Terminal transport for the flow assigned by Studio's launch environment.
use makepad_strict_json::{self as json, Value};
use makepad_studio::iteration_worker::{
    cli_capability_url, cli_code_tool, cli_environment_scope, cli_export, cli_poll_reply,
    cli_request_id, cli_service_call, cli_submit, cli_tool_name, CODE_EXPORT_LIMIT,
};
use std::path::Path;
use std::time::{Duration, Instant};

const HELP: &str = "studio-flow [--request-id <id>] <tool-or-alias> '<JSON object>'\nstudio-flow --result <id>\nstudio-flow --full <name> <code-tool> '<JSON object>'\nstudio-flow --tools | --url | --help\n\nAliases: inspect, todos, requirement, prepared, build, freeze, feedback, git_inspect, diff.\nCode-intelligence tools (code_brief, code_impact, code_references, code_search, code_neighbors, ...) answer with an envelope {request_id, worktree, revision, analysis_context_hash, coverage, freshness, basis_policy, rows, truncated, cursor, error, notes}; pass the returned cursor with the same arguments for the next page. Their scope is this lane's own worktree; `worktree` names another active lane. Run studio-flow --tools for current argument schemas and the code guidance. Run studio-flow --url for the current lane's local callback URL. Keep that capability private.\n\n--request-id <id> fixes the request identity so an interrupted call can be recovered with --result <id> (an identical repeat reuses the retained reply; changed arguments are refused). --full <name> follows every page of one code tool into <control>/exports/<name>.json (at most 8 MiB, never over an existing file). code_arch_diff answers later: read it with --result <id>.\n\nManaged screen sessions resolve their current owner from MAKEPAD_SCREEN_SESSION and MAKEPAD_SCREEN_STATE_DIR on every call, including after a split. Mirrored displays do not change ownership. Other terminals use their Studio launch environment. Other flows and global tools are refused.\n\nRead apps/studio/AGENTS.md. Inspect first; report compact todo deltas with expected revision v. Record requirements as they arrive. Prepared is an agent report; build admission is not a passing build. The human must close the prior app before the next compile. Never repeat an uncertain mutation blindly.\n\nReplies wait up to 30 seconds. Requests and immutable receipts are retained across UI restarts; interrupted requests are not replayed. Control history is bounded to 4096 receipts and 32 MiB of replies; full spools refuse new requests until reviewed cleanup.\n\nExamples:\n  studio-flow todos '{\"v\":0,\"u\":[[\"fix-size\",\"w\",\"Fix window size\"]]}'\n  studio-flow code_brief '{\"scope\":\"widgets::dock\"}'\n  studio-flow code_impact '{\"files\":[\"widgets/src/dock.rs\"],\"depth\":4}'";

fn print_tools() {
    let mut tools: Vec<Value> = makepad_studio::iteration_tools::tool_defs()
        .into_iter()
        .chain(makepad_studio::atlas::registry::lane_tool_defs(false))
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
    tools.push(json::obj(vec![
        ("guidance", json::s(makepad_studio::atlas::registry::LANE_GUIDANCE)),
        ("envelope", json::s("Code tools reply with {request_id, tool, worktree, revision, analysis_context_hash, coverage, freshness, basis_policy, rows, truncated, cursor, error, notes}. A typed error is in error.kind: stale_revision, context_mismatch, unknown_key, ambiguous_key, stale_cursor, budget_exceeded, indexer_busy (retry_after_ms), rate_limited (retry_after_ms), permission_denied, invalid_arguments, unavailable.")),
    ]));
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

/// Follow every page of one code tool into one export document. The
/// document's metadata is budgeted with its rows; when the 8 MiB limit is
/// reached inside a page the export records the cursor that produced that
/// page and how many of its rows were taken, so the caller can resume
/// exactly there; recoverable admissions (`rate_limited`, `indexer_busy`)
/// are retried after their `retry_after_ms`; every request id is retained.
fn full(flow: &str, control: &Path, name: &str, tool: &str, args: Value) -> Result<Value, String> {
    if !cli_code_tool(tool) {
        return Err("--full follows the pages of a code-intelligence tool only".into());
    }
    let Value::Obj(fields) = &args else {
        return Err("Tool arguments must be a JSON object".into());
    };
    if fields.iter().any(|(key, _)| key == "cursor") {
        return Err("--full starts from the first page; omit cursor".into());
    }
    let args = makepad_studio::atlas::envelope::redact_value(&args);
    let mut rows: Vec<Value> = Vec::new();
    let mut request_ids: Vec<Value> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut first: Option<Value> = None;
    let mut truncated = false;
    let mut resume: Option<Value> = None;
    let mut failure: Option<Value> = None;
    let mut pages = 0u32;
    let mut retries = 0u32;
    // the metadata is budgeted first: what is left holds rows
    let metadata_reserve = 64 * 1024 + args.to_json().len();
    let row_budget = CODE_EXPORT_LIMIT.saturating_sub(metadata_reserve);
    let mut bytes = 0usize;
    loop {
        let mut page_args = args.clone();
        if let (Value::Obj(fields), Some(c)) = (&mut page_args, &cursor) {
            fields.push(("cursor".into(), json::s(c)));
        }
        let id = cli_request_id();
        request_ids.push(json::s(&id));
        let reply = call(flow, control, &id, tool, page_args)?;
        if reply.get("status").and_then(Value::as_str) != Some("ok") {
            let error = reply.get("result").and_then(|r| r.get("error"));
            let kind = error.and_then(|e| e.get("kind")).and_then(Value::as_str).unwrap_or("");
            let retry_ms = error.and_then(|e| e.get("retry_after_ms")).and_then(Value::as_u64);
            if matches!(kind, "rate_limited" | "indexer_busy") && retries < 40 {
                retries += 1;
                std::thread::sleep(Duration::from_millis(retry_ms.unwrap_or(500).clamp(50, 5000)));
                continue;
            }
            // keep what was collected; the page that failed is resumable by its cursor
            failure = Some(reply);
            resume = Some(json::obj(vec![("page_cursor", cursor.as_deref().map(json::s).unwrap_or(Value::Null)), ("rows_taken", Value::Int(0))]));
            break;
        }
        let envelope = reply.get("result").cloned().ok_or("Reply has no result")?;
        pages += 1;
        let next = envelope.get("cursor").and_then(Value::as_str).map(str::to_owned);
        let mut taken = 0usize;
        let mut stopped = false;
        if let Some(Value::Arr(page)) = envelope.get("rows") {
            for row in page {
                let len = row.to_json().len() + 1;
                if bytes + len > row_budget {
                    stopped = true;
                    break;
                }
                bytes += len;
                rows.push(row.clone());
                taken += 1;
            }
        }
        truncated |= envelope.get("truncated") == Some(&Value::Bool(true)) && next.is_none();
        if first.is_none() {
            first = Some(envelope);
        }
        if stopped {
            // resume inside this page: the cursor that produced it and the rows already taken
            resume = Some(json::obj(vec![("page_cursor", cursor.as_deref().map(json::s).unwrap_or(Value::Null)), ("rows_taken", Value::Int(taken as i64)), ("next_cursor", next.as_deref().map(json::s).unwrap_or(Value::Null))]));
            break;
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
        if pages >= 4096 {
            resume = Some(json::obj(vec![("page_cursor", cursor.as_deref().map(json::s).unwrap_or(Value::Null)), ("rows_taken", Value::Int(0))]));
            break;
        }
    }
    let first = first.unwrap_or(Value::Null);
    let complete = resume.is_none() && failure.is_none();
    let mut document = vec![
        ("schema_version", Value::Int(1)),
        ("flow_id", json::s(flow)),
        ("tool", json::s(tool)),
        ("args", args),
        ("request_ids", Value::Arr(request_ids)),
        ("pages", Value::Int(pages as i64)),
        ("complete", Value::Bool(complete)),
    ];
    for key in ["worktree", "revision", "analysis_context_hash", "coverage", "freshness", "basis_policy", "notes"] {
        if let Some(v) = first.get(key) {
            document.push((key, v.clone()));
        }
    }
    document.push(("rows", Value::Arr(rows)));
    document.push(("truncated", Value::Bool(truncated || !complete)));
    document.push(("resume", resume.clone().unwrap_or(Value::Null)));
    document.push(("failure", failure.clone().unwrap_or(Value::Null)));
    let document = json::obj(document);
    let body = document.to_json();
    if body.len() > CODE_EXPORT_LIMIT {
        return Err(format!("Export document is {} bytes, over the 8 MiB limit", body.len()));
    }
    let path = cli_export(control, name, &body)?;
    Ok(json::obj(vec![
        ("status", json::s(if failure.is_some() { "uncertain" } else { "ok" })),
        ("file", json::s(path.to_string_lossy())),
        ("pages", Value::Int(pages as i64)),
        ("rows", document.get("rows").map(|r| if let Value::Arr(r) = r { r.len() } else { 0 }).map(|n| Value::Int(n as i64)).unwrap_or(Value::Null)),
        ("bytes", Value::Int(body.len() as i64)),
        ("complete", Value::Bool(complete)),
        ("resume", resume.unwrap_or(Value::Null)),
        ("failure", failure.map(|f| f.get("result").and_then(|r| r.get("error")).cloned().unwrap_or(f)).unwrap_or(Value::Null)),
    ]))
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
    if args.first().map(String::as_str) == Some("--full") {
        if args.len() < 3 {
            return Err("--full needs an export name, the code tool and its JSON arguments".into());
        }
        let name = args[1].clone();
        let tool = args[2].clone();
        let body = args.get(3).cloned().unwrap_or_else(|| "{}".into());
        if args.len() > 4 {
            return Err("Pass exactly one JSON argument object; quote it as a single shell argument".into());
        }
        let (flow, control) = cli_environment_scope()?;
        let value = full(&flow, &control, &name, &tool, parse_args(&body)?)?;
        println!("{}", value.to_json());
        return Ok(exit_code(&value));
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
