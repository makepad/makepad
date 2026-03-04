/*
 * Copyright(c) 2025 Meta Platforms, Inc. and affiliates.
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
 * @file MultiEncoderTest.cc
 *
 * @brief SVT-AV1 multi-encoder thread safety test
 *
 * Tests running multiple encoder instances on different threads with RTC
 * configuration to verify thread safety of global state initialization.
 *
 ******************************************************************************/

#include "EbSvtAv1Enc.h"
#include "gtest/gtest.h"
#include "DummyVideoSource.h"
#include <atomic>
#include <chrono>
#include <cstring>
#include <functional>
#include <memory>
#include <mutex>
#include <thread>
#include <vector>

using svt_av1_video_source::DummyVideoSource;

namespace {

// Test configuration constants
static constexpr int kNumEncoders = 2;
static constexpr int kWidth = 640;
static constexpr int kHeight = 360;
static constexpr int kNumFrames = 30;
static constexpr int kTargetBitrateKbps = 500;
static constexpr int kFrameRate = 30;
static constexpr int kTimeoutSeconds = 30;

// Global test state
static std::mutex g_log_mutex;
static std::atomic<bool> g_test_failed{false};
static std::atomic<int> g_completed_encoders{0};

// Geometry size configuration for VaryingBlockGeometrySizes test
enum class GeometrySize { SMALL, LARGE };

/*****************************************************************************
 * Helper Functions
 *****************************************************************************/

// Log an error message thread-safely and set test failure flag
static void log_error(int id, const char *msg, EbErrorType err) {
    std::lock_guard<std::mutex> lock(g_log_mutex);
    std::cerr << "[Encoder " << id << "] " << msg << ": " << err << std::endl;
    g_test_failed = true;
}

// Configure encoder with RTC settings (small block geometry)
// When geometry_size is LARGE, adjusts settings to enable all block partitions
static void configure_encoder(
    EbSvtAv1EncConfiguration &config,
    GeometrySize geometry_size = GeometrySize::SMALL) {
    // Common settings
    config.screen_content_mode = 0;
    config.fast_decode = 0;
    config.level_of_parallelism = 1;
    config.use_cpu_flags = EB_CPU_FLAGS_ALL;
    config.recon_enabled = false;
    config.enable_dg = false;

    config.source_width = kWidth;
    config.source_height = kHeight;
    config.resize_mode = RESIZE_NONE;

    config.frame_rate_numerator = kFrameRate;
    config.frame_rate_denominator = 1;

    config.encoder_bit_depth = 8;
    config.encoder_color_format = EB_YUV420;
    config.profile = MAIN_PROFILE;

    config.look_ahead_distance = 0;
    config.recode_loop = 0;

    if (geometry_size == GeometrySize::SMALL) {
        // Fast preset, RTC mode - restricted block partitions (small
        // max_block_cnt)
        config.enc_mode = 11;
        config.tune = 1;
        config.rtc = true;
        config.intra_period_length = -1;
        config.hierarchical_levels = 0;
        config.pred_structure = 1;  // LOW_DELAY

        config.rate_control_mode = SVT_AV1_RC_MODE_CBR;
        config.target_bit_rate = kTargetBitrateKbps * 1000;
        config.max_qp_allowed = 63;
        config.min_qp_allowed = 4;
    } else {
        // Slow preset, non-RTC mode - all block partitions enabled (large
        // max_block_cnt)
        config.enc_mode = 0;
        config.tune = 0;  // PSNR tune
        config.rtc = false;
        config.intra_period_length = 32;
        config.hierarchical_levels = 4;
        config.pred_structure = 2;  // RANDOM_ACCESS

        config.rate_control_mode = SVT_AV1_RC_MODE_CQP_OR_CRF;
        config.qp = 30;
        config.max_qp_allowed = 63;
        config.min_qp_allowed = 1;
    }
}

// Initialize encoder: init_handle -> set_parameter -> init
// Returns true on success, false on failure (logs error and sets g_test_failed)
static bool init_encoder(EbComponentType **encoder_handle,
                         EbSvtAv1EncConfiguration *config, int id,
                         GeometrySize geometry_size = GeometrySize::SMALL) {
    EbErrorType ret = svt_av1_enc_init_handle(encoder_handle, config);
    if (ret != EB_ErrorNone) {
        log_error(id, "svt_av1_enc_init_handle failed", ret);
        return false;
    }

    configure_encoder(*config, geometry_size);

    ret = svt_av1_enc_set_parameter(*encoder_handle, config);
    if (ret != EB_ErrorNone) {
        log_error(id, "svt_av1_enc_set_parameter failed", ret);
        svt_av1_enc_deinit_handle(*encoder_handle);
        return false;
    }

    ret = svt_av1_enc_init(*encoder_handle);
    if (ret != EB_ErrorNone) {
        log_error(id, "svt_av1_enc_init failed", ret);
        svt_av1_enc_deinit_handle(*encoder_handle);
        return false;
    }

    return true;
}

// Shutdown encoder: deinit -> deinit_handle
static void shutdown_encoder(EbComponentType *encoder_handle) {
    if (encoder_handle) {
        svt_av1_enc_deinit(encoder_handle);
        svt_av1_enc_deinit_handle(encoder_handle);
    }
}

// Encode frames from video source and drain output
// Returns true on success, false on failure
static bool encode_frames(EbComponentType *encoder_handle,
                          DummyVideoSource &video_source, int num_frames,
                          int id) {
    EbBufferHeaderType input_header;
    memset(&input_header, 0, sizeof(input_header));
    input_header.size = sizeof(EbBufferHeaderType);

    // Encode frames
    for (int frame_idx = 0; frame_idx < num_frames && !g_test_failed;
         frame_idx++) {
        EbSvtIOFormat *frame = video_source.get_next_frame();
        if (!frame) {
            log_error(id, "Failed to get frame from video source", EB_ErrorMax);
            return false;
        }

        input_header.p_buffer = reinterpret_cast<uint8_t *>(frame);
        input_header.n_filled_len = video_source.get_frame_size();
        input_header.pts = frame_idx;
        input_header.flags = 0;
        input_header.pic_type =
            (frame_idx == 0) ? EB_AV1_KEY_PICTURE : EB_AV1_INVALID_PICTURE;

        EbErrorType ret =
            svt_av1_enc_send_picture(encoder_handle, &input_header);
        if (ret != EB_ErrorNone) {
            log_error(id, "svt_av1_enc_send_picture failed", ret);
            return false;
        }

        // Try to get output (non-blocking)
        EbBufferHeaderType *output_header = nullptr;
        ret = svt_av1_enc_get_packet(encoder_handle, &output_header, 0);
        if (ret == EB_ErrorNone && output_header) {
            svt_av1_enc_release_out_buffer(&output_header);
        }
    }

    return !g_test_failed;
}

// Send EOS and drain remaining packets
static void flush_encoder(EbComponentType *encoder_handle) {
    EbBufferHeaderType eos_header;
    memset(&eos_header, 0, sizeof(eos_header));
    eos_header.size = sizeof(EbBufferHeaderType);
    eos_header.flags = EB_BUFFERFLAG_EOS;
    eos_header.pic_type = EB_AV1_INVALID_PICTURE;

    svt_av1_enc_send_picture(encoder_handle, &eos_header);

    // Drain remaining packets
    while (!g_test_failed) {
        EbBufferHeaderType *output_header = nullptr;
        EbErrorType ret =
            svt_av1_enc_get_packet(encoder_handle, &output_header, 1);

        if (ret == EB_NoErrorEmptyQueue || ret != EB_ErrorNone) {
            break;
        }

        if (output_header) {
            bool is_eos = (output_header->flags & EB_BUFFERFLAG_EOS) != 0;
            svt_av1_enc_release_out_buffer(&output_header);
            if (is_eos) {
                break;
            }
        }
    }
}

// Wait for encoder threads to complete with timeout
static void wait_for_completion(std::vector<std::thread> &threads,
                                int expected_count) {
    auto deadline = std::chrono::steady_clock::now() +
                    std::chrono::seconds(kTimeoutSeconds);

    while (g_completed_encoders < expected_count && !g_test_failed) {
        if (std::chrono::steady_clock::now() > deadline) {
            g_test_failed = true;
            std::cerr << "TIMEOUT: Test exceeded " << kTimeoutSeconds
                      << " seconds. Completed: " << g_completed_encoders.load()
                      << "/" << expected_count << std::endl;
            break;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
    }

    for (auto &t : threads) {
        if (t.joinable()) {
            t.join();
        }
    }
}

// Reset global test state
static void reset_test_state() {
    g_test_failed = false;
    g_completed_encoders = 0;
}

/*****************************************************************************
 * Tests
 *****************************************************************************/

/**
 * @brief Test that multiple encoder instances can run concurrently
 *
 * This test verifies thread safety of the encoder's global state
 * initialization, specifically the RTCD function pointers and
 * block geometry tables which are shared across encoder instances.
 */
TEST(MultiEncoderTest, ConcurrentEncoders) {
    reset_test_state();

    auto encoder_thread = [](int encoder_id) {
        // Each thread has its own video source
        DummyVideoSource video_source(IMG_FMT_420, kWidth, kHeight, 8);
        if (video_source.open_source(0, kNumFrames) != EB_ErrorNone) {
            log_error(encoder_id, "Failed to open video source", EB_ErrorMax);
            return;
        }

        EbComponentType *encoder_handle = nullptr;
        EbSvtAv1EncConfiguration config;
        memset(&config, 0, sizeof(config));

        if (!init_encoder(&encoder_handle, &config, encoder_id)) {
            video_source.close_source();
            return;
        }

        if (encode_frames(
                encoder_handle, video_source, kNumFrames, encoder_id)) {
            flush_encoder(encoder_handle);
        }

        shutdown_encoder(encoder_handle);
        video_source.close_source();
        g_completed_encoders++;
    };

    std::vector<std::thread> threads;
    for (int i = 0; i < kNumEncoders; i++) {
        threads.emplace_back(encoder_thread, i);
    }

    wait_for_completion(threads, kNumEncoders);

    EXPECT_FALSE(g_test_failed) << "Multi-encoder test failed";
    EXPECT_EQ(g_completed_encoders.load(), kNumEncoders)
        << "Not all encoders completed";
}

/**
 * @brief Test repeated init/deinit cycles with multiple encoders
 *
 * This test verifies that the reference counting for shared resources
 * works correctly across multiple init/deinit cycles.
 */
TEST(MultiEncoderTest, RepeatedInitDeinit) {
    constexpr int kIterations = 5;

    auto encoder_thread = [](int encoder_id) {
        DummyVideoSource video_source(IMG_FMT_420, kWidth, kHeight, 8);
        if (video_source.open_source(0, kNumFrames) != EB_ErrorNone) {
            log_error(encoder_id, "Failed to open video source", EB_ErrorMax);
            return;
        }

        EbComponentType *encoder_handle = nullptr;
        EbSvtAv1EncConfiguration config;
        memset(&config, 0, sizeof(config));

        if (!init_encoder(&encoder_handle, &config, encoder_id)) {
            video_source.close_source();
            return;
        }

        if (encode_frames(
                encoder_handle, video_source, kNumFrames, encoder_id)) {
            flush_encoder(encoder_handle);
        }

        shutdown_encoder(encoder_handle);
        video_source.close_source();
        g_completed_encoders++;
    };

    for (int iter = 0; iter < kIterations; iter++) {
        reset_test_state();

        std::vector<std::thread> threads;
        for (int i = 0; i < kNumEncoders; i++) {
            threads.emplace_back(encoder_thread, i);
        }

        wait_for_completion(threads, kNumEncoders);

        EXPECT_FALSE(g_test_failed) << "Iteration " << iter << " failed";
        EXPECT_EQ(g_completed_encoders.load(), kNumEncoders)
            << "Iteration " << iter << ": not all encoders completed";
    }
}

/**
 * @brief Test concurrent encoders with unsynchronized recreation
 *
 * This test verifies thread safety when multiple encoder instances
 * are created and destroyed repeatedly at different times (not synchronized).
 * Each thread independently creates, encodes a few frames, destroys,
 * and repeats this cycle multiple times.
 */
TEST(MultiEncoderTest, UnsynchronizedRecreation) {
    reset_test_state();

    constexpr int kCyclesPerThread = 3;
    constexpr int kFramesPerCycle = 5;

    auto cycling_encoder_thread = [=](int thread_id) {
        for (int cycle = 0; cycle < kCyclesPerThread && !g_test_failed;
             cycle++) {
            DummyVideoSource video_source(IMG_FMT_420, kWidth, kHeight, 8);
            if (video_source.open_source(0, kFramesPerCycle) != EB_ErrorNone) {
                log_error(
                    thread_id, "Failed to open video source", EB_ErrorMax);
                return;
            }

            EbComponentType *encoder_handle = nullptr;
            EbSvtAv1EncConfiguration config;
            memset(&config, 0, sizeof(config));

            // Add delay to desynchronize threads
            int delay_ms = (thread_id * 7 + cycle * 3) % 50;
            std::this_thread::sleep_for(std::chrono::milliseconds(delay_ms));

            if (!init_encoder(&encoder_handle, &config, thread_id)) {
                video_source.close_source();
                return;
            }

            if (encode_frames(
                    encoder_handle, video_source, kFramesPerCycle, thread_id)) {
                flush_encoder(encoder_handle);
            }

            shutdown_encoder(encoder_handle);
            video_source.close_source();
        }

        g_completed_encoders++;
    };

    std::vector<std::thread> threads;
    for (int i = 0; i < kNumEncoders; i++) {
        threads.emplace_back(cycling_encoder_thread, i);
    }

    wait_for_completion(threads, kNumEncoders);

    EXPECT_FALSE(g_test_failed) << "Unsynchronized recreation test failed";
    EXPECT_EQ(g_completed_encoders.load(), kNumEncoders)
        << "Not all encoder threads completed";
}

/**
 * @brief Test concurrent encoders with varying block geometry sizes
 *
 * This test verifies thread safety when encoder instances are created with
 * different configurations that result in different max_block_cnt values.
 * The svt_aom_blk_geom_mds global table is allocated based on max_block_cnt,
 * and if a smaller allocation is used when a larger one is needed, it will
 * cause out-of-bounds access, crash, or hang.
 *
 * Configuration strategy:
 * - GeometrySize::SMALL (enc_mode=11 + rtc=true) → small max_block_cnt
 * - GeometrySize::LARGE (enc_mode=0 + rtc=false) → large max_block_cnt
 */
TEST(MultiEncoderTest, VaryingBlockGeometrySizes) {
    reset_test_state();

    constexpr int kCyclesPerThread = 3;
    constexpr int kFramesPerCycle = 3;

    // Thread that alternates between small and large geometry configurations
    auto varying_geometry_thread = [=](int thread_id) {
        for (int cycle = 0; cycle < kCyclesPerThread && !g_test_failed;
             cycle++) {
            DummyVideoSource video_source(IMG_FMT_420, kWidth, kHeight, 8);
            if (video_source.open_source(0, kFramesPerCycle) != EB_ErrorNone) {
                log_error(
                    thread_id, "Failed to open video source", EB_ErrorMax);
                return;
            }

            EbComponentType *encoder_handle = nullptr;
            EbSvtAv1EncConfiguration config;
            memset(&config, 0, sizeof(config));

            // Add delay to desynchronize threads
            int delay_ms = (thread_id * 11 + cycle * 7) % 30;
            std::this_thread::sleep_for(std::chrono::milliseconds(delay_ms));

            // Alternate: even thread_id+cycle -> small, odd -> large
            // This creates interleaved allocation patterns
            GeometrySize geometry = ((thread_id + cycle) % 2 == 0)
                                        ? GeometrySize::SMALL
                                        : GeometrySize::LARGE;

            if (!init_encoder(&encoder_handle, &config, thread_id, geometry)) {
                video_source.close_source();
                return;
            }

            if (encode_frames(
                    encoder_handle, video_source, kFramesPerCycle, thread_id)) {
                flush_encoder(encoder_handle);
            }

            shutdown_encoder(encoder_handle);
            video_source.close_source();
        }

        g_completed_encoders++;
    };

    std::vector<std::thread> threads;
    for (int i = 0; i < kNumEncoders; i++) {
        threads.emplace_back(varying_geometry_thread, i);
    }

    wait_for_completion(threads, kNumEncoders);

    EXPECT_FALSE(g_test_failed)
        << "Varying block geometry test failed - possible svt_aom_blk_geom_mds "
           "allocation size mismatch";
    EXPECT_EQ(g_completed_encoders.load(), kNumEncoders)
        << "Not all encoder threads completed";
}

}  // namespace
