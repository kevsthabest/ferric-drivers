; Install the real-mode stub only for an already connected resident.
; No DMA allocation or connection authorization in this loader.
cpu 386
bits 16
org 0x100
start:
 mov sp,stack_top
 push cs
 pop ds
 push cs
 pop es
 mov bx,(end_program-$$+0x100+15)/16
 mov ah,0x4a
 int 0x21
 jc .allocfail
 mov ax,0x1684
 mov bx,0xfd0a
 xor di,di
 mov es,di
 int 0x2f
 mov [entry],di
 mov [entry+2],es
 mov ax,es
 or ax,di
 jz .missing
 mov ax,0x20
 call far [entry]
 jc .invalid
 test ax,ax
 jz .invalid
 cmp ax,512
 ja .invalid
 test cx,cx
 jz .invalid
 cmp cx,32
 ja .invalid
 mov bx,32
 mov ah,0x48
 int 0x21
 jc .allocfail
 mov [stubseg],ax
 mov es,ax
 xor di,di
 mov cx,512
 mov ax,0x21
 call far [entry]
 jc .invalid_free
 test ax,ax
 jz .invalid_free
 mov es,[stubseg]
 mov ax,[stubseg]
 mov [es:0x0c],ax
 mov bx,ax
 mov ax,0x22
 call far [entry]
 jc .quarantine
 test ax,ax
 jz .resident
 cmp ax,7
 je .quarantine
 mov [status],ax
 jmp .free
.resident:
 mov word [status],0
 call report
 mov dx,(end_program-$$+0x100+15)/16
 mov ax,0x3100
 int 0x21
.quarantine:
 mov word [status],7
 call report
 mov dx,(end_program-$$+0x100+15)/16
 mov ax,0x3107
 int 0x21
.invalid_free:
 mov word [status],1
.free:
 mov es,[stubseg]
 mov ah,0x49
 int 0x21
 jmp .exit
.invalid:
 mov word [status],1
 jmp .exit
.allocfail:
 mov word [status],3
 jmp .exit
.missing:
 mov word [status],9
.exit:
 call report
 mov ax,[status]
 mov ah,0x4c
 int 0x21
report:
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
 ret
entry: dd 0
stubseg: dw 0
status: dw 0
msg: db 'ATHINIT: status=$'
crlf: db 13,10,'$'
align 2
times 512 db 0
stack_top:
end_program:
