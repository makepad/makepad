// Port of box3d/include/box3d/id.h
// These ids serve as handles to internal Box3D objects. All ids are considered
// null if initialized to zero (index1 == 0; stored index is index + 1).

/// World id references a world instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WorldId {
    pub index1: u16,
    pub generation: u16,
}

/// Body id references a body instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BodyId {
    pub index1: i32,
    pub world0: u16,
    pub generation: u16,
}

/// Shape id references a shape instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ShapeId {
    pub index1: i32,
    pub world0: u16,
    pub generation: u16,
}

/// Joint id references a joint instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct JointId {
    pub index1: i32,
    pub world0: u16,
    pub generation: u16,
}

/// Contact id references a contact instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ContactId {
    pub index1: i32,
    pub world0: u16,
    pub generation: u32,
}

pub const NULL_WORLD_ID: WorldId = WorldId { index1: 0, generation: 0 };
pub const NULL_BODY_ID: BodyId = BodyId { index1: 0, world0: 0, generation: 0 };
pub const NULL_SHAPE_ID: ShapeId = ShapeId { index1: 0, world0: 0, generation: 0 };
pub const NULL_JOINT_ID: JointId = JointId { index1: 0, world0: 0, generation: 0 };
pub const NULL_CONTACT_ID: ContactId = ContactId { index1: 0, world0: 0, generation: 0 };

macro_rules! impl_id_null {
    ($t:ty) => {
        impl $t {
            /// B3_IS_NULL
            #[inline]
            pub fn is_null(self) -> bool {
                self.index1 == 0
            }
            /// B3_IS_NON_NULL
            #[inline]
            pub fn is_non_null(self) -> bool {
                self.index1 != 0
            }
        }
    };
}

impl WorldId {
    #[inline]
    pub fn is_null(self) -> bool {
        self.index1 == 0
    }
    #[inline]
    pub fn is_non_null(self) -> bool {
        self.index1 != 0
    }
}

impl_id_null!(BodyId);
impl_id_null!(ShapeId);
impl_id_null!(JointId);
impl_id_null!(ContactId);

/// Store a world id into a uint32_t.
pub fn store_world_id(id: WorldId) -> u32 {
    ((id.index1 as u32) << 16) | (id.generation as u32)
}

/// Load a uint32_t into a world id.
pub fn load_world_id(x: u32) -> WorldId {
    WorldId { index1: (x >> 16) as u16, generation: x as u16 }
}

/// Store a body id into a uint64_t.
pub fn store_body_id(id: BodyId) -> u64 {
    ((id.index1 as u64) << 32) | ((id.world0 as u64) << 16) | (id.generation as u64)
}

/// Load a uint64_t into a body id.
pub fn load_body_id(x: u64) -> BodyId {
    BodyId { index1: (x >> 32) as i32, world0: (x >> 16) as u16, generation: x as u16 }
}

/// Store a shape id into a uint64_t.
pub fn store_shape_id(id: ShapeId) -> u64 {
    ((id.index1 as u64) << 32) | ((id.world0 as u64) << 16) | (id.generation as u64)
}

/// Load a uint64_t into a shape id.
pub fn load_shape_id(x: u64) -> ShapeId {
    ShapeId { index1: (x >> 32) as i32, world0: (x >> 16) as u16, generation: x as u16 }
}

/// Store a joint id into a uint64_t.
pub fn store_joint_id(id: JointId) -> u64 {
    ((id.index1 as u64) << 32) | ((id.world0 as u64) << 16) | (id.generation as u64)
}

/// Load a uint64_t into a joint id.
pub fn load_joint_id(x: u64) -> JointId {
    JointId { index1: (x >> 32) as i32, world0: (x >> 16) as u16, generation: x as u16 }
}

/// Store a contact id into three uint32 values.
pub fn store_contact_id(id: ContactId, values: &mut [u32; 3]) {
    values[0] = id.index1 as u32;
    values[1] = id.world0 as u32;
    values[2] = id.generation;
}

/// Load a contact id from three uint32 values.
pub fn load_contact_id(values: &[u32; 3]) -> ContactId {
    ContactId { index1: values[0] as i32, world0: values[1] as u16, generation: values[2] }
}
