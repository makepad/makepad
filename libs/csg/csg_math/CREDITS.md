# Credits

- `src/portable.rs`: the constants of the portable sin, cos, tan, atan, exp and ln are fdlibm's (the
  file calls them "fixed fdlibm polynomial kernels"): `INVPIO2`, `PIO2_1`, `PIO2_2`, `PIO2_2T` (the reduction by π/2);
  `S1` to `S6` (`k_sin`) and `C1` to `C6` (`k_cos`); `ATANHI`, `ATANLO`, `AT` (`atan`); `LN2_HI`, `LN2_LO`, `INV_LN2`
  and `P1` to `P5` (`exp`); `LG` (`ln`). The same values are in the vendored libm in
  `libs/rapier/vendor/libm/src/math/` (`rem_pio2.rs`, `k_sin.rs`, `k_cos.rs`, `atan.rs`, `exp.rs`, `log.rs`), each marked
  there "origin: FreeBSD /usr/src/lib/msun/src/..." and carrying the notice below. The vendored `exp.rs` carries it as
  "Copyright (C) 2004".

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
