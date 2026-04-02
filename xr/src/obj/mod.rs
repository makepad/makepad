pub mod car;
pub mod cube;
pub mod gltf;
pub mod icosphere;
pub mod physics_view;
pub mod refractive_cube;
pub mod shooter;
pub mod tank;
pub mod tree;
pub mod view_splat;

pub use car::{car_drive_command, CarDriveConfig};
pub use cube::Cube;
pub use gltf::Gltf;
pub use icosphere::{DrawIcoSolid, IcoSphere};
pub use physics_view::PhysicsWorld3D;
pub use refractive_cube::RefractiveCube;
pub use shooter::{Shooter, XrProjectileEmitterConfig};
pub use tank::{tank_drive_command, TankDriveConfig};
pub use tree::{
    CpuPythagoreanTree, DrawTreeBranches, DrawTreeLeaves, Tree, PYTHAGOREAN_TREE_ROOT_DROP,
};
pub use view_splat::{DrawSplatPbr, ViewSplat};
