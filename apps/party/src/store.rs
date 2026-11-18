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
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// The address it came from: what it holds counts against that
    /// address's share of the budget.
    pub from: IpAddr,
    pub kind: SourceKind,
    pub frames: Vec<Jpeg>,
    /// What the guest typed, if anything.
    pub wish: Option<String>,
    pub keep: Keep,
    /// A wall or keep level asked before the source had a finished take,
    /// applied when one finishes.
    pub wanted: Option<Keep>,
    /// Takes in the order they were asked for.
    pub takes: Vec<TakeId>,
    /// What the vision model suggests redrawing it as (`ideas.rs`).
    pub ideas: Ideas,
}

/// A source's ideas: not asked for yet, being asked for, or the answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Ideas {
    #[default]
    Unasked,
    Asking,
    Ready(Arc<[String]>),
}

/// What an ask for a source's ideas is to do.
#[derive(Debug, PartialEq)]
pub enum IdeasClaim {
    /// Here they are.
    Ready(Arc<[String]>),
    /// Another ask is getting them: wait for the store to change.
    Wait,
    /// This ask gets them, from this picture of the source.
    Ask(Jpeg),
}

impl Source {
    /// Whether the party's screen may show this source's redraws: it is on
    /// the wall or kept, or asked for the wall and waits for its first redraw.
    pub fn on_screen(&self) -> bool {
        self.keep >= Keep::Wall || self.wanted.is_some_and(|w| w >= Keep::Wall)
    }
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
    /// The redrawn frames at the size the model drew them, for the screen.
    pub frames: Vec<Jpeg>,
    /// The same frames at the phone's size, for its preview: the frame
    /// itself when it is no larger.
    pub previews: Vec<Jpeg>,
    pub state: TakeState,
}

impl Take {
    /// Frame `i` at the phone's size.
    pub fn preview(&self, i: usize) -> Option<&Jpeg> {
        self.previews.get(i).or_else(|| self.frames.get(i))
    }

    /// The bytes its frames and previews hold (a preview that is its frame counted once).
    fn bytes(&self) -> usize {
        let frames: usize = self.frames.iter().map(|f| f.len()).sum();
        let previews: usize = self.previews.iter().zip(&self.frames).filter(|(p, f)| !Arc::ptr_eq(p, f)).map(|(p, _)| p.len()).sum();
        frames + previews
    }
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
                Persist::Write { path, bytes } => write_kept(path, bytes),
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

fn write_kept(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        if let Some(parent) = path.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?
            .write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(path, bytes))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No such source or take, or not this guest's.
    NotFound,
    /// The pictures held, and those the takes under way may still bring,
    /// fill the budget: nothing was added, nothing held was touched.
    Full,
    /// As `Full`, for the share of the budget of the address it came from:
    /// others still get in.
    Share,
}

/// What the store may hold and promise: in all, and of what came from any
/// one address, so a few addresses never fill the box for everyone.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub total: usize,
    pub per_address: usize,
}

impl Budget {
    /// `total`, an address's share of it per address.
    pub fn of(total: usize) -> Budget {
        Budget { total, per_address: crate::server::limits::address_share(total) }
    }

    pub const UNLIMITED: Budget = Budget { total: usize::MAX, per_address: usize::MAX };
}

/// A take may come out larger than its source (a loop redrawn at 768 px from
/// 512 px is about twice the bytes): what one is counted at until it is done.
const TAKE_ESTIMATE: usize = 2;
/// Finished takes a source holds besides the one being drawn: a guest who
/// prompts again replaces what they see, and the takes before are not shown
/// again. (A kept source's files of them stay on disk with it.)
pub const TAKES_KEPT: usize = 3;

pub struct Store {
    /// Where kept sources go: `<kept>/<run>-<source id>/`.
    kept: PathBuf,
    run: String,
    next_id: u64,
    guests: HashMap<GuestId, Guest>,
    tokens: HashMap<String, GuestId>,
    sources: BTreeMap<SourceId, Source>,
    takes: HashMap<TakeId, Take>,
    /// The bytes of pictures the store may hold and promise (see
    /// [`Store::bytes_committed`]).
    budget: Budget,
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
    pub fn new(kept: PathBuf, budget: Budget) -> Store {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        let mut bytes = [0u8; 4];
        let run = if makepad_network::tls::os_random(&mut bytes).is_ok() {
            format!("{ms}-{:08x}", u32::from_be_bytes(bytes))
        } else {
            ms.to_string()
        };
        Store {
            kept,
            run,
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

    /// A guest's new source, sent from `from`, if the budget and that
    /// address's share have room for it and its first take (`Full` or
    /// `Share` otherwise: the caller tells the guest to come back).
    pub fn add_source(&mut self, guest: GuestId, from: IpAddr, kind: SourceKind, frames: Vec<Jpeg>, wish: Option<String>, keep: Keep) -> Result<(SourceId, Vec<Persist>), StoreError> {
        if !self.guests.contains_key(&guest) {
            return Err(StoreError::NotFound);
        }
        let bytes: usize = frames.iter().map(|f| f.len()).sum();
        self.room_for(bytes * (1 + TAKE_ESTIMATE), Some(from))?;
        let id = self.id();
        self.sources.insert(id, Source { id, guest, from, kind, frames, wish, keep: Keep::Me, wanted: None, takes: Vec::new(), ideas: Ideas::Unasked });
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

    /// An ask for `source`'s ideas: the first gets to ask the vision model
    /// (about its middle frame), and the source's ideas are being asked for
    /// until [`Store::set_ideas`].
    pub fn claim_ideas(&mut self, who: Who, source: SourceId) -> Result<IdeasClaim, StoreError> {
        let s = self.owned(who, source)?;
        let claim = match (&s.ideas, s.frames.get(s.frames.len() / 2)) {
            (Ideas::Ready(ideas), _) => IdeasClaim::Ready(ideas.clone()),
            (Ideas::Asking, _) => IdeasClaim::Wait,
            (Ideas::Unasked, Some(frame)) => IdeasClaim::Ask(frame.clone()),
            (Ideas::Unasked, None) => IdeasClaim::Ready(Arc::from([])),
        };
        if matches!(claim, IdeasClaim::Ask(_)) {
            self.sources.get_mut(&source).unwrap().ideas = Ideas::Asking;
        }
        Ok(claim)
    }

    /// The vision model's ideas for `source`; `None`: it could not say, and
    /// the next ask tries again.
    pub fn set_ideas(&mut self, source: SourceId, ideas: Option<Vec<String>>) {
        let Some(s) = self.sources.get_mut(&source) else { return };
        s.ideas = ideas.map_or(Ideas::Unasked, |ideas| Ideas::Ready(ideas.into()));
        self.changed();
    }

    pub fn set_wish(&mut self, who: Who, source: SourceId, wish: Option<String>) -> Result<(), StoreError> {
        self.owned(who, source)?;
        self.sources.get_mut(&source).unwrap().wish = wish;
        self.changed();
        Ok(())
    }

    /// A new wish for `source` and the take that answers it, or neither:
    /// `Full` or `Share` (of the address the source came from) leaves the
    /// wish as it was. The new take supersedes the source's unfinished ones
    /// (a queued take goes; a running one stops at its next frame, which
    /// finds it gone), and of its finished ones the newest [`TAKES_KEPT`]
    /// stay.
    pub fn wish(&mut self, who: Who, source: SourceId, wish: Option<String>) -> Result<TakeId, StoreError> {
        let s = self.owned(who, source)?;
        self.room_for(source_bytes(s) * TAKE_ESTIMATE, Some(s.from))?;
        self.set_wish(who, source, wish)?;
        self.supersede(source);
        self.add_take(source, Quality::Fast)
    }

    /// Drops `source`'s unfinished takes and all but its newest
    /// [`TAKES_KEPT`] finished ones.
    fn supersede(&mut self, source: SourceId) {
        let Some(s) = self.sources.get(&source) else { return };
        let mut finished = 0;
        let gone: Vec<TakeId> = s
            .takes
            .iter()
            .rev()
            .filter(|t| match self.takes.get(t).map(|t| &t.state) {
                Some(TakeState::Done | TakeState::Failed(_)) => {
                    finished += 1;
                    finished > TAKES_KEPT
                }
                _ => true,
            })
            .copied()
            .collect();
        for take in gone {
            self.remove_take(take);
        }
    }

    pub(crate) fn kept_dir(&self, source: SourceId) -> PathBuf {
        self.kept.join(format!("{}-{source}", self.run))
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
        let s = self.owned(who, source)?;
        if keep >= Keep::Wall && keep > s.keep && self.shown_take(s).is_none() {
            self.sources.get_mut(&source).unwrap().wanted = Some(keep);
            self.changed();
            return Ok(Vec::new());
        }
        let was = s.keep;
        let s = self.sources.get_mut(&source).unwrap();
        s.wanted = None;
        s.keep = keep;
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
    /// `Full` when the budget has no room for what it may bring. It counts
    /// against its source's address but is not refused by its share: a
    /// guest's new pictures and wishes are what a share holds back.
    pub fn add_take(&mut self, source: SourceId, quality: Quality) -> Result<TakeId, StoreError> {
        let s = self.sources.get(&source).ok_or(StoreError::NotFound)?;
        let wish = s.wish.clone();
        self.room_for(source_bytes(s) * TAKE_ESTIMATE, None)?;
        let id = self.id();
        self.takes.insert(id, Take { id, source, wish, prompt: None, quality, frames: Vec::new(), previews: Vec::new(), state: TakeState::Queued });
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

    pub fn take_frame(&mut self, take: TakeId, frame: Jpeg, preview: Jpeg) -> Result<(), StoreError> {
        let t = self.takes.get_mut(&take).ok_or(StoreError::NotFound)?;
        t.frames.push(frame);
        t.previews.push(preview);
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
        let mut persist = match self.sources.get(&source) {
            Some(s) if s.keep == Keep::Keep && t.state == TakeState::Done => take_files(&self.kept_dir(source), t),
            _ => Vec::new(),
        };
        if t.state == TakeState::Done {
            if let Some(keep) = self.sources.get(&source).and_then(|s| s.wanted) {
                persist.extend(self.set_keep(Who::Host, source, keep)?);
            }
        }
        Ok(persist)
    }

    /// A guest's own sources, oldest first.
    pub fn sources_of(&self, guest: GuestId) -> Vec<&Source> {
        self.guests
            .get(&guest)
            .map(|g| g.sources.iter().filter_map(|s| self.sources.get(s)).collect())
            .unwrap_or_default()
    }

    /// What the big screen may show: sources on the wall or kept that have
    /// a finished take, oldest first, each with its newest finished take.
    pub fn wall(&self) -> Vec<(&Source, &Take)> {
        self.sources
            .values()
            .filter(|s| s.keep >= Keep::Wall)
            .filter_map(|s| self.shown_take(s).map(|t| (s, t)))
            .collect()
    }

    /// The take the wall shows for `source`: its newest finished one. An
    /// upgrade comes after the take it betters, a guest's new wish after
    /// both, so the newest is the best of the latest wish.
    pub fn shown_take(&self, source: &Source) -> Option<&Take> {
        source.takes.iter().rev().filter_map(|t| self.takes.get(t)).find(|t| t.state == TakeState::Done)
    }

    /// The take the party's screen shows next, `seen` the newest it has
    /// shown. A redraw on the screen newer than `seen` comes first, from its
    /// first frame (a best-quality one once finished: it betters a loop the
    /// screen has whole). Else `shown`, while it is drawn, or until its turn
    /// is over while it is still on the screen. Else the wall's next loop
    /// after `shown`'s, in turn.
    pub fn screen_next(&self, shown: Option<TakeId>, seen: TakeId, turn_over: bool) -> Option<TakeId> {
        let fresh = self
            .sources
            .values()
            .filter(|s| s.on_screen())
            .flat_map(|s| s.takes.iter().filter_map(|t| self.takes.get(t)))
            .filter(|t| t.id > seen && !t.frames.is_empty())
            .filter(|t| match t.quality {
                Quality::Fast => matches!(t.state, TakeState::Running | TakeState::Done),
                Quality::Best => t.state == TakeState::Done,
            })
            .map(|t| t.id)
            .max();
        if fresh.is_some() {
            return fresh;
        }
        let current = shown.and_then(|t| self.takes.get(&t)).filter(|t| self.sources.get(&t.source).is_some_and(Source::on_screen));
        if let Some(t) = current {
            if t.state == TakeState::Running || !turn_over {
                return Some(t.id);
            }
        }
        let wall = self.wall();
        let after = current.map(|t| t.source);
        wall.iter().find(|(s, _)| after.is_some_and(|a| s.id > a)).or(wall.first()).map(|(_, t)| t.id)
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
        let takes: usize = self.takes.values().map(Take::bytes).sum();
        sources + takes
    }

    /// What the budget counts: the pictures held, and for each take still
    /// queued or running what it may yet bring. A take admitted is never
    /// refused its frames: one larger than counted only overshoots.
    pub fn bytes_committed(&self) -> usize {
        self.sources.values().map(|s| self.committed(s)).sum()
    }

    /// What the budget counts for what came from `from`.
    pub fn bytes_committed_from(&self, from: IpAddr) -> usize {
        self.sources.values().filter(|s| s.from == from).map(|s| self.committed(s)).sum()
    }

    /// A source, its takes' frames, and what those still queued or running may yet bring.
    fn committed(&self, source: &Source) -> usize {
        let bytes = source_bytes(source);
        let takes: usize = source
            .takes
            .iter()
            .filter_map(|t| self.takes.get(t))
            .map(|t| {
                let held = t.bytes();
                match t.state {
                    TakeState::Queued | TakeState::Running => held.max(bytes * TAKE_ESTIMATE),
                    TakeState::Done | TakeState::Failed(_) => held,
                }
            })
            .sum();
        bytes + takes
    }

    fn room_for(&self, bytes: usize, from: Option<IpAddr>) -> Result<(), StoreError> {
        if self.bytes_committed().saturating_add(bytes) > self.budget.total {
            return Err(StoreError::Full);
        }
        if from.is_some_and(|from| self.bytes_committed_from(from).saturating_add(bytes) > self.budget.per_address) {
            return Err(StoreError::Share);
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

    const HERE: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
    const THERE: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 2));

    fn jpeg(n: u8) -> Jpeg {
        Arc::from(vec![0xff, 0xd8, n, 0xff, 0xd9])
    }

    #[cfg(unix)]
    #[test]
    fn kept_files_and_directories_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("party-store-private-{}-{stamp}", std::process::id()));
        let dir = root.join("kept");
        let path = dir.join("source-000.jpg");
        assert!(!root.exists(), "the kept directory starts fresh");
        let errors = Persist::run(vec![Persist::Write { path: path.clone(), bytes: jpeg(1) }]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "only the owner may read or write a kept file");
        for parent in [&root, &dir] {
            assert_eq!(std::fs::metadata(parent).unwrap().permissions().mode() & 0o777, 0o700, "only the owner may access each new directory");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The party's screen: a new redraw for it from its first frame (a
    /// best-quality one once finished), the loop shown kept for its turn,
    /// then the wall's loops in turn; a guest's own redraws never.
    #[test]
    fn the_screen_shows_each_new_redraw_then_the_wall_in_turn() {
        let mut store = Store::new(std::env::temp_dir().join("party-store-screen-unused"), Budget::UNLIMITED);
        let (a, b) = (store.join("token-a".into()), store.join("token-b".into()));
        let drawn = |store: &mut Store, source: SourceId, quality: Quality, finished: bool| {
            let take = store.add_take(source, quality).unwrap();
            store.take_running(take).unwrap();
            store.take_frame(take, jpeg(11), jpeg(11)).unwrap();
            if finished {
                store.take_finished(take, Ok(())).unwrap();
            }
            take
        };
        let (own, _) = store.add_source(b, THERE, SourceKind::Loop { fps: 8 }, vec![jpeg(1)], None, Keep::Me).unwrap();
        drawn(&mut store, own, Quality::Fast, true);
        assert_eq!(store.screen_next(None, 0, false), None, "a guest's just-me redraw is never on the screen");
        let (first, _) = store.add_source(a, HERE, SourceKind::Loop { fps: 8 }, vec![jpeg(2)], None, Keep::Me).unwrap();
        store.set_keep(Who::Guest(a), first, Keep::Wall).unwrap();
        let one = store.add_take(first, Quality::Fast).unwrap();
        store.take_running(one).unwrap();
        assert_eq!(store.screen_next(None, 0, false), None, "a redraw with no frame yet");
        store.take_frame(one, jpeg(12), jpeg(12)).unwrap();
        assert_eq!(store.screen_next(None, 0, false), Some(one), "a redraw asked for the wall, from its first frame");
        assert_eq!(store.screen_next(Some(one), one, true), Some(one), "kept while it is drawn, turn over or not");
        store.take_finished(one, Ok(())).unwrap();
        assert_eq!(store.screen_next(Some(one), one, false), Some(one), "and for its turn once finished");
        let (second, _) = store.add_source(a, HERE, SourceKind::Loop { fps: 8 }, vec![jpeg(3)], None, Keep::Me).unwrap();
        store.set_keep(Who::Guest(a), second, Keep::Wall).unwrap();
        let two = drawn(&mut store, second, Quality::Fast, true);
        assert_eq!(store.screen_next(Some(one), one, false), Some(two), "a newer redraw comes first");
        assert_eq!(store.screen_next(Some(two), two, true), Some(one), "then the wall's loops in turn");
        assert_eq!(store.screen_next(Some(one), two, true), Some(two), "round");
        let best = drawn(&mut store, first, Quality::Best, false);
        assert_eq!(store.screen_next(Some(two), two, true), Some(one), "a best-quality redraw waits until it is finished");
        store.take_finished(best, Ok(())).unwrap();
        assert_eq!(store.screen_next(Some(one), two, false), Some(best), "then betters its loop at once");
        store.set_keep(Who::Guest(a), first, Keep::Me).unwrap();
        assert_eq!(store.screen_next(Some(best), best, false), Some(two), "a loop taken off the wall leaves the screen at once");
    }

    #[test]
    fn the_wall_waits_for_a_finished_take_and_never_shows_a_raw_source() {
        let mut store = Store::new(std::env::temp_dir().join("party-store-wall-unused"), Budget::UNLIMITED);
        let guest = store.join("token-a".into());
        let (source, _) = store.add_source(guest, HERE, SourceKind::Photo, vec![jpeg(1)], None, Keep::Me).unwrap();
        assert!(store.set_keep(Who::Guest(guest), source, Keep::Wall).unwrap().is_empty(), "asking for the wall writes nothing");
        let s = store.source(Who::Host, source).unwrap();
        assert_eq!((s.keep, s.wanted), (Keep::Me, Some(Keep::Wall)), "the wall level waits for a redraw");
        assert!(store.wall().is_empty(), "the raw source stays off the wall");
        let take = store.add_take(source, Quality::Fast).unwrap();
        store.take_running(take).unwrap();
        store.take_frame(take, jpeg(11), jpeg(11)).unwrap();
        assert!(store.wall().is_empty(), "an unfinished take stays off the wall too");
        assert!(store.take_finished(take, Ok(())).unwrap().is_empty(), "the wall writes nothing when the take finishes");
        let wall = store.wall();
        assert_eq!(wall.len(), 1);
        assert_eq!((wall[0].0.id, wall[0].0.keep, wall[0].0.wanted), (source, Keep::Wall, None));
        assert_eq!(wall[0].1.id, take, "the wall shows the finished redraw");
        store.remove_take(take);
        assert!(store.wall().is_empty(), "losing the finished take must not reveal the raw source");
    }

    #[test]
    fn keep_waits_then_writes_the_source_and_finished_take_once() {
        let mut store = Store::new(std::env::temp_dir().join("party-store-waiting-keep-unused"), Budget::UNLIMITED);
        let guest = store.join("token-a".into());
        let (source, _) = store.add_source(guest, HERE, SourceKind::Loop { fps: 8 }, vec![jpeg(1), jpeg(2)], None, Keep::Me).unwrap();
        assert!(store.set_keep(Who::Guest(guest), source, Keep::Keep).unwrap().is_empty(), "keep writes nothing before a redraw");
        assert_eq!(store.source(Who::Host, source).unwrap().keep, Keep::Me);
        assert!(store.wall().is_empty());
        let take = store.add_take(source, Quality::Fast).unwrap();
        store.take_frame(take, jpeg(11), jpeg(11)).unwrap();
        store.take_frame(take, jpeg(12), jpeg(12)).unwrap();
        let steps = store.take_finished(take, Ok(())).unwrap();
        let files: Vec<_> = steps.into_iter().map(|step| match step {
            Persist::Write { path, bytes } => (path, bytes),
            other => panic!("keeping writes files: {other:?}"),
        }).collect();
        let dir = store.kept_dir(source);
        assert_eq!(files, vec![
            (dir.join("source-000.jpg"), jpeg(1)),
            (dir.join("source-001.jpg"), jpeg(2)),
            (dir.join(format!("take-{take}-000.jpg")), jpeg(11)),
            (dir.join(format!("take-{take}-001.jpg")), jpeg(12)),
        ], "the source and finished take are each written exactly once");
        let s = store.source(Who::Host, source).unwrap();
        assert_eq!((s.keep, s.wanted), (Keep::Keep, None));
        assert_eq!(store.wall()[0].1.id, take);
    }

    #[test]
    fn just_me_cancels_a_waiting_wall_or_keep_level() {
        let mut store = Store::new(std::env::temp_dir().join("party-store-cancel-keep-unused"), Budget::UNLIMITED);
        let guest = store.join("token-a".into());
        for level in [Keep::Wall, Keep::Keep] {
            let (source, _) = store.add_source(guest, HERE, SourceKind::Photo, vec![jpeg(1)], None, Keep::Me).unwrap();
            assert!(store.set_keep(Who::Guest(guest), source, level).unwrap().is_empty());
            assert!(store.set_keep(Who::Guest(guest), source, Keep::Me).unwrap().is_empty());
            assert_eq!(store.source(Who::Host, source).unwrap().wanted, None, "just me cancels {level:?}");
            let take = store.add_take(source, Quality::Fast).unwrap();
            store.take_frame(take, jpeg(11), jpeg(11)).unwrap();
            assert!(store.take_finished(take, Ok(())).unwrap().is_empty(), "the cancelled level writes nothing");
            assert_eq!(store.source(Who::Host, source).unwrap().keep, Keep::Me);
            assert!(store.wall().is_empty(), "the cancelled level never puts the source on the wall");
        }
    }

    #[test]
    fn a_failed_take_leaves_the_level_waiting_for_the_next_take() {
        let mut store = Store::new(std::env::temp_dir().join("party-store-failed-keep-unused"), Budget::UNLIMITED);
        let guest = store.join("token-a".into());
        for level in [Keep::Wall, Keep::Keep] {
            let (source, _) = store.add_source(guest, HERE, SourceKind::Photo, vec![jpeg(1)], None, Keep::Me).unwrap();
            store.set_keep(Who::Guest(guest), source, level).unwrap();
            let failed = store.add_take(source, Quality::Fast).unwrap();
            store.take_frame(failed, jpeg(9), jpeg(9)).unwrap();
            assert!(store.take_finished(failed, Err("redraw failed".into())).unwrap().is_empty());
            let s = store.source(Who::Host, source).unwrap();
            assert_eq!((s.keep, s.wanted), (Keep::Me, Some(level)), "a failed redraw leaves the level waiting");
            assert!(store.wall().is_empty());
            let take = store.add_take(source, Quality::Fast).unwrap();
            store.take_frame(take, jpeg(11), jpeg(11)).unwrap();
            let steps = store.take_finished(take, Ok(())).unwrap();
            assert_eq!(steps.len(), if level == Keep::Keep { 2 } else { 0 }, "only the source and successful take may be written");
            let s = store.source(Who::Host, source).unwrap();
            assert_eq!((s.keep, s.wanted), (level, None));
            assert_eq!(store.wall()[0].1.id, take, "the later successful redraw applies the level");
            store.delete(Who::Guest(guest), source).unwrap();
        }
    }

    #[test]
    fn a_source_is_looked_at_for_ideas_once_and_a_failed_look_is_tried_again() {
        let mut store = Store::new(std::env::temp_dir().join("party-store-ideas-unused"), Budget::UNLIMITED);
        let (a, b) = (store.join("token-a".into()), store.join("token-b".into()));
        let (source, _) = store.add_source(a, HERE, SourceKind::Loop { fps: 8 }, vec![jpeg(1), jpeg(2), jpeg(3)], None, Keep::Me).unwrap();
        assert_eq!(store.claim_ideas(Who::Guest(b), source), Err(StoreError::NotFound), "only its guest asks");
        assert_eq!(store.claim_ideas(Who::Guest(a), source), Ok(IdeasClaim::Ask(jpeg(2))), "the first ask looks at the middle frame");
        assert_eq!(store.claim_ideas(Who::Guest(a), source), Ok(IdeasClaim::Wait), "an ask meanwhile waits for that answer");
        let version = store.version;
        store.set_ideas(source, None);
        assert!(store.version > version, "an answer wakes the asks waiting for it");
        assert_eq!(store.claim_ideas(Who::Guest(a), source), Ok(IdeasClaim::Ask(jpeg(2))), "a look that failed is tried again");
        store.set_ideas(source, Some(vec!["a tiny snow globe".into()]));
        assert_eq!(store.claim_ideas(Who::Guest(a), source), Ok(IdeasClaim::Ready(Arc::from(["a tiny snow globe".to_string()]))), "then every ask gets the answer");
        store.delete(Who::Guest(a), source).unwrap();
        store.set_ideas(source, None);
        assert_eq!(store.claim_ideas(Who::Guest(a), source), Err(StoreError::NotFound), "an answer for a deleted source is let go");
    }

    #[test]
    fn deleting_a_source_discards_its_waiting_level() {
        let mut store = Store::new(std::env::temp_dir().join("party-store-deleted-keep-unused"), Budget::UNLIMITED);
        let guest = store.join("token-a".into());
        for level in [Keep::Wall, Keep::Keep] {
            let (source, steps) = store.add_source(guest, HERE, SourceKind::Photo, vec![jpeg(1)], None, level).unwrap();
            assert!(steps.is_empty());
            let take = store.add_take(source, Quality::Fast).unwrap();
            assert!(store.delete(Who::Guest(guest), source).unwrap().is_empty());
            assert_eq!(store.take_finished(take, Ok(())).err(), Some(StoreError::NotFound), "a deleted source cannot apply its waiting level");
            assert!(store.wall().is_empty());
            assert!(store.sources_of(guest).is_empty());
        }
    }

    #[test]
    fn keep_levels_deletion_and_leaving() {
        let root = std::env::temp_dir().join(format!("party-store-test-{}", std::process::id()));
        let mut store = Store::new(root.clone(), Budget::UNLIMITED);
        let a = store.join("token-a".into());
        let b = store.join("token-b".into());
        assert_eq!(store.guest("token-a"), Some(a));
        assert_eq!(store.guest("nope"), None);

        // Just me: nothing on the wall, nothing on disk.
        let (mine, persist) = store.add_source(a, HERE, SourceKind::Loop { fps: 12 }, vec![jpeg(1), jpeg(2)], None, Keep::Me).unwrap();
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
        store.take_frame(fast, jpeg(11), jpeg(11)).unwrap();
        store.take_frame(fast, jpeg(12), jpeg(12)).unwrap();
        assert!(store.take_finished(fast, Ok(())).unwrap().is_empty(), "a take of an unkept source writes nothing");
        assert!(store.set_keep(Who::Guest(a), mine, Keep::Wall).unwrap().is_empty(), "the wall writes nothing");
        let wall = store.wall();
        assert_eq!(wall.len(), 1);
        assert_eq!(wall[0].1.id, fast, "the wall shows the finished take");

        // A better take replaces it on the wall; a failed one does not.
        let best = store.add_take(mine, Quality::Best).unwrap();
        store.take_frame(best, jpeg(21), jpeg(21)).unwrap();
        store.take_finished(best, Ok(())).unwrap();
        let failed = store.add_take(mine, Quality::Fast).unwrap();
        store.take_finished(failed, Err("out of memory".into())).unwrap();
        assert_eq!(store.wall()[0].1.id, best, "quality beats fast, failures never show");

        // The best quality is asked once per wall source; a removed (yielded) take is asked again.
        assert_eq!(store.upgrade_candidate(), None, "the best take is there already");
        assert!(!store.busy());
        let fast_only = store.add_source(a, HERE, SourceKind::Photo, vec![jpeg(7)], None, Keep::Wall).unwrap().0;
        assert_eq!(store.upgrade_candidate(), None, "nothing finished to upgrade yet");
        let quick = store.add_take(fast_only, Quality::Fast).unwrap();
        assert!(store.busy(), "a queued take keeps the box busy");
        store.take_frame(quick, jpeg(71), jpeg(71)).unwrap();
        store.take_finished(quick, Ok(())).unwrap();
        assert_eq!(store.upgrade_candidate(), Some(fast_only));
        let upgrade = store.add_take(fast_only, Quality::Best).unwrap();
        assert_eq!(store.upgrade_candidate(), None, "asked once");
        store.remove_take(upgrade);
        assert!(store.take(upgrade).is_none() && store.upgrade_candidate() == Some(fast_only), "a yielded upgrade is asked again");
        let upgrade = store.add_take(fast_only, Quality::Best).unwrap();
        store.take_frame(upgrade, jpeg(72), jpeg(72)).unwrap();
        store.take_finished(upgrade, Ok(())).unwrap();
        let rewish = store.add_take(fast_only, Quality::Fast).unwrap();
        store.take_frame(rewish, jpeg(73), jpeg(73)).unwrap();
        store.take_finished(rewish, Ok(())).unwrap();
        let shown = store.wall().iter().find(|(s, _)| s.id == fast_only).map(|(_, t)| t.id);
        assert_eq!(shown, Some(rewish), "a new wish's take replaces the best one of the old wish");
        assert_eq!(store.upgrade_candidate(), Some(fast_only), "and is bettered in its turn");
        Persist::run(store.delete(Who::Guest(a), fast_only).unwrap());

        // Keep it: every frame and finished take is written; taking it back removes them.
        let written = Persist::run(store.set_keep(Who::Guest(a), mine, Keep::Keep).unwrap());
        assert!(written.is_empty(), "{written:?}");
        let dir = store.kept_dir(mine);
        let mut files: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        files.sort();
        let mut expected = vec!["source-000.jpg".to_string(), "source-001.jpg".into(), format!("take-{best}-000.jpg"), format!("take-{fast}-000.jpg"), format!("take-{fast}-001.jpg")];
        expected.sort();
        assert_eq!(files, expected, "the source's frames and its finished takes, not the failed one");
        let late = store.add_take(mine, Quality::Fast).unwrap();
        store.take_frame(late, jpeg(31), jpeg(31)).unwrap();
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
        assert_eq!(store.take_frame(pending, jpeg(41), jpeg(41)), Err(StoreError::NotFound));
        assert!(store.wall().is_empty());

        // The host may delete anyone's; leaving takes everything of a guest.
        let (theirs, _) = store.add_source(b, HERE, SourceKind::Photo, vec![jpeg(5)], Some("a wish".into()), Keep::Wall).unwrap();
        let take = store.add_take(theirs, Quality::Fast).unwrap();
        store.take_frame(take, jpeg(51), jpeg(51)).unwrap();
        store.take_finished(take, Ok(())).unwrap();
        let (kept, persist) = store.add_source(b, HERE, SourceKind::Photo, vec![jpeg(6)], None, Keep::Keep).unwrap();
        Persist::run(persist);
        let take = store.add_take(kept, Quality::Fast).unwrap();
        store.take_frame(take, jpeg(61), jpeg(61)).unwrap();
        Persist::run(store.take_finished(take, Ok(())).unwrap());
        assert!(store.kept_dir(kept).join("source-000.jpg").exists());
        Persist::run(store.delete(Who::Host, theirs).unwrap());
        Persist::run(store.leave(b));
        assert!(store.guest("token-b").is_none() && store.sources_of(b).is_empty());
        assert!(!store.kept_dir(kept).exists(), "leaving removes kept files");
        assert_eq!(store.bytes_held(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_restart_keeps_the_last_runs_kept_files() {
        let root = std::env::temp_dir().join(format!("party-store-restart-{}", std::process::id()));
        let mut first = Store::new(root.clone(), Budget::UNLIMITED);
        let guest = first.join("token-a".into());
        let (source, persist) = first.add_source(guest, HERE, SourceKind::Photo, vec![jpeg(1)], None, Keep::Me).unwrap();
        let errors = Persist::run(persist);
        assert!(errors.is_empty(), "{errors:?}");
        let take = first.add_take(source, Quality::Fast).unwrap();
        first.take_frame(take, jpeg(11), jpeg(11)).unwrap();
        let errors = Persist::run(first.take_finished(take, Ok(())).unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        let errors = Persist::run(first.set_keep(Who::Guest(guest), source, Keep::Keep).unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        let first_dir = first.kept_dir(source);
        drop(first);

        let mut second = Store::new(root.clone(), Budget::UNLIMITED);
        let guest = second.join("token-b".into());
        let (again, persist) = second.add_source(guest, HERE, SourceKind::Photo, vec![jpeg(2)], None, Keep::Me).unwrap();
        let errors = Persist::run(persist);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(again, source, "a restart reuses the first source's id");
        let next_take = second.add_take(again, Quality::Fast).unwrap();
        second.take_frame(next_take, jpeg(12), jpeg(12)).unwrap();
        let errors = Persist::run(second.take_finished(next_take, Ok(())).unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        let errors = Persist::run(second.set_keep(Who::Guest(guest), again, Keep::Keep).unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        let second_dir = second.kept_dir(again);

        assert_ne!(first_dir, second_dir, "each run keeps its own directory");
        assert_eq!(std::fs::read(first_dir.join("source-000.jpg")).unwrap(), jpeg(1).as_ref(), "the first run's source is not overwritten");
        assert_eq!(std::fs::read(first_dir.join(format!("take-{take}-000.jpg"))).unwrap(), jpeg(11).as_ref(), "the first run's take is not overwritten");
        assert_eq!(std::fs::read(second_dir.join("source-000.jpg")).unwrap(), jpeg(2).as_ref(), "the second run keeps its source");
        assert_eq!(std::fs::read(second_dir.join(format!("take-{next_take}-000.jpg"))).unwrap(), jpeg(12).as_ref(), "the second run keeps its take");

        let errors = Persist::run(second.delete(Who::Guest(guest), again).unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!second_dir.exists(), "deleting the second run's source removes its kept files");
        assert_eq!(std::fs::read(first_dir.join("source-000.jpg")).unwrap(), jpeg(1).as_ref(), "deleting in the second run leaves the first run's source");
        assert_eq!(std::fs::read(first_dir.join(format!("take-{take}-000.jpg"))).unwrap(), jpeg(11).as_ref(), "deleting in the second run leaves the first run's take");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The budget: a source and its first take need room; a full store
    /// refuses new ones and keeps all it holds; an admitted take gets every
    /// frame; room comes back when something goes.
    #[test]
    fn a_full_store_refuses_new_pictures_and_keeps_the_old() {
        let frame = |n: u8| -> Jpeg { Arc::from(vec![n; 100]) };
        let mut store = Store::new(std::env::temp_dir().join("party-store-budget-unused"), Budget { total: 700, per_address: 700 });
        let a = store.join("token-a".into());
        let (first, _) = store.add_source(a, HERE, SourceKind::Loop { fps: 12 }, vec![frame(1), frame(2)], None, Keep::Me).unwrap();
        let take = store.add_take(first, Quality::Fast).unwrap();
        assert_eq!(store.bytes_committed(), 600, "the source, and its take counted at twice its bytes");
        assert_eq!(store.add_source(a, HERE, SourceKind::Photo, vec![frame(3)], None, Keep::Me).err(), Some(StoreError::Full), "no room for a photo and its take");
        assert_eq!(store.add_take(first, Quality::Best).err(), Some(StoreError::Full), "nor for another take");
        assert_eq!(store.wish(Who::Guest(a), first, Some("as a painting".into())).err(), Some(StoreError::Full), "nor for a wish's take");
        assert_eq!(store.source(Who::Host, first).unwrap().wish, None, "and the refused wish changed nothing");
        for n in 0..5 {
            let f = frame(10 + n);
            store.take_frame(take, f.clone(), f).unwrap();
        }
        store.take_finished(take, Ok(())).unwrap();
        assert_eq!(store.bytes_held(), 700, "the admitted take overshoots its count and is kept whole");
        assert_eq!(store.source(Who::Guest(a), first).unwrap().frames.len(), 2, "what was held is untouched");
        store.delete(Who::Guest(a), first).unwrap();
        assert!(store.add_source(a, HERE, SourceKind::Photo, vec![frame(3)], None, Keep::Me).is_ok(), "room again once something goes");
    }

    /// Prompting again and again: the newest wish's take is the one drawn,
    /// the takes it supersedes go (a running one finds itself gone at its
    /// next frame), and a source holds only its newest few finished takes.
    #[test]
    fn a_new_wish_supersedes_and_old_takes_go() {
        let root = std::env::temp_dir().join(format!("party-store-supersede-{}", std::process::id()));
        let mut store = Store::new(root.clone(), Budget::UNLIMITED);
        let a = store.join("token-a".into());
        let (mine, _) = store.add_source(a, HERE, SourceKind::Loop { fps: 8 }, vec![jpeg(1)], None, Keep::Me).unwrap();
        let first = store.add_take(mine, Quality::Fast).unwrap();
        store.take_running(first).unwrap();
        let queued = store.wish(Who::Guest(a), mine, Some("as a painting".into())).unwrap();
        let newest = store.wish(Who::Guest(a), mine, Some("on a stage".into())).unwrap();
        assert!(store.take(first).is_none() && store.take(queued).is_none(), "a new wish supersedes the running and the queued take");
        assert_eq!(store.take_frame(first, jpeg(9), jpeg(9)).err(), Some(StoreError::NotFound), "the running one stops at its next frame");
        assert_eq!(store.source(Who::Host, mine).unwrap().takes, vec![newest]);

        let mut asked = vec![newest];
        store.take_finished(newest, Ok(())).unwrap();
        for _ in 0..5 {
            let take = store.wish(Who::Guest(a), mine, None).unwrap();
            store.take_finished(take, Ok(())).unwrap();
            asked.push(take);
        }
        let held = store.source(Who::Host, mine).unwrap().takes.clone();
        assert_eq!(held, asked[asked.len() - TAKES_KEPT - 1..].to_vec(), "the newest few finished takes stay");
        assert_eq!(store.shown_take(store.source(Who::Host, mine).unwrap()).map(|t| t.id), asked.last().copied());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// One address fills only its share: past it, its new pictures and
    /// wishes wait (whichever guest sends them), another address still gets
    /// in, and nothing held is dropped.
    #[test]
    fn one_address_fills_only_its_share() {
        let frame = |n: u8| -> Jpeg { Arc::from(vec![n; 100]) };
        let mut store = Store::new(std::env::temp_dir().join("party-store-share-unused"), Budget { total: 2000, per_address: 700 });
        let (a, b) = (store.join("token-a".into()), store.join("token-b".into()));
        let (first, _) = store.add_source(a, HERE, SourceKind::Loop { fps: 12 }, vec![frame(1), frame(2)], None, Keep::Me).unwrap();
        store.add_take(first, Quality::Fast).unwrap();
        assert_eq!(store.bytes_committed_from(HERE), 600);
        assert_eq!(store.add_source(a, HERE, SourceKind::Photo, vec![frame(3)], None, Keep::Me).err(), Some(StoreError::Share), "past its share an address waits");
        assert_eq!(store.add_source(b, HERE, SourceKind::Photo, vec![frame(3)], None, Keep::Me).err(), Some(StoreError::Share), "whichever guest sends from it");
        assert_eq!(store.wish(Who::Guest(a), first, Some("as a painting".into())).err(), Some(StoreError::Share), "a wish's take too");
        assert!(store.add_source(b, THERE, SourceKind::Photo, vec![frame(4)], None, Keep::Me).is_ok(), "another address still gets in");
        assert_eq!(store.source(Who::Guest(a), first).unwrap().frames.len(), 2, "nothing held is dropped");
        assert_eq!(store.source(Who::Host, first).unwrap().wish, None, "and the refused wish changed nothing");
        store.delete(Who::Guest(a), first).unwrap();
        assert!(store.add_source(a, HERE, SourceKind::Photo, vec![frame(3)], None, Keep::Me).is_ok(), "its share comes back once something goes");
    }
}
