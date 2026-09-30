# Kernels: fast Splash for per-element work

A kernel is a small function in a typed subset of Splash, compiled ahead of time to native code and run over many elements at once: vertices, instances, particles, glyphs, texels. It reads host buffers, writes its own element's records (a draw shader's instance or vertex struct, in place) and can emit a variable number of records. Everything else in a document stays ordinary Splash. Check every kernel with `kernel_check` before using it.

```splash
let pos = output(vec3)                 // one vec3 per element
let col = output(vec4, 8, 4, inst)     // view: 8-word records of buffer `inst`, words 4..7
let hf  = input(f32)                   // read-only, any element
let amp = param(1.0, 0, 10)            // uniform, set by name, no recompile
fn vertex(i) {
    let x = float(i % 256) * 0.5
    let z = float(i / 256) * 0.5
    let h = fbm2(vec2(x, z) * 0.02, 4, 2.0, 0.5) * amp
    pos[i] = vec3(x, h, z)
    col[i] = vec4(h, h, 1.0, 1.0)
}
```

In a document the same thing is a `Kernel{}` whose fields are these declarations (`pos: output(vec3)`, `vertex: fn(i) {..}`).

## Declarations (top level)
- `input(type [, stride, offset, buffer])`: read-only buffer; reads are clamped to its length.
- `output(type [, stride, offset, buffer])`: written buffer; element `i` owns words `stride*i + offset ..`. Several views of one buffer must share a stride.
- `emit_buffer(type or Layout, n [, buffer])`: up to `n` records per element plus `<name>_count`; `emit(buf, record)` appends one. Records are compacted in element order; more than `n` in one element is an error (no record is lost silently).
- `param(default [, min, max])`: an f32 uniform. Pass integers as floats (exact below 2^24) and convert with `int(p)`.
- `let math = portable` (bit-exact f64 fdlibm maths, a NaN stored as one canonical NaN: replicated or golden-matched work) or `let math = fast` (f32 polynomials, the default: looks).
- `struct S { a: 0.0, n: 0 }` (field defaults), constant tables `let T: int = [1, 2, 3]`, `const N = 8`.
- Types: `f32`/`float`, `i32`/`int`/`u32` (the same 32-bit word), `bool`, `vec2`..`vec4`, `mat4` (column-major, `m * v`), `f64`, structs, host layouts.

## Entries (exactly one)
`vertex(i)`, `instance(i)`, `element(i)`, `primitive(i)`: write records, return nothing. `reduce_sum(i)`, `reduce_min(i)`, `reduce_max(i)`: return an f32 or a vector (up to 16 lanes), combined in element order. Built-ins: `time` (f32), `seed` and `count` (int). Helpers are inlined; recursion is an error.

## Semantics
- Integers wrap; `x / 0` and `x % 0` are 0; `%` has the dividend's sign (`-7 % 2` = -1). `float -> int` truncates and saturates, NaN gives 0.
- `if c { a } elif d { b } else { e }` is also a value. Loops: `for k in a..b`, `while`, `loop`, `break`, `continue`, `return`.
- A loop with a constant bound runs its count. A loop with a run-time bound stops at 1024 iterations, and that is reported (never silently truncated); use chunks or a table for longer.
- Math: `sqrt abs floor ceil round trunc fract sign min max clamp mix step smoothstep sin cos tan atan atan2 asin acos exp exp2 log log2 pow hypot cbrt`, `length dot cross normalize` on vectors, swizzles `v.xz`. `sign(0.0)` is 0.

## Parallel and four-wide
A kernel runs on every worker, four elements per instruction, when it writes only its own element's records (`out[i]`, its views, `emit`). Reading any element of any buffer is fine (neighbours for normals, smoothing, generations). A write elsewhere makes it run on one thread, in element order: correct, slower. f64, reductions and host calls run scalar (still parallel).

## Budgets
- One element's worst case is at most 2M operations (compile error "too much work per element"). A run-time loop counts as 1024 iterations in that estimate; prefer a constant bound with a guard: `for k in 0..8 { if k < n { .. } }`.
- Work from an AI, the store, the LAN or live code is admitted only if one element's worst case (every buffer read counted as a cache miss) stays under 1 ms and the job under 2 s; otherwise it is refused before it runs, with the numbers. A running job is cancelled between elements.

## Traps
- **Literal / literal is float division**, and a constant-bound loop index is a literal: in `for k in 0..4 { k / 2 }` the result is 0.5 for k = 1, and `7 / 2` is 3.5. For integer division make one side an int: `int(k) / 2`, or divide int-typed values (element indices, `int(..)`, `let a: int = ..`). `let d: int = k / 2` is a compile error that points at it.
- `let x = 0` is a float. Integer state is `let x: int = 0`.
- `if c { 1 } else { 0 }` is a float: bit operators refuse it. Write `int(if c { 1 } else { 0 }) | 2`.
- A binary operator cannot start a line (the line ends the expression): break after the operator.
- `hash(x)` and the lattice hashes take integers: pass `int(..)` of floats.
- A record's padding is zeroed; a record you do not write stays zero.

## Stdlib and modules
Kernels see the prelude unqualified. Std modules are reached qualified (`curve.point_at(pts, cum, n, s)`) or imported: `use std.curve.*`, `use std.field as f`. A document's or library's own module is `use lib("id", "rev") as name`; it compiles exactly like the stdlib. A kernel's own function of the same name wins.
