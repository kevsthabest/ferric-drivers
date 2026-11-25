; =============================================================================
; Ferric DOS Stage F - Standalone Read-Only NTFS JLM Stub (NTFS.DLL -> NTFS.JLM)
; =============================================================================
; DDB device ID: 0xFD03 ("NTFS    ")
; Calls FERRIC.JLM (0xFD01) Block-Transport DDB service table via JEMM native
; Ring-0 `INT 20h` dynamic linking (zero OHCI dependency).
; Strictly 386 baseline (`cpu 386`), read-only, ZERO DOS INT 21h calls.
; =============================================================================

cpu 386
bits 32

NTFS_DEVICE_ID          equ 0xFD03
FERRIC_CORE_DEVICE_ID   equ 0xFD01
DLL_PROCESS_ATTACH      equ 1
DLL_PROCESS_DETACH      equ 0
VMM_SIMULATE_FAR_RET    equ 13
VMM_GET_DDB             equ 48
DDB_STRUCT_SIZE         equ 80

; Client_Reg_Struc offsets (vendor/ref/jemm/Include/JLM.INC)
CLIENT_EDI              equ 0
CLIENT_EBX              equ 16
CLIENT_ECX              equ 24
CLIENT_EAX              equ 28
CLIENT_EFLAGS           equ 48
CLIENT_ES               equ 60

; Core Block-Transport DDB service indices (crates/abi/src/fs_abi.rs)
BLK_SVC_ABI_VERSION     equ 0
BLK_SVC_DEV_INFO        equ 1
BLK_SVC_READ_SEC        equ 2

%macro VMMCall 1
    int 0x20
    dw %1
    dw 1
%endmacro

%macro CoreBlkCall 1
    int 0x20
    dw %1
    dw FERRIC_CORE_DEVICE_ID
%endmacro

extern ntfs_jlm_init
extern ntfs_jlm_dir
extern ntfs_jlm_open
extern ntfs_jlm_read_chunk

section .data align=4

    align 16
ntfs_r0_stack:
    times 8192 db 0
ntfs_r0_stack_top:

; Exported VxD_Desc_Block (DDB) at ordinal 1 for JLOAD registration.
global _ddb
_ddb:
    dd 0                    ; +00 DDB_Next
    dw 0x0100               ; +04 DDB_Version (1.0)
    dw NTFS_DEVICE_ID       ; +06 DDB_Req_Device_Number (0xFD03)
    db 0                    ; +08 DDB_Dev_Major_Version
    db 1                    ; +09 DDB_Dev_Minor_Version
    dw 0                    ; +10 DDB_Flags
    db "NTFS    "           ; +12 DDB_Name (8 bytes)
    dd 0x80000000           ; +20 DDB_Init_Order
    dd _jlm_control_proc    ; +24 DDB_Control_Proc
    dd _v86_dispatch        ; +28 DDB_V86_API_Proc
    dd 0                    ; +32 DDB_PM_API_Proc (V86-only; never alias PM selectors)
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

; -----------------------------------------------------------------------------
; Ring-0 cdecl shims into FERRIC.JLM (0xFD01) Block-Transport DDB Service Table
; JEMM patches `int 0x20; dw svc; dw 0xFD01` in-place to `call [table_entry]`
; on the first call, passing the cdecl stack arguments straight through.
; -----------------------------------------------------------------------------
global blk_abi_version
blk_abi_version:
    CoreBlkCall BLK_SVC_ABI_VERSION
    ret

global blk_dev_info
blk_dev_info:
    push dword [esp + 4]
    CoreBlkCall BLK_SVC_DEV_INFO
    add esp, 4
    ret

global blk_read_sec
blk_read_sec:
    push dword [esp + 8]                ; dst_lin
    push dword [esp + 8]                ; lba (shifted +4 by previous push)
    CoreBlkCall BLK_SVC_READ_SEC
    add esp, 8
    ret

_jlm_control_proc:
    clc
    ret

; V86-mode API dispatcher (INT 2Fh AX=1684h BX=0xFD03 far call).
_v86_dispatch:
    VMMCall VMM_SIMULATE_FAR_RET
    and dword [ebp + CLIENT_EFLAGS], ~1     ; clear CF
    movzx eax, word [ebp + CLIENT_EAX]
    test eax, eax
    jz .ping
    cmp eax, 0x32
    je .do_dir
    cmp eax, 0x33
    je .do_open
    cmp eax, 0x34
    je .do_read
    mov word [ebp + CLIENT_EAX], 0xFFFF
    ret

.ping:
    mov word [ebp + CLIENT_EAX], 0x0001
    mov word [ebp + CLIENT_EBX], NTFS_DEVICE_ID
    ret

.do_dir:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    movzx ecx, word [ebp + CLIENT_ECX]  ; cap
    mov edx, esp
    mov esp, ntfs_r0_stack_top
    push edx
    push ecx
    push eax
    call ntfs_jlm_dir
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.do_open:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; name_lin
    mov edx, esp
    mov esp, ntfs_r0_stack_top
    push edx
    push eax
    call ntfs_jlm_open
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

.do_read:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    mov edx, esp
    mov esp, ntfs_r0_stack_top
    push edx
    push eax
    call ntfs_jlm_read_chunk
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
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
    ; Verify FERRIC.JLM (0xFD01) is registered before issuing any CoreBlkCall
    ; (otherwise JEMM's Int20_Handler calls Crash_Cur_VM on missing device ID).
    push edi
    mov eax, FERRIC_CORE_DEVICE_ID
    xor edi, edi
    VMMCall VMM_GET_DDB
    pop edi
    test ecx, ecx
    jz .attach_fail
    mov edx, esp
    mov esp, ntfs_r0_stack_top
    push edx
    call ntfs_jlm_init
    pop esp
    ret 12

.attach_fail:
    xor eax, eax
    ret 12

.detach:
    mov eax, 1
    ret 12
