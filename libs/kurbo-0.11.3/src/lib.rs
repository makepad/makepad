// Copyright 2018 the Kurbo Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 2D geometry, with a focus on curves.
//!
//! The kurbo library contains data structures and algorithms for curves and
//! vector paths. It was designed to serve the needs of 2D graphics applications,
//! but it is intended to be general enough to be useful for other applications.
//! It can be used as "vocabulary types" for representing curves and paths, and
//! also contains a number of computational geometry methods.
//!
//! # Examples
//!
//! Basic UI-style geometry:
//! ```
//! use kurbo::{Insets, Point, Rect, Size, Vec2};
//!
//! let pt = Point::new(10.0, 10.0);
//! let vector = Vec2::new(5.0, -5.0);
//! let pt2 = pt + vector;
//! assert_eq!(pt2, Point::new(15.0, 5.0));
//!
//! let rect = Rect::from_points(pt, pt2);
//! assert_eq!(rect, Rect::from_origin_size((10.0, 5.0), (5.0, 5.0)));
//!
//! let insets = Insets::uniform(1.0);
//! let inset_rect = rect - insets;
//! assert_eq!(inset_rect.size(), Size::new(3.0, 3.0));
//! ```
//!
//! Finding the closest position on a [`Shape`]'s perimeter to a [`Point`]:
//!
//! ```
//! use kurbo::{Circle, ParamCurve, ParamCurveNearest, Point, Shape};
//!
//! const DESIRED_ACCURACY: f64 = 0.1;
//!
//! /// Given a shape and a point, returns the closest position on the shape's
//! /// perimeter, or `None` if the shape is malformed.
//! fn closest_perimeter_point(shape: impl Shape, pt: Point) -> Option<Point> {
//!     let mut best: Option<(Point, f64)> = None;
//!     for segment in shape.path_segments(DESIRED_ACCURACY) {
//!         let nearest = segment.nearest(pt, DESIRED_ACCURACY);
//!         if best.map(|(_, best_d)| nearest.distance_sq < best_d).unwrap_or(true) {
//!             best = Some((segment.eval(nearest.t), nearest.distance_sq))
//!         }
//!     }
//!     best.map(|(point, _)| point)
//! }
//!
//! let circle = Circle::new((5.0, 5.0), 5.0);
//! let hit_point = Point::new(5.0, -2.0);
//! let expectation = Point::new(5.0, 0.0);
//! let hit = closest_perimeter_point(circle, hit_point).unwrap();
//! assert!(hit.distance(expectation) <= DESIRED_ACCURACY);
//! ```
//!
//! # Features
//!
//! This crate either uses the standard library or the [`libm`] crate for
//! math functionality. The `std` feature is enabled by default, but can be
//! disabled, as long as the `libm` feature is enabled. This is useful for
//! `no_std` environments. However, note that the `libm` crate is not as
//! efficient as the standard library, and that this crate still uses the
//! `alloc` crate regardless.
//!
//! [`libm`]: https://docs.rs/libm

// LINEBENDER LINT SET - lib.rs - v1
// See https://linebender.org/wiki/canonical-lints/
// These lints aren't included in Cargo.toml because they
// shouldn't apply to examples and tests
#![warn(unused_crate_dependencies)]
#![warn(clippy::print_stdout, clippy::print_stderr)]
// END LINEBENDER LINT SET
#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![allow(
    clippy::unreadable_literal,
    clippy::many_single_char_names,
    clippy::excessive_precision,
    clippy::bool_to_int_with_if
)]
// The following lints are part of the Linebender standard set,
// but resolving them has been deferred for now.
// Feel free to send a PR that solves one or more of these.
#![allow(
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

// Suppress the unused_crate_dependencies lint when both std and libm are specified.
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
mod fit;
mod insets;
mod line;
mod mindist;
mod moments;
pub mod offset;
mod param_curve;
mod point;
mod quadbez;
mod quadspline;
mod rect;
mod rounded_rect;
mod rounded_rect_radii;
mod shape;
pub mod simplify;
mod size;
mod stroke;
mod svg;
mod translate_scale;
mod triangle;
mod vec2;

#[cfg(feature = "euclid")]
mod interop_euclid;

pub use crate::affine::Affine;
pub use crate::arc::{Arc, ArcAppendIter};
pub use crate::bezpath::{
    flatten, segments, BezPath, LineIntersection, MinDistance, PathEl, PathSeg, PathSegIter,
    Segments,
};
pub use crate::circle::{Circle, CirclePathIter, CircleSegment};
pub use crate::cubicbez::{cubics_to_quadratic_splines, CubicBez, CubicBezIter, CuspType};
pub use crate::ellipse::Ellipse;
pub use crate::fit::{
    fit_to_bezpath, fit_to_bezpath_opt, fit_to_cubic, CurveFitSample, ParamCurveFit,
};
pub use crate::insets::Insets;
pub use crate::line::{ConstPoint, Line, LinePathIter};
pub use crate::moments::{Moments, ParamCurveMoments};
pub use crate::param_curve::{
    Nearest, ParamCurve, ParamCurveArclen, ParamCurveArea, ParamCurveCurvature, ParamCurveDeriv,
    ParamCurveExtrema, ParamCurveNearest, DEFAULT_ACCURACY, MAX_EXTREMA,
};
pub use crate::point::Point;
pub use crate::quadbez::{QuadBez, QuadBezIter};
pub use crate::quadspline::QuadSpline;
pub use crate::rect::{Rect, RectPathIter};
pub use crate::rounded_rect::{RoundedRect, RoundedRectPathIter};
pub use crate::rounded_rect_radii::RoundedRectRadii;
pub use crate::shape::Shape;
pub use crate::size::Size;
pub use crate::stroke::{
    dash, stroke, Cap, DashIterator, Dashes, Join, Stroke, StrokeOptLevel, StrokeOpts,
};
pub use crate::svg::{SvgArc, SvgParseError};
pub use crate::translate_scale::TranslateScale;
pub use crate::triangle::{Triangle, TrianglePathIter};
pub use crate::vec2::Vec2;
