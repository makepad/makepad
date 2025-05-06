lessThanEqual — perform a component-wise less-than-or-equal comparison of two vectors
## Declaration
- ``bvec2 lessThanEqual(ivec2 x, ivec2 y)``
- ``bvec3 lessThanEqual(ivec3 x, ivec3 y)``
- ``bvec4 lessThanEqual(ivec4 x, ivec4 y)``
- ``bvec2 lessThanEqual(vec2 x, vec2 y)``
- ``bvec3 lessThanEqual(vec3 x, vec3 y)``
- ``bvec4 lessThanEqual(vec4 x, vec4 y)``
## Parameters
- ``x``:  Specifies the first vector to be used in the comparison operation.
- ``y``:  Specifies the second vector to be used in the comparison operation.
## Description
`lessThanEqual` returns a boolean vector in which each element _i_ is computed as _`x`_[_i_] ≤ _`y`_[_i_].
## See Also
- [[lessThan]]
- [[greaterThan]]
- [[greaterThanEqual]]
- [[equal]]
- [[notEqual]]
- [[any]]
- [[all]]
- [[not]]