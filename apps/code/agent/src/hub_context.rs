//! The node's context-size and KV-precision ops, behind a seam so the engine
//! and its tests do not depend on a live node. The hub applies changes at
//! its next LLM job (the session is rebuilt, weights come from the page
//! cache), so a switch sends one tiny chat job to apply it now and waits
//! for the node to report the new size or precision.

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

/// KV bytes per token of the q8_0 cache on the Qwen3.8-27B node (measured
/// resident VRAM 17.8 / 18.3 / 19.5 GiB at 16k / 32k / 64k). The node
/// reports free VRAM but not a per-size figure, so the options are estimated.
const KV_KIB_PER_TOKEN: u32 = 36;
/// Left free on the GPU when deciding what fits.
const MARGIN_MB: u32 = 512;
const APPLY_PATIENCE: Duration = Duration::from_secs(90);
/// Offered when the node accepts them (`max`); 80k is the largest that fits
/// q8 + vision + the speech pins beside the WM on the 24 GB 5090 node.
const SIZES: [u32; 5] = [16384, 32768, 49152, 65536, 81920];

const NO_OP: &str = "this node cannot change its context size or KV precision";

impl NodeHub {
    fn read(node: &Node) -> Result<Value, String> {
        HttpFleetTransport.get_json(&format!("{}/llm/context", node.base)).map_err(|e| {
            if e.contains("404") { NO_OP.to_string() } else { e }
        })
    }

    /// Applies at the next LLM job: send a one-token one so it happens now.
    fn start_apply(node: &Node) {
        if let Some(model) = &node.model {
            let _ = HttpFleetTransport.post_json(
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
            );
        }
    }
}

/// Build the report from the node's `/llm/context` answer.
pub fn report_from(v: &Value) -> ContextReport {
    let n = |k: &str| v.get(k).and_then(Value::as_u64).map(|x| x as u32);
    let current = n("loaded").or(n("configured")).unwrap_or(0);
    let kv = v.get("kv_loaded").and_then(Value::as_str).or_else(|| v.get("kv").and_then(Value::as_str));
    let kv_kib_per_token = if kv == Some("f16") { 64 } else { KV_KIB_PER_TOKEN };
    let max = n("max").unwrap_or(65536);
    let min = n("min").unwrap_or(2048);
    let free = n("vram_free_mb");
    let kv_mb = |t: u32| (t as u64 * kv_kib_per_token as u64 / 1024) as u32;
    let mut options = Vec::new();
    let mut max_fit = 0;
    for &tokens in SIZES.iter().filter(|&&t| t >= min && t <= max) {
        // Growing from the current size costs only the difference.
        let extra = kv_mb(tokens).saturating_sub(kv_mb(current));
        let fits = free.is_none_or(|f| extra + MARGIN_MB <= f);
        if fits {
            max_fit = tokens;
        }
        options.push(ContextOption { tokens, vram_mb: Some(kv_mb(tokens)), fits });
    }
    ContextReport { current, kv: kv.map(String::from), max_fit, options }
}

impl ContextHub for NodeHub {
    fn report(&mut self, node: Option<&Node>) -> Result<ContextReport, String> {
        let node = node.ok_or("no node elected yet")?;
        Ok(report_from(&Self::read(node)?))
    }

    fn set_context(&mut self, node: Option<&Node>, tokens: u32) -> Result<ContextReport, String> {
        let node = node.ok_or("no node elected yet")?;
        let now = Self::read(node)?;
        let report = report_from(&now);
        if let Some(o) = report.options.iter().find(|o| o.tokens == tokens) {
            if !o.fits {
                return Err(format!("not enough free GPU memory ({} MB free)", now.get("vram_free_mb").and_then(Value::as_u64).unwrap_or(0)));
            }
        }
        HttpFleetTransport
            .post_json(&format!("{}/llm/context", node.base), &json::obj(vec![("context", Value::Int(tokens as i64))]))
            .map_err(|e| format!("the node refused: {e}"))?;
        Self::start_apply(node);
        let started = Instant::now();
        loop {
            let v = Self::read(node)?;
            if v.get("loaded").and_then(Value::as_u64) == Some(tokens as u64) {
                return Ok(report_from(&v));
            }
            if started.elapsed() > APPLY_PATIENCE {
                return Err("the node did not apply the new size in time".into());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn set_kv(&mut self, node: Option<&Node>, kv: &str) -> Result<ContextReport, String> {
        let node = node.ok_or("no node elected yet")?;
        if !matches!(kv, "q8" | "f16") {
            return Err(format!("unknown KV precision {kv}; expected q8 or f16"));
        }
        let now = Self::read(node)?;
        if let Some(options) = now.get("kv_options").and_then(Value::as_arr) {
            if !options.iter().any(|option| option.as_str() == Some(kv)) {
                return Err(format!("this node does not support {kv} KV precision"));
            }
        }
        HttpFleetTransport
            .post_json(&format!("{}/llm/context", node.base), &json::obj(vec![("kv", json::s(kv))]))
            .map_err(|e| format!("the node refused: {e}"))?;
        Self::start_apply(node);
        let started = Instant::now();
        loop {
            let v = Self::read(node)?;
            if v.get("kv_loaded").and_then(Value::as_str) == Some(kv) {
                return Ok(report_from(&v));
            }
            if started.elapsed() > APPLY_PATIENCE {
                return Err("the node did not apply the new KV precision in time".into());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_estimates_fit_from_free_vram() {
        let v = json::parse(br#"{"configured": 16384, "loaded": 16384, "max": 65536, "min": 2048, "vram_free_mb": 1200}"#).unwrap();
        let r = report_from(&v);
        assert_eq!(r.current, 16384);
        assert_eq!(r.kv, None);
        // 16k->32k costs ~576 MB, ->48k ~1152 MB, ->64k ~1728 MB; 512 MB stays free.
        assert_eq!(r.max_fit, 32768);
        assert!(!r.options.iter().find(|o| o.tokens == 65536).unwrap().fits);
        assert_eq!(r.options[0].vram_mb, Some(576));
    }

    #[test]
    fn unloaded_node_uses_configured_and_clamps_to_max() {
        let v = json::parse(br#"{"configured": 32768, "loaded": null, "max": 32768, "kv": "f16", "kv_configured": "q8", "kv_loaded": null}"#).unwrap();
        let r = report_from(&v);
        assert_eq!(r.current, 32768);
        assert_eq!(r.kv.as_deref(), Some("f16"));
        assert_eq!(r.options[0].vram_mb, Some(1024));
        assert_eq!(r.options.iter().map(|o| o.tokens).collect::<Vec<_>>(), [16384, 32768]);
        assert_eq!(r.max_fit, 32768);
    }

    #[test]
    fn report_estimates_memory_from_loaded_kv_precision() {
        let q8 = json::parse(br#"{"loaded": 16384, "vram_free_mb": 2000, "kv": "f16", "kv_loaded": "q8"}"#).unwrap();
        let f16 = json::parse(br#"{"loaded": 16384, "vram_free_mb": 2000, "kv": "q8", "kv_loaded": "f16"}"#).unwrap();
        let q8 = report_from(&q8);
        let f16 = report_from(&f16);
        assert_eq!(q8.kv.as_deref(), Some("q8"));
        assert_eq!(f16.kv.as_deref(), Some("f16"));
        assert_eq!(q8.options[0].vram_mb, Some(576));
        assert_eq!(f16.options[0].vram_mb, Some(1024));
        assert_eq!(q8.max_fit, 49152);
        assert_eq!(f16.max_fit, 32768);
    }
}
