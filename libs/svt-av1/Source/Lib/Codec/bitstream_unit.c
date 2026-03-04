/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <stdlib.h>

#include "bitstream_unit.h"
#include "definitions.h"
#include "utility.h"
#include "svt_log.h"

#if OD_MEASURE_EC_OVERHEAD
#include <stdio.h>
#endif

static void output_bitstream_unit_dctor(EbPtr p) {
    OutputBitstreamUnit* obj = (OutputBitstreamUnit*)p;
    EB_FREE_ARRAY(obj->buffer_begin_av1);
}

/**********************************
 * Constructor
 **********************************/
EbErrorType svt_aom_output_bitstream_unit_ctor(OutputBitstreamUnit* bitstream_ptr, uint32_t buffer_size) {
    bitstream_ptr->dctor = output_bitstream_unit_dctor;
    if (buffer_size) {
        bitstream_ptr->size = buffer_size;
        EB_MALLOC_ARRAY(bitstream_ptr->buffer_begin_av1, bitstream_ptr->size);
        bitstream_ptr->buffer_av1 = bitstream_ptr->buffer_begin_av1;
    } else {
        bitstream_ptr->size             = 0;
        bitstream_ptr->buffer_begin_av1 = 0;
        bitstream_ptr->buffer_av1       = 0;
    }

    return EB_ErrorNone;
}

/**********************************
 * Reset Bitstream
 **********************************/
EbErrorType svt_aom_output_bitstream_reset(OutputBitstreamUnit* bitstream_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // Reset the write ptr to the beginning of the buffer
    bitstream_ptr->buffer_av1 = bitstream_ptr->buffer_begin_av1;

    return return_error;
}

/********************************************************************************************************************************/
/********************************************************************************************************************************/
/********************************************************************************************************************************/
/********************************************************************************************************************************/
/* Realloc when bitstream pointer size is not enough to write data of size sz */
EbErrorType svt_realloc_output_bitstream_unit(OutputBitstreamUnit* output_bitstream_ptr, uint32_t sz) {
    if (output_bitstream_ptr && sz > 0) {
        // Must add offset to realloc'd buffer to save any previously written bits
        uint64_t offset = output_bitstream_ptr->buffer_av1 - output_bitstream_ptr->buffer_begin_av1;
        assert(output_bitstream_ptr->buffer_av1 >= output_bitstream_ptr->buffer_begin_av1);
        output_bitstream_ptr->size = sz;
        EB_REALLOC_ARRAY(output_bitstream_ptr->buffer_begin_av1, output_bitstream_ptr->size);
        output_bitstream_ptr->buffer_av1 = output_bitstream_ptr->buffer_begin_av1 + offset;
    }
    return EB_ErrorNone;
}

/*A range encoder.
  See entdec.c and the references for implementation details \cite{Mar79,MNW98}.

  @INPROCEEDINGS{Mar79,
   author="Martin, G.N.N.",
   title="Range encoding: an algorithm for removing redundancy from a digitised
    message",
   booktitle="Video \& Data Recording Conference",
   year=1979,
   address="Southampton",
   month=Jul,
   URL="http://www.compressconsult.com/rangecoder/rngcod.pdf.gz"
  }
  @ARTICLE{MNW98,
   author="Alistair Moffat and Radford Neal and Ian H. Witten",
   title="Arithmetic Coding Revisited",
   journal="{ACM} Transactions on Information Systems",
   year=1998,
   volume=16,
   number=3,
   pages="256--294",
   month=Jul,
   URL="http://researchcommons.waikato.ac.nz/bitstream/handle/10289/78/content.pdf"
  }*/

/*Takes updated low and range values, renormalizes them so that
     32768 <= rng < 65536 (flushing bytes from low to the output buffer if
     necessary), and stores them back in the encoder context.
    low: The new value of low.
    rng: The new value of the range.*/
static void svt_od_ec_enc_normalize(OdEcEnc* enc, OdEcWindow low, unsigned rng) {
    int d;
    int c;
    int s;
    if (enc->error) {
        return;
    }
    c = enc->cnt;
    assert(rng <= 65535U);
    /*The number of leading zeros in the 16-bit binary representation of rng.*/
    d = 16 - OD_ILOG_NZ(rng);
    s = c + d;

    /* We flush every time "low" cannot safely and efficiently accommodate any
       more data. Overall, c must not exceed 63 at the time of byte flush out. To
       facilitate this, "s" cannot exceed 56-bits because we have to keep 1 byte
       for carry. Also, we need to subtract 16 because we want to keep room for
       the next symbol worth "d"-bits (max 15). An alternate condition would be if
       (e < d), where e = number of leading zeros in "low", indicating there is
       not enough rooom to accommodate "rng" worth of "d"-bits in "low". However,
       this approach needs additional computations: (i) compute "e", (ii) push
       the leading 0x00's as a special case.
    */
    if (s >= 40) { // 56 - 16
        unsigned char* out     = enc->buf;
        uint32_t       storage = enc->storage;
        uint32_t       offs    = enc->offs;
        if (offs + 8 > storage) {
            storage = 2 * storage + 8;
            EB_REALLOC_ARRAY_NO_CHECK(out, storage);
            if (out == NULL) {
                enc->error = -1;
                return;
            }
            enc->buf     = out;
            enc->storage = storage;
        }
        // Need to add 1 byte here since enc->cnt always counts 1 byte less
        // (enc->cnt = -9) to ensure correct operation
        uint8_t num_bytes_ready = (s >> 3) + 1;

        // Update "c" to contain the number of non-ready bits in "low". Since "low"
        // has 64-bit capacity, we need to add the (64 - 40) cushion bits and take
        // off the number of ready bits.
        c += 24 - (num_bytes_ready << 3);

        // Prepare "output" and update "low"
        uint64_t output = low >> c;
        low             = low & (((uint64_t)1 << c) - 1);

        // Prepare data and carry mask
        uint64_t mask  = (uint64_t)1 << (num_bytes_ready << 3);
        uint64_t carry = output & mask;

        mask   = mask - 0x01;
        output = output & mask;

        // Write data in a single operation
        write_enc_data_to_out_buf(out, offs, output, carry, &enc->offs, num_bytes_ready);

        // Update state of the encoder: enc->cnt to contain the number of residual
        // bits
        s = c + d - 24;
    }
    enc->low = low << d;
    enc->rng = rng << d;
    enc->cnt = s;
}

/*Initializes the encoder.
  size: The initial size of the buffer, in bytes.*/
void svt_od_ec_enc_init(OdEcEnc* enc, uint32_t size) {
    svt_od_ec_enc_reset(enc);
    EB_MALLOC_ARRAY_NO_CHECK(enc->buf, size);
    enc->storage = size;
    if (size > 0 && enc->buf == NULL) {
        enc->storage = 0;
        enc->error   = -1;
    }
}

/*Reinitializes the encoder.*/
void svt_od_ec_enc_reset(OdEcEnc* enc) {
    enc->offs = 0;
    enc->low  = 0;
    enc->rng  = 0x8000;
    /*This is initialized to -9 so that it crosses zero after we've accumulated
       one byte + one carry bit.*/
    enc->cnt   = -9;
    enc->error = 0;
#if OD_MEASURE_EC_OVERHEAD
    enc->entropy    = 0;
    enc->nb_symbols = 0;
#endif
}

/*Frees the buffers used by the encoder.*/
void svt_od_ec_enc_clear(OdEcEnc* enc) {
    EB_FREE_ARRAY(enc->buf);
}

/*Encodes a symbol given its frequency in Q15.
  fl: CDF_PROB_TOP minus the cumulative frequency of all symbols that come
  before the one to be encoded.
  fh: CDF_PROB_TOP minus the cumulative frequency of all symbols up to and
  including the one to be encoded.*/
static void svt_od_ec_encode_q15(OdEcEnc* enc, unsigned fl, unsigned fh, int s, int nsyms) {
    OdEcWindow l;
    unsigned   r;
    l = enc->low;
    r = enc->rng;
    assert(32768U <= r);
    assert(fh <= fl);
    assert(fl <= 32768U);
    assert(7 - EC_PROB_SHIFT >= 0);
    const int N = nsyms - 1;
    if (fl < CDF_PROB_TOP) {
        unsigned u = ((r >> 8) * (uint32_t)(fl >> EC_PROB_SHIFT) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB * (N - (s - 1));
        unsigned v = ((r >> 8) * (uint32_t)(fh >> EC_PROB_SHIFT) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB * (N - (s + 0));
        l += r - u;
        r = u - v;
    } else {
        r -= ((r >> 8) * (uint32_t)(fh >> EC_PROB_SHIFT) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB * (N - (s + 0));
    }
    svt_od_ec_enc_normalize(enc, l, r);
#if OD_MEASURE_EC_OVERHEAD
    enc->entropy -= OD_LOG2((double)(OD_ICDF(fh) - OD_ICDF(fl)) / CDF_PROB_TOP.);
    enc->nb_symbols++;
#endif
}

/*Encode a single binary value.
  val: The value to encode (0 or 1).
  f: The probability that the val is one, scaled by 32768.*/
void svt_od_ec_encode_bool_q15(OdEcEnc* enc, int val, unsigned f) {
    OdEcWindow l;
    unsigned   r;
    unsigned   v;
    assert(0 < f);
    assert(f < 32768U);
    l = enc->low;
    r = enc->rng;
    assert(32768U <= r);
    v = ((r >> 8) * (uint32_t)(f >> EC_PROB_SHIFT) >> (7 - EC_PROB_SHIFT));
    v += EC_MIN_PROB;
    if (val) {
        l += r - v;
    }
    r = val ? v : r - v;
    svt_od_ec_enc_normalize(enc, l, r);
#if OD_MEASURE_EC_OVERHEAD
    enc->entropy -= OD_LOG2((double)(val ? f : (32768 - f)) / 32768.);
    enc->nb_symbols++;
#endif
}

/*Encodes a symbol given a cumulative distribution function (CDF) table in Q15.
  s: The index of the symbol to encode.
  icdf: 32768 minus the CDF, such that symbol s falls in the range
         [s > 0 ? (32768 - icdf[s - 1]) : 0, 32768 - icdf[s]).
        The values must be monotonically decreasing, and icdf[nsyms - 1] must
         be 0.
  nsyms: The number of symbols in the alphabet.
         This should be at most 16.*/
void svt_od_ec_encode_cdf_q15(OdEcEnc* enc, int s, const uint16_t* icdf, int nsyms) {
    (void)nsyms;
    assert(s >= 0);
    assert(s < nsyms);
    assert(icdf[nsyms - 1] == OD_ICDF(CDF_PROB_TOP));
    svt_od_ec_encode_q15(enc, s > 0 ? icdf[s - 1] : OD_ICDF(0), icdf[s], s, nsyms);
}

/*Indicates that there are no more symbols to encode.
  All remaining output bytes are flushed to the output buffer.
  od_ec_enc_reset() should be called before using the encoder again.
  bytes: Returns the size of the encoded data in the returned buffer.
  Return: A pointer to the start of the final buffer, or NULL if there was an
           encoding error.*/
unsigned char* svt_od_ec_enc_done(OdEcEnc* enc, uint32_t* nbytes) {
    unsigned char* out;
    uint32_t       storage;
    uint32_t       offs;
    OdEcWindow     m;
    OdEcWindow     e;
    OdEcWindow     l;
    int            c;
    int            s;
    if (enc->error) {
        return NULL;
    }
#if OD_MEASURE_EC_OVERHEAD
    {
        uint32_t tell;
        /* Don't count the 1 bit we lose to raw bits as overhead. */
        tell = od_ec_enc_tell(enc) - 1;
        fprintf(stderr, "overhead: %f%%\n", 100 * (tell - enc->entropy) / enc->entropy);
        fprintf(stderr, "efficiency: %f bits/symbol\n", (double)tell / enc->nb_symbols);
    }
#endif

    l = enc->low;
    c = enc->cnt;
    s = 10;
    m = 0x3FFF;
    e = ((l + m) & ~m) | (m + 1);
    s += c;
    offs = enc->offs;

    /*Make sure there's enough room for the entropy-coded bits.*/
    out              = enc->buf;
    storage          = enc->storage;
    const int s_bits = (s + 7) >> 3;
    int       b      = MAX(s_bits, 0);
    if (offs + b > storage) {
        storage = offs + b;
        EB_REALLOC_ARRAY_NO_CHECK(out, storage);
        if (out == NULL) {
            enc->error = -1;
            return NULL;
        }
        enc->buf     = out;
        enc->storage = storage;
    }

    /*We output the minimum number of bits that ensures that the symbols encoded
       thus far will be decoded correctly regardless of the bits that follow.*/
    if (s > 0) {
        uint64_t n;
        n = ((uint64_t)1 << (c + 16)) - 1;
        do {
            assert(offs < storage);
            uint16_t val = (uint16_t)(e >> (c + 16));
            out[offs]    = (unsigned char)(val & 0x00FF);
            if (val & 0x0100) {
                assert(offs > 0);
                propagate_carry_bwd(out, offs - 1);
            }
            offs++;

            e &= n;
            s -= 8;
            c -= 8;
            n >>= 8;
        } while (s > 0);
    }
    *nbytes = offs;

    return out;
}

/*Returns the number of bits "used" by the encoded symbols so far.
  This same number can be computed in either the encoder or the decoder, and is
   suitable for making coding decisions.
  Warning: The value returned by this function can decrease compared to an
   earlier call, even after encoding more data, if there is an encoding error
   (i.e., a failure to allocate enough space for the output buffer).
  Return: The number of bits.
          This will always be slightly larger than the exact value (e.g., all
           rounding error is in the positive direction).*/
int svt_od_ec_enc_tell(const OdEcEnc* enc) {
    /*The 10 here counteracts the offset of -9 baked into cnt, and adds 1 extra
       bit, which we reserve for terminating the stream.*/
    return (enc->cnt + 10) + enc->offs * 8;
}

/*Given the current total integer number of bits used and the current value of
   rng, computes the fraction number of bits used to OD_BITRES precision.
  This is used by od_ec_enc_tell_frac() and od_ec_dec_tell_frac().
  nbits_total: The number of whole bits currently used, i.e., the value
                returned by od_ec_enc_tell() or od_ec_dec_tell().
  rng: The current value of rng from either the encoder or decoder state.
  Return: The number of bits scaled by 2**OD_BITRES.
          This will always be slightly larger than the exact value (e.g., all
           rounding error is in the positive direction).*/
uint32_t svt_od_ec_tell_frac(uint32_t nbits_total, uint32_t rng) {
    uint32_t nbits;
    int      l;
    int      i;
    /*To handle the non-integral number of bits still left in the encoder/decoder
       state, we compute the worst-case number of bits of val that must be
       encoded to ensure that the value is inside the range for any possible
       subsequent bits.
      The computation here is independent of val itself (the decoder does not
       even track that value), even though the real number of bits used after
       od_ec_enc_done() may be 1 smaller if rng is a power of two and the
       corresponding trailing bits of val are all zeros.
      If we did try to track that special case, then coding a value with a
       probability of 1/(1 << n) might sometimes appear to use more than n bits.
      This may help explain the surprising result that a newly initialized
       encoder or decoder claims to have used 1 bit.*/
    nbits = nbits_total << OD_BITRES;
    l     = 0;
    for (i = OD_BITRES; i-- > 0;) {
        int b;
        rng = rng * rng >> 15;
        b   = (int)(rng >> 16);
        l   = l << 1 | b;
        rng >>= b;
    }
    return nbits - l;
}

/*Returns the number of bits "used" by the encoded symbols so far.
  This same number can be computed in either the encoder or the decoder, and is
   suitable for making coding decisions.
  Warning: The value returned by this function can decrease compared to an
   earlier call, even after encoding more data, if there is an encoding error
   (i.e., a failure to allocate enough space for the output buffer).
  Return: The number of bits scaled by 2**OD_BITRES.
          This will always be slightly larger than the exact value (e.g., all
           rounding error is in the positive direction).*/
uint32_t svt_od_ec_enc_tell_frac(const OdEcEnc* enc) {
    return svt_od_ec_tell_frac(svt_od_ec_enc_tell(enc), enc->rng);
}

/********************************************************************************************************************************/
/********************************************************************************************************************************/
/********************************************************************************************************************************/
