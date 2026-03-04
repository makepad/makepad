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

%undef WIN64
%undef UNIX64
%ifidn __OUTPUT_FORMAT__,win32
    %define WIN64  1
%elifidn __OUTPUT_FORMAT__,win64
    %define WIN64  1
%elifidn __OUTPUT_FORMAT__,x64
    %define WIN64  1
%else
    %define UNIX64 1
%endif

%undef FORMAT_ELF
%ifidn __OUTPUT_FORMAT__,elf
    %define FORMAT_ELF 1
%elifidn __OUTPUT_FORMAT__,elf32
    %define FORMAT_ELF 1
%elifidn __OUTPUT_FORMAT__,elf64
    %define FORMAT_ELF 1
%endif

%undef FORMAT_MACHO
%ifidn __OUTPUT_FORMAT__,macho32
     %define FORMAT_MACHO 1
%elifidn __OUTPUT_FORMAT__,macho64
     %define FORMAT_MACHO 1
%endif

%ifdef FORMAT_ELF
    %undef PREFIX
%elif WIN64
    %undef PREFIX
%else
    %define PREFIX
%endif

%ifdef PREFIX
    %define mangle(x) _ %+ x
%else
    %define mangle(x) x
%endif

%macro cglobal 1
    %assign stack_offset 0
    %xdefine %1 mangle(%1)
    %ifidn   __OUTPUT_FORMAT__,macho64
    %define %1 _ %+ %1
    %endif
    global %1
    %1:
%endmacro

%macro cextern 1
    %xdefine %1 mangle(%1)
    extern %1
%endmacro

%macro DECLARE_REG 7
    %define r%1d %4
    %define r%1w %5
    %define r%1b %6
    %define r%1  %3
    %define arg%2 %7
%endmacro

%macro DECLARE_PARAM 3
    %define GET_PARAM_%1Q        mov             r%2,                   %3
    %define GET_PARAM_%1D        mov             r%2d,            dword %3
    %define GET_PARAM_%1UXD      mov             r%2d,            dword %3;GET_PARAM_%1D
    %define GET_PARAM_%1SXD      movsxd          r%2,             dword %3
    %define GET_PARAM_%1W        mov             r%2w,             word %3
    %define GET_PARAM_%1UXW      movzx           r%2,              word %3
    %define GET_PARAM_%1SXW      movsx           r%2,              word %3
    %define GET_PARAM_%1B        mov             r%2b,             byte %3
    %define GET_PARAM_%1UXB      movzx           r%2,              byte %3
    %define GET_PARAM_%1SXB      movsx           r%2,              byte %3
%endmacro

bits 64

%ifdef WIN64

    %define num_arg_in_reg 4
    %define num_volatile_reg 7

    DECLARE_REG  0,  1, RCX, ECX,  CX,   CL,   RCX
    DECLARE_REG  1,  2, RDX, EDX,  DX,   DL,   RDX
    DECLARE_REG  2,  3, R8,  R8D,  R8W,  R8B,  R8
    DECLARE_REG  3,  4, R9,  R9D,  R9W,  R9B,  R9
    DECLARE_REG  4,  5, R10, R10D, R10W, R10B, [RSP+stack_offset+28h]
    DECLARE_REG  5,  6, R11, R11D, R11W, R11B, [RSP+stack_offset+30h]
    DECLARE_REG  6,  7, RAX, EAX,  AX,   AL,   [RSP+stack_offset+38h]
    DECLARE_REG  7,  8, RSI, ESI,  SI,   NONE, [RSP+stack_offset+40h]
    DECLARE_REG  8,  9, RDI, EDI,  DI,   NONE, [RSP+stack_offset+48h]
    DECLARE_REG  9, 10, R12, R12D, R12W, R12B, [RSP+stack_offset+50h]
    DECLARE_REG 10, 11, R13, R13D, R13W, R13B, [RSP+stack_offset+58h]
    DECLARE_REG 11, 12, R14, R14D, R14W, R14B, [RSP+stack_offset+60h]
    DECLARE_REG 12, 13, R15, R15D, R15W, R15B, [RSP+stack_offset+68h]
    DECLARE_REG 13, 14, RBX, EBX,  BX,   BL,   [RSP+stack_offset+70h]
    DECLARE_REG 14, 15, RBP, EBP,  BP,   NONE, [RSP+stack_offset+78h]
    DECLARE_REG 15, 16, RSP, ESP,  SP,   NONE, [RSP+stack_offset+80h]

    DECLARE_PARAM 5, 4, arg5
    DECLARE_PARAM 6, 5, arg6

%elifdef UNIX64

    %define num_arg_in_reg   6
    %define num_volatile_reg 7

    DECLARE_REG  0,  1, RDI, EDI,  DI,   DIL,  RDI
    DECLARE_REG  1,  2, RSI, ESI,  SI,   SIL,  RSI
    DECLARE_REG  2,  3, RDX, EDX,  DX,   DL,   RDX
    DECLARE_REG  3,  4, RCX, ECX,  CX,   CL,   RCX
    DECLARE_REG  4,  5, R8,  R8D,  R8W,  R8B,  R8
    DECLARE_REG  5,  6, R9,  R9D,  R9W,  R9B,  R9
    DECLARE_REG  6,  7, RAX, EAX,  AX,   AL,   [RSP+stack_offset+ 8h]
    DECLARE_REG  7,  8, R10, R10D, R10W, R10B, [RSP+stack_offset+10h]
    DECLARE_REG  8,  9, R11, R11D, R11W, R11B, [RSP+stack_offset+18h]
    DECLARE_REG  9, 10, R12, R12D, R12W, R12B, [RSP+stack_offset+20h]
    DECLARE_REG 10, 11, R13, R13D, R13W, R13B, [RSP+stack_offset+28h]
    DECLARE_REG 11, 12, R14, R14D, R14W, R14B, [RSP+stack_offset+30h]
    DECLARE_REG 12, 13, R15, R15D, R15W, R15B, [RSP+stack_offset+38h]
    DECLARE_REG 13, 14, RBX, EBX,  BX,   BL,   [RSP+stack_offset+40h]
    DECLARE_REG 14, 15, RBP, EBP,  BP,   NONE, [RSP+stack_offset+48h]
    DECLARE_REG 15, 16, RSP, ESP,  SP,   NONE, [RSP+stack_offset+50h]

    %define GET_PARAM_5Q
    %define GET_PARAM_5D
    %define GET_PARAM_5UXD
    %define GET_PARAM_5SXD      movsxd          r4,             r4d
    %define GET_PARAM_5W
    %define GET_PARAM_5UXW      movzx           r4,             r4w
    %define GET_PARAM_5SXW      movsx           r4,             r4w
    %define GET_PARAM_5B
    %define GET_PARAM_5UXB      movzx           r4,             r4b
    %define GET_PARAM_5SXB      movsx           r4,             r4b

    %define GET_PARAM_6Q
    %define GET_PARAM_6D
    %define GET_PARAM_6UXD
    %define GET_PARAM_6SXD      movsxd          r5,             r5d
    %define GET_PARAM_6W
    %define GET_PARAM_6UXW      movzx           r5,             r5w
    %define GET_PARAM_6SXW      movsx           r5,             r5w
    %define GET_PARAM_6B
    %define GET_PARAM_6UXB      movzx           r5,             r5b
    %define GET_PARAM_6SXB      movsx           r5,             r5b

;    %define GET_PARAM_7Q        mov             r6,                   [rsp+8h]
;    %define GET_PARAM_7D        mov             r6d,            dword [rsp+8h]
;    %define GET_PARAM_7UXD      GET_PARAM_7D
;    %define GET_PARAM_7SXD      movsxd          r6,             dword [rsp+8h]
;    %define GET_PARAM_7W        mov             r6w,             word [rsp+8h]
;    %define GET_PARAM_7UXW      movzx           r6,              word [rsp+8h]
;    %define GET_PARAM_7SXW      movsx           r6,              word [rsp+8h]
;    %define GET_PARAM_7B        mov             r6b,             byte [rsp+8h]
;    %define GET_PARAM_7UXB      movzx           r6,              byte [rsp+8h]
;    %define GET_PARAM_7SXB      movsx           r6,              byte [rsp+8h]

%endif

%define GET_PARAM_7Q        mov             r6,                   arg7
%define GET_PARAM_7D        mov             r6d,            dword arg7
%define GET_PARAM_7UXD      GET_PARAM_7D
%define GET_PARAM_7SXD      movsxd          r6,             dword arg7
%define GET_PARAM_7W        mov             r6w,             word arg7
%define GET_PARAM_7UXW      movzx           r6,              word arg7
%define GET_PARAM_7SXW      movsx           r6,              word arg7
%define GET_PARAM_7B        mov             r6b,             byte arg7
%define GET_PARAM_7UXB      movzx           r6,              byte arg7
%define GET_PARAM_7SXB      movsx           r6,              byte arg7

%macro GET_PARAM_8Q 1
    mov             %1,            arg8
%endmacro

%macro GET_PARAM_8UXD 1
    mov             %1,            dword arg8
%endmacro

%macro GET_PARAM_8UXW 1
    movzx           %1,            word arg8
%endmacro

%macro GET_PARAM_8SXW 1
    movsx           %1,            word arg8
%endmacro

%macro GET_PARAM_8UXB 1
    movzx           %1,            byte arg8
%endmacro

%macro GET_PARAM_9Q 1
    mov             %1,            arg9
%endmacro

%macro GET_PARAM_9UXD 1
    mov             %1,            dword arg9
%endmacro

%macro GET_PARAM_9UXW 1
    movzx           %1,            word arg9
%endmacro

%macro GET_PARAM_9SXW 1
    movsx           %1,            word arg9
%endmacro

%macro GET_PARAM_9UXD_TO_MM 1
    movd            %1,            arg9
%endmacro

%macro GET_PARAM_9UXB 1
    movzx           %1,            byte arg9
%endmacro

%macro GET_PARAM_10Q 1
    mov             %1,            arg10
%endmacro

%macro GET_PARAM_10UXD 1
    mov             %1,            dword arg10
%endmacro

%macro GET_PARAM_10UXW 1
    movzx           %1,            word arg10
%endmacro

%macro GET_PARAM_10SXW 1
    movsx           %1,            word arg10
%endmacro

%macro GET_PARAM_10UXB 1
    movzx           %1,            byte arg10
%endmacro

%macro GET_PARAM_11SXW 1
    movsx           %1,            word arg11
%endmacro

%macro GET_PARAM_12Q 1
    mov             %1,            arg12
%endmacro

%macro GET_PARAM_12UXD 1
    mov             %1,            dword arg12
%endmacro

%macro GET_PARAM_12UXB 1
    movzx           %1,            byte arg12
%endmacro

%macro SUB_RSP 1
    sub    rsp, %1
    %assign stack_offset stack_offset+%1
%endmacro

%macro ADD_RSP 1
    add    rsp, %1
    %assign stack_offset stack_offset-%1
%endmacro

%macro PUSH_REG 1
  %if %1 >= num_volatile_reg
    push    r%1
    %assign stack_offset stack_offset+8
  %endif
%endmacro

%macro POP_REG 1
  %if %1 >= num_volatile_reg
    pop     r%1
    %assign stack_offset stack_offset-8
  %endif
%endmacro

%macro PUSH_REG_FORCE 1
    push    r%1
    %assign stack_offset stack_offset+8
%endmacro

%macro POP_REG_FORCE 1
    pop     r%1
    %assign stack_offset stack_offset-8
%endmacro

%macro GET_PARAM 1
  %if %1 >= num_arg_in_reg
    mov     r%1, [arg%1]
  %endif
%endmacro

%macro GET_PARAM_UXD 1
  %if %1 >= num_arg_in_reg
    ; if 64-bit, higher half will be zero
    mov     r %+ %1d, dword [arg%1]
  %endif
%endmacro

%macro GET_PARAM_SXD 1
  %if %1 >= num_arg_in_reg
  %ifdef OS_64BIT
    movsxd  r%1, dword [arg%1]
  %else
    mov     r%1, [arg%1]
  %endif
  %endif
%endmacro

%macro XMM_SAVE 0
  %ifdef WIN64
    SUB_RSP 16
    movdqu          [rsp],      xmm6
    SUB_RSP 16
    movdqu          [rsp],      xmm7
    SUB_RSP 16
    movdqu          [rsp],      xmm8
    SUB_RSP 16
    movdqu          [rsp],      xmm9
    SUB_RSP 16
    movdqu          [rsp],      xmm10
    SUB_RSP 16
    movdqu          [rsp],      xmm11
    SUB_RSP 16
    movdqu          [rsp],      xmm12
    SUB_RSP 16
    movdqu          [rsp],      xmm13
    SUB_RSP 16
    movdqu          [rsp],      xmm14
    SUB_RSP 16
    movdqu          [rsp],      xmm15
  %endif
%endmacro
%macro XMM_RESTORE 0
  %ifdef WIN64
    movdqu          xmm15,      [rsp]
    ADD_RSP 16
    movdqu          xmm14,      [rsp]
    ADD_RSP 16
    movdqu          xmm13,      [rsp]
    ADD_RSP 16
    movdqu          xmm12,      [rsp]
    ADD_RSP 16
    movdqu          xmm11,      [rsp]
    ADD_RSP 16
    movdqu          xmm10,      [rsp]
    ADD_RSP 16
    movdqu          xmm9,       [rsp]
    ADD_RSP 16
    movdqu          xmm8,       [rsp]
    ADD_RSP 16
    movdqu          xmm7,       [rsp]
    ADD_RSP 16
    movdqu          xmm6,       [rsp]
    ADD_RSP 16
  %endif
%endmacro
%define NEED_EMMS 1

; This is needed for ELF, otherwise the GNU linker assumes the stack is executable by default.
%ifdef FORMAT_ELF
    [SECTION .note.GNU-stack noalloc noexec nowrite progbits]
%endif
