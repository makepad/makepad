inversesqrt — return the inverse of the square root of the parameter
## Declaration
- ``float inversesqrt(float x)``
- ``vec2 inversesqrt(vec2 x)``
- ``vec3 inversesqrt(vec3 x)``
- ``vec4 inversesqrt(vec4 x)``
## Parameters
- ``x``:  Specify the value of which to take the inverse of the square root.
## Description
`inversesqrt` returns the inverse of the square root of _`x`_. i.e., the value
$$
\frac{1}{\sqrt{x}}
$$
Results are undefined if 𝑥≤0.
## See Also
- [[sqrt]]