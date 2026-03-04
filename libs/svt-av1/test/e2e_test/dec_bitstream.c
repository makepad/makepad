/*
 * Copyright(c) 2019 Netflix, Inc.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

#include "definitions.h"
#include "bitstream_unit.h"
#include "dec_bitstream.h"

/* Function used for Bitstream structure initialization
Assumes data is aligned to 4 bytes. If not aligned  then all Bitstream
accesses will be unaligned and hence  costlier. Since this is codec memory that
holds emulation prevented data, assumption of aligned to 4 bytes is valid */
void svt_aom_dec_bits_init(Bitstrm *bs, const uint8_t *data, size_t numbytes) {
    uint32_t cur_word;
    uint32_t nxt_word;
    uint32_t temp;
    uint32_t *buf;
    buf = (uint32_t *)data;
    temp = *buf++;
    cur_word = TO_BIG_ENDIAN(temp);
    temp = *buf++;
    nxt_word = TO_BIG_ENDIAN(temp);
    bs->bit_ofst = 0;
    bs->buf_base = (uint8_t *)data;
    bs->buf = buf;
    bs->cur_word = cur_word;
    bs->nxt_word = nxt_word;
    bs->buf_max = (uint8_t *)data + numbytes + 8;
    return;
}

/* Reads next numbits number of bits from the Bitstream  this updates the
Bitstream offset and consumes the bits. Section: 4.10.2 -> f(n) */
uint32_t svt_aom_dec_get_bits(Bitstrm *bs, uint32_t numbits) {
    uint32_t bits_read;
    if (0 == numbits)
        return 0;
    GET_BITS(
        bits_read, bs->buf, bs->bit_ofst, bs->cur_word, bs->nxt_word, numbits);
    return bits_read;
}

/* Get unsigned integer represented by a variable number of little-endian bytes
 */
void svt_aom_dec_get_bits_leb128(Bitstrm *bs, size_t available, size_t *value,
                                 size_t *length) {
    (void)available;
    *value = 0;
    *length = 0;

    for (int i = 0; i < 8; i++) {
        uint32_t leb128_byte = svt_aom_dec_get_bits(bs, 8);
        *value |= (((uint64_t)leb128_byte & 0x7f) << (i * 7));
        *length += 1;
        if (!(leb128_byte & 0x80))
            break;
    }
}

/* Get variable length unsigned n-bit number appearing directly in the Bitstream
 */
uint32_t svt_aom_dec_get_bits_uvlc(Bitstrm *bs) {
    int leading_zeros = 0;
    while (leading_zeros < 32 && !svt_aom_dec_get_bits(bs, 1))
        ++leading_zeros;
    // Maximum 32 bits.
    if (leading_zeros == 32)
        return UINT32_MAX;
    const uint32_t base = (1u << leading_zeros) - 1;
    const uint32_t value = svt_aom_dec_get_bits(bs, leading_zeros);
    return base + value;
}

/* Unsigned encoded integer with maximum number of values n */
uint32_t svt_aom_dec_get_bits_ns(Bitstrm *bs, uint32_t n) {
    if (n <= 1)
        return 0;
    int w = get_msb(n) + 1;  // w = FloorLog2(n) + 1
    int m = (1 << w) - n;
    int v = svt_aom_dec_get_bits(bs, w - 1);
    if (v < m)
        return v;
    return (v << 1) - m + svt_aom_dec_get_bits(bs, 1);
}

/* Signed integer converted from an n bits unsigned integer in the Bitstream */
int32_t svt_aom_dec_get_bits_su(Bitstrm *bs, uint32_t n) {
    int value = svt_aom_dec_get_bits(bs, n);
    int sign_mask = 1 << (n - 1);
    if (value & sign_mask)
        value = value - 2 * sign_mask;
    return value;
}

/* Unsigned little-endian n-byte number appearing directly in the Bitstream */
uint32_t svt_aom_dec_get_bits_le(Bitstrm *bs, uint32_t n) {
    uint32_t t = 0;
    for (uint32_t i = 0; i < n; i++)
        t += svt_aom_dec_get_bits(bs, 8) << (i * 8);
    return t;
}

uint32_t svt_aom_get_position(Bitstrm *bs) {
    return (uint32_t)(((((uint8_t *)bs->buf) - bs->buf_base) * 8) -
                      WORD_SIZE /*nxt_word*/
                      - (WORD_SIZE - bs->bit_ofst) /*cur_word*/);
}

uint8_t *svt_aom_get_bitsteam_buf(Bitstrm *bs) {
    uint8_t *bitsteam_buf = (uint8_t *)bs->buf;
    bitsteam_buf -= ((WORD_SIZE /*nxt_word*/ >> 3) +
                     ((WORD_SIZE - bs->bit_ofst) /*cur_word*/ >> 3));

    assert(bitsteam_buf == (bs->buf_base + (svt_aom_get_position(bs) >> 3)));

    return bitsteam_buf;
}
