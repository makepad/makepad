refract — calculate the refraction direction for an incident vector
## Declaration
- ``float refract(float I, float N, float eta)``
- ``vec2 refract(vec2 I, vec2 N, float eta)``
- ``vec3 refract(vec3 I, vec3 N, float eta)``
- ``vec4 refract(vec4 I, vec4 N, float eta)``
## Parameters
- ``I``:  Specifies the incident vector.
- ``N``:  Specifies the normal vector.
- ``eta``:  Specifies the ratio of indices of refraction.
## Description
For a given incident vector _`I`_, surface normal _`N`_ and ratio of indices of refraction, _`eta`_, `refract` returns the refraction vector, _`R`_.
_`R`_ is calculated as:
```
    k = 1.0 - eta * eta * (1.0 - dot(N, I) * dot(N, I));
    if (k < 0.0)
        R = genType(0.0);       // or genDType(0.0)
    else
        R = eta * I - (eta * dot(N, I) + sqrt(k)) * N;
```
The input parameters _`I`_ and _`N`_ should be normalized in order to achieve the desired result.
## See Also
- [[dot]]
- [[reflect]]