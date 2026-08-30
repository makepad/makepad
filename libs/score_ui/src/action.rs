use crate::playback::RoomSettings;
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
    #[default]
    Single,
    TwoUp,
    Continuous,
    Overview,
}

impl PageLayout {
    pub const ALL: [Self; 4] = [Self::Single, Self::TwoUp, Self::Continuous, Self::Overview];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Single => "Single page",
            Self::TwoUp => "Two-up",
            Self::Continuous => "Continuous",
            Self::Overview => "Overview",
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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InspectorTab {
    #[default]
    Properties,
    Mixer,
    History,
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
    /// Relative nudge of the dry/wet amount, in the same style as part gain.
    SetReverbMix { delta: f32 },
    SetPerspective(Perspective),
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
    OpenPath(PathBuf),
    SavePath(PathBuf),
    Save,
    NewDemo,
    Quit,
    ContextMenu { at: DVec2, semantic: Option<u64> },
    CloseContextMenu,
}

