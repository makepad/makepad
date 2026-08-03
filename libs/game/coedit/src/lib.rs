//! Host-serialized collaborative editing for Makepad Arcade (game.md
//! §"Collaborative editing (multi-Claude)").
//!
//! Every player's machine may run its own Claude editing the same game. This is
//! deliberately **not** a CRDT: text CRDTs guarantee convergence, not
//! correctness, and two AIs restructuring overlapping code would char-merge into
//! valid-looking nonsense. Our edits are chunky transactions seconds apart and
//! the host is already authoritative, so:
//!
//! - the host **orders** submissions; the source is an append-only generation
//!   history (generation N = the source after N accepted transactions),
//! - disjoint hunks **3-way auto-merge**,
//! - overlapping ones get a **semantic rebase**: the submitter is handed the new
//!   base and re-derives its diff, because the AI that wrote the change is the
//!   thing best placed to resolve it,
//! - an accepted transaction is evaluated by the host, and eval or runtime
//!   errors go back to **the proposer**, not the room.
//!
//! Clients never need the source at runtime — the sim is host-authoritative —
//! so this whole protocol is Claude↔host.

pub mod diff3;
pub mod lease;

pub use diff3::{Hunk, Merge};
pub use lease::{Lease, LeaseOutcome, LeaseTable};

use std::collections::VecDeque;

/// Who is proposing. The host's own agent is an author like any other: routing
/// local edits down a privileged path would leave the merge logic untested by
/// the case that actually runs every day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AuthorId(pub u64);

impl AuthorId {
    /// The agent running on the host device.
    pub const LOCAL: AuthorId = AuthorId(0);
}

/// A proposed edit.
///
/// `source` is the author's whole intended file rather than a patch: the diff
/// against `base_generation` is derived here, which keeps a malformed or stale
/// patch from ever being applied, and an AI writes whole files anyway. It is
/// bounded by [`Limits::max_source_bytes`].
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub author: AuthorId,
    pub intent: String,
    pub base_generation: u64,
    pub source: String,
}

/// One accepted state of the game source.
#[derive(Clone, Debug, PartialEq)]
pub struct Generation {
    pub number: u64,
    pub source: String,
    pub author: AuthorId,
    pub intent: String,
    /// Which generation the author actually wrote against. Equal to
    /// `number - 1` unless the edit merged cleanly over newer work.
    pub base_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    EmptyIntent,
    IntentTooLong,
    SourceTooLong,
    /// A base generation this host has never issued (or one already pruned).
    UnknownBase,
    /// This author already has the maximum submissions waiting.
    QueueFull,
    /// The source is byte-identical to the base — nothing to record.
    NoChange,
}

/// What the host decided about a submission.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Accepted {
        generation: u64,
        source: String,
    },
    /// The author must re-apply its intent against `base_source`. Carries what
    /// moved underneath so the AI can re-derive rather than guess.
    Rebase {
        generation: u64,
        base_source: String,
        intervening: Vec<ChangeSummary>,
        conflict_regions: usize,
    },
    Refused {
        reason: Refusal,
    },
}

/// A one-line account of an intervening generation, for the rebase payload.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeSummary {
    pub generation: u64,
    pub author: AuthorId,
    pub intent: String,
    pub hunks: Vec<Hunk>,
}

/// An eval failure addressed back to whoever proposed the generation.
#[derive(Clone, Debug, PartialEq)]
pub struct EvalReport {
    pub author: AuthorId,
    pub generation: u64,
    pub intent: String,
    pub message: String,
    /// The source the world is actually running while this is broken.
    pub last_good_generation: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_source_bytes: usize,
    pub max_intent_bytes: usize,
    pub max_pending_per_author: usize,
    pub max_leases: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Matches `makepad_game_net::protocol::MAX_COEDIT_SOURCE`: a source
            // the log accepts must still fit in the frame that hands it back to
            // a rebasing author.
            max_source_bytes: 192 * 1024,
            max_intent_bytes: 4096,
            max_pending_per_author: 4,
            max_leases: 32,
        }
    }
}

/// The append-only history. Generation 0 is the source the room started with.
#[derive(Clone, Debug)]
pub struct History {
    generations: Vec<Generation>,
}

impl History {
    pub fn new(initial: String) -> Self {
        Self {
            generations: vec![Generation {
                number: 0,
                source: initial,
                author: AuthorId::LOCAL,
                intent: String::new(),
                base_generation: 0,
            }],
        }
    }

    pub fn head(&self) -> &Generation {
        self.generations.last().expect("history is never empty")
    }

    pub fn head_number(&self) -> u64 {
        self.head().number
    }

    pub fn get(&self, number: u64) -> Option<&Generation> {
        self.generations.get(number as usize)
    }

    pub fn len(&self) -> usize {
        self.generations.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn iter(&self) -> impl Iterator<Item = &Generation> {
        self.generations.iter()
    }

    fn push(&mut self, generation: Generation) {
        debug_assert_eq!(
            generation.number,
            self.generations.len() as u64,
            "history must stay linear and append-only"
        );
        self.generations.push(generation);
    }
}

/// The host side of the intent log.
pub struct CoeditHost {
    history: History,
    leases: LeaseTable,
    limits: Limits,
    /// Bounded intake so a submitting peer cannot grow host memory between
    /// pumps; drained in arrival order, which *is* the serialization order.
    queue: VecDeque<Transaction>,
    /// Last generation whose eval succeeded. The world runs this while a newer
    /// generation is broken — the same last-good-rollback the editor already
    /// does, so the proposer can fix its own mistake against its own text.
    last_good: u64,
}

impl CoeditHost {
    pub fn new(initial_source: impl Into<String>) -> Self {
        Self::with_limits(initial_source, Limits::default())
    }

    pub fn with_limits(initial_source: impl Into<String>, limits: Limits) -> Self {
        Self {
            history: History::new(initial_source.into()),
            leases: LeaseTable::new(limits.max_leases),
            limits,
            queue: VecDeque::new(),
            last_good: 0,
        }
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn head(&self) -> &Generation {
        self.history.head()
    }

    /// The source the world should actually be running.
    pub fn last_good_source(&self) -> &str {
        &self
            .history
            .get(self.last_good)
            .expect("last_good is always a real generation")
            .source
    }

    pub fn last_good_generation(&self) -> u64 {
        self.last_good
    }

    pub fn leases(&mut self) -> &mut LeaseTable {
        &mut self.leases
    }

    // ---------------------------------------------------------------- intake

    /// Queue a submission for this pump. Bounded per author.
    pub fn enqueue(&mut self, tx: Transaction) -> Result<(), Refusal> {
        let pending = self
            .queue
            .iter()
            .filter(|queued| queued.author == tx.author)
            .count();
        if pending >= self.limits.max_pending_per_author {
            return Err(Refusal::QueueFull);
        }
        self.queue.push_back(tx);
        Ok(())
    }

    /// Process everything queued, in arrival order.
    pub fn process_queue(&mut self) -> Vec<(AuthorId, Outcome)> {
        let mut out = Vec::new();
        while let Some(tx) = self.queue.pop_front() {
            let author = tx.author;
            out.push((author, self.submit(tx)));
        }
        out
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    // ------------------------------------------------------------- the merge

    /// Order, merge and record one transaction.
    pub fn submit(&mut self, tx: Transaction) -> Outcome {
        if let Some(reason) = self.validate(&tx) {
            return Outcome::Refused { reason };
        }

        let base = self
            .history
            .get(tx.base_generation)
            .expect("validated above")
            .clone();
        let head = self.history.head().clone();

        // Wrote against the tip: nothing to merge.
        if base.number == head.number {
            return self.accept(tx, base.number);
        }

        match diff3::merge3(&base.source, &tx.source, &head.source) {
            Merge::Clean(merged) => {
                if merged == head.source {
                    // The edit is already present in the tip (identical work, or
                    // an intent someone else landed first).
                    return Outcome::Refused {
                        reason: Refusal::NoChange,
                    };
                }
                let mut merged_tx = tx;
                merged_tx.source = merged;
                self.accept(merged_tx, base.number)
            }
            Merge::Conflict { regions } => Outcome::Rebase {
                generation: head.number,
                base_source: head.source.clone(),
                intervening: self.summarize(base.number, head.number),
                conflict_regions: regions,
            },
        }
    }

    fn validate(&self, tx: &Transaction) -> Option<Refusal> {
        if tx.intent.trim().is_empty() {
            return Some(Refusal::EmptyIntent);
        }
        if tx.intent.len() > self.limits.max_intent_bytes {
            return Some(Refusal::IntentTooLong);
        }
        if tx.source.len() > self.limits.max_source_bytes {
            return Some(Refusal::SourceTooLong);
        }
        // `?` here would report "no refusal" for a base we never issued.
        let Some(base) = self.history.get(tx.base_generation) else {
            return Some(Refusal::UnknownBase);
        };
        if base.source == tx.source {
            return Some(Refusal::NoChange);
        }
        None
    }

    fn accept(&mut self, tx: Transaction, base_generation: u64) -> Outcome {
        let number = self.history.head_number() + 1;
        let generation = Generation {
            number,
            source: tx.source,
            author: tx.author,
            intent: tx.intent,
            base_generation,
        };
        let source = generation.source.clone();
        self.history.push(generation);
        Outcome::Accepted { generation: number, source }
    }

    /// What landed between an author's base and the current tip.
    fn summarize(&self, from: u64, to: u64) -> Vec<ChangeSummary> {
        let mut out = Vec::new();
        for number in (from + 1)..=to {
            let (Some(previous), Some(generation)) =
                (self.history.get(number - 1), self.history.get(number))
            else {
                continue;
            };
            out.push(ChangeSummary {
                generation: number,
                author: generation.author,
                intent: generation.intent.clone(),
                hunks: diff3::hunks(&previous.source, &generation.source),
            });
        }
        out
    }

    // ------------------------------------------------------------ eval results

    /// The host evaluated a generation successfully; it becomes last-good.
    pub fn note_eval_ok(&mut self, generation: u64) {
        if generation > self.last_good && self.history.get(generation).is_some() {
            self.last_good = generation;
        }
    }

    /// The host failed to evaluate a generation. The world stays on last-good
    /// and the failure is addressed to whoever proposed it — never broadcast to
    /// the room, which did not write it and cannot fix it.
    pub fn note_eval_error(&mut self, generation: u64, message: impl Into<String>) -> Option<EvalReport> {
        let generation = self.history.get(generation)?;
        Some(EvalReport {
            author: generation.author,
            generation: generation.number,
            intent: generation.intent.clone(),
            message: message.into(),
            last_good_generation: self.last_good,
        })
    }

    /// An author disconnected: drop its leases and any queued work.
    pub fn forget_author(&mut self, author: AuthorId) {
        self.leases.release_author(author);
        self.queue.retain(|tx| tx.author != author);
    }
}

#[cfg(test)]
mod tests;
