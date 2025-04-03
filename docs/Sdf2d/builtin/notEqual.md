notEqual — perform a component-wise not-equal-to comparison of two vectors.
## Declaration
- ``bvec2 notEqual(bvec2 x, bvec2 y)``
- ``bvec3 notEqual(bvec3 x, bvec3 y)``
- ``bvec4 notEqual(bvec4 x, bvec4 y)``
- ``bvec2 notEqual(ivec2 x, ivec2 y)``
- ``bvec3 notEqual(ivec3 x, ivec3 y)``
- ``bvec4 notEqual(ivec4 x, ivec4 y)``
- ``bvec2 notEqual(vec2 x, vec2 y)``
- ``bvec3 notEqual(vec3 x, vec3 y)``
- ``bvec4 notEqual(vec4 x, vec4 y)``
## Parameters
- ``x``:  Specifies the first vector to be used in the comparison operation.
- ``y``:  Specifies the second vector to be used in the comparison operation.
## Description
`notEqual` returns a boolean vector in which each element _i_ is computed as _`x`_[_i_] != _`y`_[_i_].
## See Also
- [[lessThan]]
- [[lessThanEqual]]
- [[greaterThan]]
- [[greaterThanEqual]]
- [[equal]]
- [[any]]
- [[all]]
- [[not]]