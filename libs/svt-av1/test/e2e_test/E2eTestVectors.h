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
 * @file E2eTestVectors.h
 *
 * @brief Defines test vectors for End to End test
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#ifndef _E2E_TEST_VECTOR_
#define _E2E_TEST_VECTOR_

#include <map>
#include "VideoSource.h"
#include "definitions.h"
#include "ConfigEncoder.h"

/** @defgroup svt_av1_e2e_test_vector Test vectors for E2E test
 *  Defines the test vectors of E2E test, with file-type, width, height and
 * file-path
 *  *** You need to get test vectors before run e2e test, we use CMaked
 * generated makefile to download test vectors, you can use 'make TestVectors'
 * to download them.
 *  @{
 */
namespace svt_av1_e2e_test_vector {

/** TestVectorFormat is enumerate type of input video file format */
typedef enum TestVectorFormat {
    YUV_VIDEO_FILE,
    Y4M_VIDEO_FILE,
    DUMMY_SOURCE
} TestVectorFormat;

/** TestVideoVector is tuple of test params in a test case */
typedef std::tuple<std::string,      /**< file name */
                   TestVectorFormat, /**< file format */
                   VideoColorFormat, /**< color format */
                   uint32_t,         /**< width */
                   uint32_t,         /**< height */
                   uint32_t,         /**< bit depth */
                   bool,             /**< compressed 2-bit in 10-bit frame */
                   uint32_t,         /**< start read position in frame */
                   uint32_t> /**< frames to test, (0) means full-frames*/
    TestVideoVector;
const std::vector<TestVideoVector> default_test_vectors = {
    std::make_tuple("kirland_640_480_30.yuv", YUV_VIDEO_FILE, IMG_FMT_420, 640,
                    480, 8, 0, 0, 60),
    std::make_tuple("niklas_640_480_30.yuv", YUV_VIDEO_FILE, IMG_FMT_420, 640,
                    480, 8, 0, 0, 60),
};

const std::vector<TestVideoVector> default_long_test_vectors = {
    std::make_tuple("kirland_640_480_30.yuv", YUV_VIDEO_FILE, IMG_FMT_420, 640,
                    480, 8, 0, 0, 1000),
    std::make_tuple("niklas_640_480_30.yuv", YUV_VIDEO_FILE, IMG_FMT_420, 640,
                    480, 8, 0, 0, 1000),
};

const std::vector<TestVideoVector> incomplete_sb_test_vectors = {
    std::make_tuple("park_joy_90p_8_420.y4m", Y4M_VIDEO_FILE, IMG_FMT_420, 160,
                    90, 8, 0, 0, 0),
    std::make_tuple("park_joy_90p_10_420.y4m", Y4M_VIDEO_FILE, IMG_FMT_420, 160,
                    90, 10, 0, 0, 0),
};

const std::vector<TestVideoVector> res_480p_test_vectors = {
    std::make_tuple("kirland_640_480_30.yuv", YUV_VIDEO_FILE, IMG_FMT_420, 640,
                    480, 8, 0, 0, 60),
    std::make_tuple("screendata.y4m", Y4M_VIDEO_FILE, IMG_FMT_420, 640, 480, 8,
                    0, 0, 0),
};

const std::vector<TestVideoVector> screen_test_vectors = {
    std::make_tuple("screendata.y4m", Y4M_VIDEO_FILE, IMG_FMT_420, 640, 480, 8,
                    0, 0, 0),
};

const std::vector<TestVideoVector> dummy_test_vectors = {
    std::make_tuple("colorbar_480p_8_420", DUMMY_SOURCE, IMG_FMT_420, 640, 480,
                    8, 0, 0, 100),
    std::make_tuple("colorbar_1080p_8_420", DUMMY_SOURCE, IMG_FMT_420, 1920,
                    1080, 8, 0, 0, 60),
    std::make_tuple("colorbar_64x64_8_420", DUMMY_SOURCE, IMG_FMT_420, 64, 64,
                    8, 0, 0, 60),
    std::make_tuple("colorbar_480p_10_420", DUMMY_SOURCE, IMG_FMT_420, 640, 480,
                    10, 0, 0, 100),
    std::make_tuple("colorbar_1080p_10_420", DUMMY_SOURCE, IMG_FMT_420, 1920,
                    1080, 10, 0, 0, 60),
    std::make_tuple("colorbar_64x64_10_420", DUMMY_SOURCE, IMG_FMT_420, 64, 64,
                    10, 0, 0, 60),
};

const std::vector<TestVideoVector> dummy_422_test_vectors = {
    std::make_tuple("colorbar_480p_8_422", DUMMY_SOURCE, IMG_FMT_422, 640, 480,
                    8, 0, 0, 100),
};

const std::vector<TestVideoVector> dummy_444_test_vectors = {
    std::make_tuple("colorbar_480p_8_444", DUMMY_SOURCE, IMG_FMT_444, 640, 480,
                    8, 0, 0, 100),
};

const std::vector<TestVideoVector> segment_test_vectors = {
    std::make_tuple("niklas_1280_720_30.y4m", Y4M_VIDEO_FILE, IMG_FMT_420, 1280,
                    720, 8, 0, 0, 0),
};

typedef std::tuple<std::string,              /**< event name */
                   uint32_t,                 /**< frame number */
                   PrivDataType,             /**< event type */
                   std::vector<std::string>> /**< parameters vector */
    TestFrameEvent;

using EncSetting = std::map<std::string, std::string>;
typedef struct EncTestSetting {
    std::string name;    // description of the test cases
    EncSetting setting;  // pairs of encoder setting, {name, value};
    std::vector<TestVideoVector> test_vectors;
    std::vector<TestFrameEvent> event_vector;

    EncTestSetting(const std::string& name_str, const EncSetting& settings,
                   const std::vector<TestVideoVector>& videos)
        : name(name_str), setting(settings), test_vectors(videos) {
    }

    EncTestSetting(const std::string& name_str, const EncSetting& settings,
                   const std::vector<TestVideoVector>& videos,
                   const std::vector<TestFrameEvent>& events)
        : name(name_str),
          setting(settings),
          test_vectors(videos),
          event_vector(events) {
    }

    std::string to_string(const std::string& fn) const {
        std::string str = get_setting_str();
        str += "test vector: ";
        str += fn;
        return str;
    }

    std::string to_cli(const TestVideoVector& vector) const {
        std::string str = "SvtAv1EncApp";
        str += get_vector_cli(vector);
        str += get_setting_cli();
        append_token(str, "StreamFile");
        str += "output.ivf";
        append_token(str, "ReconFile");
        str += "recon.yuv";
        return str;
    }

    std::string get_setting_str() const {
        std::string str(name);
        str += ": ";
        for (auto x : setting) {
            str += x.first;
            str += "=";
            str += x.second;
            str += ", ";
        }
        return str;
    }

    int color_fmt(VideoColorFormat fmt) const {
        switch (fmt) {
        case IMG_FMT_420:
        case IMG_FMT_420P10_PACKED: return 420;
        case IMG_FMT_422:
        case IMG_FMT_422P10_PACKED: return 422;
        case IMG_FMT_444:
        case IMG_FMT_444P10_PACKED: return 444;
        default: break;
        }
        return -1;
    }

    std::string get_vector_cli(const TestVideoVector& vector) const {
        std::string str;
        append_token(str, "InputFile");
        str += std::get<0>(vector);
        if (std::get<1>(vector) != Y4M_VIDEO_FILE) {
            append_token(str, "SourceWidth");
            str += std::to_string(std::get<3>(vector));
            append_token(str, "SourceHeight");
            str += std::to_string(std::get<4>(vector));
            append_token(str, "EncoderBitDepth");
            str += std::to_string(std::get<5>(vector));
            append_token(str, "EncoderColorFormat");
            str += std::to_string(color_fmt(std::get<2>(vector)));
        }
        return str;
    }

    void append_token(std::string& str, const char* n) const {
        str += " ";
        str += get_enc_token(n);
        str += " ";
    }

    std::string get_setting_cli() const {
        std::string str;
        for (auto x : setting) {
            append_token(str, x.first.c_str());
            str += x.second;
        }
        return str;
    }

    const std::string& get_setting_name() const {
        return name;
    }

    friend std::ostream& operator<<(std::ostream& os,
                                    const EncTestSetting& setting) {
        return os << setting.get_setting_str();
    }
    // used in INSTANTIATE_TEST_SUITE_P to append the param info into the test
    // name
    static std::string GetSettingName(
        const ::testing::TestParamInfo<EncTestSetting> setting) {
        return setting.param.get_setting_name();
    }

} EncTestSetting;

/**
 * @brief      Generate test vectors from config file.
 *
 * @param[in]  config_file  The configuration file name. It will read tihs file
 * from path which defined by system envrionment SVT_AV1_TEST_VECTOR_PATH.
 *
 * @return     A std::vector of test vectors.
 */
static inline const std::vector<EncTestSetting> generate_vector_from_config(
    const char* config_file) {
    std::vector<TestVideoVector> values;
    std::vector<EncTestSetting> enc_test_cases;
    std::string cfg_fn =
        svt_av1_video_source::VideoFileSource::get_vector_dir();
    cfg_fn = cfg_fn + '/' + config_file;

    FILE* file_handle;
    FOPEN(file_handle, cfg_fn.c_str(), "rt");
    if (file_handle != nullptr) {
        char line[1024] = {0};
        while (fgets(line, 1024, file_handle) != nullptr) {
            if (line[0] == '#' || line[0] == '\r' ||
                line[0] == '\n')  // skip comment line and empty line
                continue;
            char vector_fn[1024] = {0};
            uint32_t y4m = 0;
            TestVectorFormat file_type;
            char color_fmt[10];
            VideoColorFormat color_fmt_type = IMG_FMT_420;
            uint32_t w;
            uint32_t h;
            uint32_t bit_depth;
            uint32_t compressed_10bit;
            uint32_t start_frame;
            uint32_t frame_count;

            sscanf(line,
                   "%1023s %u %9s %u %u %u %u %u %u",
                   vector_fn,
                   &y4m,
                   color_fmt,
                   &w,
                   &h,
                   &bit_depth,
                   &compressed_10bit,
                   &start_frame,
                   &frame_count);

            file_type = y4m ? Y4M_VIDEO_FILE : YUV_VIDEO_FILE;
            if (strcmp(color_fmt, "420") == 0) {
                color_fmt_type = IMG_FMT_420;
            } else if (strcmp(color_fmt, "420p10") == 0) {
                color_fmt_type = IMG_FMT_420P10_PACKED;
            }
            values.push_back(TestVideoVector(vector_fn,
                                             file_type,
                                             color_fmt_type,
                                             w,
                                             h,
                                             (uint8_t)bit_depth,
                                             compressed_10bit,
                                             start_frame,
                                             frame_count));
        }
        fclose(file_handle);
    } else {
        printf("test configuration file can not be opended: %s!\n",
               cfg_fn.c_str());
    }
    enc_test_cases.push_back(EncTestSetting{
        "default_setting", std::map<std::string, std::string>(), values});
    return enc_test_cases;
}

}  // namespace svt_av1_e2e_test_vector
/** @} */  // end of svt_av1_e2e_test_vector

#endif  // _E2E_TEST_VECTOR_
