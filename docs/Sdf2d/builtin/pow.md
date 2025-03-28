pow — return the value of the first parameter raised to the power of the second
## Declaration
- ``genType pow(genType x, genType y)``
## Parameters
- ``x``:  Specify the value to raise to the power _`y`_.
- ``y``:  Specify the power to which to raise _`x`_.
## Description
`pow` returns the value of _`x`_ raised to the _`y`_ power. i.e.,
$$
x^{y}
$$
Results are undefined if _`x`_ < 0 or if _`x`_ == 0 and _`y`_ ≤ 0.
## See Also
- [[exp]]