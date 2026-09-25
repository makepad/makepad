use crate::{
    process::{self, Control, Result},
    report::{self, Branch, Notify, Update},
};
use makepad_strict_json::{self as json, Value};
use makepad_toml_parser::{parse_toml, Toml};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub const DEFAULT_SKIPS: &[&str] = &[
    "stage",
    "stage-browser",
    "stage-live",
    "sandbox",
    "source-library",
    "mixer",
    "mail",
    "wm-all",
    "wm-dyn",
    "asset-server",
    "flow-server",
    "ai-hub",
];
#[derive(Clone)]
pub struct Config {
    pub remote: String,
    pub branches: Vec<String>,
    pub poll_secs: u64,
    pub checkout: PathBuf,
    pub skip_apps: Vec<String>,
    pub model: String,
    pub targets: Vec<String>,
    pub allowed_errors: Vec<String>,
    pub no_vision: bool,
    pub parallel: usize,
    /// Run every crate's tests, not only the platform's own. Off by default.
    pub deep_tests: bool,
    pub machines: BTreeMap<String, crate::machine::Machine>,
}
impl Config {
    pub fn load(base: &Path, control: &Control) -> Result<Self> {
        let dir = base.join("local/ci");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("ci.toml");
        let installed = crate::cargo::matrix_targets(&host_target(base, control)?);
        if !path.exists() {
            let array = |a: &[String]| {
                format!(
                    "[{}]",
                    a.iter()
                        .map(|s| format!("{s:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let content=format!("remote = \"origin\"\nbranches = [\"work\"]\npoll_secs = 60\nparallel = 2\ncheckout = \"local/ci/checkout\"\nskip_apps = {}\nmodel = \"qwen3.5-4b-vision\"\ntargets = {}\n# Only explicitly listed startup errors are allowed.\nallowed_errors = []\n",array(&DEFAULT_SKIPS.iter().map(|s|s.to_string()).collect::<Vec<_>>()),array(&installed));
            use std::io::Write;
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => f.write_all(content.as_bytes()).map_err(|e| e.to_string())?,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&text, base, installed)
    }
    fn parse(text: &str, base: &Path, installed: Vec<String>) -> Result<Self> {
        crate::window_geometry::configured(text)?;
        let doc = parse_toml(text).map_err(|e| e.to_string())?;
        let string = |key: &str, default: &str| -> Result<String> {
            match doc.root.get(key) {
                None => Ok(default.into()),
                Some(t) => t
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{key} must be a string")),
            }
        };
        let array = |key: &str, default: Vec<String>| -> Result<Vec<String>> {
            match doc.root.get(key) {
                None => Ok(default),
                Some(Toml::Array(a)) => a
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| format!("{key} must contain strings"))
                    })
                    .collect(),
                _ => Err(format!("{key} must be an array")),
            }
        };
        let poll_secs = match doc.root.get("poll_secs") {
            None => 60,
            Some(t) => {
                let n = t.as_num().ok_or("poll_secs must be a number")?;
                if !n.is_finite() || n < 1.0 || n > 86400.0 || n.fract() != 0.0 {
                    return Err("poll_secs must be an integer from 1 to 86400".into());
                }
                n as u64
            }
        };
        let branches = array("branches", vec!["work".into()])?;
        if branches.is_empty() || branches.iter().any(|b| !valid_branch(b)) {
            return Err("branches must contain valid, nonempty Git branch names".into());
        }
        let remote = string("remote", "origin")?;
        if remote.is_empty() || remote.starts_with('-') {
            return Err("invalid remote".into());
        }
        let checkout = base.join(string("checkout", "local/ci/checkout")?);
        let parallel = doc
            .root
            .get("parallel")
            .map(|n| n.as_num().ok_or("parallel must be a number"))
            .transpose()?
            .unwrap_or(2.0);
        if !parallel.is_finite() || !(1.0..=64.0).contains(&parallel) || parallel.fract() != 0.0 {
            return Err("parallel must be an integer from 1 to 64".into());
        }
        let mut machines = BTreeMap::new();
        if let Some(table) = doc.root.get("machines") {
            for (name, v) in table.as_table().ok_or("machines must be a table")? {
                let t = v.as_table().ok_or("machine must be a table")?;
                let field = |k: &str| {
                    t.get(k)
                        .and_then(Toml::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| format!("machines.{name}.{k} must be a string"))
                };
                machines.insert(
                    name.clone(),
                    crate::machine::Machine {
                        tunnel: field("tunnel")?,
                        root: field("root")?,
                        os: field("os")?,
                    },
                );
            }
        }
        Ok(Self {
            remote,
            branches,
            poll_secs,
            checkout,
            skip_apps: array(
                "skip_apps",
                DEFAULT_SKIPS.iter().map(|s| s.to_string()).collect(),
            )?,
            model: string("model", "qwen3.5-4b-vision")?,
            targets: array("targets", installed)?,
            allowed_errors: array("allowed_errors", Vec::new())?,
            no_vision: false,
            parallel: parallel as usize,
            deep_tests: doc
                .root
                .get("deep_tests")
                .map(|v| v.as_bool().ok_or("deep_tests must be true or false"))
                .transpose()?
                .unwrap_or(false),
            machines,
        })
    }
}
fn valid_branch(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(['-', '/', '.'])
        && !s.ends_with(['/', '.'])
        && !s.contains("..")
        && !s.contains("@{")
        && !s.contains("//")
        && !s.chars().any(|c| c.is_control() || " ~^:?*[\\".contains(c))
        && !s
            .split('/')
            .any(|p| p.starts_with('.') || p.ends_with(".lock"))
}

pub(crate) fn probe(program: &str, args: &[String], base: &Path, control: &Control) -> Result<String> {
    let dir = base.join("local/ci");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!("probe-{}-{stamp}.log", std::process::id()));
    let result = process::run(
        program,
        args,
        base,
        &[],
        30,
        path.clone(),
        control,
        &mut |_| {},
    );
    let _ = fs::remove_file(path);
    let out = result?;
    if out.code != 0 {
        Err(format!("{program}: {}", process::error_lines(&out.out)))
    } else {
        Ok(out.out)
    }
}
pub fn installed_targets(base: &Path, control: &Control) -> Result<Vec<String>> {
    Ok(probe(
        "rustup",
        &report::strings(&["target", "list", "--installed"]),
        base,
        control,
    )?
    .lines()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(str::to_string)
    .collect())
}
pub fn host_target(base: &Path, control: &Control) -> Result<String> {
    probe("rustc", &report::strings(&["-vV"]), base, control)?
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .ok_or("rustc did not report host target".into())
}
pub fn tip(base: &Path, config: &Config, branch: &str, control: &Control) -> Result<String> {
    if !valid_branch(branch) {
        return Err("invalid branch".into());
    }
    let reference = format!("refs/heads/{branch}");
    let out = probe(
        "git",
        &[
            "ls-remote".into(),
            "--exit-code".into(),
            config.remote.clone(),
            reference.clone(),
        ],
        base,
        control,
    )?;
    out.lines()
        .find_map(|l| {
            let (hash, r) = l.split_once('\t')?;
            (r == reference
                && (hash.len() == 40 || hash.len() == 64)
                && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_string())
        })
        .ok_or_else(|| format!("remote branch {branch} not found"))
}
pub fn remote_url(base: &Path, config: &Config, control: &Control) -> Result<String> {
    Ok(probe(
        "git",
        &["remote".into(), "get-url".into(), config.remote.clone()],
        base,
        control,
    )?
    .trim()
    .into())
}

#[derive(Default)]
pub struct Queue {
    order: VecDeque<String>,
    tips: BTreeMap<String, String>,
}
impl Queue {
    pub fn push(&mut self, branch: String, tip: String) {
        if !self.tips.contains_key(&branch) {
            self.order.push_back(branch.clone());
        }
        self.tips.insert(branch, tip);
    }
    pub fn pop(&mut self) -> Option<(String, String)> {
        let branch = self.order.pop_front()?;
        let tip = self.tips.remove(&branch)?;
        Some((branch, tip))
    }
}
pub fn load_state(base: &Path, config: &Config) -> Result<BTreeMap<String, Branch>> {
    let path = base.join("local/ci/state.json");
    let mut state = BTreeMap::new();
    if path.exists() {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let v = json::parse_depth(&bytes, 32).map_err(str::to_string)?;
        for item in v
            .get("branches")
            .and_then(Value::as_arr)
            .ok_or("state.json missing branches")?
        {
            let field = |name: &str| {
                item.get(name)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| format!("state branch missing {name}"))
            };
            let mut b = Branch {
                scripts: item
                    .get("scripts")
                    .and_then(Value::as_arr)
                    .unwrap_or(&[])
                    .iter()
                    .map(report::ScriptState::from_json)
                    .collect::<Result<Vec<_>>>()?,
                name: field("name")?,
                tip: field("tip")?,
                verdict: field("verdict")?,
                finished: item
                    .get("finished")
                    .and_then(Value::as_u64)
                    .ok_or("state missing finish time")?,
                detail: item
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            };
            if b.scripts.iter().any(|s| s.color_verdict() == "red") {
                b.verdict = "red".into();
            }
            state.insert(b.name.clone(), b);
        }
    }
    for name in &config.branches {
        state.entry(name.clone()).or_insert(Branch {
            name: name.clone(),
            tip: String::new(),
            verdict: "waiting".into(),
            finished: 0,
            scripts: Vec::new(),
            detail: String::new(),
        });
    }
    state.retain(|k, _| config.branches.contains(k));
    Ok(state)
}
pub fn save_state(base: &Path, state: &BTreeMap<String, Branch>) -> Result<()> {
    let value = json::obj(vec![(
        "branches",
        Value::Arr(
            state
                .values()
                .map(|b| {
                    json::obj(vec![
                        ("name", json::s(&b.name)),
                        (
                            "scripts",
                            Value::Arr(b.scripts.iter().map(report::ScriptState::json).collect()),
                        ),
                        ("tip", json::s(&b.tip)),
                        ("verdict", json::s(&b.verdict)),
                        ("finished", Value::Int(b.finished as i64)),
                        ("detail", json::s(&b.detail)),
                    ])
                })
                .collect(),
        ),
    )]);
    let path = base.join("local/ci/state.json");
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, value.to_json())
        .and_then(|_| fs::rename(&tmp, &path))
        .map_err(|e| e.to_string())
}

pub enum Poll {
    Tip(String, String),
    Failed(String, String),
}
pub struct Poller {
    latest: Arc<Mutex<BTreeMap<String, Poll>>>,
    stop: Control,
    join: Option<thread::JoinHandle<()>>,
}
impl Poller {
    pub fn drain(&self) -> Vec<Poll> {
        let mut latest = self.latest.lock().unwrap();
        std::mem::take(&mut *latest).into_values().collect()
    }
    pub fn start(base: PathBuf, config: Config) -> Result<Self> {
        let latest = Arc::new(Mutex::new(BTreeMap::new()));
        let published = latest.clone();
        let stop = Control::default();
        let control = stop.clone();
        let join = thread::Builder::new()
            .name("ci-watcher".into())
            .spawn(move || {
                let mut next = Instant::now();
                while !control.stopped() {
                    if Instant::now() >= next {
                        for b in &config.branches {
                            let result = match tip(&base, &config, b, &control) {
                                Ok(t) => Poll::Tip(b.clone(), t),
                                Err(e) => Poll::Failed(b.clone(), e),
                            };
                            published.lock().unwrap().insert(b.clone(), result);
                        }
                        next = Instant::now() + Duration::from_secs(config.poll_secs);
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            latest,
            stop,
            join: Some(join),
        })
    }
}
impl Drop for Poller {
    fn drop(&mut self) {
        self.stop
            .shutdown
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
pub fn publish(state: &BTreeMap<String, Branch>, notify: &Notify) {
    notify(Update::Branches(state.values().cloned().collect()));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newest_tip_wins_per_branch_without_starving_other_branches() {
        let mut q = Queue::default();
        q.push("work".into(), "a".into());
        q.push("dev".into(), "x".into());
        q.push("work".into(), "b".into());
        assert_eq!(q.pop(), Some(("work".into(), "b".into())));
        assert_eq!(q.pop(), Some(("dev".into(), "x".into())));
        assert!(q.pop().is_none());
    }
    #[test]
    fn config_uses_repository_toml_parser() {
        let c = Config::parse(
            "branches=['work','dev']\npoll_secs=12\nallowed_errors=['drag_anywhere']",
            Path::new("/repo"),
            vec!["host".into()],
        )
        .unwrap();
        assert_eq!(c.targets, vec!["host"]);
        assert_eq!(c.branches, vec!["work", "dev"]);
        assert!(Config::parse("branches=['--upload-pack=x']", Path::new("/repo"), vec![]).is_err());
    }
}
