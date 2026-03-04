; Copyright © 2018-2021, VideoLAN and dav1d authors
; Copyright © 2018-2021, Two Orioles, LLC
; All rights reserved.
;
; Redistribution and use in source and binary forms, with or without
; modification, are permitted provided that the following conditions are met:
;
; 1. Redistributions of source code must retain the above copyright notice, this
;    list of conditions and the following disclaimer.
;
; 2. Redistributions in binary form must reproduce the above copyright notice,
;    this list of conditions and the following disclaimer in the documentation
;    and/or other materials provided with the distribution.
;
; THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
; ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
; WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
; DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
; ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
; (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
; ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
; (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
; SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

%include "dav1d_x86inc.asm"

%if ARCH_X86_64

SECTION_RODATA 32

; dav1d_obmc_masks[] with 64-x interleaved
obmc_masks:     db  0,  0,  0,  0
                ; 2
                db 45, 19, 64,  0
                ; 4
                db 39, 25, 50, 14, 59,  5, 64,  0
                ; 8
                db 36, 28, 42, 22, 48, 16, 53, 11, 57,  7, 61,  3, 64,  0, 64,  0
                ; 16
                db 34, 30, 37, 27, 40, 24, 43, 21, 46, 18, 49, 15, 52, 12, 54, 10
                db 56,  8, 58,  6, 60,  4, 61,  3, 64,  0, 64,  0, 64,  0, 64,  0
                ; 32
                db 33, 31, 35, 29, 36, 28, 38, 26, 40, 24, 41, 23, 43, 21, 44, 20
                db 45, 19, 47, 17, 48, 16, 50, 14, 51, 13, 52, 12, 53, 11, 55,  9
                db 56,  8, 57,  7, 58,  6, 59,  5, 60,  4, 60,  4, 61,  3, 62,  2
                db 64,  0, 64,  0, 64,  0, 64,  0, 64,  0, 64,  0, 64,  0, 64,  0

blend_shuf:     db  0,  1,  0,  1,  0,  1,  0,  1,  2,  3,  2,  3,  2,  3,  2,  3

pb_64:   times 4 db 64
pw_512:  times 2 dw 512

%macro BIDIR_JMP_TABLE 2-*
    %xdefine %1_%2_table (%%table - 2*%3)
    %xdefine %%base %1_%2_table
    %xdefine %%prefix mangle(private_prefix %+ _%1_8bpc_%2)
    %%table:
    %rep %0 - 2
        dd %%prefix %+ .w%3 - %%base
        %rotate 1
    %endrep
%endmacro

BIDIR_JMP_TABLE  blend, avx2,              4, 8, 16, 32
BIDIR_JMP_TABLE  blend_v, avx2,         2, 4, 8, 16, 32
BIDIR_JMP_TABLE  blend_h, avx2,         2, 4, 8, 16, 32, 32, 32

SECTION .text

INIT_YMM avx2

cglobal blend_8bpc, 3, 7, 7, dst, ds, tmp, w, h, mask
%define base r6-blend_avx2_table
    lea                  r6, [blend_avx2_table]
    tzcnt                wd, wm
    movifnidn         maskq, maskmp
    movifnidn            hd, hm
    movsxd               wq, dword [r6+wq*4]
    vpbroadcastd         m4, [base+pb_64]
    vpbroadcastd         m5, [base+pw_512]
    sub                tmpq, maskq
    add                  wq, r6
    lea                  r6, [dsq*3]
    jmp                  wq
.w4:
    movd                xm0, [dstq+dsq*0]
    pinsrd              xm0, [dstq+dsq*1], 1
    vpbroadcastd        xm1, [dstq+dsq*2]
    pinsrd              xm1, [dstq+r6   ], 3
    mova                xm6, [maskq]
    psubb               xm3, xm4, xm6
    punpcklbw           xm2, xm3, xm6
    punpckhbw           xm3, xm6
    mova                xm6, [maskq+tmpq]
    add               maskq, 4*4
    punpcklbw           xm0, xm6
    punpckhbw           xm1, xm6
    pmaddubsw           xm0, xm2
    pmaddubsw           xm1, xm3
    pmulhrsw            xm0, xm5
    pmulhrsw            xm1, xm5
    packuswb            xm0, xm1
    movd       [dstq+dsq*0], xm0
    pextrd     [dstq+dsq*1], xm0, 1
    pextrd     [dstq+dsq*2], xm0, 2
    pextrd     [dstq+r6   ], xm0, 3
    lea                dstq, [dstq+dsq*4]
    sub                  hd, 4
    jg .w4
    RET
ALIGN function_align
.w8:
    movq                xm1, [dstq+dsq*0]
    movhps              xm1, [dstq+dsq*1]
    vpbroadcastq         m2, [dstq+dsq*2]
    vpbroadcastq         m3, [dstq+r6   ]
    mova                 m0, [maskq]
    mova                 m6, [maskq+tmpq]
    add               maskq, 8*4
    vpblendd             m1, m1, m2, 0x30
    vpblendd             m1, m1, m3, 0xc0
    psubb                m3, m4, m0
    punpcklbw            m2, m3, m0
    punpckhbw            m3, m0
    punpcklbw            m0, m1, m6
    punpckhbw            m1, m6
    pmaddubsw            m0, m2
    pmaddubsw            m1, m3
    pmulhrsw             m0, m5
    pmulhrsw             m1, m5
    packuswb             m0, m1
    vextracti128        xm1, m0, 1
    movq       [dstq+dsq*0], xm0
    movhps     [dstq+dsq*1], xm0
    movq       [dstq+dsq*2], xm1
    movhps     [dstq+r6   ], xm1
    lea                dstq, [dstq+dsq*4]
    sub                  hd, 4
    jg .w8
    RET
ALIGN function_align
.w16:
    mova                 m0, [maskq]
    mova                xm1, [dstq+dsq*0]
    vinserti128          m1, m1, [dstq+dsq*1], 1
    psubb                m3, m4, m0
    punpcklbw            m2, m3, m0
    punpckhbw            m3, m0
    mova                 m6, [maskq+tmpq]
    add               maskq, 16*2
    punpcklbw            m0, m1, m6
    punpckhbw            m1, m6
    pmaddubsw            m0, m2
    pmaddubsw            m1, m3
    pmulhrsw             m0, m5
    pmulhrsw             m1, m5
    packuswb             m0, m1
    mova         [dstq+dsq*0], xm0
    vextracti128 [dstq+dsq*1], m0, 1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w16
    RET
ALIGN function_align
.w32:
    mova                 m0, [maskq]
    mova                 m1, [dstq]
    mova                 m6, [maskq+tmpq]
    add               maskq, 32
    psubb                m3, m4, m0
    punpcklbw            m2, m3, m0
    punpckhbw            m3, m0
    punpcklbw            m0, m1, m6
    punpckhbw            m1, m6
    pmaddubsw            m0, m2
    pmaddubsw            m1, m3
    pmulhrsw             m0, m5
    pmulhrsw             m1, m5
    packuswb             m0, m1
    mova             [dstq], m0
    add                dstq, dsq
    dec                  hd
    jg .w32
    RET

cglobal blend_v_8bpc, 4, 7, 7, dst, ds, tmp, tmps, w, h, mask
%define base r6-blend_v_avx2_table
    lea                  r6, [blend_v_avx2_table]
    tzcnt                wd, wm
    movifnidn            hd, hm
    movsxd               wq, dword [r6+wq*4]
    vpbroadcastd         m5, [base+pw_512]
    add                  wq, r6
    add               maskq, obmc_masks-blend_v_avx2_table
    jmp                  wq
.w2:
    vpbroadcastd        xm2, [maskq+2*2]
.w2_s0_loop:
    movd                xm0, [dstq+dsq*0]
    pinsrw              xm0, [dstq+dsq*1], 1
    movd                xm1, [tmpq]
    pinsrw              xm1, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    punpcklbw           xm0, xm1
    pmaddubsw           xm0, xm2
    pmulhrsw            xm0, xm5
    packuswb            xm0, xm0
    pextrw     [dstq+dsq*0], xm0, 0
    pextrw     [dstq+dsq*1], xm0, 1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w2_s0_loop
    RET
ALIGN function_align
.w4:
    vpbroadcastq        xm2, [maskq+4*2]
.w4_loop:
    movd                xm0, [dstq+dsq*0]
    pinsrd              xm0, [dstq+dsq*1], 1
    movd                xm1, [tmpq]
    pinsrd              xm1, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    punpcklbw           xm0, xm1
    pmaddubsw           xm0, xm2
    pmulhrsw            xm0, xm5
    packuswb            xm0, xm0
    movd       [dstq+dsq*0], xm0
    pextrd     [dstq+dsq*1], xm0, 1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w4_loop
    RET
ALIGN function_align
.w8:
    mova                xm3, [maskq+8*2]
.w8_loop:
    movq                xm0, [dstq+dsq*0]
    vpbroadcastq        xm1, [dstq+dsq*1]
    movq                xm2, [tmpq]
    pinsrq              xm2, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    punpcklbw           xm0, xm2
    punpckhbw           xm1, xm2
    pmaddubsw           xm0, xm3
    pmaddubsw           xm1, xm3
    pmulhrsw            xm0, xm5
    pmulhrsw            xm1, xm5
    packuswb            xm0, xm1
    movq       [dstq+dsq*0], xm0
    movhps     [dstq+dsq*1], xm0
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w8_loop
    RET
ALIGN function_align
.w16:
    vbroadcasti128       m3, [maskq+16*2]
    vbroadcasti128       m4, [maskq+16*3]
.w16_loop:
    movu                xm1, [dstq+dsq*0]
    vinserti128          m1, m1, [dstq+dsq*1], 1
    movu                xm2, [tmpq+tmpsq*0]
    vinserti128          m2, m2, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    punpcklbw            m0, m1, m2
    punpckhbw            m1, m2
    pmaddubsw            m0, m3
    pmaddubsw            m1, m4
    pmulhrsw             m0, m5
    pmulhrsw             m1, m5
    packuswb             m0, m1
    movu         [dstq+dsq*0], xm0
    vextracti128 [dstq+dsq*1], m0, 1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w16_loop
    RET
ALIGN function_align
.w32:
    mova                xm3, [maskq+16*4]
    vinserti128          m3, m3, [maskq+16*6], 1
    mova                xm4, [maskq+16*5]
    vinserti128          m4, m4, [maskq+16*7], 1
.w32_loop:
    movu                 m1, [dstq]
    movu                 m2, [tmpq]
    add                tmpq, tmpsq
    punpcklbw            m0, m1, m2
    punpckhbw            m1, m2
    pmaddubsw            m0, m3
    pmaddubsw            m1, m4
    pmulhrsw             m0, m5
    pmulhrsw             m1, m5
    packuswb             m0, m1
    movu             [dstq], m0
    add                dstq, dsq
    dec                  hd
    jg .w32_loop
    RET

cglobal blend_h_8bpc, 5, 8, 7, dst, ds, tmp, tmps, w, h, mask
%define base r6-blend_h_avx2_table
    lea                  r6, [blend_h_avx2_table]
    mov                 r7d, wd
    tzcnt                wd, wd
    mov                  hd, hm
    movsxd               wq, dword [r6+wq*4]
    vpbroadcastd         m5, [base+pw_512]
    add                  wq, r6
    lea               maskq, [base+obmc_masks+hq*2]
    lea                  hd, [hq*3]
    shr                  hd, 2 ; h * 3/4
    lea               maskq, [maskq+hq*2]
    neg                  hq
    jmp                  wq
.w2:
    movd                xm0, [dstq+dsq*0]
    pinsrw              xm0, [dstq+dsq*1], 1
    movd                xm2, [maskq+hq*2]
    movd                xm1, [tmpq]
    pinsrw              xm1, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    punpcklwd           xm2, xm2
    punpcklbw           xm0, xm1
    pmaddubsw           xm0, xm2
    pmulhrsw            xm0, xm5
    packuswb            xm0, xm0
    pextrw     [dstq+dsq*0], xm0, 0
    pextrw     [dstq+dsq*1], xm0, 1
    lea                dstq, [dstq+dsq*2]
    add                  hq, 2
    jl .w2
    RET
ALIGN function_align
.w4:
    mova                xm3, [blend_shuf]
.w4_loop:
    movd                xm0, [dstq+dsq*0]
    pinsrd              xm0, [dstq+dsq*1], 1
    movd                xm2, [maskq+hq*2]
    movd                xm1, [tmpq]
    pinsrd              xm1, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    pshufb              xm2, xm3
    punpcklbw           xm0, xm1
    pmaddubsw           xm0, xm2
    pmulhrsw            xm0, xm5
    packuswb            xm0, xm0
    movd       [dstq+dsq*0], xm0
    pextrd     [dstq+dsq*1], xm0, 1
    lea                dstq, [dstq+dsq*2]
    add                  hq, 2
    jl .w4_loop
    RET
ALIGN function_align
.w8:
    vbroadcasti128       m4, [blend_shuf]
    shufpd               m4, m4, m4, 0x03
.w8_loop:
    vpbroadcastq         m1, [dstq+dsq*0]
    movq                xm0, [dstq+dsq*1]
    vpblendd             m0, m0, m1, 0x30
    vpbroadcastd         m3, [maskq+hq*2]
    vpbroadcastq         m2, [tmpq+tmpsq*0]
    movq                xm1, [tmpq+tmpsq*1]
    vpblendd             m1, m1, m2, 0x30
    lea                tmpq, [tmpq+tmpsq*2]
    pshufb               m3, m4
    punpcklbw            m0, m1
    pmaddubsw            m0, m3
    pmulhrsw             m0, m5
    vextracti128        xm1, m0, 1
    packuswb            xm0, xm1
    movhps     [dstq+dsq*0], xm0
    movq       [dstq+dsq*1], xm0
    lea                dstq, [dstq+dsq*2]
    add                  hq, 2
    jl .w8_loop
    RET
ALIGN function_align
.w16:
    vbroadcasti128       m4, [blend_shuf]
    shufpd               m4, m4, m4, 0x0c
.w16_loop:
    movu                xm1, [dstq+dsq*0]
    vinserti128          m1, m1, [dstq+dsq*1], 1
    vpbroadcastd         m3, [maskq+hq*2]
    movu                xm2, [tmpq+tmpsq*0]
    vinserti128          m2, m2, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    pshufb               m3, m4
    punpcklbw            m0, m1, m2
    punpckhbw            m1, m2
    pmaddubsw            m0, m3
    pmaddubsw            m1, m3
    pmulhrsw             m0, m5
    pmulhrsw             m1, m5
    packuswb             m0, m1
    movu         [dstq+dsq*0], xm0
    vextracti128 [dstq+dsq*1], m0, 1
    lea                dstq, [dstq+dsq*2]
    add                  hq, 2
    jl .w16_loop
    RET
ALIGN function_align
.w32: ; w32/w64/w128
    sub                 dsq, r7
    sub                 tmpsq, r7
.w32_loop0:
    vpbroadcastw         m3, [maskq+hq*2]
    mov                  wd, r7d
.w32_loop:
    movu                 m1, [dstq]
    movu                 m2, [tmpq]
    add                tmpq, 32
    punpcklbw            m0, m1, m2
    punpckhbw            m1, m2
    pmaddubsw            m0, m3
    pmaddubsw            m1, m3
    pmulhrsw             m0, m5
    pmulhrsw             m1, m5
    packuswb             m0, m1
    movu             [dstq], m0
    add                dstq, 32
    sub                  wd, 32
    jg .w32_loop
    add                dstq, dsq
    add                tmpq, tmpsq
    inc                  hq
    jl .w32_loop0
    RET

%endif ; ARCH_X86_64
