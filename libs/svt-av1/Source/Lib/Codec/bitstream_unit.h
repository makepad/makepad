/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbBitstreamUnit_h
#define EbBitstreamUnit_h

#include "object.h"
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#ifdef _MSC_VER
#if defined(_M_X64) || defined(_M_IX86)
#include <intrin.h>
#define USE_MSC_INTRINSICS
#endif
#endif

/**********************************
 * Bitstream Unit Types
 **********************************/
typedef struct OutputBitstreamUnit {
    EbDctor  dctor;
    uint32_t size; // allocated buffer size
    uint8_t* buffer_begin_av1; // the byte buffer
    uint8_t* buffer_av1; // the byte buffer
} OutputBitstreamUnit;

/**********************************
     * Extern Function Declarations
     **********************************/
EbErrorType svt_aom_output_bitstream_unit_ctor(OutputBitstreamUnit* bitstream_ptr, uint32_t buffer_size);

EbErrorType svt_aom_output_bitstream_reset(OutputBitstreamUnit* bitstream_ptr);

/********************************************************************************************************************************/
/********************************************************************************************************************************/
/********************************************************************************************************************************/
#include "cabac_context_model.h"
/********************************************************************************************************************************/
// bitops.h
// These versions of get_msb() are only valid when n != 0 because all
// of the optimized versions are undefined when n == 0:
// https://gcc.gnu.org/onlinedocs/gcc/Other-Builtins.html

// use GNU builtins where available.
#if defined(__GNUC__) && ((__GNUC__ == 3 && __GNUC_MINOR__ >= 4) || __GNUC__ >= 4)
static INLINE int32_t get_msb(uint32_t n) {
    assert(n != 0);
    return 31 - __builtin_clz(n);
}
#elif defined(USE_MSC_INTRINSICS)
#pragma intrinsic(_BitScanReverse)

static INLINE int32_t get_msb(uint32_t n) {
    unsigned long first_set_bit;
    assert(n != 0);
    _BitScanReverse(&first_set_bit, n);
    return first_set_bit;
}

#undef USE_MSC_INTRINSICS
#else
// Returns (int32_t)floor(log2(n)). n must be > 0.
/*static*/ INLINE int32_t get_msb(uint32_t n) {
    int32_t  log   = 0;
    uint32_t value = n;
    int32_t  i;

    assert(n != 0);

    for (i = 4; i >= 0; --i) {
        const int32_t  shift = (1 << i);
        const uint32_t x     = value >> shift;
        if (x != 0) {
            value = x;
            log += shift;
        }
    }
    return log;
}
#endif
/********************************************************************************************************************************/
//odintrin.h

#define OD_CLZ0 (1)
#define OD_CLZ(x) (-get_msb(x))
#define OD_ILOG_NZ(x) (OD_CLZ0 - OD_CLZ(x))

#define OD_DIVU_DMAX (1024)

extern uint32_t svt_aom_od_divu_small_consts[OD_DIVU_DMAX][2];

/*Translate unsigned division by small divisors into multiplications.*/
#define OD_DIVU_SMALL(_x, _d)                                                 \
    ((uint32_t)((svt_aom_od_divu_small_consts[(_d) - 1][0] * (uint64_t)(_x) + \
                 svt_aom_od_divu_small_consts[(_d) - 1][1]) >>                \
                32) >>                                                        \
     (OD_ILOG_NZ(_d) - 1))

#define OD_DIVU(_x, _d) (((_d) < OD_DIVU_DMAX) ? (OD_DIVU_SMALL((_x), (_d))) : ((_x) / (_d)))

/*Enable special features for gcc and compatible compilers.*/
#if defined(__GNUC__) && defined(__GNUC_MINOR__) && defined(__GNUC_PATCHLEVEL__)
#define OD_GNUC_PREREQ(maj, min, pat) \
    ((__GNUC__ << 16) + (__GNUC_MINOR__ << 8) + __GNUC_PATCHLEVEL__ >= ((maj) << 16) + ((min) << 8) + pat) // NOLINT
#else
#define OD_GNUC_PREREQ(maj, min, pat) (0)
#endif

#if OD_GNUC_PREREQ(3, 4, 0)
#define OD_WARN_UNUSED_RESULT __attribute__((__warn_unused_result__))
#else
#define OD_WARN_UNUSED_RESULT
#endif

#if OD_GNUC_PREREQ(3, 4, 0)
#define OD_ARG_NONNULL(x) __attribute__((__nonnull__(x)))
#else
#define OD_ARG_NONNULL(x)
#endif

/** Copy n elements of memory from src to dst. The 0* term provides
compile-time type checking  */
#if !defined(OVERRIDE_OD_COPY)
#define OD_COPY(dst, src, n) (svt_memcpy((dst), (src), sizeof(*(dst)) * (n) + 0 * ((dst) - (src))))
#endif

/********************************************************************************************************************************/
//entcode.h
#define EC_PROB_SHIFT 6
#define EC_MIN_PROB 4 // must be <= (1<<EC_PROB_SHIFT)/16

/*The resolution of fractional-precision bit usage measurements, i.e.,
    3 => 1/8th bits.*/
#define OD_BITRES (3)

#define OD_ICDF AOM_ICDF

/********************************************************************************************************************************/
//entenc.h
typedef uint64_t OdEcWindow;
#define OD_EC_WINDOW_SIZE ((int32_t)sizeof(OdEcWindow) * CHAR_BIT)
#define OD_MEASURE_EC_OVERHEAD (0)

/*The entropy encoder context.*/
typedef struct OdEcEnc {
    /*Buffered output.
      This contains only the raw bits until the final call to od_ec_enc_done(),
       where all the arithmetic-coded data gets prepended to it.*/
    unsigned char* buf;
    /*The size of the buffer.*/
    uint32_t storage;
    /*The offset at which the next entropy-coded byte will be written.*/
    uint32_t offs;
    /*The low end of the current range.*/
    OdEcWindow low;
    /*The number of values in the current range.*/
    uint16_t rng;
    /*The number of bits of data in the current value.*/
    int16_t cnt;
    /*Nonzero if an error occurred.*/
    int error;
#if OD_MEASURE_EC_OVERHEAD
    double entropy;
    int    nb_symbols;
#endif
} OdEcEnc;

/*See entenc.c for further documentation.*/
void svt_od_ec_enc_init(OdEcEnc* enc, uint32_t size) OD_ARG_NONNULL(1);
void svt_od_ec_enc_reset(OdEcEnc* enc) OD_ARG_NONNULL(1);
void svt_od_ec_enc_clear(OdEcEnc* enc) OD_ARG_NONNULL(1);
void svt_od_ec_encode_bool_q15(OdEcEnc* enc, int32_t val, unsigned f_q15) OD_ARG_NONNULL(1);
void svt_od_ec_encode_cdf_q15(OdEcEnc* enc, int32_t s, const uint16_t* cdf, int32_t nsyms) OD_ARG_NONNULL(1)
    OD_ARG_NONNULL(3);
OD_WARN_UNUSED_RESULT uint8_t* svt_od_ec_enc_done(OdEcEnc* enc, uint32_t* nbytes) OD_ARG_NONNULL(1) OD_ARG_NONNULL(2);
OD_WARN_UNUSED_RESULT int32_t  svt_od_ec_enc_tell(const OdEcEnc* enc) OD_ARG_NONNULL(1);
OD_WARN_UNUSED_RESULT uint32_t svt_od_ec_enc_tell_frac(const OdEcEnc* enc) OD_ARG_NONNULL(1);

/************* endian_inl.h ********************************/
#if defined(__GNUC__)
#define LOCAL_GCC_VERSION ((__GNUC__ << 8) | __GNUC_MINOR__)
#define LOCAL_GCC_PREREQ(maj, min) (LOCAL_GCC_VERSION >= (((maj) << 8) | (min)))
#else
#define LOCAL_GCC_VERSION 0
#define LOCAL_GCC_PREREQ(maj, min) 0
#endif

// handle clang compatibility
#ifndef __has_builtin
#define __has_builtin(x) 0
#endif

// some endian fix (e.g.: mips-gcc doesn't define __BIG_ENDIAN__)
#if !defined(WORDS_BIGENDIAN) &&                   \
    (defined(__BIG_ENDIAN__) || defined(_M_PPC) || \
     (defined(__BYTE_ORDER__) && (__BYTE_ORDER__ == __ORDER_BIG_ENDIAN__)))
#define WORDS_BIGENDIAN
#endif

#if defined(WORDS_BIGENDIAN)
#define HToLE32 BSwap32
#define HToLE16 BSwap16
#define HToBE64(x) (x)
#define HToBE32(x) (x)
#else
#define HToLE32(x) (x)
#define HToLE16(x) (x)
#define HToBE64(X) BSwap64(X)
#define HToBE32(X) BSwap32(X)
#endif

#if LOCAL_GCC_PREREQ(4, 8) || __has_builtin(__builtin_bswap16)
#define HAVE_BUILTIN_BSWAP16
#endif

#if LOCAL_GCC_PREREQ(4, 3) || __has_builtin(__builtin_bswap32)
#define HAVE_BUILTIN_BSWAP32
#endif

#if LOCAL_GCC_PREREQ(4, 3) || __has_builtin(__builtin_bswap64)
#define HAVE_BUILTIN_BSWAP64
#endif

static inline uint16_t BSwap16(uint16_t x) {
#if defined(HAVE_BUILTIN_BSWAP16)
    return __builtin_bswap16(x);
#elif defined(_MSC_VER)
    return _byteswap_ushort(x);
#else
    // gcc will recognize a 'rorw $8, ...' here:
    return (x >> 8) | ((x & 0xff) << 8);
#endif // HAVE_BUILTIN_BSWAP16
}

static inline uint32_t BSwap32(uint32_t x) {
#if defined(HAVE_BUILTIN_BSWAP32)
    return __builtin_bswap32(x);
#elif defined(__i386__) || defined(__x86_64__)
    uint32_t swapped_bytes;
    __asm__ volatile("bswap %0" : "=r"(swapped_bytes) : "0"(x));
    return swapped_bytes;
#elif defined(_MSC_VER)
    return (uint32_t)_byteswap_ulong(x);
#else
    return (x >> 24) | ((x >> 8) & 0xff00) | ((x << 8) & 0xff0000) | (x << 24);
#endif // HAVE_BUILTIN_BSWAP32
}

static inline uint64_t BSwap64(uint64_t x) {
#if defined(HAVE_BUILTIN_BSWAP64)
    return __builtin_bswap64(x);
#elif defined(__x86_64__)
    uint64_t swapped_bytes;
    __asm__ volatile("bswapq %0" : "=r"(swapped_bytes) : "0"(x));
    return swapped_bytes;
#elif defined(_MSC_VER)
    return (uint64_t)_byteswap_uint64(x);
#else // generic code for swapping 64-bit values (suggested by bdb@)
    x = ((x & 0xffffffff00000000ull) >> 32) | ((x & 0x00000000ffffffffull) << 32);
    x = ((x & 0xffff0000ffff0000ull) >> 16) | ((x & 0x0000ffff0000ffffull) << 16);
    x = ((x & 0xff00ff00ff00ff00ull) >> 8) | ((x & 0x00ff00ff00ff00ffull) << 8);
    return x;
#endif // HAVE_BUILTIN_BSWAP64
}

// buf is the frame bitbuffer, offs is where carry to be added
static inline void propagate_carry_bwd(unsigned char* buf, uint32_t offs) {
    uint16_t carry = 1;
    do {
        uint16_t sum = (uint16_t)buf[offs] + 1;
        buf[offs--]  = (unsigned char)sum;
        carry        = sum >> 8;
    } while (carry);
}

// Convert to big-endian byte order and write data to buffer adding the
// carry-bit
static inline void write_enc_data_to_out_buf(unsigned char* out, uint32_t offs, uint64_t output, uint64_t carry,
                                             uint32_t* enc_offs, uint8_t num_bytes_ready) {
    const uint64_t reg = HToBE64(output << ((8 - num_bytes_ready) << 3));
    memcpy(&out[offs], &reg, 8);
    // Propagate carry backwards if exists
    if (carry) {
        assert(offs > 0);
        propagate_carry_bwd(out, offs - 1);
    }
    *enc_offs = offs + num_bytes_ready;
}

/********************************************************************************************************************************/
//bitwriter.h
typedef struct AomWriter {
    unsigned int pos;
    uint8_t*     buffer;
    uint32_t     buffer_size;
    // save a pointer to the container holding the buffer, in case the buffer must be resized
    OutputBitstreamUnit* buffer_parent;
    OdEcEnc              ec;
    uint8_t              allow_update_cdf;
} AomWriter;

static INLINE void aom_start_encode(AomWriter* br, OutputBitstreamUnit* source) {
    br->buffer        = source->buffer_av1;
    br->buffer_size   = source->size;
    br->buffer_parent = source;
    br->pos           = 0;
    svt_od_ec_enc_init(&br->ec, 62025);
}

EbErrorType svt_realloc_output_bitstream_unit(OutputBitstreamUnit* output_bitstream_ptr, uint32_t sz);

static INLINE int32_t aom_stop_encode(AomWriter* w) {
    uint32_t bytes = 0;
    uint8_t* data  = svt_od_ec_enc_done(&w->ec, &bytes);
    if (!data) {
        svt_od_ec_enc_clear(&w->ec);
        return -1;
    }
    int32_t nb_bits = svt_od_ec_enc_tell(&w->ec);
    // If buffer is smaller than data, increase buffer size
    if (w->buffer_size < bytes) {
        svt_realloc_output_bitstream_unit(w->buffer_parent,
                                          bytes + 1); // plus one for good measure
        w->buffer      = w->buffer_parent->buffer_av1;
        w->buffer_size = bytes + 1;
    }
    if (svt_memcpy != NULL) {
        svt_memcpy(w->buffer, data, bytes);
    } else {
        svt_memcpy_c(w->buffer, data, bytes);
    }

    w->pos = bytes;
    svt_od_ec_enc_clear(&w->ec);
    return nb_bits;
}

static INLINE void aom_write(AomWriter* w, int bit, int prob) {
    int p = (0x7FFFFF - (prob << 15) + prob) >> 8;
#if CONFIG_BITSTREAM_DEBUG
    AomCdfProb cdf[2] = {(AomCdfProb)p, 32767};
    bitstream_queue_push(bit, cdf, 2);
#endif
    svt_od_ec_encode_bool_q15(&w->ec, bit, p);
}

static INLINE void aom_write_bit(AomWriter* w, int bit) {
    aom_write(w, bit, 128); // aom_prob_half
}

static INLINE void aom_write_literal(AomWriter* w, unsigned data, int bits) {
    for (int bit = bits - 1; bit >= 0; bit--) {
        aom_write_bit(w, 1 & (data >> bit));
    }
}

static INLINE void aom_write_cdf(AomWriter* w, int symb, const AomCdfProb* cdf, int nsymbs) {
#if CONFIG_BITSTREAM_DEBUG
    bitstream_queue_push(symb, cdf, nsymbs);
#endif
    svt_od_ec_encode_cdf_q15(&w->ec, symb, cdf, nsymbs);
}

static INLINE void aom_write_symbol(AomWriter* w, int symb, AomCdfProb* cdf, int nsymbs) {
    aom_write_cdf(w, symb, cdf, nsymbs);
    if (w->allow_update_cdf) {
        update_cdf(cdf, symb, nsymbs);
    }
}

/********************************************************************************************************************************/
/********************************************************************************************************************************/
#ifdef __cplusplus
}
#endif

#endif // EbBitstreamUnit_h
