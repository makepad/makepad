//! Lane E: the in-flight tool state.
//!
//! Every *result* a tool produces lives in `AppState` — measurements,
//! `scene_state.section`, `scene_state.explode`, `sun` — exactly as `api.rs`
//! prescribes. What lives here is the half-finished interaction that has no
//! home in the frozen contract: the measurement being placed, the snap under
//! the cursor, a section-handle drag, the section animate-in, the day scrub.
//!
//! It has to be shared between three owners that never see each other:
//! `ToolSet` (one per viewport, inside the viewport's input chain),
//! `FabToolOverlay` (the 2D overlay widget) and `FabToolPanel` (the N-panel).
//! All three run on the UI thread, so this is a thread-local singleton rather
//! than a field. Nothing here is read by another lane; nothing survives a
//! reload (`reset_for_scene`).

use crate::api::*;
use crate::model::units::LengthUnit;
use makepad_widgets::*;
use std::cell::RefCell;

/// A measurement being placed: the committed points and the live preview.
#[derive(Clone, Debug, Default)]
pub struct MeasureDraft {
    pub kind: MeasureKind,
    pub points: Vec<Vec3f>,
    /// How each point was snapped (same length as `points`), for the glyphs.
    pub snaps: Vec<SnapKind>,
    /// Where the cursor currently is, snapped. Drives the rubber band.
    pub preview: Option<SnapHit>,
}

impl MeasureDraft {
    pub fn clear(&mut self) {
        self.points.clear();
        self.snaps.clear();
        self.preview = None;
    }

    pub fn undo(&mut self) {
        self.points.pop();
        self.snaps.pop();
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// Which draggable thing the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionHandle {
    /// Offset handle of `section.planes[i]`.
    Plane(usize),
    /// Face `0..6` of the section box: −X +X −Y +Y −Z +Z.
    BoxFace(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct SectionDrag {
    pub handle: SectionHandle,
    /// Screen position where the drag started.
    pub start: DVec2,
    /// Section value at drag start (offset in meters / box face coordinate).
    pub start_value: f32,
    /// Screen-space direction (unit) the handle moves along, and how many
    /// points one meter of movement covers.
    pub axis_screen: DVec2,
    pub points_per_meter: f64,
}

/// Section animate-in: eases the whole `SectionState` from `from` to `to`.
#[derive(Clone, Debug)]
pub struct SectionAnim {
    pub from: SectionState,
    pub to: SectionState,
    /// 0..1.
    pub t: f32,
    pub duration: f32,
}

/// The day-scrub animation (Sun Study "play day").
#[derive(Clone, Copy, Debug, Default)]
pub struct SunPlay {
    pub playing: bool,
    /// Which viewport's overlay drives the clock, so a second viewport does
    /// not double the speed.
    pub owner: usize,
    /// Hours of model time per second of wall clock.
    pub speed: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ToolSession {
    pub measure: MeasureDraft,
    /// Snap under the cursor right now (drives the snap glyph).
    pub snap_cursor: Option<SnapHit>,
    /// Box select rectangle while dragging with the Select tool.
    pub box_select: Option<(DVec2, DVec2)>,
    pub section_drag: Option<SectionDrag>,
    pub section_hover: Option<SectionHandle>,
    pub section_anim: Option<SectionAnim>,
    pub sun_play: SunPlay,
    /// Element info card (I) — the floating card next to the active element.
    pub info_card: bool,
    /// Display unit override for measurement labels. `None` = the scene's own
    /// `Units::display` (`Scene` is immutable, so the preference lives here).
    pub display_unit: Option<LengthUnit>,
    /// One-line tool feedback drawn under the view label.
    pub hint: String,
}

impl ToolSession {
    /// Wipe everything that refers to a scene (called when one is loaded).
    pub fn reset_for_scene(&mut self) {
        self.measure.clear();
        self.snap_cursor = None;
        self.box_select = None;
        self.section_drag = None;
        self.section_hover = None;
        self.section_anim = None;
        self.info_card = false;
        self.hint.clear();
    }

    /// The unit measurements are displayed in.
    pub fn units(&self, scene_units: &Units) -> Units {
        match self.display_unit {
            Some(u) => Units {
                display: u,
                precision: match u {
                    LengthUnit::Millimeter => 0,
                    LengthUnit::Centimeter => 1,
                    _ => 3,
                },
                ..*scene_units
            },
            None => *scene_units,
        }
    }

    /// True while something wants per-frame updates.
    pub fn wants_frames(&self) -> bool {
        self.sun_play.playing || self.section_anim.is_some()
    }
}

thread_local! {
    static SESSION: RefCell<ToolSession> = RefCell::new(ToolSession::default());
}

/// Borrow the session. Never nest these calls.
pub fn with<R>(f: impl FnOnce(&mut ToolSession) -> R) -> R {
    SESSION.with(|s| f(&mut s.borrow_mut()))
}
