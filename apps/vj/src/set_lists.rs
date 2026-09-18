//! The set lists on the shelf: named, locked or not, and written down.
//!
//! One live list was never the shape of the job. A night has a warm-up, a
//! peak and a set of records held back, and until now they all had to share
//! the single queue — so building the second one meant destroying the first.
//!
//! THE SHELF IS HERE, THE LIVE LIST IS STILL THE ENGINE'S. Switching copies
//! the live queue back into its slot and copies the new slot forward. Two
//! copies of the live list would rot against each other; one copy means
//! every existing queue verb, the pump and the row click included, keeps
//! working with no list selector threaded through it.
//!
//! Pure: no clock, no filesystem, no widgets. The file dialect is the marks
//! file's -- a leading word, a tolerant reader, and every line this build
//! cannot read KEPT and written back, so an older build cannot silently
//! delete what a newer one recorded.

use crate::decks::{TrackItem, TrackSideChannels};
use makepad_asset_data::{AssetId, AssetRevisionId, BlobId, MediaType};
use std::str::FromStr;

/// The most lists the shelf holds.
///
/// Not a technical limit: a picker the operator has to read during a set
/// stops being useful long before this, and a bounded shelf cannot be grown
/// without noticing by a file that got away.
pub const MAX_LISTS: usize = 8;

/// What a list is called when nothing has named it.
pub const DEFAULT_NAME: &str = "SET LIST";

/// One set list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetList {
    pub name: String,
    /// A programme rather than a working queue: nothing changes it, and
    /// nothing eats it.
    pub locked: bool,
    pub repeat: bool,
    pub shuffle: bool,
    pub items: Vec<TrackItem>,
    /// Lines inside this list that this build could not read, kept verbatim
    /// so a newer build's records survive an older build opening the file.
    pub unknown: Vec<String>,
}

/// Every list, and which one is on the decks.
#[derive(Clone, Debug, PartialEq)]
pub struct SetLists {
    lists: Vec<SetList>,
    active: usize,
}

impl Default for SetLists {
    fn default() -> Self {
        SetLists { lists: vec![SetList { name: DEFAULT_NAME.to_string(), ..Default::default() }], active: 0 }
    }
}

impl SetLists {
    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn len(&self) -> usize {
        self.lists.len()
    }

    pub fn active(&self) -> &SetList {
        &self.lists[self.active]
    }

    pub fn active_mut(&mut self) -> &mut SetList {
        let at = self.active;
        &mut self.lists[at]
    }

    pub fn get(&self, at: usize) -> Option<&SetList> {
        self.lists.get(at)
    }

    pub fn get_mut(&mut self, at: usize) -> Option<&mut SetList> {
        self.lists.get_mut(at)
    }

    /// Name and lock per list, for the panel.
    pub fn names(&self) -> Vec<(String, bool)> {
        self.lists.iter().map(|list| (list.name.clone(), list.locked)).collect()
    }

    /// Put a different list on the decks. False when it is already the one.
    pub fn switch_to(&mut self, at: usize) -> bool {
        if at >= self.lists.len() || at == self.active {
            return false;
        }
        self.active = at;
        true
    }

    /// A new, empty list, which becomes the active one. `None` when the
    /// shelf is full.
    pub fn add(&mut self, name: &str) -> Option<usize> {
        if self.lists.len() >= MAX_LISTS {
            return None;
        }
        self.lists.push(SetList { name: clean_name(name), ..Default::default() });
        self.active = self.lists.len() - 1;
        Some(self.active)
    }

    /// Take a list off the shelf. The LAST one cannot go: the console always
    /// has a set list, and an empty shelf would be a state every reader here
    /// would have to guard against for no gain.
    pub fn drop_list(&mut self, at: usize) -> bool {
        if self.lists.len() <= 1 || at >= self.lists.len() {
            return false;
        }
        self.lists.remove(at);
        // The active list follows the one that went: past it, everything
        // shifts down; on it, the shelf lands on its neighbour.
        if self.active > at || self.active >= self.lists.len() {
            self.active = self.active.saturating_sub(1);
        }
        true
    }

    pub fn rename_active(&mut self, name: &str) {
        let at = self.active;
        self.lists[at].name = clean_name(name);
    }

    /// Read a shelf back.
    ///
    /// Returns the shelf and how many lines could not be read at all, which
    /// is what decides whether writing back is safe -- see
    /// [`may_write_back`].
    ///
    /// Deliberately unfailing. Lines before the first `list` attach to an
    /// implicit first list, which is what makes the single-queue file this
    /// replaces a valid shelf file with no conversion step.
    pub fn from_text(text: &str) -> (SetLists, usize) {
        let mut lists: Vec<SetList> = Vec::new();
        let mut active = 0usize;
        let mut lossy = 0usize;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let (word, rest) = match line.split_once(' ') {
                Some((word, rest)) => (word, rest),
                None => (line, ""),
            };
            match word {
                "version" => {}
                "active" => active = rest.trim().parse().unwrap_or(0),
                "list" => {
                    if lists.len() < MAX_LISTS {
                        lists.push(SetList { name: clean_name(rest), ..Default::default() });
                    }
                }
                "lock" | "repeat" | "shuffle" => {
                    let on = rest.trim() == "1";
                    let list = open_list(&mut lists);
                    match word {
                        "lock" => list.locked = on,
                        "repeat" => list.repeat = on,
                        _ => list.shuffle = on,
                    }
                }
                "local" | "asset" => match track_from_line(word, rest) {
                    Some(item) => open_list(&mut lists).items.push(item),
                    None => {
                        // A record this build cannot turn back into a track
                        // -- an old two-word `asset` line, or one written by
                        // a build that knows more. Kept verbatim so it is
                        // still there for whichever build can read it.
                        open_list(&mut lists).unknown.push(line.to_string());
                        lossy += 1;
                    }
                },
                _ => {
                    open_list(&mut lists).unknown.push(line.to_string());
                    lossy += 1;
                }
            }
        }
        if lists.is_empty() {
            return (SetLists::default(), lossy);
        }
        let active = active.min(lists.len() - 1);
        (SetLists { lists, active }, lossy)
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from("version 1\n");
        out.push_str(&format!("active {}\n", self.active));
        for list in &self.lists {
            out.push_str(&format!("list {}\n", list.name));
            out.push_str(&format!("lock {}\n", list.locked as u8));
            out.push_str(&format!("repeat {}\n", list.repeat as u8));
            out.push_str(&format!("shuffle {}\n", list.shuffle as u8));
            for item in &list.items {
                out.push_str(&track_to_line(item));
                out.push('\n');
            }
            // Last, so a line this build did not understand cannot come
            // between two it did and change what they attach to.
            for line in &list.unknown {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}

/// The list that lines are currently attaching to, made if the file opened
/// with records before it named a list -- which is exactly the shape of the
/// single-queue file this replaces.
fn open_list(lists: &mut Vec<SetList>) -> &mut SetList {
    if lists.is_empty() {
        lists.push(SetList { name: DEFAULT_NAME.to_string(), ..Default::default() });
    }
    let last = lists.len() - 1;
    &mut lists[last]
}

/// A name that cannot break the file that holds it, and is never blank.
fn clean_name(name: &str) -> String {
    let name: String = name.trim().chars().filter(|c| *c != '\n' && *c != '\r').collect();
    match name.is_empty() {
        true => DEFAULT_NAME.to_string(),
        false => name,
    }
}

/// Whether a shelf that was read with losses may be written back over its
/// own file.
///
/// A file this build only partly understood is not overwritten just because
/// the app happened to redraw: the lines it could not read are kept, but the
/// safest thing to do with a file from a newer build is leave it alone until
/// the operator actually changes something. Once they have, their change is
/// the more recent truth and it is written.
pub fn may_write_back(lossy: bool, touched_by_hand: bool) -> bool {
    !lossy || touched_by_hand
}

/// The word a media type is written as. Only the containers a track can
/// actually be; anything else is not a record and cannot reach here.
pub fn media_word(media: MediaType) -> &'static str {
    match media {
        MediaType::Mp3 => "mp3",
        MediaType::Ogg => "ogg",
        MediaType::Wav => "wav",
        _ => "mp4",
    }
}

/// The same, back. Anything unrecognised reads as `Mp4`, which is the answer
/// the deck's own loader already gives when it cannot tell a container --
/// an unreadable word gets the app's existing answer, not a new one.
pub fn media_from_word(word: &str) -> MediaType {
    match word {
        "mp3" => MediaType::Mp3,
        "ogg" => MediaType::Ogg,
        "wav" => MediaType::Wav,
        _ => MediaType::Mp4,
    }
}

/// One record, as a line.
///
/// A store track is written SELF-DESCRIBING -- asset, revision, blob, length
/// and container -- rather than as a bare id. A deck loads a store track
/// from the blob alone and never asks the catalog, so a descriptor restores
/// on a cold start with nothing else loaded, while an id has to wait for a
/// catalog that may not have listed that record yet. The title is last
/// because it is the one field that may hold spaces.
fn track_to_line(item: &TrackItem) -> String {
    format!(
        "asset {} {} {} {} {} {}",
        item.asset,
        item.revision,
        item.media_blob,
        item.media_len,
        media_word(item.media),
        item.title,
    )
}

fn track_from_line(word: &str, rest: &str) -> Option<TrackItem> {
    if word != "asset" {
        // `local` lines name a path, which only the host can turn into a
        // track; it keeps them itself.
        return None;
    }
    let mut parts = rest.splitn(6, ' ');
    let asset = AssetId::from_str(parts.next()?.trim()).ok()?;
    let revision = AssetRevisionId::from_str(parts.next()?.trim()).ok()?;
    let media_blob = BlobId::from_str(parts.next()?.trim()).ok()?;
    let media_len: u64 = parts.next()?.trim().parse().ok()?;
    let media = media_from_word(parts.next()?.trim());
    let title = parts.next().unwrap_or("").to_string();
    Some(TrackItem {
        asset,
        revision,
        title,
        media_blob,
        media_len,
        media,
        // Not written down: the stems and the transcript are the STORE's to
        // say, they are found again when the record loads, and a stale blob
        // recorded here would be worse than no answer.
        side: TrackSideChannels::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(seed: u8, title: &str) -> TrackItem {
        TrackItem {
            asset: AssetId::from_bytes([seed; 16]),
            revision: AssetRevisionId::from_bytes([seed; 32]),
            title: title.to_string(),
            media_blob: BlobId::from_bytes([seed ^ 0x5a; 32]),
            media_len: 1000 + seed as u64,
            media: MediaType::Mp3,
            side: TrackSideChannels::default(),
        }
    }

    fn shelf() -> SetLists {
        let mut lists = SetLists::default();
        lists.rename_active("Warm up");
        lists.active_mut().items.push(item(1, "Opener"));
        lists.active_mut().items.push(item(2, "Second One With Spaces"));
        lists.add("Peak");
        lists.active_mut().locked = true;
        lists.active_mut().shuffle = true;
        lists.active_mut().items.push(item(3, "Banger"));
        lists
    }

    #[test]
    fn a_saved_line_carries_everything_a_deck_needs_to_play_it() {
        let back = SetLists::from_text(&shelf().to_text()).0;
        let record = &back.get(0).expect("first list").items[0];
        let was = item(1, "Opener");
        assert_eq!(record.asset, was.asset);
        assert_eq!(record.revision, was.revision, "the revision, so no catalog is needed");
        assert_eq!(record.media_blob, was.media_blob, "and the blob the deck actually loads");
        assert_eq!(record.media_len, was.media_len);
        assert_eq!(record.media, was.media);
        assert_eq!(record.title, "Opener");
    }

    #[test]
    fn a_title_with_spaces_survives_being_written_down() {
        let back = SetLists::from_text(&shelf().to_text()).0;
        assert_eq!(back.get(0).expect("first").items[1].title, "Second One With Spaces");
    }

    #[test]
    fn a_shelf_written_out_and_read_back_is_the_same_shelf() {
        let was = shelf();
        let (back, lossy) = SetLists::from_text(&was.to_text());
        assert_eq!(lossy, 0);
        assert_eq!(back, was, "names, locks, policies, order and the active list");
    }

    #[test]
    fn the_single_queue_file_this_replaces_reads_as_one_list_the_operator_still_has() {
        // Exactly what the old queue.txt held: bare lines, no header at all.
        let old = format!(
            "{}\nlocal F:\\music\\opener.mp3\n",
            track_to_line(&item(7, "Kept")),
        );
        let (back, lossy) = SetLists::from_text(&old);
        assert_eq!(back.len(), 1, "one implicit list");
        assert_eq!(back.active().name, DEFAULT_NAME);
        assert_eq!(back.active().items.len(), 1, "the store track resolved");
        assert_eq!(
            back.active().unknown,
            vec!["local F:\\music\\opener.mp3".to_string()],
            "and the local line is KEPT for the host to resolve",
        );
        assert_eq!(lossy, 1);
    }

    #[test]
    fn a_line_this_build_cannot_read_survives_being_written_back() {
        let text = "version 1\nactive 0\nlist Keep\nlock 0\nrepeat 0\nshuffle 0\n\
                    asset not-an-id\nsomething_from_the_future 42\n";
        let (back, lossy) = SetLists::from_text(text);
        assert_eq!(lossy, 2);
        let written = back.to_text();
        assert!(written.contains("asset not-an-id"), "an unreadable record is not deleted");
        assert!(
            written.contains("something_from_the_future 42"),
            "nor is a key this build has never heard of",
        );
        assert_eq!(SetLists::from_text(&written).0, back, "and it round-trips from there");
    }

    #[test]
    fn a_file_only_partly_understood_is_left_alone_until_a_hand_changes_something() {
        assert!(may_write_back(false, false), "nothing was lost: write freely");
        assert!(!may_write_back(true, false), "a newer build's file is not overwritten by a redraw");
        assert!(may_write_back(true, true), "but the operator's own change is the newer truth");
    }

    #[test]
    fn the_last_set_list_cannot_be_dropped() {
        let mut lists = SetLists::default();
        assert!(!lists.drop_list(0), "the console always has a set list");
        lists.add("Second");
        assert!(lists.drop_list(1));
        assert_eq!(lists.len(), 1);
        assert!(!lists.drop_list(0));
    }

    #[test]
    fn dropping_a_list_leaves_the_shelf_pointing_at_one_that_is_still_there() {
        let mut lists = SetLists::default();
        lists.add("Two");
        lists.add("Three");
        assert_eq!(lists.active_index(), 2);
        lists.drop_list(2);
        assert_eq!(lists.active_index(), 1, "the shelf lands on its neighbour");
        assert_eq!(lists.active().name, "Two");

        let mut lists = SetLists::default();
        lists.add("Two");
        lists.add("Three");
        lists.switch_to(2);
        lists.drop_list(0);
        assert_eq!(lists.active().name, "Three", "everything past the gap shifts down with it");
    }

    #[test]
    fn an_active_index_past_the_end_reads_back_as_the_first_list() {
        let (back, _) = SetLists::from_text("version 1\nactive 9\nlist Only\n");
        assert_eq!(back.active_index(), 0);
        assert_eq!(back.active().name, "Only");
    }

    #[test]
    fn a_name_with_a_newline_in_it_cannot_split_the_file() {
        let mut lists = SetLists::default();
        lists.rename_active("Sat\nurday\r night");
        assert_eq!(lists.active().name, "Saturday night");
        assert_eq!(SetLists::from_text(&lists.to_text()).0, lists);
    }

    #[test]
    fn a_set_list_names_itself_when_the_file_gives_it_none() {
        let (back, _) = SetLists::from_text("version 1\nlist   \n");
        assert_eq!(back.active().name, DEFAULT_NAME);
        let mut lists = SetLists::default();
        lists.rename_active("   ");
        assert_eq!(lists.active().name, DEFAULT_NAME);
    }

    #[test]
    fn two_set_lists_with_the_same_name_stay_two_lists() {
        let mut lists = SetLists::default();
        lists.rename_active("Peak");
        lists.add("Peak");
        assert_eq!(lists.len(), 2, "the name is a label, not an identity");
        let back = SetLists::from_text(&lists.to_text()).0;
        assert_eq!(back.len(), 2);
    }

    #[test]
    fn the_shelf_holds_no_more_lists_than_it_promises() {
        let mut lists = SetLists::default();
        while lists.len() < MAX_LISTS {
            assert!(lists.add("more").is_some());
        }
        assert_eq!(lists.add("one too many"), None);
        assert_eq!(lists.len(), MAX_LISTS);

        // And a file cannot smuggle a longer shelf past it either.
        let mut text = String::from("version 1\n");
        for n in 0..(MAX_LISTS + 5) {
            text.push_str(&format!("list L{n}\n"));
        }
        assert_eq!(SetLists::from_text(&text).0.len(), MAX_LISTS);
    }

    #[test]
    fn every_container_a_record_can_be_survives_the_round_trip() {
        for media in [MediaType::Mp3, MediaType::Ogg, MediaType::Wav, MediaType::Mp4] {
            assert_eq!(media_from_word(media_word(media)), media);
        }
        assert_eq!(
            media_from_word("something-else"),
            MediaType::Mp4,
            "the loader's own fallback, not a new one",
        );
    }
}
