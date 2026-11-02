//! Advanced NURBS curve routines: homogeneous points and derivatives, evenly spaced
//! points, second-derivative bounds, planar offset derivatives, curve extension and
//! unbounded evaluation, constrained knot removal, symbolic operators (sum/difference,
//! derivative curves, scalar products) and reparametrisations.
//!
//! Upstream: `source/SMLib/src/NL_CrvAdv.cpp`, split into the plan units
//! `nlib.crv_adv_p1`, `nlib.crv_adv_p2` (`plan/modules.toml`).

pub mod p1;
pub mod p2;
