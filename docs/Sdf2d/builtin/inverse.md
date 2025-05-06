inverse — calculate the inverse of a matrix
## Declaration
- ``mat4 inverse(mat4 m)``
## Parameters
- ``m``:  Specifies the matrix of which to take the inverse.
## Description
`inverse` returns the inverse of the matrix _`m`_. The values in the returned matrix are undefined if _`m`_ is singular or poorly-conditioned (nearly singular).
## See Also
- [[transpose]]