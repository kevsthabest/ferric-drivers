; Remove packet bridge, shut down DMA, release CardBus consumer.
; DOS TSR allocations are intentionally retained until reboot.
cpu 386
bits 16
org 0x100
 push cs
 pop ds
 mov ax,0x1684
 mov bx,0xfd0a
 xor di,di
 mov es,di
 int 0x2f
 mov [entry],di
 mov [entry+2],es
 mov ax,es
 or ax,di
 jnz .found
 mov ax,9
 jmp .report
.found:
 mov ax,0x0d
 call far [entry]
 jnc .report
 mov ax,7
.report:
 mov [status],ax
 mov dx,msg
 mov ah,9
 int 0x21
 mov dl,[status]
 add dl,'0'
 mov ah,2
 int 0x21
 mov dx,crlf
 mov ah,9
 int 0x21
 mov ax,[status]
 mov ah,0x4c
 int 0x21
entry: dd 0
status: dw 0
msg: db 'ATHSTOP: status=$'
crlf: db 13,10,'$'
