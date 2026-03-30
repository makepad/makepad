pub mod cube;
pub mod gltf;
pub mod icosphere;
pub mod physics_view;
pub mod refractive_cube;
pub mod shooter;
pub mod tree;
pub mod view_splat;

pub use cube::Cube;
pub use gltf::Gltf;
pub use icosphere::{DrawIcoSolid, IcoSphere};
pub use physics_view::PhysicsWorld3D;
pub use refractive_cube::RefractiveCube;
pub use shooter::{Shooter, XrProjectileEmitterConfig};
pub use tree::{
    CpuPythagoreanTree, DrawTreeBranches, DrawTreeLeaves, Tree, PYTHAGOREAN_TREE_ROOT_DROP,
};
pub use view_splat::{DrawSplatPbr, ViewSplat};
