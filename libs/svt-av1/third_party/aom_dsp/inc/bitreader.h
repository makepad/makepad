/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef AOM_AOM_DSP_BITREADER_H_
#define AOM_AOM_DSP_BITREADER_H_

#include <assert.h>
#include <limits.h>

#include "entdec.h"

#define ACCT_STR_PARAM
#define ACCT_STR_ARG(s)

#define aom_read(r, prob, ACCT_STR_NAME) \
  aom_read_(r, prob ACCT_STR_ARG(ACCT_STR_NAME))
#define aom_read_bit(r, ACCT_STR_NAME) \
  aom_read_bit_(r ACCT_STR_ARG(ACCT_STR_NAME))
#define aom_read_tree(r, tree, probs, ACCT_STR_NAME) \
  aom_read_tree_(r, tree, probs ACCT_STR_ARG(ACCT_STR_NAME))
#define aom_read_literal(r, bits, ACCT_STR_NAME) \
  aom_read_literal_(r, bits ACCT_STR_ARG(ACCT_STR_NAME))
#define aom_read_cdf(r, cdf, nsymbs, ACCT_STR_NAME) \
  aom_read_cdf_(r, cdf, nsymbs ACCT_STR_ARG(ACCT_STR_NAME))
#define aom_read_symbol(r, cdf, nsymbs, ACCT_STR_NAME) \
  aom_read_symbol_(r, cdf, nsymbs ACCT_STR_ARG(ACCT_STR_NAME))

#ifdef __cplusplus
extern "C" {
#endif

struct aom_reader {
  const uint8_t *buffer;
  const uint8_t *buffer_end;
  od_ec_dec ec;
  uint8_t allow_update_cdf;
};

typedef struct aom_reader aom_reader;

int aom_reader_init(aom_reader *r, const uint8_t *buffer, size_t size);

const uint8_t *aom_reader_find_begin(aom_reader *r);

const uint8_t *aom_reader_find_end(aom_reader *r);

// Returns true if the bit reader has tried to decode more data from the buffer
// than was actually provided.
int aom_reader_has_overflowed(const aom_reader *r);

// Returns the position in the bit reader in bits.
uint32_t aom_reader_tell(const aom_reader *r);

// Returns the position in the bit reader in 1/8th bits.
uint32_t aom_reader_tell_frac(const aom_reader *r);

static INLINE int aom_read_(aom_reader *r, int prob ACCT_STR_PARAM) {
  int p = (0x7FFFFF - (prob << 15) + prob) >> 8;
  int bit = od_ec_decode_bool_q15(&r->ec, p);
  return bit;
}

static INLINE int aom_read_bit_(aom_reader *r ACCT_STR_PARAM) {
  int ret;
  ret = aom_read(r, 128, NULL);  // aom_prob_half
  return ret;
}

static INLINE int aom_read_literal_(aom_reader *r, int bits ACCT_STR_PARAM) {
  int literal = 0, bit;

  for (bit = bits - 1; bit >= 0; bit--) literal |= aom_read_bit(r, NULL) << bit;
  return literal;
}

static INLINE int aom_read_cdf_(aom_reader *r, const AomCdfProb *cdf,
                                int nsymbs ACCT_STR_PARAM) {
  int symb;
  assert(cdf != NULL);
  symb = od_ec_decode_cdf_q15(&r->ec, cdf, nsymbs);
  return symb;
}

static INLINE int aom_read_symbol_(aom_reader *r, AomCdfProb *cdf,
                                   int nsymbs ACCT_STR_PARAM) {
  int ret;
  ret = aom_read_cdf(r, cdf, nsymbs, ACCT_STR_NAME);
  if (r->allow_update_cdf) update_cdf(cdf, ret, nsymbs);
  return ret;
}

#ifdef __cplusplus
}  // extern "C"
#endif

#endif  // AOM_AOM_DSP_BITREADER_H_
