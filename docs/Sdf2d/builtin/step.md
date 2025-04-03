step — generate a step function by comparing two values
## Declaration
- ``float step(float edge, float x)``
- ``vec2 step(float edge, vec2 x)``
- ``vec3 step(float edge, vec3 x)``
- ``vec4 step(float edge, vec4 x)``
- ``vec2 step(vec2 edge, vec2 x)``
- ``vec3 step(vec3 edge, vec3 x)``
- ``vec4 step(vec4 edge, vec4 x)``
## Parameters
- ``edge``: Specifies the location of the edge of the step function.
- ``x``:  Specify the value to be used to generate the step function.
## Description
`step` generates a step function by comparing _`x`_ to _`edge`_.
For element _i_ of the return value, 0.0 is returned if _`x`_[_i_] < _`edge`_[_i_], and 1.0 is returned otherwise.
## See Also
- [[mix]]
- [[smoothstep]]