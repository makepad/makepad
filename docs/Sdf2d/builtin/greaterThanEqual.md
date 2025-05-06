greaterThanEqual — perform a component-wise greater-than-or-equal comparison of two vectors
## Declaration
- ``bvec2 greaterThanEqual(ivec2 x, ivec2 y)``
- ``bvec3 greaterThanEqual(ivec3 x, ivec3 y)``
- ``bvec4 greaterThanEqual(ivec4 x, ivec4 y)``
- ``bvec2 greaterThanEqual(vec2 x, vec2 y)``
- ``bvec3 greaterThanEqual(vec3 x, vec3 y)``
- ``bvec4 greaterThanEqual(vec4 x, vec4 y)``
## Parameters
- ``x``:  Specifies the first vector to be used in the comparison operation.
- ``y``:  Specifies the second vector to be used in the comparison operation.
## Description
`greaterThanEqual` returns a boolean vector in which each element _i_ is computed as _`x`_[_i_] ≥ _`y`_[_i_].
## See Also
- [[lessThan]]
- [[lessThanEqual]]
- [[greaterThan]]
- [[equal]]
- [[notEqual]]
- [[any]]
- [[all]]
- [[not]]