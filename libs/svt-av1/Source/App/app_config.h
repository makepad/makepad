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

#ifndef EbAppConfig_h
#define EbAppConfig_h

#include <stdio.h>
#include <stdbool.h>

#include "EbSvtAv1Enc.h"

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#define fseeko _fseeki64
#define ftello _ftelli64
#endif

// Define Cross-Platform 64-bit fseek() and ftell()
/** The AppExitConditionType type is used to define the App main loop exit
conditions.
*/
typedef enum AppExitConditionType {
    APP_ExitConditionNone = 0,
    APP_ExitConditionFinished,
    APP_ExitConditionError
} AppExitConditionType;

/** The AppPortActiveType type is used to define the state of output ports in
the App.
*/
typedef enum AppPortActiveType { APP_PortActive = 0, APP_PortInactive } AppPortActiveType;

typedef enum EncPass {
    ENC_SINGLE_PASS, //single pass mode
    ENC_FIRST_PASS, // first pass of multi pass mode
    ENC_SECOND_PASS, // second pass of multi pass mode
    MAX_ENC_PASS = 2,
} EncPass;

#define WARNING_LENGTH 100

#define MAX_NUM_TOKENS 210

#ifdef _WIN32
#define FOPEN(f, s, m) fopen_s(&f, s, m)
#else
#define FOPEN(f, s, m) f = fopen(s, m)
#endif

typedef struct EbPerformanceContext {
    /****************************************
     * Computational Performance Data
     ****************************************/
    uint64_t lib_start_time[2]; // [sec, micro_sec] including init time
    uint64_t encode_start_time[2]; // [sec, micro_sec] first frame sent

    double total_execution_time; // includes init
    double total_encode_time; // not including init

    uint64_t total_latency;
    uint32_t max_latency;
    uint64_t starts_time;
    uint64_t startu_time;
    uint64_t frame_count;

    double average_speed;
    double average_latency;

    uint64_t byte_count;
    double   sum_luma_psnr;
    double   sum_cr_psnr;
    double   sum_cb_psnr;

    double sum_luma_sse;
    double sum_cr_sse;
    double sum_cb_sse;

    double sum_luma_ssim;
    double sum_cr_ssim;
    double sum_cb_ssim;

    uint64_t sum_qp;

    double vbv_buffer_bits;
    double vbv_delay_max_s;
    double vbv_delay_sum_s;
    int    vbv_delay_violations;

} EbPerformanceContext;

typedef struct MemMapFile {
    bool     allow; //allow mem mapped file
    bool     enable; //enable mem mapped file
    size_t   file_size; //size of the input file
    uint64_t align_mask; //page size alignment mask
    int      fd; //file descriptor
    size_t   y4m_seq_hdr; //y4m seq length
    size_t   y4m_frm_hdr; //y4m frame length
    uint64_t file_frame_it; //frame iterator within the input file
#ifdef _WIN32
    HANDLE map_handle; //file mapping handle
#endif
    uint64_t cur_offset; // the current offset from the file start
} MemMapFile;

// list of frames that are forced to be key frames
// pairs with force_key_frames
struct forced_key_frames {
    char**    specifiers; // list of specifiers to use to convert into frames
    uint64_t* frames;
    size_t    count;
};

typedef struct EbConfig {
    /****************************************
     * File I/O
     ****************************************/
    FILE*      input_file;
    MemMapFile mmap; //memory mapped file handler
    bool       input_file_is_fifo;
    FILE*      bitstream_file;
    FILE*      recon_file;
    FILE*      error_log_file;
    FILE*      stat_file;
    FILE*      qp_file;
    /* two pass */
    const char* stats;
    FILE*       input_stat_file;
    FILE*       output_stat_file;
    bool        y4m_input;
    char        y4m_buf[9];

    uint8_t progress; // 0 = no progress output, 1 = normal, 2 = detailed progress
    /****************************************
     * Computational Performance Data
     ****************************************/
    EbPerformanceContext performance_context;

    uint32_t input_padded_width;
    uint32_t input_padded_height;
    // -1 indicates unknown (auto-detect at earliest opportunity)
    // auto-detect is performed on load for files and at end of stream for pipes
    int64_t frames_to_be_encoded;
    int64_t frames_to_be_skipped;
    bool    need_to_skip;

    int32_t   frames_encoded;
    int32_t   buffered_input;
    uint8_t** sequence_buffer;

    uint32_t injector_frame_rate;
    uint32_t injector;
    uint32_t speed_control_flag;

    bool stop_encoder; // to signal CTRL+C Event, need to stop encoding.

    uint64_t processed_frame_count;
    uint64_t processed_byte_count;

    uint64_t ivf_count;

    struct forced_key_frames forced_keyframes;

    /****************************************
     * On-the-fly Testing
     ****************************************/
    bool eos_flag;

    EbSvtAv1EncConfiguration config;
    // Output Ports Active Flags
    AppPortActiveType output_stream_port_active;

    // Component Handle
    EbComponentType* svt_encoder_handle;

    // Buffer Pools
    EbBufferHeaderType* input_buffer_pool;
    EbBufferHeaderType* recon_buffer;

    FILE*         roi_map_file;
    SvtAv1RoiMap* roi_map;

    char* fgs_table_path;
} EbConfig;

typedef struct EncChannel {
    EbConfig*            app_cfg; // Encoder Configuration
    EbErrorType          return_error; // Error Handling
    AppExitConditionType exit_cond_output; // Processing loop exit condition
    AppExitConditionType exit_cond_recon; // Processing loop exit condition
    AppExitConditionType exit_cond_input; // Processing loop exit condition
    AppExitConditionType exit_cond; // Processing loop exit condition
    bool                 active;
} EncChannel;

typedef enum MultiPassModes {
    SINGLE_PASS, //single pass mode
    TWO_PASS, // two pass: Same Pred + final
} MultiPassModes;

typedef struct EncApp {
    SvtAv1FixedBuf rc_twopasses_stats;
} EncApp;

EbConfig* svt_config_ctor();
void      svt_config_dtor(EbConfig* app_cfg);

EbErrorType     enc_channel_ctor(EncChannel* c);
void            enc_channel_dctor(EncChannel* c);
EbErrorType     read_command_line(int32_t argc, char* const argv[], EncChannel* channel);
int             get_version(int argc, char* const argv[]);
extern uint32_t get_help(int32_t argc, char* const argv[]);
extern uint32_t get_color_help(int32_t argc, char* const argv[]);
uint32_t        get_passes(int32_t argc, char* const argv[], EncPass enc_pass[MAX_ENC_PASS]);
EbErrorType     handle_stats_file(EbConfig* app_cfg, EncPass pass, const SvtAv1FixedBuf* rc_stats_buffer);
#endif //EbAppConfig_h
