; CARDBUS.JLM experimental discovery provider (FD09).
cpu 386
bits 32
extern cb_version, cb_probe_count, cb_query, cb_acquire, cb_release
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
 jmp .bad
.ping:
 mov word [ebp+28],1
 mov word [ebp+16],0xFD09
 jmp .done
.probe:
 call cb_probe_count
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
