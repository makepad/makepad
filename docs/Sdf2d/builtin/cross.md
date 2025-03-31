cross — calculate the cross product of two vectors
## Declaration
- ``vec3 cross(vec3 x, vec3 y)``
## Parameters
- ``x``: Specifies the first of two vectors
 - ``y``:  Specifies the second of two vectors
## Description
`cross` returns the cross product of two vectors, _`x`_ and _`y`_. i.e.,
$$
\begin{pmatrix} x[1] \cdot y[2] - y[1] \cdot x[2] \\ x[2] \cdot y[0] - y[2] \cdot x[0] \\ x[0] \cdot y[1] - y[0] \cdot x[1] \end{pmatrix}
$$
## See Also
- [[dot]]