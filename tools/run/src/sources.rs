//! The run's checkouts and the worker that brings them to their configured upstreams.
use makepad_git::{follow, GitError, GitHttpMethod, GitHttpRequest, GitHttpResponse, NoopHttpSyncHooks, Repository, TreeChange};
use makepad_hal::sources::{self, SourceRepo, Sources, SourceUpdate};
use relax_compiler::{diag::Severity, domain::DomainState, reload, run_state::Event};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub(super) const RELAUNCH_REASON: &str = "a source update";
const REFRESH: Duration = Duration::from_secs(5);
const ROUND_SPACING: Duration = Duration::from_secs(20);
/// A failed round is asked again after this, doubling up to RETRY_MOST, for as long as it fails.
const RETRY_FIRST: Duration = Duration::from_secs(30);
const RETRY_MOST: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Armed { No, OnOffer, Forced }

impl Armed {
    fn for_paths(paths: &[String]) -> Self {
        match reload::watches(paths) {
            Some(false) => Self::No,
            Some(true) => Self::OnOffer,
            None => Self::Forced,
        }
    }
}

struct State {
    snapshot: Sources,
    started: bool,
    wanted: bool,
    armed: Armed,
}

impl State {
    fn update(&mut self, next: SourceUpdate) {
        if self.snapshot.update != next {
            self.snapshot.update = next;
            self.bump();
        }
    }

    /// A new snapshot. Every session's pump wakes, and the platform's signal check hands the
    /// app an `Event::Signal` for it: an idle app reads it without polling.
    fn bump(&mut self) {
        self.snapshot.generation += 1;
        makepad_hal::Hal::global().wake();
    }

    fn fail(&mut self, reason: String) {
        self.armed = Armed::No;
        self.update(SourceUpdate::Failed(reason));
    }
}

static STATE: Mutex<State> = Mutex::new(State {
    snapshot: Sources { generation: 0, repos: Vec::new(), update: SourceUpdate::Idle },
    started: false, wanted: false, armed: Armed::No,
});
static WAKE: Condvar = Condvar::new();

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn install() {
    sources::set_sources_host(snapshot_text, request_update, generation);
}

// Read on every platform wake: no worker start, a brief lock.
fn generation() -> u64 {
    state().snapshot.generation
}

/// When a failed round is asked again. There is no last attempt: a blip clears by itself, a
/// lasting failure is asked again every RETRY_MOST.
#[derive(Default)]
struct Retry(Option<Duration>);

impl Retry {
    fn failed(&mut self) -> Duration {
        let wait = self.0.map_or(RETRY_FIRST, |last| (last * 2).min(RETRY_MOST));
        self.0 = Some(wait);
        wait
    }
}

fn start_worker() {
    {
        let mut s = state();
        if s.started { return; }
        s.started = true;
    }
    let _kept = relax_compiler::zone_alloc::kept();
    if let Err(e) = std::thread::Builder::new().name("run-sources".into()).spawn(worker) {
        let mut s = state();
        s.started = false;
        s.fail(format!("cannot start source worker: {e}"));
    }
}

fn snapshot_text() -> String {
    start_worker();
    let snapshot = state().snapshot.clone();
    sources::format(&snapshot)
}

fn request_update() {
    state().wanted = true;
    start_worker();
    WAKE.notify_one();
}

fn worker() {
    let _kept = relax_compiler::zone_alloc::kept();
    let mut roots = Vec::new();
    let mut next_refresh = Instant::now();
    let mut next_round = next_refresh;
    let mut retry = Retry::default();
    loop {
        let mut s = state();
        let now = Instant::now();
        if s.wanted && now >= next_round {
            s.wanted = false;
            s.update(SourceUpdate::Fetching);
            drop(s);
            next_round = now + ROUND_SPACING;
            refresh(&mut roots);
            let round = follow_all(&roots, &mut git_http, begin_apply);
            match &round {
                Err(e) => {
                    let wait = retry.failed();
                    eprintln!("makepad run: following the upstream failed, asking again in {} s: {e}", wait.as_secs());
                    next_round = now + wait;
                    state().wanted = true;
                }
                Ok(_) => retry = Retry::default(),
            }
            finish(round);
        } else if now >= next_refresh {
            drop(s);
        } else {
            let due = if s.wanted { next_refresh.min(next_round) } else { next_refresh };
            drop(WAKE.wait_timeout(s, due - now).unwrap_or_else(|e| e.into_inner()));
            continue;
        }
        refresh(&mut roots);
        next_refresh = Instant::now() + REFRESH;
    }
}

// Repository::open searches upwards. A nested checkout removed since the run loaded
// must never turn a request for it into another update of its enclosing checkout.
fn open_root(root: &Path) -> Result<Repository, GitError> {
    let path = root.canonicalize()?;
    let repo = Repository::open(&path)?;
    if repo.workdir != path {
        return Err(GitError::InvalidRef(format!("{} is no longer a checkout", root.display())));
    }
    Ok(repo)
}

fn read_repo(root: &Path) -> Result<SourceRepo, GitError> {
    let repo = open_root(root)?;
    let mut entry = SourceRepo {
        root: root.to_string_lossy().into_owned(), head: repo.head_oid()?.to_hex(),
        branch: repo.current_branch()?.unwrap_or_default(), key: String::new(), upstream: String::new(),
    };
    if let Some(up) = follow::upstream(&repo)? {
        entry.key = follow::repo_key(&up.url).unwrap_or_default();
        match repo.resolve_ref(&up.tracking_ref) {
            Ok(tip) => entry.upstream = tip.to_hex(),
            Err(GitError::RefNotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(entry)
}

fn refresh(roots: &mut Vec<PathBuf>) {
    let heads = {
        let _run = relax_compiler::run_threads::process_work();
        reload::loaded_heads()
    };
    if let Some((heads, _)) = heads {
        *roots = heads.into_iter().map(|(root, _)| PathBuf::from(root)).collect();
    }
    // A ref may be changing underneath a refresh. Keep the last complete view until it reads.
    if let Ok(repos) = roots.iter().map(|root| read_repo(root)).collect::<Result<Vec<_>, _>>() {
        let mut s = state();
        if s.snapshot.repos != repos {
            s.snapshot.repos = repos;
            s.bump();
        }
    }
}

fn git_http(req: &GitHttpRequest) -> Result<GitHttpResponse, String> {
    let headers: Vec<_> = req.headers.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let request = relax_light_http::Request {
        method: match req.method { GitHttpMethod::Get => "GET", GitHttpMethod::Post => "POST" },
        url: &req.url, headers: &headers, body: &req.body, timeout: Duration::from_secs(60), max_body: 4 << 30,
    };
    let mut body = Vec::new();
    let resp = relax_light_http::send(&request, &mut body, &mut |_, _| {})?;
    Ok(GitHttpResponse { status_code: resp.status, headers: resp.headers, body })
}

#[derive(Debug)]
enum Round {
    Current,
    NotFollowing(String),
    Applied { changed: Vec<String> },
}

fn folder(root: &Path) -> &str {
    root.file_name().and_then(|name| name.to_str()).unwrap_or("checkout")
}

fn follow_all(
    roots: &[PathBuf],
    http: &mut dyn FnMut(&GitHttpRequest) -> Result<GitHttpResponse, String>,
    applying: impl FnOnce(&[String]),
) -> Result<Round, GitError> {
    let mut fetched = Vec::new();
    for root in roots {
        let mut repo = open_root(root)?;
        if let Some(up) = follow::upstream(&repo)? {
            let tips = follow::fetch(&mut repo, &up, http, &mut NoopHttpSyncHooks)?;
            fetched.push((root, repo, up, tips));
        }
    }
    let mut moves = Vec::new();
    let mut changed = Vec::new();
    for (root, mut repo, up, tips) in fetched {
        let name = folder(root);
        let (from, to, kind) = match follow::plan(&mut repo, &up, &tips)? {
            follow::Plan::Current => continue,
            follow::Plan::Changes(paths) => return Ok(Round::NotFollowing(format!("{name} has local changes ({})", paths.join(", ")))),
            follow::Plan::OwnCommits => return Ok(Round::NotFollowing(format!("{name} has commits of its own"))),
            follow::Plan::FastForward { from, to } => (from, to, "fast-forward"),
            follow::Plan::Mirror { from, to } => (from, to, "mirror: the upstream was rewritten; this checkout held nothing of its own"),
        };
        for change in repo.diff_commits(&from, &to)? {
            let path = match change {
                TreeChange::Added { path, .. } | TreeChange::Deleted { path, .. } | TreeChange::Modified { path, .. } => path,
            };
            changed.push(root.join(path).to_string_lossy().into_owned());
        }
        moves.push((name, repo, up.branch, from, to, kind));
    }
    if moves.is_empty() { return Ok(Round::Current); }
    applying(&changed);
    let mut prepared = Vec::new();
    for (name, mut repo, branch, from, to, kind) in moves {
        let update = follow::prepare(&mut repo, &branch, from, to)?;
        prepared.push((name, repo, update, from, to, kind));
    }
    for (name, mut repo, update, from, to, kind) in prepared {
        update.apply(&mut repo)?;
        eprintln!("makepad run: {name} follows its upstream {}..{} ({kind})", &from.to_hex()[..7], &to.to_hex()[..7]);
    }
    Ok(Round::Applied { changed })
}

fn begin_apply(changed: &[String]) {
    let _run = relax_compiler::run_threads::process_work();
    let armed = Armed::for_paths(changed);
    let mut s = state();
    s.armed = armed;
    s.update(SourceUpdate::Applying);
}

fn finish(result: Result<Round, GitError>) {
    match result {
        Ok(Round::Current) => state().update(SourceUpdate::Current),
        Ok(Round::NotFollowing(reason)) => {
            let mut s = state();
            s.armed = Armed::No;
            s.update(SourceUpdate::NotFollowing(reason));
        }
        Err(e) => state().fail(e.to_string()),
        Ok(Round::Applied { changed }) => {
            let _run = relax_compiler::run_threads::process_work();
            let armed = Armed::for_paths(&changed);
            {
                let mut s = state();
                // A refused patch or a fresh running domain can disarm us during the writes.
                if s.snapshot.update == SourceUpdate::Applying {
                    if s.armed != Armed::No { s.armed = armed; }
                    let next = if s.armed == Armed::Forced { SourceUpdate::Restarting } else { SourceUpdate::Updated };
                    s.update(next);
                }
            }
            if armed == Armed::OnOffer { reload::sources_changed(); }
            makepad_hal::Hal::global().wake_session(makepad_hal::session::HOST);
        }
    }
}

/// Whether the host should relaunch for an update now. A forced one (no watcher picks the files
/// up) needs no offer. Neither skips the app's own `can_restart`: a call postpones it.
pub(super) fn take_restart(restart_asked: bool, offered: bool, rebuild: bool) -> bool {
    let mut s = state();
    let forced = s.armed == Armed::Forced;
    // Offers stay pending in the host. Take one only after every prepared checkout has applied.
    if s.armed == Armed::No || restart_asked || (!offered && !forced) || s.snapshot.update == SourceUpdate::Applying {
        return false;
    }
    s.armed = Armed::No;
    s.update(if rebuild { SourceUpdate::Rebuilding } else { SourceUpdate::Restarting });
    true
}

pub(super) fn observe(event: &Event) {
    let Event::Patch { domain, applied: false, records } = event else { return };
    if domain.is_some() && *domain != relax_compiler::run::app_domain() { return; }
    if let Some(error) = records.iter().find(|r| r.severity == Severity::Error) {
        let mut s = state();
        if s.armed != Armed::No {
            let _kept = relax_compiler::zone_alloc::kept();
            s.fail(format!("the new version doesn't compile here: {}; the previous code keeps running", error.message));
        }
    }
}

pub(super) fn observe_domain(id: u32, next: &DomainState) {
    if matches!(next, DomainState::Running) {
        let mut s = state();
        s.armed = Armed::No;
        if s.snapshot.update == SourceUpdate::Restarting && relax_compiler::run::app_domain() == Some(id) {
            s.update(SourceUpdate::Updated);
        }
    }
}

pub(super) fn restart_failed(reason: &str) {
    let mut s = state();
    if matches!(s.snapshot.update, SourceUpdate::Restarting | SourceUpdate::Rebuilding) {
        let _kept = relax_compiler::zone_alloc::kept();
        s.fail(reason.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_git::{config, ObjectId, Signature};
    use std::fs;

    fn init(root: &Path) -> Repository {
        let git = root.join(".git");
        fs::create_dir_all(git.join("objects")).unwrap();
        fs::create_dir_all(git.join("refs/heads")).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        Repository::open(root).unwrap()
    }

    fn commit(repo: &mut Repository, text: &str) -> ObjectId {
        fs::write(repo.workdir.join("source"), text).unwrap();
        repo.stage_file("source").unwrap();
        repo.commit(text, Signature {
            name: "Test".into(), email: "test@example.org".into(), timestamp: 0, tz_offset: "+0000".into(),
        }).unwrap()
    }

    #[test]
    fn a_failed_round_is_asked_again_for_as_long_as_it_fails() {
        let mut retry = Retry::default();
        let waits: Vec<_> = (0..12).map(|_| retry.failed()).collect();
        assert!(waits[0] >= ROUND_SPACING && waits.windows(2).all(|w| w[0] <= w[1]), "a failed round backs off from the round spacing");
        assert_eq!(waits[11], RETRY_MOST, "there is no last attempt: a lasting failure is still asked again, at the ceiling");
    }

    fn no_http(_: &GitHttpRequest) -> Result<GitHttpResponse, String> {
        panic!("local upstreams must not use HTTP")
    }

    fn follower(base: &Path, name: &str) -> (Repository, Repository) {
        let mut source = init(&base.join("upstreams").join(name));
        let first = commit(&mut source, "first");
        let mut repo = init(&base.join(name));
        let cfg = repo.common_dir.join("config");
        config::set_in_file(&cfg, "remote.origin.url", source.workdir.to_str().unwrap()).unwrap();
        config::set_in_file(&cfg, "branch.main.merge", "refs/heads/main").unwrap();
        config::set_in_file(&cfg, "branch.main.remote", "origin").unwrap();
        let up = follow::upstream(&repo).unwrap().unwrap();
        follow::fetch(&mut repo, &up, &mut no_http, &mut NoopHttpSyncHooks).unwrap();
        repo.create_branch("main", &first).unwrap();
        repo.checkout_branch("main").unwrap();
        (source, repo)
    }

    #[test]
    fn sources_follow_both_checkouts_and_preserve_both_on_a_local_edit() {
        let tmp = makepad_git::test_support::tempdir().unwrap();
        let (mut first_source, first) = follower(tmp.path(), "first");
        let (mut second_source, second) = follower(tmp.path(), "second");
        let roots = [first.workdir.clone(), second.workdir.clone()];
        let tips = [commit(&mut first_source, "first update"), commit(&mut second_source, "second update")];

        let result = follow_all(&roots, &mut no_http, |_| {}).unwrap();
        let Round::Applied { changed } = result else { panic!("both clean checkouts should advance: {result:?}") };
        assert_eq!(changed, roots.map(|root| root.join("source").to_str().unwrap().to_owned()), "report both absolute changed paths");
        assert_eq!([first.head_oid().unwrap(), second.head_oid().unwrap()], tips, "both branches should reach their upstreams");

        commit(&mut first_source, "next first");
        commit(&mut second_source, "next second");
        fs::write(second.workdir.join("source"), "local edit").unwrap();
        let roots = [first.workdir.clone(), second.workdir.clone()];
        let result = follow_all(&roots, &mut no_http, |_| panic!("a tracked edit should stop the round before applying")).unwrap();
        assert!(matches!(result, Round::NotFollowing(reason) if reason == "second has local changes (source)"), "identify the checkout and the edited path");
        assert_eq!([first.head_oid().unwrap(), second.head_oid().unwrap()], tips, "neither branch should move on refusal");
        assert_eq!(fs::read(first.workdir.join("source")).unwrap(), b"first update", "the clean checkout should keep its prior files");
        assert_eq!(fs::read(second.workdir.join("source")).unwrap(), b"local edit", "the person's edit should survive");
    }
}
