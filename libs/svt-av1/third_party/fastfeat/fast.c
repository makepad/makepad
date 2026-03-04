// clang-format off
#include <stdlib.h>
#include "fast.h"


xy* svt_aom_fast9_detect_nonmax(const byte* im, int xsize, int ysize, int stride, int b, int* ret_num_corners)
{
    xy* corners;
    int num_corners = 0;
    int* scores;
    xy* nonmax;

    corners = svt_aom_fast9_detect(im, xsize, ysize, stride, b, &num_corners);
    scores = svt_aom_fast9_score(im, stride, corners, num_corners, b);
    nonmax = svt_aom_nonmax_suppression(corners, scores, num_corners, ret_num_corners);

    free(corners);
    free(scores);

    return nonmax;
}
// clang-format on
