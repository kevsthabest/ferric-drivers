; =============================================================================
; Ferric DOS Stage G - Standalone VBE + 2D Acceleration JLM Stub (VBE.JLM)
; =============================================================================
; DDB device ID: 0xFD04 ("VBE     ")
; Strictly 386 baseline (`cpu 386`), ZERO storage writes, ZERO DOS INT 21h.
; Dedicated 8 KiB Ring-0 stack, VMM_SIMULATE_FAR_RET on both V86 and PM32 APIs.
; =============================================================================

cpu 386
bits 32

VBE_DEVICE_ID           equ 0xFD04
DLL_PROCESS_ATTACH      equ 1
DLL_PROCESS_DETACH      equ 0
VMM_SIMULATE_FAR_RET    equ 13          ; VMM_Service Simulate_Far_Ret
VMM_PAGE_RESERVE        equ 0x27        ; VMM_Service _PageReserve
VMM_PAGE_COMMIT_PHYS    equ 0x2A        ; VMM_Service _PageCommitPhys
PR_SYSTEM               equ 0x80080000  ; _PageReserve system linear arena
PC_MMIO_FLAGS           equ 0x40120000  ; PC_WRITEABLE | PC_CACHEDIS | PC_INCR
DDB_STRUCT_SIZE         equ 80

; Client_Reg_Struc offsets (vendor/ref/jemm/Include/JLM.INC)
CLIENT_EDI              equ 0
CLIENT_EBX              equ 16
CLIENT_EAX              equ 28
CLIENT_EFLAGS           equ 48
CLIENT_ES               equ 60

%macro VMMCall 1
    int 0x20
    dw %1
    dw 1
%endmacro

extern vbe_jlm_init
extern vbe_jlm_get_caps
extern vbe_jlm_set_lfb
extern vbe_jlm_fill_rect
extern vbe_jlm_bit_blt
extern vbe_jlm_sync
extern vbe_jlm_release_hw

section .data align=4

    align 16
vbe_r0_stack:
    times 8192 db 0
vbe_r0_stack_top:

; Exported VxD_Desc_Block (DDB) at ordinal 1 for JLOAD registration.
global _ddb
_ddb:
    dd 0                    ; +00 DDB_Next
    dw 0x0100               ; +04 DDB_Version (1.0)
    dw VBE_DEVICE_ID        ; +06 DDB_Req_Device_Number (0xFD04)
    db 0                    ; +08 DDB_Dev_Major_Version
    db 1                    ; +09 DDB_Dev_Minor_Version
    dw 0                    ; +10 DDB_Flags
    db "VBE     "           ; +12 DDB_Name (8 bytes)
    dd 0x80000000           ; +20 DDB_Init_Order
    dd _jlm_control_proc    ; +24 DDB_Control_Proc
    dd _v86_dispatch        ; +28 DDB_V86_API_Proc
    dd _pm_dispatch         ; +32 DDB_PM_API_Proc (safe flat linear EDI)
    dd 0                    ; +36 DDB_V86_API_CSIP
    dd 0                    ; +40 DDB_PM_API_CSIP
    dd 0                    ; +44 DDB_Reference_Data
    dd 0                    ; +48 DDB_Service_Table_Ptr
    dd 0                    ; +52 DDB_Service_Table_Size
    dd 0                    ; +56 DDB_Win32_Service_Table
    dd 0                    ; +60 DDB_Prev
    dd DDB_STRUCT_SIZE      ; +64 DDB_Size (80)
    dd 0                    ; +68 DDB_Reserved1
    dd 0                    ; +72 DDB_Reserved2
    dd 0                    ; +76 DDB_Reserved3

section .text align=4

; C-ABI helper called from Rust:
;   u32 vbe_map_mmio(u32 phys, u32 npages)
; Reserves `npages` in PR_SYSTEM and commits them uncached to `phys`.
; Returns mapped flat linear address (4 KiB-aligned) or 0 on failure.
global vbe_map_mmio
vbe_map_mmio:
    push ebx
    push esi
    mov ebx, [esp + 12]                 ; phys
    mov esi, [esp + 16]                 ; npages
    or esi, esi
    jz .map_fail
    push dword 0                        ; flags = 0
    push esi                            ; npages
    push dword PR_SYSTEM
    VMMCall VMM_PAGE_RESERVE
    add esp, 12
    cmp eax, 0xFFFFFFFF
    je .map_fail
    or eax, eax
    jz .map_fail
    push eax                            ; save linear base
    mov ecx, eax
    shr ecx, 12                         ; lin_page
    mov edx, ebx
    shr edx, 12                         ; phys_page
    push dword PC_MMIO_FLAGS
    push edx                            ; phys_page
    push esi                            ; npages
    push ecx                            ; lin_page
    VMMCall VMM_PAGE_COMMIT_PHYS
    add esp, 16
    or eax, eax
    pop eax                             ; restore linear base (flags preserved)
    jz .map_fail
    pop esi
    pop ebx
    ret
.map_fail:
    xor eax, eax
    pop esi
    pop ebx
    ret

_jlm_control_proc:
    clc
    ret

; V86-mode API dispatcher (INT 2Fh AX=1684h BX=0xFD04 far call).
; Buffer pointer is ES:DI (conventional memory, validated < 0xA0000 in Rust).
_v86_dispatch:
    VMMCall VMM_SIMULATE_FAR_RET
    and dword [ebp + CLIENT_EFLAGS], ~1     ; clear CF
    movzx eax, word [ebp + CLIENT_EAX]
    test eax, eax
    jz .ping
    cmp eax, 1
    je .get_caps
    cmp eax, 2
    je .set_lfb
    cmp eax, 3
    je .fill_rect
    cmp eax, 4
    je .bit_blt
    cmp eax, 5
    je .sync
    cmp eax, 6
    je .release_hw
    mov word [ebp + CLIENT_EAX], 0xFFFF
    ret

.ping:
    mov word [ebp + CLIENT_EAX], 0x0001
    mov word [ebp + CLIENT_EBX], VBE_DEVICE_ID
    ret

.get_caps:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    push dword 0                        ; is_pm32 = 0
    push eax                            ; out_lin
    call vbe_jlm_get_caps
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.set_lfb:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    push dword 0                        ; is_pm32 = 0
    push eax                            ; desc_lin
    call vbe_jlm_set_lfb
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.fill_rect:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    push dword 0                        ; is_pm32 = 0
    push eax                            ; req_lin
    call vbe_jlm_fill_rect
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.bit_blt:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    push dword 0                        ; is_pm32 = 0
    push eax                            ; req_lin
    call vbe_jlm_bit_blt
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.sync:
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    call vbe_jlm_sync
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.release_hw:
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    call vbe_jlm_release_hw
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; Protected-mode (DPMI / HDPMI32i / DJGPP) API dispatcher.
; Buffer pointer is flat 32-bit linear EDI (never aliases PM selectors as V86 segs).
_pm_dispatch:
    VMMCall VMM_SIMULATE_FAR_RET
    and dword [ebp + CLIENT_EFLAGS], ~1     ; clear CF
    movzx eax, word [ebp + CLIENT_EAX]
    test eax, eax
    jz .pm_ping
    cmp eax, 1
    je .pm_get_caps
    cmp eax, 2
    je .pm_set_lfb
    cmp eax, 3
    je .pm_fill_rect
    cmp eax, 4
    je .pm_bit_blt
    cmp eax, 5
    je .pm_sync
    cmp eax, 6
    je .pm_release_hw
    mov dword [ebp + CLIENT_EAX], 0xFFFF
    ret

.pm_ping:
    mov word [ebp + CLIENT_EAX], 0x0001
    mov word [ebp + CLIENT_EBX], VBE_DEVICE_ID
    ret

.pm_get_caps:
    mov eax, [ebp + CLIENT_EDI]
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    push dword 1                        ; is_pm32 = 1
    push eax
    call vbe_jlm_get_caps
    add esp, 8
    pop esp
    mov dword [ebp + CLIENT_EAX], eax
    ret

.pm_set_lfb:
    mov eax, [ebp + CLIENT_EDI]
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    push dword 1                        ; is_pm32 = 1
    push eax
    call vbe_jlm_set_lfb
    add esp, 8
    pop esp
    mov dword [ebp + CLIENT_EAX], eax
    ret

.pm_fill_rect:
    mov eax, [ebp + CLIENT_EDI]
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    push dword 1                        ; is_pm32 = 1
    push eax
    call vbe_jlm_fill_rect
    add esp, 8
    pop esp
    mov dword [ebp + CLIENT_EAX], eax
    ret

.pm_bit_blt:
    mov eax, [ebp + CLIENT_EDI]
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    push dword 1                        ; is_pm32 = 1
    push eax
    call vbe_jlm_bit_blt
    add esp, 8
    pop esp
    mov dword [ebp + CLIENT_EAX], eax
    ret

.pm_sync:
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    call vbe_jlm_sync
    pop esp
    mov dword [ebp + CLIENT_EAX], eax
    ret

.pm_release_hw:
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    call vbe_jlm_release_hw
    pop esp
    mov dword [ebp + CLIENT_EAX], eax
    ret

; PE32 DLL entry point (stdcall: hModule, ulReason, pvReserved).
global DllMain
DllMain:
    mov eax, [esp + 8]
    cmp eax, DLL_PROCESS_ATTACH
    je .attach
    cmp eax, DLL_PROCESS_DETACH
    je .detach
    mov eax, 1
    ret 12

.attach:
    mov edx, esp
    mov esp, vbe_r0_stack_top
    push edx
    call vbe_jlm_init
    pop esp
    ret 12

.detach:
    mov eax, 1
    ret 12
