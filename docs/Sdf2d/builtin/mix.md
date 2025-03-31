mix — linearly interpolate between two values
## Declaration
- ``float mix(float x, float y, float a)``
- ``vec2 mix(vec2 x, vec2 y, vec2 a)``
- ``vec3 mix(vec3 x, vec3 y, vec3 a)``
- ``vec4 mix(vec4 x, vec4 y, vec4 a)``
- ``vec2 mix(vec2 x, vec2 y, float a)``
- ``vec3 mix(vec3 x, vec3 y, float a)``
- ``vec4 mix(vec4 x, vec4 y, float a)``
## Parameters
- ``x``: Specify the start of the range in which to interpolate.
- ``y``: Specify the end of the range in which to interpolate.
- ``a``: Specify the value to use to interpolate between _`x`_ and _`y`_.
## Description

`mix` performs a linear interpolation between _`x`_ and _`y`_ using _`a`_ to weight between them. The return value is computed as follows: 𝑥⋅(1−𝑎)+𝑦⋅𝑎.

For the variants of `mix` where _`a`_ is `genBType`, elements for which _`a`_[_i_] is `false`, the result for that element is taken from _`x`_, and where _`a`_[_i_] is `true`, it will be taken from _`y`_. Components of _`x`_ and _`y`_ that are not selected are allowed to be invalid floating point values and will have no effect on the results.
## See Also
- [[min]]
- [[max]]