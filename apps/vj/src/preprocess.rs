//! The preprocessing manager: what the app is allowed to work out about a
//! track before anyone asks to play it.
//!
//! Four passes can run ahead of the operator — stem separation, karaoke
//! transcription, tempo and key — and each is enabled per SOURCE: the
//! explorer's current listing, the set list, or both. That split is the whole
//! point. Separating every track in a six-hundred-record library is hours of
//! GPU time nobody asked for, while separating the next three in the set list
//! is exactly what the operator wants running while they cue the current one.
//!
//! Tempo and key are ONE job (see [`Pass::Analysis`]): the chroma pass and the
//! beat grid both fall out of a single decode, so asking for either buys both.
//! Stems and karaoke are the expensive pair, and they stay serial no matter
//! what the concurrency setting says — they contend for the one device the
//! show is already drawing on.
//!
//! Nothing here talks to the UI or the audio thread. It decides WHAT should be
//! worked on and in what order; the app owns the workers that do it.

use std::collections::HashSet;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

// ---------------------------------------------------------------------------
// where the caches live
// ---------------------------------------------------------------------------

/// An operator-chosen root for every cache this app writes, or `None` for the
/// built-in location beside the rest of the local state.
///
/// A library on a spinning disk and a scratch SSD are a real distinction on
/// the machines this runs on, and the caches are the only thing here big
/// enough to care: separated stems alone are budgeted at six gigabytes.
static CACHE_ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);

/// The three caches under a chosen root. Kept apart because they are keyed
/// differently — the wave sidecars by content digest or path, the other two by
/// the digest of the decoded samples — and a flat directory would make that
/// invisible the first time someone went looking.
pub const WAVE_SUBDIR: &str = "wave-cache";
pub const STEMS_SUBDIR: &str = "stem-cache";
pub const LYRICS_SUBDIR: &str = "lyrics-cache";

pub fn cache_root() -> Option<PathBuf> {
    CACHE_ROOT.read().ok().and_then(|root| root.clone())
}

pub fn set_cache_root(root: Option<PathBuf>) {
    if let Ok(mut slot) = CACHE_ROOT.write() {
        *slot = root;
    }
}

/// One cache's directory under the chosen root, when there is one. The
/// per-cache environment variables still win over this: they exist so a test
/// or a packaging script can pin a directory, and an operator preference must
/// not silently overrule something the machine was explicitly told.
pub fn cache_subdir(subdir: &str) -> Option<PathBuf> {
    cache_root().map(|root| root.join(subdir))
}

// ---------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------

/// The four things that can be worked out ahead of time.
///
/// `Bpm` and `Key` are separate here because they are separate to the
/// OPERATOR — two columns, two checkboxes — even though one job serves both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pass {
    Stems,
    Karaoke,
    Key,
    Bpm,
}

pub const PASSES: [Pass; 4] = [Pass::Stems, Pass::Karaoke, Pass::Key, Pass::Bpm];

impl Pass {
    /// The settings-file key. Stable: renaming one silently resets that row
    /// to its default on every machine that already has a file.
    pub fn slug(self) -> &'static str {
        match self {
            Pass::Stems => "stems",
            Pass::Karaoke => "karaoke",
            Pass::Key => "key",
            Pass::Bpm => "bpm",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Pass::Stems => "STEMS",
            Pass::Karaoke => "KARAOKE",
            Pass::Key => "KEY",
            Pass::Bpm => "BPM",
        }
    }

    fn index(self) -> usize {
        match self {
            Pass::Stems => 0,
            Pass::Karaoke => 1,
            Pass::Key => 2,
            Pass::Bpm => 3,
        }
    }
}

/// Which of the two work groups a pass is served by.
///
/// [`Pass::Bpm`] and [`Pass::Key`] both come out of one whole-track analysis,
/// so a track wanted by either is analysed once and both columns fill. Stems
/// and karaoke are a chain — separate, then transcribe the separated vocal —
/// and share the warm-up lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// One decode, one analysis: tempo, key, waveform, phrase map.
    Analysis,
    /// Separation and the transcription that reads its vocal stem.
    Warmup,
}

impl Pass {
    pub fn group(self) -> Group {
        match self {
            Pass::Stems | Pass::Karaoke => Group::Warmup,
            Pass::Key | Pass::Bpm => Group::Analysis,
        }
    }
}

/// Where a pass is allowed to look for work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Scope {
    pub explorer: bool,
    pub queue: bool,
}

impl Scope {
    pub fn off(self) -> bool {
        !self.explorer && !self.queue
    }
}

/// How many tracks ahead a pass may work by default, and the ceiling the
/// dialog will accept. The ceiling is not a performance limit — it is a limit
/// on how much of a library one careless drag can commit the machine to.
pub const DEFAULT_AHEAD: usize = 20;
pub const MAX_AHEAD: usize = 500;
/// Concurrent analysis jobs. One by default: the point of this work is that
/// nobody is waiting on it.
pub const DEFAULT_CONCURRENCY: usize = 1;
pub const MAX_CONCURRENCY: usize = 8;

/// The whole dialog, as it is written to disk.
#[derive(Clone, Debug, PartialEq)]
pub struct PreprocessSettings {
    scopes: [Scope; 4],
    /// How far down each source to look. Each enabled source contributes its
    /// own `ahead` candidates, so a pass watching both can pull up to twice
    /// this many tracks — which is what "twenty ahead in the set list AND
    /// twenty down the listing" plainly means.
    pub ahead: usize,
    /// Look at only the first minute of each record.
    ///
    /// For a library big enough that the columns are what matter and the
    /// wait is what hurts. A partial answer says so, and any record that
    /// reaches a deck is measured again in full before it plays -- so the
    /// only thing traded away is the accuracy of a column on a record
    /// nobody has touched yet.
    pub fast: bool,
    /// Analysis jobs at once. Ignored by the warm-up group, which is serial
    /// by construction; see [`Group`].
    pub concurrency: usize,
    /// Operator-chosen cache root, or `None` for the built-in one.
    pub cache_root: Option<PathBuf>,
}

impl Default for PreprocessSettings {
    /// Everything on. A blank KEY column is the bug this dialog exists to
    /// answer, and a first run that preprocesses nothing would leave it blank.
    fn default() -> Self {
        PreprocessSettings {
            scopes: [Scope { explorer: true, queue: true }; 4],
            ahead: DEFAULT_AHEAD,
            // Off: a partial answer is a trade, and a trade is the
            // operator's to make.
            fast: false,
            concurrency: DEFAULT_CONCURRENCY,
            cache_root: None,
        }
    }
}

impl PreprocessSettings {
    pub fn scope(&self, pass: Pass) -> Scope {
        self.scopes[pass.index()]
    }

    pub fn set_scope(&mut self, pass: Pass, scope: Scope) {
        self.scopes[pass.index()] = scope;
    }

    /// Is any pass in this group looking at this source?
    pub fn group_wants(&self, group: Group, explorer: bool) -> bool {
        PASSES.iter().any(|pass| {
            pass.group() == group
                && match explorer {
                    true => self.scope(*pass).explorer,
                    false => self.scope(*pass).queue,
                }
        })
    }

    pub fn group_off(&self, group: Group) -> bool {
        !self.group_wants(group, true) && !self.group_wants(group, false)
    }

    /// `key value` per line, like the scan dialog's file: order-independent,
    /// and a line that is missing or will not parse costs the operator that
    /// one setting rather than all of them.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for pass in PASSES {
            let scope = self.scope(pass);
            out.push_str(&format!(
                "{} {} {}\n",
                pass.slug(),
                u8::from(scope.explorer),
                u8::from(scope.queue),
            ));
        }
        out.push_str(&format!("ahead {}\n", self.ahead));
        out.push_str(&format!("concurrency {}\n", self.concurrency));
        out.push_str(&format!("fast {}
", u8::from(self.fast)));
        // An empty value is a real state — "use the built-in root" — and has
        // to be distinguishable from the line never having been written.
        match &self.cache_root {
            Some(root) => out.push_str(&format!("cache_root {}\n", root.display())),
            None => out.push_str("cache_root\n"),
        }
        out
    }

    pub fn from_text(body: &str) -> PreprocessSettings {
        let mut out = PreprocessSettings::default();
        for line in body.lines() {
            let mut parts = line.split_whitespace();
            let Some(key) = parts.next() else { continue };
            match key {
                "ahead" => {
                    if let Some(value) = parts.next().and_then(|v| v.parse::<usize>().ok()) {
                        out.ahead = value.clamp(1, MAX_AHEAD);
                    }
                }
                "fast" => {
                    out.fast = matches!(parts.next(), Some("1") | Some("true") | Some("on"));
                }
                "concurrency" => {
                    if let Some(value) = parts.next().and_then(|v| v.parse::<usize>().ok()) {
                        out.concurrency = value.clamp(1, MAX_CONCURRENCY);
                    }
                }
                "cache_root" => {
                    // The rest of the line, not the next token: a chosen
                    // directory may well have a space in it.
                    let rest = line["cache_root".len()..].trim();
                    out.cache_root = (!rest.is_empty()).then(|| PathBuf::from(rest));
                }
                _ => {
                    let Some(pass) = PASSES.iter().find(|pass| pass.slug() == key) else {
                        continue;
                    };
                    let (Some(explorer), Some(queue)) = (parts.next(), parts.next()) else {
                        continue;
                    };
                    out.set_scope(
                        *pass,
                        Scope { explorer: explorer == "1", queue: queue == "1" },
                    );
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// what to work on next
// ---------------------------------------------------------------------------

/// The tracks one group should work through, in the order it should take
/// them.
///
/// The set list comes first and always: it is what is about to be played, and
/// a warm-up that spends its one lane on the middle of the library while the
/// next record goes in cold has got the priority exactly backwards. Inside
/// each source the caller's order is kept — the listing's current sort is the
/// operator's own statement of what matters.
///
/// `done` is everything already finished or already known to be hopeless.
/// Filtering here rather than at the submit site is what keeps a finished
/// library from re-deciding six hundred times a second that there is nothing
/// to do.
pub fn work_list<T: Clone + Eq + Hash>(
    settings: &PreprocessSettings,
    group: Group,
    explorer: &[T],
    queue: &[T],
    done: &HashSet<T>,
) -> Vec<T> {
    let mut out = Vec::new();
    let mut seen: HashSet<T> = HashSet::new();
    let take = |source: &[T], out: &mut Vec<T>, seen: &mut HashSet<T>| {
        for item in source.iter().take(settings.ahead) {
            if done.contains(item) || !seen.insert(item.clone()) {
                continue;
            }
            out.push(item.clone());
        }
    };
    if settings.group_wants(group, false) {
        take(queue, &mut out, &mut seen);
    }
    if settings.group_wants(group, true) {
        take(explorer, &mut out, &mut seen);
    }
    out
}

// ---------------------------------------------------------------------------
// clearing and moving
// ---------------------------------------------------------------------------

/// What a destructive cache operation actually managed to do.
///
/// Reported rather than swallowed: a clear that could not remove the tracks
/// on the decks — their files are open — has to say so, or the operator is
/// left believing the disk is empty when it is not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CacheReport {
    pub removed: usize,
    pub bytes: u64,
    pub failed: Vec<PathBuf>,
}

impl CacheReport {
    pub fn summary(&self) -> String {
        let mb = self.bytes as f64 / (1024.0 * 1024.0);
        match self.failed.len() {
            0 => format!("cleared {} item(s), {mb:.0} MB", self.removed),
            failed => format!(
                "cleared {} item(s), {mb:.0} MB — {failed} could not be removed",
                self.removed
            ),
        }
    }
}

fn entry_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else { return 0 };
    if meta.is_file() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.flatten().map(|entry| entry_bytes(&entry.path())).sum()
}

/// Empty one cache directory, leaving the directory itself in place so the
/// next job does not have to re-create it.
pub fn clear_dir(dir: &Path) -> CacheReport {
    let mut report = CacheReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else { return report };
    for entry in entries.flatten() {
        let path = entry.path();
        let bytes = entry_bytes(&path);
        let removed = match path.is_dir() {
            true => std::fs::remove_dir_all(&path),
            false => std::fs::remove_file(&path),
        };
        match removed {
            Ok(()) => {
                report.removed += 1;
                report.bytes += bytes;
            }
            Err(_) => report.failed.push(path),
        }
    }
    report
}

/// Move one cache directory's contents to a new root.
///
/// Falls back to copy-then-delete when the two are on different volumes,
/// which is the normal case for this feature — moving the cache to another
/// disk is most of the reason to move it at all. Anything that cannot be
/// moved is LEFT WHERE IT IS and reported: a half-migrated cache that has
/// forgotten half of itself is worse than one that never moved.
pub fn move_dir(from: &Path, to: &Path) -> CacheReport {
    let mut report = CacheReport::default();
    let Ok(entries) = std::fs::read_dir(from) else { return report };
    if std::fs::create_dir_all(to).is_err() {
        report.failed.push(to.to_path_buf());
        return report;
    }
    for entry in entries.flatten() {
        let source = entry.path();
        let Some(name) = source.file_name() else { continue };
        let target = to.join(name);
        let bytes = entry_bytes(&source);
        if std::fs::rename(&source, &target).is_ok() {
            report.removed += 1;
            report.bytes += bytes;
            continue;
        }
        match copy_tree(&source, &target) {
            true => {
                let dropped = match source.is_dir() {
                    true => std::fs::remove_dir_all(&source),
                    false => std::fs::remove_file(&source),
                };
                match dropped {
                    Ok(()) => {
                        report.removed += 1;
                        report.bytes += bytes;
                    }
                    // Copied but not removed: the bytes are safe at the
                    // destination, so this is a leftover, not a loss.
                    Err(_) => report.failed.push(source),
                }
            }
            false => report.failed.push(source),
        }
    }
    report
}

fn copy_tree(from: &Path, to: &Path) -> bool {
    if from.is_file() {
        return std::fs::copy(from, to).is_ok();
    }
    if std::fs::create_dir_all(to).is_err() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(from) else { return false };
    let mut ok = true;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(|name| name.to_string()) else {
            ok = false;
            continue;
        };
        ok &= copy_tree(&entry.path(), &to.join(name));
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn settings_round_trip_through_text() {
        let mut settings = PreprocessSettings::default();
        settings.set_scope(Pass::Stems, Scope { explorer: false, queue: true });
        settings.set_scope(Pass::Karaoke, Scope { explorer: false, queue: false });
        settings.ahead = 7;
        settings.concurrency = 3;
        settings.cache_root = Some(PathBuf::from("D:/vj cache/with a space"));
        let back = PreprocessSettings::from_text(&settings.to_text());
        assert_eq!(back, settings);
        // A path with a space in it is the case a token-at-a-time reader
        // would silently truncate.
        assert_eq!(back.cache_root, Some(PathBuf::from("D:/vj cache/with a space")));
    }

    #[test]
    fn an_absent_or_mangled_line_costs_one_setting_and_not_the_rest() {
        let settings = PreprocessSettings::from_text(
            "stems 0 1\nahead not-a-number\nconcurrency 4\nnonsense\n",
        );
        assert_eq!(settings.scope(Pass::Stems), Scope { explorer: false, queue: true });
        // Untouched rows keep their defaults rather than becoming off.
        assert_eq!(settings.scope(Pass::Bpm), Scope { explorer: true, queue: true });
        assert_eq!(settings.ahead, DEFAULT_AHEAD, "a bad number reads as absent");
        assert_eq!(settings.concurrency, 4);
        assert_eq!(settings.cache_root, None);
        // An empty file is the defaults, not an all-off dialog.
        assert_eq!(PreprocessSettings::from_text(""), PreprocessSettings::default());
    }

    #[test]
    fn out_of_range_numbers_are_clamped_not_taken() {
        let settings = PreprocessSettings::from_text("ahead 99999\nconcurrency 0\n");
        assert_eq!(settings.ahead, MAX_AHEAD);
        assert_eq!(settings.concurrency, 1);
    }

    #[test]
    fn an_explicitly_empty_cache_root_is_the_built_in_one() {
        let settings = PreprocessSettings::from_text("cache_root\n");
        assert_eq!(settings.cache_root, None);
        // And it survives being written back out as that same state.
        assert_eq!(PreprocessSettings::from_text(&settings.to_text()).cache_root, None);
    }

    #[test]
    fn the_set_list_is_worked_before_the_listing() {
        let settings = PreprocessSettings::default();
        let list = work_list(
            &settings,
            Group::Analysis,
            &keys(&["e1", "e2"]),
            &keys(&["q1", "q2"]),
            &HashSet::new(),
        );
        assert_eq!(list, keys(&["q1", "q2", "e1", "e2"]));
    }

    #[test]
    fn a_track_in_both_sources_is_worked_once() {
        let settings = PreprocessSettings::default();
        let list = work_list(
            &settings,
            Group::Analysis,
            &keys(&["shared", "only-listing"]),
            &keys(&["shared"]),
            &HashSet::new(),
        );
        assert_eq!(list, keys(&["shared", "only-listing"]));
    }

    #[test]
    fn each_source_contributes_its_own_ahead() {
        let mut settings = PreprocessSettings::default();
        settings.ahead = 2;
        let list = work_list(
            &settings,
            Group::Analysis,
            &keys(&["e1", "e2", "e3"]),
            &keys(&["q1", "q2", "q3"]),
            &HashSet::new(),
        );
        assert_eq!(list, keys(&["q1", "q2", "e1", "e2"]), "two from each, not two in total");
    }

    #[test]
    fn a_scope_that_is_off_yields_no_work_from_that_source() {
        let mut settings = PreprocessSettings::default();
        for pass in PASSES {
            settings.set_scope(pass, Scope { explorer: false, queue: false });
        }
        settings.set_scope(Pass::Bpm, Scope { explorer: false, queue: true });
        let list = work_list(
            &settings,
            Group::Analysis,
            &keys(&["e1"]),
            &keys(&["q1"]),
            &HashSet::new(),
        );
        assert_eq!(list, keys(&["q1"]), "the listing is off for every analysis pass");
        // The warm-up group is off entirely, so it gets nothing from either.
        assert!(work_list(
            &settings,
            Group::Warmup,
            &keys(&["e1"]),
            &keys(&["q1"]),
            &HashSet::new()
        )
        .is_empty());
    }

    #[test]
    fn either_of_bpm_and_key_pulls_the_one_analysis_job() {
        let mut settings = PreprocessSettings::default();
        for pass in PASSES {
            settings.set_scope(pass, Scope::default());
        }
        // KEY alone is enough to want the analysis, because one job serves
        // both columns.
        settings.set_scope(Pass::Key, Scope { explorer: true, queue: false });
        assert!(!settings.group_off(Group::Analysis));
        assert!(settings.group_off(Group::Warmup));
        let list = work_list(
            &settings,
            Group::Analysis,
            &keys(&["e1"]),
            &keys(&["q1"]),
            &HashSet::new(),
        );
        assert_eq!(list, keys(&["e1"]));
    }

    #[test]
    fn finished_work_is_not_offered_again() {
        let settings = PreprocessSettings::default();
        let mut done = HashSet::new();
        done.insert("q1".to_string());
        done.insert("e1".to_string());
        let list = work_list(
            &settings,
            Group::Analysis,
            &keys(&["e1", "e2"]),
            &keys(&["q1", "q2"]),
            &done,
        );
        assert_eq!(list, keys(&["q2", "e2"]));
    }

    #[test]
    fn clearing_empties_a_directory_and_counts_what_went() {
        let dir = std::env::temp_dir()
            .join(format!("makepad-vj-clear-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a-track")).expect("make cache");
        std::fs::write(dir.join("one.wave"), vec![7u8; 512]).expect("write");
        std::fs::write(dir.join("a-track/spans"), vec![7u8; 256]).expect("write");
        let report = clear_dir(&dir);
        assert_eq!(report.removed, 2, "one file and one track directory");
        assert_eq!(report.bytes, 768);
        assert!(report.failed.is_empty());
        // The directory itself survives, empty.
        assert!(dir.is_dir());
        assert_eq!(std::fs::read_dir(&dir).expect("read").count(), 0);
        // Clearing an absent directory is a no-op, not a failure.
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(clear_dir(&dir), CacheReport::default());
    }

    #[test]
    fn moving_carries_the_tree_across_and_leaves_nothing_behind() {
        let base = std::env::temp_dir()
            .join(format!("makepad-vj-move-{}", std::process::id()));
        let (from, to) = (base.join("from"), base.join("to"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(from.join("a-track")).expect("make cache");
        std::fs::write(from.join("one.wave"), vec![1u8; 100]).expect("write");
        std::fs::write(from.join("a-track/vocals.pcm"), vec![1u8; 200]).expect("write");
        let report = move_dir(&from, &to);
        assert_eq!(report.removed, 2);
        assert!(report.failed.is_empty());
        assert!(to.join("one.wave").is_file());
        assert!(to.join("a-track/vocals.pcm").is_file(), "nested files come too");
        assert_eq!(std::fs::read_dir(&from).expect("read").count(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }
}
