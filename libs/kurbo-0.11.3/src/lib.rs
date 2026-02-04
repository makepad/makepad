// Copyright 2018 the Kurbo Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 2D geometry, with a focus on curves.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![allow(
    clippy::unreadable_literal,
    clippy::many_single_char_names,
    clippy::excessive_precision,
    clippy::bool_to_int_with_if,
    missing_debug_implementations,
    elided_lifetimes_in_paths,
    single_use_lifetimes,
    trivial_numeric_casts,
    unnameable_types,
    clippy::use_self,
    clippy::return_self_not_must_use,
    clippy::cast_possible_truncation,
    clippy::wildcard_imports,
    clippy::shadow_unrelated,
    clippy::missing_assert_message,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::exhaustive_enums,
    clippy::match_same_arms,
    clippy::partial_pub_fields,
    clippy::unseparated_literal_suffix,
    clippy::duplicated_attributes
)]

#[cfg(not(any(feature = "std", feature = "libm")))]
compile_error!("kurbo requires either the `std` or `libm` feature");

#[cfg(all(feature = "std", feature = "libm"))]
use libm as _;

extern crate alloc;

mod affine;
mod arc;
mod bezpath;
mod circle;
pub mod common;
mod cubicbez;
mod ellipse;
mod insets;
mod line;
mod param_curve;
mod point;
mod quadbez;
mod quadspline;
mod rect;
mod rounded_rect;
mod rounded_rect_radii;
mod shape;
mod size;
mod svg;
mod vec2;

pub use crate::affine::Affine;
pub use crate::arc::{Arc, ArcAppendIter};
pub use crate::bezpath::{flatten, segments, BezPath, PathEl, PathSeg, PathSegIter, Segments};
pub use crate::circle::Circle;
pub use crate::cubicbez::{CubicBez, CubicBezIter};
pub use crate::ellipse::Ellipse;
pub use crate::insets::Insets;
pub use crate::line::{Line, LinePathIter};
pub use crate::param_curve::{
    Nearest, ParamCurve, ParamCurveArclen, ParamCurveArea, ParamCurveCurvature, ParamCurveDeriv,
    ParamCurveExtrema, ParamCurveNearest, DEFAULT_ACCURACY, MAX_EXTREMA,
};
pub use crate::point::Point;
pub use crate::quadbez::{QuadBez, QuadBezIter};
pub use crate::quadspline::QuadSpline;
pub use crate::rect::Rect;
pub use crate::rounded_rect::RoundedRect;
pub use crate::rounded_rect_radii::RoundedRectRadii;
pub use crate::shape::Shape;
pub use crate::size::Size;
pub use crate::svg::{SvgArc, SvgParseError};
pub use crate::vec2::Vec2;
