//! The checkouts used by a run and the progress of their upstream update.
use std::fmt::Write;
use std::sync::OnceLock;

pub const NOT_UNDER_RUN: &str = "not under makepad run";

struct SourcesHost {
    snapshot: fn() -> String,
    update: fn(),
    generation: fn() -> u64,
}

// The loader's: only the ops below read it, so JIT code never lays out a OnceLock of its own.
static HOST: OnceLock<SourcesHost> = OnceLock::new();

/// [`sources_text_into`]'s answer outside `makepad run`.
pub const ABSENT: usize = usize::MAX;

/// Install the runner's snapshot reader, update request and generation reader together. The
/// runner wakes the HAL's sessions whenever the generation moves, so the platform hears it.
#[relax_api::relax_api(loader = "the runner installs the source host: JIT code asks through the sources_text_into, request_sources_update and sources_generation ops")]
pub fn set_sources_host(snapshot: fn() -> String, update: fn(), generation: fn() -> u64) {
    let _ = HOST.set(SourcesHost { snapshot, update, generation });
}

/// The snapshot's generation, read without the snapshot: 0 outside `makepad run` and before an
/// app first asked for its sources. The platform compares it on every wake and turns a change
/// into an `Event::Signal`, so an app reads [`sources`] only when it moved.
#[relax_api::relax_api]
pub fn sources_generation() -> u64 {
    HOST.get().map_or(0, |host| (host.generation)())
}

/// Copy the runner's snapshot into `out` as far as it fits: its whole length in bytes, or
/// [`ABSENT`] outside `makepad run`. Strings do not cross the Relax op boundary.
#[relax_api::relax_api]
pub fn sources_text_into(out: &mut [u8]) -> usize {
    let Some(host) = HOST.get() else { return ABSENT };
    let text = (host.snapshot)();
    let n = text.len().min(out.len());
    out[..n].copy_from_slice(&text.as_bytes()[..n]);
    text.len()
}

/// Ask the runner for a non-blocking update of every checkout: false outside `makepad run`.
#[relax_api::relax_api]
pub fn request_sources_update() -> bool {
    let Some(host) = HOST.get() else { return false };
    (host.update)();
    true
}

/// Read the runner's snapshot, if this program is running under `makepad run`.
pub fn sources_text() -> Option<String> {
    let mut out = vec![0; 1024];
    loop {
        let len = sources_text_into(&mut out);
        if len == ABSENT {
            return None;
        }
        if len <= out.len() {
            out.truncate(len);
            return String::from_utf8(out).ok();
        }
        // the snapshot is longer than the buffer: ask again with room for it
        out.resize(len, 0);
    }
}

/// Request a non-blocking update of every checkout. Read progress with [`sources`].
pub fn update_sources() -> Result<(), String> {
    if request_sources_update() { Ok(()) } else { Err(NOT_UNDER_RUN.to_owned()) }
}

/// Read the current generation, checkouts and update state.
pub fn sources() -> Option<Sources> {
    parse(&sources_text()?)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sources {
    pub generation: u64,
    /// The run's own workspace comes first.
    pub repos: Vec<SourceRepo>,
    pub update: SourceUpdate,
}

/// One checkout that supplies the run's sources.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRepo {
    pub root: String,
    /// Upstream repository key, empty when there is none.
    pub key: String,
    /// Checked-out branch, empty for a detached HEAD.
    pub branch: String,
    pub head: String,
    /// Tracking ref's commit, empty when there is none.
    pub upstream: String,
}

/// The running or most recent update.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceUpdate {
    Idle,
    Fetching,
    Applying,
    Reloading,
    Restarting,
    Rebuilding,
    Current,
    Updated,
    NotFollowing(String),
    Failed(String),
}

fn optional_field(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

/// Write one generation, update or repository record per line.
pub fn format(sources: &Sources) -> String {
    use SourceUpdate::*;
    let state = match &sources.update {
        Idle => "idle",
        Fetching => "fetching",
        Applying => "applying",
        Reloading => "reloading",
        Restarting => "restarting",
        Rebuilding => "rebuilding",
        Current => "current",
        Updated => "updated",
        NotFollowing(_) => "not-following",
        Failed(_) => "failed",
    };
    let mut text = format!("generation {}\nupdate {state}", sources.generation);
    if let NotFollowing(reason) | Failed(reason) = &sources.update {
        text.push(' ');
        text.push_str(&reason.replace("\r\n", " ").replace(['\r', '\n'], " "));
    }
    text.push('\n');
    for repo in &sources.repos {
        // tab-separated: a local key ("local/project files") and a root may hold spaces
        let field = |s: &str| optional_field(s).replace(['\t', '\r', '\n'], " ");
        writeln!(text, "repo\t{}\t{}\t{}\t{}\t{}", repo.head, field(&repo.upstream),
            field(&repo.branch), field(&repo.key), repo.root.replace(['\r', '\n'], " ")).unwrap();
    }
    text
}

/// Read a snapshot, skipping unknown record types.
pub fn parse(text: &str) -> Option<Sources> {
    let mut generation = None;
    let mut update = None;
    let mut repos = Vec::new();
    for line in text.lines() {
        let (record, data) = line.split_once([' ', '\t']).unwrap_or((line, ""));
        match record {
            "generation" => generation = Some(data.parse().ok()?),
            "update" => {
                use SourceUpdate::*;
                update = Some(match data {
                    "idle" => Idle,
                    "fetching" => Fetching,
                    "applying" => Applying,
                    "reloading" => Reloading,
                    "restarting" => Restarting,
                    "rebuilding" => Rebuilding,
                    "current" => Current,
                    "updated" => Updated,
                    _ => {
                        let (state, reason) = data.split_once(' ')?;
                        match state {
                            "not-following" => NotFollowing(reason.to_owned()),
                            "failed" => Failed(reason.to_owned()),
                            _ => return None,
                        }
                    }
                });
            }
            "repo" => {
                let mut fields = data.splitn(5, '\t');
                let head = fields.next()?;
                let mut optional = || fields.next().map(|s| if s == "-" { "" } else { s });
                let upstream = optional()?;
                let branch = optional()?;
                let key = optional()?;
                let root = fields.next()?;
                let is_oid = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
                if !is_oid(head) || (!upstream.is_empty() && !is_oid(upstream)) {
                    return None;
                }
                repos.push(SourceRepo {
                    root: root.into(), key: key.into(), branch: branch.into(),
                    head: head.into(), upstream: upstream.into(),
                });
            }
            _ => {}
        }
    }
    Some(Sources { generation: generation?, repos, update: update? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_round_trip_and_absent_host() {
        use SourceUpdate::*;
        let mut snapshot = Sources {
            generation: 7,
            repos: vec![
                SourceRepo {
                    root: "/workspace/project files".into(), key: "local/project files".into(),
                    branch: "main".into(), head: "1".repeat(40), upstream: "2".repeat(40),
                },
                SourceRepo {
                    root: "/workspace/library".into(), key: String::new(),
                    branch: String::new(), head: "3".repeat(40), upstream: String::new(),
                },
            ],
            update: Idle,
        };
        for update in [Idle, Fetching, Applying, Reloading, Restarting, Rebuilding, Current,
            Updated, NotFollowing("local edits".into()), Failed("fetch timed out".into())] {
            snapshot.update = update;
            let text = format(&snapshot) + "future record\n";
            assert_eq!(parse(&text).as_ref(), Some(&snapshot), "snapshot should round-trip for {:?}", snapshot.update);
        }
        snapshot.update = Failed("first line\r\nsecond line".into());
        assert!(format(&snapshot).lines().any(|line| line == "update failed first line second line"),
            "a multiline reason should stay in one update record");
        assert_eq!(sources_text_into(&mut [0; 8]), ABSENT, "the op should answer ABSENT without a host");
        assert_eq!(sources_text(), None, "a native program without a host has no snapshot");
        assert_eq!(update_sources(), Err(NOT_UNDER_RUN.to_owned()), "an update requires a runner host");
    }
}
