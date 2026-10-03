# Credits

## naga (SPIR-V lowering rules)

`src/shader_spirv.rs` ports a few lowering rules from the SPIR-V backend of
naga 27 (<https://github.com/gfx-rs/wgpu/tree/trunk/naga>), Copyright (c)
2025 The gfx-rs developers, used under the MIT license (naga is dual
MIT/Apache-2.0):

- integer division and remainder that never divide by zero or overflow
  (`back/spv/writer.rs`, `write_wrapped_binary_op`);
- float-to-integer conversion clamped to the target's range
  (`back/spv/block.rs`, `write_as`);
- integer `clamp` as max-then-min, `saturate` as FClamp, `mix` with a
  splatted scalar selector, `abs` of an unsigned value as a copy
  (`back/spv/block.rs`, math functions);
- `Flat` on every integer fragment input, built-ins included, and no
  interpolation decorations on vertex inputs or fragment outputs
  (`back/spv/writer.rs`, `write_varying`);
- uniform and storage buffers wrapped in a `Block` struct, matrix members
  `ColMajor` with a `MatrixStride`, read-only storage `NonWritable`
  (`back/spv/writer.rs`, `write_global_variable`, `decorate_struct_member`).

MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
