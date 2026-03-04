/*
 * Copyright(c) 2019 Netflix, Inc.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef EbAV1Structs_h
#define EbAV1Structs_h

#include "pic_buffer_desc.h"
#include "segmentation_params.h"

#ifdef __cplusplus
extern "C" {
#endif

/*!\brief OBU types. */
typedef enum ATTRIBUTE_PACKED {
    OBU_SEQUENCE_HEADER        = 1,
    OBU_TEMPORAL_DELIMITER     = 2,
    OBU_FRAME_HEADER           = 3,
    OBU_TILE_GROUP             = 4,
    OBU_METADATA               = 5,
    OBU_FRAME                  = 6,
    OBU_REDUNDANT_FRAME_HEADER = 7,
    OBU_PADDING                = 15,
} ObuType;

typedef struct ObuHeader {
    /*!<Size (1 or 2 b) of OBU header (including optional OBU extension header) */
    size_t size;

    /*!<Must be set to 0*/
    uint8_t obu_forbidden_bit;

    /*!<Specifies the type of data structure contained in the OBU payload*/
    ObuType obu_type;

    /*!<Indicates if the optional obu_extension_header is present*/
    uint8_t obu_extension_flag;

    /*!< 1: indicates that the obu_size syntax element will be present
     *   0: Indicates that the obu_size syntax element will not be present*/
    uint8_t obu_has_size_field;

    /*!<Specifies the temporal level of the data contained in the OBU*/
    uint8_t temporal_id;

    /*!<Specifies the spatial level of the data contained in the OBU*/
    uint8_t spatial_id;

    size_t payload_size;

} ObuHeader;

typedef struct DecoderModelInfo {
    /*!< Specifies the length of the decoder_buffer_delay and the
     * encoder_buffer_delay syntax elements, in bits*/
    uint8_t buffer_delay_length_minus_1;

    /*!< Number of time units of a decoding clock operating at the frequency
     *time_scale Hz that corresponds to one increment of a clock tick counter*/
    uint32_t num_units_in_decoding_tick;

    /*!<Specifies the length of the buffer_removal_time syntax element,in bits*/
    uint8_t buffer_removal_time_length_minus_1;

    /*!< Specifies the length of the frame_presentation_time syntax element,
     * in bits*/
    uint8_t frame_presentation_time_length_minus_1;

} DecoderModelInfo;

typedef struct OrderHintInfo {
    /*!<1: Indicates that tools based on the values of order hints may be used.
     *  0: Indicates that tools based on order hints are disabled */
    uint8_t enable_order_hint;

    /*!< 1: Indicates that the distance weights process may be used for
     * inter prediction */
    uint8_t enable_jnt_comp;

    /*!< 1: Indicates that the use_ref_frame_mvs syntax element may be present.
     *   0: Indicates that the use_ref_frame_mvs syntax element will not be
            present */
    uint8_t enable_ref_frame_mvs;

    /*!< Used to compute OrderHintBits*/
    uint8_t order_hint_bits;

} OrderHintInfo;

typedef struct EbTimingInfo {
    /*!< Timing info present flag */
    bool timing_info_present;

    /*!< Number of time units of a clock operating at the frequency time_scale
     * Hz that corresponds to one increment of a clock tick counter*/
    uint32_t num_units_in_display_tick;

    /*!< Number of time units that pass in one second*/
    uint32_t time_scale;

    /*!< Equal to 1 indicates that pictures should be displayed according to
     * their output order with the number of ticks between two consecutive
     * pictures specified by num_ticks_per_picture.*/
    uint8_t equal_picture_interval;

    /*!< Specifies the number of clock ticks corresponding to output time
     * between two consecutive pictures in the output order.
     * Range - [0 to (1 << 32) - 2]*/
    uint32_t num_ticks_per_picture;

} EbTimingInfo;

typedef struct SeqHeader {
    /*!< Specifies the features that can be used in the coded video sequence */
    EbAv1SeqProfile seq_profile;

    /*!<1: Specifies that the coded video sequence contains only one coded frame
     *  0: Specifies that the coded video sequence contains one or more coded
           frames */
    uint8_t still_picture;

    /*!< Specifies that the syntax elements not needed by a still picture are
     * omitted */
    uint8_t reduced_still_picture_header;

    /*!< Timing Information structure*/
    EbTimingInfo timing_info;

    /*!< Specifies whether decoder model information is present in the coded
     * video sequence */
    uint8_t decoder_model_info_present_flag;

    /*!< Decoder Mode Information structure*/
    DecoderModelInfo decoder_model_info;

    /*!< Specifies whether initial display delay information is present in the
     * coded video sequence.*/
    uint8_t initial_display_delay_present_flag;

    /*!< Indicates the number of operating points minus 1 present in the coded video sequence*/
    uint8_t operating_points_cnt_minus_1;

    /*!< Operating Point Param structure*/
    EbAv1OperatingPoint operating_point[MAX_NUM_OPERATING_POINTS];

    /*!< Specifies the number of bits minus 1 used for transmitting the frame
     * width syntax elements */
    uint8_t frame_width_bits;

    /*!< Specifies the number of bits minus 1 used for transmitting the frame
     * height syntax elements*/
    uint8_t frame_height_bits;

    /*!< Specifies the maximum frame width for the frames represented by this sequence header
     * (equals to max_frame_width_minus_1 + 1, spec 5.5.1).
     * Actual frame width could be equal to or less than this value. E.g. Use this value to indicate
     * the maximum width between renditions when switch frame feature is on.
     * Use original un-aligned max width when writing seq header.*/
    uint16_t max_frame_width;

    /*!< Specifies the maximum frame height for the frames represented by this sequence header
     * (equals to max_frame_height_minus_1 + 1, spec 5.5.1).
     * Actual frame height could be equal to or less than this value. E.g. Use this value to indicate
     * the maximum height between renditions when switch frame feature is on.
     * Use original un-aligned max height when writing seq header.*/
    uint16_t max_frame_height;

    /*!< Specifies whether frame id numbers are present in the coded video
     * sequence*/
    uint8_t frame_id_numbers_present_flag;

    /*!< Specifies the number of bits used to encode delta_frame_id
     * syntax elements*/
    uint8_t delta_frame_id_length;

    /*!<Used to calculate the number of bits used to encode the frame_id syntax
     * element.*/
    uint8_t frame_id_length;

    /*!< 1: Indicates that superblocks contain 128x128 luma samples
     *   0: Indicates that superblocks contain 64x64 luma samples.*/
    uint8_t use_128x128_superblock;

    /*Size of the superblock used for this frame*/
    BlockSize sb_size;

    /*!< superblock size in 4x4 MI unit */
    uint8_t sb_mi_size;

    /*!< superblock size inlog2 unit */
    uint8_t sb_size_log2;

    /*!< 1: Specifies that the use_filter_intra syntax element may be present.
     *   0: Specifies that the use_filter_intra syntax element will not be
     *       present*/
    uint8_t filter_intra_level;

    /*!< Specifies whether the intra edge filtering process should be enabled */
    uint8_t enable_intra_edge_filter;

    /*!< Specifies whether the intra angle delta filtering process should be enabled */
    uint8_t enable_intra_angle_delta_filter;

    /*!<1: Specifies that the mode info for inter blocks may contain the syntax
     *     element interintra.
     *  0: Specifies that the syntax element interintra will not be present */
    uint8_t enable_interintra_compound;

    /*!<1: Specifies that the mode info for inter blocks may contain the syntax
     *     element compound_type
     *  0: Specifies that the syntax element compound_type will not be present*/
    uint8_t enable_masked_compound;

    /*!<1: Indicates that the allow_warped_motion syntax element may be present
     *  0: Indicates that the allow_warped_motion syntax element will not be
     *     present*/
    uint8_t enable_warped_motion;

    /*!< 1: Indicates that the inter prediction filter type may be specified
     *      independently in the horizontal and vertical directions.
     *   0: Indicates only one filter type may be specified, which is then used
     *      in both directions.*/
    uint8_t enable_dual_filter;

    /*!< Order Hint Information structure */
    OrderHintInfo order_hint_info;

    /*!<Equal to SELECT_SCREEN_CONTENT_TOOLS, indicates that the
     * allow_screen_content_tools syntax element will be present in the frame
     * header. Otherwise, seq_force_screen_content_tools contains the value for
     * allow_screen_content_tools*/
    uint8_t seq_force_screen_content_tools;

    /*!< Equal to SELECT_INTEGER_MV indicates that the force_integer_mv syntax
     * element will be present in the frame header (providing
     * allow_screen_content_tools is equal to 1). Otherwise, seq_force_integer_mv
     * contains the value for force_integer_mv */
    uint8_t seq_force_integer_mv;

    /*!< 1: Specifies that the use_superres syntax element will be present in
     *      the uncompressed header.
     *   0: Specifies that the use_superres syntax element will not be present*/
    uint8_t enable_superres;

    /*!< 1: Specifies that cdef filtering may be enabled.
         0: specifies that cdef filtering is disabled */
    uint8_t cdef_level;

    /*!< 1: Specifies that loop restoration filtering may be enabled.
         0: Specifies that loop restoration filtering is disabled*/
    uint8_t enable_restoration;

    /*!< Colour Configuration structure*/
    EbColorConfig color_config;

    /*!< Specifies whether film grain parameters are present in the coded video sequence*/
    uint8_t film_grain_params_present;

} SeqHeader;

typedef struct FrameSize {
    /*!< Width of the frame in luma samples */
    uint16_t frame_width;
    /*!< Height of the frame in luma samples */
    uint16_t frame_height;
    /*!< Render width of the frame in luma samples */
    uint16_t render_width;
    /*!< Render height of the frame in luma samples */
    uint16_t render_height;
    /*!< Denominator of a fraction that specifies the ratio between the
     * superblock width before and after upscaling*/
    uint8_t superres_denominator;
    /*!< Width of Upscaled SuperRes */
    uint16_t superres_upscaled_width;
    /*!< Height of Upscaled SuperRes */
    uint16_t superres_upscaled_height;
} FrameSize;

typedef struct TilesInfo {
    /*!< Specifies the maximum width that can be used for a tile */
    uint16_t max_tile_width_sb;

    /*!< Specifies the maximum height that can be used for a tile */
    uint16_t max_tile_height_sb;

    /*!< Specifies minimum of base 2 logarithm of tiles column across the frame */
    uint8_t min_log2_tile_cols;

    /*!< Specifies maximum of base 2 logarithm of tiles column across the frame */
    uint8_t max_log2_tile_cols;

    /*!< Specifies maximum of base 2 logarithm of tiles row down the frame */
    uint8_t max_log2_tile_rows;

    /*!< Specifies minimum of base 2 logarithm of tiles row down the frame */
    uint8_t min_log2_tile_rows;

    /*!< Specifies minimum of base 2 logarithm of tiles */
    uint8_t min_log2_tiles;

    /*!< 1: Indicates that the tiles are uniformly spaced across the frame
     *   0: Indicates that the tile sizes are coded*/
    uint8_t uniform_tile_spacing_flag;

    /*!< Specifies the number of tiles across the frame */
    uint8_t tile_cols;

    /*!< Specifies the number of tiles down the frame */
    uint8_t tile_rows;

    /*!< Specifies the base 2 logarithm of the desired number of tiles across the frame */
    uint8_t tile_cols_log2;

    /*!< Specifies the base 2 logarithm of the desired number of tiles down the frame */
    uint8_t tile_rows_log2;

    /*!< Specifying the start column (in units of 4x4 luma samples) for each tile
     * across the image */
    uint16_t tile_col_start_mi[MAX_TILE_ROWS + 1];

    /*!< Specifying the start row (in units of 4x4 luma samples) for each tile down the image */
    uint16_t tile_row_start_mi[MAX_TILE_COLS + 1];

    /*!< Specifies which tile to use for the CDF update */
    uint16_t context_update_tile_id;

    /*!< Used to compute TileSizeBytes */
    uint8_t tile_size_bytes;
} TilesInfo;

typedef struct QuantizationParams {
    /*!< Indicates the base frame qindex */
    uint8_t base_q_idx;
    /*!< Indicates the DC quantizer relative to base_q_idx - applicable for non-RC configuration(s) only*/
    int8_t delta_q_dc[MAX_MB_PLANE];
    /*!< Indicates the AC quantizer relative to base_q_idx - applicable for non-RC configuration(s) only*/
    int8_t delta_q_ac[MAX_MB_PLANE];
    /*!<Specifies that the quantizer matrix will be used to compute quantizers*/
    uint8_t using_qmatrix;
    /*!< Specifies the level in the quantizer matrix that should be used for
     * each plane decoding */
    uint8_t qm[MAX_MB_PLANE];
    /*!< qindex for every segment ID */
    uint8_t qindex[MAX_SEGMENTS];
} QuantizationParams;

typedef struct DeltaQParams {
    /*!< Specifies whether quantizer index delta values are present */
    uint8_t delta_q_present;

    /*!< Specifies the left shift which should be applied to decoded quantizer
     * index delta values*/
    uint8_t delta_q_res;
} DeltaQParams;

typedef struct DeltaLfParams {
    /*!< Specifies whether loop filter delta values are present */
    uint8_t delta_lf_present;

    /*!< Specifies the left shift which should be applied to decoded loop filter
     * delta values*/
    uint8_t delta_lf_res;

    /*!< 1: Specifies that separate loop filter deltas are sent for horizontal
     *      luma edges, vertical luma edges, the U edges, and the V edges
     *   0: Specifies that the same loop filter delta is used for all edges */
    uint8_t delta_lf_multi;
} DeltaLfParams;

typedef struct CdefParams {
    /*!< Controls the amount of damping in the deringing filter */
    uint8_t cdef_damping;
    /*!< Specifies the number of bits needed to specify which CDEF filter to
     *apply*/
    uint8_t cdef_bits;
    /*!< Specify the strength of the primary and secondary filter of Y plane */
    uint8_t cdef_y_strength[CDEF_MAX_STRENGTHS];
    /*!< Specify the strength of the primary and secondary filter of UV plane */
    uint8_t cdef_uv_strength[CDEF_MAX_STRENGTHS];
} CdefParams;

typedef struct LrParams {
    /*!< Specifies the type of restoration used for each plane */
    RestorationType frame_restoration_type;

    /*!< Specifies the size of loop restoration units in units of samples in
     * the current plane */
    uint16_t loop_restoration_size;

    /*!< Loop Restoration size in log2 unit */
    uint8_t lr_size_log2;
} LrParams;

typedef struct SkipModeInfo {
    /*!< Flag to check skip mode allowed or not */
    int skip_mode_allowed;

    /*!< 1: Specifies that the syntax element skip_mode will be present
    *   0: Specifies that skip_mode will not be used for this frame */
    int skip_mode_flag;

    /*!< ref_frame_idx_0 */
    int ref_frame_idx_0;

    /*!< ref_frame_idx_1 */
    int ref_frame_idx_1;

} SkipModeInfo;

typedef struct GlobalMotionParams {
    /*!< Specifies the transform type */
    TransformationType gm_type;

    /*!< Global motion parameter */
    int32_t gm_params[6];

} GlobalMotionParams;

typedef struct FrameHeader {
    /*!< 1: Indicates the frame indexed by frame_to_show_map_idx is to be output.
         0: Indicates that further processing is required */
    uint8_t show_existing_frame;

    /*!< Specifies the type of the frame */
    FrameType frame_type;

    /*!< 1: Specifies that this frame should be immediately output once decoded
         0: Specifies that this frame should not be immediately output */
    uint8_t show_frame;

    /*!< Specifies the presentation time of the frame in clock ticks DispCT
     * counted from the removal time of the last random access point for the
     * operating point that is being decoded */
    uint32_t frame_presentation_time;

    /*!< 1: Specifies that the frame may be output using the show_existing_frame
     *      mechanism
     *   0: Specifies that this frame will not be output using the
            show_existing_frame mechanism */
    uint8_t showable_frame;

    /*!< 1: Indicates that error resilient mode is enabled
     *   0: Indicates that error resilient mode is disabled */
    uint8_t error_resilient_mode;

    /*!< Specifies whether the CDF update in the symbol decoding process should
     * be disabled */
    uint8_t disable_cdf_update;

    /*!< 1: Indicates that intra blocks may use palette encoding
     *   0: Indicates that palette encoding is never used */
    uint8_t allow_screen_content_tools;

    /*!< 1: Specifies that motion vectors will always be integers
     *   0: Specifies that motion vectors can contain fractional bits */
    uint8_t force_integer_mv;

    /*!< Specifies the frame id number for the current frame */
    uint32_t current_frame_id;

    /*!< Used to compute OrderHint */
    uint32_t order_hint;

    /*!< Specifies which reference frame contains the CDF values and other
     * state that should be loaded at the start of the frame */
    uint8_t primary_ref_frame;

    /*!< 1: Specifies that buffer_removal_time is present.
         0: Specifies that buffer_removal_time is not present */
    uint8_t buffer_removal_time_present_flag;

    /*!< Specifies the frame removal time in units of DecCT clock ticks counted
     * from the removal time of the last random access point for operating
     * point op_num */
    uint32_t buffer_removal_time[MAX_NUM_OPERATING_POINTS];

    /*!< Specifies the length of the buffer_removal_time syntax element */
    uint8_t refresh_frame_flags;

    /*!< Specifies the expected output order hint for each reference frame */
    uint32_t ref_order_hint[REF_FRAMES];

    /*!< 1: Indicates that intra block copy may be used in this frame.
     *   0: Indicates that intra block copy is not allowed in this frame */
    uint8_t allow_intrabc;

    /*!< Specifies which reference frames are used by inter frames */
    uint8_t ref_frame_idx[REF_FRAMES];

    /*!< An array which is indexed by a reference picture slot number
     *  1: Signifies that the corresponding reference picture slot is valid for
     *     use as a reference picture
     *  0: Signifies that the corresponding reference picture slot is not valid
     *     for use as a reference picture*/
    uint32_t ref_valid[REF_FRAMES];

    /*!< Specifies the frame id for each reference frame */
    uint32_t ref_frame_id[REF_FRAMES];

    /*!< Frame Size structure */
    FrameSize frame_size;

    /*!< 0: Specifies that motion vectors are specified to quarter pel precision
     *   1: Specifies that motion vectors are specified to eighth pel precision*/
    uint8_t allow_high_precision_mv;

    /*!< Specifies the filter selection used for performing inter prediction */
    InterpFilter interpolation_filter;

    /*!< 0: Specifies that only the SIMPLE motion mode will be used */
    uint8_t is_motion_mode_switchable;

    /*!< 1: Specifies that motion vector information from a previous frame can
     * be used when decoding the current frame
     *   0: Specifies that this information will not be used */
    uint8_t use_ref_frame_mvs;

    /*!< 1: Indicates that the end of frame CDF update is disabled
     *   0: Indicates that the end of frame CDF update is enabled */
    uint32_t ref_frame_sign_bias[TOTAL_REFS_PER_FRAME];

    /*!< 1: Indicates that the end of frame CDF update is disabled
     *   0: Indicates that the end of frame CDF update is enabled */
    uint8_t disable_frame_end_update_cdf;

    /* Number of 4x4 block columns in the frame */
    uint32_t mi_cols;

    /* Number of 4x4 block rows in the frame */
    uint32_t mi_rows;

    /* Number of 4x4 block rows in the frame aligned to SB */
    uint32_t mi_stride;

    /*!< Tile information */
    TilesInfo tiles_info;

    /*!< Quantization Parameters */
    QuantizationParams quantization_params;

    /*!< Segmentation Parameters */
    SegmentationParams segmentation_params;

    /*!< Delta Quantization Parameters */
    DeltaQParams delta_q_params;

    /*!< Delta Loop Filter Parameters */
    DeltaLfParams delta_lf_params;

    /*!< Indicates that the frame is fully lossless at the coded resolution of
     * FrameWidth by FrameHeight */
    uint8_t coded_lossless;

    /*!< Indicates that the frame is fully lossless at the upscaled resolution*/
    uint8_t all_lossless;

    /*!< Indicates the flag to set coded_lossless variable */
    uint8_t lossless_array[MAX_SEGMENTS];

    /*!< Loop Filter Parameters */
    LoopFilter loop_filter_params;

    /*!< Constrained Directional Enhancement Filter */
    CdefParams cdef_params;

    /*!< Loop Restoration Parameters */
    LrParams lr_params[MAX_MB_PLANE];

    /*!< Specifies how the transform size is determined */
    TxMode tx_mode;

    /*!< Reference Mode structure */
    ReferenceMode reference_mode;

    /*!< Skip Mode Parameters*/
    SkipModeInfo skip_mode_params;

    /*!< 1: Indicates that the syntax element motion_mode may be present
     *   0: Indicates that the syntax element motion_mode will not be present*/
    uint8_t allow_warped_motion;

    /*!< 1, specifies that the frame is restricted to a reduced subset of the
     * full set of transform types */
    uint8_t reduced_tx_set;

    /*!< Global Motion Paramters */
    //GlobalMotionParams      global_motion_params[ALTREF_FRAME + 1];

    /*!< Film Grain Parameters */
    AomFilmGrain film_grain_params;

    int32_t frame_refs_short_signaling;
} FrameHeader;

typedef struct Dequant {
    DECLARE_ALIGNED(16, int16_t, dequant_qtx[MAX_SEGMENTS][MAX_MB_PLANE][2]); // 0: DC, 1: AC
} Dequant;

#ifdef __cplusplus
}
#endif
#endif // EbAV1Structs_h
