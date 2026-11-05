# Credits

- `src/lower.rs` (`sincos`, `exp`, `log`, `tanh`, `atan`) and its copies in `examples/bench_codegen.rs` (the scalar and NEON `sincos` and
  `exp`): the coefficients and reduction constants of the f32 sin, cos, exp, log, tanh and atan that kernels run. The source is the
  Cephes Math Library by Stephen L. Moshier; the code comments call them "Cephes-style" (`src/lower.rs`,
  `src/lower/kernel.rs`, `src/kernel_prelude.splash`). What comes from it:
  - `sincos`: the three-part π/4 reduction constants (`FOPI`, `DP1`, `DP2`, `DP3`), the three sin polynomial
    coefficients and the three cos polynomial coefficients.
  - `exp`: the ln 2 split used for the reduction (`0.693_359_4`, `-2.121_944_4e-4`) and the six polynomial
    coefficients.
  - `log`: the nine polynomial coefficients and the same ln 2 split.
  - `tanh` (the small-argument polynomial) and `atan` (the polynomial): in the same section, which the comment at
    `src/lower.rs:583` calls Cephes-style.

  The values are written to f32 precision. For sin, cos, log and atan, and the ln 2 split, the same numbers at full precision are in
  `libs/rapier/vendor/wide/src/f32x8_.rs`.

- `src/lower/portable.rs` (`math: portable`): the f64 constants of the portable kernels are fdlibm's, the same as in
  `libs/csg/csg_math/src/portable.rs`, whose results these kernels reproduce bit for bit: `INVPIO2`, `PIO2_1`,
  `PIO2_2`, `PIO2_2T` (`reduce_d`); `S1` to `S6` (`k_sin`) and `C1` to `C6` (`k_cos`); `ATANHI`, `ATANLO`, `AT`
  (`p_atan`); `P1` to `P5`, `LN2_HI`, `LN2_LO`, `INV_LN2` (`p_exp`); `LG`, `LN2_HI`, `LN2_LO` (`p_ln`). The same values
  are in the vendored libm in `libs/rapier/vendor/libm/src/math/` (`rem_pio2.rs`, `k_sin.rs`, `k_cos.rs`,
  `atan.rs`, `exp.rs`, `log.rs`), each marked there "origin: FreeBSD /usr/src/lib/msun/src/..." and carrying the notice
  below. The vendored `exp.rs` carries it as "Copyright (C) 2004". `src/lower.rs` and `src/lower/kernel.rs` only mention
  fdlibm in comments.

  The notice the fdlibm sources carry, copied from `libs/rapier/vendor/libm/src/math/log.rs`:

  ```text
  /*
   * ====================================================
   * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
   *
   * Developed at SunSoft, a Sun Microsystems, Inc. business.
   * Permission to use, copy, modify, and distribute this
   * software is freely granted, provided that this notice
   * is preserved.
   * ====================================================
   */
  ```
