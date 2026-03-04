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
 * @file BitstreamWriterTest.cc
 *
 * @brief Unit test for entropy coding functions:
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/
#include <math.h>
#include <stdlib.h>
#include <random>
#include "cabac_context_model.h"
#include "bitstream_unit.h"
#include "bitreader.h"
#include "definitions.h"
#include "pic_buffer_desc.h"
#include "gtest/gtest.h"
#include "random.h"

#ifdef max
#undef max
#endif
#ifdef min
#undef min
#endif
/**
 * @brief Unit test for Bitstream writer functions:
 * - aom_write
 * - aom_write_symbols
 * - aom_write_literal
 * - aom_start_encode
 * - aom_stop_encode
 *
 * Test strategy:
 * Verify by writing bits/values and reading bits/values in paris,
 * and compare the values read out and values written.
 *
 * Expected result:
 * The values read out should match with values written.
 *
 * Test coverage:
 * To write the bits, probabilities are required to setup as context.
 * Context are populated with different probabilities, including:
 * - fixed probabilities,
 * - Random probability
 * - low probability,
 * - high probability
 * - mixed probability are
 * Meantime different bit pattern are generated, including:
 * - fixed pattern(0, 1)
 * - random pattern
 *
 */
using svt_av1_test_tool::SVTRandom;
namespace {
const int deterministic_seeds = 0xa42b;
class BitstreamWriterTest : public ::testing::Test {
  public:
    BitstreamWriterTest() : gen_(deterministic_seeds) {
        normal_probs_ = new SVTRandom(0, 255);
        low_probs_ = new SVTRandom(0, 32);
    }

    ~BitstreamWriterTest() {
        delete normal_probs_;
        delete low_probs_;
    }

    void write_random_bits(int loop) {
        // generate various proba
        for (int prob_gen_method = 0; prob_gen_method < 7; ++prob_gen_method) {
            const int total_bits = 1000;
            uint8_t probas[total_bits];

            // setup random probability in [0, 255)
            generate_random_prob(probas, total_bits, prob_gen_method);

            for (int bit_gen_method = 0; bit_gen_method < 3; ++bit_gen_method) {
                const int buffer_size = 10000;
                AomWriter bw;
                OutputBitstreamUnit output_bitstream_ptr;
                uint8_t bw_buffer[buffer_size];
                output_bitstream_ptr.buffer_av1 = bw_buffer;
                output_bitstream_ptr.buffer_begin_av1 = bw_buffer;
                output_bitstream_ptr.size = buffer_size;
                uint8_t test_bits[total_bits];

                // setup random bits 0/1
                generate_random_bits(test_bits, total_bits, bit_gen_method);

                // encode the bits
                aom_start_encode(&bw, &output_bitstream_ptr);
                for (int i = 0; i < total_bits; ++i) {
                    aom_write(&bw, test_bits[i], static_cast<int>(probas[i]));
                }
                aom_stop_encode(&bw);

                // read out the bits and verify
                aom_reader br;
                memset(&br, 0, sizeof(br));
                aom_reader_init(&br, bw_buffer, bw.pos);
                for (int i = 0; i < total_bits; ++i) {
                    GTEST_ASSERT_EQ(aom_read(&br, probas[i], nullptr),
                                    test_bits[i])
                        << "loop: " << loop << "pos: " << i << " / "
                        << total_bits << " bit_gen_method: " << bit_gen_method
                        << " prob_gen_method: " << prob_gen_method;
                }
            }
        }
    }

  private:
    void generate_random_prob(uint8_t *const probas, const int total_probas,
                              const int prob_gen_method) {
        std::bernoulli_distribution flip_dist(0.5);

        switch (prob_gen_method) {
            // extreme probas
        case 0: memset(probas, 0, total_probas * sizeof(probas[0])); break;
        case 1:
            for (int i = 0; i < total_probas; ++i)
                probas[i] = 255;
            break;
            //
        case 2:
            for (int i = 0; i < total_probas; ++i)
                probas[i] = 128;
            break;
        case 3:
            // uniform distribution between 0 ~ 255
            for (int i = 0; i < total_probas; ++i)
                probas[i] = (uint8_t)normal_probs_->random();
            break;
        case 4:
            // low probability
            for (int i = 0; i < total_probas; ++i)
                probas[i] = (uint8_t)low_probs_->random();
            break;
        case 5:
            // high probability
            for (int i = 0; i < total_probas; ++i)
                probas[i] = 255 - (uint8_t)low_probs_->random();
            break;
        case 6:
        default:
            // mix high and low probability
            for (int i = 0; i < total_probas; ++i) {
                bool flip = flip_dist(gen_);
                probas[i] = flip ? (uint8_t)low_probs_->random()
                                 : 255 - (uint8_t)low_probs_->random();
            }
            break;
        }
    }

    void generate_random_bits(uint8_t *const test_bits, const int total_bits,
                              const int bit_gen_method) {
        std::bernoulli_distribution bit_dist(0.5);

        // setup test bits
        switch (bit_gen_method) {
        case 0:
        case 1:
            memset(
                test_bits, bit_gen_method, total_bits * sizeof(test_bits[0]));
            break;
        default:
            for (int i = 0; i < total_bits; ++i)
                test_bits[i] = bit_dist(gen_);
        }
    }

  private:
    SVTRandom *normal_probs_;
    SVTRandom *low_probs_;
    std::mt19937 gen_;
};

TEST_F(BitstreamWriterTest, write_bits_random) {
    const int num_tests = 100;
    for (int i = 0; i < num_tests; ++i)
        write_random_bits(i);
}

TEST(Entropy_BitstreamWriter, write_literal_extreme_int) {
    // test max int
    constexpr int32_t max_int = std::numeric_limits<int32_t>::max();
    constexpr int32_t min_int = std::numeric_limits<int32_t>::min();

    const int buffer_size = 1024;
    OutputBitstreamUnit output_bitstream_ptr;
    uint8_t stream_buffer[buffer_size];
    output_bitstream_ptr.buffer_av1 = stream_buffer;
    output_bitstream_ptr.buffer_begin_av1 = stream_buffer;
    output_bitstream_ptr.size = buffer_size;
    AomWriter bw;

    aom_start_encode(&bw, &output_bitstream_ptr);
    aom_write_literal(&bw, max_int, 32);
    aom_write_literal(&bw, min_int, 32);
    aom_stop_encode(&bw);

    aom_reader br;
    memset(&br, 0, sizeof(br));
    aom_reader_init(&br, stream_buffer, bw.pos);
    EXPECT_EQ(aom_read_literal(&br, 32, nullptr), max_int)
        << "read max_int fail";
    EXPECT_EQ(aom_read_literal(&br, 32, nullptr), min_int)
        << "read min_int fail";
}

TEST(Entropy_BitstreamWriter, write_symbol_no_update) {
    AomWriter bw;
    memset(&bw, 0, sizeof(bw));

    const int buffer_size = 1024;
    OutputBitstreamUnit output_bitstream_ptr;
    uint8_t stream_buffer[buffer_size];
    output_bitstream_ptr.buffer_av1 = stream_buffer;
    output_bitstream_ptr.buffer_begin_av1 = stream_buffer;
    output_bitstream_ptr.size = buffer_size;
    // get default cdf
    const int base_qindex = 20;
    FRAME_CONTEXT fc;
    memset(&fc, 0, sizeof(fc));
    svt_av1_default_coef_probs(&fc, base_qindex);

    // write random bit sequences and expect read out
    // the same random sequences.
    std::bernoulli_distribution rnd(0.5);
    std::mt19937 gen(deterministic_seeds);

    aom_start_encode(&bw, &output_bitstream_ptr);
    for (int i = 0; i < 500; ++i) {
        aom_write_symbol(&bw, rnd(gen), fc.txb_skip_cdf[0][0], 2);
        aom_write_symbol(&bw, rnd(gen), fc.txb_skip_cdf[0][0], 2);
    }
    aom_stop_encode(&bw);

    // expect read out 0, 1 in order
    rnd.reset();
    gen.seed(deterministic_seeds);

    aom_reader br;
    memset(&br, 0, sizeof(br));
    aom_reader_init(&br, stream_buffer, bw.pos);
    for (int i = 0; i < 500; ++i) {
        ASSERT_EQ(aom_read_symbol(&br, fc.txb_skip_cdf[0][0], 2, nullptr),
                  rnd(gen));
        ASSERT_EQ(aom_read_symbol(&br, fc.txb_skip_cdf[0][0], 2, nullptr),
                  rnd(gen));
    }
}

TEST(Entropy_BitstreamWriter, write_symbol_with_update) {
    AomWriter bw;
    memset(&bw, 0, sizeof(bw));

    const int buffer_size = 1024;
    OutputBitstreamUnit output_bitstream_ptr;
    uint8_t stream_buffer[buffer_size];
    output_bitstream_ptr.buffer_av1 = stream_buffer;
    output_bitstream_ptr.buffer_begin_av1 = stream_buffer;
    output_bitstream_ptr.size = buffer_size;
    bw.allow_update_cdf = 1;

    // get default cdf
    const int base_qindex = 20;
    FRAME_CONTEXT fc;
    memset(&fc, 0, sizeof(fc));

    svt_av1_default_coef_probs(&fc, base_qindex);

    // write random bit sequences and expect read out
    // the same random sequences.
    std::bernoulli_distribution rnd(0.5);
    std::mt19937 gen(deterministic_seeds);

    aom_start_encode(&bw, &output_bitstream_ptr);
    for (int i = 0; i < 500; ++i) {
        aom_write_symbol(&bw, rnd(gen), fc.txb_skip_cdf[0][0], 2);
        aom_write_symbol(&bw, rnd(gen), fc.txb_skip_cdf[0][0], 2);
    }
    aom_stop_encode(&bw);

    // reset random generator
    rnd.reset();
    gen.seed(deterministic_seeds);

    aom_reader br;
    memset(&br, 0, sizeof(br));
    aom_reader_init(&br, stream_buffer, bw.pos);
    br.allow_update_cdf = 1;
    svt_av1_default_coef_probs(&fc, base_qindex);  // reset cdf
    for (int i = 0; i < 500; i++) {
        ASSERT_EQ(aom_read_symbol(&br, fc.txb_skip_cdf[0][0], 2, nullptr),
                  rnd(gen));
        ASSERT_EQ(aom_read_symbol(&br, fc.txb_skip_cdf[0][0], 2, nullptr),
                  rnd(gen));
    }
}
}  // namespace
