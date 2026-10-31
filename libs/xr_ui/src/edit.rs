//! Grab-to-edit: one gesture is one edit is one undo step. While a grab lasts the app shows its
//! items moved (optimistically, by its own deltas); when it lets go the app's sink gets one edit
//! for everything it moved. The moved items then wait for the document's rebuild that carries
//! that edit (the job it went out as), so they never jump back in between.

use std::collections::VecDeque;

/// Where an app's headset edits go: its own edit path (one undo step per commit).
pub trait EditSink<E> {
    /// A gesture in progress (nothing committed). Optional: most apps show the move locally.
    fn live(&mut self, _edit: &E) {}
    /// A finished gesture: one edit, one undo step.
    fn commit(&mut self, edit: E);
}

/// Items (by index) whose finished grab waits for the document to catch up.
#[derive(Debug, Default)]
pub struct GrabEdits {
    /// The job that carries each item's last commit (None: none outstanding, or not sent yet).
    job: Vec<Option<u64>>,
    /// Items committed but not yet sent, true while they wait.
    waiting: Vec<bool>,
    /// Finished grabs not yet given a job, oldest first.
    unsent: VecDeque<Vec<usize>>,
}

impl GrabEdits {
    /// A new set of `items` (nothing outstanding).
    pub fn reset(&mut self, items: usize) {
        self.job = vec![None; items];
        self.waiting = vec![false; items];
        self.unsent.clear();
    }

    /// The same items, `items` of them now.
    pub fn resize(&mut self, items: usize) {
        self.job.resize(items, None);
        self.waiting.resize(items, false);
    }

    /// The item's committed edit has not reached the document yet.
    pub fn waiting(&self, item: usize) -> bool {
        self.waiting.get(item).copied().unwrap_or(false)
    }

    pub fn any_waiting(&self) -> bool {
        self.waiting.iter().any(|w| *w)
    }

    /// A finished gesture moved `items`: `edit` goes to the sink as one edit.
    pub fn commit<E>(&mut self, items: Vec<usize>, edit: E, sink: &mut impl EditSink<E>) {
        for &i in &items {
            if let Some(w) = self.waiting.get_mut(i) {
                *w = true;
            }
        }
        self.unsent.push_back(items);
        sink.commit(edit);
    }

    /// The app sent the oldest finished gesture as job `job`.
    pub fn sent(&mut self, job: u64) {
        if let Some(items) = self.unsent.pop_front() {
            for i in items {
                if let Some(j) = self.job.get_mut(i) {
                    *j = Some(job);
                }
            }
        }
    }

    /// The document was rebuilt by `job`: items whose edit it carries are settled (unless
    /// `held`: a hand took them again), and `settled` is called for each.
    pub fn settle(&mut self, job: u64, held: impl Fn(usize) -> bool, mut settled: impl FnMut(usize)) {
        for i in 0..self.job.len() {
            if self.job[i].is_some_and(|j| j <= job) && !held(i) {
                self.job[i] = None;
                self.waiting[i] = false;
                settled(i);
            }
        }
    }
}
