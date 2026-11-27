; Resident resource broker, Jemm DDB FD07.
cpu 386
bits 32
extern res_version, res_register, res_unregister, res_claim, res_release, res_can_unload
section .data align=4
align 4
services:
    dd res_version, register, unregister, claim, release
global _ddb
_ddb:
    dd 0
    dw 0x0100, 0xFD07
    db 1, 0
    dw 0
    db "FRES    "
    dd 0x80000000
    dd control
    dd 0, 0, 0, 0, 0
    dd services, 5
    dd 0, 0, 80, 0, 0, 0
section .text align=4
control:
    clc
    ret
; Preserve original IF, pass cdecl parameters on a conventional stack.
%macro WRAP1 2
%1:
    pushfd
    cli
    push dword [esp+8]
    call %2
    add esp, 4
    popfd
    ret
%endmacro
%macro WRAP4 2
%1:
    pushfd
    cli
    times 4 push dword [esp+20]
    call %2
    add esp, 16
    popfd
    ret
%endmacro
WRAP1 register, res_register
WRAP1 unregister, res_unregister
WRAP4 claim, res_claim
WRAP4 release, res_release
global DllMain
DllMain:
    cmp dword [esp+8], 0
    jne .attach
    pushfd
    cli
    call res_can_unload
    popfd
    ret 12
.attach:
    mov eax, 1
    ret 12
