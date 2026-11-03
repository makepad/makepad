//! The field-domain planner boundary. Geometry is in printer-frame nanometres;
//! distances and widths are in millimetres. This module has no rendering dependency.
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Join {
    Miter(f32),
    Round,
}

#[derive(Clone, Debug)]
pub struct FirstLayer {
    pub elephant_foot: f32,
}

#[derive(Clone, Debug)]
pub struct PlanSpec {
    pub zs: Arc<[f64]>,
    pub wall_sets: Vec<Vec<WallSpec>>,
    /// Index into wall_sets for each entry of zs, including adaptive heights.
    pub layer_set: Arc<[usize]>,
    pub first_layer: FirstLayer,
    pub join: Join,
    pub comp: Bias,
    pub texture: Option<Bias>,
    pub top_one_wall: bool,
    pub thin_margin: f32,
    /// Arachne settings in wall_sets order; absent for classic walls.
    pub arachne: Option<Vec<ArachneSpec>>,
    /// The fill's classes (S1+S2): None plans walls and fill only, and the planner makes the classes.
    pub shells: Option<ShellSpec>,
}

/// Pure data from the wall generator's per-layer configuration. Lengths in mm.
#[derive(Clone, Debug)]
pub struct ArachneSpec {
    pub loops: usize,
    pub w_outer: f64,
    pub w_inner: f64,
    pub s_outer: f64,
    pub s_inner: f64,
    pub min_bead_width: f64,
    pub min_feature_size: f64,
    pub transition_length: f64,
    pub transition_filter_deviation: f64,
    pub transition_angle: f64,
    pub distribution_count: usize,
    pub precise: bool,
}

/// The planner's class rules, resolved from its settings (fab toolpath stages.rs: faces, skin,
/// shells, bridges, classes). Distances in mm, areas in mm².
#[derive(Clone, Debug)]
pub struct ShellSpec {
    pub top_layers: u32,
    pub bottom_layers: u32,
    /// A face clear of the fill (inside the wall band) grows into the shells by `face_grow`.
    pub vertical_shells: bool,
    /// Narrow solid strips over sparse infill become vertical shells, and the sparse under an
    /// internal bridge gives up a one-line anchor band.
    pub floating_shells: bool,
    /// Solid pieces get their narrow flag (IslandRegions::narrow).
    pub narrow_solid: bool,
    /// One per wall set, indexed by PlanSpec::layer_set like wall_sets (spacings follow the layer height).
    pub sets: Vec<ShellLevels>,
}

#[derive(Clone, Copy, Debug)]
pub struct ShellLevels {
    /// Faces and skins drop pieces of at most this area.
    pub speck: f32,
    pub face_grow: f32,
    /// The bottom (and external bridge) grows into its anchor by this, inside the fill.
    pub bottom_grow: f32,
    /// Shells open by this (a sliver up to twice this wide is no shell).
    pub shell_open: f32,
    /// Sparse pieces of at most this area become solid.
    pub min_sparse_area: f32,
    /// A solid piece with nothing this far from its edge over sparse is a vertical shell.
    pub vertical: f32,
    /// Internal bridges open by this, keep pieces over `bridge_area`, and grow by `bridge_grow`.
    pub bridge_open: f32,
    pub bridge_area: f32,
    pub bridge_grow: f32,
    /// The bridge above grows by this to find the sparse under it.
    pub under_grow: f32,
    /// The anchor band lies within this of the fill's edge, in pieces over `band_area`.
    pub band_core: f32,
    pub band_area: f32,
    /// A solid piece with nothing this far from its edge beside a band is a vertical shell.
    pub adjacent: f32,
    /// A solid piece with nothing this far from its edge is narrow.
    pub narrow: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct WallSpec {
    pub inset: f32,
    pub spacing: f32,
    pub width: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Bias {
    pub contour: f32,
    pub hole: f32,
}

#[derive(Clone, Debug, Default)]
pub struct LayerPlan {
    pub islands: Vec<IslandPlan>,
}

#[derive(Clone, Debug, Default)]
pub struct IslandPlan {
    /// Contour first, then holes; contours CCW, holes CW.
    pub outline: Vec<Loop>,
    pub walls: Option<WallPlan>,
    /// The fill by class, when the spec has shells and the island has walls.
    pub regions: Option<IslandRegions>,
}

/// One island's fill by class.
#[derive(Clone, Debug, Default)]
pub struct IslandRegions {
    /// Per class, its pieces, each contour directly before its holes. Disjoint (bottom wins over
    /// top); their union is the island's fill.
    pub classes: Vec<(Region, Vec<Loop>)>,
    /// One per Solid contour, in order: whether it is narrow (ShellLevels::narrow).
    pub narrow: Vec<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct WallPlan {
    pub lines: Vec<Vec<WallLine>>,
    pub fill: Vec<Loop>,
}

/// The planner's seven final classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Region {
    Top,
    /// The bottom surface or an external bridge, grown into its anchor (the planner picks the role by layer).
    Bottom,
    Solid,
    Sparse,
    InternalBridge,
    /// Narrow solid strips over sparse infill.
    VerticalShell,
    /// The one-line anchor band under an internal bridge.
    ShellBand,
}

#[derive(Clone, Debug)]
pub struct Loop {
    pub pts: Vec<[i32; 2]>,
    pub area_mm2: f32,
    pub bbox: [i32; 4],
    pub hole: bool,
}

#[derive(Clone, Debug)]
pub struct WallLine {
    pub closed: bool,
    pub odd: bool,
    pub pts: Vec<[i32; 2]>,
    pub widths: Vec<f32>,
}
