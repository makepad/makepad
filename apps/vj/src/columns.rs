//! What the two track lists put in their columns, and in what order.
//!
//! The explorer's listing and the set list are the same kind of thing drawn at
//! two widths, and the operator does not agree with us about which columns
//! matter. Someone mixing from a tagged library wants TAGS and GENRE in front
//! of them; someone playing their own records wants TITLE wide and everything
//! else out of the way. So the choice belongs to them, per list, and this
//! module is the whole of that choice: which columns exist, what each one is
//! called and how wide it wants to be, and the one settings line that
//! remembers an arrangement across restarts.
//!
//! Two rules hold everything else up.
//!
//! A layout ALWAYS carries all twelve columns, in display order, each merely
//! shown or hidden. Hiding is not removal. That is what lets the settings
//! dialog offer an ordered list with checkboxes — a hidden column keeps its
//! place, so turning it back on puts it where the operator last left it rather
//! than at the end — and it is what makes a settings file from an older build
//! readable: columns it never heard of are appended hidden instead of being
//! treated as a corrupt line.
//!
//! Slugs are FROZEN. They are the settings-file key; renaming one silently
//! resets that column on every machine that already has a file.
//!
//! Nothing here draws anything. It decides what should be drawn and in what
//! order; the list widgets own the drawing, and the widths below are only this
//! module's statement of intent, to be handed to the layout engine.

// ---------------------------------------------------------------------------
// the columns themselves
// ---------------------------------------------------------------------------

/// One column of a track list.
///
/// The declaration order is the order the settings dialog lists them in when
/// it has no layout to go by; it is NOT a display order. Display order lives
/// in [`ColumnLayout`], because it differs per list and the operator owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Title,
    Artist,
    Album,
    Genre,
    Year,
    Bitrate,
    Bpm,
    Key,
    Time,
    Stem,
    Krk,
    Tags,
    Added,
}

pub const ALL: [Column; 13] = [
    Column::Title,
    Column::Artist,
    Column::Album,
    Column::Genre,
    Column::Year,
    Column::Bitrate,
    Column::Bpm,
    Column::Key,
    Column::Time,
    Column::Stem,
    Column::Krk,
    Column::Tags,
    Column::Added,
];

/// How wide a column wants to be. Mirrors the layout engine's `Size`:
/// `Fixed(points)`, or `Fill` with optional min/max.
///
/// Kept as our own type rather than reaching for the draw crate's: this
/// module is the settings model, it is tested on its own, and a track list
/// drawn some other way should still be able to ask a column how big it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    Fixed(f64),
    Fill { min: Option<f64>, max: Option<f64> },
}

impl Column {
    /// Stable settings-file key. Renaming one silently resets that column on
    /// every machine that already has a settings file.
    pub fn slug(self) -> &'static str {
        match self {
            Column::Title => "title",
            Column::Artist => "artist",
            Column::Album => "album",
            Column::Genre => "genre",
            Column::Year => "year",
            Column::Bitrate => "bitrate",
            Column::Bpm => "bpm",
            Column::Key => "key",
            Column::Time => "time",
            Column::Stem => "stem",
            Column::Krk => "krk",
            Column::Tags => "tags",
            Column::Added => "added",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Column> {
        ALL.into_iter().find(|column| column.slug() == slug)
    }

    /// Header text on a normal console.
    pub fn label(self) -> &'static str {
        match self {
            Column::Title => "TITLE",
            Column::Artist => "ARTIST",
            Column::Album => "ALBUM",
            Column::Genre => "GENRE",
            Column::Year => "YEAR",
            Column::Bitrate => "RATE",
            Column::Bpm => "BPM",
            Column::Key => "KEY",
            Column::Time => "TIME",
            Column::Stem => "STEM",
            Column::Krk => "KRK",
            Column::Tags => "TAGS",
            Column::Added => "ADDED",
        }
    }

    /// Header text when the console is too narrow for the full word.
    ///
    /// An abbreviation, never an ellipsis: a header clipped mid-word reads as
    /// a rendering fault, and at this size the operator is recognising the
    /// column by position anyway.
    pub fn short(self) -> &'static str {
        match self {
            Column::Title => "TITLE",
            Column::Artist => "ART",
            Column::Album => "ALB",
            Column::Genre => "GEN",
            Column::Year => "YR",
            Column::Bitrate => "RT",
            Column::Bpm => "BPM",
            Column::Key => "KEY",
            Column::Time => "TM",
            Column::Stem => "S",
            Column::Krk => "K",
            Column::Tags => "TAG",
            Column::Added => "ADD",
        }
    }

    /// What this column asks the layout engine for.
    ///
    /// The fixed ones are sized to their widest real value — a three-digit
    /// tempo, a four-character key, `320` — and give back nothing when the
    /// window shrinks, which is right: they are the columns being scanned
    /// down. The free text columns fill, and only TITLE claims a minimum,
    /// because a listing whose titles have collapsed to nothing is useless
    /// however tidy the rest of the row looks.
    pub fn width(self) -> ColumnWidth {
        match self {
            Column::Title => ColumnWidth::Fill { min: Some(180.0), max: None },
            Column::Artist => ColumnWidth::Fill { min: None, max: Some(150.0) },
            Column::Album => ColumnWidth::Fill { min: None, max: Some(150.0) },
            Column::Genre => ColumnWidth::Fixed(90.0),
            Column::Year => ColumnWidth::Fixed(46.0),
            Column::Bitrate => ColumnWidth::Fixed(58.0),
            Column::Bpm => ColumnWidth::Fixed(54.0),
            Column::Key => ColumnWidth::Fixed(40.0),
            Column::Time => ColumnWidth::Fixed(52.0),
            Column::Stem => ColumnWidth::Fixed(36.0),
            Column::Krk => ColumnWidth::Fixed(30.0),
            Column::Tags => ColumnWidth::Fill { min: None, max: Some(190.0) },
            Column::Added => ColumnWidth::Fixed(100.0),
        }
    }

    /// True when this column can only be filled by the background analysis
    /// pass.
    ///
    /// Tempo and key alone. Everything else — artist, album, genre, year,
    /// bitrate, running time — comes straight out of the file's own tags at
    /// scan time and costs nothing, which is why the new metadata columns can
    /// be offered without a word about preprocessing. Showing an analysis
    /// column, by contrast, is a promise the app can only keep if that pass is
    /// enabled, so the dialog has to be able to tell the two apart.
    pub fn needs_analysis(self) -> bool {
        matches!(self, Column::Bpm | Column::Key)
    }

    fn index(self) -> usize {
        match self {
            Column::Title => 0,
            Column::Artist => 1,
            Column::Album => 2,
            Column::Genre => 3,
            Column::Year => 4,
            Column::Bitrate => 5,
            Column::Bpm => 6,
            Column::Key => 7,
            Column::Time => 8,
            Column::Stem => 9,
            Column::Krk => 10,
            Column::Tags => 11,
            Column::Added => 12,
        }
    }
}

// ---------------------------------------------------------------------------
// one list's arrangement
// ---------------------------------------------------------------------------

/// The explorer as it stands today, plus the metadata columns behind it.
///
/// The first nine are exactly what the listing draws now, in the order it
/// draws them; the four that were added with this dialog sit at the end,
/// hidden, so an operator who never opens the dialog sees no change at all.
const EXPLORER_ORDER: [Column; 13] = [
    Column::Title,
    Column::Artist,
    Column::Album,
    Column::Bpm,
    Column::Key,
    Column::Time,
    Column::Stem,
    Column::Krk,
    Column::Tags,
    Column::Genre,
    Column::Year,
    Column::Bitrate,
    Column::Added,
];

const EXPLORER_SHOWN: [Column; 8] = [
    Column::Title,
    Column::Artist,
    Column::Bpm,
    Column::Key,
    Column::Time,
    Column::Stem,
    Column::Krk,
    Column::Tags,
];

/// The set list, which is a narrow strip beside the decks rather than a table.
///
/// Only three columns fit honestly, and the operator asked for tempo and key
/// to be two of them: the question being answered while looking at the set
/// list is "does the next one mix", and that is what answers it.
const QUEUE_ORDER: [Column; 13] = [
    Column::Title,
    Column::Bpm,
    Column::Key,
    Column::Time,
    Column::Artist,
    Column::Album,
    Column::Genre,
    Column::Year,
    Column::Bitrate,
    Column::Stem,
    Column::Krk,
    Column::Tags,
    Column::Added,
];

const QUEUE_SHOWN: [Column; 3] = [Column::Title, Column::Bpm, Column::Key];

/// One list's columns: every column, in display order, each shown or not.
///
/// Every operation on this preserves the invariant that `order` holds all
/// twelve columns exactly once. Nothing outside can break it, because nothing
/// outside can build one of these except through the constructors and
/// [`ColumnLayout::from_text`], and both go through [`ColumnLayout::seal`].
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnLayout {
    order: Vec<Column>,
    /// Indexed by [`Column::index`], not parallel to `order`: reordering must
    /// not disturb what is shown, and keeping visibility in its own fixed slot
    /// is what makes that impossible to get wrong.
    shown: [bool; 13],
}

impl ColumnLayout {
    pub fn explorer_default() -> ColumnLayout {
        ColumnLayout::build(&EXPLORER_ORDER, &EXPLORER_SHOWN)
    }

    pub fn queue_default() -> ColumnLayout {
        ColumnLayout::build(&QUEUE_ORDER, &QUEUE_SHOWN)
    }

    fn build(order: &[Column], shown: &[Column]) -> ColumnLayout {
        let mut out = ColumnLayout { order: ColumnLayout::seal(order.to_vec()), shown: [false; 13] };
        for column in shown {
            out.shown[column.index()] = true;
        }
        out
    }

    /// Make any sequence of columns into a legal display order: first mention
    /// wins, duplicates are dropped, and anything missing is appended in the
    /// shipped explorer order so that a settings file from an older build
    /// gains the new columns in a sane arrangement rather than a random one.
    fn seal(order: Vec<Column>) -> Vec<Column> {
        let mut out: Vec<Column> = Vec::with_capacity(ALL.len());
        for column in order.into_iter().chain(EXPLORER_ORDER) {
            if !out.contains(&column) {
                out.push(column);
            }
        }
        out
    }

    /// Every column, in display order (hidden ones included, in place).
    ///
    /// This is what the settings dialog draws: a hidden column is still a row
    /// there, with its checkbox off and its arrows live.
    pub fn order(&self) -> &[Column] {
        &self.order
    }

    pub fn is_visible(&self, column: Column) -> bool {
        self.shown[column.index()]
    }

    /// Only the shown columns, in display order. This is what a track list
    /// iterates.
    pub fn visible(&self) -> Vec<Column> {
        self.order.iter().copied().filter(|column| self.is_visible(*column)).collect()
    }

    pub fn set_visible(&mut self, column: Column, on: bool) {
        self.shown[column.index()] = on;
    }

    pub fn toggle(&mut self, column: Column) {
        self.shown[column.index()] = !self.shown[column.index()];
    }

    /// Move one column one place earlier in the display order.
    ///
    /// A no-op at the front. It does NOT wrap: the operator is clicking an
    /// arrow repeatedly to walk a column up a list, and a column that leapt to
    /// the bottom on the last click would be a trap, not a convenience.
    pub fn move_up(&mut self, column: Column) {
        let Some(at) = self.position(column) else { return };
        if at > 0 {
            self.order.swap(at - 1, at);
        }
    }

    /// Move one column one place later in the display order. A no-op at the
    /// end, and no wrapping, for the same reason as [`ColumnLayout::move_up`].
    pub fn move_down(&mut self, column: Column) {
        let Some(at) = self.position(column) else { return };
        if at + 1 < self.order.len() {
            self.order.swap(at, at + 1);
        }
    }

    fn position(&self, column: Column) -> Option<usize> {
        self.order.iter().position(|held| *held == column)
    }

    /// One line's worth of value, for a `key value` settings file: the slugs
    /// in display order, comma separated, hidden ones prefixed with `-`.
    ///
    /// Every column is written every time, hidden included. That costs a
    /// hundred bytes and buys the thing that matters — the file states the
    /// whole arrangement, so reading it back cannot depend on what the
    /// defaults happened to be on the day it was written.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for column in &self.order {
            if !out.is_empty() {
                out.push(',');
            }
            if !self.is_visible(*column) {
                out.push('-');
            }
            out.push_str(column.slug());
        }
        out
    }

    /// Read a line back.
    ///
    /// Deliberately unfailing. A column layout is a preference, and there is
    /// no reading of a mangled line that justifies refusing to start: an
    /// unknown slug is passed over, a repeated one is placed once, and a line
    /// with nothing recognisable in it — an empty value, a truncated file, a
    /// line from some future build — reads as the shipped explorer layout, on
    /// the grounds that a listing showing its usual columns is a better answer
    /// to a bad file than a listing showing none.
    pub fn from_text(text: &str) -> ColumnLayout {
        let mut order: Vec<Column> = Vec::with_capacity(ALL.len());
        let mut shown = [false; 13];
        for token in text.split(',') {
            let token = token.trim();
            let (hidden, slug) = match token.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, token),
            };
            let Some(column) = Column::from_slug(slug) else { continue };
            // First mention wins, so a file that somehow names a column twice
            // cannot end up with it in two places or in two states.
            if order.contains(&column) {
                continue;
            }
            shown[column.index()] = !hidden;
            order.push(column);
        }
        if order.is_empty() {
            return ColumnLayout::explorer_default();
        }
        // Anything the line never mentioned is appended hidden: a build that
        // adds a column must not have it appear unasked in an arrangement the
        // operator already settled.
        ColumnLayout { order: ColumnLayout::seal(order), shown }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant the whole module rests on, asserted after anything that
    /// could disturb it.
    fn assert_whole(layout: &ColumnLayout) {
        assert_eq!(layout.order().len(), ALL.len(), "the order lost or gained a column");
        for column in ALL {
            assert_eq!(
                layout.order().iter().filter(|held| **held == column).count(),
                1,
                "{} is not present exactly once",
                column.slug(),
            );
        }
    }

    fn slugs(columns: &[Column]) -> Vec<&'static str> {
        columns.iter().map(|column| column.slug()).collect()
    }

    #[test]
    fn the_two_defaults_are_what_each_list_shows_today() {
        let explorer = ColumnLayout::explorer_default();
        assert_whole(&explorer);
        assert_eq!(
            slugs(&explorer.visible()),
            vec!["title", "artist", "bpm", "key", "time", "stem", "krk", "tags"],
        );
        // The metadata columns are there, in place, waiting to be switched on.
        assert!(!explorer.is_visible(Column::Album));
        assert!(!explorer.is_visible(Column::Genre));
        assert!(!explorer.is_visible(Column::Year));
        assert!(!explorer.is_visible(Column::Bitrate));
        assert_eq!(explorer.order()[2], Column::Album, "album keeps its old place, hidden");

        let queue = ColumnLayout::queue_default();
        assert_whole(&queue);
        assert_eq!(slugs(&queue.visible()), vec!["title", "bpm", "key"]);
    }

    #[test]
    fn both_defaults_survive_a_trip_through_the_settings_line() {
        for layout in [ColumnLayout::explorer_default(), ColumnLayout::queue_default()] {
            assert_eq!(ColumnLayout::from_text(&layout.to_text()), layout);
        }
        assert_eq!(
            ColumnLayout::queue_default().to_text(),
            "title,bpm,key,-time,-artist,-album,-genre,-year,-bitrate,-stem,-krk,-tags,-added",
        );
    }

    #[test]
    fn a_scrambled_hidden_heavy_layout_survives_a_trip_through_the_settings_line() {
        let mut layout = ColumnLayout::explorer_default();
        for column in ALL {
            layout.set_visible(column, false);
        }
        layout.set_visible(Column::Tags, true);
        layout.set_visible(Column::Year, true);
        for _ in 0..6 {
            layout.move_up(Column::Bitrate);
        }
        layout.move_down(Column::Title);
        layout.move_up(Column::Tags);
        assert_whole(&layout);
        let back = ColumnLayout::from_text(&layout.to_text());
        assert_eq!(back, layout);
        assert_eq!(back.order(), layout.order(), "order came back in the order it went");
        assert_eq!(slugs(&back.visible()), slugs(&layout.visible()));
    }

    #[test]
    fn an_older_settings_file_keeps_its_order_and_gains_the_new_columns_hidden() {
        // Exactly what a build before the metadata columns would have written.
        let layout = ColumnLayout::from_text("title,artist,bpm,key,time,stem,krk,tags");
        assert_whole(&layout);
        assert_eq!(
            slugs(&layout.order()[..8]),
            vec!["title", "artist", "bpm", "key", "time", "stem", "krk", "tags"],
            "the operator's own order is untouched",
        );
        assert_eq!(
            slugs(&layout.order()[8..]),
            vec!["album", "genre", "year", "bitrate", "added"],
            "the columns this build added land at the end",
        );
        for column in [Column::Album, Column::Genre, Column::Year, Column::Bitrate, Column::Added] {
            assert!(!layout.is_visible(column), "a new column does not appear unasked");
        }
        assert_eq!(slugs(&layout.visible()).len(), 8);
    }

    #[test]
    fn a_slug_this_build_does_not_know_is_passed_over() {
        let layout = ColumnLayout::from_text("title,waveform,-artist,rating,bpm");
        assert_whole(&layout);
        assert_eq!(slugs(&layout.order()[..3]), vec!["title", "artist", "bpm"]);
        assert!(layout.is_visible(Column::Title));
        assert!(!layout.is_visible(Column::Artist), "the minus still applied");
        assert!(layout.is_visible(Column::Bpm));
    }

    #[test]
    fn a_slug_named_twice_is_placed_once_and_in_its_first_state() {
        let layout = ColumnLayout::from_text("title,bpm,title,-bpm,-title");
        assert_whole(&layout);
        assert_eq!(slugs(&layout.order()[..2]), vec!["title", "bpm"]);
        assert!(layout.is_visible(Column::Title), "the first mention decides");
        assert!(layout.is_visible(Column::Bpm));
    }

    #[test]
    fn an_empty_or_meaningless_line_reads_as_the_shipped_explorer_layout() {
        for text in ["", "   ", ",,,", "-", "nonsense,rating,,bogus", "\u{1f3a7}"] {
            let layout = ColumnLayout::from_text(text);
            assert_whole(&layout);
            assert_eq!(
                layout,
                ColumnLayout::explorer_default(),
                "{text:?} should read as the shipped layout, not as an empty list",
            );
        }
    }

    #[test]
    fn moving_a_column_off_either_end_leaves_the_order_alone() {
        let mut layout = ColumnLayout::explorer_default();
        let before = layout.order().to_vec();
        let first = before[0];
        let last = before[before.len() - 1];
        for _ in 0..3 {
            layout.move_up(first);
            layout.move_down(last);
        }
        assert_eq!(layout.order(), before.as_slice(), "the ends do not wrap");
        assert_whole(&layout);
    }

    #[test]
    fn a_column_walked_to_the_far_end_and_back_returns_to_where_it_started() {
        let mut layout = ColumnLayout::explorer_default();
        let before = layout.order().to_vec();
        // Past the end on purpose: the extra clicks must cost nothing.
        for _ in 0..(ALL.len() + 4) {
            layout.move_down(Column::Title);
        }
        assert_whole(&layout);
        assert_eq!(*layout.order().last().expect("a column"), Column::Title);
        for _ in 0..(ALL.len() + 4) {
            layout.move_up(Column::Title);
        }
        assert_eq!(layout.order(), before.as_slice());
        assert_whole(&layout);
    }

    #[test]
    fn moving_one_column_disturbs_neither_the_others_nor_what_is_shown() {
        let mut layout = ColumnLayout::explorer_default();
        let shown_before = slugs(&layout.visible());
        layout.move_down(Column::Artist);
        assert_eq!(
            slugs(layout.order()),
            vec![
                "title", "album", "artist", "bpm", "key", "time", "stem", "krk", "tags", "genre",
                "year", "bitrate", "added",
            ],
            "artist and album swapped, nothing else moved",
        );
        assert_eq!(
            slugs(&layout.visible()),
            shown_before,
            "stepping over a hidden column changes nothing the operator can see",
        );
        assert!(!layout.is_visible(Column::Album), "a move is not an unhide");
        assert!(layout.is_visible(Column::Artist));
        // Stepping over a shown one does change it, and by exactly one place.
        layout.move_up(Column::Bpm);
        layout.move_up(Column::Bpm);
        assert_eq!(
            slugs(&layout.visible()),
            vec!["title", "bpm", "artist", "key", "time", "stem", "krk", "tags"],
        );
        assert_whole(&layout);
    }

    #[test]
    fn toggling_flips_one_column_and_leaves_the_rest_where_they_were() {
        let mut layout = ColumnLayout::explorer_default();
        layout.toggle(Column::Genre);
        assert!(layout.is_visible(Column::Genre));
        layout.toggle(Column::Title);
        assert!(!layout.is_visible(Column::Title));
        for column in ALL {
            if column == Column::Genre || column == Column::Title {
                continue;
            }
            assert_eq!(
                layout.is_visible(column),
                ColumnLayout::explorer_default().is_visible(column),
                "{} changed and should not have",
                column.slug(),
            );
        }
        layout.toggle(Column::Genre);
        layout.toggle(Column::Title);
        assert_eq!(layout, ColumnLayout::explorer_default(), "two toggles are no toggles");
        // set_visible is idempotent where toggle is not.
        layout.set_visible(Column::Genre, false);
        layout.set_visible(Column::Genre, false);
        assert!(!layout.is_visible(Column::Genre));
    }

    #[test]
    fn the_shown_columns_are_the_order_with_the_hidden_ones_struck_out() {
        let mut layout = ColumnLayout::queue_default();
        layout.set_visible(Column::Tags, true);
        layout.move_up(Column::Tags);
        let expected: Vec<Column> = layout
            .order()
            .iter()
            .copied()
            .filter(|column| layout.is_visible(*column))
            .collect();
        assert_eq!(layout.visible(), expected);
        // And it really is a subsequence of the display order, not a re-sort.
        let mut walk = layout.order().iter();
        for column in layout.visible() {
            assert!(walk.any(|held| *held == column), "{} is out of order", column.slug());
        }
    }

    #[test]
    fn hiding_every_column_is_allowed_and_costs_nothing_here() {
        let mut layout = ColumnLayout::explorer_default();
        for column in ALL {
            layout.set_visible(column, false);
        }
        assert!(layout.visible().is_empty());
        assert_whole(&layout);
        // Still writable, still readable, still the same arrangement: the UI
        // decides what to do about an empty list, this model does not care.
        let back = ColumnLayout::from_text(&layout.to_text());
        assert_eq!(back, layout);
        assert!(back.visible().is_empty());
        // And one click brings a column back exactly where it was left.
        layout.set_visible(Column::Key, true);
        assert_eq!(layout.visible(), vec![Column::Key]);
    }

    #[test]
    fn every_column_is_present_exactly_once_after_any_run_of_edits() {
        let mut layout = ColumnLayout::queue_default();
        // A deterministic thrash: walk every column around, flipping as it
        // goes, and check the invariant after each step.
        let mut step = 0usize;
        for _ in 0..4 {
            for column in ALL {
                step += 1;
                match step % 3 {
                    0 => layout.move_up(column),
                    1 => layout.move_down(column),
                    _ => layout.toggle(column),
                }
                assert_whole(&layout);
            }
        }
        assert_eq!(ColumnLayout::from_text(&layout.to_text()), layout);
    }

    #[test]
    fn the_slugs_are_unique_and_name_their_own_column_back() {
        for column in ALL {
            assert_eq!(Column::from_slug(column.slug()), Some(column));
            assert!(!column.slug().is_empty());
            assert!(
                !column.slug().contains(',') && !column.slug().starts_with('-'),
                "a slug must not collide with the line's own punctuation",
            );
            assert_eq!(
                ALL.iter().filter(|other| other.slug() == column.slug()).count(),
                1,
                "two columns share the slug {}",
                column.slug(),
            );
        }
        assert_eq!(Column::from_slug("rating"), None);
        assert_eq!(Column::from_slug("TITLE"), None, "slugs are lower case, and exactly matched");
        assert_eq!(Column::from_slug(""), None);
    }

    #[test]
    fn tempo_and_key_are_the_only_columns_that_wait_on_the_analysis_pass() {
        for column in ALL {
            assert_eq!(
                column.needs_analysis(),
                column == Column::Bpm || column == Column::Key,
                "{} is on the wrong side of the analysis line",
                column.slug(),
            );
        }
    }

    #[test]
    fn a_headers_short_form_is_never_longer_than_the_full_one() {
        for column in ALL {
            assert!(!column.label().is_empty());
            assert!(
                column.short().len() <= column.label().len(),
                "{} abbreviates to something longer",
                column.slug(),
            );
        }
    }

    #[test]
    fn the_widths_are_what_the_console_draws() {
        assert_eq!(Column::Title.width(), ColumnWidth::Fill { min: Some(180.0), max: None });
        assert_eq!(Column::Tags.width(), ColumnWidth::Fill { min: None, max: Some(190.0) });
        assert_eq!(Column::Key.width(), ColumnWidth::Fixed(40.0));
        // Title is the only column that refuses to be squeezed away.
        for column in ALL {
            if let ColumnWidth::Fill { min: Some(_), .. } = column.width() {
                assert_eq!(column, Column::Title);
            }
        }
    }
}
