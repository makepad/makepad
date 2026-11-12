//! Rigid-body dynamics in generalized coordinates: articulated trees whose
//! joints cannot drift or separate, soft contacts and constraints solved as one
//! convex problem (Newton, CG, PGS), tendons and actuators.
//!
//! A faithful Rust port of the C engine named in CREDITS.md (Apache-2.0),
//! module by module. `Model` holds what does
//! not change while stepping, `Data` the state and everything a step computes.
//! Models come from the model file reader (`model_file`) or this crate's
//! builder; `xml_export` writes a model for the reference compiler.

/// A call into a feature this port does not translate yet.
/// `model::check_supported` refuses every model that could reach one.
#[macro_export]
macro_rules! unported {
    ($name:expr) => {
        panic!("{} is not translated yet (a model that needs it is refused at load)", $name)
    };
}

pub mod builder;
pub mod collision_box;
pub mod collision_continuous;
pub mod collision_convex;
pub mod collision_driver;
pub mod collision_gjk;
pub mod collision_primitive;
pub mod collision_sdf;
pub mod core_constraint;
pub mod core_smooth;
pub mod core_util;
pub mod data;
pub mod derivative;
pub mod forward;
pub mod hull;
pub mod inline;
pub mod inverse;
pub mod io;
pub mod island;
pub mod memory;
pub mod metric;
pub mod model;
pub mod model_file;
pub mod model_spec;
pub mod name;
pub mod passive;
pub mod ray;
pub mod sdf;
pub mod sdf_torus;
pub mod sensor;
pub mod setconst;
pub mod sleep;
pub mod solver;
pub mod sort;
pub mod support;
pub mod thread;
pub mod types;
pub mod util_blas;
pub mod util_errmem;
pub mod util_misc;
pub mod util_solve;
pub mod util_sparse;
pub mod util_spatial;
pub mod xml_export;
