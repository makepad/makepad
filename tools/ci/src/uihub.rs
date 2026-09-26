//! One resident language session and matched tower, owned by the CI worker.
//! Uses the hub's image preparation/tier contract. No model has remote tools.
use crate::{
    process::{Control, Result},
    report::{Notify, Update},
};
use makepad_ai_hub::{
    local::{InstallMsg, InstallState, LocalModels},
    vision_backend::{
        decode_image_rgb8, downscale_to_fit, tier_for_free_vram, vision_suffix, VISION_PREFIX,
    },
};
use makepad_ai_llm::{
    preprocess_rgb8, GgufFile, LlamaSession, LlamaSessionConfig, VisionConfig, VisionTower,
};
use makepad_strict_json::{self as json, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Verdict {
    pub yes: bool,
    pub reasons: Vec<String>,
    pub raw: String,
    pub seconds: f64,
    pub tokens_per_sec: f64,
    pub skipped: bool,
    pub parse_error: bool,
}
impl Verdict {
    pub fn json(&self) -> Value {
        json::obj(vec![
            ("yes", Value::Bool(self.yes)),
            (
                "reasons",
                Value::Arr(self.reasons.iter().map(json::s).collect()),
            ),
            ("raw", json::s(&self.raw)),
            ("seconds", Value::F64(self.seconds)),
            ("tokens_per_sec", Value::F64(self.tokens_per_sec)),
            ("skipped", Value::Bool(self.skipped)),
            ("parse_error", Value::Bool(self.parse_error)),
        ])
    }
}
pub fn parse_verdict(raw: &str) -> Result<bool> {
    match raw
        .lines()
        .filter_map(|line| line.strip_prefix("VERDICT:"))
        .last()
        .map(str::trim)
    {
        Some("YES") => Ok(true),
        Some("NO") => Ok(false),
        Some(v) => Err(format!("invalid final VERDICT: {v}")),
        None => Err("missing VERDICT: YES or VERDICT: NO".into()),
    }
}

struct Loaded {
    session: LlamaSession,
    tower: VisionTower,
    config: VisionConfig,
}
pub struct UiHub {
    pub model: String,
    paths: Option<(PathBuf, PathBuf)>,
    loaded: Option<Loaded>,
    load_error: Option<String>,
    disabled: bool,
}
impl UiHub {
    pub fn new(model: &str, disabled: bool) -> Self {
        Self {
            model: model.into(),
            paths: None,
            loaded: None,
            load_error: None,
            disabled,
        }
    }
    fn ensure(&mut self, control: &Control, notify: &Notify) -> Result<bool> {
        control.check()?;
        if self.disabled {
            notify(Update::Model("skipped (--no-vision)".into()));
            return Ok(false);
        }
        if let Some(e) = &self.load_error {
            return Err(e.clone());
        }
        if self.loaded.is_some() {
            return Ok(true);
        }
        let result: Result<bool> = (|| {
            let local = LocalModels::open().map_err(|e| e.to_string())?;
            let spec = local
                .spec(&self.model)
                .ok_or_else(|| format!("unknown model {}", self.model))?;
            makepad_ai_hub::vision_backend::spec_is_servable(spec)?;
            if !matches!(local.install_state(&self.model), InstallState::Installed) {
                notify(Update::Model(format!(
                    "{}: skipped (model not installed)",
                    self.model
                )));
                return Ok(false);
            }
            let gguf = local
                .installed_path(&self.model, "llm-gguf")
                .ok_or("missing llm-gguf role")?;
            let mmproj = local
                .installed_path(&self.model, "mmproj")
                .ok_or("missing mmproj role")?;
            self.paths = Some((gguf.clone(), mmproj.clone()));
            notify(Update::Model(format!(
                "{}: loading vision tower",
                self.model
            )));
            let path = mmproj.to_string_lossy();
            let file = GgufFile::open(&mmproj).map_err(|e| format!("open mmproj: {e:?}"))?;
            let config =
                VisionConfig::from_gguf(&file).map_err(|e| format!("vision config: {e:?}"))?;
            drop(file);
            let tower = VisionTower::load(&path).map_err(|e| format!("load tower: {e:?}"))?;
            control.check()?;
            let session = LlamaSession::load_with_progress(
                &gguf,
                LlamaSessionConfig {
                    max_context: Some(tier_for_free_vram(None).max_context),
                    ..Default::default()
                },
                &mut |stage, frac| {
                    notify(Update::Model(format!(
                        "{}: {stage} {:.0}%",
                        self.model,
                        frac * 100.0
                    )))
                },
            )
            .map_err(|e| format!("load language model: {e:?}"))?;
            control.check()?;
            notify(Update::Model(format!(
                "{}: ready — {} — {}",
                self.model,
                tower.device_description(),
                session.device_name()
            )));
            if let Some(bytes) = peak_rss_bytes() {
                notify(Update::Log(format!(
                    "model resident process peak RSS: {bytes} bytes"
                )));
            }
            self.loaded = Some(Loaded {
                session,
                tower,
                config,
            });
            Ok(true)
        })();
        if let Err(e) = &result {
            self.load_error = Some(e.clone());
        }
        result
    }
    pub fn judge(
        &mut self,
        png: &Path,
        prompt: &str,
        control: &Control,
        notify: &Notify,
    ) -> Result<Verdict> {
        if !self.ensure(control, notify)? {
            return Ok(Verdict {
                yes: true,
                reasons: vec!["skipped (model not installed)".into()],
                raw: "skipped (model not installed)".into(),
                seconds: 0.0,
                tokens_per_sec: 0.0,
                skipped: true,
                parse_error: false,
            });
        }
        let start = Instant::now();
        let loaded = self.loaded.as_mut().unwrap();
        let bytes = std::fs::read(png).map_err(|e| e.to_string())?;
        let (rgb, w, h) = decode_image_rgb8(&bytes).map_err(|e| e.to_string())?;
        let (rgb, w, h) = downscale_to_fit(&rgb, w, h, tier_for_free_vram(None).sheet_px as usize);
        let prepared = preprocess_rgb8(&rgb, w, h, &loaded.config)
            .map_err(|e| format!("preprocess: {e:?}"))?;
        let embeddings = loaded
            .tower
            .encode(&prepared)
            .map_err(|e| format!("encode image: {e:?}"))?;
        control.check()?;
        loaded
            .session
            .reset()
            .map_err(|e| format!("reset: {e:?}"))?;
        let prefix = loaded
            .session
            .vocab()
            .tokenize(VISION_PREFIX, false, true)
            .map_err(|e| format!("tokenize: {e:?}"))?;
        let suffix = loaded
            .session
            .vocab()
            .tokenize(&vision_suffix(prompt), false, true)
            .map_err(|e| format!("tokenize: {e:?}"))?;
        loaded
            .session
            .append_tokens(&prefix)
            .map_err(|e| format!("prefill: {e:?}"))?;
        loaded
            .session
            .append_image_embeddings(&embeddings, prepared.tokens_w(), prepared.tokens_h())
            .map_err(|e| format!("image prefill: {e:?}"))?;
        loaded
            .session
            .append_tokens(&suffix)
            .map_err(|e| format!("prompt prefill: {e:?}"))?;
        let (mut raw, tokens, decode_secs) = decode(&mut loaded.session, control)?;
        let mut parsed = parse_verdict(&raw);
        if parsed.is_err() {
            let forced = force_verdict(&mut loaded.session, control)?;
            raw = format!("{raw}\n[no verdict line; asked outright]\n{forced}");
            parsed = parse_verdict(&forced);
        }
        let mut reasons: Vec<String> = raw
            .lines()
            .filter(|s| s.starts_with("- "))
            .map(str::to_string)
            .collect();
        if let Err(e) = &parsed {
            reasons.push(e.clone());
        }
        Ok(Verdict {
            yes: parsed.as_ref().copied().unwrap_or(false),
            reasons,
            raw,
            seconds: start.elapsed().as_secs_f64(),
            tokens_per_sec: tokens as f64 / decode_secs.max(0.000001),
            skipped: false,
            parse_error: parsed.is_err(),
        })
    }
    pub fn ask(&mut self, prompt: &str, control: &Control, notify: &Notify) -> Result<String> {
        if !self.ensure(control, notify)? {
            return Ok("skipped (model not installed)".into());
        }
        let session = &mut self.loaded.as_mut().unwrap().session;
        session.reset().map_err(|e| format!("reset: {e:?}"))?;
        let text = format!(
            "<|im_start|>user\n{prompt}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        let ids = session
            .vocab()
            .tokenize(&text, false, true)
            .map_err(|e| format!("tokenize: {e:?}"))?;
        session
            .append_tokens(&ids)
            .map_err(|e| format!("prefill: {e:?}"))?;
        decode(session, control).map(|(text, _, _)| text)
    }
}
/// Has the text fallen into a loop? A small greedy model, asked to describe a
/// picture, sometimes locks onto one short phrase and repeats it to the token
/// cap ("-g -g -g -g"): the tail of `text` is then one short pattern, over and
/// over. Whatever came before the loop is kept; the loop itself is not worth
/// the time it takes to finish.
pub fn is_looping(text: &str) -> bool {
    const TAIL: usize = 48;
    let bytes = text.as_bytes();
    if bytes.len() < TAIL {
        return false;
    }
    let tail = &bytes[bytes.len() - TAIL..];
    (1..=TAIL / 4).any(|period| tail.chunks(period).all(|chunk| chunk == &tail[..chunk.len()]))
}
fn decode_limit(session: &mut LlamaSession, control: &Control, limit: usize) -> Result<(String, usize, f64)> {
    let start = Instant::now();
    let mut decoder = session.vocab().text_decoder();
    let mut raw = String::new();
    let mut n = 0;
    while n < limit {
        control.check()?;
        if control.model_cancel.is_cancelled() {
            return Err("model cancelled".into());
        }
        let Some(token) = session
            .next_greedy_token()
            .map_err(|e| format!("decode: {e:?}"))?
        else {
            break;
        };
        n += 1;
        if let Some(text) = decoder.push_token(session.vocab(), token) {
            raw.push_str(&text);
            if is_looping(&raw) {
                break;
            }
        }
    }
    raw.push_str(&decoder.finish());
    Ok((raw.trim().into(), n, start.elapsed().as_secs_f64()))
}
fn decode(session: &mut LlamaSession, control: &Control) -> Result<(String, usize, f64)> {
    decode_limit(session, control, 160)
}
/// The verdict, asked for outright. When the free answer did not end in one
/// (it looped, or ran to the cap mid-sentence), the conversation is continued
/// with "VERDICT:" already written and the model finishes that line, so a
/// judgement is never lost to the way it was phrased.
fn force_verdict(session: &mut LlamaSession, control: &Control) -> Result<String> {
    let ids = session
        .vocab()
        .tokenize("\nVERDICT:", false, false)
        .map_err(|e| format!("tokenize: {e:?}"))?;
    session
        .append_tokens(&ids)
        .map_err(|e| format!("verdict prefill: {e:?}"))?;
    let (tail, _, _) = decode_limit(session, control, 4)?;
    Ok(format!("VERDICT: {}", tail.trim()))
}

/// Is the vision model's whole file set on this machine?
pub fn model_installed(model: &str) -> bool {
    LocalModels::open()
        .map(|local| matches!(local.install_state(model), InstallState::Installed))
        .unwrap_or(false)
}
pub fn install(model: &str, accept: bool, control: &Control, notify: &Notify) -> Result<()> {
    let mut local = LocalModels::open().map_err(|e| e.to_string())?;
    let spec = local.spec(model).ok_or("unknown model")?.clone();
    if (spec.gated || !local.license_acknowledged(model)) && !accept {
        return Err("review the license, then use ci --install --accept-license".into());
    }
    if accept {
        local
            .acknowledge_license(model)
            .map_err(|e| e.to_string())?;
    }
    let handle = local.start_install(model).map_err(|e| e.to_string())?;
    let mut last = Instant::now();
    let mut progress = String::new();
    loop {
        if control.stopped() {
            handle.cancel();
        }
        for msg in handle.poll() {
            match msg {
                InstallMsg::Progress { file, done, total } => {
                    progress = format!("{file}: {done}/{total} bytes")
                }
                InstallMsg::FileDone { file } => notify(Update::Log(format!("verified {file}"))),
                InstallMsg::Finished => {
                    if !matches!(local.install_state(model), InstallState::Installed) {
                        return Err("install finished without verified files".into());
                    }
                    notify(Update::Model(format!("{model}: installed")));
                    notify(Update::ModelInstalled(true));
                    return Ok(());
                }
                InstallMsg::Failed(e) => {
                    handle.cancel();
                    return Err(e);
                }
                InstallMsg::Cancelled => return Err("installation cancelled".into()),
            }
        }
        if last.elapsed() >= Duration::from_secs(1) {
            notify(Update::Log(progress.clone()));
            notify(Update::Model(progress.clone()));
            last = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub fn peak_rss_bytes() -> Option<u64> {
    #[cfg(all(unix, target_pointer_width = "64"))]
    {
        unsafe extern "C" {
            fn getrusage(who: i32, usage: *mut i64) -> i32;
        }
        let mut fields = [0i64; 18];
        if unsafe { getrusage(0, fields.as_mut_ptr()) } == 0 {
            return Some(
                fields[4].max(0) as u64 * if cfg!(target_os = "macos") { 1 } else { 1024 },
            );
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_looping_answer_is_cut_and_a_verdict_line_is_still_read() {
        assert!(is_looping("a command prompt line at the top reading sudo -g -g -g -g -g -g -g -g -g -g -g -g -g -g -g -g -g -g -g -g -g"));
        assert!(is_looping(&"ab".repeat(40)));
        assert!(!is_looping("1. The window shows a terminal with a prompt.\n2. Nothing else is open.\nVERDICT: YES"));
        assert!(!is_looping("short"));
        assert_eq!(parse_verdict("- looks fine\n[no verdict line; asked outright]\nVERDICT: YES"), Ok(true));
        assert_eq!(parse_verdict("VERDICT: NO"), Ok(false));
        assert!(parse_verdict("VERDICT: maybe").is_err());
    }
    #[test]
    fn verdict_is_last_strict_marker() {
        assert_eq!(
            parse_verdict("- clear\nVERDICT: NO\nVERDICT: YES").unwrap(),
            true
        );
        assert!(!parse_verdict("VERDICT: YES\nVERDICT: NO").unwrap());
        assert!(parse_verdict("Yes").is_err());
        assert!(parse_verdict("VERDICT: YES\nVERDICT: perhaps").is_err());
        assert!(parse_verdict(" VERDICT: YES").is_err());
    }
}

// A single long-lived owner of the !Send GPU state for all scripts of a run.
// Callers are worker threads; the UI never waits on this channel.
enum Request {
    Judge(
        PathBuf,
        String,
        Control,
        Notify,
        std::sync::mpsc::SyncSender<Result<Verdict>>,
    ),
    Ask(
        String,
        Control,
        Notify,
        std::sync::mpsc::SyncSender<Result<String>>,
    ),
}
#[derive(Clone)]
pub struct Judge {
    sender: std::sync::mpsc::SyncSender<Request>,
}
pub struct JudgeWorker {
    pub judge: Judge,
    join: Option<std::thread::JoinHandle<()>>,
}
impl JudgeWorker {
    pub fn start(model: &str, disabled: bool) -> Result<Self> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(64);
        let model = model.to_string();
        let join = std::thread::Builder::new()
            .name("ci-model".into())
            .spawn(move || {
                let mut hub = UiHub::new(&model, disabled);
                while let Ok(request) = receiver.recv() {
                    match request {
                        Request::Judge(path, prompt, control, notify, reply) => {
                            let _ = reply.send(hub.judge(&path, &prompt, &control, &notify));
                        }
                        Request::Ask(prompt, control, notify, reply) => {
                            let _ = reply.send(hub.ask(&prompt, &control, &notify));
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            judge: Judge { sender },
            join: Some(join),
        })
    }
    pub fn finish(mut self) {
        drop(self.judge);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
impl Judge {
    pub fn judge(
        &self,
        path: &Path,
        prompt: &str,
        control: &Control,
        notify: &Notify,
    ) -> Result<Verdict> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.sender
            .send(Request::Judge(
                path.into(),
                prompt.into(),
                control.clone(),
                notify.clone(),
                tx,
            ))
            .map_err(|_| "model worker stopped")?;
        rx.recv().map_err(|_| "model worker lost reply")?
    }
    pub fn ask(&self, prompt: &str, control: &Control, notify: &Notify) -> Result<String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.sender
            .send(Request::Ask(
                prompt.into(),
                control.clone(),
                notify.clone(),
                tx,
            ))
            .map_err(|_| "model worker stopped")?;
        rx.recv().map_err(|_| "model worker lost reply")?
    }
}
