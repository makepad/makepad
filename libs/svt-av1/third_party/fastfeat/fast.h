// clang-format off
#ifndef FAST_H
#define FAST_H

typedef struct { int x, y; } xy;
typedef unsigned char byte;

xy* svt_aom_fast9_detect(const byte* im, int xsize, int ysize, int stride, int b, int* ret_num_corners);

int* svt_aom_fast9_score(const byte* i, int stride, xy* corners, int num_corners, int b);

xy* svt_aom_fast9_detect_nonmax(const byte* im, int xsize, int ysize, int stride, int b, int* ret_num_corners);

xy* svt_aom_nonmax_suppression(const xy* corners, const int* scores, int num_corners, int* ret_num_nonmax);


#endif
// clang-format on
