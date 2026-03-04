[Top level](../README.md)

# Intra Block Copy

## 1. Description of the algorithm

In intra frames, intra block copy (IBC) allows for the prediction of a given
intra coded block to be a copy of another intra coded block in the same frame
(i.e. from the reconstructed part of the current frame). The copied block is
specified using a displacement vector (DV). Only integer precision DVs are
allowed since force_integer_mv will be equal to 1 for intra frames. Bilinear
interpolation is considered for chroma prediction in the case of odd DVs. IBC
is applicable only to key frames and intra-only non-key frames. When IBC is
active, all in-loop filtering is disabled for the whole frame, including
deblocking filter, CDEF and restoration filter. The prediction is generated
from the reconstructed intra pixels, where the latter would not have been
processed by the in-loop filters since the latter are disabled. The predicted
block is considered an inter predicted block using a single reference frame
(the current intra frame), and where the DV is full-pel. Only single-reference
mode is allowed.

A 256 pixels wide area just before the block being predicted is excluded
from the block copy intra search area, i.e. the valid region of the
frame consists of already reconstructed blocks that are 256 pixels away
(in a raster scan order) from the current block. Figure 1 below
illustrates the allowed search area.

![intra_block_copy_fig1](./img/intra_block_copy_fig1.png)

##### Figure 1. Diagram showing blocks not allowed in the search area.

The IBC feature is useful in encoding screen content that involves
repeated patterns, text and texture in the same frame.

## 2. Implementation of the algorithm

### Main function calls

A diagram of the main function calls associated with the IBC algorithm
is shown in Figure 2 below. The functions are shown according to the
depth of the function call.

![intra_block_copy_fig2](./img/intra_block_copy_fig2.png)

##### Figure 2. Main function calls associated with the IBC algorithm.

### Candidate Injection

In the function ```inject_intra_bc_candidates()```, up to 2 intra candidates
are injected as IBC modes. These modes are considered DC_PRED modes when coding
the block mode information in the bit stream. Simple translation is used and no
fractional DVs are allowed for this case. For Chroma, bilinear interpolation is
used to produce predicted pixels. The two candidates are determined through the
```intra_bc_search()``` function call, which is discussed next.

### DV Search

The function ```intra_bc_search()``` performs a search within the current
picture (i.e. within the already reconstructed area). The search is a
combination of a classic Diamond search followed by Hash search (CRC is used as
Hash metric). The search is only performed in full pel resolution as sub-pel
displacements are not allowed in the IBC tool in AV1.

The decoded reconstructed area is divided into two search areas: Top and Left.
As explained above, due to HW constraints, not all of the top reconstructed
area is used to derive DV vectors. To support wavefront-like SW based
processing, more constraints are added to only consider the valid SBs in such
scenario.

More detailed steps involved in the DV search are listed below:

1. Set the reference frame to ```INTRA_FRAME```.

2. Get nearest and near MVs from MV stack for the specified reference
   frame. See (```svt_av1_find_best_ref_mvs_from_stack```)

3. Set ```dv_ref``` to either nearest or near.

4. Constrain the ```dv_ref``` mv to be at least a block size away from the
   current block, and also to point at least 256 samples away to the
   left in the x direction when too close to the tile top boundary.
   (```av1_find_ref_dv```)

5. Two types of searches could be performed: Search above the current
   block (```IBC_MOTION_ABOVE```) only or search above and to the left of
   the current block (```IBC_MOTION_ABOVE``` and ```IBC_MOTION_LEFT```),
   depending on the setting of ```ibc_mode```. Up to two dv candidates could
   be generated.

6. Limits on mv sizes are computed and refined
   (```svt_av1_set_mv_search_range```).

   Perform full-pel diamond/exhaustive search followed by hash search
   (svt_av1_full_pixel_search). The hash search computes the hash of 2x2 blocks
   around each luma pixel in the reference frame. The 2x2 hashes arethen used
   to make up the 4x4 hashes, which are then used to make up the 8x8 hashes,
   and so on. All the hash values are stored in a hash table. The hash for the
   current block is then computed and compared to hash values in the hash table
   which stores the hashes from the reference frames. If a match is found, then
   there is a block in the reference frame that is the same as the current
   block. That block may then be used as an IBC candidate if its estimated cost
   is lower than all other IBC candidates.

7. Perform full-pel diamond search followed by hash search
   (```svt_av1_full_pixel_search```).

8. Make sure returned mv is within the specified mv bounds
   (```mv_check_bounds```)

9. Make sure the returned mv meets HW and SW constraints
   (```av1_is_dv_valid```)

### Control Tokens/flags

The feature is currently active only when screen content encoding is active, either through:

- Setting screen content encoding to Auto mode, where screen-content-type of pictures are flagged based on detector information, or

- Setting the screen content encoding to Manual mode, where the input sequence is encoded as screen content (occurs when “—--scm 1” is specified in the command line).

The control tokens and flags associated with the IBC feature are listed in Table 1 below.

##### Table 1. Control tokens and flags for the IBC feature.

| **Flag**      | **Level(Sequence/Picture)** | **Description**                                                                                                         |
| ---           | ---                         | ---                                                                                                                     |
| --scm         | Sequence                    | Command line token. 0: None, 1: Block Copy + Palette, 2: Auto mode (detector based), 3: Auto mode (anti-alias aware)    |
| -intrabc-mode | Configuration               | Command line token to specify IBC mode. 0: OFF, 1-3: IBC ON with intrabc levels mentioned below.,  -1: Default behavior |
| intrabc_level | Picture                     | Controls the complexity-quality trade-offs of the feature. 0: OFF, 1-6 ON                                               |
| allow_intrabc | Picture                     | For intra pictures, set to 1 when IBC is allowed, else set to 0.                                                        |

## 3. Optimization of the algorithm

##### Table 2. Optimization signals associated with the IBC feature..

| **Signal**          | **Description**                                                                                                                                                                                                                                                                                                                                                       |
| ---                 | ---                                                                                                                                                                                                                                                                                                                                                                   |
| enabled             |                                                                                                                                                                                                                                                                                                                                                                       |
| ibc_shift           | After the full-pel diamond search, a full-pel exhaustive search may be performed if the variance of the best residual out of the diamond search is still above a certain threshold. ibc_shift will shift the threshold to the left (i.e. double the threshold value), making the exhaustive search less likely to be performed. (0: No Shift; 1: Shift to left by 1). |
| ibc_direction       | Directions to perform IBC search for. 0: Left + Top; 1: Top only                                                                                                                                                                                                                                                                                                     |
| hash_4x4_blocks     | Set by get_disallow_4x4() to not hash 4x4 blocks for higher presets where 4x4 blocks are not encoded                                                                                                                                                                                                                                                                  |
| max_block_size_hash | The maximum block size that will be hashed; corresponds to the maximum block size for which an MD candidate will be generated by IBC hashing algorithm.                                                                                                                                                                                                               |


## 4. Signaling

The main signal that is sent in the bitstream to enable IBC is allow_intrabc
that is sent in the frame header. Note that IBC is only allowed for Intra coded
frames. In the sequence header screen content tools must be enabled to use IBC
at the frame level.

## Notes

The feature settings that are described in this document were compiled at
v4.0.1 of the code and may not reflect the current status of the code. The
description in this document represents an example showing how features would
interact with the SVT architecture. For the most up-to-date settings, it's
recommended to review the section of the code implementing this feature.
