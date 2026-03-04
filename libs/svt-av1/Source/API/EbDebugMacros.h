/*
* Copyright(c) 2020 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

/*
* This file contains only debug macros that are used during the development
* and are supposed to be cleaned up every tag cycle
* all macros must have the following format:
* - adding a new feature should be prefixed by FTR_
* - tuning a feature should be prefixed by TUNE_
* - enabling a feature should be prefixed by EN_
* - disabling a feature should be prefixed by DIS_
* - bug fixes should be prefixed by FIX_
* - code refactors should be prefixed by RFCTR_
* - code cleanups should be prefixed by CLN_
* - optimizations should be prefixed by OPT_
* - all macros must have a coherent comment explaining what the MACRO is doing
* - #if 0 / #if 1 are not to be used
*/

#ifndef EbDebugMacros_h
#define EbDebugMacros_h

// clang-format off

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

#define FTR_INTRA_COEFF_LVL         1 // Add coeff-level for INTRA frames using average input variance and input QP.
#define FTR_VLPD0                   1 // Use a variance-based cost model directly from spatial statistics
#define OPT_RDOQ_BIS                1 // Replace the area-based cutoff with an adaptive cutoff that uses both transform size and the coded coefficient length (EOB) towards reducing over-pruning on textured blocks
#define OPT_PER_BLK_INTRA           1 // Use block and sub-block variances (spread_var) to apply DC-only for only uniform blocks
#define CLN_REMOVE_VAR_SUB_DEPTH    1 // Remove var-sub-depth skip
#define CLN_DR                      1 // Remove depth-removal
#define OPT_DLF                     1 // Bypass DLF application when CDEF, Restoration, and Reconstruction are all disabled
#define OPT_RATE_ESTIMATION         1 // Opt rate estimation
#define FIX_INTRA_BC_CONFORMANCE    1 // Fix a conformance issue when intra-BC is ON and Palette is OFF
#define CLN_INTRABC_LEVEL_DEF       1 // Fix intra-BC level definitions
#define OPT_SC_ALLINTRA_DETECTION   1 // Update IQ-tune SC detection to support SC classes 4 and 5.

#define FTR_NIC_DREFI_NEW_LVL_DEFS  1 // New NIC/Depth-refinement levels

#define TUNE_STILL_IMAGE            1 // Tune still-image coding and reduce presets to nine: from M0 to M9

#define OPT_Q_CDEF                  1 // Derive CDEF strengh(s) from QP

#define OPT_FD2_ALLINTRA            1 // Tune fd2
#define TUNE_M7_ALLINTRA            1 // Fix M7

#define FIX_IS_DC_ONLY_SAFE         1 // Skip the spread check for 8x8 blocks when determining whether DC-only is safe, as 4x4 sub-block variance is unavailable
//FOR DEBUGGING - Do not remove
#define LOG_ENC_DONE            0 // log encoder job one
#define DEBUG_TPL               0 // Prints to debug TPL
#define DETAILED_FRAME_OUTPUT   0 // Prints detailed frame output from the library for debugging
#define DEBUG_BUFFERS           0 // Print process count and segments info
#define TUNE_CHROMA_SSIM        0 // Allows for Chroma and SSIM BDR-based Tuning
#define TUNE_CQP_CHROMA_SSIM    0 // Tune CQP qp scaling towards improved chroma and SSIM BDR

#define MIN_PIC_PARALLELIZATION 0 // Use the minimum amount of picture parallelization
#define SRM_REPORT              0 // Report SRM status
#define LAD_MG_PRINT            0 // Report LAD
#define RC_NO_R2R               0 // This is a debugging flag for RC and makes encoder to run with no R2R in RC mode
                                  // Note that the speed might impacted significantly
#if !RC_NO_R2R
#define FTR_KF_ON_FLY_SAMPLE         0 // Sample code to signal KF
#define FTR_RES_ON_FLY_SAMPLE        0 // Sample functions to change the resolution on the fly
#define FTR_RATE_ON_FLY_SAMPLE       0 // Sample functions to change bit rate
#define FTR_FRAME_RATE_ON_FLY_SAMPLE 0 // Sample functions to change frame rate
#define FTR_PER_FRAME_QUALITY_SAMPLE 0 // Sample functions to compute PSNR per frame
#endif
// Super-resolution debugging code
#define DEBUG_SCALING           0
#define DEBUG_TF                0
#define DEBUG_SUPERRES_RECODE   0
#define DEBUG_SUPERRES_ENERGY   0
#define DEBUG_RC_CAP_LOG        0 // Prints for RC cap

// Switch frame debugging code
#define DEBUG_SFRAME            0

// Variance Boost debugging code
#define DEBUG_VAR_BOOST         0
#define DEBUG_VAR_BOOST_QP      0
#define DEBUG_VAR_BOOST_STATS   0

// Anti-alias aware screen content mode debugging code
#define DEBUG_AA_SCM            0

// QP scaling debugging code
#define DEBUG_QP_SCALING        0

// Quantization matrices
#define DEBUG_QM_LEVEL          0
#define DEBUG_STARTUP_MG_SIZE   0
#define DEBUG_SEGMENT_QP        0
#define DEBUG_ROI               0
#ifdef __cplusplus
}
#endif // __cplusplus

// clang-format on

#endif // EbDebugMacros_h
