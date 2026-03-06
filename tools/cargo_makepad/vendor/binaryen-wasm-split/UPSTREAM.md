Binaryen wasm-split reference

Upstream project: https://github.com/WebAssembly/binaryen

Referenced source paths:

- `src/tools/wasm-split/wasm-split.cpp`
- `src/tools/wasm-split/instrumenter.cpp`
- `src/tools/wasm-split/instrumenter.h`
- `src/tools/wasm-split/split-options.cpp`
- `src/tools/wasm-split/split-options.h`
- `src/tools/wasm-opt.cpp`

What Makepad uses from the reference:

- The primary/secondary output model.
- Explicit split configuration rather than always-on behavior.
- Reporting split outputs separately from the main optimization pass.

What Makepad does not vendor from Binaryen:

- The full C++ wasm IR and module-splitting implementation.
- Profile-guided function splitting and placeholder imports.
- Multi-module secondary wasm generation.

Current Makepad split mode is a Rust-native data-section split that emits:

- primary wasm: `<crate>.wasm`
- secondary payload: `<crate>.data.bin`

The browser loader applies the split data payload to wasm memory before
`wasm_create_app()` runs.
