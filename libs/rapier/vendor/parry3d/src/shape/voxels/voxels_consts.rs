use super::VoxelType;

#[cfg(test)]
use {super::OctantPattern, crate::bounding_volume::Aabb};

// Index to the item of FACES_TO_VOXEL_TYPES which identifies interior voxels.
#[cfg(feature = "dim2")]
pub(super) const INTERIOR_FACE_MASK: u8 = 0b0000_1111;
#[cfg(feature = "dim3")]
pub(super) const INTERIOR_FACE_MASK: u8 = 0b0011_1111;
// Index to the item of FACES_TO_VOXEL_TYPES which identifies empty voxels.

#[cfg(feature = "dim2")]
pub(super) const EMPTY_FACE_MASK: u8 = 0b0001_0000;
#[cfg(feature = "dim3")]
pub(super) const EMPTY_FACE_MASK: u8 = 0b0100_0000;

/// The voxel type deduced from adjacency information.
///
/// See the documentation of [`VoxelType`] for additional information on what each enum variant
/// means.
///
/// In 3D there are 6 neighbor faces => 64 cases + 1 empty case.
#[cfg(feature = "dim3")]
pub(super) const FACES_TO_VOXEL_TYPES: [VoxelType; 65] = [
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Face,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Face,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Face,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Face,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Face,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Edge,
    VoxelType::Face,
    VoxelType::Face,
    VoxelType::Face,
    VoxelType::Face,
    VoxelType::Interior,
    VoxelType::Empty,
];

/// Indicates the convex features of each voxel that can lead to collisions.
///
/// The interpretation of each bit differs depending on the corresponding voxel type in
/// `FACES_TO_VOXEL_TYPES`:
/// - For `VoxelType::Vertex`: the i-th bit set to `1` indicates that the i-th AABB vertex is convex
///   and might lead to collisions.
/// - For `VoxelType::Edge`: the i-th bit set to `1` indicates that the i-th edge from `Aabb::EDGES_VERTEX_IDS`
///   is convex and might lead to collisions.
/// - For `VoxelType::Face`: the i-th bit set to `1` indicates that the i-th face from `Aabb::FACES_VERTEX_IDS`
///   is exposed and might lead to collisions.
#[cfg(feature = "dim3")]
pub(super) const FACES_TO_FEATURE_MASKS: [u16; 65] = [
    0b11111111,
    0b10011001,
    0b1100110,
    0b1010101,
    0b110011,
    0b10001,
    0b100010,
    0b10001,
    0b11001100,
    0b10001000,
    0b1000100,
    0b1000100,
    0b10101010,
    0b10001000,
    0b100010,
    0b110000,
    0b1111,
    0b1001,
    0b110,
    0b101,
    0b11,
    0b1,
    0b10,
    0b1,
    0b1100,
    0b1000,
    0b100,
    0b100,
    0b1010,
    0b1000,
    0b10,
    0b100000,
    0b11110000,
    0b10010000,
    0b1100000,
    0b1010000,
    0b110000,
    0b10000,
    0b100000,
    0b10000,
    0b11000000,
    0b10000000,
    0b1000000,
    0b1000000,
    0b10100000,
    0b10000000,
    0b100000,
    0b10000,
    0b111100000000,
    0b100100000000,
    0b11000000000,
    0b1100,
    0b1100000000,
    0b100000000,
    0b1000000000,
    0b1000,
    0b110000000000,
    0b100000000000,
    0b10000000000,
    0b100,
    0b11,
    0b10,
    0b1,
    0b1111111111111111,
    0,
];

/// Each octant is assigned three contiguous bits.
#[cfg(feature = "dim3")]
pub(super) const FACES_TO_OCTANT_MASKS: [u32; 65] = [
    0b1001001001001001001001,
    0b1010010001001010010001,
    0b10001001010010001001010,
    0b10010010010010010010010,
    0b11011001001011011001001,
    0b11111010001011111010001,
    0b111011001010111011001010,
    0b111111010010111111010010,
    0b1001011011001001011011,
    0b1010111011001010111011,
    0b10001011111010001011111,
    0b10010111111010010111111,
    0b11011011011011011011011,
    0b11111111011011111111011,
    0b111011011111111011011111,
    0b111111111111111111111111,
    0b100100100100001001001001,
    0b100110110100001010010001,
    0b110100100110010001001010,
    0b110110110110010010010010,
    0b101101100100011011001001,
    0b101000110100011111010001,
    0b101100110111011001010,
    0b110110111111010010,
    0b100100101101001001011011,
    0b100110000101001010111011,
    0b110100101000010001011111,
    0b110110000000010010111111,
    0b101101101101011011011011,
    0b101000000101011111111011,
    0b101101000111011011111,
    0b111111111111,
    0b1001001001100100100100,
    0b1010010001100110110100,
    0b10001001010110100100110,
    0b10010010010110110110110,
    0b11011001001101101100100,
    0b11111010001101000110100,
    0b111011001010000101100110,
    0b111111010010000000110110,
    0b1001011011100100101101,
    0b1010111011100110000101,
    0b10001011111110100101000,
    0b10010111111110110000000,
    0b11011011011101101101101,
    0b11111111011101000000101,
    0b111011011111000101101000,
    0b111111111111000000000000,
    0b100100100100100100100100,
    0b100110110100100110110100,
    0b110100100110110100100110,
    0b110110110110110110110110,
    0b101101100100101101100100,
    0b101000110100101000110100,
    0b101100110000101100110,
    0b110110000000110110,
    0b100100101101100100101101,
    0b100110000101100110000101,
    0b110100101000110100101000,
    0b110110000000110110000000,
    0b101101101101101101101101,
    0b101000000101101000000101,
    0b101101000000101101000,
    0b0,
    0,
];

#[cfg(feature = "dim2")]
pub(super) const FACES_TO_VOXEL_TYPES: [VoxelType; 17] = [
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Face,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Face,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Vertex,
    VoxelType::Face,
    VoxelType::Face,
    VoxelType::Face,
    VoxelType::Face,
    VoxelType::Interior,
    VoxelType::Empty,
];

#[cfg(feature = "dim2")]
pub(super) const FACES_TO_FEATURE_MASKS: [u16; 17] = [
    0b1111,
    0b1001,
    0b110,
    0b1100,
    0b11,
    0b1,
    0b10,
    0b1000,
    0b1100,
    0b1000,
    0b100,
    0b100,
    0b11,
    0b10,
    0b1,
    0b1111111111111111,
    0,
];

// NOTE: in 2D we are also using 3 bits per octant even though we technically only need two.
//       This keeps some collision-detection easier by avoiding some special-casing.
#[cfg(feature = "dim2")]
pub(super) const FACES_TO_OCTANT_MASKS: [u32; 17] = [
    0b1001001001,
    0b1011011001,
    0b11001001011,
    0b11011011011,
    0b10010001001,
    0b10000011001,
    0b10001011,
    0b11011,
    0b1001010010,
    0b1011000010,
    0b11001010000,
    0b11011000000,
    0b10010010010,
    0b10000000010,
    0b10010000,
    0b0,
    0,
];

// NOTE: this code is used to generate the constant tables
// FACES_TO_VOXEL_TYPES, FACES_TO_FEATURE_MASKS, FACES_TO_OCTANT_MASKS.
#[allow(dead_code)]
#[cfg(feature = "dim2")]
#[cfg(test)]
fn gen_const_tables() {
    // The `j-th` bit of `faces_adj_to_vtx[i]` is set to 1, if the j-th face of the AABB (based on
    // the face order depicted in `AABB::FACES_VERTEX_IDS`) is adjacent to the `i` vertex of the AABB
    // (vertices are indexed as per the diagram depicted in the `FACES_VERTEX_IDS` doc.
    // Each entry of this will always have exactly 3 bits set.
    let mut faces_adj_to_vtx = [0usize; 4];

    for fid in 0..4 {
        let vids = Aabb::FACES_VERTEX_IDS[fid];
        let key = 1 << fid;
        faces_adj_to_vtx[vids.0] |= key;
        faces_adj_to_vtx[vids.1] |= key;
    }

    /*
     * FACES_TO_VOXEL_TYPES
     */
    std::println!("const FACES_TO_VOXEL_TYPES: [VoxelType; 17] = [");
    'outer: for i in 0usize..16 {
        // If any vertex of the voxel has three faces with no adjacent voxels,
        // then the voxel type is Vertex.
        for adjs in faces_adj_to_vtx.iter() {
            if (*adjs & i) == 0 {
                std::println!("VoxelType::Vertex,");
                continue 'outer;
            }
        }

        // If one face doesn’t have any adjacent voxel,
        // then the voxel type is Face.
        for fid in 0..4 {
            if ((1 << fid) & i) == 0 {
                std::println!("VoxelType::Face,");
                continue 'outer;
            }
        }
    }

    // Add final entries for special values.
    std::println!("VoxelType::Interior,");
    std::println!("VoxelType::Empty,");
    std::println!("];");

    /*
     * FACES_TO_FEATURE_MASKS
     */
    std::println!("const FACES_TO_FEATURE_MASKS: [u16; 17] = [");
    for i in 0usize..16 {
        // Each bit set indicates a convex vertex that can lead to collisions.
        // The result will be nonzero only for `VoxelType::Vertex` voxels.
        let mut vtx_key = 0;
        for (vid, adjs) in faces_adj_to_vtx.iter().enumerate() {
            if (*adjs & i) == 0 {
                vtx_key |= 1 << vid;
            }
        }

        if vtx_key != 0 {
            std::println!("0b{:b},", vtx_key as u16);
            continue;
        }

        // Each bit set indicates an exposed face that can lead to collisions.
        // The result will be nonzero only for `VoxelType::Face` voxels.
        let mut face_key = 0;
        for fid in 0..4 {
            if ((1 << fid) & i) == 0 {
                face_key |= 1 << fid;
            }
        }

        if face_key != 0 {
            std::println!("0b{:b},", face_key as u16);
            continue;
        }
    }

    std::println!("0b{:b},", u16::MAX);
    std::println!("0,");
    std::println!("];");

    /*
     * Faces to octant masks.
     */
    std::println!("const FACES_TO_OCTANT_MASKS: [u32; 17] = [");
    for i in 0usize..16 {
        // First test if we have vertices.
        let mut octant_mask = 0;
        let mut set_mask = |mask, octant| {
            // NOTE: we don’t overwrite any mask already set for the octant.
            if (octant_mask >> (octant * 3)) & 0b0111 == 0 {
                octant_mask |= mask << (octant * 3);
            }
        };

        for (vid, adjs) in faces_adj_to_vtx.iter().enumerate() {
            if (*adjs & i) == 0 {
                set_mask(1, vid);
            }
        }

        // This is the index of the normal of the faces given by
        // Aabb::FACES_VERTEX_IDS.
        const FX: u32 = OctantPattern::FACE_X;
        const FY: u32 = OctantPattern::FACE_Y;
        const FACE_NORMALS: [u32; 4] = [FX, FX, FY, FY];

        #[allow(clippy::needless_range_loop)]
        for fid in 0..4 {
            if ((1 << fid) & i) == 0 {
                let vid = Aabb::FACES_VERTEX_IDS[fid];
                let mask = FACE_NORMALS[fid];

                set_mask(mask, vid.0);
                set_mask(mask, vid.1);
            }
        }
        std::println!("0b{:b},", octant_mask);
    }
    std::println!("0,");
    std::println!("];");
}

// NOTE: this code is used to generate the constant tables
// FACES_TO_VOXEL_TYPES, FACES_TO_FEATURE_MASKS, FACES_TO_OCTANT_MASKS.
#[allow(dead_code)]
#[cfg(feature = "dim3")]
#[cfg(test)]
fn gen_const_tables() {
    // The `j-th` bit of `faces_adj_to_vtx[i]` is set to 1, if the j-th face of the AABB (based on
    // the face order depicted in `AABB::FACES_VERTEX_IDS`) is adjacent to the `i` vertex of the AABB
    // (vertices are indexed as per the diagram depicted in the `FACES_VERTEX_IDS` doc.
    // Each entry of this will always have exactly 3 bits set.
    let mut faces_adj_to_vtx = [0usize; 8];

    // The `j-th` bit of `faces_adj_to_vtx[i]` is set to 1, if the j-th edge of the AABB (based on
    // the edge order depicted in `AABB::EDGES_VERTEX_IDS`) is adjacent to the `i` vertex of the AABB
    // (vertices are indexed as per the diagram depicted in the `FACES_VERTEX_IDS` doc.
    // Each entry of this will always have exactly 2 bits set.
    let mut faces_adj_to_edge = [0usize; 12];

    for fid in 0..6 {
        let vids = Aabb::FACES_VERTEX_IDS[fid];
        let key = 1 << fid;
        faces_adj_to_vtx[vids.0] |= key;
        faces_adj_to_vtx[vids.1] |= key;
        faces_adj_to_vtx[vids.2] |= key;
        faces_adj_to_vtx[vids.3] |= key;
    }

    #[allow(clippy::needless_range_loop)]
    for eid in 0..12 {
        let evids = Aabb::EDGES_VERTEX_IDS[eid];
        for fid in 0..6 {
            let fvids = Aabb::FACES_VERTEX_IDS[fid];
            if (fvids.0 == evids.0
                || fvids.1 == evids.0
                || fvids.2 == evids.0
                || fvids.3 == evids.0)
                && (fvids.0 == evids.1
                    || fvids.1 == evids.1
                    || fvids.2 == evids.1
                    || fvids.3 == evids.1)
            {
                let key = 1 << fid;
                faces_adj_to_edge[eid] |= key;
            }
        }
    }

    /*
     * FACES_TO_VOXEL_TYPES
     */
    std::println!("const FACES_TO_VOXEL_TYPES: [VoxelType; 65] = [");
    'outer: for i in 0usize..64 {
        // If any vertex of the voxel has three faces with no adjacent voxels,
        // then the voxel type is Vertex.
        for adjs in faces_adj_to_vtx.iter() {
            if (*adjs & i) == 0 {
                std::println!("VoxelType::Vertex,");
                continue 'outer;
            }
        }

        // If any vertex of the voxel has three faces with no adjacent voxels,
        // then the voxel type is Edge.
        for adjs in faces_adj_to_edge.iter() {
            if (*adjs & i) == 0 {
                std::println!("VoxelType::Edge,");
                continue 'outer;
            }
        }

        // If one face doesn’t have any adjacent voxel,
        // then the voxel type is Face.
        for fid in 0..6 {
            if ((1 << fid) & i) == 0 {
                std::println!("VoxelType::Face,");
                continue 'outer;
            }
        }
    }

    // Add final entries for special values.
    std::println!("VoxelType::Interior,");
    std::println!("VoxelType::Empty,");
    std::println!("];");

    /*
     * FACES_TO_FEATURE_MASKS
     */
    std::println!("const FACES_TO_FEATURE_MASKS: [u16; 65] = [");
    for i in 0usize..64 {
        // Each bit set indicates a convex vertex that can lead to collisions.
        // The result will be nonzero only for `VoxelType::Vertex` voxels.
        let mut vtx_key = 0;
        for (vid, adjs) in faces_adj_to_vtx.iter().enumerate() {
            if (*adjs & i) == 0 {
                vtx_key |= 1 << vid;
            }
        }

        if vtx_key != 0 {
            std::println!("0b{:b},", vtx_key as u16);
            continue;
        }

        // Each bit set indicates a convex edge that can lead to collisions.
        // The result will be nonzero only for `VoxelType::Edge` voxels.
        let mut edge_key = 0;
        for (eid, adjs) in faces_adj_to_edge.iter().enumerate() {
            if (*adjs & i) == 0 {
                edge_key |= 1 << eid;
            }
        }

        if edge_key != 0 {
            std::println!("0b{:b},", edge_key as u16);
            continue;
        }

        // Each bit set indicates an exposed face that can lead to collisions.
        // The result will be nonzero only for `VoxelType::Face` voxels.
        let mut face_key = 0;
        for fid in 0..6 {
            if ((1 << fid) & i) == 0 {
                face_key |= 1 << fid;
            }
        }

        if face_key != 0 {
            std::println!("0b{:b},", face_key as u16);
            continue;
        }
    }

    std::println!("0b{:b},", u16::MAX);
    std::println!("0,");
    std::println!("];");

    /*
     * Faces to octant masks.
     */
    std::println!("const FACES_TO_OCTANT_MASKS: [u32; 65] = [");
    for i in 0usize..64 {
        // First test if we have vertices.
        let mut octant_mask = 0;
        let mut set_mask = |mask, octant| {
            // NOTE: we don’t overwrite any mask already set for the octant.
            if (octant_mask >> (octant * 3)) & 0b0111 == 0 {
                octant_mask |= mask << (octant * 3);
            }
        };

        for (vid, adjs) in faces_adj_to_vtx.iter().enumerate() {
            if (*adjs & i) == 0 {
                set_mask(1, vid);
            }
        }

        // This is the index of the axis porting the edges given by
        // Aabb::EDGES_VERTEX_IDS.
        const EX: u32 = OctantPattern::EDGE_X;
        const EY: u32 = OctantPattern::EDGE_Y;
        const EZ: u32 = OctantPattern::EDGE_Z;
        const EDGE_AXIS: [u32; 12] = [EX, EY, EX, EY, EX, EY, EX, EY, EZ, EZ, EZ, EZ];
        for (eid, adjs) in faces_adj_to_edge.iter().enumerate() {
            if (*adjs & i) == 0 {
                let vid = Aabb::EDGES_VERTEX_IDS[eid];
                let mask = EDGE_AXIS[eid];

                set_mask(mask, vid.0);
                set_mask(mask, vid.1);
            }
        }

        // This is the index of the normal of the faces given by
        // Aabb::FACES_VERTEX_IDS.
        const FX: u32 = OctantPattern::FACE_X;
        const FY: u32 = OctantPattern::FACE_Y;
        const FZ: u32 = OctantPattern::FACE_Z;
        const FACE_NORMALS: [u32; 6] = [FX, FX, FY, FY, FZ, FZ];

        #[allow(clippy::needless_range_loop)]
        for fid in 0..6 {
            if ((1 << fid) & i) == 0 {
                let vid = Aabb::FACES_VERTEX_IDS[fid];
                let mask = FACE_NORMALS[fid];

                set_mask(mask, vid.0);
                set_mask(mask, vid.1);
                set_mask(mask, vid.2);
                set_mask(mask, vid.3);
            }
        }
        std::println!("0b{:b},", octant_mask);
    }
    std::println!("0,");
    std::println!("];");
}

#[cfg(test)]
mod test {
    #[test]
    fn gen_const_tables() {
        super::gen_const_tables();
    }
}
