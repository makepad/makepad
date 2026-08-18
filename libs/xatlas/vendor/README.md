# xatlas vendor pin

Upstream: https://github.com/jpcy/xatlas
Commit: `f700c7790aaa030e794b52ba7791a05c085faf0c` (master as of 2026-08-17)

Files:
- `xatlas.h` sha256 `e7675335ad8ab1c1cc9060ad153cf6b8ba2ee914282044eb5f02c49590218fbd`
- `xatlas.cpp` sha256 `0ed0283aad005c94738cb0cc4612dba264379d29dea5b3c9b242f2d4752d5df4`

License: MIT (Jonathan Young) plus bundled MIT notices for thekla_atlas / Fast-BVH.

These C++ sources are the oracle, not a build dependency of the Rust crate.
The Rust port must reproduce `Create` + `AddMesh` + `Generate` (default
`ChartOptions` / `PackOptions`) with `-DXA_MULTITHREADED=0 -DNDEBUG -DXA_DEBUG=0`.
