; CARDBUS.JLM experimental discovery provider (FD09).
cpu 386
bits 32
extern cb_version, cb_probe_count, cb_query, cb_acquire, cb_release, cb_shutdown, cb_audit
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
section .data align=4
services: dd cb_version,query,acquire,release
global _ddb
_ddb:
 dd 0
 dw 0x0100,0xFD09
 db 1,0
 dw 0
 db "CARDBUS "
 dd 0x80000000,control,dispatch
 dd 0,0,0,0
 dd services,4
 dd 0,0,80,0,0,0
section .text align=4
global cb_map
cb_map:
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

global cb_unmap
cb_unmap:
    ; cdecl: PageFree takes the original linear base, flags=0.
    push dword 0
    push dword [esp + 8]
    VMMCall VMM_PAGE_FREE
    add esp, 8
    ret


global cb_claim, cb_unclaim
cb_claim:
 push dword [esp+12]
 push dword [esp+12]
 push dword [esp+12]
 push dword 0xFD09
 FRESCall 3
 add esp,16
 ret
cb_unclaim:
 push dword [esp+12]
 push dword [esp+12]
 push dword [esp+12]
 push dword 0xFD09
 FRESCall 4
 add esp,16
 ret
control:
 clc
 ret
%macro WRAP2 2
%1:
 pushfd
 cli
 cld
 push dword [esp+12]
 push dword [esp+12]
 call %2
 add esp,8
 popfd
 ret
%endmacro
WRAP2 query,cb_query
WRAP2 acquire,cb_acquire
WRAP2 release,cb_release
dispatch:
 pushfd
 cli
 cld
 VMMCall 13
 and dword [ebp+48],~1
 movzx eax,word [ebp+28]
 cmp eax,0
 je .ping
 cmp eax,8
 je .probe
 cmp eax,9
 je .query
 cmp eax,0x0a
 je .audit
 jmp .bad
.ping:
 mov word [ebp+28],1
 mov word [ebp+16],0xFD09
 jmp .done
.probe:
 call cb_probe_count
 mov [ebp+28],eax
 jmp .done
.audit:
 movzx eax,word [ebp+16]
 push eax
 call cb_audit
 add esp,4
 mov [ebp+28],eax
 jmp .done
.query:
 movzx eax,word [ebp+60]
 shl eax,4
 movzx edx,word [ebp]
 cmp edx,0xFFD8
 ja .bad
 add eax,edx
 test eax,eax
 jz .bad
 cmp eax,0xFFFD8
 ja .bad
 push eax
 movzx eax,word [ebp+16]
 push eax
 call cb_query
 add esp,8
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
 push dword 0xFD09
 FRESCall 1
 add esp,4
 test eax,eax
 jnz .fail
 jmp .ok
.detach:
 cmp eax,0
 jne .ok
 call cb_shutdown
 test eax,eax
 jnz .fail
 push dword 0xFD09
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
