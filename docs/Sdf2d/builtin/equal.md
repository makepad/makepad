equal — perform a component-wise equal-to comparison of two vectors
## Declaration
- ``bvec2 equal(bvec2 x, bvec2 y)``
- ``bvec3 equal(bvec3 x, bvec3 y)``
- ``bvec4 equal(bvec4 x, bvec4 y)``
- ``bvec2 equal(ivec2 x, ivec2 y)``
- ``bvec3 equal(ivec3 x, ivec3 y)``
- ``bvec4 equal(ivec4 x, ivec4 y)``
- ``bvec2 equal(vec2 x, vec2 y)``
- ``bvec3 equal(vec3 x, vec3 y)``
- ``bvec4 equal(vec4 x, vec4 y)``
## Parameters
- ``x``:  Specifies the first vector to be used in the comparison operation.
- ``y``:  Specifies the second vector to be used in the comparison operation.
## Description
`equal` returns a boolean vector in which each element _i_ is computed as _`x`_[_i_] == _`y`_[_i_].
## See Also
- [[lessThan]]
- [[lessThanEqual]]
- [[greaterThan]]
- [[greaterThanEqual]]
- [[notEqual]]
- [[any]]
- [[all]]
- [[not]]