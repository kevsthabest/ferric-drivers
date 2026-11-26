; Observe the reset byte exactly as VSBHDA's IsInstalled reads it.
; Run only before VSBHDA starts. Supply P to probe through a 32-bit DPMI client.
cpu 386
bits 16
org 100h
    mov sp, stack_top
    mov bx, (stack_top - $$ + 100h + 15) / 16
    mov ah, 4ah
    int 21h
    jc failed
    mov si, 81h
    movzx cx, byte [80h]
scan_argument:
    jcxz probe
    lodsb
    and al, 0dfh
    cmp al, 'P'
    je open_dpmi
    loop scan_argument
    jmp probe
open_dpmi:
    mov ax, 1687h
    int 2fh
    test ax, ax
    jnz failed
    test bl, 1
    jz failed
    mov [entry], di
    mov [entry+2], es
    test si, si
    jz switch
    mov bx, si
    mov ah, 48h
    int 21h
    jc failed
    mov es, ax
switch:
    mov ax, 1
    call far [entry]
    jc failed
probe:
    mov dx, 226h
    mov al, 1
    out dx, al
    in al, dx
    xor al, al
    out dx, al
    mov dx, 22ah
    in al, dx
    mov bl, al
    shr al, 4
    call digit
    mov al, bl
    and al, 15
    call digit
    mov dl, 13
    call emit
    mov dl, 10
    call emit
    mov ax, 4c00h
    int 21h
digit:
    movzx si, al
    mov dl, [digits+si]
emit:
    mov ah, 02h
    int 21h
    ret
failed:
    mov dl, '?'
    call emit
    mov ax, 4c01h
    int 21h
digits db '0123456789ABCDEF'
entry dd 0
times 256 db 0
stack_top:
