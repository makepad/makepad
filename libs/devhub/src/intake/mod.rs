//! Hostile intake: GitHub issues and pull requests pass through a jail on a Linux test box before
//! anything with power sees them (docs/agents/intake.md). The jail fetches, scans, runs the Qwen
//! prefilter, has a whitepaper agent reproduce the item and gates what crosses back; the master
//! stores enums and an opaque whitepaper, and `list` shows numbers, enums and counts only.

pub mod eval;
pub mod gate;
pub mod github;
pub mod hub;
pub mod item;
pub mod jail;
pub mod paper;
pub mod prefilter;
pub mod protect;
pub mod proxy;
pub mod store;
pub mod text;

pub const USAGE: &str = "usage: makepad devhub intake <command>
  eval --hub https://<gpu node>:8123 [--model M] [--corpus <file>] [--only <family>] [--out <file.tsv>]
                                   the prefilter on our own corpus: recall and false positives per family
  jail cycle --conf <path>         (in the jail, uid intake) fetch, prefilter, whitepaper, gate, outbox
  jail proxy --conf <path>         (in the jail, uid intake-net) the allowlisting proxy
  jail canary --conf <path>        (in the jail) our hostile fixture through the real stages; checks the outbox
  pull                             (the master) the jail's finished items into the store
  list                             the store: numbers and enums only
  reply <item> fixed <commit> | not-reproducible | declined
                                   one of the three fixed replies, for the user to post
  credit <item>                    the Intake and Co-Authored-By trailers for a clean-room fix
  approve <lane>                   the user, at a terminal: approve the protected files a lane changes
  reviewed <lane> --by <reviewer> --report <file>
                                   record the adversarial review of an intake lane's t2 files
  route <relax#N|makepad#N> <lane> the lane works on an intake item (marked for good)
  replay [--days N] [--branch work]
                                   the land rule over past landings: approvals it would have asked for";

/// The jail's configuration (`/etc/intake/jail.conf`, `key=value` lines): where the proxy listens
/// and the two hubs it forwards to. Unknown keys are refused, so a typo fails at start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conf {
    /// `127.0.0.1:<port>`: the proxy's loopback address
    pub proxy: String,
    /// `https://<gpu node>:<port>`: the Qwen prefilter's hub
    pub qwen_hub: String,
    /// `https://<admin host>:<port>`: the hub that serves the model API
    pub model_hub: String,
    /// the read-only toolchain the whitepaper agent's shell sees
    pub toolchain: String,
    /// where finished items wait for the master to pull them
    pub outbox: String,
}

impl Conf {
    pub fn parse(text: &str) -> Result<Conf, String> {
        let mut c = Conf::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let (k, v) = line.split_once('=').ok_or_else(|| format!("jail.conf: `{line}` is not key=value"))?;
            let slot = match k.trim() {
                "proxy" => &mut c.proxy,
                "qwen_hub" => &mut c.qwen_hub,
                "model_hub" => &mut c.model_hub,
                "toolchain" => &mut c.toolchain,
                "outbox" => &mut c.outbox,
                other => return Err(format!("jail.conf: unknown key {other}")),
            };
            *slot = v.trim().to_string();
        }
        let loopback = c.proxy.strip_prefix("127.0.0.1:").is_some_and(|p| p.parse::<u16>().is_ok());
        if !loopback {
            return Err("jail.conf: proxy must be 127.0.0.1:<port>".into());
        }
        hub::host_port(&c.qwen_hub)?;
        hub::host_port(&c.model_hub)?;
        Ok(c)
    }

    pub fn read(path: &str) -> Result<Conf, String> {
        Conf::parse(&std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?)
    }
}

/// Lowercase hex sha256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    makepad_network::digest::sha256_hash(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `makepad devhub intake ...`
pub fn cli(args: &[&str]) -> i32 {
    match args.first().copied() {
        Some("jail") if args.get(1) == Some(&"proxy") => proxy::cli(&args[2..]),
        Some("eval") => eval::cli(&args[1..]),
        Some("jail") if args.get(1) == Some(&"cycle") => jail::cycle(&args[2..]),
        Some("jail") if args.get(1) == Some(&"canary") => jail::canary(&args[2..]),
        Some("pull") => store::pull(&args[1..]),
        Some("list") => store::list(&args[1..]),
        Some("reply") => store::reply(&args[1..]),
        Some("credit") => store::credit(&args[1..]),
        Some("approve") => protect::approve(&args[1..]),
        Some("reviewed") => protect::reviewed(&args[1..]),
        Some("route") => protect::route(&args[1..]),
        Some("replay") => protect::replay(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use makepad_strict_json::{self as json, Value};

    /// docs/agents/intake.md's invariant, and what makes it true on this side: the prefilter's one
    /// request shape carries no tools and no other ability, and the jail's proxy forwards nothing
    /// but the model routes (the hub's intake scope is tested in libs/ai/hub).
    #[test]
    fn the_invariant_holds() {
        let doc = include_str!("../../../../docs/agents/intake.md");
        let invariant = "**Invariant: a local AI hub's Qwen 3.8, which has zero abilities, acts as a prefilter, so text from\nGitHub never goes plainly into an agent with power.**";
        assert!(doc.lines().take(5).collect::<Vec<_>>().join("\n").contains(invariant), "the invariant opens docs/agents/intake.md");
        let body = super::hub::prefilter_body("qwen3.8-27b", "s", "u", 100, 1).to_json();
        let v = json::parse(body.as_bytes()).unwrap();
        let Value::Obj(fields) = &v else { panic!("an object") };
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["model", "domain", "chat_system", "chat_messages", "max_tokens", "temperature", "thinking", "seed"], "the prefilter asks for text and nothing else");
        assert!(!body.contains("tool") && !body.contains("http") && v.get("domain").and_then(Value::as_str) == Some("chat"), "no tools, no URLs, the chat domain");
        use super::proxy::{route, Route};
        for (m, t) in [("GET", "/v1/models"), ("POST", "/v1/files"), ("POST", "/v1/messages/batches"), ("POST", "/v1/complete"), ("GET", "/v1/messages"), ("POST", "/pull"), ("GET", "/artifact/x"), ("POST", "/job/x/children")] {
            assert!(route(m, t).is_none(), "the proxy refuses {m} {t}");
        }
        assert!(matches!(route("POST", "/generate"), Some(Route::Qwen { .. })) && matches!(route("POST", "/v1/messages"), Some(Route::Model)), "the two model routes pass");
    }
}
