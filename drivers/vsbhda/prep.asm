bits 16
org 100h
    xor di, di
    mov es, di
    mov ax, 1684h
    mov bx, 0fd01h
    int 2fh
    mov [entry], di
    mov [entry+2], es
    mov ax, es
    or ax, di
    jz fail
    mov ax, 002bh
    call far [entry]
    test ax, ax
    jnz fail
    mov dx, ready
    mov ah, 09h
    int 21h
    mov ax, 4c00h
    int 21h
fail:
    mov dx, error
    mov ah, 09h
    int 21h
    mov ax, 4c01h
    int 21h
entry dd 0
ready db 'M5451 prepared for VSBHDA.',13,10,'$'
error db 'Prepare failed. Check FERRIC.JLM version, fresh boot and M5451 hardware.',13,10,'$'
