use core::ops::Range;
// TODO: Replace nalgebra Array2 with a custom Vec-based storage for heightfield data
#[cfg(feature = "alloc")]
use crate::utils::Array2;

use crate::bounding_volume::Aabb;
use crate::math::{Real, Vector};
use crate::shape::{FeatureId, Triangle, TrianglePseudoNormals};

#[cfg(not(feature = "std"))]
use crate::math::ComplexField;

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// The status of the cell of a heightfield.
pub struct HeightFieldCellStatus(u8);

bitflags::bitflags! {
    impl HeightFieldCellStatus: u8 {
        /// If this bit is set, the concerned heightfield cell is subdivided using a Z pattern.
        const ZIGZAG_SUBDIVISION = 0b00000001;
        /// If this bit is set, the leftmost triangle of the concerned heightfield cell is removed.
        const LEFT_TRIANGLE_REMOVED = 0b00000010;
        /// If this bit is set, the rightmost triangle of the concerned heightfield cell is removed.
        const RIGHT_TRIANGLE_REMOVED = 0b00000100;
        /// If this bit is set, both triangles of the concerned heightfield cell are removed.
        const CELL_REMOVED = Self::LEFT_TRIANGLE_REMOVED.bits() | Self::RIGHT_TRIANGLE_REMOVED.bits();
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// Flags controlling the behavior of some operations involving heightfields.
pub struct HeightFieldFlags(u8);

bitflags::bitflags! {
    impl HeightFieldFlags: u8 {
        /// If set, a special treatment will be applied to contact manifold calculation to eliminate
        /// or fix contacts normals that could lead to incorrect bumps in physics simulation (especially
        /// on flat surfaces).
        ///
        /// This is achieved by taking into account adjacent triangle normals when computing contact
        /// points for a given triangle.
        const FIX_INTERNAL_EDGES = 1 << 0;
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
// TODO: Archive isn’t implemented for VecStorage yet.
// #[cfg_attr(
//     feature = "rkyv",
//     derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize),
//     archive(check_bytes)
// )]
#[derive(Debug, Clone)]
#[repr(C)]
/// A 3D heightfield.
pub struct HeightField {
    heights: Array2<Real>,
    status: Array2<HeightFieldCellStatus>,

    scale: Vector,
    aabb: Aabb,
    num_triangles: usize,
    flags: HeightFieldFlags,
}

#[cfg(feature = "alloc")]
impl HeightField {
    /// Initializes a new heightfield with the given heights, scaling factor, and flags.
    ///
    /// The heights are specified in such a way that the row index `i` of the array advances
    /// grid coordinates along the `z` axis, and the column index `j` of the array advances
    /// grid coordinates along the `x` axis.
    pub fn new(heights_zx: Array2<Real>, scale: Vector) -> Self {
        Self::with_flags(heights_zx, scale, HeightFieldFlags::empty())
    }

    /// Initializes a new heightfield with the given heights and a scaling factor.
    pub fn with_flags(heights_zx: Array2<Real>, scale: Vector, flags: HeightFieldFlags) -> Self {
        assert!(
            heights_zx.nrows() > 1 && heights_zx.ncols() > 1,
            "A heightfield heights must have at least 2 rows and columns."
        );
        let max = heights_zx.max();
        let min = heights_zx.min();
        let hscale = scale * 0.5;
        let aabb = Aabb::new(
            Vector::new(-hscale.x, min * scale.y, -hscale.z),
            Vector::new(hscale.x, max * scale.y, hscale.z),
        );
        let num_triangles = (heights_zx.nrows() - 1) * (heights_zx.ncols() - 1) * 2;
        let status = Array2::repeat(
            heights_zx.nrows() - 1,
            heights_zx.ncols() - 1,
            HeightFieldCellStatus::default(),
        );

        HeightField {
            heights: heights_zx,
            scale,
            aabb,
            num_triangles,
            status,
            flags,
        }
    }
}

impl HeightField {
    /// The number of rows of this heightfield.
    pub fn nrows(&self) -> usize {
        self.heights.nrows() - 1
    }

    /// The number of columns of this heightfield.
    pub fn ncols(&self) -> usize {
        self.heights.ncols() - 1
    }

    fn triangle_id(&self, i: usize, j: usize, left: bool) -> u32 {
        let tid = j * (self.heights.nrows() - 1) + i;
        if left {
            tid as u32
        } else {
            (tid + self.num_triangles / 2) as u32
        }
    }

    fn split_triangle_id(&self, id: u32) -> (usize, usize, bool) {
        let left = id < self.num_triangles as u32 / 2;
        let tri_id = if left {
            id as usize
        } else {
            id as usize - self.num_triangles / 2
        };
        let j = tri_id / (self.heights.nrows() - 1);
        let i = tri_id - j * (self.heights.nrows() - 1);
        (i, j, left)
    }

    fn face_id(&self, i: usize, j: usize, left: bool, front: bool) -> u32 {
        let tid = self.triangle_id(i, j, left);
        if front {
            tid
        } else {
            tid + self.num_triangles as u32
        }
    }

    fn quantize_floor_unclamped(&self, val: Real, cell_size: Real) -> isize {
        ((val + 0.5) / cell_size).floor() as isize
    }

    fn quantize_ceil_unclamped(&self, val: Real, cell_size: Real) -> isize {
        ((val + 0.5) / cell_size).ceil() as isize
    }

    fn quantize_floor(&self, val: Real, cell_size: Real, num_cells: usize) -> usize {
        ((val + 0.5) / cell_size)
            .floor()
            .clamp(0.0, (num_cells - 1) as Real) as usize
    }

    fn quantize_ceil(&self, val: Real, cell_size: Real, num_cells: usize) -> usize {
        ((val + 0.5) / cell_size)
            .ceil()
            .clamp(0.0, num_cells as Real) as usize
    }

    /// The pair of index of the cell containing the vertical projection of the given point.
    pub fn closest_cell_at_point(&self, pt: Vector) -> (usize, usize) {
        let scaled_pt = pt / self.scale;
        let cell_width = self.unit_cell_width();
        let cell_height = self.unit_cell_height();
        let ncells_x = self.ncols();
        let ncells_z = self.nrows();

        let j = self.quantize_floor(scaled_pt.x, cell_width, ncells_x);
        let i = self.quantize_floor(scaled_pt.z, cell_height, ncells_z);
        (i, j)
    }

    /// The pair of index of the cell containing the vertical projection of the given point.
    pub fn cell_at_point(&self, pt: Vector) -> Option<(usize, usize)> {
        let scaled_pt = pt / self.scale;
        let cell_width = self.unit_cell_width();
        let cell_height = self.unit_cell_height();
        let ncells_x = self.ncols();
        let ncells_z = self.nrows();

        if scaled_pt.x < -0.5 || scaled_pt.x > 0.5 || scaled_pt.z < -0.5 || scaled_pt.z > 0.5 {
            // Outside of the heightfield bounds.
            None
        } else {
            let j = self.quantize_floor(scaled_pt.x, cell_width, ncells_x);
            let i = self.quantize_floor(scaled_pt.z, cell_height, ncells_z);
            Some((i, j))
        }
    }

    /// The pair of index of the cell containing the vertical projection of the given point.
    pub fn unclamped_cell_at_point(&self, pt: Vector) -> (isize, isize) {
        let scaled_pt = pt / self.scale;
        let cell_width = self.unit_cell_width();
        let cell_height = self.unit_cell_height();

        let j = self.quantize_floor_unclamped(scaled_pt.x, cell_width);
        let i = self.quantize_floor_unclamped(scaled_pt.z, cell_height);
        (i, j)
    }

    /// The smallest x coordinate of the `j`-th column of this heightfield.
    pub fn x_at(&self, j: usize) -> Real {
        (-0.5 + self.unit_cell_width() * (j as Real)) * self.scale.x
    }

    /// The smallest z coordinate of the start of the `i`-th row of this heightfield.
    pub fn z_at(&self, i: usize) -> Real {
        (-0.5 + self.unit_cell_height() * (i as Real)) * self.scale.z
    }

    /// The smallest x coordinate of the `j`-th column of this heightfield.
    pub fn signed_x_at(&self, j: isize) -> Real {
        (-0.5 + self.unit_cell_width() * (j as Real)) * self.scale.x
    }

    /// The smallest z coordinate of the start of the `i`-th row of this heightfield.
    pub fn signed_z_at(&self, i: isize) -> Real {
        (-0.5 + self.unit_cell_height() * (i as Real)) * self.scale.z
    }

    /// An iterator through all the triangles of this heightfield.
    pub fn triangles(&self) -> impl Iterator<Item = Triangle> + '_ {
        HeightFieldTriangles {
            heightfield: self,
            curr: (0, 0),
            tris: self.triangles_at(0, 0),
        }
    }

    /// An iterator through all the triangles around the given point, after vertical projection on the heightfield.
    pub fn triangles_around_point(&self, point: Vector) -> HeightFieldRadialTriangles<'_> {
        let center = self.closest_cell_at_point(point);
        HeightFieldRadialTriangles {
            heightfield: self,
            center,
            curr_radius: 0,
            curr_element: 0,
            tris: self.triangles_at(center.0, center.1),
        }
    }

    /// Gets the vertices of the triangle identified by `id`.
    pub fn triangle_at_id(&self, id: u32) -> Option<Triangle> {
        let (i, j, left) = self.split_triangle_id(id);
        if left {
            self.triangles_at(i, j).0
        } else {
            self.triangles_at(i, j).1
        }
    }

    /// Gets the vertex indices of the triangle identified by `id`.
    pub fn triangle_vids_at_id(&self, id: u32) -> Option<[u32; 3]> {
        let (i, j, left) = self.split_triangle_id(id);
        if left {
            self.triangles_vids_at(i, j).0
        } else {
            self.triangles_vids_at(i, j).1
        }
    }

    /// Gets the indices of the vertices of the (up to) two triangles for the cell (i, j).
    pub fn triangles_vids_at(&self, i: usize, j: usize) -> (Option<[u32; 3]>, Option<[u32; 3]>) {
        if i >= self.heights.nrows() - 1 || j >= self.heights.ncols() - 1 {
            return (None, None);
        }

        let status = self.status[(i, j)];

        if status.contains(
            HeightFieldCellStatus::LEFT_TRIANGLE_REMOVED
                | HeightFieldCellStatus::RIGHT_TRIANGLE_REMOVED,
        ) {
            return (None, None);
        }

        let p00 = (i + j * self.heights.nrows()) as u32;
        let p10 = ((i + 1) + j * self.heights.nrows()) as u32;
        let p01 = (i + (j + 1) * self.heights.nrows()) as u32;
        let p11 = ((i + 1) + (j + 1) * self.heights.nrows()) as u32;

        if status.contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
            let tri1 = if status.contains(HeightFieldCellStatus::LEFT_TRIANGLE_REMOVED) {
                None
            } else {
                Some([p00, p10, p11])
            };

            let tri2 = if status.contains(HeightFieldCellStatus::RIGHT_TRIANGLE_REMOVED) {
                None
            } else {
                Some([p00, p11, p01])
            };

            (tri1, tri2)
        } else {
            let tri1 = if status.contains(HeightFieldCellStatus::LEFT_TRIANGLE_REMOVED) {
                None
            } else {
                Some([p00, p10, p01])
            };

            let tri2 = if status.contains(HeightFieldCellStatus::RIGHT_TRIANGLE_REMOVED) {
                None
            } else {
                Some([p10, p11, p01])
            };

            (tri1, tri2)
        }
    }

    /// The two triangles at the cell (i, j) of this heightfield.
    ///
    /// Returns `None` fore triangles that have been removed because of their user-defined status
    /// flags (described by the `HeightFieldCellStatus` bitfield).
    pub fn triangles_at(&self, i: usize, j: usize) -> (Option<Triangle>, Option<Triangle>) {
        if i >= self.heights.nrows() - 1 || j >= self.heights.ncols() - 1 {
            return (None, None);
        }

        let status = self.status[(i, j)];

        if status.contains(
            HeightFieldCellStatus::LEFT_TRIANGLE_REMOVED
                | HeightFieldCellStatus::RIGHT_TRIANGLE_REMOVED,
        ) {
            return (None, None);
        }

        let cell_width = self.unit_cell_width();
        let cell_height = self.unit_cell_height();

        let z0 = -0.5 + cell_height * (i as Real);
        let z1 = -0.5 + cell_height * ((i + 1) as Real);

        let x0 = -0.5 + cell_width * (j as Real);
        let x1 = -0.5 + cell_width * ((j + 1) as Real);

        let y00 = self.heights[(i, j)];
        let y10 = self.heights[(i + 1, j)];
        let y01 = self.heights[(i, j + 1)];
        let y11 = self.heights[(i + 1, j + 1)];

        let mut p00 = Vector::new(x0, y00, z0);
        let mut p10 = Vector::new(x0, y10, z1);
        let mut p01 = Vector::new(x1, y01, z0);
        let mut p11 = Vector::new(x1, y11, z1);

        // Apply scales:
        p00 *= self.scale;
        p10 *= self.scale;
        p01 *= self.scale;
        p11 *= self.scale;

        if status.contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
            let tri1 = if status.contains(HeightFieldCellStatus::LEFT_TRIANGLE_REMOVED) {
                None
            } else {
                Some(Triangle::new(p00, p10, p11))
            };

            let tri2 = if status.contains(HeightFieldCellStatus::RIGHT_TRIANGLE_REMOVED) {
                None
            } else {
                Some(Triangle::new(p00, p11, p01))
            };

            (tri1, tri2)
        } else {
            let tri1 = if status.contains(HeightFieldCellStatus::LEFT_TRIANGLE_REMOVED) {
                None
            } else {
                Some(Triangle::new(p00, p10, p01))
            };

            let tri2 = if status.contains(HeightFieldCellStatus::RIGHT_TRIANGLE_REMOVED) {
                None
            } else {
                Some(Triangle::new(p10, p11, p01))
            };

            (tri1, tri2)
        }
    }

    /// Computes the pseudo-normals of the triangle identified by the given id.
    ///
    /// Returns `None` if the heightfield’s [`HeightFieldFlags::FIX_INTERNAL_EDGES`] isn’t set, or
    /// if the triangle doesn’t exist due to it being removed by its status flag
    /// (`HeightFieldCellStatus::LEFT_TRIANGLE_REMOVED` or
    /// `HeightFieldCellStatus::RIGHT_TRIANGLE_REMOVED`).
    pub fn triangle_normal_constraints(&self, id: u32) -> Option<TrianglePseudoNormals> {
        if self.flags.contains(HeightFieldFlags::FIX_INTERNAL_EDGES) {
            let (i, j, left) = self.split_triangle_id(id);
            let status = self.status[(i, j)];

            let (tri_left, tri_right) = self.triangles_at(i, j);
            let tri_normal = if left {
                tri_left?.normal()?
            } else {
                tri_right?.normal()?
            };

            // TODO: we only compute bivectors where v is a specific direction
            //       (+/-X, +/-Z, or a combination of both). So this bivector
            //       calculation could be simplified/optimized quite a bit.
            // Computes the pseudo-normal of an edge where the adjacent triangle is missing.
            let bivector = |v: Vector| tri_normal.cross(v).cross(tri_normal).normalize();
            // Pseudo-normal computed from an adjacent triangle’s normal and the current triangle’s normal.
            let adj_pseudo_normal =
                |adj: Option<Triangle>| adj.map(|adj| adj.normal().unwrap_or(tri_normal));

            let diag_dir = if status.contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
                Vector::new(-1.0, 0.0, 1.0)
            } else {
                Vector::new(1.0, 0.0, 1.0)
            };

            let (left_pseudo_normal, right_pseudo_normal) = if left {
                let adj_left = adj_pseudo_normal(self.triangles_at(i.overflowing_sub(1).0, j).1)
                    .unwrap_or_else(|| bivector(-Vector::Z));
                let adj_right = adj_pseudo_normal(tri_right).unwrap_or_else(|| bivector(diag_dir));
                (adj_left, adj_right)
            } else {
                let adj_left = adj_pseudo_normal(tri_left).unwrap_or_else(|| bivector(-diag_dir));
                let adj_right = adj_pseudo_normal(self.triangles_at(i + 1, j).0)
                    .unwrap_or_else(|| bivector(Vector::Z));
                (adj_left, adj_right)
            };

            // The third neighbor depends on the combination of zigzag scheme
            // and right/left position.
            let top_or_bottom_pseudo_normal = if left
                != status.contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION)
            {
                // The neighbor is below.
                let ((bot_left, bot_right), bot_status) = if j > 0 {
                    (self.triangles_at(i, j - 1), self.status[(i, j - 1)])
                } else {
                    ((None, None), HeightFieldCellStatus::empty())
                };

                let bot_tri = if bot_status.contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
                    bot_left
                } else {
                    bot_right
                };

                adj_pseudo_normal(bot_tri).unwrap_or_else(|| bivector(-Vector::X))
            } else {
                // The neighbor is above.
                let ((top_left, top_right), top_status) = if j < self.heights.ncols() - 2 {
                    (self.triangles_at(i, j + 1), self.status[(i, j + 1)])
                } else {
                    ((None, None), HeightFieldCellStatus::empty())
                };

                let top_tri = if top_status.contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
                    top_right
                } else {
                    top_left
                };

                adj_pseudo_normal(top_tri).unwrap_or_else(|| bivector(Vector::X))
            };

            // NOTE: the normalization can only succeed due to the heightfield's definition.
            let pseudo_normal1 = (tri_normal + left_pseudo_normal).normalize();
            let pseudo_normal2 = (tri_normal + right_pseudo_normal).normalize();
            let pseudo_normal3 = (tri_normal + top_or_bottom_pseudo_normal).normalize();

            Some(TrianglePseudoNormals {
                face: tri_normal, // No need to re-normalize.
                // TODO: the normals are given in no particular order. So they are **not**
                //       guaranteed to be provided in the same order as the triangle’s edge.
                edges: [pseudo_normal1, pseudo_normal2, pseudo_normal3],
            })
        } else {
            None
        }
    }

    /// The number of cells of this heightfield along each dimension.
    pub fn num_cells_ij(&self) -> (usize, usize) {
        (self.nrows(), self.ncols())
    }

    /// The status of the `(i, j)`-th cell.
    pub fn cell_status(&self, i: usize, j: usize) -> HeightFieldCellStatus {
        self.status[(i, j)]
    }

    /// Set the status of the `(i, j)`-th cell.
    pub fn set_cell_status(&mut self, i: usize, j: usize, status: HeightFieldCellStatus) {
        self.status[(i, j)] = status;
    }

    /// The statuses of all the cells of this heightfield.
    pub fn cells_statuses(&self) -> &Array2<HeightFieldCellStatus> {
        &self.status
    }

    /// The mutable statuses of all the cells of this heightfield.
    pub fn cells_statuses_mut(&mut self) -> &mut Array2<HeightFieldCellStatus> {
        &mut self.status
    }

    /// The heightfield’s flags controlling internal-edges handling.
    pub fn flags(&self) -> HeightFieldFlags {
        self.flags
    }

    /// Sets the heightfield’s flags controlling internal-edges handling.
    pub fn set_flags(&mut self, flags: HeightFieldFlags) {
        self.flags = flags;
    }

    /// The heights of this heightfield.
    pub fn heights(&self) -> &Array2<Real> {
        &self.heights
    }

    /// The scale factor applied to this heightfield.
    pub fn scale(&self) -> Vector {
        self.scale
    }

    /// Sets the scale factor applied to this heightfield.
    pub fn set_scale(&mut self, new_scale: Vector) {
        let ratio = new_scale / self.scale;
        self.aabb.mins *= ratio;
        self.aabb.maxs *= ratio;
        self.scale = new_scale;
    }

    /// Returns a scaled version of this heightfield.
    pub fn scaled(mut self, scale: Vector) -> Self {
        self.set_scale(self.scale * scale);
        self
    }

    /// The width (extent along its local `x` axis) of each cell of this heightmap, including the scale factor.
    pub fn cell_width(&self) -> Real {
        self.unit_cell_width() * self.scale.x
    }

    /// The height (extent along its local `z` axis) of each cell of this heightmap, including the scale factor.
    pub fn cell_height(&self) -> Real {
        self.unit_cell_height() * self.scale.z
    }

    /// The width (extent along its local `x` axis) of each cell of this heightmap, excluding the scale factor.
    pub fn unit_cell_width(&self) -> Real {
        1.0 / (self.heights.ncols() as Real - 1.0)
    }

    /// The height (extent along its local `z` axis) of each cell of this heightmap, excluding the scale factor.
    pub fn unit_cell_height(&self) -> Real {
        1.0 / (self.heights.nrows() as Real - 1.0)
    }

    /// The [`Aabb`] of this heightmap.
    pub fn root_aabb(&self) -> &Aabb {
        &self.aabb
    }

    /// Converts the FeatureID of the left or right triangle at the cell `(i, j)` into a FeatureId
    /// of the whole heightfield.
    pub fn convert_triangle_feature_id(
        &self,
        i: usize,
        j: usize,
        left: bool,
        fid: FeatureId,
    ) -> FeatureId {
        match fid {
            FeatureId::Vertex(ivertex) => {
                let nrows = self.heights.nrows();
                let ij = i + j * nrows;

                if self.status[(i, j)].contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
                    if left {
                        FeatureId::Vertex([ij, ij + 1, ij + 1 + nrows][ivertex as usize] as u32)
                    } else {
                        FeatureId::Vertex([ij, ij + 1 + nrows, ij + nrows][ivertex as usize] as u32)
                    }
                } else if left {
                    FeatureId::Vertex([ij, ij + 1, ij + nrows][ivertex as usize] as u32)
                } else {
                    FeatureId::Vertex([ij + 1, ij + 1 + nrows, ij + nrows][ivertex as usize] as u32)
                }
            }
            FeatureId::Edge(iedge) => {
                let (nrows, ncols) = (self.heights.nrows(), self.heights.ncols());
                let vshift = 0; // First vertical line index.
                let hshift = (nrows - 1) * ncols; // First horizontal line index.
                let dshift = hshift + nrows * (ncols - 1); // First diagonal line index.
                let idiag = dshift + i + j * (nrows - 1);
                let itop = hshift + i + j * nrows;
                let ibottom = itop + 1;
                let ileft = vshift + i + j * (nrows - 1);
                let iright = ileft + nrows - 1;

                if self.status[(i, j)].contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
                    if left {
                        // Triangle:
                        //
                        // |\
                        // |_\
                        //
                        FeatureId::Edge([ileft, ibottom, idiag][iedge as usize] as u32)
                    } else {
                        // Triangle:
                        // ___
                        // \ |
                        //  \|
                        //
                        FeatureId::Edge([idiag, iright, itop][iedge as usize] as u32)
                    }
                } else if left {
                    // Triangle:
                    // ___
                    // | /
                    // |/
                    //
                    FeatureId::Edge([ileft, idiag, itop][iedge as usize] as u32)
                } else {
                    // Triangle:
                    //
                    //  /|
                    // /_|
                    //
                    FeatureId::Edge([ibottom, iright, idiag][iedge as usize] as u32)
                }
            }
            FeatureId::Face(iface) => {
                if iface == 0 {
                    FeatureId::Face(self.face_id(i, j, left, true))
                } else {
                    FeatureId::Face(self.face_id(i, j, left, false))
                }
            }
            FeatureId::Unknown => FeatureId::Unknown,
        }
    }

    /// The range of segment ids that may intersect the given local Aabb.
    pub fn unclamped_elements_range_in_local_aabb(
        &self,
        aabb: &Aabb,
    ) -> (Range<isize>, Range<isize>) {
        let ref_mins = aabb.mins / self.scale;
        let ref_maxs = aabb.maxs / self.scale;
        let cell_width = self.unit_cell_width();
        let cell_height = self.unit_cell_height();

        let min_x = self.quantize_floor_unclamped(ref_mins.x, cell_width);
        let min_z = self.quantize_floor_unclamped(ref_mins.z, cell_height);

        let max_x = self.quantize_ceil_unclamped(ref_maxs.x, cell_width);
        let max_z = self.quantize_ceil_unclamped(ref_maxs.z, cell_height);
        (min_z..max_z, min_x..max_x)
    }

    /// Applies the function `f` to all the triangles of this heightfield intersecting the given Aabb.
    pub fn map_elements_in_local_aabb(&self, aabb: &Aabb, f: &mut impl FnMut(u32, &Triangle)) {
        let ncells_x = self.ncols();
        let ncells_z = self.nrows();

        let ref_mins = aabb.mins / self.scale;
        let ref_maxs = aabb.maxs / self.scale;
        let cell_width = self.unit_cell_width();
        let cell_height = self.unit_cell_height();

        if ref_maxs.x <= -0.5 || ref_maxs.z <= -0.5 || ref_mins.x >= 0.5 || ref_mins.z >= 0.5 {
            // Outside of the heightfield bounds.
            return;
        }

        let min_x = self.quantize_floor(ref_mins.x, cell_width, ncells_x);
        let min_z = self.quantize_floor(ref_mins.z, cell_height, ncells_z);

        let max_x = self.quantize_ceil(ref_maxs.x, cell_width, ncells_x);
        let max_z = self.quantize_ceil(ref_maxs.z, cell_height, ncells_z);

        // TODO: find a way to avoid recomputing the same vertices
        // multiple times.
        for j in min_x..max_x {
            for i in min_z..max_z {
                let status = self.status[(i, j)];

                if status.contains(HeightFieldCellStatus::CELL_REMOVED) {
                    continue;
                }

                let z0 = -0.5 + cell_height * (i as Real);
                let z1 = z0 + cell_height;

                let x0 = -0.5 + cell_width * (j as Real);
                let x1 = x0 + cell_width;

                let y00 = self.heights[(i, j)];
                let y10 = self.heights[(i + 1, j)];
                let y01 = self.heights[(i, j + 1)];
                let y11 = self.heights[(i + 1, j + 1)];

                if (y00 > ref_maxs.y && y10 > ref_maxs.y && y01 > ref_maxs.y && y11 > ref_maxs.y)
                    || (y00 < ref_mins.y
                        && y10 < ref_mins.y
                        && y01 < ref_mins.y
                        && y11 < ref_mins.y)
                {
                    continue;
                }

                let mut p00 = Vector::new(x0, y00, z0);
                let mut p10 = Vector::new(x0, y10, z1);
                let mut p01 = Vector::new(x1, y01, z0);
                let mut p11 = Vector::new(x1, y11, z1);

                // Apply scales:
                p00 *= self.scale;
                p10 *= self.scale;
                p01 *= self.scale;
                p11 *= self.scale;

                // Build the two triangles, contact processors and call f.
                if !status.contains(HeightFieldCellStatus::LEFT_TRIANGLE_REMOVED) {
                    let tri1 = if status.contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
                        Triangle::new(p00, p10, p11)
                    } else {
                        Triangle::new(p00, p10, p01)
                    };

                    let tid = self.triangle_id(i, j, true);
                    f(tid, &tri1);
                }

                if !status.contains(HeightFieldCellStatus::RIGHT_TRIANGLE_REMOVED) {
                    let tri2 = if status.contains(HeightFieldCellStatus::ZIGZAG_SUBDIVISION) {
                        Triangle::new(p00, p11, p01)
                    } else {
                        Triangle::new(p10, p11, p01)
                    };
                    let tid = self.triangle_id(i, j, false);
                    f(tid, &tri2);
                }
            }
        }
    }
}

struct HeightFieldTriangles<'a> {
    heightfield: &'a HeightField,
    curr: (usize, usize),
    tris: (Option<Triangle>, Option<Triangle>),
}

impl Iterator for HeightFieldTriangles<'_> {
    type Item = Triangle;

    fn next(&mut self) -> Option<Triangle> {
        loop {
            if let Some(tri1) = self.tris.0.take() {
                return Some(tri1);
            } else if let Some(tri2) = self.tris.1.take() {
                return Some(tri2);
            } else {
                self.curr.0 += 1;

                if self.curr.0 >= self.heightfield.nrows() {
                    if self.curr.1 >= self.heightfield.ncols() - 1 {
                        return None;
                    }

                    self.curr.0 = 0;
                    self.curr.1 += 1;
                }

                // tri1 and tri2 are None
                self.tris = self.heightfield.triangles_at(self.curr.0, self.curr.1);
            }
        }
    }
}

/// An iterator through all the triangles around the given point, after vertical projection on the heightfield.
pub struct HeightFieldRadialTriangles<'a> {
    heightfield: &'a HeightField,
    center: (usize, usize),
    curr_radius: usize,
    curr_element: usize,
    tris: (Option<Triangle>, Option<Triangle>),
}

impl HeightFieldRadialTriangles<'_> {
    /// Returns the next triangle in this iterator.
    ///
    /// Returns `None` no triangle closest than `max_dist` remain
    /// to be yielded. The `max_dist` can be modified at each iteration
    /// as long as the new value is smaller or equal to the previous value.
    pub fn next(&mut self, max_dist: Real) -> Option<Triangle> {
        let max_rad = if max_dist == Real::MAX {
            usize::MAX
        } else {
            (max_dist / self.heightfield.cell_width())
                .ceil()
                .max((max_dist / self.heightfield.cell_height()).ceil()) as usize
        };

        loop {
            if let Some(tri1) = self.tris.0.take() {
                return Some(tri1);
            } else if let Some(tri2) = self.tris.1.take() {
                return Some(tri2);
            } else {
                let mut curr_cell = (0, 0);

                loop {
                    self.curr_element += 1;

                    if self.curr_element >= 8 * self.curr_radius {
                        if self.curr_radius >= max_rad {
                            return None;
                        }

                        self.curr_element = 0;
                        self.curr_radius += 1;
                    }

                    let side_max_index = self.curr_radius + self.curr_radius;
                    let curr_index_in_side = self.curr_element % side_max_index;
                    let curr_side = self.curr_element / side_max_index;

                    let mins = (
                        self.center.0 as isize - self.curr_radius as isize,
                        self.center.1 as isize - self.curr_radius as isize,
                    );
                    let maxs = (
                        self.center.0 as isize + self.curr_radius as isize,
                        self.center.1 as isize + self.curr_radius as isize,
                    );

                    if curr_cell.0 < 0
                        && curr_cell.1 < 0
                        && (curr_cell.0 as usize) >= self.heightfield.nrows()
                        && (curr_cell.1 as usize) >= self.heightfield.ncols()
                    {
                        // We already visited all the triangles of the heightfield.
                        return None;
                    }

                    let side_origins = [
                        (mins.0, mins.1),
                        (maxs.0, mins.1),
                        (maxs.0, maxs.1),
                        (mins.0, maxs.1),
                    ];
                    let side_directions = [(1, 0), (0, 1), (-1, 0), (0, -1)];

                    curr_cell = (
                        side_origins[curr_side].0
                            + side_directions[curr_side].0 * (curr_index_in_side as isize),
                        side_origins[curr_side].1
                            + side_directions[curr_side].1 * (curr_index_in_side as isize),
                    );

                    if curr_cell.0 >= 0
                        && curr_cell.1 >= 0
                        && (curr_cell.0 as usize) < self.heightfield.nrows()
                        && (curr_cell.1 as usize) < self.heightfield.ncols()
                    {
                        break;
                    }
                }

                self.tris = self
                    .heightfield
                    .triangles_at(curr_cell.0 as usize, curr_cell.1 as usize);
            }
        }
    }
}
