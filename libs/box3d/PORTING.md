# Box3D → Rust Porting Conventions

Rust port of Box3D (C17, by Erin Catto) from `<repo>/box3d/`. The port stays as
close to the C source as possible: same file structure, same function names,
same algorithm structure, same order of operations (float determinism!).
Do not "improve" algorithms or restructure logic. Translate mechanically.

## Scope decisions

- Single precision only (`BOX3D_DOUBLE_PRECISION` off): `Pos = Vec3`,
  `WorldTransform = Transform` (type aliases).
- SIMD: the `B3_SIMD_NONE` path only. `b3V32` = `[f32; 4]` scalar emulation,
  `B3_SIMD_WIDTH = 4`.
- Threading: `parallel_for` runs serially (single worker). No scheduler
  threads. Keep the same code structure so stage ordering matches C with
  workerCount = 1.
- Recording/replay/world_snapshot: not ported (yet).
- Tracy/dump/timer profiling: stubbed or `std::time`.
- No external crates. std only.

## Naming

- Strip the `b3`/`B3_` prefix, then:
  - Types: keep CamelCase: `b3Vec3` → `Vec3`, `b3WorldDef` → `WorldDef`,
    `b3AABB` → `AABB`.
  - Functions: snake_case: `b3MakeQuatFromAxisAngle` → `make_quat_from_axis_angle`,
    `b3Body_GetPosition` → `body_get_position`, `b3World_Step` → `world_step`,
    `b3DynamicTree_CreateProxy` → `dynamic_tree_create_proxy`.
  - Constants: `B3_LINEAR_SLOP` → `linear_slop()` (function, depends on length
    units), `B3_MAX_MANIFOLD_POINTS` → `MAX_MANIFOLD_POINTS`.
  - Enum values: `b3_dynamicBody` → `BodyType::Dynamic`,
    `b3_hullShape` → `ShapeType::Hull`.
- Struct fields: snake_case of the C name: `lowerBound` → `lower_bound`.
- One Rust module per C file: `src/dynamic_tree.rs` ← `src/dynamic_tree.c`.
  `.h`-only inline functions go in the module of the matching `.c`, or
  `math_internal.rs` etc. for header-only files.

## Core mappings

| C | Rust |
|---|---|
| `b3Vec3` struct literal `(b3Vec3){x,y,z}` | `vec3(x, y, z)` helper |
| `B3_NULL_INDEX` (-1) | `NULL_INDEX` (i32 = -1) |
| `int` indices | `i32`, cast `as usize` at slice access |
| `b3Array(T)` dynamic arrays | `Vec<T>` |
| out-params `float* t` | `&mut f32` (keep out-param style) |
| `void* userData` | `u64` (default 0) |
| `const char* name` in defs | `String` |
| B3_ASSERT | `b3_assert!` (→ `debug_assert!`) |
| B3_VALIDATE | `b3_validate!` (→ `debug_assert!`) |
| `b3Alloc/b3Free` | not needed; use Vec/Box |
| arena allocator (`b3AllocateArenaItem`) | `Vec<T>` scratch allocations |
| verstable hash sets/maps (`table.h`) | `std::collections` wrapper in `table.rs` |
| `const b3HullData*` (user-owned) | `Arc<HullData>` |
| `const b3MeshData*` | `Arc<MeshData>` |
| heightfield/compound data | `Arc<...>` |
| structs with trailing variable data + offsets | plain struct with `Vec` fields |
| function-pointer + `void* context` query callbacks | `&mut dyn FnMut(...)` |
| stored callbacks (friction mix etc.) | `Option<fn(...)>` |
| two mutable elements of one array | `get_two_mut(&mut vec, i, j)` from `core.rs`, or copy-out/copy-in for small PODs |

## Public API

The C public API is handle-based with a hidden global world array
(`b3Body_GetPosition(bodyId)`). In the port, `World` is an owned struct and the
world parameter is explicit; everything else keeps the C name:

```rust
let mut world = create_world(&world_def());      // b3CreateWorld
let body_id = create_body(&mut world, &body_def); // b3CreateBody
let p = body_get_position(&world, body_id);       // b3Body_GetPosition
```

Ids (`BodyId`, `ShapeId`, `JointId`, `ContactId`) keep their C layout
(index1/world0/generation) and null convention (`index1 == 0` is null,
so stored index is `index + 1`).

## Float determinism

- Never reorder float expressions; keep temporaries and parenthesization.
- Scalar `a*b + c` shapes in hot paths are contracted with `f32::mul_add`
  (`f64::mul_add` for the double-precision twins) — the port's equivalent of
  clang's default `-ffp-contract=on` on the C build. Canonical patterns:
  `a*b + c` → `a.mul_add(b, c)`; `a*b + c*d` → `a.mul_add(b, c*d)` (one fma
  per add); sum chains fuse left-to-right with the FIRST product plain:
  `x1*x2 + y1*y2 + z1*z2` → `z1.mul_add(z2, y1.mul_add(y2, x1*x2))`.
  `mul_add` is IEEE correctly rounded on every target, so this preserves all
  determinism properties. Do NOT contract: the wide NEON/SSE2 ops in
  contact_solver.rs (C's intrinsics aren't contracted either), the
  deterministic `atan2`/`compute_cos_sin`, or any hashing/serialization code.
- `b3Atan2` / `b3ComputeCosSin` are hand-written approximations — port digit
  for digit.
- `remainderf` has no stable Rust equivalent; `unwind_angle` implements IEEE
  remainder via f64 (see math_functions.rs).
- Use `sqrtf` → `f32::sqrt` (IEEE, deterministic). Do not use other libm
  functions (sin/cos/atan2 from std) inside the engine.

## Layout of a ported module

Keep the C file's function order. Put a header comment naming the source file.
Static file-local helpers stay `fn` (not `pub`). Everything referenced from
other modules is `pub(crate)` or `pub` (public API).
