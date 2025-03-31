faceforward — return a vector pointing in the same direction as another.
## Declaration
- ``float faceforward(float N, float I, float Nref)``
- ``vec2 faceforward(vec2 N, vec2 I, vec2 Nref)``
- ``vec3 faceforward(vec3 N, vec3 I, vec3 Nref)``
- ``vec4 faceforward(vec4 N, vec4 I, vec4 Nref)``
## Parameters
- ``N``:  Specifies the vector to orient.
- ``I``:  Specifies the incident vector.
- ``Nref``:  Specifies the reference vector.
## Description
`faceforward` orients a vector to point away from a surface as defined by its normal. If [[dot]]``(Nref, I) < 0`` `faceforward` returns _`N`_, otherwise it returns ``-N``.
## See Also
- [[reflect]]
- [[refract]]