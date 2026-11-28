; Optional diagnostic JLM: Pentium II/III Intel only, never loaded by baseline.
; Read-only fixed MSR whitelist. No arbitrary MSR, no storage or VGA access.
cpu 386
bits 32
%define DEVICE 0xFD05
%define EAX_OFF 28
%define EBX_OFF 16
%define EDI_OFF 0
%define ES_OFF 60
%define FLAGS_OFF 48
section .data align=4
global _ddb
_ddb:
 dd 0
 dw 0x0100,DEVICE
 db 1,0
 dw 0
 db "CACHE   "
 dd 0x80000000,control,dispatch,0
 times 7 dd 0
 dd 80
 times 3 dd 0
section .text
control:
 clc
 ret
global DllMain
DllMain:
 mov eax,1
 ret 12
dispatch:
 int 0x20
 dw 13,1
 and dword [ebp+FLAGS_OFF],~1
 cmp word [ebp+EAX_OFF],0
 je .ping
 cmp word [ebp+EAX_OFF],1
 jne .unsupported
 pushad
 movzx edi,word [ebp+ES_OFF]
 shl edi,4
 movzx eax,word [ebp+EDI_OFF]
 add edi,eax
 cmp edi,0x500
 jb .bad
 cmp edi,0xA0000-160
 ja .bad
 ; CPU ID flag toggling precedes CPUID, restore original flags.
 pushfd
 pop eax
 mov ecx,eax
 xor eax,0x200000
 push eax
 popfd
 pushfd
 pop eax
 push ecx
 popfd
 xor eax,ecx
 test eax,0x200000
 jz .no_cpu
 ; Explicitly isolated later-CPU instructions; baseline JLMs unchanged.
 cpu p3
 xor eax,eax
 cpuid
 cmp eax,1
 jb .no_cpu
 cmp ebx,0x756e6547
 jne .no_cpu
 cmp edx,0x49656e69
 jne .no_cpu
 cmp ecx,0x6c65746e
 jne .no_cpu
 mov eax,1
 cpuid
 mov ecx,eax
 and ecx,0x0ff0
 ; whitelist family 6 models 5,6,7,8,A,B (P2/P3 era)
 cmp ecx,0x650
 je .model_ok
 cmp ecx,0x660
 je .model_ok
 cmp ecx,0x670
 je .model_ok
 cmp ecx,0x680
 je .model_ok
 cmp ecx,0x6a0
 je .model_ok
 cmp ecx,0x6b0
 jne .no_cpu
.model_ok:
 mov ecx,edx
 and ecx,0x1020
 cmp ecx,0x1020
 jne .no_cpu
 ; Output 160 bytes: version, signature, features, count,
 ; MTRRCAP, DEF_TYPE, eight variable base/mask pairs.
 ; Zero all output first, no direction flag changes leak.
 pushfd
 cld
 push edi
 push eax
 push edx
 xor eax,eax
 mov ecx,40
 rep stosd
 pop edx
 pop eax
 pop edi
 popfd
 mov dword [edi],0x10000
 mov [edi+4],eax
 mov [edi+8],edx
 mov ecx,0xfe
 rdmsr
 mov [edi+16],eax
 mov [edi+20],edx
 and eax,0xff
 cmp eax,8
 jbe .count_ok
 mov eax,8
.count_ok:
 mov [edi+12],eax
 mov esi,eax
 mov ecx,0x2ff
 rdmsr
 mov [edi+24],eax
 mov [edi+28],edx
 add edi,32
 mov ecx,0x200
.loop:
 test esi,esi
 jz .ok
 rdmsr
 mov [edi],eax
 mov [edi+4],edx
 inc ecx
 rdmsr
 mov [edi+8],eax
 mov [edi+12],edx
 inc ecx
 add edi,16
 dec esi
 jmp .loop
.ok:
 popad
 mov word [ebp+EAX_OFF],0
 ret
.bad:
 popad
 mov word [ebp+EAX_OFF],1
 ret
.no_cpu:
 popad
.unsupported:
 mov word [ebp+EAX_OFF],6
 ret
.ping:
 mov word [ebp+EAX_OFF],1
 mov word [ebp+EBX_OFF],DEVICE
 ret
