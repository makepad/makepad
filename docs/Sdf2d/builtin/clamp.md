clamp — constrain a value to lie between two further values
## Declaration
- ``float clamp(float x, float minVal, float maxVal)``
- ``vec2 clamp(vec2 x, vec2 minVal, vec2 maxVal)``
- ``vec3 clamp(vec3 x, vec3 minVal, vec3 maxVal)``
- ``vec4 clamp(vec4 x, vec4 minVal, vec4 maxVal)``
- ``vec2 clamp(vec2 x, float minVal, float maxVal)``
- ``vec3 clamp(vec3 x, float minVal, float maxVal)``
- ``vec4 clamp(vec4 x, float minVal, float maxVal)``

## Parameters
- ``x``:  Specify the value to constrain.
- ``minVal``:  Specify the lower end of the range into which to constrain _`x`_.
- ``maxVal``:  Specify the upper end of the range into which to constrain _`x`_.
## Description
`clamp` returns the value of _`x`_ constrained to the range _`minVal`_ to _`maxVal`_. The returned value is computed as [[min]]([[max]](x, minVal), maxVal). The result is undefined if _`minVal`_ ≥ _`maxVal`_.
## See Also
- [[min]]
- [[max]]