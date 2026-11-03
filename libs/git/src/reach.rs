//! Everything one tip reaches, kept current as the tip moves, so ancestry
//! questions against it cost a set lookup:
//!
//! - [`Reach::contains`]: `git merge-base --is-ancestor <oid> <tip>`
//! - [`Reach::range`]: `git rev-list <head> --not <tip>`, plus the commits
//!   of the tip's history the walk stopped at (`<tip>` itself among them
//!   exactly when `head` contains the tip).
//!
//! [`is_ancestor`] answers the same for two commits with nothing kept.
//!
//! Moving the tip forward reads only the new commits; a tip that is no
//! descendant of the old one is read again from scratch. Shallow boundaries
//! (`<common>/shallow`) end every walk.
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::{commit::Commit, error::GitError, oid::ObjectId, repo::Repository};

#[derive(Debug, Default)]
pub struct Reach {
    tip: Option<ObjectId>,
    set: HashSet<ObjectId>,
}

/// The commits `head` has beyond a [`Reach`]'s tip.
#[derive(Debug, Default)]
pub struct Range {
    /// newest committer time first
    pub commits: Vec<(ObjectId, Commit)>,
    /// commits of the tip's history the walk reached
    pub boundary: Vec<ObjectId>,
}

impl Reach {
    pub fn new() -> Reach {
        Reach::default()
    }

    pub fn tip(&self) -> Option<ObjectId> {
        self.tip
    }

    /// Commits the tip reaches, the tip included.
    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Whether `oid` is the tip or one of its ancestors.
    pub fn contains(&self, oid: &ObjectId) -> bool {
        self.set.contains(oid)
    }

    /// Moves the tip to `tip`.
    pub fn advance(&mut self, repo: &mut Repository, tip: ObjectId) -> Result<(), GitError> {
        if self.tip == Some(tip) {
            return Ok(());
        }
        let shallow = repo.shallow_boundary()?;
        let old = self.tip;
        let mut fresh = HashSet::new();
        let mut met_old = old.is_none();
        let mut stack = vec![tip];
        while let Some(oid) = stack.pop() {
            if self.set.contains(&oid) {
                // the old set is closed under parents: only reaching the old tip itself proves
                // it is an ancestor (any other stop could be shared history of a side chain)
                met_old |= Some(oid) == old;
                continue;
            }
            if !fresh.insert(oid) || shallow.contains(&oid) {
                continue;
            }
            stack.extend(repo.read_commit(&oid)?.parents);
        }
        if met_old {
            self.set.extend(fresh);
        } else {
            // not a descendant: the old history may hold commits the new tip does not reach
            self.set.clear();
            self.tip = None;
            return self.advance(repo, tip);
        }
        self.tip = Some(tip);
        Ok(())
    }

    /// The commits `head` reaches that the tip does not.
    pub fn range(&self, repo: &mut Repository, head: ObjectId) -> Result<Range, GitError> {
        Reach::range_beyond(&[self], repo, head)
    }

    /// The commits `head` reaches that none of `reaches` does (`git rev-list <head> --not
    /// <tips>`); the boundary holds the commits of theirs the walk reached.
    pub fn range_beyond(reaches: &[&Reach], repo: &mut Repository, head: ObjectId) -> Result<Range, GitError> {
        let shallow = repo.shallow_boundary()?;
        let mut range = Range::default();
        let mut seen = HashSet::new();
        let mut stack = vec![head];
        while let Some(oid) = stack.pop() {
            if !seen.insert(oid) {
                continue;
            }
            if reaches.iter().any(|r| r.set.contains(&oid)) {
                range.boundary.push(oid);
                continue;
            }
            let commit = repo.read_commit(&oid)?;
            if !shallow.contains(&oid) {
                stack.extend(commit.parents.iter().copied());
            }
            range.commits.push((oid, commit));
        }
        range.commits.sort_by(|a, b| b.1.committer.timestamp.cmp(&a.1.committer.timestamp));
        Ok(range)
    }
}

/// Whether `a` is `b` or one of its ancestors (`git merge-base --is-ancestor a b`), exactly.
///
/// Both histories are walked newest first. Whatever `a` reaches cannot lead to `a` (history has
/// no cycles), so the walk from `b` stops where it meets `a`'s history and ends once no commit of
/// its own is left. Commit times only order the walk: a clock that ran backwards costs reads,
/// never a wrong answer. The commits read are about those either side has beyond the other.
pub fn is_ancestor(repo: &mut Repository, a: ObjectId, b: ObjectId) -> Result<bool, GitError> {
    if a == b {
        return Ok(true);
    }
    const A: u8 = 1;
    const B: u8 = 2;
    let shallow = repo.shallow_boundary()?;
    // commit -> (its time, the parents the walk follows)
    let mut read: HashMap<ObjectId, (i64, Vec<ObjectId>)> = HashMap::new();
    fn time(repo: &mut Repository, read: &mut HashMap<ObjectId, (i64, Vec<ObjectId>)>, shallow: &HashSet<ObjectId>, oid: ObjectId) -> Result<i64, GitError> {
        if let Some((t, _)) = read.get(&oid) {
            return Ok(*t);
        }
        let c = repo.read_commit(&oid)?;
        let parents = if shallow.contains(&oid) { Vec::new() } else { c.parents };
        read.insert(oid, (c.committer.timestamp, parents));
        Ok(c.committer.timestamp)
    }
    let mut flags: HashMap<ObjectId, u8> = HashMap::from([(a, A), (b, B)]);
    let mut heap = BinaryHeap::new();
    for oid in [a, b] {
        heap.push((time(repo, &mut read, &shallow, oid)?, oid));
    }
    // `b`'s own commits: reached from `b`, not (yet) known to be in `a`'s history
    while heap.iter().any(|(_, o)| flags[o] == B) {
        let Some((_, oid)) = heap.pop() else { break };
        let f = flags[&oid];
        // (every queued commit was read for its time)
        let parents = read[&oid].1.clone();
        for p in parents {
            let old = flags.get(&p).copied().unwrap_or(0);
            let new = old | f;
            if new == old {
                continue;
            }
            if p == a && new & B != 0 {
                return Ok(true);
            }
            flags.insert(p, new);
            heap.push((time(repo, &mut read, &shallow, p)?, p));
        }
    }
    Ok(false)
}
