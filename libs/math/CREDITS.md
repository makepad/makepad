# Credits

- `src/deterministic.rs`: the constants of the deterministic f32 sin, cos, tan, atan, exp and ln are
  fdlibm's (the file says its kernels "follow fdlibm/musl"): `INVPIO2`, `PIO2_1`, `PIO2_1T` (the reduction by π/2);
  `S1` to `S4` (`k_sin`) and `C0` to `C3` (`k_cos`), the values of the single-precision kernels; `ATANHI`, `ATANLO`, `AT`
  (`atan_d`); `LN2_HI`, `LN2_LO`, `INV_LN2` (`exp_d`); `LG` (`ln_d`). The same values are in the vendored
  libm in `libs/rapier/vendor/libm/src/math/` (`rem_pio2f.rs`, `k_sinf.rs`, `k_cosf.rs`, `atan.rs`, `exp.rs`,
  `log.rs`), each marked there "origin: FreeBSD /usr/src/lib/msun/src/..." and carrying the notice below. The vendored
  `exp.rs` carries it as "Copyright (C) 2004".

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
