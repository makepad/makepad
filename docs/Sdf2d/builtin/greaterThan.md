greaterThan — perform a component-wise greater-than comparison of two vectors
## Declaration
- ``bvec2 greaterThan(ivec2 x, ivec2 y)``
- ``bvec3 greaterThan(ivec3 x, ivec3 y)``
- ``bvec4 greaterThan(ivec4 x, ivec4 y)``
- ``bvec2 greaterThan(vec2 x, vec2 y)``
- ``bvec3 greaterThan(vec3 x, vec3 y)``
- ``bvec4 greaterThan(vec4 x, vec4 y)``
## Parameters
- ``x``:  Specifies the first vector to be used in the comparison operation.
- ``y``:  Specifies the second vector to be used in the comparison operation.
## Description
`greaterThan` returns a boolean vector in which each element _i_ is computed as _`x`_[_i_] > _`y`_[_i_].
## See Also
- [[lessThan]]
- [[lessThanEqual]]
- [[greaterThanEqual]]
- [[equal]]
- [[notEqual]]
- [[any]]
- [[all]]
- [[not]]