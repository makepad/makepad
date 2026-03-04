[Top level](../README.md)

# SQ\_Weight

## Description

SQ\_WEIGHT is a cost scaling factor used to determine if the evaluation of the
non-square (NSQ) shapes HA, HB, VA, VB, H4 and V4 could be skipped based on the
relative cost of the square (SQ), H and V shapes. The various shapes supported
in the AV1 Bitstream & Decoding Process Specification are shown in Figure 1
below.

![sq_weight_fig2](./img/sq_weight_fig2.png)

##### Figure 1. Block shapes in the AV1 specification.

## Implementation

### SQ vs. H/V Comparison

The first part of the algorithm compares the cost of the square block to the cost of the H/V blocks
to determine if the HA, HB, VA, VB, H4 and V4 shapes could be skipped as follows:

  - skip HA, HB and H4 if (SQ and H are valid shapes) and (H_COST > (SQ_WEIGHT * SQ_COST) / 100)
  - skip VA, VB and V4 if (SQ and V are valid shapes) and (V_COST > (SQ_WEIGHT * SQ_COST) / 100)

where X_COST refers to the cost of the block with shape X. The lower the SQ_WEIGHT, the higher the chance to skip NSQ shapes.

The SQ_WEIGHT is a scaling factor of the square shape cost, and can be made more or less aggressive based on the preset and block characteristics.
The SQ_WEIGHT is derived as follows:

where Base is a function of the encoder preset.
The offset is set in ```svt_aom_sig_deriv_mode_decision_config()``` and is a function of the non-square shape
being considered and the transform coefficient information as follows:

```
Offset = 0
If (block is H4 or V4):
   Offset += 5
If (HA (VA) and 1st H (V) has no transform coefficients OR HB (VB) and 2nd H (V) has no transform coefficients):
   Offset -= 10
```

Final offset is between -10 and +5.

### H vs. V comparison

The second part of the algorithm compares the cost of H block to the cost of
the V block to determine if the HA, HB, VA, VB, H4 and V4 shapes could be
skipped. The NSQ shapes are skipped as follows:

  - skip HA, HB and H4 if (H and V are valid shapes) and (H_COST > (HV_WEIGHT * V_COST) / 100)
  - skip VA, VB and V4 if (H and V are valid shapes) and (V_COST > (HV_WEIGHT * H_COST) / 100)

## Notes

The feature settings that are described in this document were compiled at
v4.0.1 of the code and may not reflect the current status of the code. The
description in this document represents an example showing how features would
interact with the SVT architecture. For the most up-to-date settings, it's
recommended to review the section of the code implementing this feature.
