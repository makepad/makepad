[Top level](../README.md)

# Restoration Filter

The restoration filter is applied after the constrained directional
enhancement filter (CDEF) and aims at improving the reconstructed picture by
recovering some of the information lost during the compression process.
The restoration filter involves two main filters, namely a Wiener filter
and a self-guided restoration filter with projection (SGRPROJ).

The processing of a frame by the restoration filter proceeds by
splitting the picture into restoration units. The size of the
restoration units is set in the function `set_restoration_unit_size`
and is set to the maximum restoration unit size
(```RESTORATION_UNITSIZE_MAX```) of 256 for both luma and chroma.

At the frame level, the restoration filter operates with one of the
following modes: OFF, Wiener filter, SGRPROJ filter for the whole frame,
or switching between those three modes at the restoration unit level.

## I. Wiener Filter

### 1. Description of the algorithm [To be completed]

The Wiener filter is a separable symmetric filter (7/5/3-tap
filters), where only three, two or one coefficient(s) for the horizontal
and vertical filters are included in the bit stream due to symmetry. The
constraints on the Wiener filter to reduce complexity are as follows:

  - The filter is separable. Let `c_v` and `c_h` be the
    wx1 vertical and horizontal filter kernels.
  - The filter kernels `c_v` and `c_h` are symmetric:\
   $`c_v(i)=c_v(w-1-i)`$, $`c_h(i)=c_h(w-1-i)`$, where `i=0,1...,r-1`.

  - The sum of the coefficients is 1: $`\sum c_v(i) = \sum c_h(i) = 1`$.

The design of the Wiener filter proceeds in an iterative manner:

  - Starts with initial guess for both the vertical and horizontal
    filters.
  - Design one of the two filters while holding the second filter fixed,
    then repeat the process

The filter is designed over windows of size 64x64.
The filter taps are either 7, 5, or 3 for luma and 5 or 3 for
chroma.

### 2. Implementation

**Inputs to rest\_kernel**: Output frame of the CDEF filter.

**Outputs of rest\_kernel**: Restored picture, filter parameters.

**Control flags**:

##### Table 1. Control flags for the Wiener filter.

| **Flag**            | **Level** | **Description**                                                                                                     |
| ------------------- | --------- | ------------------------------------------------------------------------------------------------------------------- |
| enable\_restoration | Sequence  | Indicates whether to use restoration filters for the whole sequence.                                                |
| enable\_restoration | Picture   | Indicates whether to use restoration filters for the whole picture .                                                |
| wn\_filter\_ctrls     | Picture   | Struct of controls for the quality-complexity tradeoff of the filter as a function of the encoder mode.           |
| allow\_intrabc      | Picture   | Indicates whether to enable intra block copy. The restoration filter is not active when allow_intrabc is set to 1.  |

### Step 1 - Splitting the frame into segments

The frame to be filtered is divided into segments to allow for parallel filtering operations on
different parts of the frame. Each segment could involve more than one restoration unit.
The sizes and number of segments are set as follows (see ```EbEncHandle.c```):

```
unit_size = 256;
rest_seg_w = MAX((max_input_luma_width /2) / unit_size, 1);
rest_seg_h = MAX((max_input_luma_height/2 ) / unit_size, 1);
rest_segment_column_count = MIN(rest_seg_w,6);
rest_segment_row_count = MIN(rest_seg_h,4);
```

Each segment may consist of several restoration units.
Each restoration unit is split into restoration processing units of size 64x64 for luma
(```#define RESTORATION_PROC_UNIT_SIZE 64```(in ```restoration.h```))

### Step 2 – Restoration filter search. Each segment goes through a search to identify the best Wiener filter parameters for each restoration unit in the segment (```restoration_seg_search```).

Loop over all restoration units in each tile segment (```foreach_rest_unit_in_tile_seg```)

- Determine the best filtering parameters for the restoration unit (```search_wiener_seg```)
   - The initial Wiener filter coeff are computed. See functions ```svt_av1_compute_stats```, ```wiener_decompose_sep_sym```, ```finalize_sym_filter```. (This step may be skipped, see optimization section).
     Check that the new filter params are an improvement over the identity filter (i.e. no filtering). If not, exit Wiener filter search and do not use Wiener filter. See function ```compute_score```.
   - Refine the initially computed Wiener filter coeffs (see function ```finer_tile_search_wiener_seg```).
     Up to three refinement steps are performed (with step sizes 4,2,1).
     In each step, the filter coeffs are shifted according to the step size.
     Filtering is applied to the filter block with the new coeff values and the SSE is computed (```try_restoration_unit_seg```).
     If the refined coeff values are better than the original, the coeff values are updated.
   - The best filter coeffs are returned, along with the corresponding SSE of the filtered block.

### Step 3 – Identify the best filtering mode for the whole frame

- Loop over the picture planes to identify the best restoration option for each of the picture planes
   - Loop over all available filtering options (```RESTORE_NONE```, ```RESTORE_WIENER```, ```RESTORE_SGRPROJ```, ```RESTORE_SWITCHABLE```),
     compute the resulting cost for using each of the options over the whole frame, and choose the best
     option for the whole frame. The selection is based on the rate-distortion cost of the different options.
     (```rest_finish_search```)

### Step 4 – Filter each restoration unit in the frame using the identified best option from step 3 above. (```av1_loop_restoration_filter_frame```)

More details on ```try_restoration_unit_seg```
- Filter stripes of height 64 (```try_restoration_unit_seg```)
  - Loop over stripes (```av1_loop_restoration_filter_unit```)
    - Loop over the restoration processing units in a stripe and apply filtering to each restoration processing unit using the already identified best filtering parameters for each restoration processing unit. (```wiener_filter_stripe```)
- Compute the sse for the filtered restoration unit (```sse_restoration_unit```)


## II. Self-Guided Restoration Filter with Subspace Projection (SGRPROJ)

### 1. Description of the algorithm

The main objective behind using the SGRPROJ filter is to smooth the
reconstructed image while preserving edges. The filter consists of two
main components, namely a self-guided filter and a subspace projection
of the reconstructed image.

#### 1.1 Self-guided filter

The self-guided filter feature makes use of a guide image where the
objective is to transfer features in the guide image to the
reconstructed picture. When the guide image is the same as the
reconstructed image, the filter is called a self-guided filter. The
objective in this case is to apply filtering that is a function of the
spatial characteristics (variance) of the immediate neighborhood of the
pixel to be filtered. The main idea behind the filter is outlined below.

A filtered value $`\mathbf{p_f}`$ of a sample value
$`\mathbf{p_r}`$ in the reconstructed image is generated as follows:

$`\mathbf{p_f=fp_r+(1-f)\mu}`$

where $`\mu`$ is the average of a small window ***w*** around $`\mathbf{p_r}`$
in the reconstructed picture and $`0<= \mathbf{f} < 1`$ is a function of the
variance of the samples in the window `w`.

  - When the variance of `w` is large, then $`\mathbf{f}`$ is close
    to 1, and the filtered sample value is very close to
    $`\mathbf{p_r}`$ i.e. very little filtering takes place,
    and high frequency features (edges) in `w` are preserved.
  - When the variance of `w` is very small, then $`\mathbf{f}`$ is
    close to 0, and the filtered sample value is very close to
    $`\mu`$, i.e. $`\mathbf{p_r}`$ is replaced by a
    value close to $`\mu`$ and smoothing takes place.

The figure below illustrates the main idea behind the filter.

![restoration_filter_fig1](./img/restoration_filter_fig1.png)

##### Figure 1. Example of how the variance of the area around the sample to filtered affects the selection of the self-guided restoration filter parameters.

The derivation of the filter parameters is outlined below.

  - Compute the mean $`\mu`$ and the square of the variance $`\sigma^2`$ of a $`(2r+1)`$x$`(2r+1)`$ window `w` around the sample $`\mathbf{p_r}`$ in the reconstructed image.
  - Define $`f=\frac{\sigma^2}{\sigma^2+\varepsilon}`$, $`\mathbf{g=(1-f)\mu}`$.
    The parameter $`\varepsilon`$ is used to tune the filter.
  - Repeat the same computations above for every sample in the window
    `w` (or for a subset of those samples). Define `F` and `G` to be the averages of $`\mathbf{f}`$ and $`\mathbf{g}`$ computed for all samples in the window `w` (or for a subset of those samples), respectively.
  - Filtering: $`\mathbf{p_f=Fp_r+G\mu}`$

The performance of the self-guided filter is generally not sufficient to
produce good quality reconstructed images. As a result, a further
restoration step is considered and involves the use of subspace
projection.

### 1.2. Subspace Projection

The main idea behind subspace projection is as follows:

  - Construct two restored versions of the reference picture generated
    using the self-guided filter using two different $`(r,
    \varepsilon)`$ parameter pairs.
  - Consider the difference between each of the two restored versions
    and the reference picture and consider the subspace generated by
    those two differences.
  - Project the difference between the source image and the
    reconstructed image into the constructed subspace.

To illustrate the idea of subspace projection, consider the following column vectorized version of the corresponding pictures:

  - $`X_s`$: Source.
  - $`X_r`$: Reconstructed.
  - $`X_1`$ and $`X_2`$ : Filtered (i.e. restored) versions of
    $`X_r`$ using self-guided filter based on parameters ( $`r_1,\varepsilon_1`$) and ( $`r_2,\varepsilon_2`$) respectively.
  - $`X_f`$ : Final restored version of $`X_r`$. ( $`X_f-X_r`$ ) is obtained by projecting ( $`X_s-X_r`$) onto the subspace generated by ( $`X_1-X_r`$ ) and ($`X_2-X_r`$ )

![restoration_filter_fig2](./img/restoration_filter_fig2.png)

##### Figure 2. Illustration of the idea of subspace projection in the SGRPROJ filter.

  - $`(X_s-X_r)=\alpha(X_1-X_r)+\beta(X_2-X_r)`$
  - $`\begin{bmatrix}\alpha \\ \beta\end{bmatrix} = (A^TA)^{-1}A^Tb`$
  - $`A=[(X_1-X_r)(X_2-X_r)],b=[(X_s-X_r)]`$
  - $`X_f=(1-\alpha-\beta)X_r + \alpha X_1+ \beta X_2`$


### 2. Implementation

**Inputs to rest\_kernel**: Output frame of the CDEF filter.

**Outputs of rest\_kernel**: Restored picture, filter parameters.

**Control flags**:

##### Table 2. Control flags for the SGRPROJ filter.

| **Flag**            | **Level** | **Description**                                                                                                     |
| ------------------- | --------- | ------------------------------------------------------------------------------------------------------------------- |
| enable\_restoration | Sequence  | Indicates whether to use restoration filters for the whole sequence.                                                |
| enable\_restoration | Picture   | Indicates whether to use restoration filters for the whole picture .                                                |
| sg\_filter\_ctrls   | Picture   | Struct that controls the quality-complexity tradeoff of the filter as a function of the encoder mode.               |
| allow\_intrabc      | Picture   | Indicates whether to enable intra block copy. The restoration filter is not active when allow\_intrabc is set to 1. |

#### Details of the implementation

The main function calls associated with the SGRPROJ filter as indicated in Figure 3 below.

![restoration_filter_fig3](./img/restoration_filter_fig3.png)

##### Figure 3. Main function calls associated with the SGRPROJ filter.

The main steps involved in the implementation of the algorithm are
outlined below, followed by more details on some of the important
functions.

#### Step 1 - Splitting the frame into segments

The frame to be filtered is divided into segments to allow for parallel
filtering operations on different parts of the frame. Each segment could
involve more than one restoration unit. The sizes and number of segments
are set as follows (see EbEncHandle.c):

```c
unit_size = 256;
rest_seg_w = MAX((max_input_luma_width /2) / unit_size, 1);
rest_seg_h = MAX((max_input_luma_height/2 ) / unit_size, 1);
rest_segment_column_count = MIN(rest_seg_w,6);
rest_segment_row_count = MIN(rest_seg_h,4);
```

Each segment may consist of a number of restoration units. Each
restoration unit is split into restoration processing units of size
64x64 for luma (```#define RESTORATION_PROC_UNIT_SIZE 64```(in restoration.h))

#### Step 2 – Restoration filter search. Each segment goes through a search to identify the best SGRPROJ filter parameters for each restoration unit in the segment (```restoration_seg_search```).

Loop over all restoration units in a given tile segment (```foreach_rest_unit_in_tile_seg```)
  - Determine the best filtering parameters for the restoration unit (```search_selfguided_restoration```)
    - Determine the search range for epsilon values \[start\_ep, end\_ep\] for epsilon values to use in optimizing the filter parameters, where epsilon is indicated in the description of the algorithm presented above.
    - Loop over the epsilon values in the range \[start\_ep, end\_ep\]
      - Loop over 64x64 restoration processing units in the restoration
        unit (apply\_sgr)
          - Filter all samples in the 64x64 restoration processing unit (av1\_selfguided\_restoration(\_avx2 or \_c). More
            details on av1\_selfguided\_restoration(\_avx2 or \_c) are
            included below.
      - Generate the projection of the (Source -Reconstructed) onto the
        subspace generated by (Filtered\_recon\_1 - Reconstructed) and
        (Filtered\_recon\_2 - Reconstructed), where Filtered\_recon\_1
        and Filtered\_recon\_2 are two restored version of
        thereconstructed picture, and generate the corresponding
        projection coordinates xq\[0\] and xq\[1\], which correspond to
        \(α) and \(β) in the description of the algorithm
        outlined above. (```get_proj_subspace```)
      - Compute the following parameters in (encode\_xq)
          - xqd\[0\]: Clamped value of xq\[0\]
          - xqd\[1\]: Clamped value of (128 - xqd\[0\] - xq\[1\])
      - Perform a refinement search around the identified parameters
        xq\[0\] and xq\[1\] to see if any other nearby parameters
        provide a better filtering error.
        (```finer_search_pixel_proj_error```)
      - Keep track of the best filtering error for the restoration unit,
        the corresponding epsilon and (xqd\[0\], xqd\[1\]) parameters.
    - Increment a counter ```sg_frame_ep_cnt[bestep]``` for the identified best epsilon value from the steps above.
    - Return the best epsilon and (xqd\[0\], xqd\[1\]) parameters for the current restoration unit.


- Filter stripes of height 64 (```try_restoration_unit_seg```)
  - Loop over stripes (```av1_loop_restoration_filter_unit```)
    - Loop over the restoration processing units in a stripe and apply filtering to each restoration processing unit using the already identified best filtering parameters for each restoration processing unit. (```sgrproj_filter_stripe```)
- Compute the sse for the filtered restoration unit (```sse_restoration_unit```)

The function calls associated with step 2 above are summarized in the
diagram shown in Figure 4 below.

![restoration_filter_fig4](./img/restoration_filter_fig4.png)

##### Figure 4. Continuation of Figure 3 showing the function calls associated with SGRPROJ filter parameter search.

#### Step 3 – Identify the best filtering mode for the whole frame

- Loop over the picture planes to identify the best restoration option for
each of the picture planes
  - Loop over all available filtering options (```RESTORE_NONE,
RESTORE_WIENER, RESTORE_SGRPROJ, RESTORE_SWITCHABLE```), compute the
resulting cost for using each of the options over the whole frame, and
choose the best option for the whole frame. The selection is based on
the rate-distortion cost of the different options.
(```rest_finish_search```)

#### Step 4 – Filter each restoration unit in the frame using the identified best option from step 3 above. (```av1_loop_restoration_filter_frame```)

#### More details on av1\_selfguided\_restoration(\_avx2 or \_c).

For a given 64x64 block in a restoration unit, integral images D and C
corresponding to the sum of elements in the 64x64 block and to the sum
of their squares, respectively, are generated. These two integral images
make is very easy to compute the mean and variance of any block with
arbitrary location and size in the 64x64 block.

The integral images D and C are used to compute the filter parameters
for each sample in the 64x64 block. The filter parameters are stored in
arrays A and B.

To filter a given sample, the filter parameters for neighboring samples
are averaged. The filter parameters are obtained from the arrays A and
B. The neighboring samples are taken from a window of size
(2r+1)\*(2r+1) around the sample to be filtered, where r could be 1 or 2
(r=0 implied SGRPROJ filter is OFF). A weighted average of the
neighboring filtering parameters for the neighboring samples is used in
filtering the current sample, as outlined above in the description of
the filter algorithm. See ```av1_selfguided_restoration_c,
selfguided_restoration_fast_internal and
selfguided_restoration_internal``` for the C implementation, av1_selfguided_restoration_avx2, integral_images, calc_ab_fast, final_filter_fast, calc_ab, final_filter for the avx2 implementation.

### 3. Optimization of the algorithm

Both the Wiener filter and the SGRPROJ filters involve, at the
restoration unit level, a search procedure for the best Wiener filter
parameters and for the best SGRPROJ filter parameters. The tradeoff
between complexity and quality is achieved by limiting the extent of the
filter parameter search.

**3.1 Wiener filter search**

Wiener filter optimization controls are set in ```svt_aom_set_wn_filter_ctrls ()```.

### Filter Tap Level

For the wiener filter, the search could be performed using either 3, 5
or 7 tap filters for luma, or 3 or 5 tap filters for chroma. The
parameter ```cm->wn_filter_ctrls.filter_tap_level``` is used to specify the level of filter
complexity, where increasing levels of filter search complexity are
defined by considering an increasing number of filter taps for both luma
and chroma, as given in the table below.

##### Table 3. Description of the Wiener filter settings for the different ```cm->wn_filter_ctrls.filter_tap_level``` values.

| **wn\_filter\_mode** | **Settings**             |
| -------------------- | ------------------------ |
| 0                    | OFF                      |
| 1                    | 3-Tap luma/ 3-Tap chroma |
| 2                    | 5-Tap luma/ 5-Tap chroma |
| 3                    | 7-Tap luma/ 5-Tap chroma |

### Filter Coeff Selection

Generally, the Wiener filter coeffs for each restoration unit are computed;
however, if the Wiener filter coeff values of ref frames are available, they
can be used instead (and the computation can be skipped). When enabled,
```cm->wn_filter_ctrls.use_prev_frame_coeffs``` will set the initial coeff
values to those chosen by the nearest list 0 reference frame for each
corresponding restoration unit. Refinement (if enabled – see next section) will
then be performed.

### Filter Coeff Refinement

After the initial filter coeff values are selected, a refinement search can be
performed to improve the coeff values. The refinement is performed iteratively,
with 3 step sizes: 4, 2, 1. By enabling
```cm->wn_filter_ctrls.max_one_refinement_step``` only a step size of 4 is used
in the refinement (smaller step sizes, which improve granularity of the coeff,
and therefore accuracy, will be skipped). To disable the refinement and
automatically use the computed coeffs without refinement, set
```cm->wn_filter_ctrls.use_refinement``` to 0.


**3.2 SGRPROJ filter search**

The search for the best SGRPROJ filter is normally performed by evaluating the
filter performance for each of the sixteen different
$`\varepsilon`$ values in the
interval $`[0,15]`$, where
$`\varepsilon`$ is used in the
outline of SGRPROJ algorithm presented above. The algorithmic optimization of
the filter search involves restricting the number of
$`\varepsilon`$ values in the
search operation. The parameter ```cm->sg_filter_ctrls.start_ep[0]``` and ```cm->sg_filter_ctrls.start_ep[1]```
determine the search's starting $`\varepsilon`$ value for luma and chroma respectively.
 ```cm->sg_filter_ctrls.start_ep[0/1]``` determine the end values for the search, while
 ```cm->sg_filter_ctrls.ep_inc[0/1]``` determine the step size. ```cm->sg_filter_ctrls.refine[0/1]```
indicate whether refinement for alpha and beta should be performed.

The optimization proceeds as follows:

1.  The SGRPROJ filter level is obtained from ```vsvt_aom_get_sg_filter_level```.

2.  The search parameters are set in ```svt_aom_set_sg_filter_ctrls```.

3.  The interval \[start\_ep, end\_ep\] of $`\varepsilon`$
    values to search is specified as follows
    (```search_selfguided_restoration```):

  - The $`\varepsilon`$ values sg\_ref\_frame\_ep\[0\] and sg\_ref\_frame\_ep\[1\] of the reference pictures are used to define the center of the interval mid\_ep as follows:
    ```c
    if (sg_ref_frame_ep[0] < 0 && sg_ref_frame_ep[1] < 0) then mid_ep = 0
    else if (sg_ref_frame_ep[1] < 0) then mid_ep = sg_ref_frame_ep[0]
    else if (sg_ref_frame_ep[0] < 0( then mid_ep = sg_ref_frame_ep[1]else mid_ep = (sg_ref_frame_ep[0] + sg_ref_frame_ep[1]) / 2
    ```
  - The interval limits are given by:
    ```c
    start_ep = 0 if (sg_ref_frame_ep[0] < 0 && sg_ref_frame_ep[1] < 0), else start_ep = max(0, mid_ep - step)
    end_ep = 8 if (sg_ref_frame_ep[0] < 0 && sg_ref_frame_ep[1] < 0), else end_ep = min(8, mid_ep + step)
    ```
### 4. Signaling

##### Table 7. Restoration filter signals.

| **Signal**                        | **Description**                                                       |
| --------------------------------- | --------------------------------------------------------------------- |
| **At the frame level**            |                                                                       |
| frame\_restoration\_type          | RESTORE\_NONE, RESTORE\_WIENER, RESTORE\_SGRPROJ, RESTORE\_SWITCHABLE |
| restoration\_unit\_size           | Size of restoration unit. For luma: 128x128 or 256x256
| **At the restoration unit level** |                                                                       |
| restoration\_type                 | RESTORE\_NONE, RESTORE\_WIENER, RESTORE\_SGRPROJ                      |
| wiener\_info                      | Vertical and horizontal filter coefficient array vfilter and hfilter. |
| sgrproj\_info                     | epsilon, projection parameters xqd\[0\] and xqd\[1\]                  |

## Notes

The feature settings that are described in this document were compiled at
v4.0.1 of the code and may not reflect the current status of the code. The
description in this document represents an example showing how features would
interact with the SVT architecture. For the most up-to-date settings, it's
recommended to review the section of the code implementing this feature.

## References

[1] Debargha Mukherjee, Shunyao Li, Yue Chen, Aamir Anis, Sarah Parker and
James Bankoski, “*A Switchable Loop-restoration with Side-information
Framework for the Emerging AV1 Video Coding*,” International Conference
on Image Processing, pp. 265-269, 2017.
