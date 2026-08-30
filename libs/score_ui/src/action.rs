use crate::{playback::RoomSettings, sound::SoundParam};
use makepad_piano_model::fx::{Perspective, ReverbPreset};
use makepad_score::model::AnnotationKind;
use makepad_widgets::*;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProductMode {
    #[default]
    Pianist,
    Editor,
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

pub fn perspective_label(perspective: Perspective) -> &'static str {
    match perspective {
        Perspective::Player => "Player",
        Perspective::Audience => "Audience",
    }
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
    SetPerspective(Perspective),
    /// Adopt one of the shipped instrument presets whole: voicing, suggested
    /// room and a clean trim on top of it.
    SetPianoPreset(usize),
    /// Move one continuous sound control. The value is in the parameter's own
    /// unit; the panel converts from slider travel before sending it.
    SetSoundParam { param: SoundParam, value: f32 },
    /// Put every control back where the current preset had it.
    ResetSoundToPreset,
    /// Lift the resonance bed's dampers all the way — the "whole instrument
    /// open" sound, reachable in one press from the panel.
    LiftDampers,
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

