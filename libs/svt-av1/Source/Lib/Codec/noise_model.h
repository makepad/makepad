/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef AOM_AOM_DSP_NOISE_MODEL_H_
#define AOM_AOM_DSP_NOISE_MODEL_H_

#include "EbConfigMacros.h"

#if CONFIG_ENABLE_FILM_GRAIN

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

#include <stdint.h>
#include "grainSynthesis.h"
#include "pic_buffer_desc.h"
#include "object.h"

/*!\brief Wrapper of data required to represent linear system of eqns and soln.
     */
typedef struct {
    double* A;
    double* b;
    double* x;
    int32_t n;
} AomEquationSystem;

/*!\brief Representation of a piecewise linear curve
     *
     * Holds n points as (x, y) pairs, that store the curve.
     */
typedef struct {
    double (*points)[2];
    int32_t num_points;
} AomNoiseStrengthLut;

/*!\brief Init the noise strength lut with the given number of points*/
int32_t svt_aom_noise_strength_lut_init(AomNoiseStrengthLut* lut, int32_t num_points);

/*!\brief Frees the noise strength lut. */
void svt_aom_noise_strength_lut_free(AomNoiseStrengthLut* lut);

/*!\brief Helper struct to model noise strength as a function of intensity.
     *
     * Internally, this structure holds a representation of a linear system
     * of equations that models noise strength (standard deviation) as a
     * function of intensity. The mapping is initially stored using a
     * piecewise representation with evenly spaced bins that cover the entire
     * domain from [min_intensity, max_intensity]. Each observation (x,y) gives a
     * constraint of the form:
     *   y_{i} (1 - a) + y_{i+1} a = y
     * where y_{i} is the value of bin i and x_{i} <= x <= x_{i+1} and
     * a = x/(x_{i+1} - x{i}). The equation system holds the corresponding
     * normal equations.
     *
     * As there may be missing data, the solution is regularized to get a
     * complete set of values for the bins. A reduced representation after
     * solving can be obtained by getting the corresponding noise_strength_lut_t.
     */
typedef struct {
    AomEquationSystem eqns;
    double            min_intensity;
    double            max_intensity;
    int32_t           num_bins;
    int32_t           num_equations;
    double            total;
} AomNoiseStrengthSolver;

/*!\brief Initializes the noise solver with the given number of bins.
     *
     * Returns 0 if initialization fails.
     *
     * \param[in]  solver    The noise solver to be initialized.
     * \param[in]  num_bins  Number of bins to use in the internal representation.
     * \param[in]  bit_depth The bit depth used to derive {min,max}_intensity.
     */
int32_t svt_aom_noise_strength_solver_init(AomNoiseStrengthSolver* solver, int32_t num_bins, int32_t bit_depth);
/*!\brief Gets the x coordinate of bin i.
     *
     * \param[in]  i  The bin whose coordinate to query.
     */
double svt_aom_noise_strength_solver_get_center(const AomNoiseStrengthSolver* solver, int32_t i);

/*!\brief Add an observation of the block mean intensity to its noise strength.
     *
     * \param[in]  block_mean  The average block intensity,
     * \param[in]  noise_std   The observed noise strength.
     */
void svt_aom_noise_strength_solver_add_measurement(AomNoiseStrengthSolver* solver, double block_mean, double noise_std);

/*!\brief Solves the current set of equations for the noise strength. */
int32_t svt_aom_noise_strength_solver_solve(AomNoiseStrengthSolver* solver);

/*!\brief Fits a reduced piecewise linear lut to the internal solution
     *
     * \param[in] max_num_points  The maximum number of output points
     * \param[out] lut  The output piecewise linear lut.
     */
int32_t svt_aom_noise_strength_solver_fit_piecewise(const AomNoiseStrengthSolver* solver, int32_t max_num_points,
                                                    AomNoiseStrengthLut* lut);

/*!\brief Helper for holding precomputed data for finding flat blocks.
     *
     * Internally a block is modeled with a low-order polynomial model. A
     * planar model would be a bunch of equations like:
     * <[y_i x_i 1], [a_1, a_2, a_3]>  = b_i
     * for each point in the block. The system matrix A with row i as [y_i x_i 1]
     * is maintained as is the inverse, inv(A'*A), so that the plane parameters
     * can be fit for each block.
     */
typedef struct {
    double* at_a_inv;
    double* A;
    int32_t num_params; // The number of parameters used for internal low-order model
    int32_t block_size; // The block size the finder was initialized with
    double  normalization; // Normalization factor (1 / (2^(bit_depth) - 1))
    int32_t use_highbd; // Whether input data should be interpreted as uint16
} AomFlatBlockFinder;

/*!\brief Init the block_finder with the given block size, bit_depth */
int32_t svt_aom_flat_block_finder_init(AomFlatBlockFinder* block_finder, int32_t block_size, int32_t bit_depth,
                                       int32_t use_highbd);
void    svt_aom_flat_block_finder_free(AomFlatBlockFinder* block_finder);

/*!\brief Helper to extract a block and low order "planar" model. */
void svt_aom_flat_block_finder_extract_block_c(const AomFlatBlockFinder* block_finder, const uint8_t* const data,
                                               int32_t w, int32_t h, int32_t stride, int32_t offsx, int32_t offsy,
                                               double* plane, double* block);

/*!\brief Runs the flat block finder on the input data.
     *
     * Find flat blocks in the input image data. Returns a map of
     * flat_blocks, where the value of flat_blocks map will be non-zero
     * when a block is determined to be flat. A higher value indicates a bigger
     * confidence in the decision.
     */
int32_t svt_aom_flat_block_finder_run(const AomFlatBlockFinder* block_finder, const uint8_t* const data, int32_t w,
                                      int32_t h, int32_t stride, uint8_t* flat_blocks);

// The noise shape indicates the allowed coefficients in the AR model.
typedef enum { AOM_NOISE_SHAPE_DIAMOND = 0, AOM_NOISE_SHAPE_SQUARE = 1 } AomNoiseShape;

// The parameters of the noise model include the shape type, lag, the
// bit depth of the input images provided, and whether the input images
// will be using uint16 (or uint8) representation.
typedef struct {
    AomNoiseShape shape;
    int32_t       lag;
    int32_t       bit_depth;
    int32_t       use_highbd;
} AomNoiseModelParams;

/*!\brief State of a noise model estimate for a single channel.
     *
     * This contains a system of equations that can be used to solve
     * for the auto-regressive coefficients as well as a noise strength
     * solver that can be used to model noise strength as a function of
     * intensity.
     */
typedef struct {
    AomEquationSystem      eqns;
    AomNoiseStrengthSolver strength_solver;
    int32_t                num_observations; // The number of observations in the eqn system
    double                 ar_gain; // The gain of the current AR filter
} AomNoiseState;

/*!\brief Complete model of noise for a planar video
     *
     * This includes a noise model for the latest frame and an aggregated
     * estimate over all previous frames that had similar parameters.
     */
typedef struct {
    AomNoiseModelParams params;
    AomNoiseState       combined_state[3]; // Combined state per channel
    AomNoiseState       latest_state[3]; // Latest state per channel
    int32_t (*coords)[2]; // Offsets (x,y) of the coefficient samples
    int32_t n; // Number of parameters (size of coords)
    int32_t bit_depth;
} AomNoiseModel;

/*!\brief Result of a noise model update. */
typedef enum {
    AOM_NOISE_STATUS_OK = 0,
    AOM_NOISE_STATUS_INVALID_ARGUMENT,
    AOM_NOISE_STATUS_INSUFFICIENT_FLAT_BLOCKS,
    AOM_NOISE_STATUS_DIFFERENT_NOISE_TYPE,
    AOM_NOISE_STATUS_INTERNAL_ERROR,
    AOM_NOISE_STATUS_INSUFFICIENT_NOISE_PIXELS,
} AomNoiseStatus;

/************************************
     * DenoiseAndModelInitData
     ************************************/
typedef struct DenoiseAndModelInitData {
    uint16_t noise_level;
    uint32_t encoder_bit_depth;
    uint32_t encoder_color_format;

    uint16_t width;
    uint16_t height;
    uint16_t stride_y;
    uint16_t stride_cb;
    uint16_t stride_cr;
    uint8_t  denoise_apply;
    bool     adaptive_film_grain;
} DenoiseAndModelInitData;

typedef struct AomDenoiseAndModel {
    EbDctor dctor;
    int32_t block_size;
    int32_t bit_depth;
    float   noise_level;

    // Size of current denoised buffer and flat_block buffer
    int32_t width;
    int32_t height;
    int32_t y_stride;
    int32_t uv_stride;
    int32_t num_blocks_w;
    int32_t num_blocks_h;

    // Buffers for image and noise_psd allocated on the fly
    float                noise_psd[3];
    uint8_t*             denoised[3];
    uint8_t*             flat_blocks;
    uint16_t*            packed[3];
    EbPictureBufferDesc* denoised_pic;
    EbPictureBufferDesc* packed_pic;

    AomFlatBlockFinder flat_block_finder;
    AomNoiseModel      noise_model;
    uint8_t            denoise_apply;
} AomDenoiseAndModel;

/************************************
     * denoise and model constructor
     ************************************/
EbErrorType svt_aom_denoise_and_model_ctor(AomDenoiseAndModel* object_ptr, EbPtr object_init_data_ptr);

/*!\brief Initializes a noise model with the given parameters.
     *
     * Returns 0 on failure.
     */
int32_t svt_aom_noise_model_init(AomNoiseModel* model, const AomNoiseModelParams params);
void    svt_aom_noise_model_free(AomNoiseModel* model);

/*\brief Save the "latest" estimate into the "combined" estimate.
     *
     * This is meant to be called when the noise modeling detected a change
     * in parameters (or for example, if a user wanted to reset estimation at
     * a shot boundary).
     */
void svt_aom_noise_model_save_latest(AomNoiseModel* noise_model);

/*!\brief Converts the noise_model parameters to the corresponding
     *    grain_parameters.
     *
     * The noise structs in this file are suitable for estimation (e.g., using
     * floats), but the grain parameters in the Bitstream are quantized. This
     * function does the conversion by selecting the correct quantization levels.
     */
int32_t svt_aom_noise_model_get_grain_parameters(AomNoiseModel* const noise_model, AomFilmGrain* film_grain);

/*!\brief Perform a Wiener filter denoising in 2D using the provided noise psd.
     *
     * \param[in]     data            Raw frame data
     * \param[out]    denoised        Denoised frame data
     * \param[in]     w               Frame width
     * \param[in]     h               Frame height
     * \param[in]     stride          Stride of the planes
     * \param[in]     chroma_sub_log2 Chroma subsampling for planes != 0.
     * \param[in]     noise_psd       The power spectral density of the noise
     * \param[in]     block_size      The size of blocks
     * \param[in]     bit_depth       Bit depth of the image
     * \param[in]     use_highbd      If true, uint8 pointers are interpreted as
     *                                uint16 and stride is measured in uint16.
     *                                This must be true when bit_depth >= 10.
     */
int32_t svt_aom_wiener_denoise_2d(const uint8_t* const data[3], uint8_t* denoised[3], int32_t w, int32_t h,
                                  int32_t stride[3], int32_t chroma_sub_log2[2], float noise_psd[3], int32_t block_size,
                                  int32_t bit_depth, int32_t use_highbd);

struct AomDenoiseAndModel;

/*!\brief Denoise the buffer and model the residual noise.
     *
     * This is meant to be called sequentially on input frames. The input buffer
     * is denoised and the residual noise is modelled. The current noise estimate
     * is populated in film_grain. Returns true on success. The grain.apply_grain
     * parameter will be true when the input buffer was successfully denoised and
     * grain was modelled. Returns false on error.
     *
     * \param[in]       ctx  Struct that holds some buffers for denoising and the
     *                       current noise estimate.
     * \param[in/out]   buf  The raw input buffer to be denoised.
     * \param[out]    grain  Output film grain parameters
     */
int32_t svt_aom_denoise_and_model_run(struct AomDenoiseAndModel* ctx, EbPictureBufferDesc* sd, AomFilmGrain* film_grain,
                                      int32_t use_highbd);

/*!\brief Allocates a context that can be used for denoising and noise modeling.
     *
     * \param[in]  bit_depth   Bit depth of buffers this will be run on.
     * \param[in]  block_size  Block size for noise modeling and flat block
     *                         estimation
     * \param[in]  noise_level The noise_level (2.5 for moderate noise, and 5 for
     *                         higher levels of noise)
     */

int32_t is_ref_noise_model_different(AomNoiseModel* const noise_model, AomNoiseModel* const ref_noise_model);

// Matrix multiply
static INLINE void multiply_mat_1_n_3(const double* m1, const double* m2, double* res, const int32_t inner_dim) {
    double  sum0 = 0, sum1 = 0, sum2 = 0;
    int32_t inner_m3 = 0;

    for (int32_t inner = 0; inner < inner_dim; ++inner, inner_m3 += 3) {
        const double m1_inner = m1[inner];
        sum0 += m1_inner * m2[inner_m3 + 0];
        sum1 += m1_inner * m2[inner_m3 + 1];
        sum2 += m1_inner * m2[inner_m3 + 2];
    }

    *(res++) = sum0;
    *(res++) = sum1;
    *(res++) = sum2;
}

static INLINE void multiply_mat_3_3_1(const double* m1, const double* m2, double* res) {
    double sum0, sum1, sum2;

    sum0 = m1[0 * 3 + 0] * m2[0 * 1 + 0];
    sum0 += m1[0 * 3 + 1] * m2[1 * 1 + 0];
    sum0 += m1[0 * 3 + 2] * m2[2 * 1 + 0];
    *(res++) = sum0;

    sum1 = m1[1 * 3 + 0] * m2[0 * 1 + 0];
    sum1 += m1[1 * 3 + 1] * m2[1 * 1 + 0];
    sum1 += m1[1 * 3 + 2] * m2[2 * 1 + 0];
    *(res++) = sum1;

    sum2 = m1[2 * 3 + 0] * m2[0 * 1 + 0];
    sum2 += m1[2 * 3 + 1] * m2[1 * 1 + 0];
    sum2 += m1[2 * 3 + 2] * m2[2 * 1 + 0];
    *(res++) = sum2;
}

static INLINE void multiply_mat_n_3_1(const double* m1, const double* m2, double* res, const int32_t m1_rows) {
    int32_t row_m3 = 0;

    for (int32_t row = 0; row < m1_rows; ++row, row_m3 += 3) {
        double sum = m1[row_m3 + 0] * m2[0 * 1 + 0];
        sum += m1[row_m3 + 1] * m2[1 * 1 + 0];
        sum += m1[row_m3 + 2] * m2[2 * 1 + 0];
        *(res++) = sum;
    }
}
#ifdef __cplusplus
} // extern "C"
#endif // __cplusplus

#endif // CONFIG_ENABLE_FILM_GRAIN

#endif // AOM_AOM_DSP_NOISE_MODEL_H_
