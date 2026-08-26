//! Generic document and scene data shared by loaders, tools, and renderers.
//!
//! Sits between format loaders (`libs/loaders/*`, plain data out) and the shell
//! viewport renderer (`crate::viewport`). Everything in this module is **pure
//! data**: no GPU handles, no `Cx`, no threads. A [`Scene`] is
//! built once on a loader thread from a [`ModelData`] and then shared as
//! `Arc<Scene>` with the UI thread. Mutable per-session state (selection,
//! visibility, section planes, explode) lives in [`SceneState`] and is versioned
//! with a `revision` counter so consumers can diff cheaply.
//!
//! ## Coordinate law
//! Inside this crate and everything downstream: **right-handed, Z up, meters**.
//! `ModelData` declares the source units and up-axis; `Scene::from_model`
//! normalises. Nothing after that ever converts again.
//!
//! ## Boundary contract
//! * Loader → model: [`model::ModelData`] (the only thing a parser must produce).
//! * Model → viewport: [`Scene`] (`batches` are GPU-ready CPU buffers), [`SceneState`],
//!   [`query::Ray`]/[`query::RayHit`]/[`query::Frustum`].
//! * Model → tools/UI: element tree, properties, quantities, groups,
//!   sheets, units formatting, snapping and measurement geometry.

pub mod batch;
pub mod bounds;
pub mod bvh;
pub mod demo;
pub mod ids;
pub mod material_semantics;
pub mod model;
pub mod overlap;
pub mod palette;
pub mod query;
pub mod scene;
pub mod sheets;
pub mod snapshot;
pub mod source;
pub mod state;
pub mod units;

pub use batch::{ElementRange, RenderBatch, Vertex, VERTEX_STRIDE};
pub use snapshot::{
    SceneSnapshot, SnapshotCamera, SnapshotElement, SnapshotLight, SnapshotMaterial, SnapshotStory,
    SnapshotTexture,
};
pub use bounds::{aabb_center, aabb_empty, aabb_extent, aabb_is_empty, aabb_radius, aabb_union, aabb_union_point, aabb_transform};
pub use bvh::{Bvh, PickOptions};
pub use ids::{ElementId, LayerId, MaterialId, MeshId, SheetId, StoryId};
pub use model::{
    CameraData, CameraSetData, ElementClass, ElementData, Handedness, LayerData, MaterialData,
    MeshData, MeshRef, ModelData, Property, PropertyValue, Quantity, StoryData, SubMesh,
    TextureData, UpAxis,
};
pub use overlap::{OverlapRecord, OverlapReport, DEFAULT_COPLANAR_TOL_M};
pub use query::{
    angle_deg, closest_point_on_segment, distance, polygon_area, snap, Frustum, MeasureKind, Ray,
    RayHit, ScreenProject, SnapHit, SnapKind, SnapOptions,
};
pub use scene::{
    Element, ElementTree, Layer, Material, Scene, SceneCamera, SceneCameraSet, ScenePart, SceneStats,
    Story,
    MAX_BATCH_VERTICES, MAX_CONTOUR_SEGMENTS, TARGET_BATCH_TRIANGLES,
};
pub use sheets::{Sheet, SheetItem, SheetLink, Stroke};
pub use source::{DemoLoader, LoadCancel, LoadError, LoadProgress, Loader};
pub use state::{ExplodeMode, ExplodeState, SceneState, SectionPlane, SectionState, Selection};
pub use units::{LengthUnit, Units};

pub use makepad_math;
pub use crate::providers::{DocumentProvider, SceneProvider};
