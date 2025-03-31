smoothstep — perform Hermite interpolation between two values
## Declaration
- ``float smoothstep(float edge0, float edge1, float x)``
- ``vec2 smoothstep(vec2 edge0, vec2 edge1, vec2 x)``
- ``vec3 smoothstep(vec3 edge0, vec3 edge1, vec3 x)``
- ``vec4 smoothstep(vec4 edge0, vec4 edge1, vec4 x)``
- ``vec2 smoothstep(float edge0, float edge1, vec2 x)``
- ``vec3 smoothstep(float edge0, float edge1, vec3 x)``
- ``vec4 smoothstep(float edge0, float edge1, vec4 x)``
## Parameters
- ``edge0``:  Specifies the value of the lower edge of the Hermite function.
- ``edge1``:  Specifies the value of the upper edge of the Hermite function.
- ``x``:  Specifies the source value for interpolation.
## Description
`smoothstep` performs smooth Hermite interpolation between 0 and 1 when _`edge0`_ < _`x`_ < _`edge1`_. This is useful in cases where a threshold function with a smooth transition is desired. `smoothstep` is equivalent to:

```
    genType t;  /* Or genDType t; */
    t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
```

`smoothstep` returns 0.0 if _`x`_ ≤ _`edge0`_ and 1.0 if _`x`_ ≥ _`edge1`_.
Results are undefined if _`edge0`_ ≥ _`edge1`_.
## See Also
- [[mix]]
- [[step]]