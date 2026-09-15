//! Document and session types. No widget or window types.

use std::fmt;

pub const DOCUMENT_VERSION: u32 = 1;
pub const SEED_VERSION: u32 = 1;
pub const MAX_NOTES: usize = 200;
pub const MAX_SOURCE_BYTES: usize = 32 * 1024;
pub const MAX_AGGREGATE_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_STORAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_UNDO: usize = 32;
pub const UNDO_COALESCE_MS: i64 = 500;
pub const SAVE_DEBOUNCE_MS: i64 = 300;
pub const MAX_SEARCH_HITS: usize = 50;
pub const MAX_SEARCH_QUERY_BYTES: usize = 256;
pub const MAX_PREVIEW_CHARS: usize = 160;
pub const MAX_SEARCH_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_READ_OUTPUT_BYTES: usize = 256 * 1024;
pub const STORAGE_KEY: &str = "notes.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(pub u64);

impl NoteId {
    pub fn wire(self) -> String {
        format!("n{:06}", self.0)
    }

    pub fn parse(s: &str) -> Option<Self> {
        let rest = s.strip_prefix('n')?;
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        rest.parse::<u64>().ok().map(NoteId)
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.wire())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FolderId {
    Notes,
    Work,
    Personal,
}

impl FolderId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Notes => "notes",
            Self::Work => "work",
            Self::Personal => "personal",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "notes" => Some(Self::Notes),
            "work" => Some(Self::Work),
            "personal" => Some(Self::Personal),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Notes => "Notes",
            Self::Work => "Work",
            Self::Personal => "Personal",
        }
    }

    pub const ALL: [FolderId; 3] = [FolderId::Notes, FolderId::Work, FolderId::Personal];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Collection {
    All,
    Folder(FolderId),
    RecentlyDeleted,
}

impl Collection {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All Notes",
            Self::Folder(f) => f.label(),
            Self::RecentlyDeleted => "Recently Deleted",
        }
    }

    pub fn search_placeholder(self) -> String {
        match self {
            Self::All => "Search Notes".to_string(),
            Self::Folder(f) => format!("Search {}", f.label()),
            Self::RecentlyDeleted => "Search Recently Deleted".to_string(),
        }
    }

    pub fn allows_create(self) -> bool {
        !matches!(self, Self::RecentlyDeleted)
    }

    pub fn create_folder(self) -> FolderId {
        match self {
            Self::Folder(f) => f,
            Self::All | Self::RecentlyDeleted => FolderId::Notes,
        }
    }

    pub const ALL: [Collection; 5] = [
        Collection::All,
        Collection::Folder(FolderId::Notes),
        Collection::Folder(FolderId::Work),
        Collection::Folder(FolderId::Personal),
        Collection::RecentlyDeleted,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Folders,
    List,
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Read,
    Edit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: NoteId,
    pub folder: FolderId,
    pub source: String,
    pub pinned: bool,
    pub created_ms: i64,
    pub edited_ms: i64,
    pub deleted_ms: Option<i64>,
}

impl Note {
    pub fn is_deleted(&self) -> bool {
        self.deleted_ms.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotesDocument {
    pub version: u32,
    pub seed_version: u32,
    pub seed_anchor_ms: i64,
    pub next_id: u64,
    pub revision: u64,
    pub notes: Vec<Note>,
}

impl Default for NotesDocument {
    fn default() -> Self {
        Self::empty(0)
    }
}

impl NotesDocument {
    pub fn empty(anchor_ms: i64) -> Self {
        Self {
            version: DOCUMENT_VERSION,
            seed_version: SEED_VERSION,
            seed_anchor_ms: anchor_ms,
            next_id: 1,
            revision: 0,
            notes: Vec::new(),
        }
    }

    pub fn note(&self, id: NoteId) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    pub fn note_mut(&mut self, id: NoteId) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ByteSelection {
    pub anchor: usize,
    pub cursor: usize,
}

impl ByteSelection {
    pub fn caret(at: usize) -> Self {
        Self { anchor: at, cursor: at }
    }

    pub fn start(self) -> usize {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> usize {
        self.anchor.max(self.cursor)
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn clamp(self, len: usize) -> Self {
        Self {
            anchor: self.anchor.min(len),
            cursor: self.cursor.min(len),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditSnapshot {
    pub source: String,
    pub selection: ByteSelection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditSession {
    pub note_id: NoteId,
    pub mode: EditorMode,
    pub selection: ByteSelection,
    pub undo: Vec<EditSnapshot>,
    pub redo: Vec<EditSnapshot>,
    pub last_type_ms: Option<i64>,
    pub last_type_end: Option<usize>,
}

impl EditSession {
    pub fn new(note_id: NoteId) -> Self {
        Self {
            note_id,
            mode: EditorMode::Read,
            selection: ByteSelection::caret(0),
            undo: Vec::new(),
            redo: Vec::new(),
            last_type_ms: None,
            last_type_end: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotesUi {
    pub collection: Collection,
    pub selected: Option<NoteId>,
    pub query: String,
    pub route: Vec<Screen>,
    pub editor: Option<EditSession>,
}

impl Default for NotesUi {
    fn default() -> Self {
        Self {
            collection: Collection::All,
            selected: None,
            query: String::new(),
            route: vec![Screen::Folders],
            editor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewBlock {
    Title(String),
    Markdown(String),
    Checklist {
        line_start: usize,
        checked: bool,
        label_markdown: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub id: NoteId,
    pub title: String,
    pub preview: String,
    pub folder: FolderId,
    pub edited_ms: i64,
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateSection {
    Pinned,
    Today,
    Yesterday,
    Previous7Days,
    Older,
    Notes,
    Deleted,
}

impl DateSection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinned => "Pinned",
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::Previous7Days => "Previous 7 Days",
            Self::Older => "Older",
            Self::Notes => "Notes",
            Self::Deleted => "Recently Deleted",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListRow {
    Section(DateSection),
    Note(SearchHit),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Compact,
    Wide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Columns {
    Stack,
    Two,
    Three,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutDecision {
    pub family: Family,
    pub columns: Columns,
    pub short_chrome: bool,
}

impl Default for LayoutDecision {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl LayoutDecision {
    pub const DEFAULT: Self = Self {
        family: Family::Wide,
        columns: Columns::Three,
        short_chrome: false,
    };

    pub fn is_compact(self) -> bool {
        self.family == Family::Compact
    }

    pub fn is_three(self) -> bool {
        self.columns == Columns::Three
    }

    pub fn is_two(self) -> bool {
        self.columns == Columns::Two
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockStyle {
    Title,
    Heading,
    Body,
    Checklist,
    Bullets,
    Numbers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineStyle {
    Bold,
    Italic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Typing,
    Paste,
    Format,
    Check,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LoadState {
    #[default]
    Loading,
    Ready,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderCount {
    pub collection: Collection,
    pub count: usize,
}
