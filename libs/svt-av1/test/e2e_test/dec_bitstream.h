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

#ifndef EbDecBitstream_h
#define EbDecBitstream_h

#ifdef __cplusplus
extern "C" {
#endif  // __cplusplus

// Defines the maximum number of bits in a Bitstream word
#define WORD_SIZE 32

// Twice the WORD_SIZE
#define DBL_WORD_SIZE (2 * (WORD_SIZE))

#define SHL(x, y) (((y) < 32) ? ((x) << (y)) : 0)
#define SHR(x, y) (((y) < 32) ? ((x) >> (y)) : 0)

#define TO_BIG_ENDIAN(x)                                              \
    ((x << 24)) | ((x & 0x0000ff00) << 8) | ((x & 0x00ff0000) >> 8) | \
        ((uint32_t)x >> 24);

static const uint8_t k_leb_128byte_mask = 0x7f;  // Binary: 01111111

/*
 * Bitstream structure
 */
typedef struct {
    /* Bitstream buffer base pointer */
    uint8_t *buf_base;

    /* Bitstream bit offset in current word. Value between 0 and 31 */
    uint32_t bit_ofst;

    /* Current Bitstream buffer pointer */
    uint32_t *buf;

    /* Current word */
    uint32_t cur_word;

    /* Next word */
    uint32_t nxt_word;

    /* Max address for Bitstream */
    uint8_t *buf_max;
} Bitstrm;

// Get m_cnt number of bits and update bffer pointers and offset.
#define GET_BITS(bits, m_pu4_buf, bit_ofst, cur_word, nxt_word, m_cnt) \
    {                                                                  \
        bits = (cur_word << bit_ofst) >> (WORD_SIZE - m_cnt);          \
        bit_ofst += m_cnt;                                             \
        if (bit_ofst > WORD_SIZE) {                                    \
            bits |= SHR(nxt_word, (DBL_WORD_SIZE - bit_ofst));         \
        }                                                              \
                                                                       \
        if (bit_ofst >= WORD_SIZE) {                                   \
            uint32_t pu4_word_tmp;                                     \
            cur_word = nxt_word;                                       \
            /* Getting the next word */                                \
            pu4_word_tmp = *(m_pu4_buf++);                             \
                                                                       \
            bit_ofst -= WORD_SIZE;                                     \
            /* Swapping little endian to big endian conversion*/       \
            nxt_word = TO_BIG_ENDIAN(pu4_word_tmp);                    \
        }                                                              \
    }

void svt_aom_dec_bits_init(Bitstrm *bs, const uint8_t *data,
                           size_t u4_numbytes);

uint32_t svt_aom_dec_get_bits_uvlc(Bitstrm *bs);
uint32_t svt_aom_dec_get_bits(Bitstrm *bs, uint32_t numbits);
void svt_aom_dec_get_bits_leb128(Bitstrm *bs, size_t available, size_t *value,
                                 size_t *length);
uint32_t svt_aom_dec_get_bits_ns(Bitstrm *bs, uint32_t n);
int32_t svt_aom_dec_get_bits_su(Bitstrm *bs, uint32_t n);
uint32_t svt_aom_dec_get_bits_le(Bitstrm *bs, uint32_t n);

uint32_t svt_aom_get_position(Bitstrm *bs);
uint8_t *svt_aom_get_bitsteam_buf(Bitstrm *bs);

#ifdef __cplusplus
}
#endif  // __cplusplus
#endif  // EbDecBitstream_h
