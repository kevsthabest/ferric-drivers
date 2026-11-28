; Experimental standalone AR2413 consumer FD0A. No activation on load.
cpu 386
bits 32
extern ath_version,ath_probe,ath_shutdown
%define VMM_PAGE_FREE 0x12
%define VMM_PAGE_RESERVE 0x27
%define VMM_PAGE_COMMIT_PHYS 0x2A
%define PR_SYSTEM 0x80080000
%define PC_MMIO_FLAGS 0x40120000
%macro VMMCall 1
 int 0x20
 dw %1,1
%endmacro
%macro FRESCall 1
 int 0x20
 dw %1,0xFD07
%endmacro
%macro CBCall 1
 int 0x20
 dw %1,0xFD09
%endmacro
section .data align=4
services: dd ath_version
global _ddb
_ddb:
 dd 0
 dw 0x0100,0xFD0A
 db 1,0
 dw 0
 db "ATH5K   "
 dd 0x80000000,control,dispatch
 dd 0,0,0,0
 dd services,1
 dd 0,0,80,0,0,0
section .text align=4
global ath_map
ath_map:
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

global ath_unmap
ath_unmap:
    ; cdecl: PageFree takes the original linear base, flags=0.
    push dword 0
    push dword [esp + 8]
    VMMCall VMM_PAGE_FREE
    add esp, 8
    ret



global ath_cb_query,ath_cb_acquire,ath_cb_release,ath_cb_master
%macro CHECK_CB 0
 push edi
 xor edi,edi
 mov eax,0xFD09
 VMMCall 48
 pop edi
 test ecx,ecx
 jz cb_missing
 cmp dword [ecx+48],0
 je cb_missing
 cmp dword [ecx+52],5
 jb cb_missing
%endmacro
cb_missing:
 mov eax,0xffffffff
 ret
ath_cb_query:
 CHECK_CB
 times 2 push dword [esp+8]
 CBCall 1
 add esp,8
 ret
ath_cb_acquire:
 CHECK_CB
 push dword 0xFD0A
 push dword [esp+8]
 CBCall 2
 add esp,8
 ret
ath_cb_release:
 CHECK_CB
 push dword 0xFD0A
 push dword [esp+8]
 CBCall 3
 add esp,8
 ret
ath_cb_master:
 CHECK_CB
 push dword [esp+8]
 push dword 0xFD0A
 push dword [esp+12]
 CBCall 4
 add esp,12
 ret
control:
 clc
 ret
dispatch:
 pushfd
 cli
 cld
 VMMCall 13
 and dword [ebp+48],~1
 movzx eax,word [ebp+28]
 test eax,eax
 jz .ping
 cmp eax,0x0a
 je .probe
 cmp eax,0x0d
 je .stop
 jmp .bad
.ping:
 mov word [ebp+28],1
 mov word [ebp+16],0xFD0A
 jmp .done
.probe:
 ; AX=0A, BX=slot. No external pointer.
 movzx eax,word [ebp+16]
 push eax
 call ath_probe
 add esp,4
 mov [ebp+28],eax
 jmp .done
.stop:
 call ath_shutdown
 mov [ebp+28],eax
 jmp .done
.bad:
 or dword [ebp+48],1
 mov dword [ebp+28],0
.done:
 popfd
 ret
global DllMain
DllMain:
 mov eax,[esp+8]
 pushfd
 cli
 push edi
 cld
 cmp eax,1
 jne .detach
 xor edi,edi
 mov eax,0xFD01
 VMMCall 48
 test ecx,ecx
 jnz .fail
 xor edi,edi
 mov eax,0xFD09
 VMMCall 48
 test ecx,ecx
 jz .fail
 cmp dword [ecx+48],0
 je .fail
 cmp dword [ecx+52],5
 jb .fail
 CBCall 0
 cmp eax,0x10000
 jne .fail
 xor edi,edi
 mov eax,0xFD07
 VMMCall 48
 test ecx,ecx
 jz .fail
 cmp dword [ecx+48],0
 je .fail
 cmp dword [ecx+52],5
 jb .fail
 FRESCall 0
 cmp eax,0x10000
 jne .fail
 push dword 0xFD0A
 FRESCall 1
 add esp,4
 test eax,eax
 jnz .fail
 jmp .ok
.detach:
 cmp eax,0
 jne .ok
 call ath_shutdown
 test eax,eax
 jnz .fail
 push dword 0xFD0A
 FRESCall 2
 add esp,4
 test eax,eax
 jnz .fail
.ok:
 mov eax,1
 jmp .return
.fail:
 xor eax,eax
.return:
 pop edi
 popfd
 ret 12
