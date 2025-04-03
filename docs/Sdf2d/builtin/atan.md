atan — return the arc-tangent of the parameters
## Declaration
- ``float atan(float x)``
- ``vec2 atan(vec2 x)``
- ``vec3 atan(vec3 x)``
- ``vec4 atan(vec4 x)``
- ``float atan(float y, float x)``
- ``vec2 atan(vec2 y, vec2 x)``
- ``vec3 atan(vec3 y, vec3 x)``
- ``vec4 atan(vec4 y, vec4 x)``
## Parameters
- ``y``: Specify the numerator of the fraction whose arctangent to return.
- ``x``:  Specify the denominator of the fraction whose arctangent to return.
- ``y_over_x``: Specify the fraction whose arctangent to return.
## Description
`atan` returns the angle whose trigonometric arctangent is 
$$
\frac{y}{x}
$$
or `𝑦_𝑜𝑣𝑒𝑟_𝑥`, depending on which overload is invoked. In the first overload, the signs of 𝑦 and 𝑥 are used to determine the quadrant that the angle lies in. The values returned by `atan` in this case are in the range [−𝜋,𝜋]. Results are undefined if 𝑥 is zero.
For the second overload, `atan` returns the angle whose tangent is `𝑦_𝑜𝑣𝑒𝑟_𝑥`. Values returned in this case are in the range
$$ \left[ -\frac{\pi}{2}, \frac{\pi}{2} \right] $$
## See Also
- [[sin]]
- [[cos]]
- [[tan]]
- [[asin]]
- [[acos]]