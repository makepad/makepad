# Unreleased (0.1.9)
* Use edition 2021.

# 0.1.8
* Do SIMD runtime detection only once on the first `Rasterizer::new` instead of on each.

# 0.1.7
* Fix x86, x86_64 no_std builds, require `std` feature for runtime detected SIMD.

# 0.1.6
* Add runtime detected AVX2 or SSE4.2 line drawing. Improves performance on compatible x86_64 CPUs.

# 0.1.5
* Remove cap of `1.0` for coverage values returned by `for_each_pixel` now `>= 1.0` means fully covered.
  This allows a minor reduction in operations / performance boost.

# 0.1.4
* Add `Rasterizer::reset`, `Rasterizer::clear` methods to allow allocation reuse.

# 0.1.3
* Fix index oob panic scenario.

# 0.1.2
* For `Point` implement `Sub`, `Add`, `SubAssign`, `AddAssign`, `PartialEq`, `PartialOrd`, `From<(x, y)>`,
  `From<[x, y]>` for easier use downstream.
* Switch `Point` `Debug` implementation to output `point(1.2, 3.4)` smaller representation referring to the `point` fn.

# 0.1.1
* Add explicit compile error when building no_std without the "libm" feature.

# 0.1
* Implement zero dependency coverage rasterization for lines, quadratic & cubic beziers.
