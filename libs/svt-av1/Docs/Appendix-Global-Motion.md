[Top level](../README.md)

# Global Motion Compensation

## 1. Description of the algorithm

Global motion compensation concerns the estimation and compensation of
motion characteristics that affect the whole frame, as for example in
video clips shot using a hand-held camera. In the example shown in the
Figure 1 below, matched features in the two images below indicate a
translation and rotation motion between the two pictures. In general,
the key steps involved in estimating global motion comprise identifying
features in both images, matching the identified features, and
estimating global motion parameters based on the matched features.

![gm_fig1](./img/gm_fig1.svg)

##### Figure 1. Example of global motion between two frames involving translation and rotation motion.

The general motion model is given by:

$`\begin{bmatrix} x_r \\  y_r \\ 1 \\
\end{bmatrix} = \begin{bmatrix} h_{11} & h_{12} & h_{13} \\  h_{21} & h_{22} &
h_{23} \\ h_{31} & h_{32} & h_{33} \\
\end{bmatrix} \begin{bmatrix} x \\  y \\ 1 \\
\end{bmatrix}`$

where $`\begin{bmatrix} x \\ y\end{bmatrix}`$  and $`\begin{bmatrix} x_r \\ y_r \end{bmatrix}`$ are the pixel coordinates in the
current and reference
frames respectively. The supported motion models include:

  - Affine projection: $`h_{31} = h_{32} = 0, h_{33}=1`$ . This
    transformation preserves parallelism and has six parameters to estimate.

  - Rotation-zoom projection: $`h_{31} = h_{32} = 0, h_{33}=1; h_{11}=h_{22};h_{12}=-h_{21}`$, which
    corresponds rotation + scaling. This transformation preserves
    angles and has four parameters to estimate.

  - Translation: $`h_{31} = h_{32} = 0, h_{33}=1; h_{11}=h_{22}=1;h_{12}=h_{21}=0`$. This transformation
    preserves orientation and size and has two parameters to estimate.

The global motion estimation involves two main steps. The first step
concerns feature matching where the objective is to identify features
that are present in both the source and reference pictures. The second
step concerns model identification, where the identified features are
used to estimate the motion model parameters. In SVT-AV1, the global
motion parameters are computed for each reference frame using feature
matching followed by applying the random sample consensus (RANSAC) algorithm. The
estimated parameters are sent in the bitstream.

### Feature matching

To identify features that are common to both the source and reference
pictures, the features from the Accelerated Segment Test (FAST) algorithm are
used as a feature detector. The Fast algorithm identifies *corner
points* by examining a circle of 16 pixels (Brensenhan circle of radius
3) around the pixel p of interest. If out of the 16 pixels, 12
contiguous pixels all have values above the pixel p by at least a given
threshold or all have values below that of p by at least a given
threshold, then the pixel is considered a feature (corner point) in the
image. Such features are robust to motion and brightness changes. Once
features on the source frame and on the reference frame are identified,
feature matching is performed by computing the normalized
cross-correlation function between the two sets of features. A feature
(i.e. corner point) is selected if:

  - The feature on the reference frame is located within a pre-specified
    distance from the feature in the source frame.

  - The correlation between the point in the reference frame and that in
    the source frame is highest.

### Model identification

The model is identified based on the matched feature points from the
feature matching step. A least squares estimation is performed to
compute the model parameters using the matched feature points. The
RANSAC (Random Sample Consensus) algorithm is used in the estimation.
The algorithm minimizes the impact of noise and outliers in the data.
The set of parameters to be estimated depends on the motion model
(Translation, rotation-zoom, affine) specified. The identified
parameters are included in the bitstream.

The RANSAC algorithm finds model parameters that yield the best match to
the motion of the identified features. The steps involved in the
algorithm are as follows:

  - A small number of matched features (corner points) are used in the
    model parameter estimation (as dictated by the number of
    parameters to estimate).

  - The remaining features are used to evaluate the fitness of the
    model by counting the number of those matched features where the
    model yields a small error (inliers). The remaining tested
    features are considered outliers.

  - Steps 1 and 2 are repeated based on another small set of matched
    features and the number of resulting outliers is recorded.

  - The process stops when the number of outliers is below a specified
    threshold.


## 2. Implementation of the algorithm

### 2.1. Global Motion inputs/outputs

**Input to Motion Estimation**: Input frames of the stream.
**Outputs of Motion Estimation**: Estimated global motion models per frame with their references.
**Input to Mode Decision**: Estimated global motion models.
**Outputs of Mode Decision**: Encoded frame with global motion encoded blocks if they provide a cost advantage.

### 2.2 Global Motion API

Table 1 below summarises the invoked functions when global motion is enabled.
The process where each function is invoked is also indicated as well as a brief
description of each function.

##### Table 1. Global motion estimation API.

| **Process**                         | **Function**             | **Purpose**                                                                                                                                    |
| ---                                 | ---                      | ---                                                                                                                                            |
| Picture Decision Process            | svt_aom_set_gm_controls  | Set global motion controls                                                                                                                     |
| Motion Estimation Process           | perform_gm_detection     | Detect whether a global motion may be identified based on the uniformity of the motion vectors produced by the normal motion estimation search |
| Motion Estimation Process           | svt_aom_global_motion_estimation | Perform global motion estimation search                                                                                                |
| Mode Decision Configuration process | set_global_motion_field  | Map the global motion information generated in EbMotionEstimationProcess to EbEncDecProcess                                                    |
| Mode Decision Process               | inject_global_candidates | Inject global motion as a mode candidate to the mode decision                                                                                  |

### Details of the implementation

The global motion data flow is summarized in the Figure 2 below.

![gm_fig2](./img/gm_fig2.svg)

##### Figure 2. Global motion data flow in the encoder pipeline.

The main algorithmic components of the global motion feature are the estimation
component which takes place in the motion estimation process, and the injection
and processing component which takes place in the Mode Decision
process(injection and processing).

### Global motion estimation

This process is executed by the ```global_motion_estimation``` function. This function
is called only for the first segment of each frame in the ```motion_estimation_kernel```
but it computes the global motion for the whole frame. The function involves a loop
that runs over all reference frames.

To compute the global motion between two frames, the FAST features of the reference
frames are extracted and matched to those of the current frame in the ```svt_av1_fast_corner_detect```
function, thanks to the fastfeat third-party library. The ```svt_av1_fast_corner_detect``` function
is first called to determine the features in the source picture. Then it is called again
from the function ```svt_av1_compute_global_motion``` to determine the features in the reference picture.

Once the features have been extracted, they are matched. This is done in the
```svt_av1_determine_correspondence``` function by two nested loops over the features of the
reference frame and the current frame. A current frame feature is matched to a reference
frame feature that maximizes their cross-correlation computed by ```svt_av1_compute_cross_correlation_c```.
However, the match is kept only if the cross-correlation is superior to the ```THRESHOLD_NCC```
threshold multiplied by the variance of the current feature patch.

The matched feature positions are further refined in the ```improve_correspondence``` function.
This function performs a double iteration to look for the best match in a patch of size
```SEARCH_SZ``` located around the previously found match position.

The rotation-zoom and affine global motion models are tested with the ```RANSAC``` algorithm
by the ransac function. This function takes as argument three function pointers:
```is_degenerate```, ```transformation``` and ```projectpoints```. They are set according to the type
of transformation that is estimated.

The minimum number of transformation estimation trials is defined by the ```MIN_TRIALS``` macro.
For each trial, the algorithm selects random feature match indices with the ```get_rand_indices``` function.

It first checks if the current match selection does not lead to a degenerated version of
the transformation with the ```is_degenerate``` function pointer. The parameters of the
transformation are then estimated by the ```find_transformation``` function pointer.
The positions of the feature matches that have not been used to compute the transformation
parameters are projected with the ```projectpoints``` function pointer. Finally, the number of
inliers and outliers of the current transformation are counted. A feature match is considered
as an outlier if its distance with its position calculated with the transformation is
superior to the ```INLIER_THRESHOLD``` macro.

The parameters of the top ```RANSAC_NUM_MOTIONS``` transformations that have the greatest
numbers of inliers and smallest position variance are kept. These transformations are
then ranked by their number of inliers and their parameters are recomputed by using
only with the inliers.

The transformation parameters are refined in the ```svt_av1_refine_integerized_param``` function.
It uses the ```svt_av1_warp_error``` function to estimate the error between the reference frame
and the current frame in order to select the model with the smallest error.

As saving global motion parameters takes space in the bit stream, the global motion model
is kept only if the potential rate-distortion gain is significant. This decision is made
by the ```svt_av1_is_enough_erroradvantage``` function thanks to the computed frame error, the storage
cost of the global motion parameters and empirical thresholds.


The AV1 specifications define four global motion types:

  - IDENTITY for an identity model,

  - TRANSLATION for a translation model,

  - ROTZOOM for a rotation and zoom model,

  - AFFINE for an affine model.


#### Injection and processing

Each block that is 8x8 or larger in size can be a candidate for local or
global warped motion. For each block, we insert in the
```inject_inter_candidates``` function global motion candidates for the
simple and compound modes for the ```LAST_FRAME``` and the ```BWDREF_FRAME```
frame types. The compound mode implementation only mixes global warped
motions for both references.

To identify global warped motion candidates, the
```warped_motion_prediction``` function has been modified to support the
compound mode for warped motions for the case where high bit-depth is
enabled and for the case where it is not.

The two main steps involved in MD are the injection of GLOBAL and GLOBAL_GLOBAL candidates, and the processing of those candidates through MD stages.
The conditions for the injection of GLOBAL candidates are as follows: For the case where downsample_level <= GM_DOWN:
1. The global motion vector points inside the current tile AND
2. (((Transformation Type > TRANSLATION AND block width >= 8 AND block height >= 8) OR Transformation type <= TRANSLATION))

Otherwise, only condition 1 above applies.

The conditions for the injection of GLOBAL_GLOBAL candidates are as follows:

For the case where downsample_level <= GM_DOWN:

1. Is_compound_enabled (i.e. compound reference mode) AND
2. allow_bipred (i.e. block height > 4 or block width > 4) AND
3. (List_0 Transformation type > TRANSLATION AND List_1 Transformation type > TRANSLATION))

Otherwise, only conditions 1 and 2 above apply.

It should be noted that for the case of compound mode prediction, only GLOBAL_GLOBAL
candidates corresponding to compound prediction modes MD_COMP_AVG and MD_COMP_DIST are injected.

The three main functions associated with the injection of GLOBAL_GLOBAL candidates are
```precompute_intra_pred_for_inter_intra```, ```inter_intra_search``` and ```determine_compound_mode```.
The first two are related to the generation of inter-intra compound candidates. The third
is related to the injection of inter-inter compound candidates.


## 3. Optimization of the algorithm

Different quality-complexity tradeoffs of the global motion algorithm can be
achieved by manipulating a set of control parameters that are set in the
gm_controls() function. These control parameters are set according to the flag
gm_level which is set in the picture decision process according to the encoder
preset. The different parameters that are controlled by the flag gm_level are
described in Table 2 below.

##### Table 2. Optimization flags associated with global motion compensation.

|**Flag**|**Level (Sequence/Picture)**|**Description**|
|--- |--- |--- |
|enabled|Picture|Enable/Disable global motion knob|
|identiy_exit|Picture|0: Generate GM params for both list_0 and list_1, 1: Do not generate GM params for list_1 if list_0/ref_idx_0 is identity.|
|rotzoom_model_only|Picture|0: Use both rotzoom and affine models, 1: Use rotzoom model only|
|bipred_only|Picture|0: Inject both unipred and bipred GM candidates, 1: Inject only bipred GM candidates. |
|bypass_based_on_me|Picture|Bypass global motion search based on the uniformity of motion estimation MVs. 0/1: Do not bypass/Bypass GM search. |
|use_stationary_block|Picture|0: Do not consider stationary_block info at me-based bypass, 1: Consider stationary_block info at me-based bypass (only if bypass_based_on_me=1)|
|use_distance_based_active_th|Picture|Active_th is the threshold used to decide on the uniformity of MVs from motion estimation. 0: Use default active_th, 1: Increase active_th based on distance to ref (only if bypass_based_on_me=1)|
|params_refinement_steps|Picture|Specify the number of refinement steps to use in the GM parameters refinement.|
|downsample_level|Picture|GM_FULL: Exhaustive search mode. GM_DOWN: GM search based on down-sampled resolution with a down-sampling factor of 2 in each dimension. GM_TRAN_ONLY: Translation only using ME MV|
| use_ref_info | do GM in the closed loop instead of the open loop and use reference information 0: off 1: on |
| layer_offset | do the detection bypass for last layer pictures   0:off     1:last layer     2:last 2 layers 3: last 3 layers |
| corners | use a fraction of corner points for computing correspondences for RANSAC in detection. 1:1/4   2:2/4   3:3/4   4:all |
| chess_rfn | skip global motion refinement using a chess pattern to skip blocks |
| match_sz | change the window size for correlation calculations. must be odd. N: NxN window size goes from 1 to 15 |
| inj_psq_glb | Inject global only if Parent SQ is global |
| pp_enabled | enable Pre-processor for GM |
| ref_idx0_only | limit the search to  ref index = 0 only |

The generated global motion information may be used in all or some of the mode decision Partitioning Decision (PD) passes.
The injection of global motion candidates in MD is controlled by the flag global_mv_injection.

## 4. Signaling

The global motion parameters are written in the bitstream for each
encoded frame with their corresponding references.

Boolean parameters encode the type of global motion models among the four available: IDENTITY, TRANSLATION, ROTZOOM or AFFINE (See Table 3).

##### Table 3. Global motion types signaled in the bitstream.

| **Frame level** | **Values**                     | **Number of bits** |
| --------------- | ------------------------------ | ------------------ |
| is\_global      | {0, 1}                         | 1                  |
| is\_rot\_zoom   | {0, 1}                         | 1                  |
| is\_translation | {0, 1}                         | 1                  |

Depending on the model complexity, several parameters are also encoded (See
Table 4). Each of those parameters corresponds to an entry in the affine
transformation matrix.

##### Table 4. Global motion parameters signaled in the bitstream.

|**Frame level**|**Number of bits**|
|--- |--- |
|Global motion parameters:|Up to 12|
|0 parameter for IDENTITY|Up to 12|
|2 parameters for TRANSLATION|Up to 12|
|4 parameters for ROTZOOM|Up to 12|
|6 parameters for AFFINE|Up to 12|

## Notes

The feature settings that are described in this document were compiled at
v4.0.1 of the code and may not reflect the current status of the code. The
description in this document represents an example showing how features would
interact with the SVT architecture. For the most up-to-date settings, it's
recommended to review the section of the code implementing this feature.

## References

[1] Sarah Parker, Yue Chen, David Barker, Peter de Rivaz, Debargha
  Mukherjee, “Global and Locally Adaptive Warped Motion Compensation in
  Video Compression,” International Conference on Image Processing, pp.
  275-279, 2017.

[2] Peter de Rivaz and Jack Haughton, “AV1 Bitstream & Decoding Process
  Specification”, 2019
