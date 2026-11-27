; =============================================================================
; Ferric Drivers - Standalone Toshiba SD Slot JLM Stub (TOSHSD.JLM, DDB 0xFD06)
; =============================================================================
; Verbs:
;   AX=0x0000  Ping / version (AX=0x0001, BX=0xFD06)
;   AX=0x003C  Initialize 1179:0805/0803 SD slot & identify card -> ES:DI
;   AX=0x003D  Read one 512-byte sector (EBX=LBA, ES:DI=buf) -> AX=0 ok
;   AX=0x003E  Power down SD slot cleanly
; Strictly 386 baseline (`cpu 386`), strictly read-only.
; =============================================================================

cpu 386
bits 32

SD_DEVICE_ID            equ 0xFD06
DLL_PROCESS_ATTACH      equ 1
DLL_PROCESS_DETACH      equ 0
VMM_SIMULATE_FAR_RET    equ 13
VMM_GET_DDB             equ 48
FERRIC_CORE_DEVICE_ID   equ 0xFD01
VMM_PAGE_FREE           equ 0x12
VMM_PAGE_RESERVE        equ 0x27
VMM_PAGE_COMMIT_PHYS    equ 0x2A
PR_SYSTEM               equ 0x80080000
PC_MMIO_FLAGS           equ 0x40120000
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

extern gate1_sd_init
extern gate1_sd_read_sec
extern gate1_sd_shutdown

section .data align=4

    align 16
sd_r0_stack:
    times 8192 db 0
sd_r0_stack_top:

global _ddb
_ddb:
    dd 0
    dw 0x0100
    dw SD_DEVICE_ID
    db 0
    db 1
    dw 0
    db "TOSHSD  "
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

global sd_map_mmio
sd_map_mmio:
    push ebx
    push esi
    mov ebx, [esp + 12]
    mov esi, [esp + 16]
    or esi, esi
    jz .map_fail
    push dword 0
    push esi
    push dword PR_SYSTEM
    VMMCall VMM_PAGE_RESERVE
    add esp, 12
    cmp eax, 0xFFFFFFFF
    je .map_fail
    or eax, eax
    jz .map_fail
    push eax
    mov ecx, eax
    shr ecx, 12
    mov edx, ebx
    shr edx, 12
    push dword PC_MMIO_FLAGS
    push edx
    push esi
    push ecx
    VMMCall VMM_PAGE_COMMIT_PHYS
    add esp, 16
    or eax, eax
    pop eax
    jz .commit_fail
    pop esi
    pop ebx
    ret
.commit_fail:
    ; Release the reserved linear pages when PageCommitPhys failed.
    push dword 0
    push eax
    VMMCall VMM_PAGE_FREE
    add esp, 8
.map_fail:
    xor eax, eax
    pop esi
    pop ebx
    ret

global sd_unmap_mmio
sd_unmap_mmio:
    ; cdecl: PageFree takes the original linear base, flags=0.
    push dword 0
    push dword [esp + 8]
    VMMCall VMM_PAGE_FREE
    add esp, 8
    ret

_jlm_control_proc:
    clc
    ret

_v86_dispatch:
    VMMCall VMM_SIMULATE_FAR_RET
    and dword [ebp + CLIENT_EFLAGS], ~1
    movzx eax, word [ebp + CLIENT_EAX]
    test eax, eax
    jz .ping
    cmp eax, 0x3C
    je .sd_init
    cmp eax, 0x3D
    je .sd_read_sec
    cmp eax, 0x3E
    je .sd_shutdown
    or dword [ebp + CLIENT_EFLAGS], 1
    ret

.ping:
    mov word [ebp + CLIENT_EAX], 0x0001
    mov word [ebp + CLIENT_EBX], SD_DEVICE_ID
    ret

.sd_init:
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
    mov edx, esp
    mov esp, sd_r0_stack_top
    push edx
    push ecx
    push eax
    call gate1_sd_init
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.sd_read_sec:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    test eax, eax
    jz .bad_buffer
    cmp edx, 0xFE00
    ja .bad_buffer
    cmp eax, 0xFFE00
    ja .bad_buffer
    mov ebx, [ebp + CLIENT_EBX]
    mov edx, esp
    mov esp, sd_r0_stack_top
    push edx
    push eax
    push ebx
    call gate1_sd_read_sec
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.sd_shutdown:
    mov edx, esp
    mov esp, sd_r0_stack_top
    push edx
    call gate1_sd_shutdown
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
    cmp eax, DLL_PROCESS_DETACH
    jne .ok
    call gate1_sd_shutdown
.ok:
    mov eax, 1
    ret 12
.attach_fail:
    xor eax, eax
    ret 12
