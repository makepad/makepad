/*
 * Copyright © 2018-2023, VideoLAN and dav1d authors
 * Copyright © 2018, Two Orioles, LLC
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are met:
 *
 * 1. Redistributions of source code must retain the above copyright notice, this
 *    list of conditions and the following disclaimer.
 *
 * 2. Redistributions in binary form must reproduce the above copyright notice,
 *    this list of conditions and the following disclaimer in the documentation
 *    and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
 * WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
 * DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
 * ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
 * (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
 * LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
 * ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
 * SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

#define decl_itx_fn(name) void(name)(uint8_t* dst, ptrdiff_t dst_stride, int16_t* coeff, int eob)

#define BF(x, suffix) svt_##x##_8bpc_##suffix

#define decl_itx2_fns(w, h, opt)                                \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_dct_##w##x##h, opt)); \
    decl_itx_fn(BF(dav1d_inv_txfm_add_identity_identity_##w##x##h, opt))

#define decl_itx12_fns(w, h, opt)                                         \
    decl_itx2_fns(w, h, opt);                                             \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_adst_##w##x##h, opt));          \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_flipadst_##w##x##h, opt));      \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_identity_##w##x##h, opt));      \
    decl_itx_fn(BF(dav1d_inv_txfm_add_adst_dct_##w##x##h, opt));          \
    decl_itx_fn(BF(dav1d_inv_txfm_add_adst_adst_##w##x##h, opt));         \
    decl_itx_fn(BF(dav1d_inv_txfm_add_adst_flipadst_##w##x##h, opt));     \
    decl_itx_fn(BF(dav1d_inv_txfm_add_flipadst_dct_##w##x##h, opt));      \
    decl_itx_fn(BF(dav1d_inv_txfm_add_flipadst_adst_##w##x##h, opt));     \
    decl_itx_fn(BF(dav1d_inv_txfm_add_flipadst_flipadst_##w##x##h, opt)); \
    decl_itx_fn(BF(dav1d_inv_txfm_add_identity_dct_##w##x##h, opt))

#define decl_itx16_fns(w, h, opt)                                         \
    decl_itx12_fns(w, h, opt);                                            \
    decl_itx_fn(BF(dav1d_inv_txfm_add_adst_identity_##w##x##h, opt));     \
    decl_itx_fn(BF(dav1d_inv_txfm_add_flipadst_identity_##w##x##h, opt)); \
    decl_itx_fn(BF(dav1d_inv_txfm_add_identity_adst_##w##x##h, opt));     \
    decl_itx_fn(BF(dav1d_inv_txfm_add_identity_flipadst_##w##x##h, opt))

#define decl_itx_fns(ext)                                   \
    decl_itx16_fns(4, 4, ext);                              \
    decl_itx16_fns(4, 8, ext);                              \
    decl_itx16_fns(4, 16, ext);                             \
    decl_itx16_fns(8, 4, ext);                              \
    decl_itx16_fns(8, 8, ext);                              \
    decl_itx16_fns(8, 16, ext);                             \
    decl_itx2_fns(8, 32, ext);                              \
    decl_itx16_fns(16, 4, ext);                             \
    decl_itx16_fns(16, 8, ext);                             \
    decl_itx12_fns(16, 16, ext);                            \
    decl_itx2_fns(16, 32, ext);                             \
    decl_itx2_fns(32, 8, ext);                              \
    decl_itx2_fns(32, 16, ext);                             \
    decl_itx2_fns(32, 32, ext);                             \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_dct_16x64, ext)); \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_dct_32x64, ext)); \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_dct_64x16, ext)); \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_dct_64x32, ext)); \
    decl_itx_fn(BF(dav1d_inv_txfm_add_dct_dct_64x64, ext))

decl_itx_fns(neon);

#undef decl_itx_fn
#undef BF
#undef decl_itx2_fns
#undef decl_itx12_fns
#undef decl_itx16_fns
#undef decl_itx_fns
