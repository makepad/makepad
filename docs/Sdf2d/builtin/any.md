any — check whether any element of a boolean vector is true
## Declaration
- ``bool any(bvec2 x)``
- ``bool any(bvec3 x)``
- ``bool any(bvec4 x)``
## Parameters
- ``x`` : Specifies the vector to be tested for truth.
## Description
`any` returns true if any element of _`x`_ is true and false otherwise. It is functionally equivalent to:

```
    bool any(bvec x)       // bvec can be bvec2, bvec3 or bvec4
    {
        bool result = false;
        int i;
        for (i = 0; i < x.length(); ++i)
        {
            result |= x[i];
        }
        return result;
    }
```
## See Also
- [[all]]
- [[not]]