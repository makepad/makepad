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

/******************************************************************************
 * @file random.h
 *
 * @brief Random generator for svt-av1 unit tests
 * - wrap C++11 random generator for different range.
 *
 * @author Cidana-Edmond, Cidana-Wenyao <wenyao.liu@cidana.com>
 *
 ******************************************************************************/

#ifndef _TEST_RANDOM_H_
#define _TEST_RANDOM_H_

#include <stdint.h>
#include <assert.h>
#include <math.h>
#include <random>

/** @defgroup svt_av1_test_tool Tool set of test
 *  Defines the tool set of unit test such as random generator, bits shifting
 * and etc...
 *  @{
 */

namespace svt_av1_test_tool {

using std::mt19937;
using std::uniform_int_distribution;
using std::uniform_real_distribution;

/** SVTRandom defines a tool class for generating random integer as unit test
 * samples and params, the tool can support a random 32-bit integer from
 * [-2^31,2^31).
 */
class SVTRandom {
  public:
    /** contructor with given minimum and maximum bound of random integer*/
    SVTRandom(const int min_bound, const int max_bound)
        : gen_(deterministic_seed_) {
        setup(min_bound, max_bound);
    }

    /** contructor with given limit bits and signed symbol*/
    SVTRandom(const int nbits, const bool is_signed)
        : gen_(deterministic_seed_) {
        calculate_bounds(nbits, is_signed);
    }

    /** contructor with given minimum and maximum bound of random real*/
    SVTRandom(const float min_bound, const float max_bound)
        : gen_(deterministic_seed_) {
        setup(min_bound, max_bound);
    }

    /** contructor with given minimum, maximum bound of random integer and seed
     */
    explicit SVTRandom(const int min_bound, const int max_bound,
                       const uint32_t seed)
        : gen_(seed) {
        setup(min_bound, max_bound);
    }

    /** contructor with given limit bits, signed symbol and seed */
    explicit SVTRandom(const int nbits, const bool is_signed,
                       const uint32_t seed)
        : gen_(seed) {
        calculate_bounds(nbits, is_signed);
    }

    /** reset generator with new seed
     * @param seed new seed for generator reset
     */
    void reset(uint32_t seed) {
        gen_.seed(seed);
    }

    /** reset generator with default seed
     */
    void reset() {
        gen_.seed(deterministic_seed_);
    }

    /** generate a new random integer with minimum and maximum bounds
     * @return:
     * value of random integer
     */
    int random() {
        return dist_nbit_(gen_);
    }

    float random_float() {
        return (float)dist_real_(gen_);
    }

    uint8_t Rand8(void) {
        return (uint8_t)(random());
    }

    uint16_t Rand16(void) {
        return (uint16_t)(random());
    }

  private:
    /** setup bounds of generator */
    void setup(const int min_bound, const int max_bound) {
        assert(min_bound <= max_bound);
        decltype(dist_nbit_)::param_type param{min_bound, max_bound};
        dist_nbit_.param(param);
    }

    void setup(const float min_bound, const float max_bound) {
        assert(min_bound <= max_bound);
        decltype(dist_real_)::param_type param{min_bound, max_bound};
        dist_real_.param(param);
    }

    /** calculate and setup bounds of generator */
    void calculate_bounds(const int nbits, const bool is_signed) {
        assert(nbits <= 32);
        int set_bits =
            is_signed ? nbits - 1 : (nbits == 32 ? nbits - 1 : nbits);
        int min_bound = 0, max_bound = 0;
        for (int i = 0; i < set_bits; i++)
            max_bound |= (1 << i);
        if (is_signed)
            min_bound = 0 - (1 << set_bits);
        setup(min_bound, max_bound);
    }

  private:
    const int deterministic_seed_{13596};   /**< seed of random generator */
    std::mt19937 gen_;                      /**< random integer generator */
    uniform_int_distribution<> dist_nbit_;  /**< rule of integer generator */
    uniform_real_distribution<> dist_real_; /**< rule of real generator */
};

}  // namespace svt_av1_test_tool
/** @} */  // end of svt_av1_test_tool

#endif  // _TEST_RANDOM_H_
