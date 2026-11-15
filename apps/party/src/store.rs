//! The party's memory: the guests, what they sent (sources: a short loop's
//! frames or one photo) and what the box made of it (takes: a redrawn copy
//! of every frame, after one prompt, by one model).
//!
//! Everything lives in memory and is gone when the party ends. Only a guest's
//! own "keep it" writes anything to disk, and taking it back, deleting the
//! source or leaving removes it again. Deleting a source removes every take
//! of it.
//!
//! The store is behind one mutex that every holder keeps for a few field
//! updates only: pictures are shared `Arc`s, and the disk work a change
//! needs comes back as [`Persist`] steps the caller runs after letting go.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type GuestId = u64;
pub type SourceId = u64;
pub type TakeId = u64;
/// One JPEG, shared between the store, the hub worker and the screen.
pub type Jpeg = Arc<[u8]>;

/// Who may see a source and its takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Keep {
    /// Only the guest's own phone.
    Me,
    /// The big screen too, until the party ends.
    Wall,
    /// The big screen, and written to disk to stay after the party.
    Keep,
}

impl Keep {
    pub fn parse(text: &str) -> Option<Keep> {
        match text {
            "me" => Some(Keep::Me),
            "wall" => Some(Keep::Wall),
            "keep" => Some(Keep::Keep),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Keep::Me => "me",
            Keep::Wall => "wall",
            Keep::Keep => "keep",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A short camera loop, played at `fps`.
    Loop { fps: u32 },
    Photo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// The fast model, while the guest waits.
    Fast,
    /// The quality model, redrawn while the box is idle.
    Best,
}

impl Quality {
    pub fn name(self) -> &'static str {
        match self {
            Quality::Fast => "fast",
            Quality::Best => "best",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TakeState {
    Queued,
    Running,
    Done,
    Failed(String),
}

pub struct Source {
    pub id: SourceId,
    pub guest: GuestId,
    pub kind: SourceKind,
    pub frames: Vec<Jpeg>,
    /// What the guest typed, if anything.
    pub wish: Option<String>,
    pub keep: Keep,
    /// Takes in the order they were asked for.
    pub takes: Vec<TakeId>,
}

pub struct Take {
    pub id: TakeId,
    pub source: SourceId,
    /// The guest's wish when the take was asked for.
    pub wish: Option<String>,
    /// What the model drew after: the wish and the picture, expanded by the
    /// vision model. `None` until the hub worker has it.
    pub prompt: Option<String>,
    pub quality: Quality,
    pub frames: Vec<Jpeg>,
    pub state: TakeState,
}

struct Guest {
    sources: Vec<SourceId>,
}

/// A disk step that a change needs, run by the caller outside the lock.
#[derive(Debug)]
pub enum Persist {
    Write { path: PathBuf, bytes: Jpeg },
    RemoveDir(PathBuf),
}

impl Persist {
    /// Runs the steps; a failure is reported and the rest still run.
    pub fn run(steps: Vec<Persist>) -> Vec<String> {
        let mut errors = Vec::new();
        for step in steps {
            let result = match &step {
                Persist::Write { path, bytes } => path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| std::fs::write(path, bytes)),
                Persist::RemoveDir(path) => match std::fs::remove_dir_all(path) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    other => other,
                },
            };
            if let Err(e) = result {
                errors.push(format!("{step:?}: {e}"));
            }
        }
        errors
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No such source or take, or not this guest's.
    NotFound,
    /// The pictures held, and those the takes under way may still bring,
    /// fill the budget: nothing was added, nothing held was touched.
    Full,
}

/// A take may come out larger than its source (a loop redrawn at 768 px from
/// 512 px is about twice the bytes): what one is counted at until it is done.
const TAKE_ESTIMATE: usize = 2;

pub struct Store {
    /// Where kept sources go: `<kept>/<source id>/`.
    kept: PathBuf,
    next_id: u64,
    guests: HashMap<GuestId, Guest>,
    tokens: HashMap<String, GuestId>,
    sources: BTreeMap<SourceId, Source>,
    takes: HashMap<TakeId, Take>,
    /// The bytes of pictures the store may hold and promise (see
    /// [`Store::bytes_committed`]).
    budget: usize,
    /// Bumped by every change: the screen redraws when it moves.
    pub version: u64,
}

/// Who asks: a guest may touch only their own sources; the host any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    Guest(GuestId),
    Host,
}

impl Store {
    pub fn new(kept: PathBuf, budget: usize) -> Store {
        Store {
            kept,
            next_id: 1,
            guests: HashMap::new(),
            tokens: HashMap::new(),
            sources: BTreeMap::new(),
            takes: HashMap::new(),
            budget,
            version: 0,
        }
    }

    fn id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn changed(&mut self) {
        self.version += 1;
    }

    /// A new guest and the token their phone sends from now on.
    pub fn join(&mut self, token: String) -> GuestId {
        let id = self.id();
        self.guests.insert(id, Guest { sources: Vec::new() });
        self.tokens.insert(token, id);
        id
    }

    pub fn guest(&self, token: &str) -> Option<GuestId> {
        self.tokens.get(token).copied()
    }

    /// A guest's new source, if the budget has room for it and its first
    /// take (`Full` otherwise: the caller tells the guest to come back).
    pub fn add_source(&mut self, guest: GuestId, kind: SourceKind, frames: Vec<Jpeg>, wish: Option<String>, keep: Keep) -> Result<(SourceId, Vec<Persist>), StoreError> {
        if !self.guests.contains_key(&guest) {
            return Err(StoreError::NotFound);
        }
        let bytes: usize = frames.iter().map(|f| f.len()).sum();
        self.room_for(bytes * (1 + TAKE_ESTIMATE))?;
        let id = self.id();
        self.sources.insert(id, Source { id, guest, kind, frames, wish, keep: Keep::Me, takes: Vec::new() });
        self.guests.get_mut(&guest).unwrap().sources.push(id);
        self.changed();
        let persist = self.set_keep(Who::Guest(guest), id, keep)?;
        Ok((id, persist))
    }

    fn owned(&self, who: Who, source: SourceId) -> Result<&Source, StoreError> {
        match self.sources.get(&source) {
            Some(s) if who == Who::Host || who == Who::Guest(s.guest) => Ok(s),
            _ => Err(StoreError::NotFound),
        }
    }

    pub fn source(&self, who: Who, source: SourceId) -> Result<&Source, StoreError> {
        self.owned(who, source)
    }

    pub fn set_wish(&mut self, who: Who, source: SourceId, wish: Option<String>) -> Result<(), StoreError> {
        self.owned(who, source)?;
        self.sources.get_mut(&source).unwrap().wish = wish;
        self.changed();
        Ok(())
    }

    /// A new wish for `source` and the take that answers it, or neither:
    /// `Full` leaves the wish as it was.
    pub fn wish(&mut self, who: Who, source: SourceId, wish: Option<String>) -> Result<TakeId, StoreError> {
        self.room_for(source_bytes(self.owned(who, source)?) * TAKE_ESTIMATE)?;
        self.set_wish(who, source, wish)?;
        self.add_take(source, Quality::Fast)
    }

    fn kept_dir(&self, source: SourceId) -> PathBuf {
        self.kept.join(source.to_string())
    }

    /// The files a kept source has: its frames and every finished take's.
    fn kept_files(&self, source: &Source) -> Vec<Persist> {
        let dir = self.kept_dir(source.id);
        let mut steps: Vec<Persist> = source
            .frames
            .iter()
            .enumerate()
            .map(|(i, bytes)| Persist::Write { path: dir.join(format!("source-{i:03}.jpg")), bytes: bytes.clone() })
            .collect();
        for take in source.takes.iter().filter_map(|t| self.takes.get(t)).filter(|t| t.state == TakeState::Done) {
            steps.extend(take_files(&dir, take));
        }
        steps
    }

    pub fn set_keep(&mut self, who: Who, source: SourceId, keep: Keep) -> Result<Vec<Persist>, StoreError> {
        let was = self.owned(who, source)?.keep;
        self.sources.get_mut(&source).unwrap().keep = keep;
        self.changed();
        Ok(match (was == Keep::Keep, keep == Keep::Keep) {
            (false, true) => self.kept_files(&self.sources[&source]),
            (true, false) => vec![Persist::RemoveDir(self.kept_dir(source))],
            _ => Vec::new(),
        })
    }

    /// Removes a source and every take of it (and its kept files).
    pub fn delete(&mut self, who: Who, source: SourceId) -> Result<Vec<Persist>, StoreError> {
        self.owned(who, source)?;
        let removed = self.sources.remove(&source).unwrap();
        for take in &removed.takes {
            self.takes.remove(take);
        }
        if let Some(guest) = self.guests.get_mut(&removed.guest) {
            guest.sources.retain(|s| *s != source);
        }
        self.changed();
        Ok(if removed.keep == Keep::Keep { vec![Persist::RemoveDir(self.kept_dir(source))] } else { Vec::new() })
    }

    /// A guest leaves: everything they sent goes, kept files included.
    pub fn leave(&mut self, guest: GuestId) -> Vec<Persist> {
        let sources = self.guests.get(&guest).map(|g| g.sources.clone()).unwrap_or_default();
        let mut steps = Vec::new();
        for source in sources {
            steps.extend(self.delete(Who::Guest(guest), source).unwrap_or_default());
        }
        self.guests.remove(&guest);
        self.tokens.retain(|_, g| *g != guest);
        self.changed();
        steps
    }

    /// A new take of `source` after its wish as it is now, queued for the
    /// hub worker.
    /// `Full` when the budget has no room for what it may bring.
    pub fn add_take(&mut self, source: SourceId, quality: Quality) -> Result<TakeId, StoreError> {
        let s = self.sources.get(&source).ok_or(StoreError::NotFound)?;
        let wish = s.wish.clone();
        self.room_for(source_bytes(s) * TAKE_ESTIMATE)?;
        let id = self.id();
        self.takes.insert(id, Take { id, source, wish, prompt: None, quality, frames: Vec::new(), state: TakeState::Queued });
        self.sources.get_mut(&source).unwrap().takes.push(id);
        self.changed();
        Ok(id)
    }

    pub fn take(&self, take: TakeId) -> Option<&Take> {
        self.takes.get(&take)
    }

    /// The hub worker's progress on a take. A take of a source deleted
    /// meanwhile is gone: `NotFound`, and the worker drops its frames.
    pub fn take_running(&mut self, take: TakeId) -> Result<(), StoreError> {
        let t = self.takes.get_mut(&take).ok_or(StoreError::NotFound)?;
        t.state = TakeState::Running;
        self.changed();
        Ok(())
    }

    pub fn take_prompt(&mut self, take: TakeId, prompt: String) -> Result<(), StoreError> {
        let t = self.takes.get_mut(&take).ok_or(StoreError::NotFound)?;
        t.prompt = Some(prompt);
        self.changed();
        Ok(())
    }

    pub fn take_frame(&mut self, take: TakeId, frame: Jpeg) -> Result<(), StoreError> {
        let t = self.takes.get_mut(&take).ok_or(StoreError::NotFound)?;
        t.frames.push(frame);
        self.changed();
        Ok(())
    }

    /// The take ends: done (and written out when its source is kept) or failed.
    pub fn take_finished(&mut self, take: TakeId, result: Result<(), String>) -> Result<Vec<Persist>, StoreError> {
        let t = self.takes.get_mut(&take).ok_or(StoreError::NotFound)?;
        t.state = match result {
            Ok(()) => TakeState::Done,
            Err(e) => TakeState::Failed(e),
        };
        let source = t.source;
        self.changed();
        let t = &self.takes[&take];
        Ok(match self.sources.get(&source) {
            Some(s) if s.keep == Keep::Keep && t.state == TakeState::Done => take_files(&self.kept_dir(source), t),
            _ => Vec::new(),
        })
    }

    /// A guest's own sources, oldest first.
    pub fn sources_of(&self, guest: GuestId) -> Vec<&Source> {
        self.guests
            .get(&guest)
            .map(|g| g.sources.iter().filter_map(|s| self.sources.get(s)).collect())
            .unwrap_or_default()
    }

    /// What the big screen may show: sources on the wall or kept, oldest
    /// first, each with its best finished take (quality over fast, newest
    /// first), if it has one yet.
    pub fn wall(&self) -> Vec<(&Source, Option<&Take>)> {
        self.sources
            .values()
            .filter(|s| s.keep >= Keep::Wall)
            .map(|s| (s, self.shown_take(s)))
            .collect()
    }

    /// The take the wall shows for `source`: its newest finished one. An
    /// upgrade comes after the take it betters, a guest's new wish after
    /// both, so the newest is the best of the latest wish.
    pub fn shown_take(&self, source: &Source) -> Option<&Take> {
        source.takes.iter().rev().filter_map(|t| self.takes.get(t)).find(|t| t.state == TakeState::Done)
    }

    /// A take that gave way to a guest's (an upgrade the box will ask for
    /// again later): gone as if never asked.
    pub fn remove_take(&mut self, take: TakeId) {
        if let Some(t) = self.takes.remove(&take) {
            if let Some(s) = self.sources.get_mut(&t.source) {
                s.takes.retain(|x| *x != take);
            }
            self.changed();
        }
    }

    /// Whether any take waits or runs.
    pub fn busy(&self) -> bool {
        self.takes.values().any(|t| matches!(t.state, TakeState::Queued | TakeState::Running))
    }

    /// The oldest source on the wall that shows a fast take with no best
    /// quality asked since (a failed one counts as asked: no retries in a
    /// loop).
    pub fn upgrade_candidate(&self) -> Option<SourceId> {
        self.sources
            .values()
            .filter(|s| s.keep >= Keep::Wall)
            .find(|s| {
                let Some(shown) = self.shown_take(s) else { return false };
                let after = s.takes.iter().skip_while(|t| **t != shown.id).skip(1);
                shown.quality == Quality::Fast && !after.filter_map(|t| self.takes.get(t)).any(|t| t.quality == Quality::Best)
            })
            .map(|s| s.id)
    }

    /// Bytes of pictures held, for the log.
    pub fn bytes_held(&self) -> usize {
        let sources: usize = self.sources.values().map(source_bytes).sum();
        let takes: usize = self.takes.values().flat_map(|t| &t.frames).map(|f| f.len()).sum();
        sources + takes
    }

    /// What the budget counts: the pictures held, and for each take still
    /// queued or running what it may yet bring. A take admitted is never
    /// refused its frames: one larger than counted only overshoots.
    pub fn bytes_committed(&self) -> usize {
        let pending: usize = self
            .takes
            .values()
            .filter(|t| matches!(t.state, TakeState::Queued | TakeState::Running))
            .map(|t| {
                let estimate = self.sources.get(&t.source).map_or(0, source_bytes) * TAKE_ESTIMATE;
                estimate.saturating_sub(t.frames.iter().map(|f| f.len()).sum())
            })
            .sum();
        self.bytes_held() + pending
    }

    fn room_for(&self, bytes: usize) -> Result<(), StoreError> {
        if self.bytes_committed().saturating_add(bytes) > self.budget {
            return Err(StoreError::Full);
        }
        Ok(())
    }

    pub fn kept_root(&self) -> &Path {
        &self.kept
    }
}

fn source_bytes(source: &Source) -> usize {
    source.frames.iter().map(|f| f.len()).sum()
}

fn take_files(dir: &Path, take: &Take) -> Vec<Persist> {
    take.frames
        .iter()
        .enumerate()
        .map(|(i, bytes)| Persist::Write { path: dir.join(format!("take-{}-{i:03}.jpg", take.id)), bytes: bytes.clone() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(n: u8) -> Jpeg {
        Arc::from(vec![0xff, 0xd8, n, 0xff, 0xd9])
    }

    #[test]
    fn keep_levels_deletion_and_leaving() {
        let root = std::env::temp_dir().join(format!("party-store-test-{}", std::process::id()));
        let mut store = Store::new(root.clone(), usize::MAX);
        let a = store.join("token-a".into());
        let b = store.join("token-b".into());
        assert_eq!(store.guest("token-a"), Some(a));
        assert_eq!(store.guest("nope"), None);

        // Just me: nothing on the wall, nothing on disk.
        let (mine, persist) = store.add_source(a, SourceKind::Loop { fps: 12 }, vec![jpeg(1), jpeg(2)], None, Keep::Me).unwrap();
        assert!(persist.is_empty(), "a source just for me writes nothing");
        assert!(store.wall().is_empty(), "a source just for me is not on the wall");
        assert_eq!(store.source(Who::Guest(b), mine).err(), Some(StoreError::NotFound), "another guest cannot see it");
        assert_eq!(store.set_keep(Who::Guest(b), mine, Keep::Wall).err(), Some(StoreError::NotFound), "nor change it");

        // A take, then the wall.
        store.set_wish(Who::Guest(a), mine, Some("as a painting".into())).unwrap();
        let fast = store.add_take(mine, Quality::Fast).unwrap();
        assert_eq!(store.take(fast).unwrap().wish.as_deref(), Some("as a painting"), "a take holds the wish it was asked with");
        store.take_running(fast).unwrap();
        store.take_prompt(fast, "a watercolour painting of two people".into()).unwrap();
        store.take_frame(fast, jpeg(11)).unwrap();
        store.take_frame(fast, jpeg(12)).unwrap();
        assert!(store.take_finished(fast, Ok(())).unwrap().is_empty(), "a take of an unkept source writes nothing");
        assert!(store.set_keep(Who::Guest(a), mine, Keep::Wall).unwrap().is_empty(), "the wall writes nothing");
        let wall = store.wall();
        assert_eq!(wall.len(), 1);
        assert_eq!(wall[0].1.map(|t| t.id), Some(fast), "the wall shows the finished take");

        // A better take replaces it on the wall; a failed one does not.
        let best = store.add_take(mine, Quality::Best).unwrap();
        store.take_frame(best, jpeg(21)).unwrap();
        store.take_finished(best, Ok(())).unwrap();
        let failed = store.add_take(mine, Quality::Fast).unwrap();
        store.take_finished(failed, Err("out of memory".into())).unwrap();
        assert_eq!(store.wall()[0].1.map(|t| t.id), Some(best), "quality beats fast, failures never show");

        // The best quality is asked once per wall source; a removed (yielded) take is asked again.
        assert_eq!(store.upgrade_candidate(), None, "the best take is there already");
        assert!(!store.busy());
        let fast_only = store.add_source(a, SourceKind::Photo, vec![jpeg(7)], None, Keep::Wall).unwrap().0;
        assert_eq!(store.upgrade_candidate(), None, "nothing finished to upgrade yet");
        let quick = store.add_take(fast_only, Quality::Fast).unwrap();
        assert!(store.busy(), "a queued take keeps the box busy");
        store.take_frame(quick, jpeg(71)).unwrap();
        store.take_finished(quick, Ok(())).unwrap();
        assert_eq!(store.upgrade_candidate(), Some(fast_only));
        let upgrade = store.add_take(fast_only, Quality::Best).unwrap();
        assert_eq!(store.upgrade_candidate(), None, "asked once");
        store.remove_take(upgrade);
        assert!(store.take(upgrade).is_none() && store.upgrade_candidate() == Some(fast_only), "a yielded upgrade is asked again");
        let upgrade = store.add_take(fast_only, Quality::Best).unwrap();
        store.take_frame(upgrade, jpeg(72)).unwrap();
        store.take_finished(upgrade, Ok(())).unwrap();
        let rewish = store.add_take(fast_only, Quality::Fast).unwrap();
        store.take_frame(rewish, jpeg(73)).unwrap();
        store.take_finished(rewish, Ok(())).unwrap();
        let shown = store.wall().iter().find(|(s, _)| s.id == fast_only).and_then(|(_, t)| t.map(|t| t.id));
        assert_eq!(shown, Some(rewish), "a new wish's take replaces the best one of the old wish");
        assert_eq!(store.upgrade_candidate(), Some(fast_only), "and is bettered in its turn");
        Persist::run(store.delete(Who::Guest(a), fast_only).unwrap());

        // Keep it: every frame and finished take is written; taking it back removes them.
        let written = Persist::run(store.set_keep(Who::Guest(a), mine, Keep::Keep).unwrap());
        assert!(written.is_empty(), "{written:?}");
        let dir = root.join(mine.to_string());
        let mut files: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        files.sort();
        let mut expected = vec!["source-000.jpg".to_string(), "source-001.jpg".into(), format!("take-{best}-000.jpg"), format!("take-{fast}-000.jpg"), format!("take-{fast}-001.jpg")];
        expected.sort();
        assert_eq!(files, expected, "the source's frames and its finished takes, not the failed one");
        let late = store.add_take(mine, Quality::Fast).unwrap();
        store.take_frame(late, jpeg(31)).unwrap();
        Persist::run(store.take_finished(late, Ok(())).unwrap());
        assert!(dir.join(format!("take-{late}-000.jpg")).exists(), "a take finished after keep is written too");
        Persist::run(store.set_keep(Who::Guest(a), mine, Keep::Wall).unwrap());
        assert!(!dir.exists(), "taking keep back removes the files");

        // Deleting removes every take; the worker's late frames find nothing.
        Persist::run(store.set_keep(Who::Guest(a), mine, Keep::Keep).unwrap());
        let pending = store.add_take(mine, Quality::Best).unwrap();
        Persist::run(store.delete(Who::Guest(a), mine).unwrap());
        assert!(!dir.exists(), "deleting a kept source removes its files");
        for take in [fast, best, failed, late, pending] {
            assert!(store.take(take).is_none(), "take {take} went with its source");
        }
        assert_eq!(store.take_frame(pending, jpeg(41)), Err(StoreError::NotFound));
        assert!(store.wall().is_empty());

        // The host may delete anyone's; leaving takes everything of a guest.
        let (theirs, _) = store.add_source(b, SourceKind::Photo, vec![jpeg(5)], Some("a wish".into()), Keep::Wall).unwrap();
        let (kept, persist) = store.add_source(b, SourceKind::Photo, vec![jpeg(6)], None, Keep::Keep).unwrap();
        Persist::run(persist);
        assert!(root.join(kept.to_string()).join("source-000.jpg").exists());
        Persist::run(store.delete(Who::Host, theirs).unwrap());
        Persist::run(store.leave(b));
        assert!(store.guest("token-b").is_none() && store.sources_of(b).is_empty());
        assert!(!root.join(kept.to_string()).exists(), "leaving removes kept files");
        assert_eq!(store.bytes_held(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The budget: a source and its first take need room; a full store
    /// refuses new ones and keeps all it holds; an admitted take gets every
    /// frame; room comes back when something goes.
    #[test]
    fn a_full_store_refuses_new_pictures_and_keeps_the_old() {
        let frame = |n: u8| -> Jpeg { Arc::from(vec![n; 100]) };
        let mut store = Store::new(std::env::temp_dir().join("party-store-budget-unused"), 700);
        let a = store.join("token-a".into());
        let (first, _) = store.add_source(a, SourceKind::Loop { fps: 12 }, vec![frame(1), frame(2)], None, Keep::Me).unwrap();
        let take = store.add_take(first, Quality::Fast).unwrap();
        assert_eq!(store.bytes_committed(), 600, "the source, and its take counted at twice its bytes");
        assert_eq!(store.add_source(a, SourceKind::Photo, vec![frame(3)], None, Keep::Me).err(), Some(StoreError::Full), "no room for a photo and its take");
        assert_eq!(store.add_take(first, Quality::Best).err(), Some(StoreError::Full), "nor for another take");
        assert_eq!(store.wish(Who::Guest(a), first, Some("as a painting".into())).err(), Some(StoreError::Full), "nor for a wish's take");
        assert_eq!(store.source(Who::Host, first).unwrap().wish, None, "and the refused wish changed nothing");
        for n in 0..5 {
            store.take_frame(take, frame(10 + n)).unwrap();
        }
        store.take_finished(take, Ok(())).unwrap();
        assert_eq!(store.bytes_held(), 700, "the admitted take overshoots its count and is kept whole");
        assert_eq!(store.source(Who::Guest(a), first).unwrap().frames.len(), 2, "what was held is untouched");
        store.delete(Who::Guest(a), first).unwrap();
        assert!(store.add_source(a, SourceKind::Photo, vec![frame(3)], None, Keep::Me).is_ok(), "room again once something goes");
    }
}
