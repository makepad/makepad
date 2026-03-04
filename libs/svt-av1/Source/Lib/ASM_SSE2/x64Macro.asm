;
; Copyright(c) 2019 Intel Corporation
;
; This source code is subject to the terms of the BSD 2 Clause License and
; the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
; was not distributed with this source code in the LICENSE file, you can
; obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
; Media Patent License 1.0 was not distributed with this source code in the
; PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
;

%macro MACRO_INIT_MMX 3
    mov             %2,             %3
    movq            %1,             %2
%endmacro

%macro MACRO_INIT_XMM 3
    mov             %2,             %3
    movq            %1,             %2
    punpcklqdq      %1,             %1
%endmacro

%macro MACRO_UNPACK 13
    movdqa          xmm%10,         xmm%2
    movdqa          xmm%11,         xmm%4
    movdqa          xmm%12,         xmm%6
    movdqa          xmm%13,         xmm%8
    punpckl%1       xmm%2,          xmm%3       ; 07 06 05 04 03 02 01 00 wd: 13 03 12 02 11 01 10 00 dq: 31 21 11 01 30 20 10 00 qdq: 70 60 50 40 30 20 10 00
    punpckh%1       xmm%10,         xmm%3       ; 17 16 15 14 13 12 11 10     17 07 16 06 15 05 14 04     33 23 13 03 32 22 12 02      71 61 51 41 31 21 11 01
    punpckl%1       xmm%4,          xmm%5       ; 27 26 25 24 23 22 21 20     33 23 32 22 31 21 30 20     35 25 15 05 34 24 14 04      72 62 52 42 32 22 12 02
    punpckh%1       xmm%11,         xmm%5       ; 37 36 35 34 33 32 31 30     37 27 36 26 35 25 34 24     37 27 17 07 36 26 16 06      73 63 53 43 33 23 13 03
    punpckl%1       xmm%6,          xmm%7       ; 47 46 45 44 43 42 41 40     53 43 52 42 51 41 50 40     71 61 51 41 70 60 50 40      74 64 54 44 34 24 14 04
    punpckh%1       xmm%12,         xmm%7       ; 57 56 55 54 53 52 51 50     57 47 56 46 55 45 54 44     73 63 53 43 72 62 52 42      75 65 55 45 35 25 15 05
    punpckl%1       xmm%8,          xmm%9       ; 67 66 65 64 63 62 61 60     73 63 72 62 71 61 70 60     75 65 55 45 74 64 54 44      76 66 56 46 36 26 16 06
    punpckh%1       xmm%13,         xmm%9       ; 77 76 75 74 73 72 71 70     77 67 76 66 75 65 74 64     77 67 57 47 76 66 56 46      77 67 57 47 37 27 17 07
%endmacro
