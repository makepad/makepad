//! First-class architecture workspaces and tools over generic tagged documents.

pub mod drawing;
pub mod plan;
mod tour_bridge;
pub mod walk;

pub use drawing::{CameraMarker, Drawing2D, Drawing2DEditor, EdgeClass, Primitive};
pub use plan::{generate as generate_plan, levels, PlanOptions};
pub use tour_bridge::from_document as tour_scene;
pub use walk::{front_door_entry, WalkEntry, WalkSettings};

/// Product workspaces supplied by the architecture feature area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchitectureWorkspace {
    Quad,
    Walkthrough,
    Sections,
    Sun,
}

pub const WORKSPACES: [ArchitectureWorkspace; 4] = [
    ArchitectureWorkspace::Quad,
    ArchitectureWorkspace::Walkthrough,
    ArchitectureWorkspace::Sections,
    ArchitectureWorkspace::Sun,
];

/// Automated analysis, route planning, camera tracks, QA, and track rendering.
pub mod tours {
    pub use makepad_fab_tour::*;
    pub use super::tour_bridge::from_document;
}

pub mod sections {
    pub use crate::tools::section::*;
}

pub mod sun_study {
    pub use crate::tools::sun_study::*;
}

pub mod measure {
    pub use crate::tools::measure::*;
}

pub mod isolate {
    pub use crate::tools::isolate::*;
}

pub mod explode {
    pub use crate::tools::explode::*;
}

pub use crate::sheets::view::FabSheetView as Drawing2DArea;
