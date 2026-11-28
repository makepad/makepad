//! The node's context-size and KV-precision ops, behind a seam so the engine
//! and its tests do not depend on a live node. The hub applies changes at
//! its next LLM job. Context switches ask the hub to load immediately and
//! wait for the load job before reading the fitted window.

use super::event::{ContextOption, ContextReport};
use makepad_ai_hub::providers::qwen::{FleetTransport, HttpFleetTransport};
use makepad_strict_json::{self as json, Value};
use std::time::{Duration, Instant};

/// Where the chat session runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub base: String,
    pub model: Option<String>,
}

pub trait ContextHub: Send {
    /// The node's current context, KV precision and the sizes it offers.
    fn report(&mut self, node: Option<&Node>) -> Result<ContextReport, String>;
    /// Switch the node to `tokens`. Returns the report after the switch, or
    /// why it did not happen (not enough VRAM, op unsupported); on `Err` the
    /// node keeps its old size.
    fn set_context(&mut self, node: Option<&Node>, tokens: u32) -> Result<ContextReport, String>;
    /// Switch the attention KV cache to "q8" or "f16" and wait for it to load.
    fn set_kv(&mut self, node: Option<&Node>, kv: &str) -> Result<ContextReport, String>;
}

/// The fleet node the chat session runs on: `GET/POST /llm/context`.
pub struct NodeHub;

const APPLY_PATIENCE: Duration = Duration::from_secs(90);

const NO_OP: &str = "this node cannot change its context size or KV precision";

impl NodeHub {
    fn read<T: FleetTransport>(transport: &mut T, node: &Node) -> Result<Value, String> {
        transport.get_json(&format!("{}/llm/context", node.base)).map_err(|e| {
            if e.contains("404") { NO_OP.to_string() } else { e }
        })
    }

    /// Applies at the next LLM job: send a one-token one so it happens now.
    fn start_apply<T: FleetTransport>(transport: &mut T, node: &Node) -> Result<Option<String>, String> {
        let Some(model) = &node.model else { return Ok(None) };
        let reply = transport.post_json(
            &format!("{}/generate", node.base),
            &json::obj(vec![
                ("model", json::s(model.as_str())),
                ("domain", json::s("chat")),
                ("chat_system", json::s("")),
                (
                    "chat_messages",
                    Value::Arr(vec![json::obj(vec![("role", json::s("user")), ("text", json::s("."))])]),
                ),
                ("max_tokens", Value::Int(1)),
                ("temperature", Value::F64(0.0)),
                ("seed", Value::Int(1)),
            ]),
        )?;
        Ok(reply.get("job_id").and_then(Value::as_str).map(str::to_owned))
    }

    /// Prefer the load job's terminal verdict; older nodes may only report the loaded settings.
    fn apply<T: FleetTransport>(
        transport: &mut T, node: &Node, setting: (&str, Value), what: &str,
        applied: impl Fn(&Value) -> bool,
    ) -> Result<ContextReport, String> {
        let reply = transport
            .post_json(&format!("{}/llm/context", node.base), &json::obj(vec![
                setting,
                ("apply", Value::Bool(true)),
            ]))
            .map_err(|e| format!("the node refused: {e}"))?;
        let job_id = match reply.get("job_id").and_then(Value::as_str) {
            Some(id) => Some(id.to_string()),
            None => Self::start_apply(transport, node)?,
        };
        let started = Instant::now();
        loop {
            if let Some(id) = &job_id {
                let status = transport.get_json(&format!("{}/job/{id}", node.base))?;
                if let Some(result) = apply_job_outcome(&status) {
                    result?;
                    return Ok(report_from(&Self::read(transport, node)?));
                }
            } else {
                let v = Self::read(transport, node)?;
                if applied(&v) {
                    return Ok(report_from(&v));
                }
            }
            if started.elapsed() > APPLY_PATIENCE {
                return Err(format!("the node did not apply the new {what} in time"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn set_kv_with<T: FleetTransport>(transport: &mut T, node: &Node, kv: &str) -> Result<ContextReport, String> {
        if !matches!(kv, "q8" | "f16") {
            return Err(format!("unknown KV precision {kv}; expected q8 or f16"));
        }
        let now = Self::read(transport, node)?;
        if let Some(options) = now.get("kv_options").and_then(Value::as_arr) {
            if !options.iter().any(|option| option.as_str() == Some(kv)) {
                return Err(format!("this node does not support {kv} KV precision"));
            }
        }
        Self::apply(transport, node, ("kv", json::s(kv)), "KV precision", |v| {
            v.get("kv_loaded").and_then(Value::as_str) == Some(kv)
        })
    }
}

/// Build the report from the node's `/llm/context` answer.
pub fn report_from(v: &Value) -> ContextReport {
    let n = |key| v.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
    let fit = n("fit");
    let free = v.get("vram_free_mb").and_then(Value::as_u64);
    let bytes_per_token = v.get("bytes_per_token").and_then(Value::as_u64).filter(|&b| b > 0);
    let current = n("loaded").or(n("configured")).or(fit).unwrap_or(0);
    let kv = v.get("kv_loaded").and_then(Value::as_str).or_else(|| v.get("kv").and_then(Value::as_str));
    let min = n("min").unwrap_or(makepad_ai_hub::llm_context::MIN_CONTEXT);
    let options = makepad_ai_hub::gpu_yield::context_options(n("model_max"), fit, min)
        .into_iter()
        .map(|tokens| ContextOption {
            tokens,
            vram_mb: bytes_per_token.map(|b| (u64::from(tokens).saturating_mul(b) / (1 << 20)) as u32),
            fits: match fit {
                Some(0) => false,
                Some(f) => tokens <= f,
                None => free.is_none(),
            },
        })
        .collect();
    ContextReport { current, kv: kv.map(String::from), max_fit: fit.unwrap_or(0), options }
}

fn apply_job_outcome(status: &Value) -> Option<Result<(), String>> {
    match status.get("state").and_then(Value::as_str)? {
        "done" => Some(Ok(())),
        "error" => Some(Err(status.get("error").and_then(Value::as_str).unwrap_or("context load failed").to_string())),
        "cancelled" => Some(Err("context load cancelled".into())),
        _ => None,
    }
}

impl ContextHub for NodeHub {
    fn report(&mut self, node: Option<&Node>) -> Result<ContextReport, String> {
        let node = node.ok_or("no node elected yet")?;
        Ok(report_from(&Self::read(&mut HttpFleetTransport, node)?))
    }

    fn set_context(&mut self, node: Option<&Node>, tokens: u32) -> Result<ContextReport, String> {
        let node = node.ok_or("no node elected yet")?;
        Self::apply(&mut HttpFleetTransport, node, ("context", Value::Int(i64::from(tokens))), "size", |v| {
            let loaded = v.get("loaded").and_then(Value::as_u64).filter(|&n| n > 0);
            let fit = v.get("fit").and_then(Value::as_u64);
            loaded.is_some() && (loaded == Some(u64::from(tokens)) || loaded == fit)
        })
    }

    fn set_kv(&mut self, node: Option<&Node>, kv: &str) -> Result<ContextReport, String> {
        let node = node.ok_or("no node elected yet")?;
        Self::set_kv_with(&mut HttpFleetTransport, node, kv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_from_uses_node_model_max_fit_and_bytes_per_token_up_to_262k() {
        let report = |json: &[u8]| report_from(&json::parse(json).unwrap());
        let full = report(br#"{"configured":null,"loaded":262144,"fit":262144,"model_max":262144,"min":2048,"vram_free_mb":180000,"bytes_per_token":18432,"kv":"q8","kv_loaded":"q8"}"#);
        assert_eq!((full.current, full.max_fit), (262_144, 262_144));
        assert_eq!(full.options.last(), Some(&ContextOption { tokens: 262_144, vram_mb: Some(4608), fits: true }));

        let limited = report(br#"{"loaded":114688,"fit":114688,"model_max":262144,"min":2048,"vram_free_mb":1700,"bytes_per_token":38016}"#);
        assert_eq!((limited.current, limited.max_fit), (114_688, 114_688));
        assert!(limited.options.iter().any(|o| o.tokens == 114_688 && o.fits));
        assert!(limited.options.iter().any(|o| o.tokens == 131_072 && !o.fits));

        for (free, available) in [("4000", false), ("null", true)] {
            let input = format!(r#"{{"loaded":null,"configured":null,"fit":null,"model_max":262144,"min":2048,"vram_free_mb":{free},"bytes_per_token":38016}}"#);
            let unknown = report(input.as_bytes());
            assert_eq!((unknown.current, unknown.max_fit), (0, 0));
            assert!(!unknown.options.is_empty());
            assert!(unknown.options.iter().all(|o| o.fits == available));
        }

        let cold = report(br#"{"loaded":null,"configured":null,"fit":262144,"model_max":262144,"vram_free_mb":180000,"bytes_per_token":18432}"#);
        assert_eq!((cold.current, cold.max_fit), (262_144, 262_144));
    }

    #[test]
    fn unloaded_node_uses_configured_and_clamps_to_model_max() {
        let v = json::parse(br#"{"configured":32768,"loaded":null,"fit":16384,"model_max":32768,"kv":"f16","kv_loaded":null}"#).unwrap();
        let r = report_from(&v);
        assert_eq!((r.current, r.max_fit), (32_768, 16_384));
        assert_eq!(r.kv.as_deref(), Some("f16"));
        assert_eq!(r.options.iter().map(|o| o.tokens).collect::<Vec<_>>(), [16_384, 32_768]);
        assert!(r.options.iter().all(|o| o.vram_mb.is_none()));
    }

    #[test]
    fn report_uses_loaded_kv_and_reported_memory_cost() {
        let v = json::parse(br#"{"loaded":16384,"model_max":16384,"kv":"f16","kv_loaded":"q8","bytes_per_token":38016}"#).unwrap();
        let r = report_from(&v);
        assert_eq!(r.kv.as_deref(), Some("q8"));
        assert_eq!(r.options[0].vram_mb, Some(594));
    }

    #[test]
    fn context_apply_reports_terminal_job_outcomes() {
        let outcome = |s: &[u8]| apply_job_outcome(&json::parse(s).unwrap());
        assert_eq!(outcome(br#"{"state":"done"}"#), Some(Ok(())));
        assert_eq!(outcome(br#"{"state":"error","error":"load refused"}"#), Some(Err("load refused".into())));
        assert_eq!(outcome(br#"{"state":"cancelled"}"#), Some(Err("context load cancelled".into())));
        assert_eq!(outcome(br#"{"state":"running"}"#), None);
    }

    #[test]
    fn set_kv_applies_via_job_and_surfaces_errors() {
        use std::collections::VecDeque;

        struct Transport(VecDeque<(&'static str, Result<Value, String>)>);
        impl Transport {
            fn next(&mut self, method: &str, url: &str) -> Result<Value, String> {
                let (expected, result) = self.0.pop_front().expect("unexpected request");
                assert_eq!(format!("{method} {url}"), expected);
                result
            }
        }
        impl FleetTransport for Transport {
            fn get_json(&mut self, url: &str) -> Result<Value, String> { self.next("GET", url) }
            fn post_json(&mut self, url: &str, body: &Value) -> Result<Value, String> {
                if url == "/llm/context" {
                    assert_eq!(body.get("kv").and_then(Value::as_str), Some("f16"));
                    assert_eq!(body.get("apply").and_then(Value::as_bool), Some(true));
                }
                self.next("POST", url)
            }
        }

        let node = Node { base: String::new(), model: None };
        let read_options = ("GET /llm/context", Ok(json::obj(vec![("kv_options", Value::Arr(vec![json::s("f16")]))])));
        let load = ("POST /llm/context", Ok(json::obj(vec![("job_id", json::s("load"))])));
        let read_loaded = ("GET /llm/context", Ok(json::obj(vec![("kv_loaded", json::s("f16"))])));
        let mut success = Transport(VecDeque::from([
            read_options.clone(), load.clone(), ("GET /job/load", Ok(json::obj(vec![("state", json::s("done"))]))), read_loaded.clone(),
        ]));
        assert_eq!(NodeHub::set_kv_with(&mut success, &node, "f16").unwrap().kv.as_deref(), Some("f16"));
        assert!(success.0.is_empty());

        let mut failed = Transport(VecDeque::from([
            read_options.clone(), load,
            ("GET /job/load", Ok(json::obj(vec![("state", json::s("error")), ("error", json::s("load refused"))]))),
        ]));
        assert_eq!(NodeHub::set_kv_with(&mut failed, &node, "f16").unwrap_err(), "load refused");
        assert!(failed.0.is_empty());

        let no_job = ("POST /llm/context", Ok(json::obj(vec![])));
        let mut legacy = Transport(VecDeque::from([read_options.clone(), no_job.clone(), read_loaded]));
        assert_eq!(NodeHub::set_kv_with(&mut legacy, &node, "f16").unwrap().kv.as_deref(), Some("f16"));
        assert!(legacy.0.is_empty());

        let node = Node { model: Some("model".into()), ..node };
        let mut fallback = Transport(VecDeque::from([read_options, no_job, ("POST /generate", Err("apply refused".into()))]));
        assert_eq!(NodeHub::set_kv_with(&mut fallback, &node, "f16").unwrap_err(), "apply refused");
        assert!(fallback.0.is_empty());
    }
}
