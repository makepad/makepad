//! The vision judge on the AI fleet: one grab and one acceptance text go to the vision model on the
//! fastest hub that has it loaded ([`judges`]), and the answer comes back with its
//! `VERDICT: YES|NO`.
//! The model stays loaded on that hub between grabs. A hub that fails is left out and the next one
//! asked. Was tools/ci's in-process judge (the model on the CI box itself).

use makepad_ai_hub::client::{ContentProvider, LocalService};
use makepad_ai_hub::fleet::{self, BoxSnapshot};
use makepad_ai_hub::job::{fleet_snapshots, generate, process_origin, FleetRouter, GenPick, GenRouter};
use makepad_ai_hub::protocol::GenerateRequestJson;
use makepad_ai_hub::registry::Domain;
use makepad_strict_json::{self as json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The CI box's model (its ci.toml `model`).
pub const MODEL: &str = "qwen3.5-4b-vision";
/// The answer's budget: a bullet per numbered point, then the verdict line.
const MAX_TOKENS: u32 = 400;
/// Hubs asked for one grab before the judge gives up.
const ATTEMPTS: usize = 3;
/// With no hub able to take it now (they update and restart with landings), a grab waits this long.
const NO_HUB_WAIT: Duration = Duration::from_secs(180);
/// How long a box tries to reach any hub before it says it cannot.
const FLEET_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct Verdict {
    pub yes: bool,
    pub raw: String,
    pub secs: f64,
    /// the hub that answered
    pub hub: String,
    pub parse_error: bool,
}

impl Verdict {
    pub fn json(&self) -> Value {
        json::obj(vec![("yes", Value::Bool(self.yes)), ("raw", json::s(&self.raw)), ("seconds", Value::F64(self.secs)), ("hub", json::s(&self.hub)), ("parse_error", Value::Bool(self.parse_error))])
    }
}

/// The last `VERDICT:` line of an answer (tools/ci uihub.rs `parse_verdict`).
pub fn parse_verdict(raw: &str) -> Result<bool, String> {
    match raw.lines().filter_map(|line| line.trim().strip_prefix("VERDICT:")).last().map(str::trim) {
        Some("YES") => Ok(true),
        Some("NO") => Ok(false),
        Some(v) => Err(format!("invalid final VERDICT: {v}")),
        None => Err("missing VERDICT: YES or VERDICT: NO".into()),
    }
}

/// The fleet router minus the hubs that failed this judge.
struct Avoiding {
    failed: Arc<Mutex<Vec<String>>>,
    /// the hub the last route picked
    picked: Arc<Mutex<Option<String>>>,
}

impl GenRouter for Avoiding {
    fn use_idle_admission(&self, domain: &str, request: &GenerateRequestJson) -> bool {
        FleetRouter::default().use_idle_admission(domain, request)
    }

    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        let mut all = excluded.to_vec();
        all.extend(self.failed.lock().unwrap().iter().cloned());
        let pick = FleetRouter { installed_only: false }.route_snapshots(judges(fleet_snapshots(&all)), domain, request)?;
        *self.picked.lock().unwrap() = Some(pick.base_url.clone());
        Ok(pick)
    }
}

/// The hubs a grab goes to: those with the model loaded, else those with it ready (installed),
/// else every hub; of them, the fastest GPU's (the fleet's speed table: an RTX PRO 6000, then a
/// 5090, ...). A model is never loaded just to pick a box while another hub has it loaded.
pub fn judges(snapshots: Vec<BoxSnapshot>) -> Vec<BoxSnapshot> {
    let state = |s: &BoxSnapshot| fleet::affinity(s, MODEL);
    let pool: Vec<BoxSnapshot> = [4, 3]
        .iter()
        .map(|want| snapshots.iter().filter(|s| state(s) == Some(*want)).cloned().collect::<Vec<_>>())
        .find(|v| !v.is_empty())
        .unwrap_or(snapshots);
    let best = pool.iter().map(fleet::gpu_rank).max().unwrap_or(0);
    pool.into_iter().filter(|s| fleet::gpu_rank(s) == best).collect()
}

pub struct Judge {
    failed: Arc<Mutex<Vec<String>>>,
    /// no fleet: every judgement says so (a failed step, not a pass)
    pub off: Option<String>,
}

impl Judge {
    /// Starts fleet discovery (hubs announce every 2 s; a first route waits for them). This process
    /// may use the LAN: the lease marks its runs as test instances, which keep it off.
    pub fn start() -> Judge {
        // SAFETY: set before any thread of this process reads the environment (main's first act)
        unsafe { std::env::set_var("MAKEPAD_LAN", "1") };
        let d = makepad_ai_hub::discovery::start_listener();
        let off = match &d {
            makepad_ai_hub::discovery::Discovery::Unavailable { reason } => Some(reason.to_string()),
            _ => None,
        };
        Judge { failed: Arc::new(Mutex::new(Vec::new())), off }
    }

    /// Waits until a hub is heard (at most `secs`).
    pub fn wait_for_hubs(&self, secs: f64) -> usize {
        let t = Instant::now();
        loop {
            let n = makepad_ai_hub::discovery::start_listener().nodes().len();
            if n > 0 || t.elapsed().as_secs_f64() >= secs {
                return n;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Why this box cannot reach the fleet at all (its OS keeps this process off the LAN), after
    /// the time a fleet-wide hub restart takes; nothing when a hub answers.
    pub fn unreachable(&self) -> Option<String> {
        if let Some(why) = &self.off {
            return Some(format!("no AI fleet ({why})"));
        }
        let t = Instant::now();
        loop {
            let nodes = makepad_ai_hub::discovery::start_listener().nodes();
            if nodes.iter().any(|n| LocalService::new(&n.base_url).health().is_ok()) {
                return None;
            }
            if t.elapsed() >= FLEET_WAIT {
                return Some(if nodes.is_empty() { format!("no AI fleet hub heard in {} s", FLEET_WAIT.as_secs()) } else { format!("no hub answers{}", health_errors()) });
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// One grab judged against `prompt`.
    pub fn judge(&self, image: &Path, prompt: &str) -> Result<Verdict, String> {
        if let Some(why) = &self.off {
            return Err(format!("vision judge: no AI fleet ({why})"));
        }
        let bytes = std::fs::read(image).map_err(|e| format!("{}: {e}", image.display()))?;
        if self.wait_for_hubs(6.0) == 0 {
            return Err("vision judge: no AI fleet hub heard in 6 s".into());
        }
        let mut last = String::new();
        let asked = Instant::now();
        let mut hubs_failed = 0;
        while hubs_failed < ATTEMPTS {
            let request = GenerateRequestJson {
                model: MODEL.into(),
                prompt: Some(prompt.to_string()),
                input_b64: Some(makepad_network::digest::base64_encode(&bytes)),
                input_content_type: Some("image/png".into()),
                max_tokens: Some(MAX_TOKENS),
                ..Default::default()
            };
            let t = Instant::now();
            let picked = Arc::new(Mutex::new(None));
            let router: Arc<dyn GenRouter> = Arc::new(Avoiding { failed: self.failed.clone(), picked: picked.clone() });
            match generate(router, process_origin("ci-leg"), Domain::Vision, request, makepad_ai_hub::job::Want::Outputs { media: 0, text: 1 }, &|| false, &mut |_, _| {}, Duration::from_millis(200)) {
                Ok(out) => {
                    let raw = out.text.unwrap_or_default();
                    let (yes, parse_error) = match parse_verdict(&raw) {
                        Ok(y) => (y, false),
                        Err(_) => (false, true),
                    };
                    return Ok(Verdict { yes, raw, secs: t.elapsed().as_secs_f64(), hub: out.node.clone(), parse_error });
                }
                Err(e) => {
                    last = e.to_string();
                    // the hub it went to is left out next time (a dead or failing hub); no pick: no hub
                    match picked.lock().unwrap().take() {
                        Some(hub) => {
                            last = format!("{hub}: {last}");
                            self.failed.lock().unwrap().push(hub);
                            hubs_failed += 1;
                        }
                        None if asked.elapsed() >= NO_HUB_WAIT => return Err(format!("vision judge: {last}{}", health_errors())),
                        None => std::thread::sleep(Duration::from_secs(10)),
                    }
                }
            }
        }
        Err(format!("vision judge: {last}"))
    }
}

/// Why each hub heard does not answer `/health` (what the router's "unavailable" leaves out).
fn health_errors() -> String {
    let errors: Vec<String> = makepad_ai_hub::discovery::start_listener().nodes().iter().filter_map(|n| LocalService::new(&n.base_url).health().err().map(|e| format!("{}: {e}", n.base_url))).collect();
    if errors.is_empty() {
        String::new()
    } else {
        format!(" (health: {})", errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_verdict_line_decides() {
        assert_eq!(parse_verdict("- a\n- b\nVERDICT: YES"), Ok(true));
        assert_eq!(parse_verdict("VERDICT: YES\nthen\n  VERDICT: NO  "), Ok(false));
        assert!(parse_verdict("no verdict").is_err());
        assert!(parse_verdict("VERDICT: MAYBE").is_err());
    }
}
