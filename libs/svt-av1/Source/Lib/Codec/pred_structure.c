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

#include <stdlib.h>

#include "pred_structure.h"
#include "utility.h"
#include "common_dsp_rtcd.h"
#include "sequence_control_set.h"
/**********************************************************
 * Macros
 **********************************************************/
#define PRED_STRUCT_INDEX(hierarchicalLevelCount, predType) ((hierarchicalLevelCount) * PRED_TOTAL_COUNT + (predType))

// clang-format off
/**********************************************************
 * Instructions for how to create a Predicion Structure
 *
 * Overview:
 *   The prediction structure consists of a collection
 *   of Prediction Structure Entires, which themselves
 *   consist of reference and dependent lists.  The
 *   reference lists are exactly like those found in
 *   the standard and can be clipped in order to reduce
 *   the number of references.
 *
 *   Dependent lists are the corollary to reference lists,
 *   the describe how a particular picture is referenced.
 *   Dependent lists can also be clipped at predefined
 *   junctions (i.e. the list_count array) in order
 *   to reduce the number of references.  Note that the
 *   dependent deltaPOCs must be grouped together in order
 *   of ascending referencePicture in order for the Dependent
 *   List clip to work properly.
 *
 *   All control and RPS information is derived from
 *   these lists.  The lists for a structure are defined
 *   for both P & b-picture variants.  In the case of
 *   P-pictures, only Lists 0 are used.
 *
 *   Negative deltaPOCs are for backward-referencing pictures
 *   in display order and positive deltaPOCs are for
 *   forward-referencing pictures.
 *
 *   Please note that there is no assigned coding order,
 *   the PictureManager will start pictures as soon as
 *   their references become available.
 *
 *   Any prediction structure is possible; however, we are
 *     restricting usage to the following controls:
 *     # Hierarchical Levels
 *     # Number of References
 *     # b-pictures enabled
 *     # Intra Refresh Period
 *
 *  To Get Low Delay P, only use List 0
 *  To Get Low Delay b, replace List 1 with List 0
 *  To Get Random Access, use the preduction structure as is
 **********************************************************/
/************************************************
 * Flat
 *
 *  I-b-b-b-b-b-b-b-b
 *
 * Display & Coding Order:
 *  0 1 2 3 4 5 6 7 8
 *
 ************************************************/
static PredictionStructureEntry flat_pred_struct[] = {{
    0, // GOP Index 0 - Temporal Layer
    0 // GOP Index 0 - Decode Order
}};

/************************************************
* Random Access - Two-Level Hierarchical
*
*    b   b   b   b      Temporal Layer 1
*   / \ / \ / \ / \
*  I---b---b---b---b    Temporal Layer 0
*
* Display Order:
*  0 1 2 3 4 5 6 7 8
*
* Coding Order:
*  0 2 1 4 3 6 5 8 7
************************************************/
static PredictionStructureEntry two_level_hierarchical_pred_struct[] = {
    {
        0, // GOP Index 0 - Temporal Layer
        0 // GOP Index 0 - Decode Order
    },
    {
        1, // GOP Index 1 - Temporal Layer
        1 // GOP Index 1 - Decode Order
    }
};

/************************************************
* Three-Level Hierarchical
*
*      b   b       b   b       b   b        Temporal Layer 2
*     / \ / \     / \ / \     / \ / \
*    /   b   \   /   b   \   /   b   \      Temporal Layer 1
*   /   / \   \ /   / \   \ /   / \   \
*  I-----------b-----------b-----------b    Temporal Layer 0
*
* Display Order:
*  0   1 2 3   4   5 6 7   8   9 1 1   1
*                                0 1   2
*
* Coding Order:
*  0   3 2 4   1   7 6 8   5   1 1 1   9
*                              1 0 2
************************************************/
static PredictionStructureEntry three_level_hierarchical_pred_struct[] = {
    {
        0, // GOP Index 0 - Temporal Layer
        0 // GOP Index 0 - Decode Order
    },
    {
        2, // GOP Index 1 - Temporal Layer
        2 // GOP Index 1 - Decode Order
    },
    {
        1, // GOP Index 2 - Temporal Layer
        1 // GOP Index 2 - Decode Order
    },
    {
        2, // GOP Index 3 - Temporal Layer
        3 // GOP Index 3 - Decode Order
    }
};

/************************************************************************************************************
* Four-Level Hierarchical
*
*
*          b     b           b     b               b     b           b     b           Temporal Layer 3
*         / \   / \         / \   / \             / \   / \         / \   / \
*        /   \ /   \       /   \ /   \           /   \ /   \       /   \ /   \
*       /     b     \     /     b     \         /     b     \     /     b     \        Temporal Layer 2
*      /     / \     \   /     / \     \       /     / \     \   /     / \     \
*     /     /   \     \ /     /   \     \     /     /   \     \ /     /   \     \
*    /     /     ------b------     \     \   /     /     ------b------     \     \     Temporal Layer 1
*   /     /           / \           \     \ /     /           / \           \     \
*  I---------------------------------------b---------------------------------------b   Temporal Layer 0
*
* Display Order:
*  0       1  2  3     4     5  6  7       8       9  1  1     1     1  1  1       1
*                                                     0  1     2     3  4  5       6
*
* Coding Order:
*  0       4  3  5     2     7  6  8       1       1  1  1     1     1  1  1       9
*                                                  2  1  3     0     5  4  6
*
***********************************************************************************************************/
static PredictionStructureEntry four_level_hierarchical_pred_struct[] = {
    {
        0, // GOP Index 0 - Temporal Layer
        0 // GOP Index 0 - Decode Order
    },
    {
        3, // GOP Index 1 - Temporal Layer
        3 // GOP Index 1 - Decode Order
    },
    {
        2, // GOP Index 2 - Temporal Layer
        2 // GOP Index 2 - Decode Order
    },
    {
        3, // GOP Index 3 - Temporal Layer
        4 // GOP Index 3 - Decode Order
    },
    {
        1, // GOP Index 4 - Temporal Layer
        1 // GOP Index 4 - Decode Order
    },
    {
        3, // GOP Index 5 - Temporal Layer
        6 // GOP Index 5 - Decode Order
    },
    {
        2, // GOP Index 6 - Temporal Layer
        5 // GOP Index 6 - Decode Order
    },
    {
        3, // GOP Index 7 - Temporal Layer
        7 // GOP Index 7 - Decode Order
    }
};

/***********************************************************************************************************
* Five-Level Level Hierarchical
*
*           b     b           b     b               b     b           b     b              Temporal Layer 4
*          / \   / \         / \   / \             / \   / \         / \   / \
*         /   \ /   \       /   \ /   \           /   \ /   \       /   \ /   \
*        /     b     \     /     b     \         /     b     \     /     b     \           Temporal Layer 3
*       /     / \     \   /     / \     \       /     / \     \   /     / \     \
*      /     /   \     \ /     /   \     \     /     /   \     \ /     /   \     \
*     /     /     ------b------     \     \   /     /     ------b------     \     \        Temporal Layer 2
*    /     /           / \           \     \ /     /           / \           \     \
*   /     /           /   \-----------------b------------------   \           \     \      Temporal Layer 1
*  /     /           /                     / \                     \           \     \
* I-----------------------------------------------------------------------------------b    Temporal Layer 0
*
* Display Order:
*  0        1  2  3     4     5  6  7       8       9  1  1     1     1  1  1         1
*                                                      0  1     2     3  4  5         6
*
* Coding Order:
*  0        5  4  6     3     8  7  9       2       1  1  1     1     1  1  1         1
*                                                   2  1  3     0     5  4  6
*
***********************************************************************************************************/
static PredictionStructureEntry five_level_hierarchical_pred_struct[] = {
    {
        0, // GOP Index 0 - Temporal Layer
        0 // GOP Index 0 - Decode Order
    },
    {
        4, // GOP Index 1 - Temporal Layer
        4 // GOP Index 1 - Decode Order
    },
    {
        3, // GOP Index 2 - Temporal Layer
        3 // GOP Index 2 - Decode Order
    },
    {
        4, // GOP Index 3 - Temporal Layer
        5 // GOP Index 3 - Decode Order
    },
    {
        2, // GOP Index 4 - Temporal Layer
        2 // GOP Index 4 - Decode Order
    },
    {
        4, // GOP Index 5 - Temporal Layer
        7 // GOP Index 5 - Decode Order
    },
    {
        3, // GOP Index 6 - Temporal Layer
        6 // GOP Index 6 - Decode Order
    },
    {
        4, // GOP Index 7 - Temporal Layer
        8 // GOP Index 7 - Decode Order
    },
    {
        1, // GOP Index 8 - Temporal Layer
        1 // GOP Index 8 - Decode Order
    },
    {
        4, // GOP Index 9 - Temporal Layer
        11 // GOP Index 9 - Decode Order
    },
    {
        3, // GOP Index 10 - Temporal Layer
        10 // GOP Index 10 - Decode Order
    },
    {
        4, // GOP Index 11 - Temporal Layer
        12 // GOP Index 11 - Decode Order
    },
    {
        2, // GOP Index 12 - Temporal Layer
        9 // GOP Index 12 - Decode Order
    },
    {
        4, // GOP Index 13 - Temporal Layer
        14 // GOP Index 13 - Decode Order
    },
    {
        3, // GOP Index 14 - Temporal Layer
        13 // GOP Index 14 - Decode Order

    },
    {
        4, // GOP Index 15 - Temporal Layer
        15 // GOP Index 15 - Decode Order
    }
};

/**********************************************************************************************************************************************************************************************************************
* Six-Level Level Hierarchical
*
*
*              b     b           b     b               b     b           b     b                   b     b           b     b               b     b           b     b               Temporal Layer 5
*             / \   / \         / \   / \             / \   / \         / \   / \                 / \   / \         / \   / \             / \   / \         / \   / \
*            /   \ /   \       /   \ /   \           /   \ /   \       /   \ /   \               /   \ /   \       /   \ /   \           /   \ /   \       /   \ /   \
*           /     b     \     /     b     \         /     b     \     /     b     \             /     b     \     /     b     \         /     b     \     /     b     \            Temporal Layer 4
*          /     / \     \   /     / \     \       /     / \     \   /     / \     \           /     / \     \   /     / \     \       /     / \     \   /     / \     \
*         /     /   \     \ /     /   \     \     /     /   \     \ /     /   \     \         /     /   \     \ /     /   \     \     /     /   \     \ /     /   \     \
*        /     /     ------b------     \     \   /     /     ------b------     \     \       /     /     ------b------     \     \   /     /     ------b------     \     \         Temporal Layer 3
*       /     /           / \           \     \ /     /           / \           \     \     /     /           / \           \     \ /     /           / \           \     \
*      /     /           /   \-----------------b------------------   \           \     \   /     /           /   \-----------------b------------------   \           \     \       Temporal Layer 2
*     /     /           /                     / \                     \           \     \ /     /           /                     / \                     \           \     \
*    /     /           /                     /   \---------------------------------------b---------------------------------------/   \                     \           \     \     Temporal Layer 1
*   /     /           /                     /                                           / \                                           \                     \           \     \
*  I---------------------------------------------------------------------------------------------------------------------------------------------------------------------------b   Temporal Layer 0
*
* Display Order:
*  0           1  2  3     4     5  6  7       8       9  1  1     1     1  1  1         1         1  1  1     2     2  2  2       2       2  2  2     2     2  3  3           3
*                                                         0  1     2     3  4  5         6         7  8  9     0     1  2  3       4       5  6  7     8     9  0  1           2
*
* Coding Order:
*  0           6  5  7     4     9  8  1       3       1  1  1     1     1  1  1         2         2  2  2     1     2  2  2       1       2  2  2     2     3  3  3           1
*                                      0               3  2  4     1     6  5  7                   1  0  2     9     4  3  5       8       8  7  9     6     1  0  2
*
**********************************************************************************************************************************************************************************************************************/
static PredictionStructureEntry six_level_hierarchical_pred_struct[] = {
    {
        0, // GOP Index 0 - Temporal Layer
        0 // GOP Index 0 - Decode Order
    },
    {
        5, // GOP Index 1 - Temporal Layer
        5 // GOP Index 1 - Decode Order
    },
    {
        4, // GOP Index 2 - Temporal Layer
        4 // GOP Index 2 - Decode Order
    },
    {
        5, // GOP Index 3 - Temporal Layer
        6 // GOP Index 3 - Decode Order
    },
    {
        3, // GOP Index 4 - Temporal Layer
        3 // GOP Index 4 - Decode Order
    },
    {
        5, // GOP Index 5 - Temporal Layer
        8 // GOP Index 5 - Decode Order
    },
    {
        4, // GOP Index 6 - Temporal Layer
        7 // GOP Index 6 - Decode Order
    },
    {
        5, // GOP Index 7 - Temporal Layer
        9 // GOP Index 7 - Decode Order
    },
    {
        2, // GOP Index 8 - Temporal Layer
        2 // GOP Index 8 - Decode Order
    },
    {
        5, // GOP Index 9 - Temporal Layer
        12 // GOP Index 9 - Decode Order
    },
    {
        4, // GOP Index 10 - Temporal Layer
        11 // GOP Index 10 - Decode Order
    },
    {
        5, // GOP Index 11 - Temporal Layer
        13 // GOP Index 11 - Decode Order
    },
    {
        3, // GOP Index 12 - Temporal Layer
        10 // GOP Index 12 - Decode Order
    },
    {
        5, // GOP Index 13 - Temporal Layer
        15 // GOP Index 13 - Decode Order
    },
    {
        4, // GOP Index 14 - Temporal Layer
        14 // GOP Index 14 - Decode Order
    },
    {
        5, // GOP Index 15 - Temporal Layer
        16 // GOP Index 15 - Decode Order
    },
    {
        1, // GOP Index 16 - Temporal Layer
        1 // GOP Index 16 - Decode Order
    },
    {
        5, // GOP Index 17 - Temporal Layer
        20 // GOP Index 17 - Decode Order
    },
    {
        4, // GOP Index 18 - Temporal Layer
        19 // GOP Index 18 - Decode Order
    },
    {
        5, // GOP Index 19 - Temporal Layer
        21 // GOP Index 19 - Decode Order
    },
    {
        3, // GOP Index 20 - Temporal Layer
        18 // GOP Index 20 - Decode Order
    },
    {
        5, // GOP Index 21 - Temporal Layer
        23 // GOP Index 21 - Decode Order
    },
    {
        4, // GOP Index 22 - Temporal Layer
        22 // GOP Index 22 - Decode Order
    },
    {
        5, // GOP Index 23 - Temporal Layer
        24 // GOP Index 23 - Decode Order
    },
    {
        2, // GOP Index 24 - Temporal Layer
        17 // GOP Index 24 - Decode Order
    },
    {
        5, // GOP Index 25 - Temporal Layer
        27 // GOP Index 25 - Decode Order
    },
    {
        4, // GOP Index 26 - Temporal Layer
        26 // GOP Index 26 - Decode Order
    },
    {
        5, // GOP Index 27 - Temporal Layer
        28 // GOP Index 27 - Decode Order
    },
    {
        3, // GOP Index 28 - Temporal Layer
        25 // GOP Index 28 - Decode Order
    },
    {
        5, // GOP Index 29 - Temporal Layer
        30 // GOP Index 29 - Decode Order
    },
    {
        4, // GOP Index 30 - Temporal Layer
        29 // GOP Index 30 - Decode Order
    },
    {
        5, // GOP Index 31 - Temporal Layer
        31 // GOP Index 31 - Decode Order
    }
};
// clang-format on

/************************************************
     * Prediction Structure Config
     *   Contains a collection of basic control data
     *   for the basic prediction structure.
     ************************************************/
typedef struct PredictionStructureConfig {
    uint32_t                  entry_count;
    PredictionStructureEntry* entry_array;
} PredictionStructureConfig;

/************************************************
 * Prediction Structure Config Array
 ************************************************/
static const PredictionStructureConfig g_prediction_structure_config_array[] = {
    {1, flat_pred_struct},
    {2, two_level_hierarchical_pred_struct},
    {4, three_level_hierarchical_pred_struct},
    {8, four_level_hierarchical_pred_struct},
    {16, five_level_hierarchical_pred_struct},
    {32, six_level_hierarchical_pred_struct},
    {0, (PredictionStructureEntry*)NULL} // Terminating Code, must always come last!
};

/************************************************
 * Get Prediction Structure
 ************************************************/
PredictionStructure* svt_aom_get_prediction_structure(PredictionStructureGroup* pred_struct_group_ptr,
                                                      PredStructure pred_struct, uint32_t levels_of_hierarchy) {
    // Determine the Index value
    uint32_t pred_struct_index = PRED_STRUCT_INDEX(levels_of_hierarchy, pred_struct);

    return pred_struct_group_ptr->prediction_structure_ptr_array[pred_struct_index];
}

static void prediction_structure_dctor(EbPtr p) {
    PredictionStructure* obj = (PredictionStructure*)p;
    if (obj->pred_struct_entry_ptr_array) {
        EB_FREE_2D(obj->pred_struct_entry_ptr_array);
    }
}

/********************************************************************************************
 * Prediction Structure Ctor
 *
 * GOP Type:
 *   For Low Delay P, eliminate Ref List 0
 *   Fow Low Delay b, copy Ref List 0 into Ref List 1
 *   For Random Access, leave config as is
 *
 * number_of_references:
 *   Clip the Ref Lists
 *
 *  Summary:
 *
 *  The Pred Struct Ctor constructs the Reference Lists, Dependent Lists, and RPS for each
 *    valid prediction structure position. The full prediction structure is composed of four
 *    sections:
 *    a. Leading Pictures
 *    b. Initialization Pictures
 *    c. Steady-state Pictures
 *    d. Trailing Pictures
 *
 *  by definition, the Prediction Structure Config describes the Steady-state Picture
 *    Set. From the PS Config, the other sections are determined by following a simple
 *    set of construction rules. These rules are:
 *    -Leading Pictures use only List 1 of the Steady-state for forward-prediction
 *    -Init Pictures don't violate CRA mechanics
 *    -Steady-state Pictures come directly from the PS Config
 *    -Following pictures use only List 0 of the Steady-state for rear-prediction
 *
 *  In general terms, Leading and Trailing pictures are useful when trying to reduce
 *    the number of base-layer pictures in the presense of scene changes.  Trailing
 *    pictures are also useful for terminating sequences.  Init pictures are needed
 *    when using multiple references that expand outside of a Prediction Structure.
 *    Steady-state pictures are the normal use cases.
 *
 *  Leading and Trailing Pictures are not applicable to Low Delay prediction structures.
 *
 *  Below are a set of example PS diagrams
 *
 *  Low-delay P, Flat, 2 reference:
 *
 *                    I---P---P
 *
 *  Display Order     0   1   2
 *
 *  Sections:
 *    Let PredStructSize = N
 *    Leading Pictures:     [null]  Size: 0
 *    Init Pictures:        [0-1]   Size: Ceil(MaxReference, N) - N + 1
 *    Stead-state Pictures: [2]     Size: N
 *    Trailing Pictures:    [null]  Size: 0
 *    ------------------------------------------
 *      Total Size: Ceil(MaxReference, N) + 1
 *
 *  Low-delay b, 3-level, 2 references:
 *
 *                         b   b     b   b
 *                        /   /     /   /
 *                       /   b     /   b
 *                      /   /     /   /
 *                     I---------b-----------b
 *
 *  Display Order      0   1 2 3 4   5 6 7   8
 *
 *  Sections:
 *    Let PredStructSize = N
 *    Leading Pictures:     [null]  Size: 0
 *    Init Pictures:        [1-4]   Size: Ceil(MaxReference, N) - N + 1
 *    Stead-state Pictures: [5-8]   Size: N
 *    Trailing Pictures:    [null]  Size: 0
 *    ------------------------------------------
 *      Total Size: Ceil(MaxReference, N) + 1
 *
 *  Random Access, 3-level structure with 3 references:
 *
 *                   p   p       b   b       b   b       b   b       p   p
 *                    \   \     / \ / \     / \ / \     / \ / \     /   /
 *                     P   \   /   b   \   /   b   \   /   b   \   /   P
 *                      \   \ /   / \   \ /   / \   \ /   / \   \ /   /
 *                       ----I-----------b-----------b-----------b----
 *  Display Order:   0 1 2   3   4 5 6   7   8 9 1   1   1 1 1   1   1 1 1
 *                                               0   1   2 3 4   5   6 7 8
 *
 *  Decode Order:    2 1 3   0   6 5 7   4   1 9 1   8   1 1 1   1   1 1 1
 *                                           0   1       4 3 5   2   6 7 8
 *
 *  Sections:
 *    Let PredStructSize = N
 *    Leading Pictures:      [0-2]   Size: N - 1
 *    Init Pictures:         [3-11]  Size: Ceil(MaxReference, N) - N + 1
 *    Steady-state Pictures: [12-15] Size: N
 *    Trailing Pictures:     [16-18] Size: N - 1
 *    ------------------------------------------
 *      Total Size: 2*N + Ceil(MaxReference, N) - 1
 *
 *  Encoding Order:
 *                   -------->----------->----------->-----------|------->
 *                                                   |           |
 *                                                   |----<------|
 *
 *
 *  Timeline:
 *
 *  The timeline is a tool that is used to determine for how long a
 *    picture should be preserved for future reference. The concept of
 *    future reference is equivalently defined as dependence.  The RPS
 *    mechanism works by signaling for each picture in the DPB whether
 *    it is used for direct reference, kept for future reference, or
 *    discarded.  The timeline merely provides a means of determing
 *    the reference picture states at each prediction structure position.
 *    Its also important to note that all signaling should be done relative
 *    to decode order, not display order. Display order is irrelevant except
 *    for signaling the POC.
 *
 *  Timeline Example: 3-Level Hierarchical with Leading and Trailing Pictures, 3 References
 *
 *                   p   p       b   b       b   b       b   b       p   p   Temporal Layer 2
 *                    \   \     / \ / \     / \ / \     / \ / \     /   /
 *                     P   \   /   b   \   /   b   \   /   b   \   /   P     Temporal Layer 1
 *                      \   \ /   / \   \ /   / \   \ /   / \   \ /   /
 *                       ----I-----------b-----------b-----------b----       Temporal Layer 0
 *
 *  Decode Order:    2 1 3   0   6 5 7   4   1 9 1   8   1 1 1   1   1 1 1
 *                                           0   1       4 3 5   2   6 7 8
 *
 *  Display Order:   0 1 2   3   4 5 6   7   8 9 1   1   1 1 1   1   1 1 1
 *                                               0   1   2 3 4   5   6 7 8
 *            X --->
 *
 *                                1 1 1 1 1 1 1 1 1   DECODE ORDER
 *            0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8
 *
 *         |----------------------------------------------------------
 *         |
 *  Y D  0 |  \ x---x-x-x-x---x-x-x---x-x-x
 *  | E  1 |    \ x
 *  | C  2 |      \
 *  | O  3 |        \
 *  v D  4 |          \ x---x-x-x-x---x-x-x---x-x-x
 *    E  5 |            \ x-x
 *       6 |              \
 *    O  7 |                \
 *    R  8 |                  \ x---x-x-x-x---x-x-x
 *    D  9 |                    \ x-x
 *    E 10 |                      \
 *    R 11 |                        \
 *      12 |                          \ x---x-x-x-x
 *      13 |                            \ x-x
 *      14 |                              \
 *      15 |                                \
 *      16 |                                  \
 *      17 |                                    \ x
 *      18 |                                      \
 *
 *  Interpreting the timeline:
 *
 *  The most important detail to keep in mind is that all signaling
 *    is done in Decode Order space. The symbols mean the following:
 *    'x' directly referenced picture
 *    '-' picture kept for future reference
 *    ' ' not referenced, inferred discard
 *    '\' eqivalent to ' ', deliminiter that nothing can be to the left of
 *
 *  The basic steps for constructing the timeline are to increment through
 *    each position in the prediction structure (Y-direction on the timeline)
 *    and mark the appropriate state: directly referenced, kept for future reference,
 *    or discarded.  As shown, base-layer pictures are referenced much more
 *    frequently than by the other layers.
 *
 *  The RPS is constructed by looking at each 'x' position in the timeline and
 *    signaling each 'y' reference as depicted in the timeline. DPB analysis is
 *    fairly straigtforward - the total number of directly-referenced and
 *    kept-for-future-reference pictures should not exceed the DPB size.
 *
 *  The RPS Ctor code follows these construction steps.
 ******************************************************************************************/
static EbErrorType prediction_structure_ctor(PredictionStructure*             pred_struct,
                                             const PredictionStructureConfig* pred_struct_cfg,
                                             const PredStructure              pred_type) {
    pred_struct->dctor = prediction_structure_dctor;

    pred_struct->pred_type = pred_type;

    // Set total Entry Count
    pred_struct->pred_struct_entry_count = pred_struct_cfg->entry_count;

    // Set the Section Indices
    pred_struct->init_pic_index = 0;

    // Allocate the entry array
    EB_CALLOC_2D(pred_struct->pred_struct_entry_ptr_array, pred_struct->pred_struct_entry_count, 1);

    //----------------------------------------
    // Construct Steady-state Pictures
    //   -Copy directly from the Config
    //----------------------------------------
    for (unsigned int entry_idx = 0; entry_idx < pred_struct->pred_struct_entry_count; ++entry_idx) {
        PredictionStructureEntry* cfg_entry  = &pred_struct_cfg->entry_array[entry_idx];
        PredictionStructureEntry* pred_entry = pred_struct->pred_struct_entry_ptr_array[entry_idx];

        // Set the Temporal Layer Index
        pred_entry->temporal_layer_index = cfg_entry->temporal_layer_index;

        // Set the Decode Order
        pred_entry->decode_order = (pred_type == RANDOM_ACCESS) ? cfg_entry->decode_order : entry_idx;
    }

    return EB_ErrorNone;
}

static void prediction_structure_group_dctor(EbPtr p) {
    PredictionStructureGroup* obj = (PredictionStructureGroup*)p;
    EB_DELETE_PTR_ARRAY(obj->prediction_structure_ptr_array, obj->prediction_structure_count);
}

/*************************************************
 * Prediction Structure Group Ctor
 *
 * Summary: Converts the Prediction Structure Config
 *   into the usable Prediction Structure with RPS and
 *   Dependent List control.
 *
 * From each config, several prediction structures
 *   are created. These include:
 *   -Variable Number of References
 *      # [1 - 4]
 *   -Temporal Layers
 *      # [1 - 6]
 *   -GOP Type
 *      # Low Delay P
 *      # Low Delay b
 *      # Random Access
 *
 *************************************************/
EbErrorType svt_aom_prediction_structure_group_ctor(PredictionStructureGroup* pred_struct_group_ptr) {
    pred_struct_group_ptr->dctor = prediction_structure_group_dctor;

    pred_struct_group_ptr->prediction_structure_count = MAX_TEMPORAL_LAYERS * PRED_TOTAL_COUNT;
    EB_ALLOC_PTR_ARRAY(pred_struct_group_ptr->prediction_structure_ptr_array,
                       pred_struct_group_ptr->prediction_structure_count);
    for (uint32_t hierarchical_levels = 0; hierarchical_levels < MAX_TEMPORAL_LAYERS; hierarchical_levels++) {
        for (PredStructure pred_type = 0; pred_type < PRED_TOTAL_COUNT; ++pred_type) {
            uint32_t pred_struct_index = PRED_STRUCT_INDEX(hierarchical_levels, pred_type);

            EB_NEW(pred_struct_group_ptr->prediction_structure_ptr_array[pred_struct_index],
                   prediction_structure_ctor,
                   &g_prediction_structure_config_array[hierarchical_levels],
                   pred_type);
        }
    }
    return EB_ErrorNone;
}
