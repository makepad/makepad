dFdx, dFdy — return the partial derivative of an argument with respect to x or y
## Declaration
- ``genType dFdx(genType p)``
## Parameters
- ``p``:  Specifies the expression of which to take the partial derivative.
## Description
_Available only in the fragment shader_, `dFdx` and `dFdy` return the partial derivative of expression _`p`_ in x and y, respectively. Deviatives are calculated using local differencing. Expressions that imply higher order derivatives such as `dFdx(dFdx(n))` have undefined results, as do mixed-order derivatives such as `dFdx(dFdy(n))`. It is assumed that the expression _`p`_ is continuous and therefore, expressions evaluated via non-uniform control flow may be undefined.