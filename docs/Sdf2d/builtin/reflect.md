reflect — calculate the reflection direction for an incident vector
## Declaration
- ``genType reflect(genType I, genType N)``
## Parameters
- ``I``:  Specifies the incident vector.
- ``N``:  Specifies the normal vector.
## Description
For a given incident vector `I` and surface normal `N` `reflect` returns the reflection direction calculated as ``I - 2.0 * dot(N, I) * N``.
_`N`_ should be normalized in order to achieve the desired result.
## See Also
- [[dot]]
- [[refract]]