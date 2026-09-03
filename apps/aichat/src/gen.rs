//! The assistant's own generative service: `gen.image{prompt}`.
//!
//! The hub's pipelines are not an app, so no app registers them; the
//! panel registers this service in-process beside whatever apps join.
//! An `image` call runs the creator pipeline on a worker thread (the
//! runner is blocking: node pick over the LAN fleet, request, poll,
//! fetch), streams the node's progress into the card, writes the picture
//! under the makepad home's `gen` folder and answers with the path — the
//! model then hands that path to `photos.add`, which puts it on the wall.
//! Nothing goes through the asset store. Without the `engine` feature (the
//! web page) the service still exists and says it cannot.

use makepad_ai_services::engine::ServiceRegistry;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_widgets::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

/// The service id on the bus.
pub const SERVICE_ID: &str = "gen";

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        SERVICE_ID,
        "Generate",
        "The machine's generative pipelines, on the fleet's GPU nodes: a \
         picture from a prompt. The picture is saved on this machine under \
         the makepad home's gen folder and the answer carries its path. To \
         SHOW it, call photos.add with that path (launch Photos first with \
         os.launch if it is not running) — the wall then glides onto it. A \
         generation takes half a minute on a warm node, longer when a node \
         must load the model.",
    )
    .with_tool(ToolDef::new(
        "image",
        "Generate one picture from a text prompt on a fleet image node; saves it under the makepad home's gen folder and returns the path.",
        r#"{"type":"object","properties":{"prompt":{"type":"string","description":"what the picture shows, in plain words"},"width":{"type":"integer","description":"pixels, optional (default 1024)"},"height":{"type":"integer","description":"pixels, optional (default 1024)"}},"required":["prompt"]}"#,
        Risk::Act,
    ))
}

/// What the worker reports back.
enum GenMsg {
    Progress(String, u16),
    Done(Result<GenDone, String>),
}

struct GenDone {
    path: PathBuf,
    node: String,
}

struct Job {
    call_id: String,
    cancel: Arc<AtomicBool>,
    rx: Receiver<GenMsg>,
}

/// The in-process port plus the jobs in flight.
pub struct GenService {
    port: AiServicePort,
    jobs: Vec<Job>,
}

impl GenService {
    /// Open the service and register it in the panel's registry. `None`
    /// only when the manifest does not validate (a programming error).
    pub fn open(registry: &ServiceRegistry) -> Option<GenService> {
        let (port, link) = AiServicePort::in_process(manifest()).ok()?;
        registry.register(link, "built in", None).ok()?;
        Some(GenService { port, jobs: Vec::new() })
    }

    /// Drain the port and the workers; called on every panel event.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        for ev in self.port.handle_event(cx, event) {
            match ev {
                PortEvent::Call(call) => self.start(call),
                PortEvent::Cancel { call_id } => {
                    if let Some(job) = self.jobs.iter().find(|j| j.call_id == call_id) {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                }
                PortEvent::Registered(_)
                | PortEvent::ChatOpen { .. }
                | PortEvent::Subscribe { .. }
                | PortEvent::Unsubscribe { .. } => {}
            }
        }
        self.poll();
    }

    fn start(&mut self, call: ServiceCall) {
        let id = call.call_id.clone();
        if call.tool != "image" {
            self.port.reply(ToolResult::refused(&id, format!("gen has no tool `{}`; it has image", call.tool)));
            return;
        }
        let args = match parse_image_args(&call.args) {
            Ok(a) => a,
            Err(why) => {
                self.port.reply(ToolResult::refused(&id, why));
                return;
            }
        };
        #[cfg(not(feature = "engine"))]
        {
            let _ = args;
            self.port.reply(ToolResult::unavailable(&id, "this build has no pipeline runtime; pictures need the native app"));
        }
        #[cfg(feature = "engine")]
        {
            use makepad_widgets::makepad_platform::thread::SignalToUI;
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = std::sync::mpsc::channel();
            let worker_cancel = cancel.clone();
            let spawned = std::thread::Builder::new().name("gen-image".into()).spawn(move || {
                let progress_tx = tx.clone();
                let mut progress = |note: &str, permille: u16| {
                    let _ = progress_tx.send(GenMsg::Progress(note.to_string(), permille));
                    SignalToUI::set_ui_signal();
                };
                let result = run_image(&args, &worker_cancel, &mut progress);
                let _ = tx.send(GenMsg::Done(result));
                SignalToUI::set_ui_signal();
            });
            match spawned {
                Ok(_) => self.jobs.push(Job { call_id: id, cancel, rx }),
                Err(e) => self.port.reply(ToolResult::failed(&id, format!("could not start the generation: {e}"))),
            }
        }
    }

    fn poll(&mut self) {
        let mut done: Vec<usize> = Vec::new();
        for (i, job) in self.jobs.iter().enumerate() {
            loop {
                match job.rx.try_recv() {
                    Ok(GenMsg::Progress(note, permille)) => self.port.progress(&job.call_id, &note, permille),
                    Ok(GenMsg::Done(Ok(out))) => {
                        let path = out.path.to_string_lossy().to_string();
                        self.port.reply(
                            ToolResult::ok(
                                &job.call_id,
                                format!("saved {path} (made on {}). Show it on the wall with photos.add {{\"path\":\"{path}\"}}.", out.node),
                                "saved",
                            )
                            .with_data(format!("{{\"path\":{},\"node\":{}}}", json_string(&path), json_string(&out.node))),
                        );
                        done.push(i);
                        break;
                    }
                    Ok(GenMsg::Done(Err(e))) => {
                        let result = if e == "cancelled" { ToolResult::cancelled(&job.call_id) } else { ToolResult::failed(&job.call_id, e) };
                        self.port.reply(result);
                        done.push(i);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.port.reply(ToolResult::failed(&job.call_id, "the generation worker died"));
                        done.push(i);
                        break;
                    }
                }
            }
        }
        for i in done.into_iter().rev() {
            self.jobs.remove(i);
        }
    }
}

/// The arguments of one `image` call, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageArgs {
    pub prompt: String,
    pub width: u32,
    pub height: u32,
}

/// Sizes are clamped to what the image nodes serve; a prompt is required.
pub fn parse_image_args(args: &str) -> Result<ImageArgs, String> {
    use makepad_strict_json as json;
    let fields = match json::parse(args.as_bytes()) {
        Ok(json::Value::Obj(fields)) => fields,
        _ => return Err("image needs a JSON object with a `prompt`".to_string()),
    };
    let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let prompt = get("prompt").and_then(|v| v.as_str().map(|s| s.trim().to_string())).unwrap_or_default();
    if prompt.is_empty() {
        return Err("image needs a `prompt`".to_string());
    }
    if prompt.len() > 4000 {
        return Err("the prompt is longer than 4000 bytes".to_string());
    }
    let dim = |key: &str| -> Result<u32, String> {
        match get(key) {
            None => Ok(1024),
            Some(json::Value::Int(i)) if (256..=2048).contains(&i) => Ok(i as u32),
            Some(_) => Err(format!("`{key}` must be an integer from 256 to 2048")),
        }
    };
    Ok(ImageArgs { prompt, width: dim("width")?, height: dim("height")? })
}

/// A short file-name-safe slug of the prompt.
pub fn slug(prompt: &str) -> String {
    let mut out = String::new();
    for ch in prompt.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "picture".to_string() } else { out }
}

/// The file extension for a content type the nodes produce.
pub fn extension_for(content_type: &str) -> &'static str {
    match content_type.split(';').next().unwrap_or("").trim() {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

fn json_string(s: &str) -> String {
    makepad_strict_json::s(s.to_string()).to_json()
}

#[cfg(feature = "engine")]
fn run_image(args: &ImageArgs, cancel: &Arc<AtomicBool>, progress: &mut dyn FnMut(&str, u16)) -> Result<GenDone, String> {
    use makepad_asset_creator::makepad_ai_hub::home::makepad_home;
    use makepad_asset_creator::makepad_strict_json as json;
    use makepad_asset_creator::runner::generate_bytes;
    let body = json::obj(vec![
        ("prompt", json::s(args.prompt.clone())),
        ("width", json::Value::Int(args.width as i64)),
        ("height", json::Value::Int(args.height as i64)),
    ]);
    let seed = (Cx::time_now().max(0.0) * 1_000_000_000.0) as u64;
    progress("finding an image node", 0);
    let generated = generate_bytes("image.generate", &body, seed, cancel, progress)?;
    let artifact = generated.artifact.ok_or("the node returned no picture")?;
    let dir = makepad_home().join("gen");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let stamp = Cx::time_now().max(0.0) as u64;
    let path = dir.join(format!("{stamp}-{}.{}", slug(&args.prompt), extension_for(&artifact.content_type)));
    std::fs::write(&path, &artifact.bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(GenDone { path, node: generated.node })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_validates_and_the_tool_acts() {
        let m = manifest();
        m.validate().expect("a manifest the wire accepts");
        assert_eq!(m.id, "gen");
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].name, "image");
        assert_eq!(m.tools[0].risk, Risk::Act);
    }

    #[test]
    fn image_args_need_a_prompt_and_sane_sizes() {
        let a = parse_image_args(r#"{"prompt":"a red bicycle on the moon"}"#).unwrap();
        assert_eq!((a.width, a.height), (1024, 1024));
        let b = parse_image_args(r#"{"prompt":"x","width":512,"height":768}"#).unwrap();
        assert_eq!((b.width, b.height), (512, 768));
        assert!(parse_image_args(r#"{"width":512}"#).unwrap_err().contains("prompt"));
        assert!(parse_image_args(r#"{"prompt":"x","width":16}"#).unwrap_err().contains("256"));
        assert!(parse_image_args("nope").is_err());
    }

    #[test]
    fn file_names_are_slugs_with_the_right_extension() {
        assert_eq!(slug("A red bicycle, on the Moon!"), "a-red-bicycle-on-the-moon");
        assert_eq!(slug("   "), "picture");
        assert!(slug(&"word ".repeat(30)).len() <= 41);
        assert_eq!(extension_for("image/png"), "png");
        assert_eq!(extension_for("image/jpeg; charset=binary"), "jpg");
        assert_eq!(extension_for("application/octet-stream"), "png");
    }
}
