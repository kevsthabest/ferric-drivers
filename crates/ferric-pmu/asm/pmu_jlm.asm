; =============================================================================
; Ferric Drivers - Standalone ALi M7101 PMU JLM Stub (PMU.JLM, DDB 0xFD05)
; =============================================================================
; Verbs:
;   AX=0x0000  Ping / version (AX=0x0001, BX=0xFD05)
;   AX=0x0039  Query M7101 battery, throttle, and backlight -> ES:DI (CX cap)
;   AX=0x003A  Set CPU clock throttle (BX=0 off, 1..7 duty/8) -> ES:DI
;   AX=0x003B  Set LCD CCFT backlight (BX=0 off, 1..7 PWM, 8 max) -> ES:DI
; Strictly 386 baseline (`cpu 386`), read-only with respect to storage.
; =============================================================================

cpu 386
bits 32

PMU_DEVICE_ID           equ 0xFD05
DLL_PROCESS_ATTACH      equ 1
DLL_PROCESS_DETACH      equ 0
VMM_SIMULATE_FAR_RET    equ 13
VMM_GET_DDB             equ 48
FERRIC_CORE_DEVICE_ID   equ 0xFD01
DDB_STRUCT_SIZE         equ 80

CLIENT_EDI              equ 0
CLIENT_EBX              equ 16
CLIENT_ECX              equ 24
CLIENT_EAX              equ 28
CLIENT_EFLAGS           equ 48
CLIENT_ES               equ 60

%macro VMMCall 1
    int 0x20
    dw %1
    dw 1
%endmacro

extern gate1_m7101_info
extern gate1_m7101_set_throttle
extern gate1_m7101_set_backlight

section .data align=4

    align 16
pmu_r0_stack:
    times 8192 db 0
pmu_r0_stack_top:

global _ddb
_ddb:
    dd 0
    dw 0x0100
    dw PMU_DEVICE_ID
    db 0
    db 1
    dw 0
    db "PMU     "
    dd 0x80000000
    dd _jlm_control_proc
    dd _v86_dispatch
    dd 0
    dd 0
    dd 0
    dd 0
    dd 0
    dd 0
    dd 0
    dd 0
    dd DDB_STRUCT_SIZE
    dd 0
    dd 0
    dd 0

section .text align=4

_jlm_control_proc:
    clc
    ret

_v86_dispatch:
    VMMCall VMM_SIMULATE_FAR_RET
    and dword [ebp + CLIENT_EFLAGS], ~1
    movzx eax, word [ebp + CLIENT_EAX]
    test eax, eax
    jz .ping
    cmp eax, 0x39
    je .m7101_info
    cmp eax, 0x3A
    je .m7101_throttle
    cmp eax, 0x3B
    je .m7101_backlight
    or dword [ebp + CLIENT_EFLAGS], 1
    ret

.ping:
    mov word [ebp + CLIENT_EAX], 0x0001
    mov word [ebp + CLIENT_EBX], PMU_DEVICE_ID
    ret

.m7101_info:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    test ecx, ecx
    jz .bad_buffer
    test eax, eax
    jz .bad_buffer
    add edx, ecx
    cmp edx, 0x10000
    ja .bad_buffer
    lea edx, [eax + ecx]
    cmp edx, 0x100000
    ja .bad_buffer
    test ecx, ecx
    jz .bad_buffer
    test eax, eax
    jz .bad_buffer
    add edx, ecx
    cmp edx, 0x10000
    ja .bad_buffer
    lea edx, [eax + ecx]
    cmp edx, 0x100000
    ja .bad_buffer
    test ecx, ecx
    jz .bad_buffer
    test eax, eax
    jz .bad_buffer
    add edx, ecx
    cmp edx, 0x10000
    ja .bad_buffer
    lea edx, [eax + ecx]
    cmp edx, 0x100000
    ja .bad_buffer
    mov edx, esp
    mov esp, pmu_r0_stack_top
    push edx
    push ecx
    push eax
    call gate1_m7101_info
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.m7101_throttle:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    movzx ebx, word [ebp + CLIENT_EBX]
    mov edx, esp
    mov esp, pmu_r0_stack_top
    push edx
    push ecx
    push eax
    push ebx
    call gate1_m7101_set_throttle
    add esp, 12
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.m7101_backlight:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    movzx ebx, word [ebp + CLIENT_EBX]
    mov edx, esp
    mov esp, pmu_r0_stack_top
    push edx
    push ecx
    push eax
    push ebx
    call gate1_m7101_set_backlight
    add esp, 12
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.bad_buffer:
    mov word [ebp + CLIENT_EAX], 0
    or dword [ebp + CLIENT_EFLAGS], 1
    ret

global DllMain
DllMain:
    mov eax, [esp + 8]
    cmp eax, DLL_PROCESS_ATTACH
    jne .check_detach
    ; Jemm v5.83+ Get_DDB: EAX=device ID, EDI=0, ECX=result.
    ; The legacy core includes both controllers; reject duplicate ownership
    ; before any client can initialize hardware.
    push edi
    mov eax, FERRIC_CORE_DEVICE_ID
    xor edi, edi
    VMMCall VMM_GET_DDB
    pop edi
    test ecx, ecx
    jnz .attach_fail
    jmp .ok
.check_detach:
.ok:
    mov eax, 1
    ret 12
.attach_fail:
    xor eax, eax
    ret 12
