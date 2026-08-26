//! 2D sheets / layouts (plans, sections, elevations, layout books).
//!
//! Coordinates are **sheet millimeters**, origin bottom-left, Y up (paper
//! space). `scale` relates paper to model (1:100 → `100.0`). The sheet viewer
//! (lane E) draws these with `DrawVector`-class shaders; nothing here knows
//! about pixels.

use crate::model::ids::{ElementId, SheetId};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    /// Line width in sheet millimeters (0 = hairline).
    pub width_mm: f32,
    /// Linear RGBA.
    pub color: [f32; 4],
    /// Dash pattern in sheet millimeters, empty = solid.
    pub dash: [f32; 2],
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke {
            width_mm: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            dash: [0.0, 0.0],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SheetItem {
    /// Polyline; `closed` joins the last point to the first.
    Path {
        points: Vec<[f32; 2]>,
        closed: bool,
        stroke: Stroke,
    },
    /// Filled polygon (evenodd), optional outline.
    Fill {
        points: Vec<[f32; 2]>,
        color: [f32; 4],
        stroke: Option<Stroke>,
    },
    Arc {
        center: [f32; 2],
        radius: f32,
        start_deg: f32,
        end_deg: f32,
        stroke: Stroke,
    },
    Text {
        pos: [f32; 2],
        text: String,
        /// Cap height in sheet millimeters.
        height_mm: f32,
        angle_deg: f32,
        color: [f32; 4],
    },
    /// Hatch region; `pattern` names the source pattern, the viewer maps
    /// unknown names to a generic 45° hatch.
    Hatch {
        points: Vec<[f32; 2]>,
        pattern: String,
        color: [f32; 4],
    },
}

/// A hot rectangle on the sheet that maps back to a 3D element (for
/// sheet ↔ model cross-highlighting).
#[derive(Clone, Debug, PartialEq)]
pub struct SheetLink {
    pub rect_mm: [f32; 4],
    pub element: ElementId,
}

#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub id: SheetId,
    pub name: String,
    /// Paper size in millimeters (A3 landscape = [420, 297]).
    pub size_mm: [f32; 2],
    /// 1:N — 100.0 for 1:100.
    pub scale: f32,
    pub items: Vec<SheetItem>,
    pub links: Vec<SheetLink>,
    /// Which story this sheet documents, when known (plans).
    pub story: Option<crate::model::ids::StoryId>,
}
