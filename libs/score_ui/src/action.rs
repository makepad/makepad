use crate::{playback::RoomSettings, sound::SoundParam};
use makepad_piano_model::fx::ReverbPreset;
use crate::sound::InstrumentId;
use makepad_score::model::AnnotationKind;
use makepad_widgets::*;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProductMode {
    #[default]
    Pianist,
    Editor,
}

/// What a drag on the page MEANS. The reader chooses it; it is never inferred
/// from what happened to be under the pointer.
///
/// The old rule — a drag that starts on a note edits that note, a drag on
/// paper moves the page — made every mis-aimed drag a silent edit of the
/// music. So the tool is explicit, the safe one is the default, and the one
/// that moves music has to be asked for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScoreTool {
    /// Read and move about. Dragging anywhere moves the paper — over a note
    /// exactly as over empty staff — and the wheel zooms. Nothing on the page
    /// can be changed by dragging. This is where you land.
    #[default]
    Navigate,
    /// Choose music and operate on it: click a note, drag a band across a run,
    /// ⇧ or ⌘ to add. What is chosen can be transposed and deleted.
    Select,
    /// Direct manipulation: drag a note to change its pitch and its beat,
    /// click a bar to write one, delete what is chosen.
    Edit,
}

impl ScoreTool {
    pub const ALL: [Self; 3] = [Self::Navigate, Self::Select, Self::Edit];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Navigate => "Navigate",
            Self::Select => "Select",
            Self::Edit => "Edit",
        }
    }

    /// The single key that arms it.
    pub const fn shortcut(self) -> &'static str {
        match self {
            Self::Navigate => "H",
            Self::Select => "V",
            Self::Edit => "N",
        }
    }

    /// What the status bar says the pointer will now do.
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Navigate => {
                "Navigate · drag anywhere to move the page, scroll to zoom · notes are safe"
            }
            Self::Select => {
                "Select · click or drag a band over notes · ↑↓ transposes, ⌫ deletes"
            }
            Self::Edit => {
                "Edit · drag a note to move it, click a bar to write one · ⌫ deletes"
            }
        }
    }

    /// True for the tools that may change the music by pointer alone.
    pub const fn edits(self) -> bool {
        matches!(self, Self::Edit)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PageLayout {
    /// The document as one strip, page beside page, left to right.
    #[default]
    Single,
    /// The same strip in openings: two pages to a spread.
    TwoUp,
    /// The strip turned on its side: a column, page above page.
    Continuous,
}

impl PageLayout {
    pub const ALL: [Self; 3] = [Self::Single, Self::TwoUp, Self::Continuous];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Single => "Pages",
            Self::TwoUp => "Two-up",
            Self::Continuous => "Continuous",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AnnotationTool {
    #[default]
    None,
    Highlight,
    Circle,
    Text,
    Fingering,
    Ink,
}

impl AnnotationTool {
    pub const fn kind(self) -> Option<AnnotationKind> {
        match self {
            Self::None => None,
            Self::Highlight => Some(AnnotationKind::Highlight),
            Self::Circle => Some(AnnotationKind::Circle),
            Self::Text => Some(AnnotationKind::Text),
            Self::Fingering => Some(AnnotationKind::Fingering),
            Self::Ink => Some(AnnotationKind::Ink),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DialogKind {
    #[default]
    None,
    Open,
    SaveAs,
    ScoreSetup,
    PageSetup,
    Preferences,
    Keymap,
    About,
    AnnotationText,
    /// The music library browser.
    Library,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InspectorTab {
    #[default]
    Properties,
    Mixer,
    /// Everything that shapes the piano sound, in one panel.
    Sound,
    History,
}

impl InspectorTab {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Properties => "Properties",
            Self::Mixer => "Mixer",
            Self::Sound => "Sound",
            Self::History => "History",
        }
    }
}

/// Labels for the room controls, so the shell never re-derives them.
pub fn room_summary(room: RoomSettings) -> String {
    format!(
        "{} · {:.0}%",
        crate::playback::reverb_preset_label(room.preset),
        room.mix * 100.0
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaletteCommand {
    Staccato,
    Accent,
    Tenuto,
    Sharp,
    Flat,
    Natural,
}

/// One switch in the Preferences dialog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrefToggle {
    StartInEditor,
    AuditionOnHover,
    FollowCursor,
    Metronome,
    CountIn,
    DarkPaper,
}

/// Which native panel a Browse… button asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowseTarget {
    Open,
    SaveDirectory,
    /// The folder the music library lists.
    LibraryDirectory,
}

#[derive(Clone, Debug)]
pub enum ScoreAction {
    SetMode(ProductMode),
    /// Arm a pointer tool. The one thing that decides what a drag means.
    SetTool(ScoreTool),
    /// Move every selected note by this many semitones, as one undo step.
    Transpose(i32),
    /// Remove every selected note, as one undo step.
    DeleteSelection,
    ToggleMode,
    ToggleChrome,
    SetPageLayout(PageLayout),
    PageDelta(i32),
    FirstPage,
    LastPage,
    ZoomBy(f64),
    /// Zoom out until the whole document is in view. There is no separate
    /// overview mode: the overview is where the zoom control ends up.
    FitAllPages,
    FitPage,
    RevealControls(bool),
    PlayPause,
    Stop,
    ToggleMetronome,
    ToggleCountIn,
    ToggleFollow,
    ToggleLoop,
    SetTempo(f64),
    SeekQuarter(f64),
    SetReverbPreset(ReverbPreset),
    /// Move one of the two continuous sound controls. The value is in the
    /// parameter's own unit; the panel converts from slider travel first.
    SetSoundParam { param: SoundParam, value: f32 },
    /// Pick an instrument from the list. The engine follows it.
    SelectInstrument(InstrumentId),
    SetAnnotationTool(AnnotationTool),
    ApplyAnnotationText(String),
    SetInspectorTab(InspectorTab),
    SelectMore,
    SelectAll,
    ClearSelection,
    Undo,
    Redo,
    SetDuration(u8),
    EnterPitch(char),
    ApplyPalette(PaletteCommand),
    SetPartGain { part: usize, delta: f32 },
    SetPartPan { part: usize, delta: f32 },
    TogglePartMute(usize),
    TogglePartSolo(usize),
    OpenDialog(DialogKind),
    CloseDialog,
    /// Escape: the innermost thing that is open, in order.
    Dismiss,
    TogglePref(PrefToggle),
    /// Write the preferences out and close the dialog.
    ApplyPreferences,
    /// Score setup: the tempo the piece plays at.
    ApplyScoreSetup { tempo: f64 },
    /// Page setup: how pages are laid out and how large the staff is drawn.
    ApplyPageSetup { layout: PageLayout, zoom: f64 },
    /// Local, in-dialog edits so the panel shows what Apply will do.
    SetDialogLayout(PageLayout),
    SetDialogZoom(f64),
    SetDialogTempo(f64),
    Browse(BrowseTarget),
    OpenRecent(usize),
    /// Open the piece at this index of the current library page.
    OpenLibraryEntry(usize),
    /// Point the library at another folder and list it.
    SetLibraryDir(PathBuf),
    RescanLibrary,
    /// Page the library list when the folder holds more than it can show.
    LibraryPage(i32),
    OpenPath(PathBuf),
    SavePath(PathBuf),
    Save,
    NewDemo,
    Quit,
    ContextMenu { at: DVec2, semantic: Option<u64> },
    CloseContextMenu,
}

