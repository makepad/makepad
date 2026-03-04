;
; Copyright (c) 2016, Alliance for Open Media. All rights reserved
;
; This source code is subject to the terms of the BSD 2 Clause License and
; the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
; was not distributed with this source code in the LICENSE file, you can
; obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
; Media Patent License 1.0 was not distributed with this source code in the
; PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
;

;

%include "x86inc.asm"

SECTION_RODATA
pw_4:  times 8 dw 4
pw_8:  times 8 dw 8
pw_16: times 4 dd 16
pw_32: times 4 dd 32

SECTION .text
INIT_XMM sse2
cglobal highbd_dc_predictor_4x4, 4, 5, 4, dst, stride, above, left, goffset
  GET_GOT     goffsetq

  movq                  m0, [aboveq]
  movq                  m2, [leftq]
  paddw                 m0, m2
  pshuflw               m1, m0, 0xe
  paddw                 m0, m1
  pshuflw               m1, m0, 0x1
  paddw                 m0, m1
  paddw                 m0, [GLOBAL(pw_4)]
  psraw                 m0, 3
  pshuflw               m0, m0, 0x0
  movq    [dstq          ], m0
  movq    [dstq+strideq*2], m0
  lea                 dstq, [dstq+strideq*4]
  movq    [dstq          ], m0
  movq    [dstq+strideq*2], m0

  RESTORE_GOT
  RET

INIT_XMM sse2
cglobal highbd_dc_predictor_8x8, 4, 5, 4, dst, stride, above, left, goffset
  GET_GOT     goffsetq

  pxor                  m1, m1
  movu                  m0, [aboveq]
  movu                  m2, [leftq]
  DEFINE_ARGS dst, stride, stride3, one
  mov                 oned, 0x00010001
  lea             stride3q, [strideq*3]
  movd                  m3, oned
  pshufd                m3, m3, 0x0
  paddw                 m0, m2
  pmaddwd               m0, m3
  packssdw              m0, m1
  pmaddwd               m0, m3
  packssdw              m0, m1
  pmaddwd               m0, m3
  paddw                 m0, [GLOBAL(pw_8)]
  psrlw                 m0, 4
  pshuflw               m0, m0, 0x0
  punpcklqdq            m0, m0
  movu   [dstq           ], m0
  movu   [dstq+strideq*2 ], m0
  movu   [dstq+strideq*4 ], m0
  movu   [dstq+stride3q*2], m0
  lea                 dstq, [dstq+strideq*8]
  movu   [dstq           ], m0
  movu   [dstq+strideq*2 ], m0
  movu   [dstq+strideq*4 ], m0
  movu   [dstq+stride3q*2], m0

  RESTORE_GOT
  RET

INIT_XMM sse2
cglobal highbd_v_predictor_4x4, 3, 3, 1, dst, stride, above
  movq                  m0, [aboveq]
  movq    [dstq          ], m0
  movq    [dstq+strideq*2], m0
  lea                 dstq, [dstq+strideq*4]
  movq    [dstq          ], m0
  movq    [dstq+strideq*2], m0
  RET

INIT_XMM sse2
cglobal highbd_v_predictor_8x8, 3, 3, 1, dst, stride, above
  movu                  m0, [aboveq]
  DEFINE_ARGS dst, stride, stride3
  lea             stride3q, [strideq*3]
  movu   [dstq           ], m0
  movu   [dstq+strideq*2 ], m0
  movu   [dstq+strideq*4 ], m0
  movu   [dstq+stride3q*2], m0
  lea                 dstq, [dstq+strideq*8]
  movu   [dstq           ], m0
  movu   [dstq+strideq*2 ], m0
  movu   [dstq+strideq*4 ], m0
  movu   [dstq+stride3q*2], m0
  RET




