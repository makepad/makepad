# Credits

- `src/splash_math.rs` (`log_f32`): the nine polynomial coefficients and the ln 2 split (`-2.121_944_4e-4`,
  `0.693_359_4`) of the f32 natural log. They are the ones the Splash kernels use (`platform/script/compute/src/lower.rs`,
  `log`), whose source is the Cephes Math Library by Stephen L. Moshier; see `platform/script/compute/CREDITS.md`.

- `src/splash_math.rs` (`ln`, `atan`): the f64 constants are fdlibm's, the ones of the portable kernels
  (`platform/script/compute/src/lower/portable.rs`, `p_ln` and `p_atan`; see `platform/script/compute/CREDITS.md`):
  `LG`, `LN2_HI`, `LN2_LO` (`ln`) and `ATANHI`, `ATANLO`, `AT` (`atan`). The same values are in the
  vendored libm in `libs/rapier/vendor/libm/src/math/` (`log.rs`, `atan.rs`), marked there "origin: FreeBSD
  /usr/src/lib/msun/src/..." and carrying the notice below.

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
