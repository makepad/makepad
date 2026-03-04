; Copyright © 2021, VideoLAN and dav1d authors
; Copyright © 2021, Two Orioles, LLC
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

SECTION_RODATA 64

; dav1d_obmc_masks[] * -512
obmc_masks_avx2:
            dw      0,      0,  -9728,      0, -12800,  -7168,  -2560,      0
            dw -14336, -11264,  -8192,  -5632,  -3584,  -1536,      0,      0
            dw -15360, -13824, -12288, -10752,  -9216,  -7680,  -6144,  -5120
            dw  -4096,  -3072,  -2048,  -1536,      0,      0,      0,      0
            dw -15872, -14848, -14336, -13312, -12288, -11776, -10752, -10240
            dw  -9728,  -8704,  -8192,  -7168,  -6656,  -6144,  -5632,  -4608
            dw  -4096,  -3584,  -3072,  -2560,  -2048,  -2048,  -1536,  -1024
            dw      0,      0,      0,      0,      0,      0,      0,      0

blend_shuf:     db 0,  1,  0,  1,  0,  1,  0,  1,  2,  3,  2,  3,  2,  3,  2,  3
pw_m512:       times 2 dw -512

%macro BIDIR_JMP_TABLE 2-*
    %xdefine %1_%2_table (%%table - 2*%3)
    %xdefine %%base %1_%2_table
    %xdefine %%prefix mangle(private_prefix %+ _%1_16bpc_%2)
    %%table:
    %rep %0 - 2
        dd %%prefix %+ .w%3 - %%base
        %rotate 1
    %endrep
%endmacro

BIDIR_JMP_TABLE blend,      avx2,    4, 8, 16, 32
BIDIR_JMP_TABLE blend_v,    avx2, 2, 4, 8, 16, 32
BIDIR_JMP_TABLE blend_h,    avx2, 2, 4, 8, 16, 32, 64, 128

SECTION .text

INIT_YMM avx2
; (a * (64 - m) + b * m + 32) >> 6
; = (((b - a) * m + 32) >> 6) + a
; = (((b - a) * (m << 9) + 16384) >> 15) + a
;   except m << 9 overflows int16_t when m == 64 (which is possible),
;   but if we negate m it works out (-64 << 9 == -32768).
; = (((a - b) * (m * -512) + 16384) >> 15) + a
cglobal blend_16bpc, 3, 7, 7, dst, ds, tmp, w, h, mask
%define base r6-blend_avx2_table
    lea                  r6, [blend_avx2_table]
    tzcnt                wd, wm
    movifnidn            hd, hm
    movsxd               wq, dword[r6+wq*4]
    movifnidn         maskq, maskmp
    vpbroadcastd         m6, [base+pw_m512]
    add                  wq, r6
    lea                  r6, [dsq*3]
    jmp                  wq
.w4:
    pmovzxbw             m3, [maskq]
    movq                xm0, [dstq+dsq*0]
    movhps              xm0, [dstq+dsq*1]
    vpbroadcastq         m1, [dstq+dsq*2]
    vpbroadcastq         m2, [dstq+r6   ]
    vpblendd             m0, m0, m1, 0x30
    vpblendd             m0, m0, m2, 0xc0
    psubw                m1, m0, [tmpq]
    add               maskq, 16
    add                tmpq, 32
    pmullw               m3, m6
    pmulhrsw             m1, m3
    paddw                m0, m1
    vextracti128        xm1, m0, 1
    movq       [dstq+dsq*0], xm0
    movhps     [dstq+dsq*1], xm0
    movq       [dstq+dsq*2], xm1
    movhps     [dstq+r6   ], xm1
    lea                dstq, [dstq+dsq*4]
    sub                  hd, 4
    jg .w4
    RET
.w8:
    pmovzxbw             m4, [maskq+16*0]
    pmovzxbw             m5, [maskq+16*1]
    mova                xm0, [dstq+dsq*0]
    vinserti128          m0, m0, [dstq+dsq*1], 1
    mova                xm1, [dstq+dsq*2]
    vinserti128          m1, m1, [dstq+r6   ], 1
    psubw                m2, m0, [tmpq+32*0]
    psubw                m3, m1, [tmpq+32*1]
    add               maskq, 16*2
    add                tmpq, 32*2
    pmullw               m4, m6
    pmullw               m5, m6
    pmulhrsw             m2, m4
    pmulhrsw             m3, m5
    paddw                m0, m2
    paddw                m1, m3
    mova         [dstq+dsq*0], xm0
    vextracti128 [dstq+dsq*1], m0, 1
    mova         [dstq+dsq*2], xm1
    vextracti128 [dstq+r6   ], m1, 1
    lea                dstq, [dstq+dsq*4]
    sub                  hd, 4
    jg .w8
    RET
.w16:
    pmovzxbw             m4, [maskq+16*0]
    pmovzxbw             m5, [maskq+16*1]
    mova                 m0,     [dstq+dsq*0]
    psubw                m2, m0, [tmpq+ 32*0]
    mova                 m1,     [dstq+dsq*1]
    psubw                m3, m1, [tmpq+ 32*1]
    add               maskq, 16*2
    add                tmpq, 32*2
    pmullw               m4, m6
    pmullw               m5, m6
    pmulhrsw             m2, m4
    pmulhrsw             m3, m5
    paddw                m0, m2
    paddw                m1, m3
    mova       [dstq+dsq*0], m0
    mova       [dstq+dsq*1], m1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w16
    RET
.w32:
    pmovzxbw             m4, [maskq+16*0]
    pmovzxbw             m5, [maskq+16*1]
    mova                 m0,     [dstq+32*0]
    psubw                m2, m0, [tmpq+32*0]
    mova                 m1,     [dstq+32*1]
    psubw                m3, m1, [tmpq+32*1]
    add               maskq, 16*2
    add                tmpq, 32*2
    pmullw               m4, m6
    pmullw               m5, m6
    pmulhrsw             m2, m4
    pmulhrsw             m3, m5
    paddw                m0, m2
    paddw                m1, m3
    mova        [dstq+32*0], m0
    mova        [dstq+32*1], m1
    add                dstq, dsq
    dec                  hd
    jg .w32
    RET

INIT_XMM avx2
cglobal blend_v_16bpc, 4, 7, 7, dst, ds, tmp, tmps, w, h
%define base r6-blend_v_avx2_table
    lea                  r6, [blend_v_avx2_table]
    tzcnt                wd, wm
    movifnidn            hd, hm
    movsxd               wq, dword[r6+wq*4]
    add                  wq, r6
    jmp                  wq
.w2:
    vpbroadcastd         m2, [base+obmc_masks_avx2+2*2]
.w2_loop:
    movd                 m0, [dstq+dsq*0]
    pinsrd               m0, [dstq+dsq*1], 1
    movd                 m1, [tmpq+tmpsq*0]
    pinsrd               m1, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    psubw                m1, m0, m1
    pmulhrsw             m1, m2
    paddw                m0, m1
    movd       [dstq+dsq*0], m0
    pextrd     [dstq+dsq*1], m0, 1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w2_loop
    RET
.w4:
    vpbroadcastq         m2, [base+obmc_masks_avx2+4*2]
.w4_loop:
    movq                 m0, [dstq+dsq*0]
    movhps               m0, [dstq+dsq*1]
    movq                 m1, [tmpq+tmpsq*0]
    movhps               m1, [tmpq+tmpsq*1]
    psubw                m1, m0, m1
    lea                tmpq, [tmpq+tmpsq*2]
    pmulhrsw             m1, m2
    paddw                m0, m1
    movq       [dstq+dsq*0], m0
    movhps     [dstq+dsq*1], m0
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w4_loop
    RET
INIT_YMM avx2
.w8:
    vbroadcasti128       m2, [base+obmc_masks_avx2+8*2]
.w8_loop:
    movu                xm0, [dstq+dsq*0]
    vinserti128          m0, m0, [dstq+dsq*1], 1
    movu                xm1, [tmpq+tmpsq*0]
    vinserti128          m1, m1, [tmpq+tmpsq*1], 1
    psubw                m1, m0, m1
    lea                tmpq, [tmpq+tmpsq*2]
    pmulhrsw             m1, m2
    paddw                m0, m1
    movu         [dstq+dsq*0], xm0
    vextracti128 [dstq+dsq*1], m0, 1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w8_loop
    RET
.w16:
    mova                 m4, [base+obmc_masks_avx2+16*2]
.w16_loop:
    movu                 m0,     [dstq+dsq*0]
    psubw                m2, m0, [tmpq+ tmpsq*0]
    movu                 m1,     [dstq+dsq*1]
    psubw                m3, m1, [tmpq+ tmpsq*1]
    lea                tmpq, [tmpq+tmpsq*2]
    pmulhrsw             m2, m4
    pmulhrsw             m3, m4
    paddw                m0, m2
    paddw                m1, m3
    movu       [dstq+dsq*0], m0
    movu       [dstq+dsq*1], m1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w16_loop
    RET
.w32:
%if WIN64
    movaps         [rsp+ 8], xmm6
    movaps         [rsp+24], xmm7
%endif
    mova                 m6, [base+obmc_masks_avx2+32*2]
    vbroadcasti128       m7, [base+obmc_masks_avx2+32*3]
.w32_loop:
    movu                 m0,     [dstq+dsq*0  +32*0]
    psubw                m3, m0, [tmpq+tmpsq*0]
    movu                xm2,     [dstq+dsq*0+  32*1]
    movu                xm5,     [tmpq+tmpsq*0+32*1]
    movu                 m1,     [dstq+dsq*1  +32*0]
    psubw                m4, m1, [tmpq+tmpsq*1]
    vinserti128          m2, m2,     [dstq+dsq*1+  32*1], 1
    vinserti128          m5, m5,     [tmpq+tmpsq*1+32*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    psubw                m5, m2, m5
    pmulhrsw             m3, m6
    pmulhrsw             m4, m6
    pmulhrsw             m5, m7
    paddw                m0, m3
    paddw                m1, m4
    paddw                m2, m5
    movu         [dstq+dsq*0+32*0], m0
    movu         [dstq+dsq*1+32*0], m1
    movu         [dstq+dsq*0+32*1], xm2
    vextracti128 [dstq+dsq*1+32*1], m2, 1
    lea                dstq, [dstq+dsq*2]
    sub                  hd, 2
    jg .w32_loop
%if WIN64
    movaps             xmm6, [rsp+ 8]
    movaps             xmm7, [rsp+24]
%endif
    RET

%macro BLEND_H_ROW 2
    movu                 m0,     [dstq+32*%2]
    psubw                m2, m0, [tmpq+32*%1]
    movu                 m1,     [dstq+32*(%2+1)]
    psubw                m3, m1, [tmpq+32*(%1+1)]
    pmulhrsw             m2, m4
    pmulhrsw             m3, m4
    paddw                m0, m2
    paddw                m1, m3
    movu   [dstq+32*%2], m0
    movu   [dstq+32*(%2+1)], m1
%endmacro

INIT_XMM avx2
cglobal blend_h_16bpc, 4, 7, 6, dst, ds, tmp, tmps, w, h, mask
%define base r6-blend_h_avx2_table
    lea                  r6, [blend_h_avx2_table]
    tzcnt                wd, wm
    mov                  hd, hm
    movsxd               wq, dword[r6+wq*4]
    add                  wq, r6
    lea               maskq, [base+obmc_masks_avx2+hq*2]
    lea                  hd, [hq*3]
    shr                  hd, 2 ; h * 3/4
    lea               maskq, [maskq+hq*2]
    neg                  hq
    jmp                  wq
.w2:
    movd                 m0, [dstq+dsq*0]
    pinsrd               m0, [dstq+dsq*1], 1
    movd                 m2, [maskq+hq*2]
    movd                 m0, [tmpq+tmpsq*0]
    pinsrd               m0, [tmpq+tmpsq*1], 1
    lea                tmpq, [tmpq+tmpsq*2]
    punpcklwd            m2, m2
    psubw                m1, m0, m1
    pmulhrsw             m1, m2
    paddw                m0, m1
    movd       [dstq+dsq*0], m0
    pextrd     [dstq+dsq*1], m0, 1
    lea                dstq, [dstq+dsq*2]
    add                  hq, 2
    jl .w2
    RET
.w4:
    mova                 m3, [blend_shuf]
.w4_loop:
    movq                 m0, [dstq+dsq*0]
    movhps               m0, [dstq+dsq*1]
    movd                 m2, [maskq+hq*2]
    movq                 m1, [tmpq+tmpsq*0]
    movhps               m1, [tmpq+tmpsq*1]
    psubw                m1, m0, m1
    lea                tmpq, [tmpq+tmpsq*2]
    pshufb               m2, m3
    pmulhrsw             m1, m2
    paddw                m0, m1
    movq       [dstq+dsq*0], m0
    movhps     [dstq+dsq*1], m0
    lea                dstq, [dstq+dsq*2]
    add                  hq, 2
    jl .w4_loop
    RET
INIT_YMM avx2
.w8:
    vbroadcasti128       m3, [blend_shuf]
    shufpd               m3, m3, m3, 0x0c
.w8_loop:
    movu                xm0, [dstq+dsq*0]
    vinserti128          m0, m0, [dstq+dsq*1], 1
    vpbroadcastd         m2, [maskq+hq*2]
    movu                xm1, [tmpq+tmpsq*0]
    vinserti128          m1, m1, [tmpq+tmpsq*1], 1
    psubw                m1, m0, m1
    lea                tmpq, [tmpq+tmpsq*2]
    pshufb               m2, m3
    pmulhrsw             m1, m2
    paddw                m0, m1
    movu         [dstq+dsq*0], xm0
    vextracti128 [dstq+dsq*1], m0, 1
    lea                dstq, [dstq+dsq*2]
    add                  hq, 2
    jl .w8_loop
    RET
.w16:
    vpbroadcastw         m4, [maskq+hq*2]
    vpbroadcastw         m5, [maskq+hq*2+2]
    movu                 m0,     [dstq+dsq*0]
    psubw                m2, m0, [tmpq+ tmpsq*0]
    movu                 m1,     [dstq+dsq*1]
    psubw                m3, m1, [tmpq+ tmpsq*1]
    lea                tmpq, [tmpq+tmpsq*2]
    pmulhrsw             m2, m4
    pmulhrsw             m3, m5
    paddw                m0, m2
    paddw                m1, m3
    movu       [dstq+dsq*0], m0
    movu       [dstq+dsq*1], m1
    lea                dstq, [dstq+dsq*2]
    add                  hq, 2
    jl .w16
    RET
.w32:
    vpbroadcastw         m4, [maskq+hq*2]
    BLEND_H_ROW           0, 0
    add                dstq, dsq
    add                tmpq, tmpsq
    inc                  hq
    jl .w32
    RET
.w64:
    vpbroadcastw         m4, [maskq+hq*2]
    BLEND_H_ROW           0, 0
    BLEND_H_ROW           2, 2
    add                dstq, dsq
    add                tmpq, tmpsq
    inc                  hq
    jl .w64
    RET
.w128:
    vpbroadcastw         m4, [maskq+hq*2]
    BLEND_H_ROW           0,  0
    BLEND_H_ROW           2,  2
    BLEND_H_ROW           4,  4
    BLEND_H_ROW           6,  6
    add                dstq, dsq
    add                tmpq, tmpsq
    inc                  hq
    jl .w128
    RET

%endif ; ARCH_X86_64
