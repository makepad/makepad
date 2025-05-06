mod — compute value of one parameter modulo another
## Declaration
- ``float mod(float x, float y)``
- ``vec2 mod(vec2 x, vec2 y)``
- ``vec3 mod(vec3 x, vec3 y)``
- ``vec4 mod(vec4 x, vec4 y)``
- ``vec2 mod(vec2 x, float y)``
- ``vec3 mod(vec3 x, float y)``
- ``vec4 mod(vec4 x, float y)``
## Parameters
- ``x``:  Specify the value to evaluate.
- ``y``:  Specify the value by which to perform the modulo.
## Description
`mod` returns the value of _`x`_ modulo _`y`_. This is computed as _`x`_ - _`y`_ * [[floor]](_`x`_/_`y`_).
## See Also
- [[floor]]