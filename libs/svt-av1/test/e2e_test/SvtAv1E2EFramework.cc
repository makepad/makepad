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
 * @file SvtAv1E2EFramework.cc
 *
 * @brief Impelmentation of End to End test framework
 *
 * @author Cidana-Edmond Cidana-Ryan Cidana-Wenyao
 *
 ******************************************************************************/
#include <algorithm>

#include "EbSvtAv1Enc.h"
#include "Y4mVideoSource.h"
#include "YuvVideoSource.h"
#include "DummyVideoSource.h"
#include "gtest/gtest.h"
#include "definitions.h"
#include "RefDecoder.h"
#include "SvtAv1E2EFramework.h"
#include "CompareTools.h"
#include "ConfigEncoder.h"
#include "EbSvtAv1Metadata.h"

#ifdef _WIN32
#define fseeko _fseeki64
#define ftello _ftelli64
#endif

using namespace svt_av1_e2e_test;
using namespace svt_av1_e2e_tools;

VideoSource *SvtAv1E2ETestFramework::prepare_video_src(
    const TestVideoVector &vector) {
    VideoSource *video_src = nullptr;
    switch (std::get<1>(vector)) {
    case YUV_VIDEO_FILE:
        video_src = new YuvVideoSource(std::get<0>(vector),
                                       std::get<2>(vector),
                                       std::get<3>(vector),
                                       std::get<4>(vector),
                                       (uint8_t)std::get<5>(vector));
        break;
    case Y4M_VIDEO_FILE:
        video_src = new Y4MVideoSource(std::get<0>(vector),
                                       std::get<2>(vector),
                                       std::get<3>(vector),
                                       std::get<4>(vector),
                                       (uint8_t)std::get<5>(vector));
        break;
    case DUMMY_SOURCE:
        video_src = new DummyVideoSource(std::get<2>(vector),
                                         std::get<3>(vector),
                                         std::get<4>(vector),
                                         (uint8_t)std::get<5>(vector));
        break;
    default: assert(0); break;
    }
    return video_src;
}

EbColorFormat SvtAv1E2ETestFramework::setup_video_format(VideoColorFormat fmt) {
    switch (fmt) {
    case IMG_FMT_420:
    case IMG_FMT_420P10_PACKED: return EB_YUV420;
    case IMG_FMT_422:
    case IMG_FMT_422P10_PACKED: return EB_YUV422;
    case IMG_FMT_444:
    case IMG_FMT_444P10_PACKED: return EB_YUV444;
    default: assert(0); break;
    }
    return EB_YUV400;
}

void SvtAv1E2ETestFramework::setup_src_param(const VideoSource *source,
                                             EbSvtAv1EncConfiguration &config) {
    config.encoder_color_format =
        setup_video_format(source->get_image_format());
    config.source_width = source->get_width_with_padding();
    config.source_height = source->get_height_with_padding();
    config.encoder_bit_depth = source->get_bit_depth();
}

SvtAv1E2ETestFramework::SvtAv1E2ETestFramework() : enc_setting(GetParam()) {
    memset(&av1enc_ctx_, 0, sizeof(av1enc_ctx_));
    video_src_ = nullptr;
    psnr_src_ = nullptr;
    recon_queue_ = nullptr;
    refer_dec_ = nullptr;
    output_file_ = nullptr;
    obu_frame_header_size_ = 0;
    collect_ = nullptr;
    ref_compare_ = nullptr;
    collect_ = new PerformanceCollect(typeid(this).name());
    use_ext_qp_ = false;
    enable_recon = false;
    enable_decoder = false;
    enable_stat = false;
    enable_save_bitstream = false;
    enable_analyzer = false;
    enable_config = false;
    enc_config_ = create_enc_config();
    insert_blank_interval = 0;
}

SvtAv1E2ETestFramework::~SvtAv1E2ETestFramework() {
    if (collect_) {
        delete collect_;
        collect_ = nullptr;
    }
    if (enc_config_) {
        release_enc_config(enc_config_);
        enc_config_ = nullptr;
    }
}

void SvtAv1E2ETestFramework::config_test() {
    enable_stat = true;
    if (enable_config) {
        // iterate the mappings and update config
        for (auto &x : enc_setting.setting) {
            if (x.first == "BlankFrame")
                insert_blank_interval = std::stoi(x.second);
            else
                set_enc_config(enc_config_, x.first.c_str(), x.second.c_str());
            printf("EncSetting: %s = %s\n", x.first.c_str(), x.second.c_str());
        }
    }
    // sort frame event vector
    if (enc_setting.event_vector.size() > 0) {
        std::sort(enc_setting.event_vector.begin(),
                  enc_setting.event_vector.end(),
                  [](TestFrameEvent evt1, TestFrameEvent evt2) {
                      return std::get<1>(evt1) < std::get<1>(evt2);
                  });
    }
}

void SvtAv1E2ETestFramework::update_enc_setting() {
    if (enable_config) {
        copy_enc_param(&av1enc_ctx_.enc_params, enc_config_);
        setup_src_param(video_src_, av1enc_ctx_.enc_params);
        if (recon_queue_)
            av1enc_ctx_.enc_params.recon_enabled = 1;
    }
}

void SvtAv1E2ETestFramework::post_process() {
    if (enable_stat)
        output_stat();
}

void SvtAv1E2ETestFramework::init_test(TestVideoVector &test_vector) {
    start_pos_ = std::get<7>(test_vector);
    frames_to_test_ = std::get<8>(test_vector);
    video_src_ = prepare_video_src(test_vector);
    psnr_src_ = prepare_video_src(test_vector);

    EbErrorType return_error = EB_ErrorNone;

    // check for video source
    ASSERT_NE(video_src_, nullptr) << "video source create failed!";
    return_error = video_src_->open_source(start_pos_, frames_to_test_);
    ASSERT_EQ(return_error, EB_ErrorNone)
        << "open_source return error:" << return_error;
    // Check input parameters
    uint32_t width = video_src_->get_width_with_padding();
    uint32_t height = video_src_->get_height_with_padding();
    uint32_t bit_depth = video_src_->get_bit_depth();
    ASSERT_GT(width, 0u) << "Video vector width error.";
    ASSERT_GT(height, 0u) << "Video vector height error.";
    ASSERT_TRUE(bit_depth == 10 || bit_depth == 8)
        << "Video vector bitDepth error.";

    video_src_->set_blank_frame(insert_blank_interval);

    //
    // Init handle
    //
    return_error = svt_av1_enc_init_handle(&av1enc_ctx_.enc_handle,
                                           &av1enc_ctx_.enc_params);
    ASSERT_EQ(return_error, EB_ErrorNone)
        << "svt_av1_enc_init_handle return error:" << return_error;
    ASSERT_NE(av1enc_ctx_.enc_handle, nullptr)
        << "svt_av1_enc_init_handle return null handle.";
    setup_src_param(video_src_, av1enc_ctx_.enc_params);
    av1enc_ctx_.enc_params.recon_enabled = 0;

    //
    // Prepare input and output buffer
    //
    // Input Buffer
    av1enc_ctx_.input_picture_buffer = new EbBufferHeaderType;
    ASSERT_NE(av1enc_ctx_.input_picture_buffer, nullptr)
        << "Malloc memory for inputPictureBuffer failed.";
    av1enc_ctx_.input_picture_buffer->p_buffer = nullptr;
    av1enc_ctx_.input_picture_buffer->size = sizeof(EbBufferHeaderType);
    av1enc_ctx_.input_picture_buffer->p_app_private = nullptr;
    av1enc_ctx_.input_picture_buffer->pic_type = EB_AV1_INVALID_PICTURE;
    av1enc_ctx_.input_picture_buffer->metadata = nullptr;

    // Output buffer
    av1enc_ctx_.output_stream_buffer = new EbBufferHeaderType;
    ASSERT_NE(av1enc_ctx_.output_stream_buffer, nullptr)
        << "Malloc memory for output_stream_buffer failed.";
    av1enc_ctx_.output_stream_buffer->p_buffer =
        new uint8_t[BITSTREAM_BUFFER_SIZE(width * height)];
    ASSERT_NE(av1enc_ctx_.output_stream_buffer->p_buffer, nullptr)
        << "Malloc memory for output_stream_buffer->p_buffer failed.";
    av1enc_ctx_.output_stream_buffer->size = sizeof(EbBufferHeaderType);
    av1enc_ctx_.output_stream_buffer->n_alloc_len =
        BITSTREAM_BUFFER_SIZE(width * height);
    av1enc_ctx_.output_stream_buffer->p_app_private = nullptr;
    av1enc_ctx_.output_stream_buffer->pic_type = EB_AV1_INVALID_PICTURE;
    av1enc_ctx_.output_stream_buffer->metadata = nullptr;

    // update encoder settings
    update_enc_setting();

    if (enable_recon) {
        // create recon queue to store the recon yuvs
        VideoFrameParam param;
        param.format = video_src_->get_image_format();
        param.width = video_src_->get_width_with_padding();
        param.height = video_src_->get_height_with_padding();
        param.bits_per_sample = video_src_->get_bit_depth();
        recon_queue_ = create_frame_queue(param);
        ASSERT_NE(recon_queue_, nullptr) << "can not create recon sink!!";
        if (recon_queue_)
            av1enc_ctx_.enc_params.recon_enabled = 1;
    }

    // set the parameter to encoder
    return_error = svt_av1_enc_set_parameter(av1enc_ctx_.enc_handle,
                                             &av1enc_ctx_.enc_params);
    ASSERT_EQ(return_error, EB_ErrorNone)
        << "svt_av1_enc_set_parameter return error:" << return_error;

    // initial encoder
    return_error = svt_av1_enc_init(av1enc_ctx_.enc_handle);
    ASSERT_EQ(return_error, EB_ErrorNone)
        << "svt_av1_enc_init return error:" << return_error;

    obu_frame_header_size_ = OBU_FRAME_HEADER_SIZE;

    // create reference decoder if required.
    if (enable_decoder) {
        refer_dec_ = create_reference_decoder(enable_analyzer);
        if (enable_invert_tile_decoding)
            refer_dec_->set_invert_tile_decoding_order();
        ASSERT_NE(refer_dec_, nullptr) << "can not create reference decoder!!";
    }

    // create IvfFile if required.
    if (enable_save_bitstream) {
        std::string fn = std::get<0>(test_vector) + ".ivf";
        output_file_ = new IvfFile(fn);
    }

    ASSERT_NE(psnr_src_, nullptr) << "PSNR source create failed!";
    EbErrorType err = psnr_src_->open_source(start_pos_, frames_to_test_);
    ASSERT_EQ(err, EB_ErrorNone) << "open_source return error:" << err;
}

void SvtAv1E2ETestFramework::deinit_test() {
    EbErrorType return_error = svt_av1_enc_deinit(av1enc_ctx_.enc_handle);
    ASSERT_EQ(return_error, EB_ErrorNone)
        << "svt_av1_enc_deinit return error:" << return_error;

    // Destruct the component
    return_error = svt_av1_enc_deinit_handle(av1enc_ctx_.enc_handle);
    ASSERT_EQ(return_error, EB_ErrorNone)
        << "svt_av1_enc_deinit_handle return error:" << return_error;
    av1enc_ctx_.enc_handle = nullptr;

    // Clear the intput and output buffer
    if (av1enc_ctx_.output_stream_buffer != nullptr) {
        if (av1enc_ctx_.output_stream_buffer->p_buffer != nullptr) {
            delete[] av1enc_ctx_.output_stream_buffer->p_buffer;
        }
        delete av1enc_ctx_.output_stream_buffer;
        av1enc_ctx_.output_stream_buffer = nullptr;
    }
    if (av1enc_ctx_.input_picture_buffer != nullptr) {
        delete av1enc_ctx_.input_picture_buffer;
        av1enc_ctx_.input_picture_buffer = nullptr;
    }

    // release recon queue
    if (recon_queue_) {
        delete recon_queue_;
        recon_queue_ = nullptr;
    }
    // release decoder;
    if (refer_dec_) {
        delete refer_dec_;
        refer_dec_ = nullptr;
    }

    // close and release the video src
    ASSERT_NE(video_src_, nullptr);
    ASSERT_NE(psnr_src_, nullptr);
    video_src_->close_source();
    psnr_src_->close_source();
    delete video_src_;
    video_src_ = nullptr;
    delete psnr_src_;
    psnr_src_ = nullptr;

    // close the Bitstream file
    if (output_file_) {
        delete output_file_;
        output_file_ = nullptr;
    }

    if (ref_compare_) {
        delete ref_compare_;
        ref_compare_ = nullptr;
    }
}

void SvtAv1E2ETestFramework::output_stat() {
    /** PSNR report */
    int count = 0;
    double psnr[4];
    pnsr_statistics_.get_statistics(count, psnr[0], psnr[1], psnr[2], psnr[3]);
    if (count > 0) {
        printf(
            "PSNR: %d frames, total: %0.4f, luma: %0.4f, cb: %0.4f, cr: "
            "%0.4f\r\n",
            count,
            psnr[0],
            psnr[1],
            psnr[2],
            psnr[3]);
    }
    pnsr_statistics_.reset();

    /** performance report */
    if (collect_) {
        const char ENCODING[] = "encoding";
        uint32_t frame_count = video_src_->get_frame_count();
        uint64_t total_enc_time = collect_->read_count(ENCODING);
        if (total_enc_time) {
            printf("Enc Performance: %.2fsec/frame (%.4fFPS)\n",
                   (double)total_enc_time / frame_count / 1000,
                   (double)frame_count * 1000 / total_enc_time);
        }
    }
}

void SvtAv1E2ETestFramework::gen_frame_event(const EncTestSetting &setting,
                                             uint32_t frame_count,
                                             void **head) {
    EbPrivDataNode *node = nullptr;
    for (const TestFrameEvent &event : setting.event_vector) {
        if (std::get<1>(event) == frame_count) {
            printf("%s param list:\t", std::get<0>(event).c_str());
            for (const std::string &str : std::get<3>(event))
                printf("%s\t", str.c_str());
            printf("\n");
            EbPrivDataNode *new_node =
                (EbPrivDataNode *)malloc(sizeof(EbPrivDataNode));
            ASSERT_NE(new_node, nullptr);
            switch (std::get<2>(event)) {
            case REF_FRAME_SCALING_EVENT: {
                EbRefFrameScale *data =
                    (EbRefFrameScale *)malloc(sizeof(EbRefFrameScale));
                ASSERT_NE(data, nullptr);
                data->scale_mode = std::stoi(std::get<3>(event)[0]);
                data->scale_denom = std::stoi(std::get<3>(event)[1]);
                data->scale_kf_denom = std::stoi(std::get<3>(event)[2]);
                new_node->size = sizeof(EbRefFrameScale);
                new_node->node_type = REF_FRAME_SCALING_EVENT;
                new_node->data = data;
            } break;
            default: GTEST_FAIL() << "unhandled frame event"; break;
            }
            new_node->next = node;
            node = new_node;
        }
    }
    *head = node;
}

static void free_private_data_list(void *node_head) {
    while (node_head) {
        EbPrivDataNode *node = (EbPrivDataNode *)node_head;
        node_head = node->next;
        if (node->data) {
            free(node->data);
            node->data = nullptr;
        }
        free(node);
    };
}

void SvtAv1E2ETestFramework::run_encode_process() {
    static const char READ_SRC[] = "read_src";
    static const char ENCODING[] = "encoding";
    static const char RECON[] = "recon";
    static const char CONFORMANCE[] = "conformance";

    EbErrorType return_error = EB_ErrorNone;

    uint32_t frame_count = video_src_->get_frame_count();
    ASSERT_GT(frame_count, 0u) << "video srouce file does not contain frame!!";
    if (recon_queue_)
        recon_queue_->set_frame_count(frame_count);

    if (output_file_)
        write_output_header();

    uint8_t *frame = nullptr;
    bool src_file_eos = false;
    bool enc_file_eos = false;
    bool rec_file_eos = recon_queue_ ? false : true;
    bool early_termination = false;
    do {
        if (!src_file_eos) {
            // read yuv frame
            if (!early_termination) {
                TimeAutoCount counter(READ_SRC, collect_);
                frame = (uint8_t *)video_src_->get_next_frame();
            } else
                frame = nullptr;

            // send yuv frame to encoder
            {
                TimeAutoCount counter(ENCODING, collect_);
                if (frame != nullptr && frame_count) {
                    frame_count--;
                    // Fill in Buffers Header control data
                    av1enc_ctx_.input_picture_buffer->p_buffer = frame;
                    av1enc_ctx_.input_picture_buffer->n_filled_len =
                        video_src_->get_frame_size();
                    av1enc_ctx_.input_picture_buffer->flags = 0;
                    free_private_data_list(
                        av1enc_ctx_.input_picture_buffer->p_app_private);
                    gen_frame_event(
                        enc_setting,
                        video_src_->get_frame_index(),
                        &av1enc_ctx_.input_picture_buffer->p_app_private);
                    av1enc_ctx_.input_picture_buffer->pts =
                        video_src_->get_frame_index();
                    av1enc_ctx_.input_picture_buffer->pic_type =
                        EB_AV1_INVALID_PICTURE;
                    av1enc_ctx_.input_picture_buffer->qp =
                        video_src_->get_frame_qp(video_src_->get_frame_index());
                    av1enc_ctx_.input_picture_buffer->metadata = nullptr;
                    // Send the picture
                    EXPECT_EQ(EB_ErrorNone,
                              return_error = svt_av1_enc_send_picture(
                                  av1enc_ctx_.enc_handle,
                                  av1enc_ctx_.input_picture_buffer))
                        << "svt_av1_enc_send_picture error at: "
                        << av1enc_ctx_.input_picture_buffer->pts;
                }

                // send eos to encoder if this is last frame
                if (frame_count == 0 || frame == nullptr) {
                    src_file_eos = true;  // send eos only once
                    EbBufferHeaderType headerPtrLast;
                    headerPtrLast.n_alloc_len = 0;
                    headerPtrLast.n_filled_len = 0;
                    headerPtrLast.n_tick_count = 0;
                    headerPtrLast.p_app_private = nullptr;
                    headerPtrLast.flags = EB_BUFFERFLAG_EOS;
                    headerPtrLast.p_buffer = nullptr;
                    headerPtrLast.pic_type = EB_AV1_INVALID_PICTURE;
                    headerPtrLast.metadata = nullptr;
                    av1enc_ctx_.input_picture_buffer->flags = EB_BUFFERFLAG_EOS;
                    EXPECT_EQ(EB_ErrorNone,
                              return_error = svt_av1_enc_send_picture(
                                  av1enc_ctx_.enc_handle, &headerPtrLast))
                        << "svt_av1_enc_send_picture EOS error";
                }
            }
        }

        // get reconstructed frame
        if (recon_queue_ && !rec_file_eos) {
            TimeAutoCount counter(RECON, collect_);
            get_recon_frame(av1enc_ctx_, recon_queue_, rec_file_eos);
        }

        if (!enc_file_eos) {
            // try to get one encoded frame, flush the encoder
            // if src_file_eos is true
            do {
                // non-blocking call if not using low-delay
                EbBufferHeaderType *enc_out = nullptr;
                {
                    TimeAutoCount counter(ENCODING, collect_);
                    uint8_t pic_send_done =
                        (src_file_eos && rec_file_eos) ? 1 : 0;
                    return_error = svt_av1_enc_get_packet(
                        av1enc_ctx_.enc_handle, &enc_out, pic_send_done);
                    ASSERT_NE(return_error, EB_ErrorMax)
                        << "Error while encoding, code:" << enc_out->flags;
                }

                // process the output buffer
                if (return_error != EB_NoErrorEmptyQueue && enc_out) {
                    if (enc_out->flags & EB_BUFFERFLAG_EOS) {
                        enc_file_eos = true;
                        printf("Encoder EOS\n");
                    } else {
                        // send to reference decoder
                        TimeAutoCount counter(CONFORMANCE, collect_);
                        process_compress_data(enc_out);
                    }
                    // check if the process has encounter error, break out if
                    // true, like the recon frame does not match with decoded
                    // frame.
                    if (HasFatalFailure())
                        early_termination = true;
                } else {
                    if (return_error != EB_NoErrorEmptyQueue) {
                        enc_file_eos = true;
                        GTEST_FAIL() << "encoder return: " << return_error;
                    }
                    break;
                }

                // Release the output buffer
                if (enc_out != nullptr)
                    svt_av1_enc_release_out_buffer(&enc_out);
            } while (src_file_eos && !enc_file_eos);
        }  // if (!enc_file_eos)
    } while (!rec_file_eos || !src_file_eos || !enc_file_eos);

    /** complete the reference buffers in list comparison with recon */
    if (ref_compare_) {
        TimeAutoCount counter(CONFORMANCE, collect_);
        ASSERT_TRUE(ref_compare_->flush_video());
        delete ref_compare_;
        ref_compare_ = nullptr;
    }
}

void SvtAv1E2ETestFramework::run_test() {
    config_test();
    for (auto test_vector : enc_setting.test_vectors) {
        std::string fn = std::get<0>(test_vector);
        std::cout << "Start test case " << enc_setting.to_string(fn)
                  << std::endl;
        init_test(test_vector);
        EXPECT_NO_FATAL_FAILURE(run_encode_process())
            << "Fatal Error on running test case " << enc_setting.to_string(fn);
        post_process();
        deinit_test();
    }
}

void SvtAv1E2ETestFramework::run_death_test() {
    ::testing::FLAGS_gtest_death_test_style = "threadsafe";
    config_test();
    for (auto test_vector : enc_setting.test_vectors) {
        std::string fn = std::get<0>(test_vector);
        std::cout << "Start test case " << enc_setting.to_string(fn)
                  << std::endl;
        EXPECT_EXIT(
            {
                init_test(test_vector);
                run_encode_process();
                post_process();
                deinit_test();
                if (HasFatalFailure())
                    exit(-1);
                else
                    exit(0);
            },
            ::testing::ExitedWithCode(0),
            ".*")
            << "Fatal Error on running test case " << enc_setting.to_string(fn)
            << "\ncli command: " << enc_setting.to_cli(test_vector);
    }
}

void SvtAv1E2ETestFramework::write_output_header() {
    char header[IVF_STREAM_HEADER_SIZE];
    header[0] = 'D';
    header[1] = 'K';
    header[2] = 'I';
    header[3] = 'F';
    mem_put_le16(header + 4, 0);           // version
    mem_put_le16(header + 6, 32);          // header size
    mem_put_le32(header + 8, AV1_FOURCC);  // fourcc
    mem_put_le16(header + 12, av1enc_ctx_.enc_params.source_width);   // width
    mem_put_le16(header + 14, av1enc_ctx_.enc_params.source_height);  // height
    mem_put_le32(header + 16,
                 av1enc_ctx_.enc_params.frame_rate_numerator);  // rate
    mem_put_le32(header + 20,
                 av1enc_ctx_.enc_params.frame_rate_denominator);  // scale
    mem_put_le32(header + 24, 0);                                 // length
    mem_put_le32(header + 28, 0);                                 // unused
    if (output_file_ && output_file_->file)
        fwrite(header, 1, IVF_STREAM_HEADER_SIZE, output_file_->file);
}

static void write_ivf_frame_header(
    svt_av1_e2e_test::SvtAv1E2ETestFramework::IvfFile *ivf,
    uint32_t byte_count) {
    char header[IVF_FRAME_HEADER_SIZE];
    int32_t write_location = 0;

    mem_put_le32(&header[write_location], (int32_t)byte_count);
    write_location = write_location + 4;
    mem_put_le32(&header[write_location],
                 (int32_t)((ivf->ivf_count) & 0xFFFFFFFF));
    write_location = write_location + 4;
    mem_put_le32(&header[write_location], (int32_t)((ivf->ivf_count) >> 32));
    write_location = write_location + 4;

    ivf->byte_count_since_ivf = (byte_count);

    ivf->ivf_count++;
    fflush(stdout);

    if (ivf->file)
        fwrite(header, 1, IVF_FRAME_HEADER_SIZE, ivf->file);
}

void SvtAv1E2ETestFramework::write_compress_data(
    const EbBufferHeaderType *output) {
    write_ivf_frame_header(output_file_, output->n_filled_len);
    fwrite(output->p_buffer, 1, output->n_filled_len, output_file_->file);
}

void SvtAv1E2ETestFramework::process_compress_data(
    const EbBufferHeaderType *data) {
    ASSERT_NE(data, nullptr);
    if (refer_dec_ == nullptr) {
        if (output_file_)
            write_compress_data(data);
        return;
    }

    decode_compress_data(data->p_buffer, data->n_filled_len);
}

void SvtAv1E2ETestFramework::decode_compress_data(const uint8_t *data,
                                                  const uint32_t size) {
    ASSERT_NE(data, nullptr);
    ASSERT_GT(size, 0u);

    // input the compressed data into decoder
    ASSERT_EQ(refer_dec_->decode(data, size), RefDecoder::REF_CODEC_OK);

    VideoFrame ref_frame;
    while (refer_dec_->get_frame(ref_frame) == RefDecoder::REF_CODEC_OK) {
        if (recon_queue_) {
            // compare tools
            if (ref_compare_ == nullptr) {
                ref_compare_ =
                    create_ref_compare_queue(ref_frame, recon_queue_);
                ASSERT_NE(ref_compare_, nullptr);
            }
            // Compare ref decode output with recon output.
            ASSERT_TRUE(ref_compare_->compare_video(ref_frame))
                << "image compare failed on " << ref_frame.timestamp;

            // PSNR tool
            check_psnr(ref_frame);
        }
    }
}

void SvtAv1E2ETestFramework::check_psnr(const VideoFrame &frame) {
    // Calculate psnr with input frame and
    EbSvtIOFormat *src_frame =
        psnr_src_->get_frame_by_index((uint32_t)frame.timestamp);
    if (src_frame) {
        double luma_psnr = 0.0;
        double cb_psnr = 0.0;
        double cr_psnr = 0.0;
        psnr_frame(src_frame,
                   video_src_->get_bit_depth(),
                   frame,
                   luma_psnr,
                   cb_psnr,
                   cr_psnr);
        pnsr_statistics_.add(luma_psnr, cb_psnr, cr_psnr);
        // TODO: Check PSNR value reasonable here?
    }
}

static bool transfer_frame_planes(VideoFrame *frame) {
    if (frame->buf_size > VideoFrame::calculate_max_frame_size(*frame)) {
        printf("buffer size doesn't match!\n");
        return false;
    }

    uint32_t ss_y = 0;
    if (frame->format == IMG_FMT_420 ||
        frame->format == IMG_FMT_420P10_PACKED ||
        frame->format == IMG_FMT_I420) {
        ss_y = 1;
    }
    if (frame->stride[3]) {
        uint32_t offset = frame->stride[0] * frame->height +
                          ((frame->stride[1] + frame->stride[2]) *
                           ((frame->height + ss_y) >> ss_y));
        memcpy(frame->planes[3], frame->buffer + offset, frame->buf_size >> 2);
    }
    if (frame->stride[2]) {
        uint32_t offset = frame->stride[0] * frame->height +
                          (frame->stride[1] * ((frame->height + ss_y) >> ss_y));
        memcpy(frame->planes[2], frame->buffer + offset, frame->buf_size >> 2);
    }
    if (frame->stride[1]) {
        uint32_t offset = frame->stride[0] * frame->height;
        memcpy(frame->planes[1], frame->buffer + offset, frame->buf_size >> 2);
    }
    return true;
}

void SvtAv1E2ETestFramework::get_recon_frame(const SvtAv1Context &ctxt,
                                             FrameQueue *recon, bool &is_eos) {
    do {
        VideoFrame *new_frame = recon->get_empty_frame();
        ASSERT_NE(new_frame, nullptr)
            << "can not get new frame for recon frame!!";
        ASSERT_NE(new_frame->buffer, nullptr)
            << "can not get new buffer of recon frame!!";

        EbBufferHeaderType recon_frame{};
        recon_frame.size = sizeof(EbBufferHeaderType);
        recon_frame.p_buffer = new_frame->buffer;
        recon_frame.n_alloc_len = new_frame->buf_size;
        recon_frame.p_app_private = nullptr;
        recon_frame.metadata = nullptr;
        // non-blocking call until all input frames are sent
        EbErrorType recon_status =
            svt_av1_get_recon(ctxt.enc_handle, &recon_frame);
        ASSERT_NE(recon_status, EB_ErrorMax)
            << "Error while outputing recon, code:" << recon_frame.flags;
        if (recon_status == EB_NoErrorEmptyQueue) {
            recon->delete_frame(new_frame);
            break;
        } else {
            // adjust recon frame size if resize feature enabled in encoder
            if (recon_frame.n_filled_len != new_frame->buf_size &&
                recon_frame.metadata) {
                for (size_t i = 0; i < recon_frame.metadata->sz; i++) {
                    SvtMetadataT *metadata =
                        recon_frame.metadata->metadata_array[i];
                    ASSERT_NE(metadata, nullptr);
                    if (metadata->type == EB_AV1_METADATA_TYPE_FRAME_SIZE) {
                        ASSERT_EQ(metadata->sz, sizeof(SvtMetadataFrameSizeT));
                        SvtMetadataFrameSizeT *frame_size =
                            (SvtMetadataFrameSizeT *)metadata->payload;
                        new_frame->width = frame_size->width;
                        new_frame->height = frame_size->height;
                        new_frame->disp_width = frame_size->disp_width;
                        new_frame->disp_height = frame_size->disp_height;
                        new_frame->stride[0] = frame_size->stride;
                        new_frame->stride[2] = new_frame->stride[1] =
                            (frame_size->stride + frame_size->subsampling_x) >>
                            frame_size->subsampling_x;
                        new_frame->buf_size = recon_frame.n_filled_len;
                        break;
                    }
                }
            }
            ASSERT_LE(recon_frame.n_filled_len, new_frame->buf_size)
                << "recon frame size incorrect@" << recon_frame.pts;
            // mark the recon eos flag
            if (recon_frame.flags & EB_BUFFERFLAG_EOS)
                is_eos = true;
            transfer_frame_planes(new_frame);
            new_frame->timestamp = recon_frame.pts;
            recon->add_frame(new_frame);
        }
    } while (true);
}

SvtAv1E2ETestFramework::IvfFile::IvfFile(const std::string &path) {
    FOPEN(file, path.c_str(), "wb");
    byte_count_since_ivf = 0;
    ivf_count = 0;
}
