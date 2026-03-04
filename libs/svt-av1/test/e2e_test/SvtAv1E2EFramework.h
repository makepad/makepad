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
 * @file SvtAv1E2EFramework.h
 *
 * @brief Defines a test framework for End to End test
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#ifndef _SVT_AV1_E2E_FRAMEWORK_H_
#define _SVT_AV1_E2E_FRAMEWORK_H_

#include "E2eTestVectors.h"
#include "FrameQueue.h"
#include "PerformanceCollect.h"
#include "CompareTools.h"
#include "definitions.h"
#include "RefDecoder.h"
// Copied from EbAppProcessCmd.c
#define LONG_ENCODE_FRAME_ENCODE 4000
#define SPEED_MEASUREMENT_INTERVAL 2000
#define START_STEADY_STATE 1000
#define AV1_FOURCC 0x31305641  // used for ivf header
#define IVF_STREAM_HEADER_SIZE 32
#define IVF_FRAME_HEADER_SIZE 12
#define OBU_FRAME_HEADER_SIZE 3
#define TD_SIZE 2

/** @defgroup svt_av1_e2e_test Test framework for E2E test
 *  Defines the framework body of E2E test for the mainly test progress
 *  @{
 */
namespace svt_av1_e2e_test {
#include "E2eTestVectors.h"

using namespace svt_av1_e2e_test_vector;
using namespace svt_av1_e2e_tools;
using namespace svt_av1_video_source;

/** SvtAv1Context is a set of test contexts in whole test progress */
typedef struct {
    EbComponentType
        *enc_handle; /**< encoder handle, created from encoder library */
    EbSvtAv1EncConfiguration enc_params; /**< encoder parameter set */
    EbBufferHeaderType
        *output_stream_buffer; /**< output buffer of encoder in test */
    EbBufferHeaderType
        *input_picture_buffer; /**< input buffer of encoder in test */
} SvtAv1Context;

/** SvtAv1E2ETestFramework is a class with implementation of video source
 * control, encoding progress, decoding progress, data collection and data
 * comparison */
class SvtAv1E2ETestFramework : public ::testing::TestWithParam<EncTestSetting> {
  public:
    struct IvfFile {
        FILE *file;
        uint64_t byte_count_since_ivf;
        uint64_t ivf_count;
        explicit IvfFile(const std::string &path);
        IvfFile(const IvfFile &) = delete;
        IvfFile &operator=(const IvfFile &) = delete;
        ~IvfFile() {
            if (file) {
                fclose(file);
                file = nullptr;
            }
            byte_count_since_ivf = 0;
            ivf_count = 0;
        }
    };

  protected:
    SvtAv1E2ETestFramework();
    virtual ~SvtAv1E2ETestFramework();

  protected:
    /* configure test switches */
    virtual void config_test();

    /* change the encoder settings */
    virtual void update_enc_setting();

    /** Add custom process here, which will be invoked after
     encoding loop is finished, like output stats,
     analyse the Bitstream generated.
    */
    virtual void post_process();

    /** Initialize the test, including
     create and setup encoder, setup input and output buffer
     create decoder if required.
    */
    void init_test(TestVideoVector &test_vector);

    /** deinitialize the test, including destropy the encoder, release
        the input and output buffer, close the source file;
    */
    void deinit_test();

    /** run the encode loop */
    void run_encode_process();

    /* wrapper of the whole test process, and it will
       iterate all the test vectors and run the test
    */
    void run_test();
    /* wrapper of the whole test process, and it will
       iterate all the test vectors and run the test.
       This test will run in a child process, so it
       will not break the whole test.
    */
    void run_death_test();

    /* generate event list by frame settings,
       e.g. reference scaling
    */
    void gen_frame_event(const EncTestSetting &setting, uint32_t frame_count,
                         void **head);

  public:
    static VideoSource *prepare_video_src(const TestVideoVector &vector);
    static EbColorFormat setup_video_format(VideoColorFormat fmt);
    static void setup_src_param(const VideoSource *source,
                                EbSvtAv1EncConfiguration &config);
    /** get reconstructed frame from encoder, it should call after send data
     * @param ctxt  context of encoder
     * @param recon  video frame queue of reconstructed
     * @param is_eos  flag of recon frames is eos
     * into decoder */
    static void get_recon_frame(const SvtAv1Context &ctxt, FrameQueue *recon,
                                bool &is_eos);

  private:
    /** write ivf header to output file */
    void write_output_header();
    /** write compressed data into file
     * @param output  compressed data from encoder
     */
    void write_compress_data(const EbBufferHeaderType *output);
    /** process compressed data by write to file for send to decoder
     * @param data  compressed data from encoder
     */
    void process_compress_data(const EbBufferHeaderType *data);
    /** send compressed data to decoder
     * @param data  compressed data from encoder, single OBU
     * @param size  size of compressed data
     */
    void decode_compress_data(const uint8_t *data, const uint32_t size);
    /** check video frame psnr with source
     * @param frame  video frame from reference decoder
     */
    void check_psnr(const VideoFrame &frame);

    /* TODO: add comments */
    void output_stat();

  protected:
    VideoSource *video_src_;   /**< video source context */
    SvtAv1Context av1enc_ctx_; /**< AV1 encoder context */
    uint32_t start_pos_;       /**< start position of video frame */
    uint32_t frames_to_test_;  /**< frame count for this test */
    FrameQueue *recon_queue_;  /**< reconstructed frame collection */
    RefDecoder *refer_dec_;    /**< reference decoder context */
    IvfFile *output_file_;     /**< file handle for save encoder output data */
    uint8_t obu_frame_header_size_; /**< size of obu frame header */
    PerformanceCollect *collect_;   /**< performance and time collection*/
    VideoSource *psnr_src_;         /**< video source context for psnr */
    ICompareQueue *ref_compare_; /**< sink of reference to compare with recon*/
    PsnrStatistics pnsr_statistics_; /**< psnr statistics recorder.*/
    bool use_ext_qp_; /**< flag of use external qp from video source or not*/
    EncTestSetting enc_setting;
    /* test configuration */
    bool enable_recon; /**< flag to control if make encoder output recon yuvs or
                          not */
    bool enable_decoder;        /**< flag to control if create av1 decoder */
    bool enable_stat;           /**< flag to control if output encoder stat */
    bool enable_save_bitstream; /**< flag to control if the Bitstream is saved
                                   on disk */
    bool
        enable_analyzer; /**< flag to control if create decoder with analyzer */
    bool enable_config;  /**< flag to control if use configuratio of encoder
                            params */
    bool enable_invert_tile_decoding;
    void *enc_config_; /**< handle of encoder configuration data structure */
    int insert_blank_interval; /**< interval of inserting blank frame in
                                  source*/
};

}  // namespace svt_av1_e2e_test
/** @} */  // end of svt_av1_e2e_test_vector

#endif  //_SVT_AV1_E2E_FRAMEWORK_H_
