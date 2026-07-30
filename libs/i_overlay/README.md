# iOverlay

[![crates.io version](https://img.shields.io/crates/v/i_overlay.svg)](https://crates.io/crates/i_overlay)
[![docs.rs docs](https://docs.rs/i_overlay/badge.svg)](https://docs.rs/i_overlay)
[![tests](https://github.com/iShape-Rust/iOverlay/actions/workflows/tests.yml/badge.svg)](https://github.com/iShape-Rust/iOverlay/actions/workflows/tests.yml)
[![codecov](https://codecov.io/gh/iShape-Rust/iOverlay/branch/main/graph/badge.svg)](https://codecov.io/gh/iShape-Rust/iOverlay)
[![license](https://img.shields.io/crates/l/i_overlay.svg)](https://crates.io/crates/i_overlay)

![Balloons](readme/balloons.svg)

iOverlay is a high-performance polygon overlay engine for Rust. It solves robust boolean operations on complex polygons for GIS, CAD, and graphics workflows, built for developers who need reliable geometry at scale across integer and floating-point APIs.

iOverlay powers polygon boolean operations in [geo](https://github.com/georust/geo).
    
## Table of Contents

- [Why iOverlay?](#why-ioverlay)
- [Features](#features)
- [Demo](#demo)
- [Performance](#performance)
- [Getting Started](#getting-started)
  - [Quick Start](#quick-start)
- [Boolean Operations](#boolean-operations)
  - [Simple Example](#simple-example)
  - [Overlay Rules](#overlay-rules)
  - [Edge Attributes and Provenance](#edge-attributes-and-provenance)
- [Spatial Predicates](#spatial-predicates)
- [Custom Point Type Support](#custom-point-type-support)
- [Slicing & Clipping](#slicing--clipping)
  - [Slicing a Polygon with a Polyline](#slicing-a-polygon-with-a-polyline)
  - [Clipping a Polyline by a Polygon](#clipping-a-polyline-by-a-polygon)
- [Buffering](#buffering)
  - [Offseting a Path](#offseting-a-path)
  - [Offseting a Polygon](#offseting-a-polygon)
  - [LineCap](#linecap)
  - [LineJoin](#linejoin)
- [FAQ](#faq)
- [License](#license)

&nbsp;
## Why iOverlay?

- Built for robust polygon overlays where precision matters (GIS, CAD, graphics).
- High performance with predictable results across complex inputs.
- Supports both integer and floating-point APIs for flexible pipelines.
- OGC-valid output is available when strict topology is required.
- Core overlay engine used in [geo](https://github.com/georust/geo).

&nbsp;
## Features

- **Boolean Operations**: union, intersection, difference, and exclusion.
- **Spatial Predicates**: `intersects`, `disjoint`, `interiors_intersect`, `touches`, `within`, `covers` with early-exit optimization.
- **Polyline Operations**: clip and slice.
- **Polygons**: with holes, self-intersections, and multiple contours.
- **Simplification**: removes degenerate vertices and merges collinear edges.
- **Buffering**: offsets paths and polygons.
- **Fill Rules**: even-odd, non-zero, positive and negative.
- **Data Types**: Supports `i16`/`i32`/`i64` integer APIs and `f32`/`f64` floating-point APIs.

&nbsp;
## Demo

- [Stars Rotation](https://ishape-rust.github.io/iShape-js/overlay/stars_demo.html)
- [Boolean Operations](https://ishape-rust.github.io/iShape-js/overlay/shapes_editor.html)
- [Stroke Offset](https://ishape-rust.github.io/iShape-js/overlay/stroke.html)
- [Polygon Offset](https://ishape-rust.github.io/iShape-js/overlay/outline.html)
- [Overlay Editor](https://ishape-rust.github.io/iShape-js/overlay/overlay_editor.html)

&nbsp;
## Performance

iOverlay supports:

- `i16`/`i32`/`i64` math solvers
- `on`/`off` multithreading feature

<img src="readme/average_relative_time.svg" alt="Average relative time for iOverlay Rust solvers" style="max-width:860px;width:100%;">

For bigger data sets, the math engine and multithreading mode have a larger impact on runtime.

See the detailed reports: [Performance Comparison](https://ishape-rust.github.io/iShape-js/overlay/performance/performance.html) and [Rust Solver Benchmarks](https://ishape-rust.github.io/iShape-js/overlay/performance/rust_i_overlay.html)

&nbsp;
## Getting Started

Add the following to your Cargo.toml:
```toml
[dependencies]
i_overlay = "^7.0"
```

Read full [documentation](https://ishape-rust.github.io/iShape-js/overlay/doc.html)

### Quick Start

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

let subj = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
let clip = [[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]];

let result = subj.overlay(&clip, OverlayRule::Intersect, FillRule::EvenOdd);
println!("result: {:?}", result);
```

&nbsp;
## Boolean Operations

### Simple Example

<img src="readme/example_union.svg" alt="Simple Example" style="width:400px;">
Here's an example of performing a union operation between two polygons:

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

// Define the subject "O"
let subj = [
    // main contour
    vec![
      [1.0, 0.0],
      [4.0, 0.0],
      [4.0, 5.0],
      [1.0, 5.0], // the contour is auto closed!
    ],
    // hole contour
    vec![
      [2.0, 1.0],
      [2.0, 4.0],
      [3.0, 4.0],
      [3.0, 1.0], // the contour is auto closed!
    ],
];

// Define the clip "-"
let clip = [
    // main contour
    [0.0, 2.0],
    [5.0, 2.0],
    [5.0, 3.0],
    [0.0, 3.0], // the contour is auto closed!
];

let result = subj.overlay(&clip, OverlayRule::Union, FillRule::EvenOdd);

println!("result: {:?}", result);
```
&nbsp;

The result is a vec of shapes:
```text
[
    // first shape
    [
        // main contour (counterclockwise order)
        [
            [0.0, 3.0], [0.0, 2.0], [1.0, 2.0], [1.0, 0.0], [4.0, 0.0], [4.0, 2.0], [5.0, 2.0], [5.0, 3.0], [4.0, 3.0], [4.0, 5.0], [1.0, 5.0], [1.0, 3.0]
        ],
        // first hole (clockwise order)
        [
            [2.0, 1.0], [2.0, 2.0], [3.0, 2.0], [3.0, 1.0]
        ],
        // second hole (clockwise order)
        [
            [2.0, 3.0], [2.0, 4.0], [3.0, 4.0], [3.0, 3.0]
        ]
    ]
    // ... other shapes if present
]
```
&nbsp;

The `overlay` function returns a `Vec<Shapes>`:

- `Vec<Shape>`: A collection of shapes.
- `Shape`: Represents a shape made up of:
  - `Vec<Contour>`: A list of contours.
  - The first contour is the outer boundary (counterclockwise), and subsequent contours represent holes (clockwise).
- `Contour`: A sequence of points (`Vec<P: FloatPointCompatible>`) forming a closed contour.

**Note**: By default, outer boundaries are counterclockwise and holes are clockwise—unless `main_direction` is set. [More information](https://ishape-rust.github.io/iShape-js/overlay/contours/contours.html) about contours.


&nbsp;
### Overlay Rules
| A,B | A ∪ B | A ∩ B | A - B | B - A | A ⊕ B |
|---------|---------------|----------------------|----------------|--------------------|----------------|
| <img src="readme/ab.svg" alt="AB" style="width:100px;"> | <img src="readme/union.svg" alt="Union" style="width:100px;"> | <img src="readme/intersection.svg" alt="Intersection" style="width:100px;"> | <img src="readme/difference_ab.svg" alt="Difference" style="width:100px;"> | <img src="readme/difference_ba.svg" alt="Inverse Difference" style="width:100px;"> | <img src="readme/exclusion.svg" alt="Exclusion" style="width:100px;"> |

&nbsp;
### Edge Attributes and Provenance

Use `EdgeOverlay` when the result boundary needs to keep data from the original input edges: source ids, layer ids, material ids, constraints, styles, or any other edge provenance. The regular `Overlay` API remains optimized for plain geometry; this API is opt-in and works with user-defined payloads.

When an input edge is split by intersections, its data is copied by default. When coincident edges are merged, your `OverlayEdgeData` implementation decides what the resulting data means. A common policy is to keep identical values and mark conflicts as `Undefined`.

```rust
use i_overlay::core::edge_data::{EdgeDataMerge, OverlayEdgeData};
use i_overlay::core::edge_overlay::{EdgeOverlay, InputEdge};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::ShapeType;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::segm::boolean::ShapeCountBoolean;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeKind {
    Red,
    Green,
    Undefined,
}

impl OverlayEdgeData for EdgeKind {
    fn merge(ctx: EdgeDataMerge<ShapeCountBoolean, Self>) -> Self {
        match (ctx.lhs_data, ctx.rhs_data) {
            (EdgeKind::Red, EdgeKind::Red) => EdgeKind::Red,
            (EdgeKind::Green, EdgeKind::Green) => EdgeKind::Green,
            _ => EdgeKind::Undefined,
        }
    }
}

let mut overlay = EdgeOverlay::new(8);

let red_square = [
    [0, 0], [4, 0], [4, 4], [0, 4],
];
let green_square = [
    [2, 0], [6, 0], [6, 4], [2, 4],
];

for edge in red_square.windows(2).map(|w| (w[0], w[1]))
    .chain([(red_square[3], red_square[0])])
{
    overlay.add_edge(InputEdge {
        a: IntPoint::new(edge.0[0], edge.0[1]),
        b: IntPoint::new(edge.1[0], edge.1[1]),
        data: EdgeKind::Red,
    }, ShapeType::Subject);
}

for edge in green_square.windows(2).map(|w| (w[0], w[1]))
    .chain([(green_square[3], green_square[0])])
{
    overlay.add_edge(InputEdge {
        a: IntPoint::new(edge.0[0], edge.0[1]),
        b: IntPoint::new(edge.1[0], edge.1[1]),
        data: EdgeKind::Green,
    }, ShapeType::Clip);
}

let shapes = overlay.build_vector_shapes(OverlayRule::Union, FillRule::NonZero);

// The result is grouped as shapes -> contours -> edges.
// Each output edge contains geometry, fill, and the propagated user data.
assert_eq!(shapes.len(), 1);
assert!(shapes[0][0].iter().any(|edge| edge.data == EdgeKind::Undefined));
```

This API currently targets integer boolean operations and exports the result as vector shapes. It keeps the same contour structure as the regular polygon API, but each contour item is an edge with propagated data. Collinear edges are simplified only when their data is equal, so attribute boundaries are preserved.

&nbsp;
## Spatial Predicates

When you only need to know *whether* two shapes have a spatial relationship—not compute their intersection geometry—use spatial predicates for better performance:

```rust
use i_overlay::float::relate::FloatRelate;

let outer = vec![[0.0, 0.0], [0.0, 20.0], [20.0, 20.0], [20.0, 0.0]];
let inner = vec![[5.0, 5.0], [5.0, 15.0], [15.0, 15.0], [15.0, 5.0]];
let adjacent = vec![[20.0, 0.0], [20.0, 10.0], [30.0, 10.0], [30.0, 0.0]];
let distant = vec![[100.0, 100.0], [100.0, 110.0], [110.0, 110.0], [110.0, 100.0]];

// intersects: shapes share any point (interior or boundary)
assert!(outer.intersects(&inner));
assert!(outer.intersects(&adjacent));  // edge contact counts

// disjoint: shapes share no points (negation of intersects)
assert!(outer.disjoint(&distant));

// interiors_intersect: interiors overlap (stricter than intersects)
assert!(outer.interiors_intersect(&inner));
assert!(!outer.interiors_intersect(&adjacent));  // edge-only contact

// touches: boundaries intersect but interiors don't
assert!(outer.touches(&adjacent));
assert!(!outer.touches(&inner));  // interiors overlap

// within: first shape completely inside second
assert!(inner.within(&outer));
assert!(!outer.within(&inner));

// covers: first shape completely contains second
assert!(outer.covers(&inner));
assert!(!inner.covers(&outer));
```

These methods use early-exit optimization, returning as soon as the predicate can be determined without processing remaining segments.

### Fixed-Scale Predicates

For consistent precision across operations, use `FixedScaleFloatRelate`:

```rust
use i_overlay::float::scale::FixedScaleFloatRelate;

let square = vec![[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]];
let other = vec![[5.0, 5.0], [5.0, 15.0], [15.0, 15.0], [15.0, 5.0]];

let scale = 1000.0;  // or 1.0 / grid_size

let result = square.intersects_with_fixed_scale(&other, scale);
assert!(result.unwrap());
```

For more control, use `FloatPredicateOverlay` directly:

```rust
use i_overlay::float::relate::FloatPredicateOverlay;

let square = vec![[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]];
let clip = vec![[5.0, 5.0], [5.0, 15.0], [15.0, 15.0], [15.0, 5.0]];

// Use fixed-scale constructor
let mut overlay = FloatPredicateOverlay::with_subj_and_clip_fixed_scale(
    &square, &clip, 1000.0
).unwrap();

assert!(overlay.intersects());
```

&nbsp;
## Custom Point Type Support
`iOverlay` allows users to define custom point types, as long as they implement the `FloatPointCompatible` trait.
```rust
use i_overlay::i_float::float::compatible::FloatPointCompatible;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

#[derive(Clone, Copy, Debug)]
struct CustomPoint {
  x: f32,
  y: f32,
}

impl FloatPointCompatible for CustomPoint {
  type Scalar = f32;

  fn from_xy(x: f32, y: f32) -> Self {
    Self { x, y }
  }

  fn x(&self) -> f32 {
    self.x
  }

  fn y(&self) -> f32 {
    self.y
  }
}

let subj = [
    CustomPoint { x: 0.0, y: 0.0 },
    CustomPoint { x: 0.0, y: 3.0 },
    CustomPoint { x: 3.0, y: 3.0 },
    CustomPoint { x: 3.0, y: 0.0 },
];

let clip = [
    CustomPoint { x: 1.0, y: 1.0 },
    CustomPoint { x: 1.0, y: 2.0 },
    CustomPoint { x: 2.0, y: 2.0 },
    CustomPoint { x: 2.0, y: 1.0 },
];

let result = subj.overlay(&clip, OverlayRule::Difference, FillRule::EvenOdd);

println!("result: {:?}", result);

```

&nbsp;
## Slicing & Clipping

### Slicing a Polygon with a Polyline
<img src="readme/example_slice.svg" alt="Slicing Example" style="width:400px;">

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::float::slice::FloatSlice;

let polygon = [
    [1.0, 1.0],
    [1.0, 4.0],
    [4.0, 4.0],
    [4.0, 1.0],
];

let slicing_line = [
    [3.0, 5.0],
    [2.0, 2.0],
    [3.0, 3.0],
    [2.0, 0.0],
];

let result = polygon.slice_by(&slicing_line, FillRule::NonZero);

println!("result: {:?}", result);
```
&nbsp;

### Clipping a Polyline by a Polygon
<img src="readme/example_clip.svg" alt="Clip Example" style="width:400px;">

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::float::clip::FloatClip;
use i_overlay::string::clip::ClipRule;

let polygon = [
    [1.0, 1.0],
    [1.0, 4.0],
    [4.0, 4.0],
    [4.0, 1.0],
];

let string_line = [
    [3.0, 5.0],
    [2.0, 2.0],
    [3.0, 3.0],
    [2.0, 0.0],
];

let clip_rule = ClipRule { invert: false, boundary_included: false };
let result = string_line.clip_by(&polygon, FillRule::NonZero, clip_rule);

println!("result: {:?}", result);
```

&nbsp;

## Buffering

### Offsetting a Path
<img src="readme/example_offseting_path.svg" alt="Path Example" style="width:400px;">

```rust
use i_overlay::mesh::stroke::offset::StrokeOffset;
use i_overlay::mesh::style::{LineCap, LineJoin, StrokeStyle};

let path = [
    [ 2.0, 1.0],
    [ 5.0, 1.0],
    [ 8.0, 4.0],
    [11.0, 4.0],
    [11.0, 1.0],
    [ 8.0, 1.0],
    [ 5.0, 4.0],
    [ 2.0, 4.0],
];

let style = StrokeStyle::new(1.0)
    .line_join(LineJoin::Miter(1.0))
    .start_cap(LineCap::Round(0.1))
    .end_cap(LineCap::Square);

let shapes = path.stroke(style, false);

println!("result: {:?}", shapes);
```
&nbsp;

### Offsetting a Polygon
<img src="readme/example_offseting_polygon.svg" alt="Path Example" style="width:400px;">

```rust
use i_overlay::mesh::outline::offset::OutlineOffset;
use i_overlay::mesh::style::{LineJoin, OutlineStyle};

let shape = vec![
    vec![
        [2.0, 1.0],
        [4.0, 1.0],
        [5.0, 2.0],
        [13.0, 2.0],
        [13.0, 3.0],
        [12.0, 3.0],
        [12.0, 4.0],
        [11.0, 4.0],
        [11.0, 3.0],
        [10.0, 3.0],
        [9.0, 4.0],
        [8.0, 4.0],
        [8.0, 3.0],
        [5.0, 3.0],
        [5.0, 4.0],
        [4.0, 5.0],
        [2.0, 5.0],
        [1.0, 4.0],
        [1.0, 2.0]
    ],
    vec![
        [2.0, 4.0],
        [4.0, 4.0],
        [4.0, 2.0],
        [2.0, 2.0]
    ],
];

let style = OutlineStyle::new(0.2).line_join(LineJoin::Round(0.1));
let shapes = shape.outline(&style);

println!("shapes: {:?}", &shapes);
```

**Note**: 
- Offsetting a polygon works reliably only with valid polygons. Ensure that:
  - No self-intersections.
  - Outer boundaries are **counterclockwise**, holes are **clockwise**—unless `main_direction` is set.
  
  If polygon validity cannot be guaranteed, it is recommended to apply the [simplify_shape](https://github.com/iShape-Rust/iOverlay/blob/main/iOverlay/src/float/simplify.rs) operation before offsetting.  
  [More information](https://ishape-rust.github.io/iShape-js/overlay/contours/contours.html) on contour orientation.

&nbsp;
### LineCap
| Butt | Square | Round | Custom |
|------|--------|-------|--------|
| <img src="readme/line_cap_butt.svg" alt="Butt" style="width:100px;"> | <img src="readme/line_cap_square.svg" alt="Square" style="width:100px;"> | <img src="readme/line_cap_round.svg" alt="Round" style="width:100px;"> | <img src="readme/line_cap_custom.svg" alt="Custom" style="width:100px;"> |

&nbsp;
### LineJoin
| Bevel | Miter | Round |
|-------|--------|-------|
| <img src="readme/line_join_bevel.svg" alt="Bevel" style="width:100px;"> | <img src="readme/line_join_mitter.svg" alt="Miter" style="width:100px;"> | <img src="readme/line_join_round.svg" alt="Round" style="width:100px;"> |

&nbsp;

## FAQ
### 1. When should I use `FloatOverlay`, `SingleFloatOverlay`, or `FloatOverlayGraph`?

- Use **`FloatOverlay`** when you perform **repeated overlay operations**:
  ```rust
  use i_overlay::core::fill_rule::FillRule;
  use i_overlay::core::overlay_rule::OverlayRule;
  use i_overlay::float::overlay::FloatOverlay;

  let subj = vec![[0.0, 0.0], [0.0, 5.0], [5.0, 5.0], [5.0, 0.0]];
  let clip = vec![[2.0, 2.0], [2.0, 4.0], [4.0, 4.0], [4.0, 2.0]];
  let next_clip = vec![[1.0, 1.0], [1.0, 3.0], [3.0, 3.0], [3.0, 1.0]];

  let mut overlay = FloatOverlay::with_subj_and_clip(&subj, &clip);
  let result = overlay.overlay(OverlayRule::Difference, FillRule::EvenOdd);

  overlay.reinit_with_subj_and_clip(&subj, &next_clip);
  let next_result = overlay.overlay(OverlayRule::Difference, FillRule::EvenOdd);
  // ...
  ```

- Use **`SingleFloatOverlay`** trait for **one-shot operations**.

- Use **`FloatOverlayGraph`** if you need to extract **multiple boolean results** (e.g. union and intersection) from the **same input geometry** without recomputing.

---

### 2. I need to union many shapes at once. What's the most efficient way?

Use the [`simplify_shape`](https://github.com/iShape-Rust/iOverlay/blob/main/iOverlay/src/float/simplify.rs) operation:

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::float::simplify::SimplifyShape;

let shapes = vec![
    vec![[0.0, 0.0], [0.0, 2.0], [2.0, 2.0], [2.0, 0.0]],
    vec![[2.0, 0.0], [2.0, 2.0], [4.0, 2.0], [4.0, 0.0]],
];

let result = shapes.simplify_shape(FillRule::EvenOdd);
```

It internally merges shapes efficiently and is typically faster and more robust than chaining many `overlay()` calls manually.

---

### 3. How do I use a fixed grid size (fixed precision) for float overlays?

Use `FixedScaleFloatOverlay` or `FloatOverlay::with_subj_and_clip_fixed_scale`. The scale is
`scale = 1.0 / grid_size`.

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::scale::FixedScaleFloatOverlay;

let subj = vec![[0.0, 0.0], [0.0, 5.0], [5.0, 5.0], [5.0, 0.0]];
let clip = vec![[2.0, 2.0], [2.0, 4.0], [4.0, 4.0], [4.0, 2.0]];

let grid_size = 0.001;
let scale = 1.0 / grid_size;

let result = subj
    .overlay_with_fixed_scale(&clip, OverlayRule::Difference, FillRule::EvenOdd, scale)
    .expect("scale does not fit input bounds");
```

If you need more control, use `FloatPointAdapter::with_scale` and `FloatOverlay::with_adapter`.

---

### 4. How do I select the integer engine for float overlays?

If you need control over the float-to-integer precision range, select the integer engine at
compile time with the `from_*` constructors. The supported engines are `i16`, `i32`, and `i64`.
The default engine is `i32`; use `i64` when the input bounds need a wider integer range.

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::overlay::FloatOverlay;

let subj = vec![[0.0, 0.0], [0.0, 5.0], [5.0, 5.0], [5.0, 0.0]];
let clip = vec![[2.0, 2.0], [2.0, 4.0], [4.0, 4.0], [4.0, 2.0]];

let mut overlay = FloatOverlay::<[f64; 2], i64>::from_subj_and_clip(&subj, &clip);
let result = overlay.overlay(OverlayRule::Difference, FillRule::EvenOdd);
```

The sugar traits also use `i32` by default. Use the `*_as::<I>` methods when you want to keep
the shorthand API and still select the integer engine explicitly:

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

let subj = vec![[0.0, 0.0], [0.0, 5.0], [5.0, 5.0], [5.0, 0.0]];
let clip = vec![[2.0, 2.0], [2.0, 4.0], [4.0, 4.0], [4.0, 2.0]];

let result = subj.overlay_as::<i64>(&clip, OverlayRule::Difference, FillRule::EvenOdd);
```

---

### 5. How do I enable OGC-valid output?

Set the `ogc` flag in `OverlayOptions`.

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::overlay::{FloatOverlay, OverlayOptions};

//     0   1   2   3   4   5
//   5 ┌───────────────────┐
//     │                   │
//   4 │   ┌───────┐       │
//     │   │ ░   ░ │       │   Two L-shaped holes share vertices at (2,2) and (3,3)
//   3 │   │   ┌───●───┐   │
//     │   │ ░ │   │ ░ │   │   ░ = holes
//   2 │   └───●───┘   │   │
//     │       │ ░   ░ │   │   The shared edge disconnects the interior
//   1 │       └───────┘   │
//     │                   │
//   0 └───────────────────┘
//
// OGC Simple Feature Specification (ISO 19125-1) states:
// "The interior of every Surface is a connected point set."

let subj = vec![vec![[0.0, 0.0], [5.0, 0.0], [5.0, 5.0], [0.0, 5.0]]];
let clip = vec![
    vec![[1.0, 2.0], [1.0, 4.0], [3.0, 4.0], [3.0, 3.0], [2.0, 3.0], [2.0, 2.0]],
    vec![[2.0, 1.0], [2.0, 2.0], [3.0, 2.0], [3.0, 3.0], [4.0, 3.0], [4.0, 1.0]],
];

let options = OverlayOptions::<f64>::ogc();
let mut overlay = FloatOverlay::with_subj_and_clip_custom(&subj, &clip, options, Default::default());
let result = overlay.overlay(OverlayRule::Difference, FillRule::EvenOdd);

assert_eq!(result.len(), 2);
```

---

## License

Licensed under either of:
- MIT license (LICENSE-MIT)
- Apache License, Version 2.0 (LICENSE-APACHE)
