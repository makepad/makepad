//! The hub control API from a terminal (contract:
//! local/agent_state/fast-rust/hub-vision/CONTROL_API.md), plus a chat probe
//! that can show the model a picture.
//!
//! ```text
//! hub-control <base> get | watch | load <model> | unload <model>
//! hub-control <base> yield <mb> [unload] [reserve_mb]   (what the WM asks under GPU pressure)
//! hub-control <base> context <tokens> | kv f16|q8        (applied at once)
//! hub-control <base> ask <model> <prompt> [--image <png|jpg>] [--max N]
//!             [--session S] [--thinking true|false] [--system <text>] [--temp T]
//! hub-control <base> gen <body.json>          (POST /generate, wait, print status)
//! hub-control <base> artifact <id> <out-file>
//! hub-control <base> raw <path>                (GET any JSON route, e.g. /v1/model_inventory)
//! ```
//!
//! `<base>` is e.g. `https://10.0.0.149:8123` (fleet TLS + credential as for
//! any hub client). `ask` prints the answer, then one line of timings: total,
//! time to the first token, generated tokens and decode tok/s.

use makepad_ai_hub::providers::qwen::{FleetTransport, HttpFleetTransport};
use makepad_strict_json::{self as json, Value};
use std::time::{Duration, Instant};

fn main() {
    if let Err(err) = run() {
        eprintln!("hub-control: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = args.first().ok_or("usage: hub-control <base> <command> ...")?.trim_end_matches('/').to_string();
    let command = args.get(1).map(String::as_str).unwrap_or("get");
    let rest = &args[2.min(args.len())..];
    let mut t = HttpFleetTransport;
    match command {
        "get" => println!("{}", t.get_json(&format!("{base}/control"))?.to_json()),
        "watch" => {
            let mut since = 0u64;
            loop {
                let v = t.get_json(&format!("{base}/control/watch?since={since}"))?;
                since = v.get("seq").and_then(Value::as_u64).unwrap_or(since);
                println!("{}", v.to_json());
            }
        }
        "yield" => {
            // what the WM sends under GPU pressure: yield <mb> [unload] [reserve_mb]
            let mb: i64 = rest.first().ok_or("how many MB?")?.parse().map_err(|_| "MB must be a number")?;
            let unload = rest.iter().any(|a| a == "unload");
            let mut fields = vec![("mb", Value::Int(mb)), ("unload", Value::Bool(unload))];
            if let Some(reserve) = rest.iter().skip(1).find_map(|a| a.parse::<i64>().ok()) {
                fields.push(("reserve_mb", Value::Int(reserve)));
            }
            let v = t.post_json(&format!("{base}/control/yield"), &json::obj(fields))?;
            println!("{}", v.to_json());
            if let Some(job) = v.get("job_id").and_then(Value::as_str) {
                let done = wait_job(&mut t, &base, job)?;
                println!("{}", done.get("text").and_then(Value::as_str).unwrap_or("(no report)"));
            }
        }
        "load" | "unload" => {
            let model = rest.first().ok_or("which model?")?;
            let v = t.post_json(
                &format!("{base}/control/model"),
                &json::obj(vec![("model", json::s(model.as_str())), ("action", json::s(command))]),
            )?;
            println!("{}", v.to_json());
            if let Some(job) = v.get("job_id").and_then(Value::as_str) {
                wait_job(&mut t, &base, job)?;
            }
        }
        "context" | "kv" => {
            let value = rest.first().ok_or("which value?")?;
            let field = if command == "context" {
                ("context", Value::Int(value.parse::<i64>().map_err(|e| e.to_string())?))
            } else {
                ("kv", json::s(value.as_str()))
            };
            let v = t.post_json(
                &format!("{base}/llm/context"),
                &json::obj(vec![field, ("apply", Value::Bool(true))]),
            )?;
            println!("{}", v.to_json());
            if let Some(job) = v.get("job_id").and_then(Value::as_str) {
                wait_job(&mut t, &base, job)?;
                println!("{}", t.get_json(&format!("{base}/llm/context"))?.to_json());
            }
        }
        "ask" => ask(&mut t, &base, rest)?,
        "gen" => {
            let path = rest.first().ok_or("gen <body.json>")?;
            let text = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            let body = json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
            let started = Instant::now();
            let v = t.post_json(&format!("{base}/generate"), &body)?;
            let job = v.get("job_id").and_then(Value::as_str).ok_or("no job id")?.to_string();
            let done = wait_job(&mut t, &base, &job)?;
            println!("{}", done.to_json());
            eprintln!("total={:.2}s", started.elapsed().as_secs_f64());
        }
        "raw" => {
            let path = rest.first().ok_or("raw <path>")?;
            println!("{}", t.get_json(&format!("{base}{path}"))?.to_json());
        }
        "artifact" => {
            let (id, out) = (rest.first().ok_or("artifact <id> <out>")?, rest.get(1).ok_or("artifact <id> <out>")?);
            let url = format!("{base}/artifact/{id}");
            makepad_ai_hub::client::mark_if_fleet(&base);
            let response = makepad_ai_hub::http_client::http_fetch(&makepad_ai_hub::http_client::HttpClientRequest::get(&url))
                .map_err(|e| e.to_string())?;
            let bytes = response.read_body_to_vec(256 << 20).map_err(|e| e.to_string())?;
            std::fs::write(out, &bytes).map_err(|e| format!("{out}: {e}"))?;
            eprintln!("{} bytes -> {out}", bytes.len());
        }
        other => return Err(format!("unknown command {other}")),
    }
    Ok(())
}

fn wait_job(t: &mut HttpFleetTransport, base: &str, job: &str) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let v = t.get_json(&format!("{base}/job/{job}"))?;
        match v.get("state").and_then(Value::as_str) {
            Some("done") => {
                eprintln!("{job} done in {:.1}s", started.elapsed().as_secs_f64());
                return Ok(v);
            }
            Some("error") | Some("cancelled") => {
                return Err(format!("{job}: {}", v.get("error").and_then(Value::as_str).unwrap_or("failed")))
            }
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn ask(t: &mut HttpFleetTransport, base: &str, rest: &[String]) -> Result<(), String> {
    let model = rest.first().ok_or("ask <model> <prompt>")?;
    let prompt = rest.get(1).ok_or("ask <model> <prompt>")?;
    let (mut images, mut max, mut session, mut thinking, mut system, mut temp) =
        (Vec::new(), 512i64, None, None, String::new(), 0.0f64);
    let mut i = 2;
    while i + 1 < rest.len() {
        let value = &rest[i + 1];
        match rest[i].as_str() {
            "--image" => {
                let bytes = std::fs::read(value).map_err(|e| format!("{value}: {e}"))?;
                images.push(json::s(
                    String::from_utf8(makepad_base64::base64_encode(&bytes, &makepad_base64::BASE64_STANDARD))
                        .unwrap(),
                ));
            }
            "--max" => max = value.parse().map_err(|_| "--max N")?,
            "--session" => session = Some(value.clone()),
            "--thinking" => thinking = Some(value == "true"),
            "--system" => system = value.clone(),
            "--temp" => temp = value.parse().map_err(|_| "--temp T")?,
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    let prompt = match prompt.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => prompt.clone(),
    };
    let mut message = vec![("role", json::s("user")), ("text", json::s(prompt))];
    if !images.is_empty() {
        message.push(("images", Value::Arr(images)));
    }
    let mut body = vec![
        ("model", json::s(model.as_str())),
        ("domain", json::s("chat")),
        ("chat_system", json::s(system)),
        ("chat_messages", Value::Arr(vec![json::obj(message)])),
        ("max_tokens", Value::Int(max)),
        ("temperature", Value::F64(temp)),
        ("seed", Value::Int(1)),
    ];
    if let Some(session) = session {
        body.push(("chat_session", json::s(session)));
    }
    if let Some(thinking) = thinking {
        body.push(("thinking", Value::Bool(thinking)));
    }
    let started = Instant::now();
    let v = t.post_json(&format!("{base}/generate"), &json::obj(body))?;
    let job = v.get("job_id").and_then(Value::as_str).ok_or("no job id")?.to_string();
    let (mut first, mut stages) = (None, Vec::new());
    loop {
        let v = t.get_json(&format!("{base}/job/{job}"))?;
        let gen = v.get("serving").and_then(|s| s.get("gen_tokens")).and_then(Value::as_u64).unwrap_or(0);
        if first.is_none() && gen > 0 {
            first = Some((started.elapsed(), gen));
        }
        if let Some(stage) = v.get("stage").and_then(Value::as_str) {
            if stages.last().map(String::as_str) != Some(stage) && !stage.starts_with("decode") {
                stages.push(stage.to_string());
            }
        }
        match v.get("state").and_then(Value::as_str) {
            Some("done") => {
                let total = started.elapsed();
                let text = v.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                println!("{text}");
                let rate = first.map(|(at, g0)| (gen.saturating_sub(g0)) as f64 / (total - at).as_secs_f64().max(1e-3));
                eprintln!(
                    "total={:.2}s first_token={} gen={gen} decode={} stages={}",
                    total.as_secs_f64(),
                    first.map_or("?".to_string(), |(at, _)| format!("{:.2}s", at.as_secs_f64())),
                    rate.map_or("?".to_string(), |r| format!("{r:.1}tok/s")),
                    stages.join(" | ")
                );
                return Ok(());
            }
            Some("error") | Some("cancelled") => {
                return Err(v.get("error").and_then(Value::as_str).unwrap_or("failed").to_string())
            }
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}
