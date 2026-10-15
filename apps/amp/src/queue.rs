//! Our own play order: what plays now, what the person queued by hand, and
//! the rest of the list playback started from, in the order that list was
//! shown (a playlist sorted by # falling plays newest first).
//!
//! The app owns this order; the music system only ever holds its next few
//! tracks. The control worker keeps the system's own queue equal to
//! [`PlayQueue::desired`] ([`reconcile`] gives the fewest commands for
//! that), so a track that ends is followed by ours without the app having
//! to be quick. Every track change the app sees is checked against the
//! queue ([`PlayQueue::observe`]): the next one advances it, a skip in the
//! system's own window skips along, and anything else — the person played
//! something in the system itself — detaches the queue: it stops steering
//! until something is played from the app again.
//!
//! The model follows Spotify's: tracks queued by hand play before the rest
//! of the list, "Play Next" puts one at the front, and playing another list
//! replaces the list but keeps what was queued by hand.
//!
//! The queue is kept between runs ([`PlayQueue::serialize`]). A queue read
//! back is *parked*: it shows, but does not steer, so starting the app never
//! changes what the player plays or holds. Play, Next, Previous or a jump
//! wakes it, and so does the player turning up on the parked track by
//! itself.

/// Tracks kept to go back to with Previous.
const HISTORY: usize = 200;
/// Of those, kept between runs.
const SAVED_HISTORY: usize = 50;
/// The first line of a saved queue.
const HEADER: &str = "music-queue 1";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueItem {
    pub uri: String,
    pub name: String,
    pub artist: String,
}

/// Where an upcoming track sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// Queued by hand: plays before the rest of the list.
    User,
    /// The rest of the list playback started from.
    List,
}

/// What a track change seen in the player meant for the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observed {
    /// The track the queue says is playing.
    Same,
    /// A track further on in the queue: the queue moved on to it.
    Advanced,
    /// The track before this one: the player went back.
    Back,
    /// The player ran on into its own next track (a track ended before ours
    /// was in its queue): the queue moved on to its own next, which the
    /// caller plays instead.
    Overran,
    /// Something the queue did not ask for: it stopped steering.
    Detached,
    /// The parked queue's own track plays: it steers again.
    Resumed,
    /// The queue is not steering; nothing to follow.
    Ignored,
}

#[derive(Clone, Debug, Default)]
pub struct PlayQueue {
    pub current: Option<QueueItem>,
    pub user: Vec<QueueItem>,
    pub list: Vec<QueueItem>,
    /// What `list` came from, in words ("Starred", "Song Radio: Toxygene").
    pub source: String,
    history: Vec<QueueItem>,
    /// Whether the player follows this queue.
    pub driving: bool,
    /// Read back from the last run and not steering yet: see the module.
    pub parked: bool,
}

impl PlayQueue {
    /// Play `items[start]`, then the rest of `items` after it; what was
    /// queued by hand stays in front of them. Returns the track to play.
    pub fn play_list(&mut self, mut items: Vec<QueueItem>, start: usize, source: &str) -> Option<QueueItem> {
        if start >= items.len() {
            return None;
        }
        let rest = items.split_off(start + 1);
        let first = items.pop()?;
        self.leave_current();
        self.current = Some(first.clone());
        self.list = rest;
        self.source = source.to_string();
        self.drive();
        Some(first)
    }

    /// Play one track on its own: the list ends with it.
    pub fn play_one(&mut self, item: QueueItem) -> QueueItem {
        self.play_list(vec![item.clone()], 0, "").unwrap_or(item)
    }

    /// Steer again from `now`, the track the player plays already: what is
    /// queued by hand plays after it, the old list is gone. Used when the
    /// person queues something while the queue is not steering.
    pub fn adopt(&mut self, now: QueueItem) {
        // A parked queue is kept whole: it steers again only from its own
        // track.
        if self.parked {
            if self.current.as_ref().is_some_and(|current| current.uri == now.uri) {
                self.drive();
            }
            return;
        }
        if !self.driving || self.current.is_none() {
            self.current = Some(now);
            self.list.clear();
            self.source.clear();
            self.drive();
        }
    }

    /// Steer: the player follows this queue from now on.
    pub fn drive(&mut self) {
        self.driving = true;
        self.parked = false;
    }

    /// Stop steering: the player plays something the queue does not hold
    /// (a whole playlist or album of the system's own).
    pub fn detach(&mut self) {
        self.driving = false;
        self.parked = false;
    }

    pub fn enqueue(&mut self, items: Vec<QueueItem>) {
        self.user.extend(items);
    }

    pub fn play_next(&mut self, item: QueueItem) {
        self.user.insert(0, item);
    }

    /// Everything that plays after the current track, in order.
    pub fn upcoming(&self) -> impl Iterator<Item = (Section, &QueueItem)> {
        self.user.iter().map(|item| (Section::User, item)).chain(self.list.iter().map(|item| (Section::List, item)))
    }

    pub fn upcoming_len(&self) -> usize {
        self.user.len() + self.list.len()
    }

    /// The next `count` tracks the player should hold in its own queue, up
    /// to the first it cannot hold (`holds` says which it can: a local file
    /// plays in this app); none while the queue is not steering.
    pub fn desired(&self, count: usize, holds: impl Fn(&QueueItem) -> bool) -> Vec<String> {
        if !self.driving {
            return Vec::new();
        }
        self.upcoming().map(|(_, item)| item).take_while(|item| holds(item)).take(count).map(|item| item.uri.clone()).collect()
    }

    fn leave_current(&mut self) {
        if let Some(current) = self.current.take() {
            self.history.push(current);
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
    }

    /// Take the upcoming track at `at` (0-based over [`Self::upcoming`]).
    fn take(&mut self, at: usize) -> Option<QueueItem> {
        if at < self.user.len() {
            Some(self.user.remove(at))
        } else if at - self.user.len() < self.list.len() {
            Some(self.list.remove(at - self.user.len()))
        } else {
            None
        }
    }

    /// Play the upcoming track at `at`: everything before it counts as
    /// played. Returns the track to play.
    pub fn jump(&mut self, at: usize) -> Option<QueueItem> {
        if at >= self.upcoming_len() {
            return None;
        }
        for _ in 0..at {
            let skipped = self.take(0);
            self.leave_current();
            self.current = skipped;
        }
        let next = self.take(0)?;
        self.leave_current();
        self.current = Some(next.clone());
        self.drive();
        Some(next)
    }

    /// Move on to the next track; `None` at the end.
    pub fn advance(&mut self) -> Option<QueueItem> {
        self.jump(0)
    }

    /// Go back to the track before; the current one goes back to the front
    /// of the queue. `None` with nothing to go back to.
    pub fn previous(&mut self) -> Option<QueueItem> {
        let back = self.history.pop()?;
        if let Some(current) = self.current.take() {
            self.list.insert(0, current);
        }
        self.current = Some(back.clone());
        self.drive();
        Some(back)
    }

    /// The player plays `uri` now: follow it. `ran_on` says the player
    /// moved to what its own queue had next (it went on by itself, not the
    /// person choosing something in it).
    pub fn observe(&mut self, uri: &str, ran_on: bool) -> Observed {
        if self.parked {
            // Only the parked queue's own tracks wake it; anything else
            // leaves it as it was saved.
            if self.current.as_ref().is_some_and(|current| current.uri == uri) {
                self.drive();
                return Observed::Resumed;
            }
            let ahead = self.upcoming().position(|(_, item)| item.uri == uri);
            if let Some(at) = ahead {
                self.jump(at);
                return Observed::Resumed;
            }
            return Observed::Ignored;
        }
        if !self.driving {
            return Observed::Ignored;
        }
        if self.current.as_ref().is_some_and(|current| current.uri == uri) {
            return Observed::Same;
        }
        let ahead = self.upcoming().position(|(_, item)| item.uri == uri);
        if let Some(at) = ahead {
            self.jump(at);
            return Observed::Advanced;
        }
        if self.history.last().is_some_and(|item| item.uri == uri) {
            self.previous();
            return Observed::Back;
        }
        if ran_on && self.advance().is_some() {
            return Observed::Overran;
        }
        self.driving = false;
        Observed::Detached
    }

    pub fn remove(&mut self, at: usize) -> Option<QueueItem> {
        self.take(at)
    }

    /// Move the upcoming track at `from` into the gap `to` (0 = in front of
    /// everything, `upcoming_len()` = behind everything). It joins the
    /// section of the track it lands behind — in front of the first list
    /// track counts as queued by hand, so a track dragged to the very top
    /// plays next.
    pub fn move_to(&mut self, from: usize, to: usize) {
        let len = self.upcoming_len();
        if from >= len || to > len || to == from || to == from + 1 {
            return;
        }
        let Some(item) = self.take(from) else { return };
        let to = if to > from { to - 1 } else { to };
        if to <= self.user.len() {
            self.user.insert(to, item);
        } else {
            self.list.insert(to - self.user.len(), item);
        }
    }

    /// Forget everything that would play next.
    pub fn clear(&mut self) {
        self.user.clear();
        self.list.clear();
    }
}

impl PlayQueue {
    /// The queue as kept between runs: a header, then one line per field,
    /// tracks as `kind<TAB>uri<TAB>name<TAB>artist`. Tabs and line breaks in
    /// names become spaces.
    pub fn serialize(&self) -> String {
        fn clean(text: &str) -> String {
            text.replace(['\t', '\n', '\r'], " ")
        }
        let mut out = format!("{HEADER}\nsource\t{}\n", clean(&self.source));
        let mut track = |kind: &str, item: &QueueItem| {
            out.push_str(&format!("{kind}\t{}\t{}\t{}\n", clean(&item.uri), clean(&item.name), clean(&item.artist)));
        };
        let start = self.history.len().saturating_sub(SAVED_HISTORY);
        for item in &self.history[start..] {
            track("history", item);
        }
        if let Some(current) = &self.current {
            track("current", current);
        }
        for item in &self.user {
            track("user", item);
        }
        for item in &self.list {
            track("list", item);
        }
        out
    }

    /// A saved queue read back, parked; `None` when the text is not one or
    /// holds nothing to play. Unknown lines are skipped, so a later version's
    /// file still reads.
    pub fn parse(text: &str) -> Option<PlayQueue> {
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let mut queue = PlayQueue::default();
        for line in lines {
            let mut parts = line.split('\t');
            let kind = parts.next().unwrap_or("");
            if kind == "source" {
                queue.source = parts.next().unwrap_or("").to_string();
                continue;
            }
            let (Some(uri), name, artist) = (parts.next().filter(|uri| !uri.is_empty()), parts.next().unwrap_or(""), parts.next().unwrap_or("")) else { continue };
            let item = QueueItem { uri: uri.to_string(), name: name.to_string(), artist: artist.to_string() };
            match kind {
                "history" => queue.history.push(item),
                "current" => queue.current = Some(item),
                "user" => queue.user.push(item),
                "list" => queue.list.push(item),
                _ => {}
            }
        }
        if queue.current.is_none() && queue.upcoming_len() == 0 {
            return None;
        }
        queue.parked = true;
        Some(queue)
    }
}

/// A change to the player's own queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueOp {
    /// Take out the queued track at this position.
    Remove(usize),
    /// Queue this track at the end.
    Add(String),
}

/// The fewest commands that turn the player's queued tracks `actual` into
/// `desired`: tracks can only be taken out anywhere and added at the end,
/// so the longest run of `actual` that already reads as the start of
/// `desired` stays, everything else goes (last first, so the positions
/// hold), and what is missing is added.
pub fn reconcile(actual: &[String], desired: &[String]) -> Vec<QueueOp> {
    let mut keep = vec![false; actual.len()];
    let mut matched = 0;
    for (at, uri) in actual.iter().enumerate() {
        if desired.get(matched) == Some(uri) {
            keep[at] = true;
            matched += 1;
        }
    }
    let mut ops: Vec<QueueOp> = (0..actual.len()).rev().filter(|&at| !keep[at]).map(QueueOp::Remove).collect();
    ops.extend(desired[matched..].iter().cloned().map(QueueOp::Add));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(uri: &str) -> QueueItem {
        QueueItem { uri: uri.into(), name: uri.to_uppercase(), artist: String::new() }
    }

    fn items(uris: &[&str]) -> Vec<QueueItem> {
        uris.iter().map(|uri| item(uri)).collect()
    }

    fn upcoming(queue: &PlayQueue) -> Vec<String> {
        queue.upcoming().map(|(_, item)| item.uri.clone()).collect()
    }

    /// Playing a row plays the list as it is shown from that row on: a
    /// playlist sorted newest first plays newest first.
    #[test]
    fn a_list_plays_on_in_the_order_it_was_shown() {
        let mut queue = PlayQueue::default();
        let shown = items(&["d", "c", "b", "a"]);
        assert_eq!(queue.play_list(shown, 1, "Starred").unwrap().uri, "c");
        assert_eq!(upcoming(&queue), ["b", "a"]);
        assert_eq!(queue.source, "Starred");
        assert_eq!(queue.desired(3, |_| true), ["b", "a"]);
        assert_eq!(queue.advance().unwrap().uri, "b");
        assert_eq!(queue.advance().unwrap().uri, "a");
        assert_eq!(queue.advance(), None);
    }

    /// Tracks queued by hand play before the rest of the list; Play Next
    /// goes in front of them; another list keeps them.
    #[test]
    fn queued_tracks_play_before_the_list_and_survive_a_new_list() {
        let mut queue = PlayQueue::default();
        queue.play_list(items(&["a", "b", "c"]), 0, "One");
        queue.enqueue(items(&["x", "y"]));
        queue.play_next(item("n"));
        assert_eq!(upcoming(&queue), ["n", "x", "y", "b", "c"]);
        assert_eq!(queue.upcoming().nth(3).unwrap().0, Section::List);
        queue.play_list(items(&["p", "q"]), 0, "Two");
        assert_eq!(upcoming(&queue), ["n", "x", "y", "q"]);
    }

    /// The player's track changes are followed: the next track advances,
    /// a skip ahead skips along, going back goes back, and anything else
    /// detaches until something is played from the app again.
    #[test]
    fn track_changes_advance_skip_go_back_or_detach() {
        let mut queue = PlayQueue::default();
        queue.play_list(items(&["a", "b", "c", "d"]), 0, "L");
        assert_eq!(queue.observe("a", false), Observed::Same);
        assert_eq!(queue.observe("b", false), Observed::Advanced);
        assert_eq!(queue.observe("d", false), Observed::Advanced);
        assert_eq!(upcoming(&queue), Vec::<String>::new());
        assert_eq!(queue.observe("c", false), Observed::Back);
        assert_eq!(queue.current.as_ref().unwrap().uri, "c");
        assert_eq!(upcoming(&queue), ["d"]);
        assert_eq!(queue.observe("elsewhere", false), Observed::Detached);
        assert!(queue.desired(3, |_| true).is_empty());
        assert_eq!(queue.observe("d", false), Observed::Ignored);
        // Queuing while detached steers again from what plays.
        queue.adopt(item("elsewhere"));
        queue.enqueue(items(&["z"]));
        assert_eq!(queue.desired(3, |_| true), ["z"]);
        assert_eq!(queue.observe("z", false), Observed::Advanced);
    }

    /// A track ended before ours was in the player's queue and the player
    /// ran on into its own next: the queue plays its own next instead of
    /// letting go.
    #[test]
    fn the_player_running_on_by_itself_is_corrected() {
        let mut queue = PlayQueue::default();
        queue.play_list(items(&["c", "b", "a"]), 0, "Reversed");
        assert_eq!(queue.observe("autoplay", true), Observed::Overran);
        assert_eq!(queue.current.as_ref().unwrap().uri, "b");
        assert!(queue.driving);
        // At the end of the queue there is nothing to correct to.
        queue.advance();
        assert_eq!(queue.observe("autoplay", true), Observed::Detached);
    }

    #[test]
    fn previous_puts_the_current_track_back_in_front() {
        let mut queue = PlayQueue::default();
        queue.play_list(items(&["a", "b", "c"]), 0, "L");
        queue.advance();
        assert_eq!(queue.previous().unwrap().uri, "a");
        assert_eq!(upcoming(&queue), ["b", "c"]);
        assert_eq!(queue.previous(), None);
    }

    #[test]
    fn tracks_move_remove_and_jump() {
        let mut queue = PlayQueue::default();
        queue.play_list(items(&["now", "a", "b"]), 0, "L");
        queue.enqueue(items(&["x"]));
        assert_eq!(upcoming(&queue), ["x", "a", "b"]);
        // "b" dragged to the top plays next, queued by hand.
        queue.move_to(2, 0);
        assert_eq!(upcoming(&queue), ["b", "x", "a"]);
        assert_eq!(queue.user.len(), 2);
        // "b" dragged behind everything joins the list.
        queue.move_to(0, 3);
        assert_eq!(upcoming(&queue), ["x", "a", "b"]);
        assert_eq!(queue.remove(1).unwrap().uri, "a");
        assert_eq!(queue.jump(1).unwrap().uri, "b");
        assert_eq!(upcoming(&queue), Vec::<String>::new());
        queue.enqueue(items(&["y"]));
        queue.clear();
        assert_eq!(queue.upcoming_len(), 0);
    }

    fn uris(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The player's queue is brought in line with as few commands as
    /// there can be.
    #[test]
    fn reconcile_keeps_what_already_matches() {
        assert!(reconcile(&uris(&["a", "b", "c"]), &uris(&["a", "b", "c"])).is_empty());
        assert_eq!(reconcile(&uris(&["a", "b"]), &uris(&["a", "b", "c"])), vec![QueueOp::Add("c".into())]);
        // The track that played is gone from the front: one more added.
        assert_eq!(reconcile(&uris(&["b", "c"]), &uris(&["b", "c", "d"])), vec![QueueOp::Add("d".into())]);
        // A stranger in front is taken out, the rest stays.
        assert_eq!(reconcile(&uris(&["x", "a", "b"]), &uris(&["a", "b", "c"])), vec![QueueOp::Remove(0), QueueOp::Add("c".into())]);
        // Out of order: the tail goes, last first.
        assert_eq!(
            reconcile(&uris(&["a", "c", "b"]), &uris(&["a", "b", "c"])),
            vec![QueueOp::Remove(1), QueueOp::Add("c".into())]
        );
        // Cleared: everything goes.
        assert_eq!(reconcile(&uris(&["a", "b"]), &[]), vec![QueueOp::Remove(1), QueueOp::Remove(0)]);
    }

    /// A saved queue reads back whole — history, current, queued by hand,
    /// the list and where it came from — and parked: it steers nothing and
    /// wants nothing of the player's queue until it is woken.
    #[test]
    fn a_saved_queue_reads_back_parked() {
        let mut queue = PlayQueue::default();
        queue.play_list(items(&["h", "a", "b", "c"]), 0, "Starred\tsorted");
        queue.advance();
        queue.enqueue(vec![QueueItem { uri: "u".into(), name: "Tab\there".into(), artist: "X".into() }]);
        let back = PlayQueue::parse(&queue.serialize()).unwrap();
        assert!(back.parked && !back.driving);
        assert_eq!(back.current.as_ref().unwrap().uri, "a");
        assert_eq!(back.user[0].name, "Tab here");
        assert_eq!(upcoming(&back), vec!["u", "b", "c"]);
        assert_eq!(back.source, "Starred sorted");
        assert!(back.desired(3, |_| true).is_empty(), "a parked queue asks the player for nothing");
        // Previous still has the track before.
        let mut back = back;
        assert_eq!(back.previous().unwrap().uri, "h");
        assert!(back.driving && !back.parked);
        // Junk, another header or nothing to play reads as no queue.
        assert!(PlayQueue::parse("something else").is_none());
        assert!(PlayQueue::parse("music-queue 1\nsource\tX\n").is_none());
        // Unknown lines of a later version are skipped.
        let text = "music-queue 1\nfuture\twhatever\ncurrent\tt:1\tOne\tA\n";
        assert_eq!(PlayQueue::parse(text).unwrap().current.unwrap().name, "One");
    }

    /// A parked queue wakes only on its own tracks: the player turning up
    /// on the current one, or on one further on; anything else leaves it be,
    /// and queuing something keeps it whole.
    #[test]
    fn a_parked_queue_wakes_on_its_own_tracks() {
        let mut queue = PlayQueue::default();
        queue.play_list(items(&["a", "b", "c"]), 0, "L");
        let mut parked = PlayQueue::parse(&queue.serialize()).unwrap();
        assert_eq!(parked.observe("x", false), Observed::Ignored);
        assert!(parked.parked);
        parked.adopt(item("x"));
        parked.enqueue(items(&["q"]));
        assert!(parked.parked);
        assert_eq!(upcoming(&parked), vec!["q", "b", "c"]);
        assert_eq!(parked.observe("a", false), Observed::Resumed);
        assert!(parked.driving && !parked.parked);
        let mut parked = PlayQueue::parse(&queue.serialize()).unwrap();
        assert_eq!(parked.observe("c", false), Observed::Resumed);
        assert_eq!(parked.current.as_ref().unwrap().uri, "c");
    }
}
