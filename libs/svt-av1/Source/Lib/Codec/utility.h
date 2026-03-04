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

#ifndef EbUtility_h
#define EbUtility_h

#include "definitions.h"
#include "common_dsp_rtcd.h"
#ifdef __cplusplus
extern "C" {
#endif
#include <limits.h>

/****************************
     * UTILITY FUNCTIONS
     ****************************/
typedef struct BlockList {
    uint8_t  list_size;
    uint16_t blk_mds_table[3]; //stores a max of 3 redundant blocks
} BlockList_t;

typedef enum GeomIndex {
    GEOM_0, //64x64  ->16x16  NSQ:OFF
    GEOM_1, //64x64  ->16x16  NSQ:ON (only H & V shapes, but not 16x8 and 8x16)
    GEOM_2, //64x64  ->8x8    NSQ:OFF
    GEOM_3, //64x64  ->8x8    NSQ:ON (only H & V shapes, but not 8x4 and 4x8 and not 16x8 and 8x16)
    GEOM_4, //64x64  ->8x8    NSQ:ON (only H & V shapes, but not 8x4 and 4x8)
    GEOM_5, //64x64  ->8x8    NSQ:ON (only H & V shapes)
    GEOM_6, //64x64  ->4x4    NSQ:ON (only H & V shapes)
    GEOM_7, //64x64  ->4x4    NSQ:ON (only H, V, H4, V4 shapes)
    GEOM_8, //64x64  ->4x4    NSQ:ON (all shapes)
    GEOM_9, //128x128->4x4    NSQ:ON (all shapes)
    GEOM_10, //128x128->8x8    NSQ:ON (only H, V, H4, V4 shapes)
    GEOM_TOT
} GeomIndex;

typedef struct BlockGeom {
    uint8_t sq_size; // size of parent square

    uint8_t   bwidth; // block width
    uint8_t   bheight; // block height
    uint8_t   bwidth_uv; // block width for Chroma 4:2:0
    uint8_t   bheight_uv; // block height for Chroma 4:2:0
    BlockSize bsize; // bloc size
    BlockSize bsize_uv; // bloc size for Chroma 4:2:0

    uint16_t d1_depth_offset; // offset to the next d1 sq block
    uint16_t ns_depth_offset; // offset to the next nsq block (skip remaining d2 blocks)
#if _DEBUG
    // when debugging, track the mds_idx for each block so can confirm we are using the
    // correct MDS when we get the BlkGeom. Should not be used in the code though.
    uint32_t mds_idx;
#endif
} BlockGeom;

void svt_aom_build_blk_geom(GeomIndex geom, BlockGeom* blk_geom_table);

static INLINE const BlockGeom* get_blk_geom_mds(const BlockGeom* blk_geom_table, uint32_t bidx_mds) {
    return &blk_geom_table[bidx_mds];
}

// CU Stats Helper Functions
typedef struct CodedBlockStats {
    uint8_t depth;
    uint8_t size;
    uint8_t size_log2;
    uint8_t org_x;
    uint8_t org_y;
    uint8_t cu_num_in_depth;
    uint8_t parent32x32_index;
} CodedBlockStats;

const CodedBlockStats* svt_aom_get_coded_blk_stats(const uint32_t cu_idx);

/****************************
     * MACROS
     ****************************/

#define MULTI_LINE_MACRO_BEGIN do {
#define MULTI_LINE_MACRO_END \
    }                        \
    while (0)

//**************************************************
// MACROS
//**************************************************
#define NOT_USED_VALUE 0
#define DIVIDE_AND_ROUND(x, y) (((x) + ((y) >> 1)) / (y))
#define DIVIDE_AND_CEIL(x, y) (((x) + ((y) - 1)) / (y))
#define MAX(x, y) ((x) > (y) ? (x) : (y))
#define MIN(x, y) ((x) < (y) ? (x) : (y))
#define MEDIAN(a, b, c)                   ((a)>(b)?(a)>?(b)>?(b)::(a):(b)>?(a)>?(a)::(b))
#define CLIP3(min_val, max_val, a) (((a) < (min_val)) ? (min_val) : (((a) > (max_val)) ? (max_val) : (a)))
#define CLIP3EQ(min_val, max_val, a) (((a) <= (min_val)) ? (min_val) : (((a) >= (max_val)) ? (max_val) : (a)))
#define BITDEPTH_MIDRANGE_VALUE(precision) (1 << ((precision) - 1))
#define SWAP(a, b)                    \
    MULTI_LINE_MACRO_BEGIN(a) ^= (b); \
    (b) ^= (a);                       \
    (a) ^= (b);                       \
    MULTI_LINE_MACRO_END
#define ABS(a) (((a) < 0) ? (-(a)) : (a))
#define EB_ABS_DIFF(a, b) ((a) > (b) ? ((a) - (b)) : ((b) - (a)))
#define EB_DIFF_SQR(a, b) (((a) - (b)) * ((a) - (b)))
#define SQR(x) ((x) * (x))
#define POW2(x) (1 << (x))
#define SIGN(a, b) (((a - b) < 0) ? (-1) : ((a - b) > 0) ? (1) : 0)
#define ROUND(a) (a >= 0) ? (a + 1 / 2) : (a - 1 / 2);
#define UNSIGNED_DEC(x)                                      \
    MULTI_LINE_MACRO_BEGIN(x) = (((x) > 0) ? ((x) - 1) : 0); \
    MULTI_LINE_MACRO_END
#define CIRCULAR_ADD(x, max) (((x) >= (max)) ? ((x) - (max)) : ((x) < 0) ? ((max) + (x)) : (x))
#define CIRCULAR_ADD_UNSIGNED(x, max) (((x) >= (max)) ? ((x) - (max)) : (x))
#define CEILING(x, base) ((((x) + (base) - 1) / (base)) * (base))
#define POW2_CHECK(x) ((x) == ((x) & (-((int32_t)(x)))))
#define ROUND_UP_MUL_8(x) ((x) + ((8 - ((x) & 0x7)) & 0x7))
#define ROUND_UP_MULT(x, mult) ((x) + (((mult) - ((x) & ((mult) - 1))) & ((mult) - 1)))

// rounds down to the next power of two
#define FLOOR_POW2(x)                        \
    MULTI_LINE_MACRO_BEGIN(x) |= ((x) >> 1); \
    (x) |= ((x) >> 2);                       \
    (x) |= ((x) >> 4);                       \
    (x) |= ((x) >> 8);                       \
    (x) |= ((x) >> 16);                      \
    (x) -= ((x) >> 1);                       \
    MULTI_LINE_MACRO_END

// rounds up to the next power of two
#define CEIL_POW2(x)                \
    MULTI_LINE_MACRO_BEGIN(x) -= 1; \
    (x) |= ((x) >> 1);              \
    (x) |= ((x) >> 2);              \
    (x) |= ((x) >> 4);              \
    (x) |= ((x) >> 8);              \
    (x) |= ((x) >> 16);             \
    (x) += 1;                       \
    MULTI_LINE_MACRO_END
#define LOG2F_8(x)              \
    (((x) < 0x0002u)       ? 0u \
         : ((x) < 0x0004u) ? 1u \
         : ((x) < 0x0008u) ? 2u \
         : ((x) < 0x0010u) ? 3u \
         : ((x) < 0x0020u) ? 4u \
         : ((x) < 0x0040u) ? 5u \
                           : 6u)

#define TWO_D_INDEX(x, y, stride) (((y) * (stride)) + (x))

// MAX_CU_COUNT is used to find the total number of partitions for the max partition depth and for
// each parent partition up to the root partition level (i.e. SB level).

// MAX_CU_COUNT is given by SUM from k=1 to n (4^(k-1)), reduces by using the following finite sum
// SUM from k=1 to n (q^(k-1)) = (q^n - 1)/(q-1) => (4^n - 1) / 3
#define MAX_CU_COUNT(max_depth_count) ((((1 << (max_depth_count)) * (1 << (max_depth_count))) - 1) / 3)

//**************************************************
// CONSTANTS
//**************************************************
#define MIN_UNSIGNED_VALUE 0
#define MAX_UNSIGNED_VALUE ~0u
#define MIN_SIGNED_VALUE ~0 - ((signed)(~0u >> 1))
#define MAX_SIGNED_VALUE ((signed)(~0u >> 1))
#define CONST_SQRT2 (1.4142135623730950488016887242097) /*sqrt(2)*/

#define MINI_GOP_MAX_COUNT 31
#define MINI_GOP_WINDOW_MAX_COUNT 16 // window subdivision: 16 x 2L

#define MIN_HIERARCHICAL_LEVEL 1
static const uint8_t mini_gop_offset[MAX_HIERARCHICAL_LEVEL - MIN_HIERARCHICAL_LEVEL] = {1, 3, 7, 15, 31};

typedef struct MiniGopStats {
    uint8_t hierarchical_levels;
    uint8_t start_index;
    uint8_t end_index;
    uint8_t length;
} MiniGopStats;

const MiniGopStats* svt_aom_get_mini_gop_stats(const uint32_t mini_gop_index);

typedef enum MinigopIndex {
    L6_INDEX    = 0,
    L5_0_INDEX  = 1,
    L4_0_INDEX  = 2,
    L3_0_INDEX  = 3,
    L2_0_INDEX  = 4,
    L2_1_INDEX  = 5,
    L3_1_INDEX  = 6,
    L2_2_INDEX  = 7,
    L2_3_INDEX  = 8,
    L4_1_INDEX  = 9,
    L3_2_INDEX  = 10,
    L2_4_INDEX  = 11,
    L2_5_INDEX  = 12,
    L3_3_INDEX  = 13,
    L2_6_INDEX  = 14,
    L2_7_INDEX  = 15,
    L5_1_INDEX  = 16,
    L4_2_INDEX  = 17,
    L3_4_INDEX  = 18,
    L2_8_INDEX  = 19,
    L2_9_INDEX  = 20,
    L3_5_INDEX  = 21,
    L2_10_INDEX = 22,
    L2_11_INDEX = 23,
    L4_3_INDEX  = 24,
    L3_6_INDEX  = 25,
    L2_12_INDEX = 26,
    L2_13_INDEX = 27,
    L3_7_INDEX  = 28,
    L2_14_INDEX = 29,
    L2_15_INDEX = 30
} MinigopIndex;

// Right shift that replicates gcc's implementation

static inline int gcc_right_shift(int a, unsigned shift) {
    if (!a) {
        return 0;
    }
    if (a > 0) {
        return a >> shift;
    }
    static const unsigned sbit = 1u << (sizeof(sbit) * CHAR_BIT - 1);
    a                          = (unsigned)a >> shift;
    while (shift) {
        a |= sbit >> shift--;
    }
    return a ^ sbit;
}

static INLINE int convert_to_trans_prec(int allow_hp, int coor) {
    if (allow_hp) {
        return ROUND_POWER_OF_TWO_SIGNED(coor, WARPEDMODEL_PREC_BITS - 3);
    } else {
        return ROUND_POWER_OF_TWO_SIGNED(coor, WARPEDMODEL_PREC_BITS - 2) * 2;
    }
}

/* Convert Floating Point to Fixed Point example: int32_t val_fp8 = FLOAT2FP(val_float, 8, int32_t) */
#define FLOAT2FP(x_float, base_move, fp_type) ((fp_type)((x_float) * (((fp_type)(1)) << (base_move))))

/* Convert Fixed Point to Floating Point example: double val = FP2FLOAT(val_fp8, 8, int32_t, double) */
#define FP2FLOAT(x_fp, base_move, fp_type, float_type) \
    ((((float_type)((fp_type)(x_fp))) / ((float_type)(((fp_type)1) << (base_move)))))

#ifndef FIXED_POINT_ASSERT_TEST
#if NDEBUG
#define FIXED_POINT_ASSERT_TEST 0
#else
#define FIXED_POINT_ASSERT_TEST 1
#endif
#endif

#if FIXED_POINT_ASSERT_TEST
void svt_fixed_point_test_breakpoint(char* file, unsigned line);
#define FP_ASSERT(expression)                                                                             \
    if (!(expression)) {                                                                                  \
        fprintf(stderr, "ERROR: FP_ASSERT Fixed Point overload %s:%u\n", __FILE__, (unsigned)(__LINE__)); \
        svt_fixed_point_test_breakpoint(__FILE__, (unsigned)(__LINE__));                                  \
        assert(0);                                                                                        \
        abort();                                                                                          \
    }
#else /*FIXED_POINT_ASSERT_TEST*/
#define FP_ASSERT(expression)
#endif

#ifdef __cplusplus
}
#endif

#endif // EbUtility_h
