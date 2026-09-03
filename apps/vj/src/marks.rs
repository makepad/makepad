//! Everything an operator has marked on a track, as one typed record.
//!
//! The cue, the loop slots and the bookmark used to be three shapes in two
//! places: a `cue <secs>` line that the slot reader silently dropped on the
//! floor, bare `<start> <end>` lines for the slots, and a bookmark that was
//! never written down at all. Adding a hot cue to that meant a fourth
//! shape, so this is one list instead, with room for the marks Phase 2
//! brings.
//!
//! It stays out of the analysis cache. That cache is discarded whenever a
//! detector changes, and operator work must never ride a derived cache's
//! lifetime.

/// What a mark means. The kinds beyond the three a deck keeps today are
/// here because the file format has to survive meeting them: a build that
/// does not know a kind steps over its line rather than losing the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkKind {
    /// The red marker: where play and CUE return to.
    Cue,
    /// The green in-point with no out yet.
    Bookmark,
    /// A saved loop in one of the blue slots.
    Loop,
    /// A numbered point in the bank: pressing it sends the record there.
    HotCue,
    /// A numbered point that carries a RUNNING loop to it rather than
    /// replacing it, and seeks when none is running.
    Jump,
    /// The ends of the track the analysis found sound between.
    Intro,
    Outro,
    /// A grid a hand corrected: `start` is the first beat, `len` is how
    /// long a beat is, and `slot` is that beat's seat in the bar. A grid
    /// IS a mark by this file's own shape -- a point, with a length, at a
    /// bar position -- so it needs no second format.
    Grid,
}

impl MarkKind {
    fn word(self) -> &'static str {
        match self {
            MarkKind::Cue => "cue",
            MarkKind::Bookmark => "bookmark",
            MarkKind::Loop => "loop",
            MarkKind::HotCue => "hotcue",
            MarkKind::Jump => "jump",
            MarkKind::Intro => "intro",
            MarkKind::Outro => "outro",
            MarkKind::Grid => "grid",
        }
    }

    fn from_word(word: &str) -> Option<MarkKind> {
        Some(match word {
            "cue" => MarkKind::Cue,
            "bookmark" => MarkKind::Bookmark,
            "loop" => MarkKind::Loop,
            "hotcue" => MarkKind::HotCue,
            "jump" => MarkKind::Jump,
            "intro" => MarkKind::Intro,
            "outro" => MarkKind::Outro,
            "grid" => MarkKind::Grid,
            _ => return None,
        })
    }
}

/// One mark. A point simply has no length.
#[derive(Clone, PartialEq, Debug)]
pub struct Mark {
    pub kind: MarkKind,
    pub start_secs: f64,
    pub len_secs: f64,
    /// Which pad or chip this is, as DATA rather than as a position in a
    /// list: deleting one frees its number instead of shuffling every mark
    /// after it onto a different pad.
    pub slot: u16,
    /// What the operator called it. May be empty, and may hold spaces.
    pub label: String,
    /// ARGB. Zero means "whatever the palette says for this slot".
    pub colour: u32,
}

impl Mark {
    /// Where a span ends. For a point, where it starts.
    pub fn end_secs(&self) -> f64 {
        self.start_secs + self.len_secs
    }
}

/// Every mark on one track.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct MarkRecord {
    marks: Vec<Mark>,
}

/// What a grid mark's label says when the grid is not to be changed.
const LOCKED: &str = "locked";

/// The format this build writes. A reader meeting a higher number still
/// takes what it recognises, because every line stands on its own.
const VERSION: u32 = 1;

impl MarkRecord {
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Every mark of one kind, in slot order.
    /// Every mark, in the order the record keeps them: by kind, then by
    /// number. For a reader that wants more than one kind at a time.
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    pub fn of_kind(&self, kind: MarkKind) -> impl Iterator<Item = &Mark> {
        self.marks.iter().filter(move |mark| mark.kind == kind)
    }

    /// Add a mark, replacing whatever held that kind and slot.
    pub fn put(&mut self, mark: Mark) {
        self.clear_slot(mark.kind, mark.slot);
        self.marks.push(mark);
        // Kind, then slot: the file reads in the order a deck fills it.
        self.marks.sort_by(|a, b| (a.kind.word(), a.slot).cmp(&(b.kind.word(), b.slot)));
    }

    /// Free one slot. The marks beside it keep their own numbers.
    pub fn clear_slot(&mut self, kind: MarkKind, slot: u16) {
        self.marks.retain(|mark| mark.kind != kind || mark.slot != slot);
    }

    fn clear_kind(&mut self, kind: MarkKind) {
        self.marks.retain(|mark| mark.kind != kind);
    }

    pub fn cue(&self) -> Option<f64> {
        self.of_kind(MarkKind::Cue).next().map(|mark| mark.start_secs)
    }

    pub fn set_cue(&mut self, secs: Option<f64>) {
        self.set_point(MarkKind::Cue, secs);
    }

    /// The corrected grid, if a hand left one: tempo, first beat, bar
    /// seat, and whether it was locked against further change.
    pub fn grid(&self) -> Option<(f64, f64, u32, bool)> {
        self.of_kind(MarkKind::Grid)
            .next()
            .filter(|mark| mark.len_secs > 1e-4 && mark.start_secs >= 0.0)
            .map(|mark| {
                (
                    60.0 / mark.len_secs,
                    mark.start_secs,
                    mark.slot as u32 % 4,
                    mark.label == LOCKED,
                )
            })
    }

    pub fn set_grid(&mut self, grid: Option<(f64, f64, u32, bool)>) {
        match grid {
            Some((bpm, first_beat_secs, phase, locked)) if bpm > 1.0 => self.put(Mark {
                kind: MarkKind::Grid,
                start_secs: first_beat_secs,
                len_secs: 60.0 / bpm,
                slot: (phase % 4) as u16,
                // The label is the file's own free-text field, and "this
                // grid is settled" is exactly the kind of thing it is for.
                label: if locked { LOCKED.to_string() } else { String::new() },
                colour: 0,
            }),
            _ => self.clear_kind(MarkKind::Grid),
        }
    }

    pub fn bookmark(&self) -> Option<f64> {
        self.of_kind(MarkKind::Bookmark).next().map(|mark| mark.start_secs)
    }

    pub fn set_bookmark(&mut self, secs: Option<f64>) {
        self.set_point(MarkKind::Bookmark, secs);
    }

    fn set_point(&mut self, kind: MarkKind, secs: Option<f64>) {
        self.clear_kind(kind);
        let Some(secs) = secs.filter(|secs| secs.is_finite() && *secs >= 0.0) else {
            return;
        };
        self.put(Mark {
            kind,
            start_secs: secs,
            len_secs: 0.0,
            slot: 0,
            label: String::new(),
            colour: 0,
        });
    }

    /// The loop slots as `(start, end)` pairs, in slot order.
    pub fn loops(&self) -> Vec<(f64, f64)> {
        self.of_kind(MarkKind::Loop).map(|mark| (mark.start_secs, mark.end_secs())).collect()
    }

    /// Replace every loop slot, numbering them from zero.
    pub fn set_loops(&mut self, spans: &[(f64, f64)]) {
        self.clear_kind(MarkKind::Loop);
        for (index, (start, end)) in spans.iter().enumerate() {
            if !start.is_finite() || !end.is_finite() || *start < 0.0 {
                continue;
            }
            self.put(Mark {
                kind: MarkKind::Loop,
                start_secs: *start,
                len_secs: (end - start).max(0.0),
                slot: index as u16,
                label: String::new(),
                colour: 0,
            });
        }
    }

    /// One line per mark: the kind, the start, the length, then `slot`,
    /// `colour` and `label` as named fields. The label goes last, because it
    /// is the only one that may hold a space.
    pub fn to_text(&self) -> String {
        let mut text = format!("marks {VERSION}\n");
        for mark in &self.marks {
            text.push_str(&format!(
                "{} {} {} slot {}",
                mark.kind.word(),
                mark.start_secs,
                mark.len_secs,
                mark.slot
            ));
            if mark.colour != 0 {
                text.push_str(&format!(" colour {:08x}", mark.colour));
            }
            if !mark.label.is_empty() {
                text.push_str(&format!(" label {}", mark.label));
            }
            text.push('\n');
        }
        text
    }

    /// Read a marks file: this format, or either shape the old build wrote
    /// (a `cue <secs>` line, and bare `<start> <end>` loop lines).
    pub fn from_text(text: &str) -> MarkRecord {
        let mut record = MarkRecord::default();
        let mut old_slot = 0u16;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("marks ") {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(head) = parts.next() else { continue };
            // The old format's loop line: two bare numbers, nothing else.
            if let Some(start) = crate::durable::seconds(head) {
                let Some(end) = parts.next().and_then(crate::durable::seconds) else { continue };
                if start < 0.0 || end < start {
                    continue;
                }
                record.put(Mark {
                    kind: MarkKind::Loop,
                    start_secs: start,
                    len_secs: end - start,
                    slot: old_slot,
                    label: String::new(),
                    colour: 0,
                });
                old_slot = old_slot.saturating_add(1);
                continue;
            }
            let Some(kind) = MarkKind::from_word(head) else { continue };
            let Some(start) = parts.next().and_then(crate::durable::seconds) else { continue };
            if start < 0.0 {
                continue;
            }
            let rest: Vec<&str> = parts.collect();
            // The old `cue <secs>` line stops here, with no length after it,
            // and so does any line that goes straight to a named field.
            let named = |word: &str| word == "slot" || word == "colour" || word == "label";
            let len = match rest.first() {
                None => 0.0,
                Some(word) if named(word) => 0.0,
                Some(word) => match crate::durable::seconds(word) {
                    Some(len) if len >= 0.0 => len,
                    _ => continue,
                },
            };
            let mut slot = 0u16;
            let mut colour = 0u32;
            let mut label = String::new();
            let mut index = 0;
            while index < rest.len() {
                match rest[index] {
                    "slot" => {
                        slot = rest.get(index + 1).and_then(|w| w.parse().ok()).unwrap_or(0);
                        index += 2;
                    }
                    "colour" => {
                        colour = rest
                            .get(index + 1)
                            .and_then(|w| u32::from_str_radix(w, 16).ok())
                            .unwrap_or(0);
                        index += 2;
                    }
                    // Last, and takes the rest of the line verbatim.
                    "label" => {
                        if let Some(at) = line.find(" label ") {
                            label = line[at + 7..].to_string();
                        }
                        break;
                    }
                    _ => index += 1,
                }
            }
            record.put(Mark { kind, start_secs: start, len_secs: len, slot, label, colour });
        }
        record
    }
}

#[cfg(test)]
mod tests {
    /// A corrected grid rides the marks file as what it is: a beat, how
    /// long a beat is, and that beat's seat in the bar.
    #[test]
    fn a_corrected_grid_survives_the_round_trip() {
        let mut record = MarkRecord::default();
        record.set_cue(Some(12.5));
        record.set_grid(Some((128.0, 0.1875, 2, true)));
        let back = MarkRecord::from_text(&record.to_text());
        let (bpm, first, phase, locked) = back.grid().expect("a grid");
        assert!(locked, "and that it was settled");
        assert!((bpm - 128.0).abs() < 1e-9, "{bpm}");
        assert!((first - 0.1875).abs() < 1e-9);
        assert_eq!(phase, 2);
        assert_eq!(back.cue(), Some(12.5), "and everything beside it");

        // Cleared, it leaves nothing behind.
        let mut record = record;
        record.set_grid(None);
        assert!(MarkRecord::from_text(&record.to_text()).grid().is_none());
    }

    /// A file written before grids were a thing still reads, and one
    /// written now still reads on a build that has never heard of them.
    #[test]
    fn a_grid_line_is_stepped_over_by_a_reader_that_does_not_know_it() {
        let mut written = MarkRecord::default();
        written.set_cue(Some(12.5));
        written.set_grid(Some((128.0, 0.1875, 2, false)));
        // A kind from a build newer than this reader, in the middle.
        let text = written.to_text().replace("grid ", "wibble ");
        let record = MarkRecord::from_text(&text);
        assert_eq!(record.cue(), Some(12.5), "the lines it knows still land");
        assert!(record.grid().is_none(), "and the one it does not is stepped over");
    }

    use super::*;

    fn full() -> MarkRecord {
        let mut record = MarkRecord::default();
        record.set_cue(Some(12.5));
        record.set_bookmark(Some(3.25));
        record.set_loops(&[(30.0, 34.0), (40.0, 44.5)]);
        record
    }

    #[test]
    fn a_record_carries_the_three_marks_a_deck_already_had() {
        let record = full();
        assert_eq!(record.cue(), Some(12.5));
        assert_eq!(record.bookmark(), Some(3.25), "and the one that was never written down");
        assert_eq!(record.loops(), vec![(30.0, 34.0), (40.0, 44.5)]);
    }

    #[test]
    fn it_survives_a_trip_through_its_own_text() {
        let record = full();
        let back = MarkRecord::from_text(&record.to_text());
        assert_eq!(back.cue(), record.cue());
        assert_eq!(back.bookmark(), record.bookmark());
        assert_eq!(back.loops(), record.loops());
    }

    #[test]
    fn the_file_the_old_build_wrote_still_reads() {
        // `cue <secs>` then bare `<start> <end>` lines, which is every marks
        // file on the operator's disk today.
        let old = "cue 12.5\n30 34\n40 44.5\n";
        let record = MarkRecord::from_text(old);
        assert_eq!(record.cue(), Some(12.5));
        assert_eq!(record.loops(), vec![(30.0, 34.0), (40.0, 44.5)]);
        assert_eq!(record.bookmark(), None, "the old format never had one");
    }

    #[test]
    fn the_files_actually_sitting_on_the_operators_disk_read() {
        // Copied from `local/vj/loop-marks`: these are every shape the old
        // build ever wrote, and losing one would lose real marks.
        assert_eq!(MarkRecord::from_text("cue 0\n").cue(), Some(0.0));
        assert_eq!(
            MarkRecord::from_text("cue 42.34451711026616\n").cue(),
            Some(42.34451711026616)
        );
        let both = MarkRecord::from_text("cue 0\n89.96230995415127 94.44835399718788\n");
        assert_eq!(both.cue(), Some(0.0));
        assert_eq!(both.loops(), vec![(89.96230995415127, 94.44835399718788)]);
    }

    #[test]
    fn a_line_that_parses_but_cannot_mean_a_position_is_dropped() {
        let torn = "marks 1\ncue NaN\nloop 30 inf\nloop 10 10\nloop -1 5\nnonsense\n";
        let record = MarkRecord::from_text(torn);
        assert_eq!(record.cue(), None, "NaN is not a position");
        assert_eq!(record.loops(), vec![(10.0, 20.0)], "and neither is infinity");
    }

    #[test]
    fn a_named_line_says_a_length_where_an_old_bare_line_said_an_end() {
        // The one place the two formats disagree about a number, and the
        // reason there is no ambiguity: the old form never had a kind word
        // in front of it, so a line beginning `loop` is always the new one.
        assert_eq!(MarkRecord::from_text("loop 10 4").loops(), vec![(10.0, 14.0)]);
        assert_eq!(MarkRecord::from_text("10 14").loops(), vec![(10.0, 14.0)]);
        // And an old line whose end precedes its start is not a span at all.
        assert!(MarkRecord::from_text("10 4").loops().is_empty());
    }

    #[test]
    fn a_slot_deleted_does_not_renumber_the_slots_beside_it() {
        let mut record = MarkRecord::default();
        record.set_loops(&[(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)]);
        record.clear_slot(MarkKind::Loop, 1);
        let kept: Vec<(u16, f64)> = record
            .of_kind(MarkKind::Loop)
            .map(|mark| (mark.slot, mark.start_secs))
            .collect();
        assert_eq!(kept, vec![(0, 1.0), (2, 5.0)], "slot 2 is still slot 2");
        let back = MarkRecord::from_text(&record.to_text());
        let after: Vec<(u16, f64)> =
            back.of_kind(MarkKind::Loop).map(|m| (m.slot, m.start_secs)).collect();
        assert_eq!(after, kept, "and the gap survives the file");
    }

    #[test]
    fn a_point_has_no_length_and_a_span_does() {
        let mut record = MarkRecord::default();
        record.set_cue(Some(5.0));
        record.set_loops(&[(10.0, 14.0)]);
        let cue = record.of_kind(MarkKind::Cue).next().unwrap();
        assert_eq!(cue.len_secs, 0.0, "a cue is a place, not a stretch");
        let span = record.of_kind(MarkKind::Loop).next().unwrap();
        assert_eq!(span.len_secs, 4.0);
        assert_eq!(span.end_secs(), 14.0);
    }

    #[test]
    fn a_label_with_spaces_comes_back_whole() {
        let mut record = MarkRecord::default();
        record.put(Mark {
            kind: MarkKind::HotCue,
            start_secs: 8.0,
            len_secs: 0.0,
            slot: 4,
            label: "second   drop".into(),
            colour: 0xff5c_39ff,
        });
        let back = MarkRecord::from_text(&record.to_text());
        let mark = back.of_kind(MarkKind::HotCue).next().unwrap();
        assert_eq!(mark.label, "second   drop");
        assert_eq!(mark.colour, 0xff5c_39ff);
        assert_eq!(mark.slot, 4);
    }

    #[test]
    fn a_kind_this_build_does_not_know_is_stepped_over_not_choked_on() {
        let future = "marks 1\ncue 2\nteleport 9 0 slot 1\nloop 10 10\n";
        let record = MarkRecord::from_text(future);
        assert_eq!(record.cue(), Some(2.0));
        assert_eq!(record.loops(), vec![(10.0, 20.0)], "the rest still reads");
    }

    #[test]
    fn an_empty_record_writes_something_a_reader_can_still_open() {
        let record = MarkRecord::default();
        let back = MarkRecord::from_text(&record.to_text());
        assert_eq!(back.cue(), None);
        assert!(back.loops().is_empty());
    }
}
