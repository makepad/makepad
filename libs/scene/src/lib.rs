//! Presentation descriptions and pure geometry shared by Makepad renderers.
pub mod camera;
pub mod decal;
pub mod entity;
pub mod environment;
pub mod heading;
pub mod hud;
pub mod light;
pub mod mesh;
pub mod particles;
pub mod terrain;
pub mod voxel;
pub mod water;
pub mod world;

pub use camera::{camera_shake_offset, CameraEffects, CameraState};
pub use decal::*;
pub use entity::*;
pub use environment::*;
pub use heading::*;
pub use hud::{layout as hud_layout, Crosshair, CrosshairStyle, HudAlign, HudAnchor, HudBar,
    HudDoc, HudElement, HudKind, HudLine, HudPlaced, HudPulse, HudSlot, HudStack, HudValue};
pub use light::*;
pub use particles::*;
pub use terrain::*;
pub use voxel::{ChunkKey, ChunkMesh, VoxelView};
pub use water::{WaterSurface, WaterView, WaterWave, MAX_WAVES};
pub use world::*;
